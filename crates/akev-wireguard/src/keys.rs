//! Server and client key pairs (Curve25519), generated once and persisted in the data
//! directory as `wireguard.json` (owner-only permissions on Unix).
//!
//! Both private keys live on this machine: the client's is handed to the device inside the
//! client configuration (QR code / `.conf` file). Regenerating replaces both pairs, which
//! invalidates every configuration handed out before.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use boringtun::x25519::{PublicKey, StaticSecret};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// File name of the persisted keys inside the data directory.
pub const KEYS_FILE: &str = "wireguard.json";

/// A Curve25519 private key (clamped like `wg genkey`).
#[derive(Clone, PartialEq, Eq)]
pub struct PrivateKey([u8; 32]);

impl PrivateKey {
    /// A fresh random key.
    pub fn generate() -> Self {
        let mut bytes = [0u8; 32];
        rand::Rng::fill(&mut rand::rng(), &mut bytes[..]);
        Self::from_bytes(bytes)
    }

    /// Wrap raw key bytes (clamped).
    pub fn from_bytes(mut bytes: [u8; 32]) -> Self {
        bytes[0] &= 248;
        bytes[31] &= 127;
        bytes[31] |= 64;
        PrivateKey(bytes)
    }

    /// Parse the base64 form used in WireGuard configurations.
    pub fn from_base64(s: &str) -> Option<Self> {
        decode_key(s).map(Self::from_bytes)
    }

    /// The base64 form used in WireGuard configurations.
    pub fn to_base64(&self) -> String {
        STANDARD.encode(self.0)
    }

    /// The matching public key.
    pub fn public_key(&self) -> PublicKeyBytes {
        PublicKeyBytes(PublicKey::from(&self.secret()).to_bytes())
    }

    /// The key as boringtun's type.
    pub fn secret(&self) -> StaticSecret {
        StaticSecret::from(self.0)
    }
}

impl std::fmt::Debug for PrivateKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PrivateKey(<redacted>)")
    }
}

/// A Curve25519 public key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PublicKeyBytes(pub [u8; 32]);

impl PublicKeyBytes {
    /// Parse the base64 form.
    pub fn from_base64(s: &str) -> Option<Self> {
        decode_key(s).map(PublicKeyBytes)
    }

    /// The base64 form used in WireGuard configurations.
    pub fn to_base64(&self) -> String {
        STANDARD.encode(self.0)
    }

    /// The key as boringtun's type.
    pub fn key(&self) -> PublicKey {
        PublicKey::from(self.0)
    }
}

fn decode_key(s: &str) -> Option<[u8; 32]> {
    let raw = STANDARD.decode(s.trim()).ok()?;
    raw.try_into().ok()
}

/// The server's and the (single) client's key pairs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireguardKeys {
    /// Akev's own key (its public half goes into the client's `[Peer]` section).
    pub server: PrivateKey,
    /// The device's key (goes into the client's `[Interface]` section).
    pub client: PrivateKey,
    /// When these keys were generated (unix ms).
    pub created_at: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KeysFile {
    version: u32,
    server_private_key: String,
    client_private_key: String,
    created_at: u64,
}

/// Errors reading or writing the key file.
#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    /// Filesystem error.
    #[error("failed to access {path}: {source}")]
    Io {
        /// The key file.
        path: PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
}

impl WireguardKeys {
    /// Two fresh key pairs.
    pub fn generate() -> Self {
        WireguardKeys {
            server: PrivateKey::generate(),
            client: PrivateKey::generate(),
            created_at: now_ms(),
        }
    }

    /// Load `dir/wireguard.json`, or generate and save new keys when it is missing. A file
    /// that can't be parsed is moved aside (`wireguard.json.invalid`) and replaced.
    pub fn load_or_generate(dir: &Path) -> Result<Self, KeyError> {
        let path = dir.join(KEYS_FILE);
        match std::fs::read(&path) {
            Ok(raw) => match Self::parse(&raw) {
                Some(keys) => {
                    restrict_permissions(&path);
                    return Ok(keys);
                }
                None => {
                    let aside = dir.join(format!("{KEYS_FILE}.invalid"));
                    tracing::warn!(
                        "{} is not a valid WireGuard key file; moving it to {} and generating new keys",
                        path.display(),
                        aside.display()
                    );
                    let _ = std::fs::rename(&path, &aside);
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => return Err(KeyError::Io { path, source }),
        }
        let keys = Self::generate();
        keys.save(dir)?;
        Ok(keys)
    }

    /// Generate new key pairs and save them over the old ones.
    pub fn regenerate(dir: &Path) -> Result<Self, KeyError> {
        let keys = Self::generate();
        keys.save(dir)?;
        Ok(keys)
    }

    fn parse(raw: &[u8]) -> Option<Self> {
        let file: KeysFile = serde_json::from_slice(raw).ok()?;
        Some(WireguardKeys {
            server: PrivateKey::from_base64(&file.server_private_key)?,
            client: PrivateKey::from_base64(&file.client_private_key)?,
            created_at: file.created_at,
        })
    }

    /// Write `dir/wireguard.json` atomically (temp file + rename), readable by the owner only.
    pub fn save(&self, dir: &Path) -> Result<(), KeyError> {
        let path = dir.join(KEYS_FILE);
        let file = KeysFile {
            version: 1,
            server_private_key: self.server.to_base64(),
            client_private_key: self.client.to_base64(),
            created_at: self.created_at,
        };
        let tmp = dir.join(format!(".{KEYS_FILE}.tmp"));
        let io = |source| KeyError::Io {
            path: path.clone(),
            source,
        };
        let json = serde_json::to_vec_pretty(&file).map_err(|e| io(std::io::Error::other(e)))?;
        write_private(&tmp, &json).map_err(io)?;
        std::fs::rename(&tmp, &path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            io(e)
        })?;
        Ok(())
    }
}

#[cfg(unix)]
fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let _ = std::fs::remove_file(path);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(data)?;
    f.sync_all()
}

#[cfg(not(unix))]
fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    // The data directory is already per-user on Windows (%LOCALAPPDATA%).
    std::fs::write(path, data)
}

/// Tighten a key file written by something else (e.g. restored from a backup).
fn restrict_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path)
            && meta.permissions().mode() & 0o077 != 0
        {
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_clamped_and_round_trip() {
        let k = PrivateKey::generate();
        let b64 = k.to_base64();
        assert_eq!(b64.len(), 44);
        let back = PrivateKey::from_base64(&b64).unwrap();
        assert_eq!(back, k);
        assert_eq!(k.0[0] & 7, 0);
        assert_eq!(k.0[31] & 0x80, 0);
        assert_eq!(k.0[31] & 0x40, 0x40);
        assert!(PrivateKey::from_base64("nope").is_none());
        assert!(PrivateKey::from_base64(&STANDARD.encode([1u8; 31])).is_none());
    }

    #[test]
    fn public_key_matches_rfc7748_vector() {
        // RFC 7748 §6.1 (Alice): X25519 clamps the scalar itself, so clamping on load
        // doesn't change the public key.
        let k = PrivateKey::from_base64("dwdtCnMYpX08FsFyUbJmRd9ML4frwJkqsXf7pR25LCo=").unwrap();
        assert_eq!(
            k.public_key().to_base64(),
            "hSDwCYkwp1R0i33ctD73Wg2/Og0mOBr066SpjqqbTmo="
        );
        let p =
            PublicKeyBytes::from_base64("hSDwCYkwp1R0i33ctD73Wg2/Og0mOBr066SpjqqbTmo=").unwrap();
        assert_eq!(p, k.public_key());
    }

    #[test]
    fn persisted_once_and_reloaded() {
        let dir = tempfile::tempdir().unwrap();
        let a = WireguardKeys::load_or_generate(dir.path()).unwrap();
        let b = WireguardKeys::load_or_generate(dir.path()).unwrap();
        assert_eq!(a, b, "keys must be stable across restarts");
        assert_ne!(a.server, a.client);
        let raw = std::fs::read_to_string(dir.path().join(KEYS_FILE)).unwrap();
        assert!(raw.contains(&a.server.to_base64()));
        assert!(raw.contains("clientPrivateKey"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join(KEYS_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let c = WireguardKeys::regenerate(dir.path()).unwrap();
        assert_ne!(c.server, a.server);
        assert_ne!(c.client, a.client);
        assert_eq!(WireguardKeys::load_or_generate(dir.path()).unwrap(), c);
        assert!(!dir.path().join(format!(".{KEYS_FILE}.tmp")).exists());
    }

    #[test]
    fn invalid_file_is_replaced_and_kept_aside() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(KEYS_FILE), b"{ not json").unwrap();
        let keys = WireguardKeys::load_or_generate(dir.path()).unwrap();
        assert!(dir.path().join("wireguard.json.invalid").exists());
        assert_eq!(WireguardKeys::load_or_generate(dir.path()).unwrap(), keys);
    }

    #[cfg(unix)]
    #[test]
    fn loose_permissions_are_tightened() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let keys = WireguardKeys::load_or_generate(dir.path()).unwrap();
        let path = dir.path().join(KEYS_FILE);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(WireguardKeys::load_or_generate(dir.path()).unwrap(), keys);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn debug_output_redacts_private_keys() {
        let keys = WireguardKeys::generate();
        let dbg = format!("{keys:?}");
        assert!(!dbg.contains(&keys.server.to_base64()));
        assert!(dbg.contains("redacted"));
    }
}
