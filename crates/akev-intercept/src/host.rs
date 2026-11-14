//! Description of the machine interceptors run on: operating system, environment variables,
//! home directory and a root prefix for well-known absolute paths.
//!
//! Everything platform-specific is decided at *runtime* from [`HostEnv::os`] rather than with
//! `cfg` attributes, so the macOS and Windows code paths are compiled and unit-tested on
//! every platform (with fake directories). Only genuine OS APIs are `cfg`-gated.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Operating system family.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Os {
    /// Linux (and other freedesktop-style Unixes).
    Linux,
    /// macOS.
    MacOs,
    /// Windows.
    Windows,
    /// Anything else.
    Other,
}

impl Os {
    /// The OS this binary was compiled for.
    pub fn current() -> Os {
        if cfg!(target_os = "linux") || cfg!(target_os = "freebsd") || cfg!(target_os = "openbsd") {
            Os::Linux
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else if cfg!(windows) {
            Os::Windows
        } else {
            Os::Other
        }
    }

    /// `PATH` list separator.
    pub fn path_separator(self) -> char {
        if self == Os::Windows { ';' } else { ':' }
    }

    /// Stable lowercase name (`linux`, `macos`, `windows`, `other`).
    pub fn as_str(self) -> &'static str {
        match self {
            Os::Linux => "linux",
            Os::MacOs => "macos",
            Os::Windows => "windows",
            Os::Other => "other",
        }
    }
}

/// Snapshot of the host environment used for detection.
#[derive(Clone, Debug)]
pub struct HostEnv {
    /// Operating system family.
    pub os: Os,
    /// Environment variables (keys upper-cased on Windows, where they're case-insensitive).
    pub vars: HashMap<String, String>,
    /// The user's home directory.
    pub home: Option<PathBuf>,
    /// Prefix prepended to well-known absolute system paths (`/` in production; a temporary
    /// directory in tests).
    pub root: PathBuf,
}

impl HostEnv {
    /// The current process' environment.
    pub fn current() -> Self {
        let os = Os::current();
        let vars = std::env::vars_os()
            .filter_map(|(k, v)| {
                let k = k.into_string().ok()?;
                let v = v.into_string().ok()?;
                Some((
                    if os == Os::Windows {
                        k.to_uppercase()
                    } else {
                        k
                    },
                    v,
                ))
            })
            .collect();
        HostEnv {
            os,
            vars,
            home: dirs::home_dir(),
            root: PathBuf::from("/"),
        }
    }

    /// An empty environment for `os` (tests).
    pub fn empty(os: Os) -> Self {
        HostEnv {
            os,
            vars: HashMap::new(),
            home: None,
            root: PathBuf::from("/"),
        }
    }

    /// Builder: set a variable.
    pub fn with_var(mut self, key: &str, value: impl Into<String>) -> Self {
        let key = if self.os == Os::Windows {
            key.to_uppercase()
        } else {
            key.to_string()
        };
        self.vars.insert(key, value.into());
        self
    }

    /// An environment variable (non-empty values only).
    pub fn var(&self, key: &str) -> Option<&str> {
        let v = if self.os == Os::Windows {
            self.vars.get(&key.to_uppercase())
        } else {
            self.vars.get(key)
        };
        v.map(String::as_str).filter(|v| !v.is_empty())
    }

    /// A path-valued environment variable.
    pub fn var_path(&self, key: &str) -> Option<PathBuf> {
        self.var(key).map(PathBuf::from)
    }

    /// Resolve a well-known absolute system path (e.g. `/opt/google/chrome/chrome`) under
    /// [`HostEnv::root`].
    pub fn sys_path(&self, absolute: &str) -> PathBuf {
        let rel = absolute.trim_start_matches('/');
        if rel.is_empty() {
            self.root.clone()
        } else {
            self.root.join(rel)
        }
    }

    /// `~/<rel>` if the home directory is known.
    pub fn home_path(&self, rel: &str) -> Option<PathBuf> {
        self.home.as_ref().map(|h| h.join(rel))
    }

    /// Directories listed in `PATH`.
    pub fn path_dirs(&self) -> Vec<PathBuf> {
        self.var("PATH")
            .map(|p| {
                p.split(self.os.path_separator())
                    .filter(|d| !d.is_empty())
                    .map(PathBuf::from)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Find an executable by name in `PATH` (honouring `PATHEXT` on Windows).
    pub fn find_in_path(&self, name: &str) -> Option<PathBuf> {
        let candidates = self.executable_names(name);
        self.path_dirs()
            .into_iter()
            .flat_map(|dir| {
                candidates
                    .iter()
                    .map(move |c| dir.join(c))
                    .collect::<Vec<_>>()
            })
            .find(|p| is_executable(p))
    }

    /// First of several executable names found in `PATH`.
    pub fn find_any_in_path(&self, names: &[&str]) -> Option<PathBuf> {
        names.iter().find_map(|n| self.find_in_path(n))
    }

    fn executable_names(&self, name: &str) -> Vec<String> {
        if self.os != Os::Windows || Path::new(name).extension().is_some() {
            return vec![name.to_string()];
        }
        let exts = self.var("PATHEXT").unwrap_or(".COM;.EXE;.BAT;.CMD");
        exts.split(';')
            .filter(|e| !e.is_empty())
            .map(|e| format!("{name}{}", e.to_lowercase()))
            .collect()
    }

    /// The user's login shell (`$SHELL`), falling back to bash/sh.
    pub fn user_shell(&self) -> String {
        if let Some(shell) = self.var("SHELL") {
            return shell.to_string();
        }
        let bash = self.sys_path("/bin/bash");
        if bash.exists() {
            "/bin/bash".to_string()
        } else {
            "/bin/sh".to_string()
        }
    }

    /// The desktop environment (`XDG_CURRENT_DESKTOP`, upper-cased), e.g. `GNOME`, `KDE`.
    pub fn desktop(&self) -> String {
        self.var("XDG_CURRENT_DESKTOP")
            .unwrap_or_default()
            .to_uppercase()
    }
}

/// Exists, is a regular file (following symlinks) and — on Unix — has an execute bit.
pub fn is_executable(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    use super::*;

    /// Create an executable file (with optional content) at `path`, creating parent dirs.
    pub fn make_exe(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    /// A Linux-like env rooted in `root`, with `PATH=<root>/usr/bin` and home `<root>/home`.
    pub fn fake_env(os: Os, root: &Path) -> HostEnv {
        let mut env = HostEnv::empty(os);
        env.root = root.to_path_buf();
        env.home = Some(root.join("home"));
        let path = root.join("usr/bin");
        std::fs::create_dir_all(&path).unwrap();
        env.with_var("PATH", path.to_string_lossy())
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::*;
    use super::*;

    #[test]
    fn find_in_fake_path() {
        let dir = tempfile::tempdir().unwrap();
        let env = fake_env(Os::Linux, dir.path());
        assert_eq!(env.find_in_path("google-chrome"), None);
        make_exe(&dir.path().join("usr/bin/google-chrome"), "#!/bin/sh\n");
        assert_eq!(
            env.find_in_path("google-chrome"),
            Some(dir.path().join("usr/bin/google-chrome"))
        );
        assert_eq!(
            env.find_any_in_path(&["chromium", "google-chrome"]),
            Some(dir.path().join("usr/bin/google-chrome"))
        );
        // Directories and non-executables don't count.
        std::fs::create_dir_all(dir.path().join("usr/bin/dir-not-exe")).unwrap();
        assert_eq!(env.find_in_path("dir-not-exe"), None);
        #[cfg(unix)]
        {
            std::fs::write(dir.path().join("usr/bin/plain"), "").unwrap();
            assert_eq!(env.find_in_path("plain"), None);
        }
    }

    #[test]
    fn windows_pathext_and_case_insensitive_vars() {
        let dir = tempfile::tempdir().unwrap();
        let env = HostEnv::empty(Os::Windows)
            .with_var("Path", dir.path().to_string_lossy())
            .with_var("PathExt", ".EXE;.CMD");
        assert_eq!(env.var("PATH"), env.var("path"));
        make_exe(&dir.path().join("adb.exe"), "");
        assert_eq!(env.find_in_path("adb"), Some(dir.path().join("adb.exe")));
        assert_eq!(
            env.find_in_path("adb.exe"),
            Some(dir.path().join("adb.exe"))
        );
    }

    #[test]
    fn sys_path_and_vars() {
        let mut env = HostEnv::empty(Os::Linux).with_var("EMPTY", "");
        env.root = PathBuf::from("/tmp/root");
        assert_eq!(env.sys_path("/opt/x"), PathBuf::from("/tmp/root/opt/x"));
        assert_eq!(env.sys_path("/"), PathBuf::from("/tmp/root"));
        assert_eq!(env.var("EMPTY"), None);
        assert_eq!(Os::Windows.path_separator(), ';');
        assert_eq!(Os::Linux.path_separator(), ':');
    }
}
