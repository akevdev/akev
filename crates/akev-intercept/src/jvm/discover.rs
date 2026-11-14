//! Finding running JVMs and the Java executables that can attach to them.
//!
//! JVMs are listed with `jps -l` (it reads the JVMs' perf-data files, so it only shows JVMs of
//! the current user that are attachable). Without `jps` — a JRE, or no Java on `PATH` — Akev
//! falls back to the process table: processes started by the `java` launcher, limited on
//! Linux to the current user's processes in the same mount namespace (JVMs in containers
//! can't load an agent from a host path).

use crate::host::{HostEnv, Os, is_executable};
use crate::runner::{CommandSpec, ProcessEntry};
use crate::util::Sys;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Hard limit for `jps -l` (within the registry's 8 s budget for one interceptor's listing).
pub(crate) const JPS_TIMEOUT: Duration = Duration::from_secs(5);
/// Environment variables that inject options (or agents) into every JVM: removed for the JVMs
/// Akev runs itself (`jps`, the attacher).
pub(crate) const JVM_OPTION_VARS: &[&str] =
    &["JAVA_TOOL_OPTIONS", "_JAVA_OPTIONS", "JDK_JAVA_OPTIONS"];
/// Longest command line kept for display.
const MAX_COMMAND_LINE: usize = 400;

/// A running JVM.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct JvmProcess {
    pub pid: u32,
    /// Main class, `-jar` path or `module/class` (empty if unknown).
    pub main: String,
    /// Command line (may be empty or truncated).
    pub command_line: String,
    /// The JVM's installation directory, when known.
    pub java_home: Option<PathBuf>,
    /// `JAVA_VERSION` from the installation's `release` file.
    pub java_version: Option<String>,
}

impl JvmProcess {
    /// Short human name: `com.example.Main` → `Main`, `/srv/app.jar` → `app.jar`.
    pub fn display_name(&self) -> String {
        short_name(&self.main).unwrap_or_else(|| format!("Java process {}", self.pid))
    }
}

/// `org.acme.Main` → `Main`; `/x/app.jar` → `app.jar`; `mod/org.acme.Main` → `Main`.
pub(crate) fn short_name(main: &str) -> Option<String> {
    let main = main.trim();
    if main.is_empty() {
        return None;
    }
    let lower = main.to_ascii_lowercase();
    if lower.ends_with(".jar") || lower.ends_with(".war") {
        return main.rsplit(['/', '\\']).next().map(str::to_string);
    }
    let class = main.rsplit('/').next().unwrap_or(main);
    Some(class.rsplit('.').next().unwrap_or(class).to_string())
}

/// How the process list was obtained.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Method {
    Jps,
    ProcessTable,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Jps => "jps",
            Method::ProcessTable => "process-table",
        }
    }
}

/// Result of one scan.
#[derive(Clone, Debug)]
pub(crate) struct Discovery {
    pub processes: Vec<JvmProcess>,
    pub method: Method,
    /// The `jps` used, if any.
    pub jps: Option<PathBuf>,
    /// Why `jps` wasn't used (not found, failed, timed out).
    pub jps_error: Option<String>,
}

fn exe(os: Os, name: &str) -> String {
    if os == Os::Windows {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// `<home>` of a `<home>/bin/java[w][.exe]` (symlinks resolved).
pub(crate) fn home_of_java_exe(java: &Path) -> Option<PathBuf> {
    let real = std::fs::canonicalize(java).unwrap_or_else(|_| java.to_path_buf());
    let name = real.file_name()?.to_string_lossy().to_ascii_lowercase();
    if !matches!(name.as_str(), "java" | "javaw" | "java.exe" | "javaw.exe") {
        return None;
    }
    let bin = real.parent()?;
    if bin.file_name()? != "bin" {
        return None;
    }
    bin.parent().map(Path::to_path_buf)
}

/// `JAVA_VERSION` of an installation (`<home>/release`; a Java 8 `<jdk>/jre` home has it one
/// level up).
pub(crate) fn release_version(home: &Path) -> Option<String> {
    let mut files = vec![home.join("release")];
    if home.file_name().is_some_and(|n| n == "jre")
        && let Some(parent) = home.parent()
    {
        files.push(parent.join("release"));
    }
    files.iter().find_map(|f| {
        let text = std::fs::read_to_string(f).ok()?;
        text.lines().find_map(|l| {
            let v = l.strip_prefix("JAVA_VERSION=")?.trim().trim_matches('"');
            (!v.is_empty()).then(|| v.to_string())
        })
    })
}

/// Java executables to run the attacher with, best first, existing ones only, without
/// duplicates: the target's own installation (same version; a full JDK has the Attach API),
/// `$JAVA_HOME`, `java` on `PATH`, SDKMAN's current Java and the platform's usual JDK
/// directories. On macOS `/usr/bin/java` is skipped: without a JDK it's a stub that opens an
/// install dialog.
pub(crate) fn java_candidates(env: &HostEnv, target_home: Option<&Path>) -> Vec<PathBuf> {
    let java = exe(env.os, "java");
    let mut list: Vec<PathBuf> = Vec::new();
    if let Some(home) = target_home {
        list.push(home.join("bin").join(&java));
    }
    if let Some(home) = env.var_path("JAVA_HOME") {
        list.push(home.join("bin").join(&java));
    }
    if let Some(p) = env.find_in_path("java")
        && !(env.os == Os::MacOs && p == Path::new("/usr/bin/java"))
    {
        list.push(p);
    }
    if let Some(p) = env.home_path(".sdkman/candidates/java/current/bin/java") {
        list.push(p);
    }
    let mut scan = |dir: PathBuf, suffix: &[&str]| {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return;
        };
        let mut found: Vec<PathBuf> = entries
            .flatten()
            .map(|e| suffix.iter().fold(e.path(), |p, s| p.join(s)))
            .collect();
        // Newest-looking first (`java-25-openjdk` before `java-21-openjdk`).
        found.sort_by_key(|p| std::cmp::Reverse(natural_key(p)));
        list.extend(found);
    };
    match env.os {
        Os::Linux => scan(env.sys_path("/usr/lib/jvm"), &["bin", "java"]),
        Os::MacOs => {
            scan(
                env.sys_path("/Library/Java/JavaVirtualMachines"),
                &["Contents", "Home", "bin", "java"],
            );
            if let Some(dir) = env.home_path("Library/Java/JavaVirtualMachines") {
                scan(dir, &["Contents", "Home", "bin", "java"]);
            }
        }
        Os::Windows => {
            for var in ["PROGRAMFILES", "PROGRAMW6432"] {
                if let Some(pf) = env.var_path(var) {
                    for vendor in [
                        "Java",
                        "Eclipse Adoptium",
                        "Microsoft",
                        "Zulu",
                        "Amazon Corretto",
                    ] {
                        scan(pf.join(vendor), &["bin", "java.exe"]);
                    }
                }
            }
        }
        Os::Other => {}
    }
    let mut seen = Vec::new();
    list.into_iter()
        .filter(|p| is_executable(p))
        .filter(|p| {
            let key = std::fs::canonicalize(p).unwrap_or_else(|_| p.clone());
            if seen.contains(&key) {
                false
            } else {
                seen.push(key);
                true
            }
        })
        .collect()
}

/// Sort key comparing embedded numbers numerically (`jdk-9` < `jdk-17`).
fn natural_key(p: &Path) -> Vec<(u64, String)> {
    let s = p.to_string_lossy();
    let mut out = Vec::new();
    let mut digits = String::new();
    let mut text = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() {
            if !text.is_empty() {
                out.push((0, std::mem::take(&mut text)));
            }
            digits.push(c);
        } else {
            if !digits.is_empty() {
                out.push((digits.parse().unwrap_or(u64::MAX), String::new()));
                digits.clear();
            }
            text.push(c);
        }
    }
    if !digits.is_empty() {
        out.push((digits.parse().unwrap_or(u64::MAX), String::new()));
    }
    if !text.is_empty() {
        out.push((0, text));
    }
    out
}

/// `jps`: `$JAVA_HOME/bin`, `PATH`, or next to the real `java` on `PATH`.
pub(crate) fn find_jps(env: &HostEnv) -> Option<PathBuf> {
    let jps = exe(env.os, "jps");
    if let Some(home) = env.var_path("JAVA_HOME") {
        let p = home.join("bin").join(&jps);
        if is_executable(&p) {
            return Some(p);
        }
    }
    if let Some(p) = env.find_in_path("jps") {
        return Some(p);
    }
    let java = env.find_in_path("java")?;
    let home = home_of_java_exe(&java)?;
    let p = home.join("bin").join(&jps);
    is_executable(&p).then_some(p)
}

/// Parse `jps -l`: `<pid> <main class | jar path | module/class>` per line. JVMs whose
/// details can't be read (other users, stale perf-data files) are skipped.
pub(crate) fn parse_jps(out: &str) -> Vec<(u32, String)> {
    out.lines()
        .filter_map(|line| {
            let line = line.trim();
            let (pid, rest) = match line.split_once(char::is_whitespace) {
                Some((p, r)) => (p, r.trim()),
                None => (line, ""),
            };
            let pid: u32 = pid.parse().ok()?;
            if rest.contains("process information unavailable") {
                return None;
            }
            let main = if rest.contains("information unavailable") {
                ""
            } else {
                rest
            };
            Some((pid, main.to_string()))
        })
        .collect()
}

/// Options of the `java` launcher that take a separate value.
const VALUE_OPTIONS: &[&str] = &[
    "-cp",
    "-classpath",
    "--class-path",
    "-p",
    "--module-path",
    "--upgrade-module-path",
    "--add-modules",
    "--add-reads",
    "--add-exports",
    "--add-opens",
    "--limit-modules",
    "--patch-module",
    "--source",
    "--enable-native-access",
];

/// Main class, `-jar` file or `module/class` of a `java` command line.
pub(crate) fn main_from_cmdline(cmd: &[String]) -> Option<String> {
    let mut args = cmd.iter().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-jar" | "-m" | "--module" => return args.next().cloned(),
            a if VALUE_OPTIONS.contains(&a) => {
                args.next();
            }
            a if a.starts_with("--module=") => return Some(a["--module=".len()..].to_string()),
            a if a.starts_with('-') || a.starts_with('@') => {}
            a => return Some(a.to_string()),
        }
    }
    None
}

/// Started by the `java` launcher (by process name or by `argv[0]`).
pub(crate) fn is_java_launcher(p: &ProcessEntry) -> bool {
    let is_java = |name: &str| {
        let n = name.to_ascii_lowercase();
        matches!(n.as_str(), "java" | "javaw" | "java.exe" | "javaw.exe")
    };
    is_java(&p.name)
        || p.cmd.first().is_some_and(|a| {
            Path::new(a)
                .file_name()
                .is_some_and(|f| is_java(&f.to_string_lossy()))
        })
}

/// JVMs that are Akev's own helpers or JDK tools (jps, jcmd, keytool, javac, jar, jshell…),
/// never offered as targets.
pub(crate) fn is_tool(main: &str) -> bool {
    if main == "dev.akev.Attach" || main.ends_with("/dev.akev.Attach") {
        return true;
    }
    // Java 9+: tools run from the JDK's own modules (`jdk.jcmd/sun.tools.jps.Jps`).
    if let Some((module, _)) = main.split_once('/')
        && (module.starts_with("jdk.") || module.starts_with("java."))
    {
        return true;
    }
    // Java 8: no module prefix.
    ["sun.tools.", "sun.security.tools.", "com.sun.tools."]
        .iter()
        .any(|p| main.starts_with(p))
}

/// Installation directory of a running JVM: on Linux from `/proc/<pid>/exe`, elsewhere from
/// an absolute `java` path in its command line.
pub(crate) fn java_home_of(
    env: &HostEnv,
    pid: u32,
    proc_entry: Option<&ProcessEntry>,
) -> Option<PathBuf> {
    if env.os == Os::Linux
        && let Ok(exe) = std::fs::read_link(env.sys_path(&format!("/proc/{pid}/exe")))
    {
        // A JDK updated while the JVM runs shows up as `/path/bin/java (deleted)`.
        let exe = PathBuf::from(
            exe.to_string_lossy()
                .trim_end_matches(" (deleted)")
                .to_string(),
        );
        let under_root = env.sys_path(&exe.to_string_lossy());
        if let Some(home) = [exe, under_root]
            .iter()
            .find(|p| p.is_file())
            .and_then(|p| home_of_java_exe(p))
        {
            return Some(home);
        }
    }
    let first = proc_entry?.cmd.first()?;
    let path = Path::new(first);
    if path.is_absolute() {
        home_of_java_exe(path)
    } else {
        None
    }
}

/// Linux: whether `pid` runs as the current user in Akev's mount namespace (otherwise the
/// JVM can't be attached to, or can't see the agent jar). Always true elsewhere.
pub(crate) fn attachable_here(env: &HostEnv, pid: u32) -> bool {
    #[cfg(unix)]
    if env.os == Os::Linux {
        use std::os::unix::fs::MetadataExt;
        let dir = env.sys_path(&format!("/proc/{pid}"));
        let Ok(meta) = std::fs::metadata(&dir) else {
            return false;
        };
        // SAFETY: getuid has no preconditions and can't fail.
        if meta.uid() != unsafe { libc::getuid() } {
            return false;
        }
        let ours = std::fs::read_link(env.sys_path("/proc/self/ns/mnt"));
        let theirs = std::fs::read_link(dir.join("ns/mnt"));
        if let (Ok(a), Ok(b)) = (ours, theirs) {
            return a == b;
        }
        return true;
    }
    let _ = (env, pid);
    true
}

fn truncate(s: String) -> String {
    if s.chars().count() <= MAX_COMMAND_LINE {
        return s;
    }
    let mut t: String = s.chars().take(MAX_COMMAND_LINE - 1).collect();
    t.push('…');
    t
}

/// List the JVMs this user can attach to.
pub(crate) async fn discover(sys: &Sys) -> Discovery {
    let procs = sys.processes(false).await;
    let by_pid: HashMap<u32, &ProcessEntry> = procs.iter().map(|p| (p.pid, p)).collect();
    let own_pid = std::process::id();
    let jps = find_jps(&sys.env);
    let mut jps_error = None;
    let listed = match &jps {
        None => {
            jps_error = Some("jps not found (install a JDK or set JAVA_HOME)".to_string());
            None
        }
        Some(path) => {
            let mut spec = CommandSpec::new(path).arg("-l").timeout(JPS_TIMEOUT);
            for var in JVM_OPTION_VARS {
                spec = spec.env_remove(*var);
            }
            match sys.runner.output(&spec).await {
                Ok(out) if out.success() => Some(parse_jps(&out.stdout)),
                Ok(out) => {
                    jps_error = Some(format!("jps failed: {}", out.error_summary()));
                    None
                }
                Err(e) => {
                    jps_error = Some(format!("jps failed: {e}"));
                    None
                }
            }
        }
    };
    let (method, found): (Method, Vec<(u32, String)>) = match listed {
        Some(list) => (Method::Jps, list),
        None => (
            Method::ProcessTable,
            procs
                .iter()
                .filter(|p| is_java_launcher(p) && attachable_here(&sys.env, p.pid))
                .map(|p| (p.pid, main_from_cmdline(&p.cmd).unwrap_or_default()))
                .collect(),
        ),
    };
    let mut versions: HashMap<PathBuf, Option<String>> = HashMap::new();
    let mut processes: Vec<JvmProcess> = found
        .into_iter()
        .filter(|(pid, main)| *pid != own_pid && !is_tool(main))
        .map(|(pid, main)| {
            let entry = by_pid.get(&pid).copied();
            let main = if main.is_empty() {
                entry
                    .and_then(|p| main_from_cmdline(&p.cmd))
                    .unwrap_or_default()
            } else {
                main
            };
            let java_home = java_home_of(&sys.env, pid, entry);
            let java_version = java_home.as_ref().and_then(|h| {
                versions
                    .entry(h.clone())
                    .or_insert_with(|| release_version(h))
                    .clone()
            });
            JvmProcess {
                pid,
                main,
                command_line: truncate(entry.map(|p| p.cmd.join(" ")).unwrap_or_default()),
                java_home,
                java_version,
            }
        })
        .filter(|p| !is_tool(&p.main))
        .collect();
    processes.sort_by_key(|p| p.pid);
    Discovery {
        processes,
        method,
        jps,
        jps_error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::testutil::{fake_env, make_exe};
    use crate::runner::CommandOutput;
    use crate::runner::mock::MockRunner;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn jps_output() {
        let out = "4242 org.acme.Main\n17 /srv/app.jar --port 8080\n99 jdk.jcmd/sun.tools.jps.Jps\n\
                   5 -- process information unavailable\n6 -- main class information unavailable\n7\nnot a pid\n";
        assert_eq!(
            parse_jps(out),
            vec![
                (4242, "org.acme.Main".to_string()),
                (17, "/srv/app.jar --port 8080".to_string()),
                (99, "jdk.jcmd/sun.tools.jps.Jps".to_string()),
                (6, String::new()),
                (7, String::new()),
            ]
        );
        assert!(is_tool("jdk.jcmd/sun.tools.jps.Jps"));
        assert!(is_tool("java.base/sun.security.tools.keytool.Main"));
        assert!(is_tool("jdk.compiler/com.sun.tools.javac.Main"));
        assert!(is_tool("sun.tools.jps.Jps"), "Java 8");
        assert!(is_tool("com.sun.tools.javac.Main"), "Java 8");
        assert!(is_tool("dev.akev.Attach"));
        assert!(!is_tool("org.acme.Main"));
        assert!(
            !is_tool("acme.app/org.acme.Main"),
            "applications run as modules"
        );
        assert!(!is_tool("/opt/jdk.tools/app.jar"));
        assert!(!is_tool(""));
    }

    #[test]
    fn main_class_from_command_lines() {
        assert_eq!(
            main_from_cmdline(&s(&[
                "java",
                "-Xmx1g",
                "-cp",
                "a.jar:b.jar",
                "-Dx=y",
                "org.acme.Main",
                "--flag"
            ])),
            Some("org.acme.Main".into())
        );
        assert_eq!(
            main_from_cmdline(&s(&[
                "/usr/bin/java",
                "-javaagent:x.jar",
                "-jar",
                "/srv/app.jar",
                "serve"
            ])),
            Some("/srv/app.jar".into())
        );
        assert_eq!(
            main_from_cmdline(&s(&[
                "java",
                "--module-path",
                "mods",
                "-m",
                "acme.app/org.acme.Main"
            ])),
            Some("acme.app/org.acme.Main".into())
        );
        assert_eq!(
            main_from_cmdline(&s(&["java", "--module=acme.app/org.acme.Main"])),
            Some("acme.app/org.acme.Main".into())
        );
        assert_eq!(
            main_from_cmdline(&s(&[
                "java",
                "@argfile",
                "--add-opens",
                "java.base/java.lang=ALL-UNNAMED",
                "Main"
            ])),
            Some("Main".into())
        );
        assert_eq!(main_from_cmdline(&s(&["java", "-version"])), None);
        assert_eq!(main_from_cmdline(&[]), None);
    }

    #[test]
    fn short_names() {
        assert_eq!(short_name("org.acme.Main").as_deref(), Some("Main"));
        assert_eq!(
            short_name("/srv/my app/app.jar").as_deref(),
            Some("app.jar")
        );
        assert_eq!(
            short_name("C:\\apps\\tool.JAR").as_deref(),
            Some("tool.JAR")
        );
        assert_eq!(
            short_name("acme.app/org.acme.Main").as_deref(),
            Some("Main")
        );
        assert_eq!(short_name("Main").as_deref(), Some("Main"));
        assert_eq!(short_name("  "), None);
        let p = JvmProcess {
            pid: 7,
            ..Default::default()
        };
        assert_eq!(p.display_name(), "Java process 7");
    }

    #[test]
    fn java_launcher_detection() {
        let entry = |name: &str, cmd: &[&str]| ProcessEntry {
            pid: 1,
            name: name.into(),
            cmd: s(cmd),
            ..Default::default()
        };
        assert!(is_java_launcher(&entry("java", &[])));
        assert!(is_java_launcher(&entry("javaw.exe", &[])));
        assert!(is_java_launcher(&entry(
            "main",
            &["/opt/jdk/bin/java", "Main"]
        )));
        assert!(!is_java_launcher(&entry("javac", &["javac", "A.java"])));
        assert!(!is_java_launcher(&entry("bash", &["bash", "-c", "java"])));
    }

    #[test]
    fn release_files_and_homes() {
        let dir = tempfile::tempdir().unwrap();
        let jdk = dir.path().join("jdk-21");
        make_exe(&jdk.join("bin/java"), "");
        std::fs::write(
            jdk.join("release"),
            "IMPLEMENTOR=\"X\"\nJAVA_VERSION=\"21.0.2\"\n",
        )
        .unwrap();
        assert_eq!(
            home_of_java_exe(&jdk.join("bin/java")),
            Some(std::fs::canonicalize(&jdk).unwrap())
        );
        assert_eq!(release_version(&jdk).as_deref(), Some("21.0.2"));
        // Java 8: java.home is <jdk>/jre and the release file is one level up.
        let jdk8 = dir.path().join("jdk8");
        make_exe(&jdk8.join("jre/bin/java"), "");
        std::fs::write(jdk8.join("release"), "JAVA_VERSION=\"1.8.0_392\"\n").unwrap();
        assert_eq!(
            release_version(&jdk8.join("jre")).as_deref(),
            Some("1.8.0_392")
        );
        assert_eq!(release_version(dir.path()), None);
        // Not a java binary.
        make_exe(&jdk.join("bin/javac"), "");
        assert_eq!(home_of_java_exe(&jdk.join("bin/javac")), None);
        make_exe(&dir.path().join("java"), "");
        assert_eq!(
            home_of_java_exe(&dir.path().join("java")),
            None,
            "not in a bin/ dir"
        );
    }

    #[test]
    fn candidates_order_and_dedup() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let target = root.join("opt/jdk-17");
        make_exe(&target.join("bin/java"), "");
        let java_home = root.join("opt/jdk-21");
        make_exe(&java_home.join("bin/java"), "");
        make_exe(&root.join("usr/bin/java"), "");
        for v in ["java-11-openjdk", "java-25-openjdk", "java-9"] {
            make_exe(&root.join("usr/lib/jvm").join(v).join("bin/java"), "");
        }
        std::fs::create_dir_all(root.join("usr/lib/jvm/jre-broken")).unwrap();
        let env = fake_env(Os::Linux, root).with_var("JAVA_HOME", java_home.to_string_lossy());
        let list = java_candidates(&env, Some(&target));
        assert_eq!(
            list,
            vec![
                target.join("bin/java"),
                java_home.join("bin/java"),
                root.join("usr/bin/java"),
                root.join("usr/lib/jvm/java-25-openjdk/bin/java"),
                root.join("usr/lib/jvm/java-11-openjdk/bin/java"),
                root.join("usr/lib/jvm/java-9/bin/java"),
            ]
        );
        // The same installation twice is listed once.
        let again = java_candidates(&env, Some(&java_home));
        assert_eq!(
            again
                .iter()
                .filter(|p| **p == java_home.join("bin/java"))
                .count(),
            1
        );
        // Nothing installed: nothing to offer.
        let empty = tempfile::tempdir().unwrap();
        assert!(java_candidates(&fake_env(Os::Linux, empty.path()), None).is_empty());
    }

    #[test]
    fn macos_skips_the_java_stub() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut env = fake_env(Os::MacOs, root);
        env = env.with_var("PATH", "/usr/bin");
        let jdk = root.join("Library/Java/JavaVirtualMachines/temurin-21.jdk/Contents/Home");
        make_exe(&jdk.join("bin/java"), "");
        let list = java_candidates(&env, None);
        assert!(!list.contains(&PathBuf::from("/usr/bin/java")));
        assert_eq!(list, vec![jdk.join("bin/java")]);
    }

    #[test]
    fn jps_lookup() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let env = fake_env(Os::Linux, root);
        assert_eq!(find_jps(&env), None);
        // Only `java` on PATH (a symlink into a JDK): jps is found next to the real binary.
        let jdk = root.join("usr/lib/jvm/java-21");
        make_exe(&jdk.join("bin/java"), "");
        make_exe(&jdk.join("bin/jps"), "");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(jdk.join("bin/java"), root.join("usr/bin/java")).unwrap();
            assert_eq!(
                find_jps(&env),
                Some(std::fs::canonicalize(&jdk).unwrap().join("bin/jps"))
            );
        }
        make_exe(&root.join("usr/bin/jps"), "");
        assert_eq!(find_jps(&env), Some(root.join("usr/bin/jps")));
        let home = root.join("opt/jdk");
        make_exe(&home.join("bin/jps"), "");
        let env = env.with_var("JAVA_HOME", home.to_string_lossy());
        assert_eq!(find_jps(&env), Some(home.join("bin/jps")));
    }

    fn machine(root: &Path) -> (Arc<MockRunner>, Arc<Sys>) {
        let runner = MockRunner::new();
        let env = fake_env(Os::Linux, root);
        let sys = Sys::new(runner.clone(), env);
        (runner, sys)
    }

    use std::sync::Arc;

    #[tokio::test]
    async fn discovers_with_jps_and_enriches_from_the_process_table() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let jdk = root.join("usr/lib/jvm/java-21");
        make_exe(&jdk.join("bin/java"), "");
        std::fs::write(jdk.join("release"), "JAVA_VERSION=\"21.0.5\"\n").unwrap();
        make_exe(&root.join("usr/bin/jps"), "");
        #[cfg(unix)]
        {
            std::fs::create_dir_all(root.join("proc/4242")).unwrap();
            std::os::unix::fs::symlink(jdk.join("bin/java"), root.join("proc/4242/exe")).unwrap();
        }
        let (runner, sys) = machine(root);
        runner.on_prefix(
            "jps -l",
            CommandOutput::ok(format!(
                "4242 org.acme.Main\n77 jdk.jcmd/sun.tools.jps.Jps\n{} dev.akev.Attach\n88\n",
                std::process::id() + 1
            )),
        );
        runner.processes.lock().extend([
            ProcessEntry {
                pid: 4242,
                name: "java".into(),
                cmd: s(&["java", "-cp", "app", "org.acme.Main"]),
                ..Default::default()
            },
            ProcessEntry {
                pid: 88,
                name: "java".into(),
                cmd: s(&["java", "-jar", "/srv/tool.jar"]),
                ..Default::default()
            },
        ]);
        let d = discover(&sys).await;
        assert_eq!(d.method, Method::Jps);
        assert_eq!(d.jps_error, None);
        let pids: Vec<u32> = d.processes.iter().map(|p| p.pid).collect();
        assert_eq!(pids, vec![88, 4242], "tools filtered, sorted by pid");
        let acme = &d.processes[1];
        assert_eq!(acme.main, "org.acme.Main");
        assert_eq!(acme.command_line, "java -cp app org.acme.Main");
        #[cfg(unix)]
        {
            assert_eq!(acme.java_home, Some(std::fs::canonicalize(&jdk).unwrap()));
            assert_eq!(acme.java_version.as_deref(), Some("21.0.5"));
        }
        // jps had no main class for 88: taken from its command line.
        assert_eq!(d.processes[0].main, "/srv/tool.jar");
        // jps runs without injected JVM options.
        let call = runner.calls.lock()[0].clone();
        assert!(call.env_remove.iter().any(|k| k == "JAVA_TOOL_OPTIONS"));
        assert_eq!(call.timeout, JPS_TIMEOUT);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn falls_back_to_the_process_table_without_jps() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let (runner, sys) = machine(root);
        // Attachability is checked through <root>/proc: create entries owned by us.
        for pid in [500, 501] {
            std::fs::create_dir_all(root.join(format!("proc/{pid}/ns"))).unwrap();
        }
        runner.processes.lock().extend([
            ProcessEntry {
                pid: 500,
                name: "java".into(),
                cmd: s(&["/opt/jdk/bin/java", "-jar", "/srv/app.jar"]),
                ..Default::default()
            },
            ProcessEntry {
                pid: 501,
                name: "bash".into(),
                cmd: s(&["bash"]),
                ..Default::default()
            },
            // Not in our /proc view (other namespace / user): skipped.
            ProcessEntry {
                pid: 502,
                name: "java".into(),
                cmd: s(&["java", "Main"]),
                ..Default::default()
            },
        ]);
        let d = discover(&sys).await;
        assert_eq!(d.method, Method::ProcessTable);
        assert!(d.jps_error.as_deref().unwrap().contains("jps not found"));
        assert_eq!(d.processes.len(), 1);
        assert_eq!(d.processes[0].pid, 500);
        assert_eq!(d.processes[0].main, "/srv/app.jar");
        assert_eq!(d.processes[0].display_name(), "app.jar");
        assert!(runner.calls.lock().is_empty(), "no commands without jps");

        // A failing jps also falls back.
        make_exe(&root.join("usr/bin/jps"), "");
        runner.on_prefix("jps", CommandOutput::failed("Error: could not attach"));
        let d = discover(&sys).await;
        assert_eq!(d.method, Method::ProcessTable);
        assert!(d.jps_error.as_deref().unwrap().contains("could not attach"));
    }

    #[test]
    fn natural_sort_keys() {
        let mut v = vec![
            PathBuf::from("jdk-9"),
            PathBuf::from("jdk-17"),
            PathBuf::from("jdk-11"),
        ];
        v.sort_by_key(|p| std::cmp::Reverse(natural_key(p)));
        assert_eq!(
            v,
            vec![
                PathBuf::from("jdk-17"),
                PathBuf::from("jdk-11"),
                PathBuf::from("jdk-9")
            ]
        );
    }
}
