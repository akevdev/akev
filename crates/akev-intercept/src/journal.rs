//! Where crash-recovery journals live, and who owns them.
//!
//! The system proxy backup and the Android device journal record settings Akev changed
//! *outside* its own process, so they must outlive the process — and its data directory.
//! An ephemeral run (`--ephemeral`) uses a temporary data directory that is deleted on exit
//! (or wiped by a reboot after a crash), and a custom `--data-dir` is never looked at by the
//! next start. Journals are therefore kept in a stable per-user place that every start scans:
//!
//! * for the platform data directory (the default: `~/.local/share/akev`, `~/Library/
//!   Application Support/akev`, `%LOCALAPPDATA%\akev`), in the data directory itself (where
//!   they always were);
//! * for any other data directory, in `<platform data dir>/akev/recovery/`, one file per data
//!   directory (`<name>-<hash of the data dir>.json`).
//!
//! Every journal records the owning process (random instance id, pid and process name), so a
//! journal belonging to another *running* Akev instance is never touched.

use crate::host::{HostEnv, Os};
use crate::util::{Sys, short_hash};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Random id of this process' Akev instance (distinguishes us from an earlier process that
/// happened to have the same pid, e.g. pid 1 in containers).
pub(crate) fn instance_id() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| uuid::Uuid::new_v4().simple().to_string())
}

/// The platform data directory Akev uses by default (`akev_api::default_data_dir`), derived
/// from `env` so tests can fake it.
pub(crate) fn platform_data_dir(env: &HostEnv) -> Option<PathBuf> {
    let base = match env.os {
        Os::Linux | Os::Other => env
            .var_path("XDG_DATA_HOME")
            .filter(|p| p.is_absolute())
            .or_else(|| env.home_path(".local/share")),
        Os::MacOs => env.home_path("Library/Application Support"),
        Os::Windows => env.var_path("LOCALAPPDATA"),
    };
    base.map(|p| p.join("akev"))
}

/// Data directories Akev uses when none is given: `$AKEV_DATA_DIR` and the platform
/// default, derived from `env` so tests can fake them.
pub(crate) fn default_data_dirs(env: &HostEnv) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = env.var_path("AKEV_DATA_DIR").into_iter().collect();
    if let Some(p) = platform_data_dir(env)
        && !out.contains(&p)
    {
        out.push(p);
    }
    out
}

/// `<platform data dir>/recovery`: journals of data directories other than the platform one.
pub(crate) fn recovery_dir(env: &HostEnv) -> Option<PathBuf> {
    platform_data_dir(env).map(|d| d.join("recovery"))
}

/// `("system-proxy-backup", "json")` for `system-proxy-backup.json`.
fn split_name(file_name: &str) -> (&str, &str) {
    file_name.rsplit_once('.').unwrap_or((file_name, ""))
}

/// Where the journal `file_name` (e.g. `system-proxy-backup.json`) of `data_dir` is kept.
///
/// The platform data directory keeps its journals inside; every other data directory (an
/// ephemeral temp dir, `--data-dir`, `$AKEV_DATA_DIR`) gets a file in [`recovery_dir`], so the
/// journal survives the directory and is found by every start. Without a home directory there
/// is nowhere stable to put it, and `data_dir` is used.
pub(crate) fn journal_file(env: &HostEnv, data_dir: &Path, file_name: &str) -> PathBuf {
    let platform = platform_data_dir(env);
    if platform.as_deref() == Some(data_dir) {
        return data_dir.join(file_name);
    }
    match recovery_dir(env) {
        Some(root) => {
            let (stem, ext) = split_name(file_name);
            let key = short_hash(data_dir.to_string_lossy().as_bytes());
            root.join(format!("{stem}-{key}.{ext}"))
        }
        None => data_dir.join(file_name),
    }
}

/// Every journal named `file_name` a new start should look at: the ones in the default data
/// directories and all files in [`recovery_dir`]. Paths may not exist.
pub(crate) fn scan_journals(env: &HostEnv, file_name: &str) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = default_data_dirs(env)
        .into_iter()
        .map(|d| d.join(file_name))
        .collect();
    let (stem, ext) = split_name(file_name);
    let prefix = format!("{stem}-");
    let suffix = format!(".{ext}");
    if let Some(root) = recovery_dir(env)
        && let Ok(entries) = std::fs::read_dir(&root)
    {
        let mut found: Vec<PathBuf> = entries
            .flatten()
            .filter(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                name.strip_prefix(&prefix)
                    .and_then(|rest| rest.strip_suffix(&suffix))
                    .is_some_and(|key| {
                        key.len() == 16 && key.bytes().all(|b| b.is_ascii_hexdigit())
                    })
            })
            .map(|e| e.path())
            .collect();
        found.sort();
        out.extend(found);
    }
    dedup(out)
}

/// Remove duplicates, keeping the first occurrence.
pub(crate) fn dedup(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::with_capacity(paths.len());
    for p in paths {
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

/// Is a live process named `live` the program recorded as `recorded`? Tolerates Linux's
/// 15-character `comm` truncation and a missing record (conservative: same program).
pub(crate) fn same_program(live: &str, recorded: &str) -> bool {
    if recorded.is_empty() || live == recorded {
        return true;
    }
    let (short, long) = if live.len() <= recorded.len() {
        (live, recorded)
    } else {
        (recorded, live)
    };
    short.len() >= 15 && long.starts_with(short)
}

/// This process' name as seen in the process table (recorded in journals to guard against
/// pid reuse).
pub(crate) async fn own_process_name(sys: &Sys) -> String {
    let own = std::process::id();
    let procs = sys.processes(false).await;
    procs
        .iter()
        .find(|p| p.pid == own)
        .map(|p| p.name.clone())
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::testutil::fake_env;

    #[test]
    fn journals_of_non_default_dirs_live_in_the_recovery_dir() {
        let dir = tempfile::tempdir().unwrap();
        let env = fake_env(Os::Linux, dir.path());
        let platform = dir.path().join("home/.local/share/akev");
        assert_eq!(platform_data_dir(&env), Some(platform.clone()));
        assert_eq!(
            journal_file(&env, &platform, "system-proxy-backup.json"),
            platform.join("system-proxy-backup.json")
        );
        let tmp = Path::new("/tmp/akev-AbC123");
        let j = journal_file(&env, tmp, "system-proxy-backup.json");
        assert!(j.starts_with(platform.join("recovery")), "{}", j.display());
        assert!(!j.starts_with(tmp));
        // Stable per data dir, distinct between data dirs and between journal kinds.
        assert_eq!(j, journal_file(&env, tmp, "system-proxy-backup.json"));
        assert_ne!(
            j,
            journal_file(
                &env,
                Path::new("/tmp/akev-Other"),
                "system-proxy-backup.json"
            )
        );
        let android = journal_file(&env, tmp, "android-proxy.json");
        assert_ne!(j, android);

        // Scanning finds them (and the default dirs' files), nothing else.
        std::fs::create_dir_all(j.parent().unwrap()).unwrap();
        std::fs::write(&j, "{}").unwrap();
        std::fs::write(&android, "[]").unwrap();
        std::fs::write(
            j.with_file_name(format!(
                "{}.corrupt-1",
                j.file_name().unwrap().to_string_lossy()
            )),
            "x",
        )
        .unwrap();
        let found = scan_journals(&env, "system-proxy-backup.json");
        assert_eq!(found, vec![platform.join("system-proxy-backup.json"), j]);
        assert_eq!(
            scan_journals(&env, "android-proxy.json"),
            vec![platform.join("android-proxy.json"), android]
        );

        // No home: nowhere stable, so the data dir itself.
        let bare = HostEnv::empty(Os::Linux);
        assert_eq!(
            journal_file(&bare, tmp, "android-proxy.json"),
            tmp.join("android-proxy.json")
        );
    }

    #[test]
    fn default_dirs_follow_platform_conventions() {
        let dir = tempfile::tempdir().unwrap();
        let env = fake_env(Os::Linux, dir.path());
        assert_eq!(
            default_data_dirs(&env),
            vec![dir.path().join("home/.local/share/akev")]
        );
        let env = env
            .with_var("XDG_DATA_HOME", "/xdg")
            .with_var("AKEV_DATA_DIR", "/custom");
        assert_eq!(
            default_data_dirs(&env),
            vec![PathBuf::from("/custom"), PathBuf::from("/xdg/akev")]
        );
        // `$AKEV_DATA_DIR` isn't the platform dir: its journals go to the recovery dir.
        assert!(
            journal_file(&env, Path::new("/custom"), "android-proxy.json")
                .starts_with("/xdg/akev/recovery")
        );
        let mut mac = HostEnv::empty(Os::MacOs);
        mac.home = Some("/Users/me".into());
        assert_eq!(
            default_data_dirs(&mac),
            vec![PathBuf::from("/Users/me/Library/Application Support/akev")]
        );
        let win = HostEnv::empty(Os::Windows)
            .with_var("APPDATA", "C:/Users/me/AppData/Roaming")
            .with_var("LOCALAPPDATA", "C:/Users/me/AppData/Local");
        assert_eq!(
            default_data_dirs(&win),
            vec![PathBuf::from("C:/Users/me/AppData/Local/akev")]
        );
    }

    #[test]
    fn program_name_matching() {
        assert!(same_program("akev", "akev"));
        assert!(same_program("anything", ""));
        assert!(!same_program("vim", "akev"));
        assert!(
            same_program("akev-desktop-ap", "akev-desktop-app"),
            "comm truncation"
        );
        assert!(
            !same_program("akev", "akev-desktop"),
            "short names must match exactly"
        );
    }
}
