//! The JVM agent against a real JDK: its Java self-test (with and without access to JSSE
//! internals), `-javaagent:` startup with a real `Instrumentation`, and the `Attach` helper
//! attaching to a live process. Skipped (with a note) when no JDK (`java`, `javac`,
//! `keytool`) is installed.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The jar `build.rs` produced and the crate embeds.
const AGENT_JAR: &str = concat!(env!("OUT_DIR"), "/akev-agent.jar");
const JAVA_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/java");

struct Jdk {
    java: PathBuf,
    javac: PathBuf,
    keytool: PathBuf,
}

/// Major version of a `java` binary (8 for "1.8.0_x", 11 for "11.0.2", ...); 0 if unknown.
fn java_major(java: &std::path::Path) -> u32 {
    let out = match Command::new(java).arg("-version").output() {
        Ok(o) => o,
        Err(_) => return 0,
    };
    let text = String::from_utf8_lossy(&out.stderr);
    let Some(start) = text.find('"') else {
        return 0;
    };
    let version = text[start + 1..].split('"').next().unwrap_or("");
    let mut parts = version.split(['.', '_', '-', '+']);
    match parts.next().and_then(|p| p.parse::<u32>().ok()) {
        Some(1) => parts.next().and_then(|p| p.parse().ok()).unwrap_or(0),
        Some(n) => n,
        None => 0,
    }
}

fn find_tool(name: &str) -> Option<PathBuf> {
    let exe = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    if let Some(home) = std::env::var_os("JAVA_HOME").filter(|h| !h.is_empty()) {
        let p = PathBuf::from(home).join("bin").join(&exe);
        if p.is_file() {
            return Some(p);
        }
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(&exe))
        .find(|p| p.is_file())
}

fn jdk() -> Option<Jdk> {
    let jdk = Jdk {
        java: find_tool("java")?,
        javac: find_tool("javac")?,
        keytool: find_tool("keytool")?,
    };
    Some(jdk)
}

macro_rules! require_jdk {
    () => {
        match jdk() {
            Some(j) => j,
            None => {
                eprintln!("skipping: no JDK (java, javac, keytool) found");
                return;
            }
        }
    };
}

fn run(cmd: &mut Command) -> std::process::Output {
    let out = cmd.output().expect("run command");
    assert!(
        out.status.success(),
        "{cmd:?} failed: {}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn keytool(jdk: &Jdk, dir: &Path, args: &[&str]) {
    run(Command::new(&jdk.keytool)
        .current_dir(dir)
        .args(args)
        .args(["-storepass", "changeit"]));
}

/// Akev CA + a server certificate it issued, a separately trusted self-signed server
/// ("other") and an untrusted one — all valid for 127.0.0.1.
fn make_certs(jdk: &Jdk, dir: &Path) {
    let ec = [
        "-keyalg",
        "EC",
        // Java 8's keytool has no -groupname; 256-bit EC is secp256r1 on every version.
        "-keysize",
        "256",
        "-validity",
        "3",
        "-storetype",
        "PKCS12",
    ];
    let genkey = |store: &str, alias: &str, dname: &str, ext: &[&str]| {
        let mut args = vec![
            "-genkeypair",
            "-alias",
            alias,
            "-dname",
            dname,
            "-keystore",
            store,
        ];
        args.extend(ec);
        args.extend(ext);
        keytool(jdk, dir, &args);
    };
    genkey("akev-ca.p12", "ca", "CN=Akev Test CA", &["-ext", "bc:c"]);
    keytool(
        jdk,
        dir,
        &[
            "-exportcert",
            "-rfc",
            "-alias",
            "ca",
            "-keystore",
            "akev-ca.p12",
            "-file",
            "akev-ca.pem",
        ],
    );
    genkey("akev-leaf.p12", "leaf", "CN=leaf.test", &[]);
    keytool(
        jdk,
        dir,
        &[
            "-certreq",
            "-alias",
            "leaf",
            "-keystore",
            "akev-leaf.p12",
            "-file",
            "leaf.csr",
        ],
    );
    keytool(
        jdk,
        dir,
        &[
            "-gencert",
            "-alias",
            "ca",
            "-keystore",
            "akev-ca.p12",
            "-infile",
            "leaf.csr",
            "-outfile",
            "leaf.pem",
            "-rfc",
            "-validity",
            "3",
            "-ext",
            "SAN=dns:leaf.test,ip:127.0.0.1",
            "-ext",
            "EKU=serverAuth,clientAuth",
        ],
    );
    keytool(
        jdk,
        dir,
        &[
            "-importcert",
            "-noprompt",
            "-alias",
            "ca",
            "-file",
            "akev-ca.pem",
            "-keystore",
            "akev-leaf.p12",
        ],
    );
    keytool(
        jdk,
        dir,
        &[
            "-importcert",
            "-noprompt",
            "-alias",
            "leaf",
            "-file",
            "leaf.pem",
            "-keystore",
            "akev-leaf.p12",
        ],
    );
    genkey(
        "other.p12",
        "other",
        "CN=other.test",
        &["-ext", "SAN=dns:other.test,ip:127.0.0.1"],
    );
    genkey(
        "untrusted.p12",
        "untrusted",
        "CN=other.test",
        &["-ext", "SAN=dns:other.test,ip:127.0.0.1"],
    );
}

fn java_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            java_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "java") {
            out.push(p);
        }
    }
}

/// Compile the agent sources plus `extra` source dirs into `<dir>/classes`.
fn compile(jdk: &Jdk, dir: &Path, extra: &[&Path]) -> PathBuf {
    let classes = dir.join("classes");
    // Java 8's javac doesn't create the output directory.
    std::fs::create_dir_all(&classes).unwrap();
    let mut files = Vec::new();
    java_files(&Path::new(JAVA_DIR).join("src"), &mut files);
    for d in extra {
        java_files(d, &mut files);
    }
    run(Command::new(&jdk.javac)
        .args(["-encoding", "UTF-8", "-nowarn", "-d"])
        .arg(&classes)
        .args(&files));
    classes
}

fn self_test(patched: bool) {
    let jdk = require_jdk!();
    let dir = tempfile::tempdir().unwrap();
    make_certs(&jdk, dir.path());
    let classes = compile(&jdk, dir.path(), &[&Path::new(JAVA_DIR).join("test")]);
    let mut cmd = Command::new(&jdk.java);
    if patched {
        // Read by Java 9+ launchers only (Java 8 has no modules and needs no --add-opens).
        cmd.env(
            "JDK_JAVA_OPTIONS",
            "--add-opens=java.base/javax.net.ssl=ALL-UNNAMED --add-opens=java.base/sun.security.ssl=ALL-UNNAMED",
        );
    } else {
        cmd.env_remove("JDK_JAVA_OPTIONS");
    }
    let out = cmd
        .arg("-cp")
        .arg(&classes)
        .arg("dev.akev.AgentSelfTest")
        .arg(dir.path())
        .arg(if patched { "patched" } else { "unpatched" })
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success() && stdout.contains("ALL OK"),
        "self-test failed:\n{stdout}\n{stderr}"
    );
    let passed: usize = stdout
        .lines()
        .find_map(|l| l.strip_prefix("ALL OK "))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(passed >= 60, "only {passed} checks ran");
}

#[test]
fn agent_self_test_without_jsse_access() {
    self_test(false);
}

#[test]
fn agent_self_test_with_jsse_access() {
    self_test(true);
}

const PROBE: &str = r#"
import java.net.ProxySelector;
import java.net.URI;
import javax.net.ssl.SSLContext;

public class Probe {
    public static void main(String[] args) throws Exception {
        do {
            System.out.println("SELECT " + ProxySelector.getDefault().select(URI.create("https://example.com/")).get(0)
                + " props=" + System.getProperty("https.proxyHost") + ":" + System.getProperty("https.proxyPort")
                + " ctx=" + SSLContext.getDefault().getProtocol());
            System.out.flush();
            Thread.sleep(100);
        } while (args.length > 0 && args[0].equals("loop"));
    }
}
"#;

fn probe_classes(jdk: &Jdk, dir: &Path) -> PathBuf {
    let src = dir.join("probe-src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("Probe.java"), PROBE).unwrap();
    let classes = dir.join("probe");
    std::fs::create_dir_all(&classes).unwrap();
    run(Command::new(&jdk.javac)
        .arg("-d")
        .arg(&classes)
        .arg(src.join("Probe.java")));
    classes
}

#[test]
fn javaagent_premain_uses_instrumentation_to_update_the_default_context() {
    let jdk = require_jdk!();
    let dir = tempfile::tempdir().unwrap();
    let ca = dir.path().join("ca.pem");
    std::fs::write(&ca, FIXTURE_CA).unwrap();
    let status = dir.path().join("status.txt");
    let classes = probe_classes(&jdk, dir.path());
    let enc = |p: &Path| {
        p.to_string_lossy()
            .replace('%', "%25")
            .replace('&', "%26")
            .replace(' ', "%20")
    };
    let agent_args = format!(
        "proxyHost=127.0.0.1&proxyPort=18080&caCert={}&statusFile={}",
        enc(&ca),
        enc(&status)
    );
    let out = run(Command::new(&jdk.java)
        .arg(format!("-javaagent:{AGENT_JAR}={agent_args}"))
        .arg("-cp")
        .arg(&classes)
        .arg("Probe"));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("SELECT HTTP @ ") && stdout.contains(":18080"),
        "{stdout}"
    );
    assert!(stdout.contains("props=127.0.0.1:18080"), "{stdout}");
    assert!(
        stderr.contains(
            "[Akev] HTTP(S) traffic of this JVM now goes through Akev at 127.0.0.1:18080"
        ),
        "{stderr}"
    );
    // Java 16+ denies illegal access, so a warning there would mean the explicit module opens
    // failed. Java 9-15 permit it by default and may still print the JVM's own one-time warning.
    if java_major(&jdk.java) >= 16 {
        assert!(
            !stderr.to_lowercase().contains("illegal reflective access"),
            "{stderr}"
        );
    }
    let report = std::fs::read_to_string(&status).unwrap();
    for line in [
        "result=ok",
        "entry=premain",
        "existingDefaultContext=patched",
        "proxyProperties=set",
        "proxySelector=installed",
    ] {
        assert!(
            report.lines().any(|l| l == line),
            "{line} missing:\n{report}"
        );
    }
}

/// Lines a child prints, collected in the background.
fn collect(child: &mut Child) -> Arc<Mutex<Vec<String>>> {
    let lines = Arc::new(Mutex::new(Vec::new()));
    let out = child.stdout.take().unwrap();
    let sink = lines.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            sink.lock().unwrap().push(line);
        }
    });
    lines
}

fn wait_for(lines: &Arc<Mutex<Vec<String>>>, what: &str, pred: impl Fn(&str) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if lines.lock().unwrap().last().is_some_and(|l| pred(l)) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}: {:?}",
            lines.lock().unwrap().last()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn attach_helper_attaches_and_restores_a_live_jvm() {
    let jdk = require_jdk!();
    let dir = tempfile::tempdir().unwrap();
    let ca = dir.path().join("ca.pem");
    std::fs::write(&ca, FIXTURE_CA).unwrap();
    let classes = probe_classes(&jdk, dir.path());
    let attach = |args: &[&str]| {
        Command::new(&jdk.java)
            .arg("-cp")
            .arg(AGENT_JAR)
            .arg("dev.akev.Attach")
            .args(args)
            .output()
            .unwrap()
    };

    // Usage errors and the Attach API check don't need a target.
    assert_eq!(attach(&[]).status.code(), Some(2));
    let check = attach(&["--check"]);
    let check_out = String::from_utf8_lossy(&check.stdout).to_string();
    if check.status.code() == Some(3) {
        eprintln!("skipping: this Java has no Attach API ({check_out})");
        return;
    }
    assert_eq!(check.status.code(), Some(0), "{check_out}");
    assert!(check_out.contains("attach=available"));

    let mut child = Command::new(&jdk.java)
        .arg("-cp")
        .arg(&classes)
        .arg("Probe")
        .arg("loop")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let lines = collect(&mut child);
    let child = KillOnDrop(child);
    let pid = child.0.id().to_string();
    wait_for(&lines, "the probe to start", |l| {
        l.starts_with("SELECT DIRECT props=null:null")
    });

    let ca_s = ca.to_string_lossy().to_string();
    let out = attach(&[&pid, "--proxy", "127.0.0.1:18080", "--ca", &ca_s, "--quiet"]);
    let report = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{report}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        report.lines().any(|l| l == "result=ok") && report.contains("entry=agentmain"),
        "{report}"
    );
    assert!(
        report
            .lines()
            .any(|l| l == "existingDefaultContext=patched"),
        "Instrumentation opens JSSE: {report}"
    );
    wait_for(&lines, "the proxy", |l| {
        l.contains(":18080 props=127.0.0.1:18080")
    });

    let out = attach(&[&pid, "--restore"]);
    let report = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{report}");
    assert!(report.lines().any(|l| l == "restored=true"), "{report}");
    wait_for(&lines, "direct again", |l| {
        l.starts_with("SELECT DIRECT props=null:null")
    });

    // A process that isn't a JVM (or is gone) can't be attached to.
    drop(child);
    let out = attach(&[&pid, "--restore"]);
    assert_eq!(
        out.status.code(),
        Some(4),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("stage=attach"));
}

/// A CA certificate (the agent only needs to load it).
const FIXTURE_CA: &str = "-----BEGIN CERTIFICATE-----
MIIBajCCARGgAwIBAgIJAOtXYAlrviU6MAoGCCqGSM49BAMDMCAxHjAcBgNVBAMT
FUFrZXZvIFRlc3QgRml4dHVyZSBDQTAgFw0yNjExMDkxODE1MTJaGA8yMTI2MTAx
NjE4MTUxMlowIDEeMBwGA1UEAxMVQWtldm8gVGVzdCBGaXh0dXJlIENBMFkwEwYH
KoZIzj0CAQYIKoZIzj0DAQcDQgAEitclRLYmDauB6eyLh5hBhb2Tfwq0jgOYWQJT
S8ipvdzvy12WZlWZzSExKkb7l8Pj+2/tcpQNmkF2f0FhzsJg/qMyMDAwHQYDVR0O
BBYEFPGuruAq4eXCdJFcRO4JQsJmx/PVMA8GA1UdEwEB/wQFMAMBAf8wCgYIKoZI
zj0EAwMDRwAwRAIgMcMP8XoE8qj459WwM8IolV5T/iCOzEjTOhBYD/gPcTwCIAlj
XnngYOMTLPoRGM+ymf+L8nJz6ekQrSuqdP7IM3KL
-----END CERTIFICATE-----
";
