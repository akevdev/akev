//! On-disk persistence of the CA: `ca.pem` + `ca.key` in the data directory.
//!
//! * Both files are written atomically (temp file in the same directory → fsync → rename),
//!   key first, so a crash never leaves a truncated file behind. A crash between the two
//!   renames leaves a key/cert mismatch, which the next start detects and repairs.
//! * The key is created with mode `0600` on Unix (and an existing key with looser
//!   permissions is tightened on load). On Windows it gets an explicit, protected DACL that
//!   grants the current user full control and nobody else anything (re-applied on load), so
//!   it never inherits a looser ACL from wherever the data directory lives.
//! * Files are only replaced automatically when they are missing, unparseable, mismatched or
//!   not yet valid. A CA that expires soon or has expired is kept (devices trust it; the user
//!   regenerates it, see [`crate::CaInfo::expiry`]). Whenever files are replaced — here, by
//!   [`regenerate`] or by [`save`] — the previous ones are kept as `ca.pem.bak` /
//!   `ca.key.bak` and restored if the new ones can't be written. I/O errors other than "not
//!   found" (e.g. permission denied) are returned instead of regenerating, so a transient
//!   problem never destroys a CA that devices already trust (an HTTP Toolkit weakness).

use std::ffi::OsString;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use time::OffsetDateTime;

use crate::{CaError, CertificateAuthority, Result, info::rfc3339};

/// File name of the CA certificate inside the data directory.
pub const CA_CERT_FILE: &str = "ca.pem";
/// File name of the CA private key (PKCS#8 PEM) inside the data directory.
pub const CA_KEY_FILE: &str = "ca.key";

struct Paths {
    cert: PathBuf,
    key: PathBuf,
}

impl Paths {
    fn new(dir: &Path) -> Self {
        Paths {
            cert: dir.join(CA_CERT_FILE),
            key: dir.join(CA_KEY_FILE),
        }
    }
}

pub(crate) fn load_or_generate(dir: &Path) -> Result<CertificateAuthority> {
    ensure_dir(dir)?;
    let paths = Paths::new(dir);
    let cert = read_if_exists(&paths.cert)?;
    let key = read_if_exists(&paths.key)?;

    let reason = match (cert, key) {
        (None, None) => {
            tracing::info!(dir = %dir.display(), "no CA certificate found; generating a new one");
            return generate_and_persist(&paths);
        }
        (Some(_), None) => format!("{CA_KEY_FILE} is missing"),
        (None, Some(_)) => format!("{CA_CERT_FILE} is missing"),
        (Some(cert), Some(key)) => match parse_pair(&cert, &key) {
            Ok(ca) => match renewal_reason(&ca, OffsetDateTime::now_utc()) {
                None => {
                    tighten_key_permissions(&paths.key);
                    if let Some(warning) = ca.info().expiry_warning("in Settings → Certificates")
                    {
                        tracing::warn!(dir = %dir.display(), "{warning}");
                    }
                    return Ok(ca);
                }
                Some(reason) => reason,
            },
            Err(e) => e.to_string(),
        },
    };

    tracing::warn!(
        dir = %dir.display(),
        %reason,
        "regenerating the CA certificate; previous files are kept as *.bak and clients must trust the new CA"
    );
    generate_and_persist(&paths)
}

pub(crate) fn regenerate(dir: &Path) -> Result<CertificateAuthority> {
    ensure_dir(dir)?;
    generate_and_persist(&Paths::new(dir))
}

pub(crate) fn save(ca: &CertificateAuthority, dir: &Path) -> Result<()> {
    ensure_dir(dir)?;
    replace_with_backup(ca, &Paths::new(dir))
}

/// Generate a CA in memory (nothing on disk changes if that fails), then replace the files.
fn generate_and_persist(paths: &Paths) -> Result<CertificateAuthority> {
    let ca = CertificateAuthority::generate()?;
    replace_with_backup(&ca, paths)?;
    tracing::info!(
        subject = %ca.details.subject,
        sha256 = %ca.details.sha256_fingerprint,
        not_after = %rfc3339(ca.details.not_after),
        "generated new CA certificate"
    );
    Ok(ca)
}

/// Move the current files aside as `*.bak`, write `ca`'s, and put the backups back if that
/// fails (so a full disk never leaves the data directory without a CA).
fn replace_with_backup(ca: &CertificateAuthority, paths: &Paths) -> Result<()> {
    let moved = backup(paths)?;
    if let Err(e) = persist(ca, paths) {
        for (path, bak) in moved {
            if let Err(restore) = fs::rename(&bak, &path) {
                tracing::error!(
                    path = %path.display(),
                    error = %restore,
                    "could not restore the previous CA file from its backup"
                );
            }
        }
        return Err(e);
    }
    Ok(())
}

fn persist(ca: &CertificateAuthority, paths: &Paths) -> Result<()> {
    write_atomic(&paths.key, ca.key_pem().as_bytes(), true)?;
    write_atomic(&paths.cert, ca.chain_pem().as_bytes(), false)?;
    Ok(())
}

fn parse_pair(cert: &[u8], key: &[u8]) -> Result<CertificateAuthority> {
    let cert = std::str::from_utf8(cert)
        .map_err(|_| CaError::Invalid(format!("{CA_CERT_FILE} is not valid UTF-8 PEM")))?;
    let key = std::str::from_utf8(key)
        .map_err(|_| CaError::Invalid(format!("{CA_KEY_FILE} is not valid UTF-8 PEM")))?;
    CertificateAuthority::from_pem_allow_expired(cert, key)
}

/// Why a parseable CA should nevertheless be replaced automatically, if at all. Expiry is
/// never a reason: devices trust the CA, so the user decides when to replace it.
pub(crate) fn renewal_reason(ca: &CertificateAuthority, now: OffsetDateTime) -> Option<String> {
    let details = &ca.details;
    if now < details.not_before {
        return Some(format!(
            "the CA certificate is not valid until {} (was the clock wrong when it was created?)",
            rfc3339(details.not_before)
        ));
    }
    None
}

fn ensure_dir(dir: &Path) -> io::Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    if let Some(parent) = dir.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    #[cfg(unix)]
    let builder = {
        use std::os::unix::fs::DirBuilderExt as _;
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder
    };
    #[cfg(not(unix))]
    let builder = fs::DirBuilder::new();
    match builder.create(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists && dir.is_dir() => Ok(()),
        Err(e) => Err(e),
    }
}

/// Read a file, mapping only "not found" to `None`; every other error is surfaced.
fn read_if_exists(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(CaError::Io(io::Error::new(
            e.kind(),
            format!("cannot read {}: {e}", path.display()),
        ))),
    }
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name: OsString = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Move existing CA files aside as `*.bak` (replacing older backups). Returns the
/// `(file, backup)` pairs that were moved.
fn backup(paths: &Paths) -> Result<Vec<(PathBuf, PathBuf)>> {
    let mut moved = Vec::new();
    for (path, private) in [(&paths.cert, false), (&paths.key, true)] {
        let bak = with_suffix(path, ".bak");
        match fs::rename(path, &bak) {
            Ok(()) => {
                moved.push((path.clone(), bak.clone()));
                if private {
                    set_private_permissions(&bak)?;
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => {
                for (path, bak) in moved {
                    let _ = fs::rename(bak, path);
                }
                return Err(CaError::Io(io::Error::new(
                    e.kind(),
                    format!(
                        "cannot back up {} to {}: {e}",
                        path.display(),
                        bak.display()
                    ),
                )));
            }
        }
    }
    Ok(moved)
}

/// Write `contents` to `path` atomically. `private` files get mode 0600 on Unix.
pub(crate) fn write_atomic(path: &Path, contents: &[u8], private: bool) -> io::Result<()> {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let file_name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
    let mut nonce = [0u8; 8];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut nonce)
        .map_err(|_| io::Error::other("system random number generator failed"))?;
    let mut tmp_name = OsString::from(".");
    tmp_name.push(file_name);
    tmp_name.push(format!(".{:016x}.tmp", u64::from_ne_bytes(nonce)));
    let tmp = dir.join(tmp_name);

    let result = (|| {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(if private { 0o600 } else { 0o644 });
        }
        let mut file = options.open(&tmp)?;
        if private {
            // The umask can only remove bits; make the mode exact regardless.
            set_private_permissions(&tmp)?;
        }
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result?;
    sync_dir(dir);
    Ok(())
}

#[cfg(unix)]
fn set_private_permissions(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(windows)]
fn set_private_permissions(path: &Path) -> io::Result<()> {
    windows_acl::restrict_to_current_user(path)
}

#[cfg(not(any(unix, windows)))]
fn set_private_permissions(_path: &Path) -> io::Result<()> {
    Ok(())
}

/// Best-effort: make the renames durable.
fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(handle) = fs::File::open(dir) {
        let _ = handle.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

/// Fix a key file that is readable by group/others (e.g. copied in by the user).
fn tighten_key_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let too_open = fs::metadata(path).is_ok_and(|meta| meta.permissions().mode() & 0o077 != 0);
        if too_open {
            match set_private_permissions(path) {
                Ok(()) => {
                    tracing::warn!(path = %path.display(), "CA key was readable by other users; permissions reset to 0600")
                }
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "CA key is readable by other users and its permissions could not be fixed")
                }
            }
        }
    }
    #[cfg(windows)]
    if let Err(e) = set_private_permissions(path) {
        tracing::warn!(path = %path.display(), error = %e, "could not restrict the CA key to the current user");
    }
    #[cfg(not(any(unix, windows)))]
    let _ = path;
}

/// The Windows equivalent of mode `0600`.
#[cfg(windows)]
mod windows_acl {
    use std::io;
    use std::os::windows::ffi::OsStrExt as _;
    use std::path::Path;

    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE};
    use windows_sys::Win32::Security::Authorization::{SE_FILE_OBJECT, SetNamedSecurityInfoW};
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACL, ACL_REVISION, AddAccessAllowedAce, DACL_SECURITY_INFORMATION,
        GetLengthSid, GetTokenInformation, InitializeAcl, PROTECTED_DACL_SECURITY_INFORMATION,
        TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    /// Closes the process token.
    struct Token(HANDLE);

    impl Drop for Token {
        fn drop(&mut self) {
            // SAFETY: the handle came from a successful `OpenProcessToken`.
            unsafe { CloseHandle(self.0) };
        }
    }

    /// Replace `path`'s DACL with one entry granting the current user full control, and stop
    /// it inheriting entries from its directory (`PROTECTED_DACL_SECURITY_INFORMATION`).
    pub(super) fn restrict_to_current_user(path: &Path) -> io::Result<()> {
        let mut handle: HANDLE = std::ptr::null_mut();
        // SAFETY: `handle` is a valid out pointer; the current-process pseudo handle needs no
        // closing.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut handle) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let token = Token(handle);

        // The token's user SID (size query first).
        let mut len = 0u32;
        // SAFETY: a null buffer of length 0 only asks for the required size.
        unsafe { GetTokenInformation(token.0, TokenUser, std::ptr::null_mut(), 0, &mut len) };
        if len == 0 {
            return Err(io::Error::last_os_error());
        }
        // u64 elements keep the buffer aligned for TOKEN_USER.
        let mut user_buf = vec![0u64; (len as usize).div_ceil(8)];
        // SAFETY: the buffer is at least `len` bytes.
        let ok = unsafe {
            GetTokenInformation(
                token.0,
                TokenUser,
                user_buf.as_mut_ptr().cast(),
                len,
                &mut len,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: GetTokenInformation wrote a TOKEN_USER (whose SID points into the same
        // buffer, which outlives every use below).
        let sid = unsafe { (*user_buf.as_ptr().cast::<TOKEN_USER>()).User.Sid };
        // SAFETY: `sid` is a valid SID.
        let sid_len = unsafe { GetLengthSid(sid) } as usize;

        // ACL header + one access-allowed ACE (whose `SidStart` is the first SID word),
        // rounded up to a DWORD boundary.
        let acl_len = (size_of::<ACL>() + size_of::<ACCESS_ALLOWED_ACE>() - size_of::<u32>()
            + sid_len)
            .next_multiple_of(4);
        let acl_len_u32 = u32::try_from(acl_len).map_err(|_| io::Error::other("ACL too large"))?;
        let mut acl_buf = vec![0u32; acl_len / 4];
        let acl = acl_buf.as_mut_ptr().cast::<ACL>();
        // SAFETY: `acl` points to `acl_len` writable, DWORD-aligned bytes.
        if unsafe { InitializeAcl(acl, acl_len_u32, ACL_REVISION) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the ACL was sized for exactly this ACE.
        if unsafe { AddAccessAllowedAce(acl, ACL_REVISION, FILE_ALL_ACCESS, sid) } == 0 {
            return Err(io::Error::last_os_error());
        }

        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: `wide` is NUL-terminated; `acl` is a valid ACL; owner, group and SACL are
        // left unchanged (null).
        let status = unsafe {
            SetNamedSecurityInfoW(
                wide.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                acl,
                std::ptr::null(),
            )
        };
        if status != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
        Ok(())
    }
}
