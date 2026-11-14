//! The embedded Java agent jar and the `dev.akev.Attach` helper that loads it into a JVM.

use crate::runner::{CommandOutput, CommandSpec};
use crate::util::{atomic_write, short_hash};
use serde_json::{Map, Value, json};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::discover::JVM_OPTION_VARS;

/// The agent jar built by `build.rs`; empty when Akev was built without a JDK.
pub(crate) static AGENT_JAR: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/akev-agent.jar"));

/// Whether this build carries the JVM agent.
pub(crate) fn agent_available() -> bool {
    !AGENT_JAR.is_empty()
}

/// Upper bound for one attach: starting a JVM, the attach handshake (the target's attach
/// listener starts on demand) and the agent's work.
pub(crate) const ATTACH_TIMEOUT: Duration = Duration::from_secs(45);

/// `dev.akev.Attach` exit codes.
pub(crate) mod exit {
    pub const USAGE: i32 = 2;
    pub const NO_ATTACH_API: i32 = 3;
    pub const ATTACH_FAILED: i32 = 4;
    pub const LOAD_FAILED: i32 = 5;
    pub const AGENT_ERROR: i32 = 6;
    pub const NO_STATUS: i32 = 7;
}

/// `<data_dir>/jvm/akev-agent-<hash>.jar`, written if missing. Content-addressed because a JVM
/// keeps the jar it loaded open: a new Akev version never rewrites a jar in use.
pub(crate) fn ensure_agent_jar(data_dir: &Path) -> io::Result<PathBuf> {
    let path = data_dir
        .join("jvm")
        .join(format!("akev-agent-{}.jar", short_hash(AGENT_JAR)));
    let current = std::fs::metadata(&path).map(|m| m.len()).ok();
    if current != Some(AGENT_JAR.len() as u64) {
        atomic_write(&path, AGENT_JAR)?;
    }
    Ok(path)
}

/// What the attacher should do in the target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AttachAction {
    Apply {
        /// `host:port` of the proxy.
        proxy: String,
        /// Akev CA (PEM).
        ca: PathBuf,
        intercept_loopback: bool,
        /// `auto` / `always` / `never` (`None` = the agent's default, `auto`).
        proxy_properties: Option<String>,
    },
    Restore,
}

/// `java -cp <jar> dev.akev.Attach <pid> ...`, without inherited JVM option variables.
pub(crate) fn attach_spec(java: &Path, jar: &Path, pid: u32, action: &AttachAction) -> CommandSpec {
    let mut spec = CommandSpec::new(java)
        .arg("-cp")
        .arg(jar)
        .arg("dev.akev.Attach")
        .arg(pid.to_string())
        .timeout(ATTACH_TIMEOUT);
    match action {
        AttachAction::Apply {
            proxy,
            ca,
            intercept_loopback,
            proxy_properties,
        } => {
            spec = spec.arg("--proxy").arg(proxy).arg("--ca").arg(ca);
            if *intercept_loopback {
                spec = spec.arg("--intercept-loopback");
            }
            if let Some(mode) = proxy_properties {
                spec = spec.arg("--proxy-properties").arg(mode);
            }
        }
        AttachAction::Restore => spec = spec.arg("--restore"),
    }
    for var in JVM_OPTION_VARS {
        spec = spec.env_remove(*var);
    }
    spec
}

/// The attacher's `key=value` report (the agent's status, or the attacher's own error).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct AttachReport {
    pub fields: Vec<(String, String)>,
}

impl AttachReport {
    pub fn parse(stdout: &str) -> Self {
        let fields = stdout
            .lines()
            .filter_map(|l| {
                let (k, v) = l.trim_end_matches('\r').split_once('=')?;
                let k = k.trim();
                let valid = !k.is_empty()
                    && k.chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.');
                valid.then(|| (k.to_string(), v.trim().to_string()))
            })
            .collect();
        AttachReport { fields }
    }

    /// Last value of `key`.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .filter(|v| !v.is_empty())
    }

    pub fn all(&self, key: &str) -> Vec<&str> {
        self.fields
            .iter()
            .filter(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .filter(|v| !v.is_empty())
            .collect()
    }

    pub fn ok(&self) -> bool {
        self.get("result") == Some("ok")
    }

    /// Everything as a JSON object; repeated `warning`s become `warnings: [...]`.
    pub fn to_json(&self) -> Value {
        let mut map = Map::new();
        for (k, v) in &self.fields {
            if k != "warning" {
                map.insert(k.clone(), json!(v));
            }
        }
        map.insert("warnings".into(), json!(self.all("warning")));
        Value::Object(map)
    }
}

/// Did this `java` fail to even run the attacher (too old, broken install, no Attach API), so
/// the next Java candidate should be tried?
pub(crate) fn try_next_java(out: &CommandOutput, report: &AttachReport) -> bool {
    match out.code {
        Some(exit::NO_ATTACH_API) => true,
        // The JVM didn't start or couldn't load the class (e.g. Java 7: "Unsupported
        // major.minor version 52.0"): the attacher reports nothing.
        Some(code) if code != 0 => report.get("result").is_none(),
        None => report.get("result").is_none(),
        _ => false,
    }
}

/// Plain-language explanation of a failed attach / restore.
pub(crate) fn explain_failure(out: &CommandOutput, report: &AttachReport, target: &str) -> String {
    let detail = report
        .get("error")
        .map(str::to_string)
        .unwrap_or_else(|| out.error_summary());
    match out.code {
        Some(exit::ATTACH_FAILED) => format!(
            "Couldn't attach to {target}: {detail}. The JVM must run as your user on this machine \
             (not in a container) and not have been started with -XX:+DisableAttachMechanism."
        ),
        Some(exit::LOAD_FAILED) => format!(
            "{target} refused the Akev agent: {detail}. If it runs with \
             -XX:-EnableDynamicAgentLoading, restart it without that option."
        ),
        Some(exit::AGENT_ERROR) => format!("The Akev agent couldn't configure {target}: {detail}"),
        Some(exit::NO_STATUS) => detail,
        Some(exit::NO_ATTACH_API) => format!(
            "The Java Attach API isn't available ({detail}). Install a JDK (not just a JRE) or point JAVA_HOME at one."
        ),
        Some(exit::USAGE) => format!("Internal error running the attacher: {detail}"),
        _ => format!("Attaching to {target} failed: {detail}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg_attr(akev_no_agent, ignore = "built without a JDK")]
    fn embedded_jar_is_a_complete_agent() {
        let has = |needle: &str| memchr::memmem::find(AGENT_JAR, needle.as_bytes()).is_some();
        assert!(AGENT_JAR.starts_with(b"PK\x03\x04"), "a zip/jar");
        for class in [
            "dev/akev/AkevAgent.class",
            "dev/akev/Attach.class",
            "dev/akev/Interception.class",
            "dev/akev/AkevTrustManager.class",
            "dev/akev/AkevProxySelector.class",
            "META-INF/MANIFEST.MF",
        ] {
            assert!(has(class), "{class} missing from the agent jar");
        }
        assert!(has("Agent-Class: dev.akev.AkevAgent"));
        assert!(has("Premain-Class: dev.akev.AkevAgent"));
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("java");
        let hash = super::super::source_hash::java_sources_hash(&dir).unwrap();
        assert!(
            has(&format!("Akev-Source-Hash: {hash}")),
            "embedded jar built from these sources"
        );
    }

    #[test]
    #[cfg_attr(akev_no_agent, ignore = "built without a JDK")]
    fn the_agent_is_java_8_bytecode() {
        // Major version 52 (0x34): loads into any JVM ≥ 8.
        let class = memchr::memmem::find(AGENT_JAR, b"\xCA\xFE\xBA\xBE").unwrap();
        assert_eq!(&AGENT_JAR[class + 6..class + 8], &[0x00, 0x34]);
    }

    #[test]
    #[cfg_attr(akev_no_agent, ignore = "built without a JDK")]
    fn agent_jar_written_once_per_version() {
        let dir = tempfile::tempdir().unwrap();
        let p = ensure_agent_jar(dir.path()).unwrap();
        assert!(p.starts_with(dir.path().join("jvm")));
        assert!(
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("akev-agent-")
        );
        assert_eq!(std::fs::read(&p).unwrap(), AGENT_JAR);
        let mtime = std::fs::metadata(&p).unwrap().modified().unwrap();
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(ensure_agent_jar(dir.path()).unwrap(), p);
        assert_eq!(
            std::fs::metadata(&p).unwrap().modified().unwrap(),
            mtime,
            "not rewritten"
        );
        // A truncated file is repaired.
        std::fs::write(&p, b"PK").unwrap();
        ensure_agent_jar(dir.path()).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), AGENT_JAR);
    }

    #[test]
    fn attach_commands() {
        let apply = AttachAction::Apply {
            proxy: "127.0.0.1:8000".into(),
            ca: PathBuf::from("/d/ca.pem"),
            intercept_loopback: true,
            proxy_properties: Some("never".into()),
        };
        let spec = attach_spec(
            Path::new("/jdk/bin/java"),
            Path::new("/d/jvm/a.jar"),
            42,
            &apply,
        );
        assert_eq!(
            spec.to_string(),
            "/jdk/bin/java -cp /d/jvm/a.jar dev.akev.Attach 42 --proxy 127.0.0.1:8000 --ca /d/ca.pem --intercept-loopback --proxy-properties never"
        );
        assert_eq!(spec.timeout, ATTACH_TIMEOUT);
        for var in JVM_OPTION_VARS {
            assert!(spec.env_remove.iter().any(|k| k == var));
        }
        let restore = attach_spec(
            Path::new("java"),
            Path::new("a.jar"),
            7,
            &AttachAction::Restore,
        );
        assert_eq!(
            restore.to_string(),
            "java -cp a.jar dev.akev.Attach 7 --restore"
        );
        let plain = AttachAction::Apply {
            proxy: "[::1]:8000".into(),
            ca: PathBuf::from("ca.pem"),
            intercept_loopback: false,
            proxy_properties: None,
        };
        assert_eq!(
            attach_spec(Path::new("java"), Path::new("a.jar"), 7, &plain).to_string(),
            "java -cp a.jar dev.akev.Attach 7 --proxy [::1]:8000 --ca ca.pem"
        );
    }

    #[test]
    fn reports() {
        let r = AttachReport::parse(
            "action=apply\nproxy=127.0.0.1:8000\nwarning=one\nwarning=two = with equals\n\
             garbage line\n=novalue\nresult=ok\r\nmessage=Done\n",
        );
        assert!(r.ok());
        assert_eq!(r.get("proxy"), Some("127.0.0.1:8000"));
        assert_eq!(r.all("warning"), vec!["one", "two = with equals"]);
        assert_eq!(r.get("missing"), None);
        let j = r.to_json();
        assert_eq!(j["message"], "Done");
        assert_eq!(j["warnings"], json!(["one", "two = with equals"]));
        assert!(j.get("warning").is_none());
        assert!(!AttachReport::parse("result=error\nerror=x").ok());
        assert!(!AttachReport::default().ok());
    }

    #[test]
    fn failures_are_explained_and_retried_with_other_javas() {
        let out = |code: i32, stdout: &str, stderr: &str| CommandOutput {
            code: Some(code),
            stdout: stdout.into(),
            stderr: stderr.into(),
        };
        let attach = out(
            4,
            "result=error\nstage=attach\nerror=com.sun.tools.attach.AttachNotSupportedException: Unable to open socket file\n",
            "",
        );
        let r = AttachReport::parse(&attach.stdout);
        assert!(!try_next_java(&attach, &r));
        let msg = explain_failure(&attach, &r, "Main (PID 42)");
        assert!(msg.starts_with(
            "Couldn't attach to Main (PID 42): com.sun.tools.attach.AttachNotSupportedException"
        ));
        assert!(msg.contains("DisableAttachMechanism"));

        let no_api = out(
            3,
            "result=error\nstage=attach-api\nerror=ClassNotFoundException\n",
            "",
        );
        assert!(try_next_java(&no_api, &AttachReport::parse(&no_api.stdout)));
        assert!(
            explain_failure(&no_api, &AttachReport::parse(&no_api.stdout), "x")
                .contains("Install a JDK")
        );

        // Java 7 can't load the attacher: no report → next candidate.
        let old = out(
            1,
            "",
            "Exception in thread \"main\" java.lang.UnsupportedClassVersionError: Unsupported major.minor version 52.0",
        );
        let r = AttachReport::parse(&old.stdout);
        assert!(try_next_java(&old, &r));
        assert!(explain_failure(&old, &r, "x").contains("UnsupportedClassVersionError"));

        let agent = out(
            6,
            "result=error\nerror=java.io.FileNotFoundException: /x/ca.pem\nmessage=Akev agent failed\n",
            "",
        );
        let r = AttachReport::parse(&agent.stdout);
        assert!(!try_next_java(&agent, &r));
        assert_eq!(
            explain_failure(&agent, &r, "Main (PID 1)"),
            "The Akev agent couldn't configure Main (PID 1): java.io.FileNotFoundException: /x/ca.pem"
        );
        let refused = out(
            5,
            "result=error\nstage=load\nerror=com.sun.tools.attach.AgentLoadException: nope\n",
            "",
        );
        assert!(
            explain_failure(&refused, &AttachReport::parse(&refused.stdout), "x")
                .contains("refused the Akev agent")
        );
        let ok = out(0, "result=ok\n", "");
        assert!(!try_next_java(&ok, &AttachReport::parse(&ok.stdout)));
    }
}
