//! One Akev instance per data directory.
//!
//! The desktop app and the `akev` daemon both read and write `settings.json`, `rules.json`
//! and the autosave in the data directory; two of them on the same directory would clobber
//! each other. [`DataDirLock::acquire`] takes an exclusive lock on `akev.lock` (an OS file
//! lock, released automatically when the process exits, even after a crash) and describes the
//! holder in `instance.json` next to it — a separate file because Windows doesn't let other
//! processes read a locked file. A second instance fails with an error naming the first.
//! Ephemeral sessions (`--ephemeral`) don't take the lock.

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

/// The lock file in the data directory.
pub const LOCK_FILE: &str = "akev.lock";
/// Describes the instance holding the lock.
pub const INSTANCE_FILE: &str = "instance.json";

/// Contents of `instance.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceInfo {
    /// Process id of the holder.
    pub pid: u32,
    /// `desktop app` or `command line`.
    pub kind: String,
    /// Akev version of the holder.
    pub version: String,
    /// The holder's API URL, once it is listening.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_url: Option<String>,
    /// When the holder started (RFC 3339, UTC).
    pub started_at: String,
}

impl InstanceInfo {
    fn current(desktop: bool) -> Self {
        InstanceInfo {
            pid: std::process::id(),
            kind: instance_kind(desktop).to_string(),
            version: akev_core::VERSION.to_string(),
            api_url: None,
            started_at: now_rfc3339(),
        }
    }
}

/// What kind of program this is, for the "already running" message.
fn instance_kind(desktop: bool) -> &'static str {
    if desktop {
        "desktop app"
    } else {
        "command line"
    }
}

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .replace_nanosecond(0)
        .ok()
        .and_then(|t| {
            t.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_default()
}

/// Why the data directory couldn't be locked.
#[derive(Debug, thiserror::Error)]
pub enum LockError {
    /// Another instance holds the lock.
    #[error("{}", held_message(dir, holder.as_deref(), instance_kind(*desktop)))]
    Held {
        dir: PathBuf,
        holder: Option<Box<InstanceInfo>>,
        /// This instance is the desktop app.
        desktop: bool,
    },
    /// The lock file couldn't be opened or locked.
    #[error("couldn't lock the data directory {}: {source}", dir.display())]
    Io { dir: PathBuf, source: io::Error },
}

/// `this_kind`: what we are (the desktop app has no `--data-dir` to suggest).
fn held_message(dir: &Path, holder: Option<&InstanceInfo>, this_kind: &str) -> String {
    let who = match holder {
        Some(h) => {
            let kind = match h.kind.as_str() {
                "desktop app" => "the Akev desktop app".to_string(),
                "command line" => "the akev command".to_string(),
                other => format!("Akev ({other})"),
            };
            let mut details = vec![format!("pid {}", h.pid)];
            if let Some(url) = &h.api_url {
                details.push(format!("API {url}"));
            }
            if !h.started_at.is_empty() {
                details.push(format!("started {}", h.started_at));
            }
            format!(": {kind} ({})", details.join(", "))
        }
        None => String::new(),
    };
    let advice = if this_kind == "desktop app" {
        "Quit it first."
    } else {
        "Quit it first, or use a different data directory (--data-dir)."
    };
    format!(
        "another Akev instance is already using the data directory {}{who}. {advice}",
        dir.display()
    )
}

/// The exclusive lock on a data directory, held until dropped.
#[derive(Debug)]
pub struct DataDirLock {
    file: Option<File>,
    dir: PathBuf,
    info: Mutex<InstanceInfo>,
}

impl DataDirLock {
    /// Lock `dir` (which must exist) for this process (the desktop app when `desktop`) and
    /// write `instance.json`.
    ///
    /// On file systems without lock support the directory is used unlocked (logged).
    pub fn acquire(dir: &Path, desktop: bool) -> Result<DataDirLock, LockError> {
        let io_err = |source| LockError::Io {
            dir: dir.to_path_buf(),
            source,
        };
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join(LOCK_FILE))
            .map_err(io_err)?;
        let file = match file.try_lock() {
            Ok(()) => Some(file),
            Err(TryLockError::WouldBlock) => {
                return Err(LockError::Held {
                    dir: dir.to_path_buf(),
                    holder: read_instance(dir).map(Box::new),
                    desktop,
                });
            }
            Err(TryLockError::Error(e)) if e.kind() == io::ErrorKind::Unsupported => {
                tracing::warn!(
                    dir = %dir.display(),
                    "file locks aren't supported here; another Akev instance could use this data directory at the same time"
                );
                None
            }
            Err(TryLockError::Error(e)) => return Err(io_err(e)),
        };
        let lock = DataDirLock {
            file,
            dir: dir.to_path_buf(),
            info: Mutex::new(InstanceInfo::current(desktop)),
        };
        lock.write_instance();
        Ok(lock)
    }

    /// Record the API URL in `instance.json` once the API listens.
    pub fn set_api_url(&self, api_url: &str) {
        self.info.lock().api_url = Some(api_url.to_string());
        self.write_instance();
    }

    fn write_instance(&self) {
        let info = self.info.lock().clone();
        let path = self.dir.join(INSTANCE_FILE);
        if let Err(e) = crate::persist::write_json_atomic(&path, &info, 0o644) {
            tracing::debug!(error = %e, "couldn't write {}", path.display());
        }
    }
}

impl Drop for DataDirLock {
    fn drop(&mut self) {
        let ours = self.info.lock().clone();
        if read_instance(&self.dir)
            .is_some_and(|i| i.pid == ours.pid && i.started_at == ours.started_at)
        {
            let _ = std::fs::remove_file(self.dir.join(INSTANCE_FILE));
        }
        if let Some(file) = self.file.take() {
            let _ = file.unlock();
        }
    }
}

/// Read the `instance.json` of a data directory.
pub fn read_instance(dir: &Path) -> Option<InstanceInfo> {
    let bytes = std::fs::read(dir.join(INSTANCE_FILE)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_lock_on_the_same_dir_fails_and_names_the_holder() {
        let dir = tempfile::tempdir().unwrap();
        let first = DataDirLock::acquire(dir.path(), false).unwrap();
        first.set_api_url("http://127.0.0.1:45456");
        let info = read_instance(dir.path()).unwrap();
        assert_eq!(info.pid, std::process::id());
        assert_eq!(info.api_url.as_deref(), Some("http://127.0.0.1:45456"));

        let err = DataDirLock::acquire(dir.path(), false).unwrap_err();
        assert!(matches!(err, LockError::Held { .. }), "{err:?}");
        let message = err.to_string();
        assert!(
            message.contains("already using the data directory"),
            "{message}"
        );
        assert!(
            message.contains(&format!("pid {}", std::process::id())),
            "{message}"
        );
        assert!(message.contains("http://127.0.0.1:45456"), "{message}");
        assert!(message.contains("--data-dir"), "{message}");

        // Released on drop: the next instance gets it, and instance.json describes it.
        drop(first);
        assert!(read_instance(dir.path()).is_none());
        let _second = DataDirLock::acquire(dir.path(), false).unwrap();
        let info = read_instance(dir.path()).unwrap();
        assert_eq!(info.api_url, None);
    }

    #[test]
    fn different_dirs_lock_independently() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let _la = DataDirLock::acquire(a.path(), false).unwrap();
        let _lb = DataDirLock::acquire(b.path(), false).unwrap();
    }

    #[test]
    fn messages_without_holder_details() {
        let msg = held_message(Path::new("/data"), None, "command line");
        assert_eq!(
            msg,
            "another Akev instance is already using the data directory /data. Quit it first, or use a different data directory (--data-dir)."
        );
        assert!(
            held_message(Path::new("/data"), None, "desktop app")
                .ends_with("/data. Quit it first.")
        );
        let holder = InstanceInfo {
            pid: 7,
            kind: "desktop app".into(),
            version: "0.1.0".into(),
            api_url: None,
            started_at: String::new(),
        };
        assert!(
            held_message(Path::new("/data"), Some(&holder), "command line")
                .contains(": the Akev desktop app (pid 7)")
        );
    }
}
