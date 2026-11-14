//! `system-proxy`: point the operating system's proxy setting at Akev.
//!
//! Backends (chosen at runtime from [`HostEnv`]):
//! * **GNOME** and GNOME-based desktops: `gsettings` keys under `org.gnome.system.proxy`;
//! * **KDE Plasma**: `kioslaverc` `[Proxy Settings]`, written with `kwriteconfig6`/`5`;
//! * **macOS**: `networksetup`, for every enabled network service;
//! * **Windows**: the per-user WinINet values under
//!   `HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings`, followed by a
//!   settings-changed notification so running apps pick them up.
//!
//! Crash safety: before anything is changed, the previous value of every setting Akev is
//! about to touch is written to a backup file tagged with the owning process:
//! `<data_dir>/system-proxy-backup.json` for the platform data directory, and a file in the
//! per-user recovery directory for any other data directory (ephemeral temp dirs and custom
//! `--data-dir`s may be gone by the next start; see [`crate::journal`]). Deactivation (and
//! shutdown) restores those values and deletes the file. A backup whose owner is gone (Akev
//! crashed or was killed) is restored when the registry is built, on the next listing and
//! before the next activation, whatever data directory the dead run used. A backup owned by
//! another *running* Akev instance is never touched.
//!
//! `gsettings set` exits 0 even when dconf can't commit the change (no D-Bus session), so
//! GNOME changes are read back and only count once they stuck.
//!
//! Only the proxy is changed. Trusting the CA system-wide is left to the developer: the
//! metadata carries per-OS instructions (and the matching removal commands).

use crate::host::{HostEnv, Os, is_executable};
use crate::journal::{
    dedup, instance_id, journal_file, own_process_name, same_program, scan_journals,
};
use crate::runner::CommandSpec;
use crate::scripts::posix_quote;
use crate::util::{
    Descriptor, PROBE_TTL, ProbeCache, Sys, atomic_write, host_for_url, local_proxy_host, tags,
};
use crate::{InterceptContext, InterceptError, Interceptor, Result};
use akev_core::interceptor::{ActivationResult, InterceptorCategory, InterceptorInfo};
use async_trait::async_trait;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// File (in the data directory) holding the settings to restore.
pub const BACKUP_FILE: &str = "system-proxy-backup.json";
const BACKUP_VERSION: u32 = 1;
/// Timeout for one settings command (`gsettings`, `networksetup`...).
const CMD_TIMEOUT: Duration = Duration::from_secs(8);
/// Minimum delay between two automatic recovery attempts for the same backup.
const RECOVERY_RETRY: Duration = Duration::from_secs(60);
/// Delay before reading GNOME settings back a second time.
const GSETTINGS_RECHECK: Duration = if cfg!(test) {
    Duration::from_millis(10)
} else {
    Duration::from_millis(300)
};
/// Hosts that keep bypassing the proxy (Akev's own UI and API live on loopback).
pub(crate) const LOOPBACK_BYPASS: &[&str] = &["localhost", "127.0.0.0/8", "::1"];

/// `<data_dir>/system-proxy-backup.json`: where the backup of the platform data directory
/// lives. Backups of other data directories are kept in the per-user recovery directory
/// instead (see [`crate::journal`]).
#[cfg(test)]
pub(crate) fn backup_path(data_dir: &Path) -> PathBuf {
    data_dir.join(BACKUP_FILE)
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ------------------------------------------------------------------------------ snapshots

/// One `gsettings` value, as GVariant text (`'manual'`, `8000`, `['localhost']`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct GsettingsValue {
    pub schema: String,
    pub key: String,
    pub value: String,
}

/// One `kioslaverc` `[Proxy Settings]` entry (`None` = the key was absent).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct KdeValue {
    pub key: String,
    pub value: Option<String>,
}

/// One `networksetup -get(secure)webproxy` result.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MacProxy {
    pub enabled: bool,
    pub server: String,
    pub port: u16,
}

/// `networksetup -getautoproxyurl` result.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MacAutoProxy {
    pub enabled: bool,
    pub url: String,
}

/// Proxy settings of one macOS network service.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MacService {
    pub name: String,
    pub web: MacProxy,
    pub secure: MacProxy,
    pub auto_proxy: MacAutoProxy,
}

/// The WinINet values Akev touches (`None` = value absent).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WinInetSettings {
    /// `ProxyEnable` (DWORD).
    pub proxy_enable: Option<u32>,
    /// `ProxyServer` (e.g. `127.0.0.1:8000` or `http=...;https=...`).
    pub proxy_server: Option<String>,
    /// `ProxyOverride` (bypass list, `;`-separated).
    pub proxy_override: Option<String>,
    /// `AutoConfigURL` (PAC; takes precedence over the manual proxy, so it's removed while
    /// Akev is active).
    pub auto_config_url: Option<String>,
}

/// Previous settings of one backend.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "backend", rename_all = "kebab-case")]
pub(crate) enum Snapshot {
    Gnome {
        values: Vec<GsettingsValue>,
    },
    Kde {
        values: Vec<KdeValue>,
    },
    #[serde(rename = "macos")]
    MacOs {
        services: Vec<MacService>,
    },
    Windows {
        settings: WinInetSettings,
    },
}

impl Snapshot {
    fn kind(&self) -> &'static str {
        match self {
            Snapshot::Gnome { .. } => "gnome",
            Snapshot::Kde { .. } => "kde",
            Snapshot::MacOs { .. } => "macos",
            Snapshot::Windows { .. } => "windows",
        }
    }
}

/// Contents of the backup file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Backup {
    pub version: u32,
    /// [`instance_id`] of the owning process.
    pub instance: String,
    /// Owning process id.
    pub pid: u32,
    /// Owning process name as seen in the process table (guards against pid reuse).
    pub process_name: String,
    /// Unix seconds.
    pub created_at: u64,
    /// `host:port` the system proxy was pointed at.
    pub proxy: String,
    /// Previous settings, in the order they were captured.
    pub snapshots: Vec<Snapshot>,
}

pub(crate) enum BackupState {
    None,
    Valid(Backup),
    Corrupt(String),
}

/// Read the backup file at `path`.
pub(crate) fn read_backup(path: &Path) -> BackupState {
    match std::fs::read(path) {
        Ok(bytes) => match serde_json::from_slice::<Backup>(&bytes) {
            Ok(b) => BackupState::Valid(b),
            Err(e) => BackupState::Corrupt(e.to_string()),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => BackupState::None,
        Err(e) => BackupState::Corrupt(e.to_string()),
    }
}

fn write_backup(path: &Path, backup: &Backup) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(backup).map_err(io::Error::other)?;
    atomic_write(path, &bytes)
}

fn remove_backup(path: &Path) {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => {
            tracing::warn!("couldn't remove the system proxy backup: {e}");
        }
        _ => {}
    }
}

/// Move an unreadable backup aside (keeping it for manual inspection).
fn quarantine_backup(path: &Path, reason: &str) {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| BACKUP_FILE.to_string());
    let aside = path.with_file_name(format!("{name}.corrupt-{}", unix_now()));
    tracing::warn!(
        "unreadable system proxy backup ({reason}); moved to {} — check your proxy settings",
        aside.display()
    );
    if let Err(e) = std::fs::rename(path, &aside) {
        tracing::warn!("couldn't move the unreadable backup aside: {e}");
    }
}

// ------------------------------------------------------------------------------- backends

/// A settings store Akev knows how to change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Backend {
    Gnome {
        gsettings: PathBuf,
    },
    Kde {
        kwriteconfig: PathBuf,
        config_file: PathBuf,
        dbus_send: Option<PathBuf>,
    },
    MacOs {
        networksetup: PathBuf,
    },
    Windows,
}

impl Backend {
    fn id(&self) -> &'static str {
        match self {
            Backend::Gnome { .. } => "gnome",
            Backend::Kde { .. } => "kde",
            Backend::MacOs { .. } => "macos",
            Backend::Windows => "windows",
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Backend::Gnome { .. } => "GNOME",
            Backend::Kde { .. } => "KDE Plasma",
            Backend::MacOs { .. } => "macOS",
            Backend::Windows => "Windows",
        }
    }
}

/// `<XDG_CONFIG_HOME or ~/.config>/kioslaverc`.
fn kde_config_file(env: &HostEnv) -> Option<PathBuf> {
    env.var_path("XDG_CONFIG_HOME")
        .or_else(|| env.home_path(".config"))
        .map(|d| d.join("kioslaverc"))
}

/// Is the `org.gnome.system.proxy` schema installed (in any `XDG_DATA_DIRS` entry)?
fn gnome_schema_installed(env: &HostEnv) -> bool {
    let dirs: Vec<PathBuf> = match env.var("XDG_DATA_DIRS") {
        Some(v) => v
            .split(':')
            .filter(|d| !d.is_empty())
            .map(PathBuf::from)
            .collect(),
        None => vec![env.sys_path("/usr/local/share"), env.sys_path("/usr/share")],
    };
    dirs.iter().any(|d| {
        d.join("glib-2.0/schemas/org.gnome.system.proxy.gschema.xml")
            .is_file()
    })
}

fn gnome_backend(env: &HostEnv) -> Option<Backend> {
    let gsettings = env.find_in_path("gsettings")?;
    gnome_schema_installed(env).then_some(Backend::Gnome { gsettings })
}

fn kde_backend(env: &HostEnv) -> Option<Backend> {
    Some(Backend::Kde {
        kwriteconfig: env.find_any_in_path(&["kwriteconfig6", "kwriteconfig5"])?,
        config_file: kde_config_file(env)?,
        dbus_send: env.find_in_path("dbus-send"),
    })
}

fn macos_backend(env: &HostEnv) -> Option<Backend> {
    let networksetup = env.sys_path("/usr/sbin/networksetup");
    is_executable(&networksetup).then_some(Backend::MacOs { networksetup })
}

/// Backends to configure on this machine.
pub(crate) fn detect_backends(env: &HostEnv) -> Vec<Backend> {
    match env.os {
        Os::Linux => {
            let kde = env.desktop().split(':').any(|d| d.trim() == "KDE");
            let mut out = Vec::new();
            if kde && let Some(b) = kde_backend(env) {
                out.push(b);
            }
            // GTK apps, Chromium and Firefox ("use system proxy settings") read GSettings on
            // GNOME-like desktops; setting it elsewhere is harmless and restored too.
            if let Some(b) = gnome_backend(env) {
                out.push(b);
            }
            out
        }
        Os::MacOs => macos_backend(env).into_iter().collect(),
        Os::Windows => vec![Backend::Windows],
        Os::Other => Vec::new(),
    }
}

/// The backend able to restore `snap`, independent of the current desktop session (crash
/// recovery may run from a different session than the one that changed the settings).
fn backend_for_snapshot(env: &HostEnv, snap: &Snapshot) -> Option<Backend> {
    match snap {
        Snapshot::Gnome { .. } => env
            .find_in_path("gsettings")
            .map(|gsettings| Backend::Gnome { gsettings }),
        Snapshot::Kde { .. } => kde_backend(env),
        Snapshot::MacOs { .. } => macos_backend(env),
        Snapshot::Windows { .. } => (env.os == Os::Windows).then_some(Backend::Windows),
    }
}

// ---------------------------------------------------------------------------------- GNOME

/// Keys captured and changed, `(schema, key)`.
const GNOME_KEYS: &[(&str, &str)] = &[
    ("org.gnome.system.proxy", "mode"),
    ("org.gnome.system.proxy", "ignore-hosts"),
    ("org.gnome.system.proxy.http", "host"),
    ("org.gnome.system.proxy.http", "port"),
    ("org.gnome.system.proxy.https", "host"),
    ("org.gnome.system.proxy.https", "port"),
];

/// A GVariant string literal.
pub(crate) fn gvariant_str(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// Values to set, in order (the mode last, so nothing sees "manual" with stale hosts).
pub(crate) fn gnome_target(host: &str, port: u16) -> Vec<GsettingsValue> {
    let v = |schema: &str, key: &str, value: String| GsettingsValue {
        schema: schema.into(),
        key: key.into(),
        value,
    };
    let bypass = LOOPBACK_BYPASS
        .iter()
        .map(|h| gvariant_str(h))
        .collect::<Vec<_>>()
        .join(", ");
    vec![
        v("org.gnome.system.proxy.http", "host", gvariant_str(host)),
        v("org.gnome.system.proxy.http", "port", port.to_string()),
        v("org.gnome.system.proxy.https", "host", gvariant_str(host)),
        v("org.gnome.system.proxy.https", "port", port.to_string()),
        v(
            "org.gnome.system.proxy",
            "ignore-hosts",
            format!("[{bypass}]"),
        ),
        v("org.gnome.system.proxy", "mode", gvariant_str("manual")),
    ]
}

/// Unescaped contents of a GVariant string literal (`'it\'s'` or `"it's"`).
fn gvariant_string_contents(text: &str) -> Option<String> {
    let quote = text.chars().next().filter(|c| *c == '\'' || *c == '"')?;
    let inner = text.strip_prefix(quote)?.strip_suffix(quote)?;
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            out.push(chars.next()?);
        } else {
            out.push(c);
        }
    }
    Some(out)
}

/// Do two GVariant texts denote the same value? (`gsettings get` prints a string containing a
/// single quote with double quotes, so string literals are compared by their contents.)
pub(crate) fn gvariant_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim(), b.trim());
    a == b
        || matches!(
            (gvariant_string_contents(a), gvariant_string_contents(b)),
            (Some(x), Some(y)) if x == y
        )
}

/// `gsettings set` reports a change dconf couldn't commit only on stderr (exit code 0).
fn gsettings_commit_warning(out: &crate::runner::CommandOutput) -> Option<String> {
    let lower = out.stderr.to_ascii_lowercase();
    (lower.contains("failed to commit") || lower.contains("dconf-warning"))
        .then(|| out.error_summary())
}

// ------------------------------------------------------------------------------------ KDE

const KDE_GROUP: &str = "Proxy Settings";
/// Keys captured and changed.
const KDE_KEYS: &[&str] = &[
    "ProxyType",
    "httpProxy",
    "httpsProxy",
    "NoProxyFor",
    "ReversedException",
];

/// Values to set, in order (`ProxyType=1` = manual, last). KDE stores `scheme://host port`.
pub(crate) fn kde_target(host: &str, port: u16) -> Vec<(&'static str, String)> {
    let proxy = format!("http://{} {port}", host_for_url(host));
    vec![
        ("httpProxy", proxy.clone()),
        ("httpsProxy", proxy),
        ("NoProxyFor", "localhost,127.0.0.1,::1".to_string()),
        ("ReversedException", "false".to_string()),
        ("ProxyType", "1".to_string()),
    ]
}

/// Values of `keys` in `[group]` of a KConfig file (`key[$e]` variants included).
pub(crate) fn kconfig_values(content: &str, group: &str, keys: &[&str]) -> Vec<KdeValue> {
    let mut found: HashMap<String, String> = HashMap::new();
    let mut in_group = false;
    for line in content.lines().map(str::trim) {
        if line.starts_with('[') && line.ends_with(']') {
            in_group = &line[1..line.len() - 1] == group;
        } else if in_group
            && !line.starts_with('#')
            && let Some((k, v)) = line.split_once('=')
        {
            let k = k.trim();
            let k = k.split_once('[').map(|(base, _)| base).unwrap_or(k);
            found.insert(k.to_string(), v.trim().to_string());
        }
    }
    keys.iter()
        .map(|k| KdeValue {
            key: k.to_string(),
            value: found.get(*k).cloned(),
        })
        .collect()
}

// ---------------------------------------------------------------------------------- macOS

/// Enabled services from `networksetup -listallnetworkservices`.
pub(crate) fn parse_mac_services(out: &str) -> Vec<String> {
    out.lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty() && !l.contains("asterisk (*)") && !l.starts_with('*'))
        .map(String::from)
        .collect()
}

fn mac_field<'a>(out: &'a str, name: &str) -> Option<&'a str> {
    out.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        (k.trim() == name).then(|| v.trim())
    })
}

/// Parse `networksetup -getwebproxy <service>`.
pub(crate) fn parse_mac_proxy(out: &str) -> MacProxy {
    let server = mac_field(out, "Server").unwrap_or_default();
    MacProxy {
        enabled: mac_field(out, "Enabled").is_some_and(|v| v.eq_ignore_ascii_case("yes")),
        server: if server == "(null)" {
            String::new()
        } else {
            server.to_string()
        },
        port: mac_field(out, "Port")
            .and_then(|p| p.parse().ok())
            .unwrap_or(0),
    }
}

/// Parse `networksetup -getautoproxyurl <service>`.
pub(crate) fn parse_mac_auto_proxy(out: &str) -> MacAutoProxy {
    let url = mac_field(out, "URL").unwrap_or_default();
    MacAutoProxy {
        enabled: mac_field(out, "Enabled").is_some_and(|v| v.eq_ignore_ascii_case("yes")),
        url: if url == "(null)" {
            String::new()
        } else {
            url.to_string()
        },
    }
}

/// `networksetup` sometimes reports errors on stdout with exit code 0.
fn mac_output_error(out: &crate::runner::CommandOutput) -> Option<String> {
    let text = format!("{}\n{}", out.stdout, out.stderr);
    let bad = !out.success()
        || text.contains("** Error")
        || text.contains("is not a recognized network service")
        || text.contains("requires admin");
    bad.then(|| out.error_summary())
}

// -------------------------------------------------------------------------------- Windows

/// Access to the per-user WinINet settings (the real registry on Windows; a fake in tests).
pub(crate) trait InternetSettingsStore: Send + Sync + 'static {
    /// Read the current values.
    fn read(&self) -> io::Result<WinInetSettings>;
    /// Write all values (`None` deletes the value) and notify running apps.
    fn write(&self, settings: &WinInetSettings) -> io::Result<()>;
}

/// Store used where there is no WinINet.
#[cfg_attr(windows, allow(dead_code))]
pub(crate) struct UnsupportedStore;

impl InternetSettingsStore for UnsupportedStore {
    fn read(&self) -> io::Result<WinInetSettings> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "WinINet settings exist only on Windows",
        ))
    }
    fn write(&self, _settings: &WinInetSettings) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "WinINet settings exist only on Windows",
        ))
    }
}

fn platform_store() -> Arc<dyn InternetSettingsStore> {
    #[cfg(windows)]
    {
        Arc::new(win::RegistryStore)
    }
    #[cfg(not(windows))]
    {
        Arc::new(UnsupportedStore)
    }
}

/// The WinINet values that route everything through `host:port`.
pub(crate) fn windows_target(host: &str, port: u16) -> WinInetSettings {
    WinInetSettings {
        proxy_enable: Some(1),
        proxy_server: Some(format!("{}:{port}", host_for_url(host))),
        proxy_override: Some("localhost;127.*;[::1]".into()),
        auto_config_url: None,
    }
}

#[cfg(windows)]
mod win {
    //! Real registry access (HKCU, so no elevation is needed).

    use super::{InternetSettingsStore, WinInetSettings};
    use std::io;
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::Networking::WinInet::{
        INTERNET_OPTION_REFRESH, INTERNET_OPTION_SETTINGS_CHANGED, InternetSetOptionW,
    };
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_DWORD, REG_EXPAND_SZ, REG_SZ,
        RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    };

    const KEY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    fn os_err(rc: u32) -> io::Error {
        io::Error::from_raw_os_error(rc as i32)
    }

    struct Key(HKEY);

    impl Drop for Key {
        fn drop(&mut self) {
            // SAFETY: `self.0` is a key handle opened by `open` and closed exactly once.
            unsafe {
                RegCloseKey(self.0);
            }
        }
    }

    fn open(access: u32) -> io::Result<Key> {
        let path = wide(KEY_PATH);
        let mut hkey: HKEY = std::ptr::null_mut();
        // SAFETY: `path` is NUL-terminated and outlives the call; `hkey` is a valid out-pointer.
        let rc = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, access, &mut hkey) };
        if rc != ERROR_SUCCESS {
            return Err(os_err(rc));
        }
        Ok(Key(hkey))
    }

    fn query(key: &Key, name: &str) -> io::Result<Option<(u32, Vec<u8>)>> {
        let name = wide(name);
        let mut ty: u32 = 0;
        let mut len: u32 = 0;
        // SAFETY: valid key and NUL-terminated name; a null data pointer asks for the size.
        let rc = unsafe {
            RegQueryValueExW(
                key.0,
                name.as_ptr(),
                std::ptr::null(),
                &mut ty,
                std::ptr::null_mut(),
                &mut len,
            )
        };
        if rc == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        if rc != ERROR_SUCCESS {
            return Err(os_err(rc));
        }
        let mut buf = vec![0u8; len as usize];
        // SAFETY: `buf` has `len` writable bytes, which is what we pass.
        let rc = unsafe {
            RegQueryValueExW(
                key.0,
                name.as_ptr(),
                std::ptr::null(),
                &mut ty,
                buf.as_mut_ptr(),
                &mut len,
            )
        };
        if rc == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        if rc != ERROR_SUCCESS {
            return Err(os_err(rc));
        }
        buf.truncate(len as usize);
        Ok(Some((ty, buf)))
    }

    fn read_dword(key: &Key, name: &str) -> io::Result<Option<u32>> {
        Ok(query(key, name)?.and_then(|(ty, data)| {
            (ty == REG_DWORD && data.len() >= 4)
                .then(|| u32::from_le_bytes([data[0], data[1], data[2], data[3]]))
        }))
    }

    fn read_string(key: &Key, name: &str) -> io::Result<Option<String>> {
        Ok(query(key, name)?.and_then(|(ty, data)| {
            if ty != REG_SZ && ty != REG_EXPAND_SZ {
                return None;
            }
            let units: Vec<u16> = data
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            let end = units.iter().position(|u| *u == 0).unwrap_or(units.len());
            Some(String::from_utf16_lossy(&units[..end]))
        }))
    }

    fn set_dword(key: &Key, name: &str, value: u32) -> io::Result<()> {
        let name = wide(name);
        let data = value.to_le_bytes();
        // SAFETY: valid key, NUL-terminated name, 4 readable bytes.
        let rc = unsafe { RegSetValueExW(key.0, name.as_ptr(), 0, REG_DWORD, data.as_ptr(), 4) };
        if rc == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(os_err(rc))
        }
    }

    fn set_string(key: &Key, name: &str, value: &str) -> io::Result<()> {
        let name = wide(name);
        let data: Vec<u8> = wide(value).iter().flat_map(|u| u.to_le_bytes()).collect();
        let len =
            u32::try_from(data.len()).map_err(|_| io::Error::other("registry value too long"))?;
        // SAFETY: valid key, NUL-terminated name, `len` readable bytes (incl. the NUL).
        let rc = unsafe { RegSetValueExW(key.0, name.as_ptr(), 0, REG_SZ, data.as_ptr(), len) };
        if rc == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(os_err(rc))
        }
    }

    fn delete(key: &Key, name: &str) -> io::Result<()> {
        let name = wide(name);
        // SAFETY: valid key and NUL-terminated name.
        let rc = unsafe { RegDeleteValueW(key.0, name.as_ptr()) };
        if rc == ERROR_SUCCESS || rc == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(os_err(rc))
        }
    }

    fn notify() {
        // SAFETY: documented global notifications; no buffers are passed.
        unsafe {
            InternetSetOptionW(
                std::ptr::null(),
                INTERNET_OPTION_SETTINGS_CHANGED,
                std::ptr::null(),
                0,
            );
            InternetSetOptionW(
                std::ptr::null(),
                INTERNET_OPTION_REFRESH,
                std::ptr::null(),
                0,
            );
        }
    }

    /// HKCU WinINet settings.
    pub(crate) struct RegistryStore;

    impl InternetSettingsStore for RegistryStore {
        fn read(&self) -> io::Result<WinInetSettings> {
            let key = open(KEY_READ)?;
            Ok(WinInetSettings {
                proxy_enable: read_dword(&key, "ProxyEnable")?,
                proxy_server: read_string(&key, "ProxyServer")?,
                proxy_override: read_string(&key, "ProxyOverride")?,
                auto_config_url: read_string(&key, "AutoConfigURL")?,
            })
        }

        fn write(&self, s: &WinInetSettings) -> io::Result<()> {
            let key = open(KEY_READ | KEY_WRITE)?;
            let result: io::Result<()> = (|| {
                match s.proxy_enable {
                    Some(v) => set_dword(&key, "ProxyEnable", v)?,
                    None => delete(&key, "ProxyEnable")?,
                }
                for (name, value) in [
                    ("ProxyServer", &s.proxy_server),
                    ("ProxyOverride", &s.proxy_override),
                    ("AutoConfigURL", &s.auto_config_url),
                ] {
                    match value {
                        Some(v) => set_string(&key, name, v)?,
                        None => delete(&key, name)?,
                    }
                }
                Ok(())
            })();
            notify();
            result
        }
    }
}

// --------------------------------------------------------------------- trust instructions

/// Per-OS instructions for trusting (and later untrusting) the CA system-wide. Akev never
/// runs these itself: changing the OS trust store is the developer's decision.
pub(crate) fn trust_instructions(ctx: &InterceptContext, os: Os) -> Value {
    use sha2::{Digest, Sha256};
    let sha256 = hex::encode_upper(Sha256::digest(&ctx.ca_der));
    let ca = ctx.ca_path.to_string_lossy().into_owned();
    match os {
        Os::Windows => {
            let ca_q = format!("\"{ca}\"");
            json!({
                "summary": "Windows apps (Edge, Chrome, .NET, most native apps) use the Windows certificate store. Add the Akev CA to your user's Trusted Root store (Windows asks for confirmation), and remove it when you're done.",
                "options": [
                    {
                        "title": "Current user (Command Prompt or PowerShell)",
                        "install": [format!("certutil -user -addstore Root {ca_q}")],
                        "remove": [format!("powershell -NoProfile -Command \"Get-ChildItem Cert:\\CurrentUser\\Root | Where-Object {{ $_.GetCertHashString('SHA256') -eq '{sha256}' }} | Remove-Item\"")],
                    },
                    {
                        "title": "Certificate manager (GUI)",
                        "steps": [
                            format!("Double-click {ca} → Install Certificate… → Current User → Place all certificates in: Trusted Root Certification Authorities → Finish."),
                            "To remove: run certmgr.msc → Trusted Root Certification Authorities → Certificates → delete the Akev certificate.",
                        ],
                    },
                ],
                "notes": ["Firefox uses its own store unless security.enterprise_roots.enabled is on (the default on Windows).", "Java, Node.js and Python ignore the Windows store: use the terminal interceptor's environment variables for them."],
                "caSha256": sha256,
            })
        }
        Os::MacOs => {
            let ca_q = posix_quote(&ca);
            json!({
                "summary": "macOS apps (Safari, Chrome, curl, URLSession) use the Keychain. Add the Akev CA as a trusted root, and remove it when you're done.",
                "options": [
                    {
                        "title": "Login keychain (current user, asks for your password)",
                        "install": [format!("security add-trusted-cert -r trustRoot -k ~/Library/Keychains/login.keychain-db {ca_q}")],
                        "remove": [format!("security remove-trusted-cert {ca_q}"), format!("security delete-certificate -Z {sha256} ~/Library/Keychains/login.keychain-db")],
                    },
                    {
                        "title": "System keychain (all users)",
                        "install": [format!("sudo security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain {ca_q}")],
                        "remove": [format!("sudo security remove-trusted-cert -d {ca_q}"), format!("sudo security delete-certificate -Z {sha256} /Library/Keychains/System.keychain")],
                    },
                    {
                        "title": "Keychain Access (GUI)",
                        "steps": [
                            format!("Open {ca} with Keychain Access and add it to the login keychain."),
                            "Double-click the Akev certificate → Trust → When using this certificate: Always Trust.",
                            "To remove: select the certificate in Keychain Access and delete it.",
                        ],
                    },
                ],
                "notes": ["Firefox uses its own store unless security.enterprise_roots.enabled is on.", "Java, Node.js and Python ignore the Keychain: use the terminal interceptor's environment variables for them."],
                "caSha256": sha256,
            })
        }
        _ => {
            let ca_q = posix_quote(&ca);
            let nickname = posix_quote(&crate::trust::nss_nickname(&ctx.ca_der));
            json!({
                "summary": "Most Linux tools read the distribution's CA bundle; Chrome and Chromium read the NSS database in ~/.pki/nssdb. Add the Akev CA where you need it (requires sudo for the system bundle), and remove it when you're done.",
                "options": [
                    {
                        "title": "Fedora, RHEL, Arch, openSUSE (p11-kit)",
                        "install": [format!("sudo trust anchor --store {ca_q}")],
                        "remove": [format!("sudo trust anchor --remove {ca_q}")],
                    },
                    {
                        "title": "Debian, Ubuntu",
                        "install": [format!("sudo cp {ca_q} /usr/local/share/ca-certificates/akev-ca.crt"), "sudo update-ca-certificates".to_string()],
                        "remove": ["sudo rm /usr/local/share/ca-certificates/akev-ca.crt".to_string(), "sudo update-ca-certificates --fresh".to_string()],
                    },
                    {
                        "title": "Chrome / Chromium (NSS, current user)",
                        "install": [format!("certutil -d sql:$HOME/.pki/nssdb -A -t C,, -n {nickname} -i {ca_q}")],
                        "remove": [format!("certutil -d sql:$HOME/.pki/nssdb -D -n {nickname}")],
                    },
                ],
                "notes": ["Firefox keeps its own certificate store: use the Firefox interceptor, or import the CA in Settings → Privacy & Security → Certificates.", "Java, Node.js and Python may ignore the system bundle: use the terminal interceptor's environment variables for them."],
                "caSha256": sha256,
            })
        }
    }
}

// ---------------------------------------------------------------------------- interceptor

/// Who owns a backup file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Owner {
    /// This process.
    Us,
    /// Another Akev process that is still running.
    OtherInstance { pid: u32 },
    /// Nobody: the owner exited without restoring (crash, kill, power loss).
    Stale,
}

struct Inner {
    sys: Arc<Sys>,
    store: Arc<dyn InternetSettingsStore>,
    backends: ProbeCache<Vec<Backend>>,
    /// Serialises activation, deactivation and recovery.
    op: tokio::sync::Mutex<()>,
    /// Last automatic recovery attempt per data directory.
    recovery_attempts: Mutex<HashMap<PathBuf, Instant>>,
}

/// `system-proxy`.
pub(crate) struct SystemProxyInterceptor {
    inner: Arc<Inner>,
    /// Background crash recovery started by the constructor (awaited by tests).
    #[cfg_attr(not(test), allow(dead_code))]
    startup_recovery: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl SystemProxyInterceptor {
    const DESCRIPTOR: Descriptor = Descriptor {
        id: "system-proxy",
        name: "System Proxy",
        description: "Intercept all HTTP traffic on this machine by pointing the OS-wide proxy setting at Akev (your previous settings are restored when you stop, even after a crash)",
        category: InterceptorCategory::System,
        tags: tags::SYSTEM,
        icon: "desktop",
        needs_config: false,
    };

    pub(crate) fn new(sys: Arc<Sys>) -> Self {
        Self::with_store(sys, platform_store())
    }

    /// With a custom WinINet store (tests). Starts crash recovery of every backup a previous
    /// run may have left (default data directories and the recovery directory) in the
    /// background when called inside a Tokio runtime.
    pub(crate) fn with_store(sys: Arc<Sys>, store: Arc<dyn InternetSettingsStore>) -> Self {
        let inner = Arc::new(Inner {
            sys,
            store,
            backends: ProbeCache::new(PROBE_TTL),
            op: tokio::sync::Mutex::new(()),
            recovery_attempts: Mutex::new(HashMap::new()),
        });
        let startup = tokio::runtime::Handle::try_current().ok().map(|rt| {
            let inner = inner.clone();
            rt.spawn(async move {
                for path in scan_journals(&inner.sys.env, BACKUP_FILE) {
                    let _guard = inner.op.lock().await;
                    inner.recover_file(&path, true).await;
                }
            })
        });
        SystemProxyInterceptor {
            inner,
            startup_recovery: Mutex::new(startup),
        }
    }

    #[cfg(test)]
    async fn wait_startup_recovery(&self) {
        let task = self.startup_recovery.lock().take();
        if let Some(task) = task {
            let _ = task.await;
        }
    }
}

impl Inner {
    /// Where the backup of `data_dir` is kept (see [`crate::journal::journal_file`]).
    fn journal_path(&self, data_dir: &Path) -> PathBuf {
        journal_file(&self.sys.env, data_dir, BACKUP_FILE)
    }

    /// Every backup file that may hold settings to restore: `data_dir`'s, the default data
    /// directories' and the recovery directory's.
    fn recovery_candidates(&self, data_dir: &Path) -> Vec<PathBuf> {
        let mut paths = vec![self.journal_path(data_dir)];
        paths.extend(scan_journals(&self.sys.env, BACKUP_FILE));
        dedup(paths)
    }

    async fn backends(&self) -> Vec<Backend> {
        let env = &self.sys.env;
        self.backends
            .get(|| async move { detect_backends(env) })
            .await
    }

    async fn run(&self, spec: CommandSpec) -> Result<crate::runner::CommandOutput> {
        let line = spec.to_string();
        self.sys
            .runner
            .output(&spec.timeout(CMD_TIMEOUT))
            .await
            .map_err(|e| InterceptError::Failed(format!("`{line}`: {e}")))
    }

    async fn run_ok(&self, spec: CommandSpec) -> Result<String> {
        let line = spec.to_string();
        let out = self.run(spec).await?;
        if out.success() {
            Ok(out.stdout)
        } else {
            Err(InterceptError::Failed(format!(
                "`{line}` failed: {}",
                out.error_summary()
            )))
        }
    }

    async fn networksetup(&self, tool: &Path, args: &[&str]) -> Result<String> {
        let spec = CommandSpec::new(tool).args(args.iter().copied());
        let line = spec.to_string();
        let out = self.run(spec).await?;
        match mac_output_error(&out) {
            Some(e) => Err(InterceptError::Failed(format!("`{line}` failed: {e}"))),
            None => Ok(out.stdout),
        }
    }

    async fn store_read(&self) -> Result<WinInetSettings> {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || store.read())
            .await
            .map_err(|e| InterceptError::Failed(format!("registry task failed: {e}")))?
            .map_err(|e| InterceptError::Failed(format!("couldn't read Internet Settings: {e}")))
    }

    async fn store_write(&self, settings: WinInetSettings) -> Result<()> {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || store.write(&settings))
            .await
            .map_err(|e| InterceptError::Failed(format!("registry task failed: {e}")))?
            .map_err(|e| InterceptError::Failed(format!("couldn't write Internet Settings: {e}")))
    }

    /// Set GNOME keys in order (stopping at the first failing `set`) and read them back.
    /// `gsettings set` exits 0 even when dconf can't commit the change (no D-Bus session: cron,
    /// `sudo -u`, a CI runner), and `gsettings get` keeps reading the old on-disk value, so a
    /// write only counts once a fresh read returns it.
    async fn gsettings_write(&self, gsettings: &Path, values: &[&GsettingsValue]) -> Result<()> {
        let mut commit_warning = None;
        for v in values {
            let spec = CommandSpec::new(gsettings).args([
                "set",
                v.schema.as_str(),
                v.key.as_str(),
                v.value.as_str(),
            ]);
            let line = spec.to_string();
            let out = self.run(spec).await?;
            if !out.success() {
                return Err(InterceptError::Failed(format!(
                    "`{line}` failed: {}",
                    out.error_summary()
                )));
            }
            if let Some(w) = gsettings_commit_warning(&out) {
                commit_warning.get_or_insert(w);
            }
        }
        let mut pending: Vec<&GsettingsValue> = values.to_vec();
        for attempt in 0..2 {
            if attempt > 0 {
                // Give dconf a moment before concluding the writes were lost.
                tokio::time::sleep(GSETTINGS_RECHECK).await;
            }
            let mut still = Vec::new();
            for v in pending {
                let current = self
                    .run_ok(CommandSpec::new(gsettings).args([
                        "get",
                        v.schema.as_str(),
                        v.key.as_str(),
                    ]))
                    .await;
                if !matches!(current, Ok(out) if gvariant_eq(&out, &v.value)) {
                    still.push(v);
                }
            }
            pending = still;
            if pending.is_empty() {
                break;
            }
        }
        if !pending.is_empty() {
            let not_stuck: Vec<String> = pending
                .iter()
                .map(|v| format!("{} {}", v.schema, v.key))
                .collect();
            return Err(InterceptError::Failed(format!(
                "the GNOME proxy settings didn't change ({}){}",
                not_stuck.join(", "),
                match commit_warning {
                    Some(w) => format!(": {w} (is a D-Bus session available?)"),
                    None => String::new(),
                }
            )));
        }
        Ok(())
    }

    async fn kde_write(&self, b: &Backend, key: &str, value: Option<&str>) -> Result<()> {
        let Backend::Kde {
            kwriteconfig,
            config_file,
            ..
        } = b
        else {
            return Err(InterceptError::Failed("not a KDE backend".into()));
        };
        let mut spec = CommandSpec::new(kwriteconfig)
            .arg("--file")
            .arg(config_file)
            .args(["--group", KDE_GROUP, "--key", key]);
        spec = match value {
            Some(v) => spec.arg(v),
            None => spec.arg("--delete"),
        };
        self.run_ok(spec).await.map(drop)
    }

    async fn kde_notify(&self, b: &Backend) {
        if let Backend::Kde {
            dbus_send: Some(dbus),
            ..
        } = b
        {
            let spec = CommandSpec::new(dbus).args([
                "--session",
                "--type=signal",
                "/KIO/Scheduler",
                "org.kde.KIO.Scheduler.reparseSlaveConfiguration",
                "string:",
            ]);
            if let Err(e) = self.run_ok(spec).await {
                tracing::debug!("couldn't notify KIO of proxy changes: {e}");
            }
        }
    }

    /// Capture the current settings `b` is about to change.
    async fn snapshot(&self, b: &Backend) -> Result<Snapshot> {
        match b {
            Backend::Gnome { gsettings } => {
                let mut values = Vec::new();
                for (schema, key) in GNOME_KEYS {
                    let out = self
                        .run_ok(CommandSpec::new(gsettings).args(["get", schema, key]))
                        .await?;
                    values.push(GsettingsValue {
                        schema: schema.to_string(),
                        key: key.to_string(),
                        value: out.trim().to_string(),
                    });
                }
                Ok(Snapshot::Gnome { values })
            }
            Backend::Kde { config_file, .. } => {
                let content = match std::fs::read_to_string(config_file) {
                    Ok(c) => c,
                    Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
                    Err(e) => {
                        return Err(InterceptError::Failed(format!(
                            "couldn't read {}: {e}",
                            config_file.display()
                        )));
                    }
                };
                Ok(Snapshot::Kde {
                    values: kconfig_values(&content, KDE_GROUP, KDE_KEYS),
                })
            }
            Backend::MacOs { networksetup } => {
                let list = self
                    .networksetup(networksetup, &["-listallnetworkservices"])
                    .await?;
                let mut services = Vec::new();
                for name in parse_mac_services(&list) {
                    let web = self
                        .networksetup(networksetup, &["-getwebproxy", &name])
                        .await?;
                    let secure = self
                        .networksetup(networksetup, &["-getsecurewebproxy", &name])
                        .await?;
                    let auto = self
                        .networksetup(networksetup, &["-getautoproxyurl", &name])
                        .await?;
                    services.push(MacService {
                        name,
                        web: parse_mac_proxy(&web),
                        secure: parse_mac_proxy(&secure),
                        auto_proxy: parse_mac_auto_proxy(&auto),
                    });
                }
                if services.is_empty() {
                    return Err(InterceptError::Unavailable(
                        "no enabled network service".into(),
                    ));
                }
                Ok(Snapshot::MacOs { services })
            }
            Backend::Windows => Ok(Snapshot::Windows {
                settings: self.store_read().await?,
            }),
        }
    }

    /// Point `b` at `host:port`. For macOS, `services` lists the services to change (from
    /// the snapshot just taken).
    async fn apply(&self, b: &Backend, snap: &Snapshot, host: &str, port: u16) -> Result<()> {
        match b {
            Backend::Gnome { gsettings } => {
                let target = gnome_target(host, port);
                let values: Vec<&GsettingsValue> = target.iter().collect();
                self.gsettings_write(gsettings, &values).await
            }
            Backend::Kde { .. } => {
                for (key, value) in kde_target(host, port) {
                    self.kde_write(b, key, Some(&value)).await?;
                }
                self.kde_notify(b).await;
                Ok(())
            }
            Backend::MacOs { networksetup } => {
                let Snapshot::MacOs { services } = snap else {
                    return Err(InterceptError::Failed("snapshot/backend mismatch".into()));
                };
                let port_s = port.to_string();
                for svc in services {
                    let name = svc.name.as_str();
                    self.networksetup(networksetup, &["-setwebproxy", name, host, &port_s])
                        .await?;
                    self.networksetup(networksetup, &["-setsecurewebproxy", name, host, &port_s])
                        .await?;
                    if svc.auto_proxy.enabled {
                        self.networksetup(networksetup, &["-setautoproxystate", name, "off"])
                            .await?;
                    }
                }
                Ok(())
            }
            Backend::Windows => self.store_write(windows_target(host, port)).await,
        }
    }

    /// Put back the settings captured in `snap`.
    async fn restore(&self, snap: &Snapshot) -> Result<()> {
        let b = backend_for_snapshot(&self.sys.env, snap).ok_or_else(|| {
            InterceptError::Unavailable(format!(
                "the tools needed to restore the {} proxy settings aren't available",
                snap.kind()
            ))
        })?;
        match (snap, &b) {
            (Snapshot::Gnome { values }, Backend::Gnome { gsettings }) => {
                // The mode first, so apps stop using the proxy straight away. Stop at the first
                // failure: keys restored after a failed `mode` would make the settings no longer
                // look like Akev's, and the retry would then leave the proxy switched on.
                let mut ordered: Vec<&GsettingsValue> = values.iter().collect();
                ordered.sort_by_key(|v| v.key != "mode");
                self.gsettings_write(gsettings, &ordered).await
            }
            (Snapshot::Kde { values }, Backend::Kde { .. }) => {
                let mut ordered: Vec<&KdeValue> = values.iter().collect();
                ordered.sort_by_key(|v| v.key != "ProxyType");
                let mut first_err = None;
                for v in ordered {
                    if let Err(e) = self.kde_write(&b, &v.key, v.value.as_deref()).await {
                        first_err.get_or_insert(e);
                    }
                }
                self.kde_notify(&b).await;
                first_err.map_or(Ok(()), Err)
            }
            (Snapshot::MacOs { services }, Backend::MacOs { networksetup }) => {
                let mut first_err = None;
                for svc in services {
                    for (set, state, p) in [
                        ("-setwebproxy", "-setwebproxystate", &svc.web),
                        ("-setsecurewebproxy", "-setsecurewebproxystate", &svc.secure),
                    ] {
                        let port = p.port.to_string();
                        let mut steps: Vec<Vec<&str>> = Vec::new();
                        if !p.server.is_empty() && p.port > 0 {
                            steps.push(vec![set, &svc.name, &p.server, &port]);
                        }
                        if !p.enabled {
                            steps.push(vec![state, &svc.name, "off"]);
                        }
                        for args in steps {
                            if let Err(e) = self.networksetup(networksetup, &args).await {
                                first_err.get_or_insert(e);
                            }
                        }
                    }
                    if svc.auto_proxy.enabled
                        && let Err(e) = self
                            .networksetup(networksetup, &["-setautoproxystate", &svc.name, "on"])
                            .await
                    {
                        first_err.get_or_insert(e);
                    }
                }
                first_err.map_or(Ok(()), Err)
            }
            (Snapshot::Windows { settings }, Backend::Windows) => {
                self.store_write(settings.clone()).await
            }
            _ => Err(InterceptError::Failed("snapshot/backend mismatch".into())),
        }
    }

    /// Do the settings captured by `snap` still point at `proxy` (`host:port`)? `None` when
    /// that can't be determined. Used so a restore never clobbers settings the user changed
    /// after Akev set them.
    async fn still_points_at(&self, snap: &Snapshot, proxy: &str) -> Option<bool> {
        let (host, port) = proxy.rsplit_once(':')?;
        let host = host.trim_start_matches('[').trim_end_matches(']');
        let port: u16 = port.parse().ok()?;
        let b = backend_for_snapshot(&self.sys.env, snap)?;
        match (&b, snap) {
            (Backend::Gnome { gsettings }, _) => {
                let mut current = Vec::new();
                for (schema, key) in [
                    ("org.gnome.system.proxy", "mode"),
                    ("org.gnome.system.proxy.http", "host"),
                    ("org.gnome.system.proxy.http", "port"),
                ] {
                    let out = self
                        .run_ok(CommandSpec::new(gsettings).args(["get", schema, key]))
                        .await
                        .ok()?;
                    current.push(out.trim().to_string());
                }
                Some(current == [gvariant_str("manual"), gvariant_str(host), port.to_string()])
            }
            (Backend::Kde { config_file, .. }, _) => {
                let content = std::fs::read_to_string(config_file).unwrap_or_default();
                let values = kconfig_values(&content, KDE_GROUP, &["ProxyType", "httpProxy"]);
                let ours = kde_target(host, port);
                let expected_proxy = ours.iter().find(|(k, _)| *k == "httpProxy").map(|(_, v)| v);
                Some(
                    values[0].value.as_deref() == Some("1")
                        && values[1].value.as_ref() == expected_proxy,
                )
            }
            (Backend::MacOs { networksetup }, Snapshot::MacOs { services }) => {
                for svc in services {
                    let out = self
                        .networksetup(networksetup, &["-getwebproxy", &svc.name])
                        .await
                        .ok()?;
                    let p = parse_mac_proxy(&out);
                    if p.enabled && p.server == host && p.port == port {
                        return Some(true);
                    }
                }
                Some(false)
            }
            (Backend::Windows, _) => {
                let current = self.store_read().await.ok()?;
                let target = windows_target(host, port);
                Some(current.proxy_enable == Some(1) && current.proxy_server == target.proxy_server)
            }
            _ => None,
        }
    }

    /// Restore every snapshot (most recent first). Returns the ones that failed.
    ///
    /// Unless `force`, a backend whose settings no longer point at `backup.proxy` (someone
    /// changed them after Akev did) is left as it is.
    async fn restore_all(&self, backup: &Backup, force: bool) -> Vec<Snapshot> {
        let mut failed = Vec::new();
        for snap in backup.snapshots.iter().rev() {
            if !force && self.still_points_at(snap, &backup.proxy).await == Some(false) {
                tracing::info!(
                    backend = snap.kind(),
                    "proxy settings were changed since Akev set them; leaving them as they are"
                );
                continue;
            }
            if let Err(e) = self.restore(snap).await {
                tracing::warn!(
                    backend = snap.kind(),
                    "couldn't restore system proxy settings: {e}"
                );
                failed.push(snap.clone());
            }
        }
        failed.reverse();
        failed
    }

    /// Restore `backup` (see [`Inner::restore_all`]) and update its file at `path`: removed
    /// on success, trimmed to the snapshots still to restore otherwise. Returns whether
    /// everything was restored.
    async fn restore_and_record(&self, path: &Path, backup: &Backup, force: bool) -> bool {
        let failed = self.restore_all(backup, force).await;
        if failed.is_empty() {
            remove_backup(path);
            return true;
        }
        let remaining = Backup {
            snapshots: failed,
            ..backup.clone()
        };
        if let Err(e) = write_backup(path, &remaining) {
            tracing::warn!("couldn't update the system proxy backup: {e}");
        }
        false
    }

    async fn owner(&self, b: &Backup) -> Owner {
        if b.instance == instance_id() {
            return Owner::Us;
        }
        if b.pid == std::process::id() {
            // Our pid, another instance id: the process that wrote it is gone.
            return Owner::Stale;
        }
        self.sys.invalidate_processes().await;
        let procs = self.sys.processes(false).await;
        if procs.is_empty() {
            // Can't tell: never undo the settings of a possibly running instance.
            return Owner::OtherInstance { pid: b.pid };
        }
        match procs.iter().find(|p| p.pid == b.pid) {
            Some(p) if same_program(&p.name, &b.process_name) => {
                Owner::OtherInstance { pid: b.pid }
            }
            _ => Owner::Stale,
        }
    }

    /// Restore the backup at `path` if its owner is gone. Without `force`, attempts are
    /// rate-limited per file. Callers hold `op`.
    async fn recover_file(&self, path: &Path, force: bool) {
        let backup = match read_backup(path) {
            BackupState::None => return,
            BackupState::Corrupt(e) => {
                quarantine_backup(path, &e);
                return;
            }
            BackupState::Valid(b) => b,
        };
        if backup.instance == instance_id() {
            return;
        }
        {
            let mut attempts = self.recovery_attempts.lock();
            let recent = attempts
                .get(path)
                .is_some_and(|at| at.elapsed() < RECOVERY_RETRY);
            if recent && !force {
                return;
            }
            attempts.insert(path.to_path_buf(), Instant::now());
        }
        match self.owner(&backup).await {
            Owner::Stale => {
                if self.restore_and_record(path, &backup, false).await {
                    tracing::info!(
                        proxy = %backup.proxy,
                        "restored the system proxy settings left behind by a previous Akev run"
                    );
                } else {
                    tracing::warn!(
                        "some system proxy settings from a previous Akev run couldn't be restored; will retry"
                    );
                }
            }
            Owner::OtherInstance { pid } => {
                tracing::debug!(
                    pid,
                    "system proxy is managed by another running Akev instance"
                );
            }
            Owner::Us => {}
        }
    }

    fn metadata(&self, ctx: &InterceptContext, backends: &[Backend], active: bool) -> Value {
        let host = local_proxy_host(ctx);
        json!({
            "backends": backends.iter().map(|b| json!({"id": b.id(), "name": b.label()})).collect::<Vec<_>>(),
            "proxy": format!("{}:{}", host_for_url(&host), ctx.proxy_port),
            "bypass": LOOPBACK_BYPASS,
            "backupFile": self.journal_path(&ctx.data_dir),
            "active": active,
            "trust": trust_instructions(ctx, self.sys.env.os),
            "note": "Apps that follow the system proxy (browsers, most GUI apps) are captured. Many CLI tools and runtimes read environment variables instead: use a Fresh Terminal for those.",
        })
    }
}

fn no_backend_reason(os: Os) -> String {
    match os {
        Os::Linux => "No supported proxy settings found: needs GNOME (gsettings with the org.gnome.system.proxy schema) or KDE Plasma (kwriteconfig6/kwriteconfig5)".into(),
        Os::MacOs => "networksetup not found".into(),
        _ => "System proxy configuration isn't supported on this OS".into(),
    }
}

#[async_trait]
impl Interceptor for SystemProxyInterceptor {
    fn id(&self) -> &str {
        Self::DESCRIPTOR.id
    }

    async fn info(&self, ctx: &InterceptContext) -> InterceptorInfo {
        let d = Self::DESCRIPTOR;
        let inner = &self.inner;
        let os = inner.sys.env.os;
        if os == Os::Other {
            return d.unsupported("this OS");
        }
        // Crash recovery (rate-limited per backup), unless an activation/deactivation is
        // running (don't stall listing).
        if let Ok(_guard) = inner.op.try_lock() {
            for path in inner.recovery_candidates(&ctx.data_dir) {
                inner.recover_file(&path, false).await;
            }
        }
        let mut owner = None;
        let mut configured = None;
        if let BackupState::Valid(b) = read_backup(&inner.journal_path(&ctx.data_dir)) {
            owner = Some(inner.owner(&b).await);
            configured = Some(b.proxy);
        }
        let backends = inner.backends().await;
        let active = owner == Some(Owner::Us);
        let mut metadata = inner.metadata(ctx, &backends, active);
        if active && let Some(configured) = configured {
            // Differs from `proxy` after a port change: activating again re-points it.
            metadata["configuredProxy"] = json!(configured);
        }
        let reason = if backends.is_empty() {
            Some(no_backend_reason(os))
        } else if let Some(Owner::OtherInstance { pid }) = owner {
            Some(format!(
                "Another running Akev instance (pid {pid}) has set the system proxy"
            ))
        } else {
            None
        };
        InterceptorInfo {
            available: reason.is_none() || active,
            active,
            unavailable_reason: if active { None } else { reason },
            metadata,
            ..d.info()
        }
    }

    async fn activate(&self, ctx: &InterceptContext, _options: Value) -> Result<ActivationResult> {
        let inner = &self.inner;
        let os = inner.sys.env.os;
        if os == Os::Other {
            return Err(InterceptError::Unavailable(no_backend_reason(os)));
        }
        let _guard = inner.op.lock().await;
        inner.backends.invalidate().await;
        let backends = inner.backends().await;
        if backends.is_empty() {
            return Err(InterceptError::Unavailable(no_backend_reason(os)));
        }
        let host = local_proxy_host(ctx);
        let port = ctx.proxy_port;
        let proxy = format!("{}:{port}", host_for_url(&host));
        let journal = inner.journal_path(&ctx.data_dir);

        // Settings left behind by a previous run (whatever data directory it used) go back
        // first, or the snapshot below would record Akev's dead proxy as "previous".
        for path in inner.recovery_candidates(&ctx.data_dir) {
            let existing = match read_backup(&path) {
                BackupState::Valid(b) => b,
                BackupState::Corrupt(e) => {
                    quarantine_backup(&path, &e);
                    continue;
                }
                BackupState::None => continue,
            };
            let ours_to_manage = path == journal;
            match inner.owner(&existing).await {
                Owner::Us if path == journal => {
                    // Already active: re-point it (the proxy port may have changed).
                    for snap in &existing.snapshots {
                        if let Some(b) = backend_for_snapshot(&inner.sys.env, snap) {
                            inner.apply(&b, snap, &host, port).await?;
                        }
                    }
                    let updated = Backup {
                        proxy: proxy.clone(),
                        ..existing
                    };
                    write_backup(&journal, &updated)?;
                    return Ok(ActivationResult {
                        success: true,
                        message: Some(format!("The system proxy already points at Akev ({proxy})")),
                        metadata: inner.metadata(ctx, &backends, true),
                    });
                }
                // This process, but for another data directory: not ours to touch here.
                Owner::Us => {}
                Owner::OtherInstance { pid } if ours_to_manage => {
                    return Err(InterceptError::Unavailable(format!(
                        "another running Akev instance (pid {pid}) has set the system proxy; stop it there first"
                    )));
                }
                Owner::OtherInstance { .. } => {}
                Owner::Stale => {
                    if !inner.restore_and_record(&path, &existing, false).await {
                        return Err(InterceptError::Failed(
                            "couldn't restore the proxy settings left behind by a previous Akev run".into(),
                        ));
                    }
                }
            }
        }

        // 1. Capture everything first: nothing has changed if this fails.
        let mut snapshots = Vec::new();
        for b in &backends {
            let snap = inner.snapshot(b).await.map_err(|e| {
                InterceptError::Failed(format!(
                    "couldn't read the current {} proxy settings: {e}",
                    b.label()
                ))
            })?;
            snapshots.push(snap);
        }
        // 2. Journal (crash safety).
        let backup = Backup {
            version: BACKUP_VERSION,
            instance: instance_id().to_string(),
            pid: std::process::id(),
            process_name: own_process_name(&inner.sys).await,
            created_at: unix_now(),
            proxy: proxy.clone(),
            snapshots,
        };
        write_backup(&journal, &backup).map_err(|e| {
            InterceptError::Failed(format!("couldn't save the current proxy settings: {e}"))
        })?;
        // 3. Apply; on failure put everything back.
        for (b, snap) in backends.iter().zip(&backup.snapshots) {
            if let Err(e) = inner.apply(b, snap, &host, port).await {
                // Force: half-applied settings don't "point at Akev" yet but must go back.
                let restored = inner.restore_and_record(&journal, &backup, true).await;
                return Err(InterceptError::Failed(format!(
                    "couldn't set the {} proxy: {e}{}",
                    b.label(),
                    if restored {
                        " (previous settings restored)"
                    } else {
                        ""
                    }
                )));
            }
        }
        let names = backends
            .iter()
            .map(Backend::label)
            .collect::<Vec<_>>()
            .join(" + ");
        Ok(ActivationResult {
            success: true,
            message: Some(format!(
                "System proxy ({names}) now points at Akev ({proxy}). For HTTPS, apps must trust the Akev CA: see the instructions. Your previous settings come back when you stop."
            )),
            metadata: inner.metadata(ctx, &backends, true),
        })
    }

    async fn deactivate(&self, ctx: &InterceptContext, _options: Value) -> Result<()> {
        let inner = &self.inner;
        let _guard = inner.op.lock().await;
        let path = inner.journal_path(&ctx.data_dir);
        let backup = match read_backup(&path) {
            BackupState::Valid(b) => b,
            BackupState::Corrupt(e) => {
                quarantine_backup(&path, &e);
                return Ok(());
            }
            BackupState::None => return Ok(()),
        };
        match inner.owner(&backup).await {
            Owner::OtherInstance { pid } => {
                tracing::info!(
                    pid,
                    "system proxy belongs to another running Akev instance; left alone"
                );
                Ok(())
            }
            Owner::Us | Owner::Stale => {
                inner.backends.invalidate().await;
                if inner.restore_and_record(&path, &backup, false).await {
                    Ok(())
                } else {
                    Err(InterceptError::Failed(
                        "some proxy settings couldn't be restored; Akev will retry on the next start"
                            .into(),
                    ))
                }
            }
        }
    }

    async fn deactivate_all(&self, ctx: &InterceptContext) -> Result<()> {
        self.deactivate(ctx, json!({})).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::testutil::{fake_env, make_exe, native};
    use crate::runner::mock::MockRunner;
    use crate::runner::{CommandOutput, ProcessEntry};
    use crate::util::testctx::ctx;

    /// In-memory "gsettings": `get`/`set` against a map, recording every call.
    fn fake_gsettings(runner: &MockRunner, initial: &[(&str, &str, &str)]) -> GsettingsDb {
        let store: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(
            initial
                .iter()
                .map(|(s, k, v)| (format!("{s} {k}"), v.to_string()))
                .collect(),
        ));
        let s = store.clone();
        runner.on(move |spec| {
            if spec.program_name() != "gsettings" {
                return None;
            }
            let args = spec.args_lossy();
            let key = format!("{} {}", args.get(1)?, args.get(2)?);
            Some(Ok(match args[0].as_str() {
                "get" => match s.lock().get(&key) {
                    Some(v) => CommandOutput::ok(format!("{v}\n")),
                    None => CommandOutput::failed("No such key"),
                },
                "set" => {
                    s.lock().insert(key, args.get(3)?.clone());
                    CommandOutput::ok("")
                }
                _ => CommandOutput::failed("bad"),
            }))
        });
        store
    }

    const GNOME_DEFAULTS: &[(&str, &str, &str)] = &[
        ("org.gnome.system.proxy", "mode", "'none'"),
        (
            "org.gnome.system.proxy",
            "ignore-hosts",
            "['localhost', '127.0.0.0/8', '::1', '*.corp']",
        ),
        ("org.gnome.system.proxy.http", "host", "''"),
        ("org.gnome.system.proxy.http", "port", "0"),
        ("org.gnome.system.proxy.https", "host", "'corp-proxy'"),
        ("org.gnome.system.proxy.https", "port", "3128"),
    ];

    fn gnome_env(dir: &Path) -> HostEnv {
        make_exe(&dir.join("usr/bin/gsettings"), "");
        let schema = dir.join("usr/share/glib-2.0/schemas/org.gnome.system.proxy.gschema.xml");
        std::fs::create_dir_all(schema.parent().unwrap()).unwrap();
        std::fs::write(&schema, "<schemalist/>").unwrap();
        fake_env(Os::Linux, dir).with_var("XDG_CURRENT_DESKTOP", "ubuntu:GNOME")
    }

    /// A process table where the current test process is visible (so liveness checks work).
    fn with_own_process(runner: &MockRunner) {
        runner.processes.lock().push(ProcessEntry {
            pid: std::process::id(),
            name: "akev-test".into(),
            ..Default::default()
        });
    }

    /// In-memory gsettings database (`"<schema> <key>"` → GVariant text).
    type GsettingsDb = Arc<Mutex<HashMap<String, String>>>;

    fn gnome_setup() -> (
        tempfile::TempDir,
        Arc<MockRunner>,
        GsettingsDb,
        SystemProxyInterceptor,
        InterceptContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let env = gnome_env(dir.path());
        let runner = MockRunner::new();
        with_own_process(&runner);
        let store = fake_gsettings(&runner, GNOME_DEFAULTS);
        let i = SystemProxyInterceptor::with_store(
            Sys::new(runner.clone(), env),
            Arc::new(UnsupportedStore),
        );
        let c = ctx(&dir.path().join("data"));
        (dir, runner, store, i, c)
    }

    /// Where `i` keeps the backup for `c`'s data directory.
    fn journal(i: &SystemProxyInterceptor, c: &InterceptContext) -> PathBuf {
        i.inner.journal_path(&c.data_dir)
    }

    #[test]
    fn gvariant_and_targets() {
        assert_eq!(gvariant_str("it's \\"), r"'it\'s \\'");
        let t = gnome_target("127.0.0.1", 8000);
        assert_eq!(t.last().unwrap().key, "mode", "mode is switched last");
        assert_eq!(t.last().unwrap().value, "'manual'");
        assert!(
            t.iter().any(
                |v| v.key == "ignore-hosts" && v.value == "['localhost', '127.0.0.0/8', '::1']"
            )
        );
        assert!(t.iter().any(|v| v.schema == "org.gnome.system.proxy.https"
            && v.key == "port"
            && v.value == "8000"));
        let k = kde_target("::1", 9000);
        assert_eq!(k[0], ("httpProxy", "http://[::1] 9000".to_string()));
        assert_eq!(k.last().unwrap(), &("ProxyType", "1".to_string()));
        let w = windows_target("127.0.0.1", 8000);
        assert_eq!(w.proxy_enable, Some(1));
        assert_eq!(w.proxy_server.as_deref(), Some("127.0.0.1:8000"));
        assert_eq!(w.auto_config_url, None, "PAC would take precedence");
    }

    #[test]
    fn kconfig_parsing() {
        let content = "[General]\nProxyType=9\n\n[Proxy Settings]\n# comment\nProxyType=2\nhttpProxy[$e]=http://corp 3128\nProxy Config Script=http://wpad/wpad.dat\n[Other]\nhttpsProxy=nope\n";
        let v = kconfig_values(content, "Proxy Settings", KDE_KEYS);
        let get = |k: &str| v.iter().find(|x| x.key == k).unwrap().value.clone();
        assert_eq!(get("ProxyType").as_deref(), Some("2"));
        assert_eq!(get("httpProxy").as_deref(), Some("http://corp 3128"));
        assert_eq!(get("httpsProxy"), None);
        assert_eq!(
            kconfig_values("", "Proxy Settings", KDE_KEYS)
                .iter()
                .filter(|x| x.value.is_some())
                .count(),
            0
        );
    }

    #[test]
    fn mac_parsing() {
        let list = "An asterisk (*) denotes that a network service is disabled.\nWi-Fi\n*Bluetooth PAN\nUSB 10/100/1000 LAN\nThunderbolt Bridge\n";
        assert_eq!(
            parse_mac_services(list),
            vec!["Wi-Fi", "USB 10/100/1000 LAN", "Thunderbolt Bridge"]
        );
        let p = parse_mac_proxy(
            "Enabled: Yes\nServer: proxy.corp\nPort: 3128\nAuthenticated Proxy Enabled: 0\n",
        );
        assert_eq!(
            p,
            MacProxy {
                enabled: true,
                server: "proxy.corp".into(),
                port: 3128
            }
        );
        let off =
            parse_mac_proxy("Enabled: No\nServer: \nPort: 0\nAuthenticated Proxy Enabled: 0\n");
        assert_eq!(off, MacProxy::default());
        let auto = parse_mac_auto_proxy("URL: http://wpad/proxy.pac\nEnabled: Yes\n");
        assert!(auto.enabled && auto.url == "http://wpad/proxy.pac");
        assert_eq!(
            parse_mac_auto_proxy("URL: (null)\nEnabled: No\n"),
            MacAutoProxy::default()
        );
        assert!(
            mac_output_error(&CommandOutput::ok(
                "** Error: The parameters were not valid."
            ))
            .is_some()
        );
        assert!(mac_output_error(&CommandOutput::ok("")).is_none());
    }

    #[test]
    fn backend_detection() {
        let dir = tempfile::tempdir().unwrap();
        let env = fake_env(Os::Linux, dir.path());
        assert!(detect_backends(&env).is_empty());
        // gsettings without the schema isn't enough.
        make_exe(&dir.path().join("usr/bin/gsettings"), "");
        assert!(detect_backends(&env).is_empty());
        let env = gnome_env(dir.path());
        assert_eq!(
            detect_backends(&env)
                .iter()
                .map(Backend::id)
                .collect::<Vec<_>>(),
            vec!["gnome"]
        );
        make_exe(&dir.path().join("usr/bin/kwriteconfig6"), "");
        assert_eq!(
            detect_backends(&env).len(),
            1,
            "KDE tools only count on a KDE session"
        );
        let kde = env.clone().with_var("XDG_CURRENT_DESKTOP", "KDE");
        let ids: Vec<_> = detect_backends(&kde).iter().map(Backend::id).collect();
        assert_eq!(ids, vec!["kde", "gnome"]);
        match &detect_backends(&kde)[0] {
            Backend::Kde { config_file, .. } => {
                assert_eq!(config_file, &dir.path().join("home/.config/kioslaverc"))
            }
            other => panic!("{other:?}"),
        }
        // XDG_DATA_DIRS is honoured for the schema lookup.
        let elsewhere = env.clone().with_var(
            "XDG_DATA_DIRS",
            dir.path().join("nowhere").to_string_lossy(),
        );
        assert!(detect_backends(&elsewhere).is_empty());

        let mac = tempfile::tempdir().unwrap();
        let menv = fake_env(Os::MacOs, mac.path());
        assert!(detect_backends(&menv).is_empty());
        make_exe(&mac.path().join("usr/sbin/networksetup"), "");
        assert_eq!(
            detect_backends(&menv),
            vec![Backend::MacOs {
                networksetup: mac.path().join("usr/sbin/networksetup")
            }]
        );
        assert_eq!(
            detect_backends(&HostEnv::empty(Os::Windows)),
            vec![Backend::Windows]
        );
        assert!(detect_backends(&HostEnv::empty(Os::Other)).is_empty());
    }

    #[tokio::test]
    async fn gnome_activate_backup_and_restore() {
        let (_dir, runner, store, i, c) = gnome_setup();
        let info = i.info(&c).await;
        assert!(info.available && !info.active, "{info:?}");
        assert_eq!(info.metadata["backends"][0]["id"], json!("gnome"));
        assert!(info.metadata["trust"]["options"].as_array().unwrap().len() >= 2);

        let res = i.activate(&c, json!({})).await.unwrap();
        assert!(res.success);
        assert!(res.message.unwrap().contains("127.0.0.1:8000"));
        {
            let s = store.lock();
            assert_eq!(s["org.gnome.system.proxy mode"], "'manual'");
            assert_eq!(s["org.gnome.system.proxy.http host"], "'127.0.0.1'");
            assert_eq!(s["org.gnome.system.proxy.https port"], "8000");
        }
        // The backup was written before the first `set`, outside the (non-default) data dir.
        let j = journal(&i, &c);
        assert!(!j.starts_with(&c.data_dir), "{}", j.display());
        assert!(!backup_path(&c.data_dir).exists());
        let BackupState::Valid(b) = read_backup(&j) else {
            panic!("no backup")
        };
        assert_eq!(b.instance, instance_id());
        assert_eq!(b.pid, std::process::id());
        assert_eq!(b.process_name, "akev-test");
        let Snapshot::Gnome { values } = &b.snapshots[0] else {
            panic!()
        };
        assert!(
            values
                .iter()
                .any(|v| v.key == "mode" && v.value == "'none'")
        );
        let lines = runner.call_lines();
        let first_set = lines
            .iter()
            .position(|l| l.starts_with("gsettings set"))
            .unwrap();
        assert_eq!(
            lines[..first_set]
                .iter()
                .filter(|l| l.starts_with("gsettings get"))
                .count(),
            GNOME_KEYS.len(),
            "snapshot completes before any change"
        );
        // Every key was read back after being set.
        assert!(
            lines[first_set..]
                .iter()
                .filter(|l| l.starts_with("gsettings get"))
                .count()
                >= GNOME_KEYS.len()
        );
        assert!(i.info(&c).await.active);

        // Activating again just re-points it.
        let again = i.activate(&c, json!({})).await.unwrap();
        assert!(again.message.unwrap().contains("already"));

        i.deactivate(&c, json!({})).await.unwrap();
        let s = store.lock().clone();
        for (schema, key, value) in GNOME_DEFAULTS {
            assert_eq!(s[&format!("{schema} {key}")], *value, "{schema} {key}");
        }
        assert!(!journal(&i, &c).exists());
        assert!(!i.info(&c).await.active);
        // Deactivating with nothing to restore is a no-op.
        i.deactivate_all(&c).await.unwrap();
    }

    #[tokio::test]
    async fn failed_apply_rolls_back() {
        let (_dir, runner, store, i, c) = gnome_setup();
        // Refuse to switch the mode: everything already set must be rolled back.
        runner.on(|spec| {
            let args = spec.args_lossy();
            (spec.program_name() == "gsettings"
                && args[0] == "set"
                && args[2] == "mode"
                && args[3] == "'manual'")
                .then(|| Ok(CommandOutput::failed("permission denied")))
        });
        let err = i.activate(&c, json!({})).await.unwrap_err();
        assert!(
            err.to_string().contains("previous settings restored"),
            "{err}"
        );
        let s = store.lock().clone();
        assert_eq!(s["org.gnome.system.proxy.http host"], "''");
        assert_eq!(s["org.gnome.system.proxy.https host"], "'corp-proxy'");
        assert!(!journal(&i, &c).exists());
    }

    #[tokio::test]
    async fn failed_snapshot_changes_nothing() {
        let (_dir, runner, _store, i, c) = gnome_setup();
        runner.on_prefix(
            "gsettings get org.gnome.system.proxy.https port",
            CommandOutput::failed("dbus error"),
        );
        assert!(matches!(
            i.activate(&c, json!({})).await,
            Err(InterceptError::Failed(_))
        ));
        assert!(
            !runner
                .call_lines()
                .iter()
                .any(|l| l.starts_with("gsettings set"))
        );
        assert!(!journal(&i, &c).exists());
    }

    fn stale_backup(pid: u32, process_name: &str) -> Backup {
        Backup {
            version: BACKUP_VERSION,
            instance: "previous-run".into(),
            pid,
            process_name: process_name.into(),
            created_at: 1,
            proxy: "127.0.0.1:8000".into(),
            snapshots: vec![Snapshot::Gnome {
                values: GNOME_DEFAULTS
                    .iter()
                    .map(|(s, k, v)| GsettingsValue {
                        schema: s.to_string(),
                        key: k.to_string(),
                        value: v.to_string(),
                    })
                    .collect(),
            }],
        }
    }

    #[tokio::test]
    async fn stale_backup_restored_at_construction() {
        let dir = tempfile::tempdir().unwrap();
        let env = gnome_env(dir.path());
        let runner = MockRunner::new();
        with_own_process(&runner);
        // Settings left pointing at a dead Akev.
        let store = fake_gsettings(
            &runner,
            &[
                ("org.gnome.system.proxy", "mode", "'manual'"),
                ("org.gnome.system.proxy", "ignore-hosts", "[]"),
                ("org.gnome.system.proxy.http", "host", "'127.0.0.1'"),
                ("org.gnome.system.proxy.http", "port", "8000"),
                ("org.gnome.system.proxy.https", "host", "'127.0.0.1'"),
                ("org.gnome.system.proxy.https", "port", "8000"),
            ],
        );
        let default_dir = dir.path().join("home/.local/share/akev");
        write_backup(&backup_path(&default_dir), &stale_backup(999_999, "akev")).unwrap();

        let i = SystemProxyInterceptor::with_store(
            Sys::new(runner.clone(), env),
            Arc::new(UnsupportedStore),
        );
        i.wait_startup_recovery().await;
        assert_eq!(store.lock()["org.gnome.system.proxy mode"], "'none'");
        assert_eq!(
            store.lock()["org.gnome.system.proxy.https host"],
            "'corp-proxy'"
        );
        assert!(!backup_path(&default_dir).exists());
    }

    /// Make the fake gsettings look like Akev set it (and nobody touched it since).
    fn point_at_akev(store: &GsettingsDb) {
        let mut s = store.lock();
        s.insert("org.gnome.system.proxy mode".into(), "'manual'".into());
        s.insert(
            "org.gnome.system.proxy.http host".into(),
            "'127.0.0.1'".into(),
        );
        s.insert("org.gnome.system.proxy.http port".into(), "8000".into());
    }

    #[tokio::test]
    async fn stale_backup_restored_on_info_for_custom_data_dir() {
        let (_dir, _runner, store, i, c) = gnome_setup();
        i.wait_startup_recovery().await;
        point_at_akev(&store);
        // Our own pid but another instance id: a previous process that had our pid.
        let j = journal(&i, &c);
        std::fs::create_dir_all(j.parent().unwrap()).unwrap();
        write_backup(&j, &stale_backup(std::process::id(), "akev-test")).unwrap();
        let info = i.info(&c).await;
        assert!(!info.active);
        assert_eq!(store.lock()["org.gnome.system.proxy mode"], "'none'");
        assert!(!j.exists());
    }

    #[tokio::test]
    async fn other_running_instance_is_left_alone() {
        let (_dir, runner, store, i, c) = gnome_setup();
        runner.processes.lock().push(ProcessEntry {
            pid: 4242,
            name: "akev".into(),
            ..Default::default()
        });
        point_at_akev(&store);
        write_backup(&journal(&i, &c), &stale_backup(4242, "akev")).unwrap();

        let info = i.info(&c).await;
        assert!(!info.available && !info.active);
        assert!(info.unavailable_reason.unwrap().contains("pid 4242"));
        assert!(matches!(
            i.activate(&c, json!({})).await,
            Err(InterceptError::Unavailable(_))
        ));
        i.deactivate_all(&c).await.unwrap();
        assert_eq!(
            store.lock()["org.gnome.system.proxy mode"],
            "'manual'",
            "not ours to restore"
        );
        assert!(journal(&i, &c).exists());

        // Same pid, different program (pid reused after the owner died): stale.
        runner.processes.lock().retain(|p| p.pid != 4242);
        runner.processes.lock().push(ProcessEntry {
            pid: 4242,
            name: "vim".into(),
            ..Default::default()
        });
        i.deactivate_all(&c).await.unwrap();
        assert_eq!(store.lock()["org.gnome.system.proxy mode"], "'none'");
        assert!(!journal(&i, &c).exists());
    }

    #[tokio::test]
    async fn unknown_process_table_is_conservative() {
        let dir = tempfile::tempdir().unwrap();
        let runner = MockRunner::new(); // empty process table
        let store = fake_gsettings(&runner, GNOME_DEFAULTS);
        let i = SystemProxyInterceptor::with_store(
            Sys::new(runner, gnome_env(dir.path())),
            Arc::new(UnsupportedStore),
        );
        let c = ctx(&dir.path().join("data"));
        point_at_akev(&store);
        write_backup(&journal(&i, &c), &stale_backup(4243, "akev")).unwrap();
        i.info(&c).await;
        assert_eq!(store.lock()["org.gnome.system.proxy mode"], "'manual'");
    }

    #[tokio::test]
    async fn settings_changed_since_are_not_clobbered() {
        let (_dir, runner, store, i, c) = gnome_setup();
        i.wait_startup_recovery().await;
        // After the crash the user switched to a PAC file themselves.
        store
            .lock()
            .insert("org.gnome.system.proxy mode".into(), "'auto'".into());
        write_backup(&journal(&i, &c), &stale_backup(999_999, "akev")).unwrap();
        i.info(&c).await;
        assert_eq!(store.lock()["org.gnome.system.proxy mode"], "'auto'");
        assert!(
            !runner
                .call_lines()
                .iter()
                .any(|l| l.starts_with("gsettings set"))
        );
        assert!(!journal(&i, &c).exists(), "nothing left to restore");

        // Same when the user changes it while Akev is active.
        i.activate(&c, json!({})).await.unwrap();
        store
            .lock()
            .insert("org.gnome.system.proxy.http host".into(), "'corp'".into());
        let sets = runner
            .call_lines()
            .iter()
            .filter(|l| l.starts_with("gsettings set"))
            .count();
        i.deactivate(&c, json!({})).await.unwrap();
        assert_eq!(
            runner
                .call_lines()
                .iter()
                .filter(|l| l.starts_with("gsettings set"))
                .count(),
            sets
        );
        assert_eq!(store.lock()["org.gnome.system.proxy.http host"], "'corp'");
    }

    /// Make the backup at `path` look like it was left by a process that died.
    fn orphan_backup(path: &Path) {
        let BackupState::Valid(mut b) = read_backup(path) else {
            panic!("no backup at {}", path.display())
        };
        b.instance = "crashed-run".into();
        b.pid = 999_999;
        write_backup(path, &b).unwrap();
    }

    #[tokio::test]
    async fn ephemeral_run_backup_survives_its_data_dir_and_is_restored() {
        let dir = tempfile::tempdir().unwrap();
        let env = gnome_env(dir.path());
        let runner = MockRunner::new();
        with_own_process(&runner);
        let store = fake_gsettings(&runner, GNOME_DEFAULTS);
        // `--ephemeral`: a throwaway data dir that's deleted on exit (or by a reboot).
        let ephemeral = tempfile::tempdir().unwrap();
        let c = ctx(&ephemeral.path().join("akev-AbC123"));
        let first = SystemProxyInterceptor::with_store(
            Sys::new(runner.clone(), env.clone()),
            Arc::new(UnsupportedStore),
        );
        first.wait_startup_recovery().await;
        first.activate(&c, json!({})).await.unwrap();
        assert_eq!(store.lock()["org.gnome.system.proxy mode"], "'manual'");
        let j = journal(&first, &c);
        assert!(
            j.starts_with(dir.path().join("home/.local/share/akev/recovery")),
            "{}",
            j.display()
        );

        // Akev is killed and its temp dir disappears.
        orphan_backup(&j);
        drop(first);
        drop(ephemeral);
        assert!(!c.data_dir.exists());

        // The next start, whatever its data dir, puts the settings back.
        let next = SystemProxyInterceptor::with_store(
            Sys::new(runner.clone(), env),
            Arc::new(UnsupportedStore),
        );
        next.wait_startup_recovery().await;
        let s = store.lock().clone();
        for (schema, key, value) in GNOME_DEFAULTS {
            assert_eq!(s[&format!("{schema} {key}")], *value, "{schema} {key}");
        }
        assert!(!j.exists());
    }

    #[tokio::test]
    async fn activation_restores_settings_left_by_a_dead_run_of_another_data_dir() {
        let dir = tempfile::tempdir().unwrap();
        let env = gnome_env(dir.path());
        let runner = MockRunner::new();
        with_own_process(&runner);
        let store = fake_gsettings(&runner, GNOME_DEFAULTS);
        point_at_akev(&store);
        // Left by a dead ephemeral run (another data dir).
        let dead = journal_file(&env, Path::new("/tmp/akev-dead"), BACKUP_FILE);
        write_backup(&dead, &stale_backup(999_999, "akev")).unwrap();
        // Restoring fails at first (e.g. no D-Bus session yet).
        let refuse = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let r = refuse.clone();
        runner.on(move |spec| {
            let args = spec.args_lossy();
            (r.load(std::sync::atomic::Ordering::SeqCst)
                && spec.program_name() == "gsettings"
                && args[0] == "set"
                && args[2] == "mode"
                && args[3] == "'none'")
                .then(|| Ok(CommandOutput::failed("dconf error")))
        });
        // Built outside a runtime: no startup recovery, so activation must notice it itself.
        let sys = Sys::new(runner.clone(), env);
        let i = std::thread::spawn(move || {
            SystemProxyInterceptor::with_store(sys, Arc::new(UnsupportedStore))
        })
        .join()
        .unwrap();
        let c = ctx(&dir.path().join("other-data"));

        let err = i.activate(&c, json!({})).await.unwrap_err();
        assert!(err.to_string().contains("previous Akev run"), "{err}");
        assert!(!journal(&i, &c).exists(), "nothing snapshotted");
        assert!(dead.exists(), "kept for a later retry");

        refuse.store(false, std::sync::atomic::Ordering::SeqCst);
        i.activate(&c, json!({})).await.unwrap();
        assert!(!dead.exists());
        let BackupState::Valid(b) = read_backup(&journal(&i, &c)) else {
            panic!("no backup")
        };
        let Snapshot::Gnome { values } = &b.snapshots[0] else {
            panic!()
        };
        assert!(
            values
                .iter()
                .any(|v| v.key == "mode" && v.value == "'none'"),
            "the dead proxy must not be recorded as the previous setting: {values:?}"
        );
        i.deactivate(&c, json!({})).await.unwrap();
        assert_eq!(store.lock()["org.gnome.system.proxy mode"], "'none'");
        assert_eq!(store.lock()["org.gnome.system.proxy.http host"], "''");
    }

    /// A gsettings that can't reach dconf: `set` prints a warning, exits 0 and changes nothing.
    fn fake_gsettings_without_session_bus(runner: &MockRunner, store: &GsettingsDb) {
        let s = store.clone();
        runner.on(move |spec| {
            if spec.program_name() != "gsettings" {
                return None;
            }
            let args = spec.args_lossy();
            let key = format!("{} {}", args.get(1)?, args.get(2)?);
            Some(Ok(match args[0].as_str() {
                "get" => match s.lock().get(&key) {
                    Some(v) => CommandOutput::ok(format!("{v}\n")),
                    None => CommandOutput::failed("No such key"),
                },
                _ => CommandOutput {
                    code: Some(0),
                    stdout: String::new(),
                    stderr: "\n(process:4242): dconf-WARNING **: 10:00:00.000: failed to commit changes to dconf: Cannot autolaunch D-Bus without X11 $DISPLAY\n".into(),
                },
            }))
        });
    }

    #[tokio::test]
    async fn uncommitted_gsettings_restore_keeps_the_backup() {
        let dir = tempfile::tempdir().unwrap();
        let env = gnome_env(dir.path());
        let runner = MockRunner::new();
        with_own_process(&runner);
        let store = fake_gsettings(&runner, GNOME_DEFAULTS);
        point_at_akev(&store);
        fake_gsettings_without_session_bus(&runner, &store);
        let default_dir = dir.path().join("home/.local/share/akev");
        let path = backup_path(&default_dir);
        write_backup(&path, &stale_backup(999_999, "akev")).unwrap();

        // E.g. `akev capture` from cron: recovery runs but nothing can be written.
        let i = SystemProxyInterceptor::with_store(
            Sys::new(runner.clone(), env),
            Arc::new(UnsupportedStore),
        );
        i.wait_startup_recovery().await;
        assert_eq!(store.lock()["org.gnome.system.proxy mode"], "'manual'");
        let BackupState::Valid(kept) = read_backup(&path) else {
            panic!("the only record of the previous settings was deleted")
        };
        assert_eq!(kept.snapshots, stale_backup(999_999, "akev").snapshots);
    }

    #[tokio::test]
    async fn uncommitted_gsettings_activation_fails_cleanly() {
        let (_dir, runner, store, i, c) = gnome_setup();
        i.wait_startup_recovery().await;
        fake_gsettings_without_session_bus(&runner, &store);
        let err = i.activate(&c, json!({})).await.unwrap_err();
        assert!(err.to_string().contains("D-Bus"), "{err}");
        assert_eq!(store.lock()["org.gnome.system.proxy mode"], "'none'");
        // Nothing changed, so nothing is left to restore and nothing claims to be active.
        assert!(!journal(&i, &c).exists());
        assert!(!i.info(&c).await.active);
    }

    #[test]
    fn gvariant_comparison() {
        assert!(gvariant_eq("'manual'\n", "'manual'"));
        assert!(gvariant_eq("\"it's\"", &gvariant_str("it's")));
        assert!(gvariant_eq("@as []", "@as []"));
        assert!(!gvariant_eq("'none'", "'manual'"));
        assert!(!gvariant_eq("8000", "'8000'"));
    }

    #[tokio::test]
    async fn corrupt_backup_is_quarantined() {
        let (_dir, _runner, store, i, c) = gnome_setup();
        let j = journal(&i, &c);
        let journal_dir = j.parent().unwrap();
        std::fs::create_dir_all(journal_dir).unwrap();
        std::fs::write(&j, "{not json").unwrap();
        i.activate(&c, json!({})).await.unwrap();
        assert_eq!(store.lock()["org.gnome.system.proxy mode"], "'manual'");
        let prefix = format!("{}.corrupt-", j.file_name().unwrap().to_string_lossy());
        let aside = std::fs::read_dir(journal_dir)
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with(&prefix));
        assert!(aside);
        i.deactivate(&c, json!({})).await.unwrap();
    }

    #[tokio::test]
    async fn kde_backend_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        make_exe(&dir.path().join("usr/bin/kwriteconfig6"), "");
        make_exe(&dir.path().join("usr/bin/dbus-send"), "");
        let env = fake_env(Os::Linux, dir.path()).with_var("XDG_CURRENT_DESKTOP", "KDE");
        let cfg = native(dir.path(), "home/.config/kioslaverc");
        std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();
        std::fs::write(&cfg, "[Proxy Settings]\nProxyType=0\nNoProxyFor=corp\n").unwrap();
        let runner = MockRunner::new();
        with_own_process(&runner);
        // A kwriteconfig that really edits the (single-group) file.
        runner.on(|spec| {
            if spec.program_name() != "kwriteconfig6" {
                return None;
            }
            let a = spec.args_lossy();
            let file = PathBuf::from(&a[1]);
            let content = std::fs::read_to_string(&file).unwrap_or_default();
            let mut map: std::collections::BTreeMap<String, String> = content
                .lines()
                .filter_map(|l| {
                    l.split_once('=')
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                })
                .collect();
            if a[6] == "--delete" {
                map.remove(&a[5]);
            } else {
                map.insert(a[5].clone(), a[6].clone());
            }
            let mut out = format!("[{}]\n", a[3]);
            for (k, v) in map {
                out.push_str(&format!("{k}={v}\n"));
            }
            std::fs::write(&file, out).unwrap();
            Some(Ok(CommandOutput::ok("")))
        });
        runner.on_prefix("dbus-send", CommandOutput::ok(""));
        let i = SystemProxyInterceptor::with_store(
            Sys::new(runner.clone(), env),
            Arc::new(UnsupportedStore),
        );
        let c = ctx(&dir.path().join("data"));
        i.activate(&c, json!({})).await.unwrap();
        let file = cfg.to_string_lossy().into_owned();
        let lines = runner.call_lines();
        assert!(lines.contains(&format!("kwriteconfig6 --file {file} --group Proxy Settings --key httpProxy http://127.0.0.1 8000")));
        assert!(lines.contains(&format!(
            "kwriteconfig6 --file {file} --group Proxy Settings --key ProxyType 1"
        )));
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("dbus-send --session --type=signal /KIO/Scheduler"))
        );

        assert!(
            std::fs::read_to_string(&cfg)
                .unwrap()
                .contains("httpProxy=http://127.0.0.1 8000")
        );

        i.deactivate(&c, json!({})).await.unwrap();
        let lines = runner.call_lines();
        assert!(lines.contains(&format!(
            "kwriteconfig6 --file {file} --group Proxy Settings --key ProxyType 0"
        )));
        assert!(lines.contains(&format!(
            "kwriteconfig6 --file {file} --group Proxy Settings --key NoProxyFor corp"
        )));
        assert!(lines.contains(&format!(
            "kwriteconfig6 --file {file} --group Proxy Settings --key httpProxy --delete"
        )));
        assert_eq!(
            std::fs::read_to_string(&cfg).unwrap(),
            "[Proxy Settings]\nNoProxyFor=corp\nProxyType=0\n"
        );
    }

    #[tokio::test]
    async fn macos_backend_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        make_exe(&dir.path().join("usr/sbin/networksetup"), "");
        let runner = MockRunner::new();
        with_own_process(&runner);
        runner.on_prefix(
            "networksetup -listallnetworkservices",
            CommandOutput::ok(
                "An asterisk (*) denotes that a network service is disabled.\nWi-Fi\n*iPhone USB\n",
            ),
        );
        runner.on_prefix(
            "networksetup -getwebproxy Wi-Fi",
            CommandOutput::ok("Enabled: No\nServer: \nPort: 0\nAuthenticated Proxy Enabled: 0\n"),
        );
        runner.on_prefix(
            "networksetup -getsecurewebproxy Wi-Fi",
            CommandOutput::ok(
                "Enabled: Yes\nServer: corp\nPort: 3128\nAuthenticated Proxy Enabled: 0\n",
            ),
        );
        runner.on_prefix(
            "networksetup -getautoproxyurl Wi-Fi",
            CommandOutput::ok("URL: http://wpad/p.pac\nEnabled: Yes\n"),
        );
        runner.on_prefix("networksetup -set", CommandOutput::ok(""));
        let i = SystemProxyInterceptor::with_store(
            Sys::new(runner.clone(), fake_env(Os::MacOs, dir.path())),
            Arc::new(UnsupportedStore),
        );
        let c = ctx(&dir.path().join("data"));
        i.activate(&c, json!({})).await.unwrap();
        let lines = runner.call_lines();
        assert!(lines.contains(&"networksetup -setwebproxy Wi-Fi 127.0.0.1 8000".to_string()));
        assert!(
            lines.contains(&"networksetup -setsecurewebproxy Wi-Fi 127.0.0.1 8000".to_string())
        );
        assert!(lines.contains(&"networksetup -setautoproxystate Wi-Fi off".to_string()));
        assert!(
            !lines.iter().any(|l| l.contains("iPhone USB")),
            "disabled services untouched"
        );
        let n = lines.len();

        // What networksetup reports now.
        runner.on_prefix(
            "networksetup -getwebproxy Wi-Fi",
            CommandOutput::ok(
                "Enabled: Yes\nServer: 127.0.0.1\nPort: 8000\nAuthenticated Proxy Enabled: 0\n",
            ),
        );
        i.deactivate(&c, json!({})).await.unwrap();
        let restore = &runner.call_lines()[n..];
        assert!(restore.contains(&"networksetup -setwebproxystate Wi-Fi off".to_string()));
        assert!(restore.contains(&"networksetup -setsecurewebproxy Wi-Fi corp 3128".to_string()));
        assert!(!restore.contains(&"networksetup -setsecurewebproxystate Wi-Fi off".to_string()));
        assert!(restore.contains(&"networksetup -setautoproxystate Wi-Fi on".to_string()));

        // `networksetup` errors printed with exit code 0 are failures.
        runner.on_prefix(
            "networksetup -getwebproxy Wi-Fi",
            CommandOutput::ok("Enabled: No\nServer: \nPort: 0\nAuthenticated Proxy Enabled: 0\n"),
        );
        runner.on_prefix(
            "networksetup -setwebproxy Wi-Fi 127.0.0.1",
            CommandOutput::ok("** Error: requires admin privileges"),
        );
        assert!(i.activate(&c, json!({})).await.is_err());
        assert!(!journal(&i, &c).exists(), "rolled back");
    }

    struct FakeStore {
        state: Mutex<WinInetSettings>,
        writes: Mutex<usize>,
    }

    impl InternetSettingsStore for FakeStore {
        fn read(&self) -> io::Result<WinInetSettings> {
            Ok(self.state.lock().clone())
        }
        fn write(&self, settings: &WinInetSettings) -> io::Result<()> {
            *self.state.lock() = settings.clone();
            *self.writes.lock() += 1;
            Ok(())
        }
    }

    #[tokio::test]
    async fn windows_backend_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let before = WinInetSettings {
            proxy_enable: Some(0),
            proxy_server: Some("corp:8080".into()),
            proxy_override: None,
            auto_config_url: Some("http://wpad/wpad.dat".into()),
        };
        let store = Arc::new(FakeStore {
            state: Mutex::new(before.clone()),
            writes: Mutex::new(0),
        });
        let runner = MockRunner::new();
        with_own_process(&runner);
        let env = HostEnv::empty(Os::Windows)
            .with_var("APPDATA", dir.path().join("roaming").to_string_lossy())
            .with_var("LOCALAPPDATA", dir.path().join("local").to_string_lossy());
        let i = SystemProxyInterceptor::with_store(Sys::new(runner, env), store.clone());
        let c = ctx(&dir.path().join("data"));
        let info = i.info(&c).await;
        assert!(info.available);
        assert!(
            info.metadata["trust"]["options"][0]["install"][0]
                .as_str()
                .unwrap()
                .starts_with("certutil -user -addstore Root")
        );

        i.activate(&c, json!({})).await.unwrap();
        assert_eq!(*store.state.lock(), windows_target("127.0.0.1", 8000));
        i.deactivate_all(&c).await.unwrap();
        assert_eq!(*store.state.lock(), before);
        assert_eq!(*store.writes.lock(), 2);
    }

    #[tokio::test]
    async fn unavailable_without_backends() {
        let dir = tempfile::tempdir().unwrap();
        let i = SystemProxyInterceptor::with_store(
            Sys::new(MockRunner::new(), fake_env(Os::Linux, dir.path())),
            Arc::new(UnsupportedStore),
        );
        let c = ctx(&dir.path().join("data"));
        let info = i.info(&c).await;
        assert!(info.supported && !info.available);
        assert!(info.unavailable_reason.unwrap().contains("gsettings"));
        assert!(matches!(
            i.activate(&c, json!({})).await,
            Err(InterceptError::Unavailable(_))
        ));
        let other = SystemProxyInterceptor::with_store(
            Sys::new(MockRunner::new(), HostEnv::empty(Os::Other)),
            Arc::new(UnsupportedStore),
        );
        assert!(!other.info(&c).await.supported);
    }

    #[test]
    fn trust_instructions_per_os() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = ctx(dir.path());
        c.ca_path = PathBuf::from("/home/me/my data/ca.pem");
        let linux = trust_instructions(&c, Os::Linux);
        let text = linux.to_string();
        assert!(text.contains("sudo trust anchor --store '/home/me/my data/ca.pem'"));
        assert!(text.contains("update-ca-certificates"));
        // The same nickname the trust panel looks for.
        let nickname = posix_quote(&crate::trust::nss_nickname(&c.ca_der));
        assert!(nickname.starts_with("'Akev CA "), "{nickname}");
        assert!(text.contains(&format!(
            "certutil -d sql:$HOME/.pki/nssdb -A -t C,, -n {nickname} -i"
        )));
        assert!(text.contains(&format!(
            "certutil -d sql:$HOME/.pki/nssdb -D -n {nickname}"
        )));
        let mac = trust_instructions(&c, Os::MacOs).to_string();
        assert!(mac.contains("security add-trusted-cert"));
        assert!(mac.contains("remove-trusted-cert"));
        let win = trust_instructions(&c, Os::Windows);
        assert_eq!(win["caSha256"].as_str().unwrap().len(), 64);
        assert!(win.to_string().contains("GetCertHashString('SHA256')"));
    }
}
