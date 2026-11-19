//! On-disk state in the data directory: `settings.json`, `rules.json`, `daemon.json`.
//!
//! * Files are written atomically (temp file + rename) so a crash never leaves a half-written
//!   file behind.
//! * Unparseable files are moved aside to `<name>.bak` and replaced by defaults, so a corrupt
//!   or incompatible file never prevents startup (and the user's data is not silently lost).
//! * Files that may contain secrets (`settings.json` can hold client-certificate passphrases,
//!   `daemon.json` holds the API token) are created with mode 0600 on Unix.

use parking_lot::Mutex;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Name of the settings file in the data directory.
pub const SETTINGS_FILE: &str = "settings.json";
/// Name of the rules file in the data directory.
pub const RULES_FILE: &str = "rules.json";
/// Name of the daemon discovery file in the data directory.
pub const DAEMON_FILE: &str = "daemon.json";

/// Outcome of loading a JSON state file.
#[derive(Debug, PartialEq)]
pub enum LoadOutcome {
    /// The file was read and parsed.
    Loaded,
    /// The file didn't exist; defaults are used.
    Missing,
    /// The file was corrupt; it was moved to the contained backup path and defaults are used.
    Recovered(PathBuf),
    /// The file couldn't be read (permissions...). Defaults are used and the file is left alone.
    Unreadable(String),
}

/// Load a JSON file, falling back to `T::default()`. Corrupt files are backed up as
/// `<file>.bak` and replaced with the defaults.
pub fn load_json_or_default<T>(path: &Path) -> (T, LoadOutcome)
where
    T: DeserializeOwned + Serialize + Default,
{
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return (T::default(), LoadOutcome::Missing);
        }
        Err(e) => return (T::default(), LoadOutcome::Unreadable(e.to_string())),
    };
    match serde_json::from_slice::<T>(&bytes) {
        Ok(v) => (v, LoadOutcome::Loaded),
        Err(err) => {
            let backup = backup_path(path);
            if let Err(e) = std::fs::rename(path, &backup) {
                tracing::warn!("failed to back up corrupt {}: {e}", path.display());
            }
            let value = T::default();
            if let Err(e) = write_json_atomic(path, &value, file_mode_for(path)) {
                tracing::warn!("failed to write default {}: {e}", path.display());
            }
            tracing::warn!(
                "{} was invalid ({err}); moved to {} and replaced with defaults",
                path.display(),
                backup.display()
            );
            (value, LoadOutcome::Recovered(backup))
        }
    }
}

/// `settings.json` → `settings.json.bak`
pub fn backup_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".bak");
    path.with_file_name(name)
}

fn file_mode_for(path: &Path) -> u32 {
    match path.file_name().and_then(|n| n.to_str()) {
        Some(RULES_FILE) => 0o644,
        _ => 0o600,
    }
}

/// Serialize `value` as pretty JSON and write it atomically with the given Unix mode.
pub fn write_json_atomic<T: Serialize + ?Sized>(
    path: &Path,
    value: &T,
    mode: u32,
) -> std::io::Result<()> {
    write_atomic(path, &pretty_json(value)?, mode)
}

/// Write bytes atomically: write a sibling temp file (created with `mode`), fsync, rename.
pub fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> std::io::Result<()> {
    write_atomic_with(path, mode, |f| f.write_all(bytes))
}

/// Like [`write_atomic`], with the content written by `write` (e.g. streamed). On error the
/// temp file is removed and `path` is left untouched.
pub fn write_atomic_with<T>(
    path: &Path,
    mode: u32,
    write: impl FnOnce(&mut std::fs::File) -> std::io::Result<T>,
) -> std::io::Result<T> {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(
        ".{file_name}.{}.tmp",
        uuid::Uuid::new_v4().simple()
    ));
    let result = (|| {
        let mut f = open_with_mode(&tmp, mode)?;
        let value = write(&mut f)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)?;
        Ok(value)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(unix)]
fn open_with_mode(path: &Path, mode: u32) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)
}

#[cfg(not(unix))]
fn open_with_mode(path: &Path, _mode: u32) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

/// Persist a settings/rules value to `dir/name` (0600 for settings, 0644 for rules).
#[cfg(test)]
pub fn save_state<T: Serialize + ?Sized>(dir: &Path, name: &str, value: &T) -> std::io::Result<()> {
    let path = dir.join(name);
    write_json_atomic(&path, value, file_mode_for(&path))
}

/// A state file written in the background: saves are serialized on the caller's thread,
/// written atomically on the blocking pool (never fsyncing on an async worker), applied in
/// order, and coalesced — when saves are requested faster than the disk keeps up, only the
/// newest snapshot is written.
#[derive(Debug)]
pub struct StateFile {
    path: PathBuf,
    mode: u32,
    /// Generation of the newest requested snapshot.
    requested: AtomicU64,
    /// Generation of the snapshot on disk; held while writing so writes never interleave.
    written: Mutex<u64>,
}

impl StateFile {
    /// A state file `dir/name` (0600 for settings, 0644 for rules).
    pub fn new(dir: &Path, name: &str) -> Arc<StateFile> {
        let path = dir.join(name);
        let mode = file_mode_for(&path);
        Arc::new(StateFile {
            path,
            mode,
            requested: AtomicU64::new(0),
            written: Mutex::new(0),
        })
    }

    /// Serialize `value` now and write it in the background (synchronously when called
    /// outside a tokio runtime).
    pub fn save<T: Serialize + ?Sized>(self: &Arc<Self>, value: &T) {
        let bytes = match pretty_json(value) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("failed to serialize {}: {e}", self.path.display());
                return;
            }
        };
        let generation = self.requested.fetch_add(1, Ordering::AcqRel) + 1;
        match tokio::runtime::Handle::try_current() {
            Ok(rt) => {
                let file = self.clone();
                rt.spawn_blocking(move || {
                    // Superseded before we got to run: the newer save writes instead.
                    if file.requested.load(Ordering::Acquire) == generation {
                        file.write(generation, &bytes);
                    }
                });
            }
            Err(_) => self.write(generation, &bytes),
        }
    }

    /// Write `value` synchronously (used on shutdown), superseding pending saves.
    pub fn save_blocking<T: Serialize + ?Sized>(&self, value: &T) -> std::io::Result<()> {
        let bytes = pretty_json(value)?;
        let generation = self.requested.fetch_add(1, Ordering::AcqRel) + 1;
        let mut written = self.written.lock();
        write_atomic(&self.path, &bytes, self.mode)?;
        *written = generation.max(*written);
        Ok(())
    }

    /// True when the newest requested snapshot is on disk (no pending or failed save).
    pub fn is_clean(&self) -> bool {
        *self.written.lock() >= self.requested.load(Ordering::Acquire)
    }

    fn write(&self, generation: u64, bytes: &[u8]) {
        let mut written = self.written.lock();
        if *written >= generation {
            return; // a newer snapshot is already on disk
        }
        match write_atomic(&self.path, bytes, self.mode) {
            Ok(()) => *written = generation,
            Err(e) => tracing::warn!("failed to save {}: {e}", self.path.display()),
        }
    }
}

fn pretty_json<T: Serialize + ?Sized>(value: &T) -> std::io::Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// Default Akev data directory: `~/.local/share/akev` on Linux, `~/Library/Application
/// Support/akev` on macOS and `%LOCALAPPDATA%\akev` on Windows (not the roaming profile, which
/// domain profiles copy between machines — a bad place for the CA key and spilled traffic).
pub fn default_data_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        dirs::data_local_dir().map(|d| d.join("akev"))
    }
    #[cfg(not(windows))]
    {
        dirs::data_dir().map(|d| d.join("akev"))
    }
}

/// Contents of `daemon.json`, written by a running daemon so the CLI (`akev status`,
/// `akev stop`, `akev mcp`, ...) can discover and authenticate to it.
#[derive(Clone, Debug, PartialEq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonInfo {
    /// Process id of the daemon.
    pub pid: u32,
    /// Base URL of its API, e.g. `http://127.0.0.1:45456`.
    pub api_url: String,
    /// API token.
    pub token: String,
    /// Current proxy port.
    pub proxy_port: u16,
    /// Akev version of the daemon.
    pub version: String,
}

impl DaemonInfo {
    /// Path of `daemon.json` inside a data directory.
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join(DAEMON_FILE)
    }

    /// Read `daemon.json` from a data directory (None if absent or unparseable).
    pub fn read(data_dir: &Path) -> Option<DaemonInfo> {
        let bytes = std::fs::read(Self::path(data_dir)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Write `daemon.json` (mode 0600: it contains the API token).
    pub fn write(&self, data_dir: &Path) -> std::io::Result<()> {
        write_json_atomic(&Self::path(data_dir), self, 0o600)
    }

    /// Remove `daemon.json` if it still describes this daemon (same pid & token) — so a
    /// second daemon that took over discovery isn't clobbered.
    pub fn remove_if_owned(&self, data_dir: &Path) -> std::io::Result<()> {
        match Self::read(data_dir) {
            Some(current) if current.pid == self.pid && current.token == self.token => {
                std::fs::remove_file(Self::path(data_dir))
            }
            _ => Ok(()),
        }
    }
}

/// Whether a process with this pid is running (best effort; `false` when unknown).
pub fn process_alive(pid: u32) -> bool {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
    let pid = Pid::from_u32(pid);
    let Ok(mut system) = System::new() else {
        return false;
    };
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing(),
    );
    system.process(pid).is_some()
}

/// Whether something accepts TCP connections at the host/port of an API URL (a stale
/// `daemon.json` whose pid was reused by an unrelated process fails this).
pub fn api_listening(api_url: &str) -> bool {
    use std::net::ToSocketAddrs;
    let Ok(url) = url::Url::parse(api_url) else {
        return false;
    };
    let (Some(host), Some(port)) = (url.host_str(), url.port_or_known_default()) else {
        return false;
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let Ok(mut addrs) = (host, port).to_socket_addrs() else {
        return false;
    };
    addrs.any(|addr| {
        std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(300)).is_ok()
    })
}

/// Create a directory (and parents), restricting it to the current user on Unix when we
/// created it.
pub fn ensure_dir(dir: &Path) -> std::io::Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use akev_core::rules::RuleSet;
    use akev_core::settings::Settings;

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let (s, outcome): (Settings, _) = load_json_or_default(&dir.path().join(SETTINGS_FILE));
        assert_eq!(s, Settings::default());
        assert_eq!(outcome, LoadOutcome::Missing);
    }

    #[test]
    fn partial_settings_use_serde_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SETTINGS_FILE);
        std::fs::write(&path, r#"{"proxy":{"port":9123},"futureField":true}"#).unwrap();
        let (s, outcome): (Settings, _) = load_json_or_default(&path);
        assert_eq!(outcome, LoadOutcome::Loaded);
        assert_eq!(s.proxy.port, 9123);
        assert_eq!(s.proxy.bind_address, "127.0.0.1");
        assert!(s.proxy.http2);
        assert_eq!(s.capture, Default::default());
    }

    #[test]
    fn corrupt_file_is_backed_up_and_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(RULES_FILE);
        std::fs::write(&path, b"{ this is not json").unwrap();
        let (rules, outcome): (RuleSet, _) = load_json_or_default(&path);
        assert_eq!(rules, RuleSet::default());
        let backup = dir.path().join("rules.json.bak");
        assert_eq!(outcome, LoadOutcome::Recovered(backup.clone()));
        assert_eq!(std::fs::read(&backup).unwrap(), b"{ this is not json");
        // replaced by valid defaults
        let reread: RuleSet = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(reread, RuleSet::default());
    }

    #[test]
    fn save_and_reload_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Settings::default();
        s.proxy.port = 8123;
        s.ui = serde_json::json!({"theme": "dark"});
        save_state(dir.path(), SETTINGS_FILE, &s).unwrap();
        let (back, outcome): (Settings, _) = load_json_or_default(&dir.path().join(SETTINGS_FILE));
        assert_eq!(outcome, LoadOutcome::Loaded);
        assert_eq!(back, s);
        // no temp files left behind
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn secret_files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        save_state(dir.path(), SETTINGS_FILE, &Settings::default()).unwrap();
        let mode = std::fs::metadata(dir.path().join(SETTINGS_FILE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        let info = DaemonInfo {
            pid: 1,
            api_url: "http://127.0.0.1:1".into(),
            token: "t".into(),
            proxy_port: 2,
            version: "0".into(),
        };
        info.write(dir.path()).unwrap();
        let mode = std::fs::metadata(DaemonInfo::path(dir.path()))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn daemon_info_remove_only_when_owned() {
        let dir = tempfile::tempdir().unwrap();
        let ours = DaemonInfo {
            pid: 42,
            api_url: "http://127.0.0.1:45456".into(),
            token: "abc".into(),
            proxy_port: 8000,
            version: "0.1.0".into(),
        };
        ours.write(dir.path()).unwrap();
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(DaemonInfo::path(dir.path())).unwrap()).unwrap();
        assert_eq!(json["apiUrl"], "http://127.0.0.1:45456");
        assert_eq!(json["proxyPort"], 8000);
        assert_eq!(DaemonInfo::read(dir.path()), Some(ours.clone()));

        let other = DaemonInfo {
            pid: 43,
            token: "zzz".into(),
            ..ours.clone()
        };
        other.write(dir.path()).unwrap();
        ours.remove_if_owned(dir.path()).unwrap();
        assert_eq!(DaemonInfo::read(dir.path()), Some(other.clone()));
        other.remove_if_owned(dir.path()).unwrap();
        assert_eq!(DaemonInfo::read(dir.path()), None);
        // removing again is a no-op
        other.remove_if_owned(dir.path()).unwrap();
    }

    fn port_on_disk(dir: &Path) -> u16 {
        let s: Settings =
            serde_json::from_slice(&std::fs::read(dir.join(SETTINGS_FILE)).unwrap()).unwrap();
        s.proxy.port
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn state_file_coalesces_and_keeps_the_newest() {
        let dir = tempfile::tempdir().unwrap();
        let file = StateFile::new(dir.path(), SETTINGS_FILE);
        assert!(file.is_clean(), "nothing requested yet");
        let mut s = Settings::default();
        for port in 1000..1100 {
            s.proxy.port = port;
            file.save(&s);
        }
        for _ in 0..500 {
            if file.is_clean() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert!(file.is_clean());
        assert_eq!(port_on_disk(dir.path()), 1099);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join(SETTINGS_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn state_file_without_runtime_writes_synchronously() {
        let dir = tempfile::tempdir().unwrap();
        let file = StateFile::new(dir.path(), SETTINGS_FILE);
        let mut s = Settings::default();
        s.proxy.port = 4242;
        file.save(&s);
        assert!(file.is_clean());
        assert_eq!(port_on_disk(dir.path()), 4242);
        s.proxy.port = 4343;
        file.save_blocking(&s).unwrap();
        assert_eq!(port_on_disk(dir.path()), 4343);
    }

    #[test]
    fn failed_saves_stay_dirty() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist");
        let file = StateFile::new(&missing, RULES_FILE);
        file.save(&RuleSet::default());
        assert!(
            !file.is_clean(),
            "a failed save must be retried on shutdown"
        );
        assert!(file.save_blocking(&RuleSet::default()).is_err());
        std::fs::create_dir(&missing).unwrap();
        file.save_blocking(&RuleSet::default()).unwrap();
        assert!(file.is_clean());
    }

    #[test]
    fn process_liveness() {
        assert!(process_alive(std::process::id()));
        // Pids are bounded well below u32::MAX on every supported OS.
        assert!(!process_alive(u32::MAX - 7));
    }

    #[test]
    fn api_liveness() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(api_listening(&format!("http://127.0.0.1:{port}")));
        drop(listener);
        assert!(!api_listening(&format!("http://127.0.0.1:{port}")));
        assert!(!api_listening("not a url"));
    }

    #[test]
    fn backup_path_appends_suffix() {
        assert_eq!(
            backup_path(Path::new("/x/settings.json")),
            PathBuf::from("/x/settings.json.bak")
        );
    }
}
