//! Tracking of launched applications (browsers, terminals, Electron apps).
//!
//! A launched app is "active" while a tracked child is alive *or* the process table shows a
//! process we recognise as ours (by its command line or an `AKEV_*` environment marker).
//! The scan covers launchers that exit immediately (Opera's `launcher.exe`, `gnome-terminal`
//! handing off to its server, a second Chrome launch forwarding to the running instance) and
//! apps still running from before an Akev restart.

use crate::runner::{ChildHandle, ProcessEntry, ProcessSignal};
use crate::util::{Sys, stop_processes};
use parking_lot::Mutex;
use std::path::Path;
use std::time::Duration;

/// How long to watch a fresh launch for an immediate crash.
pub(crate) const LAUNCH_SETTLE: Duration = if cfg!(test) {
    Duration::from_millis(20)
} else {
    Duration::from_millis(500)
};

/// Children launched by one interceptor.
#[derive(Default)]
pub(crate) struct Tracker {
    children: Mutex<Vec<ChildHandle>>,
}

impl Tracker {
    pub fn add(&self, child: ChildHandle) {
        self.children.lock().push(child);
    }

    /// Live children (exited ones are dropped).
    pub fn alive(&self) -> Vec<ChildHandle> {
        let mut children = self.children.lock();
        children.retain(ChildHandle::is_running);
        children.clone()
    }

    /// Remove and return all live children.
    pub fn take(&self) -> Vec<ChildHandle> {
        self.children
            .lock()
            .drain(..)
            .filter(ChildHandle::is_running)
            .collect()
    }

    /// Pids of live children.
    pub fn pids(&self) -> Vec<u32> {
        self.alive().iter().filter_map(ChildHandle::pid).collect()
    }
}

/// Does `cmd` contain `flag=value` (e.g. `--user-data-dir=/x`) or `flag value`?
pub(crate) fn cmd_has_flag_value(cmd: &[String], flag: &str, value: &Path) -> bool {
    let value = value.to_string_lossy();
    let joined = format!("{flag}={value}");
    cmd.iter().enumerate().any(|(i, arg)| {
        arg == &joined || (arg == flag && cmd.get(i + 1).map(|v| v.as_str() == value) == Some(true))
    })
}

/// Pids in `procs` matching `pred`, excluding `exclude` (already-tracked children).
pub(crate) fn matching_pids(
    procs: &[ProcessEntry],
    exclude: &[u32],
    pred: impl Fn(&ProcessEntry) -> bool,
) -> Vec<u32> {
    procs
        .iter()
        .filter(|p| !exclude.contains(&p.pid) && pred(p))
        .map(|p| p.pid)
        .collect()
}

/// Stop everything in `tracker` plus the extra (scanned) pids, politely then forcefully.
pub(crate) async fn stop_all(
    sys: &Sys,
    tracker: &Tracker,
    extra_pids: Vec<u32>,
    polite: ProcessSignal,
) -> usize {
    let children = tracker.take();
    let count = children.len() + extra_pids.len();
    if count > 0 {
        stop_processes(sys, &children, &extra_pids, polite).await;
    }
    count
}

/// Last `max` bytes of a launch log, for error messages.
pub(crate) fn log_tail(path: &Path, max: usize) -> String {
    let Ok(bytes) = std::fs::read(path) else {
        return String::new();
    };
    let start = bytes.len().saturating_sub(max);
    String::from_utf8_lossy(&bytes[start..]).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn flag_matching() {
        let dir = PathBuf::from("/data/profiles/chrome");
        let cmd: Vec<String> = [
            "chrome",
            "--user-data-dir=/data/profiles/chrome",
            "--no-first-run",
        ]
        .map(String::from)
        .to_vec();
        assert!(cmd_has_flag_value(&cmd, "--user-data-dir", &dir));
        assert!(!cmd_has_flag_value(
            &cmd,
            "--user-data-dir",
            Path::new("/data/profiles/chrome2")
        ));
        let ff: Vec<String> = [
            "firefox",
            "-profile",
            "/data/profiles/firefox",
            "-no-remote",
        ]
        .map(String::from)
        .to_vec();
        assert!(cmd_has_flag_value(
            &ff,
            "-profile",
            Path::new("/data/profiles/firefox")
        ));
        assert!(!cmd_has_flag_value(
            &ff,
            "-profile",
            Path::new("/data/profiles")
        ));
        assert!(!cmd_has_flag_value(
            &["-profile".to_string()],
            "-profile",
            Path::new("/x")
        ));
    }

    #[test]
    fn matching_excludes_tracked() {
        let procs = vec![
            ProcessEntry {
                pid: 10,
                name: "a".into(),
                ..Default::default()
            },
            ProcessEntry {
                pid: 11,
                name: "a".into(),
                ..Default::default()
            },
            ProcessEntry {
                pid: 12,
                name: "b".into(),
                ..Default::default()
            },
        ];
        assert_eq!(matching_pids(&procs, &[10], |p| p.name == "a"), vec![11]);
    }

    #[test]
    fn log_tail_reads_end() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("l.log");
        std::fs::write(&p, "0123456789\n").unwrap();
        assert_eq!(log_tail(&p, 4), "789");
        assert_eq!(log_tail(&dir.path().join("missing"), 4), "");
    }
}
