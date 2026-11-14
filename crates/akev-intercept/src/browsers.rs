//! Browser detection: where each browser lives on Linux (PATH, well-known paths, Snap,
//! Flatpak), macOS (`.app` bundles) and Windows (Program Files / LocalAppData), plus cheap
//! version detection (metadata files first; `--version` only where it's safe and fast).

use crate::host::{HostEnv, Os};
use crate::runner::CommandSpec;
use crate::util::{Sys, short_hash};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

/// Browser engine family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Engine {
    Chromium,
    Firefox,
}

/// Where and how to find one browser on each platform.
#[derive(Debug)]
pub(crate) struct BrowserSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub icon: &'static str,
    /// Executable names searched in `PATH` (Linux).
    pub linux_bins: &'static [&'static str],
    /// Absolute well-known paths (Linux).
    pub linux_paths: &'static [&'static str],
    /// Snap names.
    pub snaps: &'static [&'static str],
    /// Flatpak application ids.
    pub flatpaks: &'static [&'static str],
    /// `(bundle name, executable)` in `/Applications` or `~/Applications` (macOS).
    pub mac_apps: &'static [(&'static str, &'static str)],
    /// `(env var holding a base dir, relative path with '/' separators)` (Windows).
    pub win_paths: &'static [(&'static str, &'static str)],
}

impl BrowserSpec {
    /// Does this browser exist at all on `os`?
    pub fn supported_on(&self, os: Os) -> bool {
        match os {
            Os::Linux => {
                !(self.linux_bins.is_empty()
                    && self.linux_paths.is_empty()
                    && self.flatpaks.is_empty())
            }
            Os::MacOs => !self.mac_apps.is_empty(),
            Os::Windows => !self.win_paths.is_empty(),
            Os::Other => false,
        }
    }
}

const PF: &str = "PROGRAMFILES";
const PF86: &str = "PROGRAMFILES(X86)";
const LAD: &str = "LOCALAPPDATA";

/// All Chromium-family browsers.
pub(crate) static CHROMIUM_BROWSERS: &[BrowserSpec] = &[
    BrowserSpec {
        id: "chrome",
        name: "Chrome",
        description: "Intercept a fresh, independent Chrome window (separate profile, logins persist)",
        icon: "chrome",
        linux_bins: &["google-chrome-stable", "google-chrome"],
        linux_paths: &["/opt/google/chrome/chrome"],
        snaps: &[],
        flatpaks: &["com.google.Chrome"],
        mac_apps: &[("Google Chrome.app", "Google Chrome")],
        win_paths: &[
            (PF, "Google/Chrome/Application/chrome.exe"),
            (PF86, "Google/Chrome/Application/chrome.exe"),
            (LAD, "Google/Chrome/Application/chrome.exe"),
        ],
    },
    BrowserSpec {
        id: "chromium",
        name: "Chromium",
        description: "Intercept a fresh, independent Chromium window",
        icon: "chromium",
        linux_bins: &["chromium", "chromium-browser"],
        linux_paths: &[
            "/usr/lib/chromium/chromium",
            "/usr/lib/chromium-browser/chromium-browser",
            "/usr/lib64/chromium-browser/chromium-browser",
        ],
        snaps: &["chromium"],
        flatpaks: &["org.chromium.Chromium"],
        mac_apps: &[("Chromium.app", "Chromium")],
        win_paths: &[
            (LAD, "Chromium/Application/chrome.exe"),
            (PF, "Chromium/Application/chrome.exe"),
        ],
    },
    BrowserSpec {
        id: "brave",
        name: "Brave",
        description: "Intercept a fresh, independent Brave window",
        icon: "brave",
        linux_bins: &["brave-browser", "brave-browser-stable", "brave"],
        linux_paths: &["/opt/brave.com/brave/brave"],
        snaps: &["brave"],
        flatpaks: &["com.brave.Browser"],
        mac_apps: &[("Brave Browser.app", "Brave Browser")],
        win_paths: &[
            (PF, "BraveSoftware/Brave-Browser/Application/brave.exe"),
            (PF86, "BraveSoftware/Brave-Browser/Application/brave.exe"),
            (LAD, "BraveSoftware/Brave-Browser/Application/brave.exe"),
        ],
    },
    BrowserSpec {
        id: "edge",
        name: "Microsoft Edge",
        description: "Intercept a fresh, independent Microsoft Edge window",
        icon: "edge",
        linux_bins: &["microsoft-edge-stable", "microsoft-edge"],
        linux_paths: &["/opt/microsoft/msedge/msedge"],
        snaps: &[],
        flatpaks: &["com.microsoft.Edge"],
        mac_apps: &[("Microsoft Edge.app", "Microsoft Edge")],
        win_paths: &[
            (PF86, "Microsoft/Edge/Application/msedge.exe"),
            (PF, "Microsoft/Edge/Application/msedge.exe"),
            (LAD, "Microsoft/Edge/Application/msedge.exe"),
        ],
    },
    BrowserSpec {
        id: "opera",
        name: "Opera",
        description: "Intercept a fresh, independent Opera window",
        icon: "opera",
        linux_bins: &["opera", "opera-stable"],
        linux_paths: &[
            "/usr/lib/x86_64-linux-gnu/opera/opera",
            "/usr/lib64/opera/opera",
            "/usr/lib/opera/opera",
        ],
        snaps: &["opera"],
        flatpaks: &["com.opera.Opera"],
        mac_apps: &[("Opera.app", "Opera")],
        win_paths: &[
            (LAD, "Programs/Opera/opera.exe"),
            (PF, "Opera/opera.exe"),
            (LAD, "Programs/Opera/launcher.exe"),
        ],
    },
    BrowserSpec {
        id: "vivaldi",
        name: "Vivaldi",
        description: "Intercept a fresh, independent Vivaldi window",
        icon: "vivaldi",
        linux_bins: &["vivaldi-stable", "vivaldi"],
        linux_paths: &["/opt/vivaldi/vivaldi"],
        snaps: &["vivaldi"],
        flatpaks: &["com.vivaldi.Vivaldi"],
        mac_apps: &[("Vivaldi.app", "Vivaldi")],
        win_paths: &[
            (LAD, "Vivaldi/Application/vivaldi.exe"),
            (PF, "Vivaldi/Application/vivaldi.exe"),
        ],
    },
    BrowserSpec {
        id: "arc",
        name: "Arc",
        description: "Intercept a fresh, independent Arc window (experimental: Arc may ignore some flags)",
        icon: "arc",
        linux_bins: &[],
        linux_paths: &[],
        snaps: &[],
        flatpaks: &[],
        mac_apps: &[("Arc.app", "Arc")],
        win_paths: &[(LAD, "Microsoft/WindowsApps/Arc.exe")],
    },
];

/// Firefox variants.
pub(crate) static FIREFOX_BROWSERS: &[BrowserSpec] = &[
    BrowserSpec {
        id: "firefox",
        name: "Firefox",
        description: "Intercept a fresh, independent Firefox window (separate profile, logins persist)",
        icon: "firefox",
        linux_bins: &["firefox", "firefox-esr"],
        linux_paths: &[
            "/usr/lib/firefox/firefox",
            "/usr/lib64/firefox/firefox",
            "/opt/firefox/firefox",
        ],
        snaps: &["firefox"],
        flatpaks: &["org.mozilla.firefox"],
        mac_apps: &[("Firefox.app", "firefox")],
        win_paths: &[
            (PF, "Mozilla Firefox/firefox.exe"),
            (PF86, "Mozilla Firefox/firefox.exe"),
            (LAD, "Mozilla Firefox/firefox.exe"),
        ],
    },
    BrowserSpec {
        id: "firefox-developer",
        name: "Firefox Developer Edition",
        description: "Intercept a fresh, independent Firefox Developer Edition window",
        icon: "firefox-developer",
        linux_bins: &[
            "firefox-developer-edition",
            "firefox-developer",
            "firefox-dev",
        ],
        linux_paths: &[
            "/opt/firefox-developer-edition/firefox",
            "/opt/firefox-dev/firefox",
        ],
        snaps: &[],
        flatpaks: &[],
        mac_apps: &[("Firefox Developer Edition.app", "firefox")],
        win_paths: &[
            (PF, "Firefox Developer Edition/firefox.exe"),
            (PF86, "Firefox Developer Edition/firefox.exe"),
            (LAD, "Firefox Developer Edition/firefox.exe"),
        ],
    },
    BrowserSpec {
        id: "firefox-nightly",
        name: "Firefox Nightly",
        description: "Intercept a fresh, independent Firefox Nightly window",
        icon: "firefox-nightly",
        linux_bins: &["firefox-nightly"],
        linux_paths: &["/opt/firefox-nightly/firefox"],
        snaps: &[],
        flatpaks: &["org.mozilla.firefox.nightly"],
        mac_apps: &[("Firefox Nightly.app", "firefox")],
        win_paths: &[
            (PF, "Firefox Nightly/firefox.exe"),
            (PF86, "Firefox Nightly/firefox.exe"),
            (LAD, "Firefox Nightly/firefox.exe"),
        ],
    },
];

/// How a browser is installed (affects where its profile may live).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum InstallKind {
    /// A regular executable.
    Native,
    /// A Snap (`/snap/bin/<name>`).
    Snap { name: String },
    /// A Flatpak; `exe` is the exported wrapper script.
    Flatpak {
        app_id: String,
        installation: PathBuf,
    },
    /// A macOS `.app` bundle.
    AppBundle { bundle: PathBuf },
}

impl InstallKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            InstallKind::Native => "native",
            InstallKind::Snap { .. } => "snap",
            InstallKind::Flatpak { .. } => "flatpak",
            InstallKind::AppBundle { .. } => "app-bundle",
        }
    }
}

/// A detected browser installation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Installation {
    /// What to execute.
    pub exe: PathBuf,
    pub kind: InstallKind,
}

/// Find `spec` on this machine (filesystem checks only; no processes are run).
pub(crate) fn detect(spec: &BrowserSpec, env: &HostEnv) -> Option<Installation> {
    let native = |exe: PathBuf| {
        Some(Installation {
            exe,
            kind: InstallKind::Native,
        })
    };
    match env.os {
        Os::Linux => {
            if let Some(exe) = env.find_any_in_path(spec.linux_bins) {
                // A /snap/bin entry found via PATH is still a snap.
                if let Some(name) = snap_name_of(&exe, env) {
                    return Some(Installation {
                        exe,
                        kind: InstallKind::Snap { name },
                    });
                }
                return native(exe);
            }
            if let Some(exe) = spec
                .linux_paths
                .iter()
                .map(|p| env.sys_path(p))
                .find(|p| crate::host::is_executable(p))
            {
                return native(exe);
            }
            for name in spec.snaps {
                let exe = env.sys_path("/snap/bin").join(name);
                if exe.exists() {
                    return Some(Installation {
                        exe,
                        kind: InstallKind::Snap {
                            name: name.to_string(),
                        },
                    });
                }
            }
            for app_id in spec.flatpaks {
                for installation in flatpak_installations(env) {
                    let exe = installation.join("exports").join("bin").join(app_id);
                    if exe.exists() {
                        return Some(Installation {
                            exe,
                            kind: InstallKind::Flatpak {
                                app_id: app_id.to_string(),
                                installation,
                            },
                        });
                    }
                }
            }
            None
        }
        Os::MacOs => {
            let mut roots = vec![env.sys_path("/Applications")];
            if let Some(home_apps) = env.home_path("Applications") {
                roots.push(home_apps);
            }
            for (bundle, exe) in spec.mac_apps {
                for root in &roots {
                    let bundle_path = root.join(bundle);
                    let exe_path = bundle_path.join("Contents").join("MacOS").join(exe);
                    if exe_path.is_file() {
                        return Some(Installation {
                            exe: exe_path,
                            kind: InstallKind::AppBundle {
                                bundle: bundle_path,
                            },
                        });
                    }
                }
            }
            None
        }
        Os::Windows => spec.win_paths.iter().find_map(|(base, rel)| {
            let mut p = env.var_path(base)?;
            for part in rel.split('/') {
                p.push(part);
            }
            p.is_file().then_some(Installation {
                exe: p,
                kind: InstallKind::Native,
            })
        }),
        Os::Other => None,
    }
}

fn snap_name_of(exe: &Path, env: &HostEnv) -> Option<String> {
    let snap_bin = env.sys_path("/snap/bin");
    let parent = exe.parent()?;
    (parent == snap_bin)
        .then(|| exe.file_name().map(|n| n.to_string_lossy().into_owned()))
        .flatten()
}

/// User then system Flatpak installations.
pub(crate) fn flatpak_installations(env: &HostEnv) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(user) = env
        .var_path("XDG_DATA_HOME")
        .or_else(|| env.home_path(".local/share"))
    {
        out.push(user.join("flatpak"));
    }
    out.push(env.sys_path("/var/lib/flatpak"));
    out
}

/// Directory a confined browser can write its profile to, if it's not free to use
/// `default` (`<data_dir>/profiles/<id>`): Snap: the snap's own `$HOME`; Flatpak: the app's
/// data dir.
///
/// Like `default`, the result is specific to `data_dir` (`akev-profiles/<hash of data_dir>/
/// <id>`): an Akev instance recognises the browsers it launched by their profile, so two
/// instances with different data directories (e.g. the desktop app and an ephemeral
/// `akev capture`) must never share one, or one would report — and stop — the other's
/// browser as its own.
pub(crate) fn confined_profile_dir(
    inst: &Installation,
    env: &HostEnv,
    data_dir: &Path,
    default: &Path,
) -> PathBuf {
    let leaf = default
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_else(|| "profile".into());
    let base = match &inst.kind {
        InstallKind::Snap { name } => env.home_path(&format!("snap/{name}/current/akev-profiles")),
        InstallKind::Flatpak { app_id, .. } => {
            env.home_path(&format!(".var/app/{app_id}/data/akev-profiles"))
        }
        _ => None,
    };
    match base {
        Some(dir) => dir
            .join(short_hash(data_dir.to_string_lossy().as_bytes()))
            .join(leaf),
        None => default.to_path_buf(),
    }
}

// ------------------------------------------------------------------------------- versions

fn version_cache() -> &'static Mutex<HashMap<String, Option<String>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// A key that changes whenever the installation is updated.
fn version_key(inst: &Installation, env: &HostEnv) -> String {
    let stamp = |p: &Path| -> String {
        let real = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        let meta = std::fs::metadata(&real).ok();
        format!(
            "{}:{}:{:?}",
            real.display(),
            meta.as_ref().map(|m| m.len()).unwrap_or(0),
            meta.and_then(|m| m.modified().ok())
        )
    };
    match &inst.kind {
        InstallKind::Snap { name } => stamp(&env.sys_path("/snap").join(name).join("current")),
        InstallKind::Flatpak {
            app_id,
            installation,
        } => stamp(
            &installation
                .join("app")
                .join(app_id)
                .join("current")
                .join("active"),
        ),
        InstallKind::AppBundle { bundle } => stamp(&bundle.join("Contents").join("Info.plist")),
        InstallKind::Native => stamp(&inst.exe),
    }
}

/// Detected version (cached until the installation changes).
pub(crate) async fn version(sys: &Sys, inst: &Installation, engine: Engine) -> Option<String> {
    let key = version_key(inst, &sys.env);
    if let Some(v) = version_cache().lock().get(&key) {
        return v.clone();
    }
    let v = probe_version(sys, inst, engine).await;
    version_cache().lock().insert(key, v.clone());
    v
}

async fn probe_version(sys: &Sys, inst: &Installation, engine: Engine) -> Option<String> {
    let env = &sys.env;
    match &inst.kind {
        InstallKind::AppBundle { bundle } => {
            let plist = std::fs::read_to_string(bundle.join("Contents").join("Info.plist")).ok()?;
            plist_string(&plist, "CFBundleShortVersionString")
        }
        InstallKind::Snap { name } => {
            let yaml = std::fs::read_to_string(
                env.sys_path("/snap")
                    .join(name)
                    .join("current/meta/snap.yaml"),
            )
            .ok()?;
            yaml_top_level(&yaml, "version")
        }
        InstallKind::Flatpak { app_id, .. } => {
            let flatpak = env.find_in_path("flatpak")?;
            let out = sys
                .runner
                .output(
                    &CommandSpec::new(flatpak)
                        .args(["info", app_id.as_str()])
                        .timeout(Duration::from_secs(3)),
                )
                .await
                .ok()?;
            out.stdout.lines().find_map(|l| {
                l.trim()
                    .strip_prefix("Version:")
                    .map(|v| v.trim().to_string())
            })
        }
        InstallKind::Native => {
            if engine == Engine::Firefox
                && let Some(v) = firefox_application_ini_version(&inst.exe, env)
            {
                return Some(v);
            }
            if env.os == Os::Windows {
                // Never run a Windows browser with --version: it opens a window.
                return windows_version_dir(&inst.exe);
            }
            let out = sys
                .runner
                .output(
                    &CommandSpec::new(&inst.exe)
                        .arg("--version")
                        .timeout(Duration::from_secs(4)),
                )
                .await
                .ok()?;
            parse_version_token(&out.stdout).or_else(|| parse_version_token(&out.stderr))
        }
    }
}

fn firefox_application_ini_version(exe: &Path, env: &HostEnv) -> Option<String> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(real) = std::fs::canonicalize(exe)
        && let Some(d) = real.parent()
    {
        dirs.push(d.to_path_buf());
    }
    if let Some(d) = exe.parent() {
        dirs.push(d.to_path_buf());
    }
    if env.os == Os::Linux {
        // Distro launchers like /usr/bin/firefox are shell scripts, not symlinks.
        dirs.push(env.sys_path("/usr/lib64/firefox"));
        dirs.push(env.sys_path("/usr/lib/firefox"));
    }
    dirs.iter().find_map(|d| {
        let ini = std::fs::read_to_string(d.join("application.ini")).ok()?;
        ini_value(&ini, "App", "Version")
    })
}

/// Chrome-style installs keep a `<version>` directory next to the executable.
pub(crate) fn windows_version_dir(exe: &Path) -> Option<String> {
    let dir = exe.parent()?;
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| {
            n.split('.').count() >= 3
                && n.split('.')
                    .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
        })
        .max_by_key(|n| {
            n.split('.')
                .map(|p| p.parse::<u64>().unwrap_or(0))
                .collect::<Vec<_>>()
        })
}

/// First dotted-number token, e.g. `Google Chrome 131.0.6778.85` → `131.0.6778.85`.
pub(crate) fn parse_version_token(s: &str) -> Option<String> {
    s.split(|c: char| c.is_whitespace() || c == ',' || c == '(' || c == ')')
        .map(|t| t.trim_start_matches('v'))
        .find(|t| {
            t.contains('.')
                && t.starts_with(|c: char| c.is_ascii_digit())
                && t.split('.')
                    .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric()))
        })
        .map(str::to_string)
}

/// `<key>K</key><string>V</string>` from an XML plist.
pub(crate) fn plist_string(xml: &str, key: &str) -> Option<String> {
    let marker = format!("<key>{key}</key>");
    let rest = &xml[xml.find(&marker)? + marker.len()..];
    let start = rest.find("<string>")? + "<string>".len();
    let end = rest[start..].find("</string>")?;
    // The value must be the very next element.
    if !rest[..start - "<string>".len()].trim().is_empty() {
        return None;
    }
    Some(rest[start..start + end].trim().to_string())
}

/// `key=value` within `[section]` of an INI file.
pub(crate) fn ini_value(ini: &str, section: &str, key: &str) -> Option<String> {
    let mut in_section = false;
    for line in ini.lines().map(str::trim) {
        if line.starts_with('[') && line.ends_with(']') {
            in_section = line[1..line.len() - 1].eq_ignore_ascii_case(section);
        } else if in_section
            && let Some((k, v)) = line.split_once('=')
            && k.trim().eq_ignore_ascii_case(key)
        {
            return Some(v.trim().to_string());
        }
    }
    None
}

/// A top-level scalar from a simple YAML document (`version: '131.0'`).
pub(crate) fn yaml_top_level(yaml: &str, key: &str) -> Option<String> {
    yaml.lines().find_map(|line| {
        let rest = line.strip_prefix(key)?.strip_prefix(':')?;
        let v = rest.trim().trim_matches(|c| c == '\'' || c == '"');
        (!v.is_empty()).then(|| v.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::testutil::{fake_env, make_exe};
    use crate::runner::CommandOutput;
    use crate::runner::mock::MockRunner;

    fn spec(id: &str) -> &'static BrowserSpec {
        CHROMIUM_BROWSERS
            .iter()
            .chain(FIREFOX_BROWSERS)
            .find(|s| s.id == id)
            .unwrap()
    }

    #[test]
    fn ids_are_unique_and_supported_where_expected() {
        let mut ids: Vec<_> = CHROMIUM_BROWSERS
            .iter()
            .chain(FIREFOX_BROWSERS)
            .map(|s| s.id)
            .collect();
        ids.sort();
        let len = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), len);
        assert!(!spec("arc").supported_on(Os::Linux));
        assert!(spec("arc").supported_on(Os::MacOs));
        assert!(spec("arc").supported_on(Os::Windows));
        for s in CHROMIUM_BROWSERS.iter().filter(|s| s.id != "arc") {
            assert!(s.supported_on(Os::Linux), "{}", s.id);
        }
    }

    #[test]
    fn linux_detection_order() {
        let dir = tempfile::tempdir().unwrap();
        let env = fake_env(Os::Linux, dir.path());
        assert_eq!(detect(spec("chrome"), &env), None);

        // Flatpak (user installation) is the fallback...
        let fp = env
            .home_path(".local/share/flatpak/exports/bin/com.google.Chrome")
            .unwrap();
        make_exe(&fp, "#!/bin/sh\n");
        let inst = detect(spec("chrome"), &env).unwrap();
        assert_eq!(inst.exe, fp);
        assert!(
            matches!(inst.kind, InstallKind::Flatpak { ref app_id, .. } if app_id == "com.google.Chrome")
        );

        // ...a well-known path beats it...
        let opt = env.sys_path("/opt/google/chrome/chrome");
        make_exe(&opt, "");
        assert_eq!(
            detect(spec("chrome"), &env).unwrap(),
            Installation {
                exe: opt,
                kind: InstallKind::Native
            }
        );

        // ...and PATH wins over everything (first listed name first).
        make_exe(&dir.path().join("usr/bin/google-chrome"), "");
        make_exe(&dir.path().join("usr/bin/google-chrome-stable"), "");
        assert_eq!(
            detect(spec("chrome"), &env).unwrap().exe,
            dir.path().join("usr/bin/google-chrome-stable")
        );
    }

    #[test]
    fn linux_snap_detection() {
        let dir = tempfile::tempdir().unwrap();
        let env = fake_env(Os::Linux, dir.path());
        make_exe(&env.sys_path("/snap/bin/chromium"), "");
        let inst = detect(spec("chromium"), &env).unwrap();
        assert_eq!(
            inst.kind,
            InstallKind::Snap {
                name: "chromium".into()
            }
        );

        // Found through PATH, still recognised as a snap.
        let env2 = env
            .clone()
            .with_var("PATH", env.sys_path("/snap/bin").to_string_lossy());
        assert_eq!(
            detect(spec("chromium"), &env2).unwrap().kind,
            InstallKind::Snap {
                name: "chromium".into()
            }
        );

        let profile = confined_profile_dir(
            &inst,
            &env,
            Path::new("/data"),
            Path::new("/data/profiles/chromium"),
        );
        let snap_profiles = env
            .home_path("snap/chromium/current/akev-profiles")
            .unwrap();
        assert!(profile.starts_with(&snap_profiles), "{}", profile.display());
        assert_eq!(profile.file_name().unwrap(), "chromium");
        // Stable for a data dir, distinct between data dirs (two Akev instances never share
        // a confined profile, just like native installs).
        assert_eq!(
            profile,
            confined_profile_dir(
                &inst,
                &env,
                Path::new("/data"),
                Path::new("/data/profiles/chromium")
            )
        );
        let other = confined_profile_dir(
            &inst,
            &env,
            Path::new("/tmp/akev-eph"),
            Path::new("/tmp/akev-eph/profiles/chromium"),
        );
        assert!(other.starts_with(&snap_profiles));
        assert_ne!(profile, other);
    }

    #[test]
    fn flatpak_profile_dir() {
        let inst = Installation {
            exe: PathBuf::from("/var/lib/flatpak/exports/bin/org.mozilla.firefox"),
            kind: InstallKind::Flatpak {
                app_id: "org.mozilla.firefox".into(),
                installation: "/var/lib/flatpak".into(),
            },
        };
        let mut env = HostEnv::empty(Os::Linux);
        env.home = Some(PathBuf::from("/home/u"));
        let p = confined_profile_dir(
            &inst,
            &env,
            Path::new("/d"),
            Path::new("/d/profiles/firefox"),
        );
        assert_eq!(
            p,
            PathBuf::from("/home/u/.var/app/org.mozilla.firefox/data/akev-profiles")
                .join(short_hash(b"/d"))
                .join("firefox")
        );
        assert_ne!(
            p,
            confined_profile_dir(
                &inst,
                &env,
                Path::new("/e"),
                Path::new("/e/profiles/firefox")
            )
        );
        let native = Installation {
            exe: "/usr/bin/firefox".into(),
            kind: InstallKind::Native,
        };
        assert_eq!(
            confined_profile_dir(&native, &env, Path::new("/d"), Path::new("/d/p")),
            PathBuf::from("/d/p")
        );
    }

    #[test]
    fn macos_detection() {
        let dir = tempfile::tempdir().unwrap();
        let env = fake_env(Os::MacOs, dir.path());
        let exe = env
            .home_path("Applications/Brave Browser.app/Contents/MacOS/Brave Browser")
            .unwrap();
        make_exe(&exe, "");
        let inst = detect(spec("brave"), &env).unwrap();
        assert_eq!(inst.exe, exe);
        assert!(matches!(inst.kind, InstallKind::AppBundle { .. }));
        // /Applications takes precedence over ~/Applications.
        let sys_exe = env.sys_path("/Applications/Brave Browser.app/Contents/MacOS/Brave Browser");
        make_exe(&sys_exe, "");
        assert_eq!(detect(spec("brave"), &env).unwrap().exe, sys_exe);
        assert_eq!(detect(spec("chrome"), &env), None);
    }

    #[test]
    fn windows_detection_and_version_dir() {
        let dir = tempfile::tempdir().unwrap();
        let lad = dir.path().join("LocalAppData");
        let env = HostEnv::empty(Os::Windows)
            .with_var("LOCALAPPDATA", lad.to_string_lossy())
            .with_var("ProgramFiles", dir.path().join("PF").to_string_lossy());
        let exe = lad
            .join("Google")
            .join("Chrome")
            .join("Application")
            .join("chrome.exe");
        make_exe(&exe, "");
        std::fs::create_dir_all(exe.parent().unwrap().join("130.0.6723.117")).unwrap();
        std::fs::create_dir_all(exe.parent().unwrap().join("131.0.6778.86")).unwrap();
        std::fs::create_dir_all(exe.parent().unwrap().join("SetupMetrics")).unwrap();
        let inst = detect(spec("chrome"), &env).unwrap();
        assert_eq!(inst.exe, exe);
        assert_eq!(
            windows_version_dir(&inst.exe).as_deref(),
            Some("131.0.6778.86")
        );
        assert_eq!(detect(spec("edge"), &env), None);
    }

    #[tokio::test]
    async fn version_probing() {
        let dir = tempfile::tempdir().unwrap();
        let env = fake_env(Os::Linux, dir.path());
        let exe = dir.path().join("usr/bin/vivaldi");
        make_exe(&exe, "#!/bin/sh\n");
        let runner = MockRunner::new();
        runner.on_prefix(
            "vivaldi --version",
            CommandOutput::ok("Vivaldi 7.0.3495.18 stable\n"),
        );
        let sys = Sys::new(runner.clone(), env.clone());
        let inst = Installation {
            exe: exe.clone(),
            kind: InstallKind::Native,
        };
        assert_eq!(
            version(&sys, &inst, Engine::Chromium).await.as_deref(),
            Some("7.0.3495.18")
        );
        // Cached: no second process.
        assert_eq!(
            version(&sys, &inst, Engine::Chromium).await.as_deref(),
            Some("7.0.3495.18")
        );
        assert_eq!(runner.call_lines().len(), 1);

        // Firefox: application.ini, no process at all.
        let ff = dir.path().join("opt/ff/firefox");
        make_exe(&ff, "");
        std::fs::write(
            dir.path().join("opt/ff/application.ini"),
            "[App]\nVendor=Mozilla\nVersion=132.0.2\n[Gecko]\nVersion=1\n",
        )
        .unwrap();
        let inst = Installation {
            exe: ff,
            kind: InstallKind::Native,
        };
        assert_eq!(
            version(&sys, &inst, Engine::Firefox).await.as_deref(),
            Some("132.0.2")
        );
        assert_eq!(runner.call_lines().len(), 1);

        // Snap metadata.
        make_exe(&env.sys_path("/snap/bin/chromium"), "");
        let meta = env.sys_path("/snap/chromium/current/meta/snap.yaml");
        std::fs::create_dir_all(meta.parent().unwrap()).unwrap();
        std::fs::write(
            &meta,
            "name: chromium\nversion: '131.0.6778.85'\nsummary: x\n",
        )
        .unwrap();
        let inst = Installation {
            exe: env.sys_path("/snap/bin/chromium"),
            kind: InstallKind::Snap {
                name: "chromium".into(),
            },
        };
        assert_eq!(
            version(&sys, &inst, Engine::Chromium).await.as_deref(),
            Some("131.0.6778.85")
        );
    }

    #[test]
    fn version_tokens() {
        assert_eq!(
            parse_version_token("Google Chrome 131.0.6778.85 ").as_deref(),
            Some("131.0.6778.85")
        );
        assert_eq!(
            parse_version_token("Mozilla Firefox 132.0.2").as_deref(),
            Some("132.0.2")
        );
        assert_eq!(
            parse_version_token("Chromium 131.0.6778.85 Fedora Project").as_deref(),
            Some("131.0.6778.85")
        );
        assert_eq!(
            parse_version_token("Mozilla Firefox 133.0b9").as_deref(),
            Some("133.0b9")
        );
        assert_eq!(
            parse_version_token("Opera v114.0.5282.21").as_deref(),
            Some("114.0.5282.21")
        );
        assert_eq!(parse_version_token("no version here."), None);
        assert_eq!(parse_version_token(""), None);
    }

    #[test]
    fn metadata_parsers() {
        let plist = "<dict>\n<key>CFBundleName</key>\n<string>Chrome</string>\n<key>CFBundleShortVersionString</key>\n\t<string>131.0.6778.86</string>\n</dict>";
        assert_eq!(
            plist_string(plist, "CFBundleShortVersionString").as_deref(),
            Some("131.0.6778.86")
        );
        assert_eq!(plist_string(plist, "Missing"), None);
        assert_eq!(
            plist_string("<key>A</key><integer>1</integer><string>x</string>", "A"),
            None
        );
        assert_eq!(
            ini_value("[Gecko]\nVersion=1\n[App]\nVersion = 2\n", "app", "version").as_deref(),
            Some("2")
        );
        assert_eq!(
            yaml_top_level("name: x\nversion: \"1.2\"\n  version: nested\n", "version").as_deref(),
            Some("1.2")
        );
    }
}
