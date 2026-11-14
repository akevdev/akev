//! Interceptor-level tests with a scripted runner and a fake filesystem root. The real agent
//! is exercised end to end by `crates/akev-intercept/tests/java_agent.rs` and
//! `crates/akev-api/tests/jvm_attach.rs`.

use super::*;
use crate::host::testutil::{fake_env, make_exe};
use crate::runner::mock::{MockRunner, line_of};
use crate::runner::{CommandOutput, ProcessEntry};
use crate::util::testctx::ctx;
use std::io;

const TARGET: u32 = 4242;

const APPLIED: &str = "action=apply\nproxy=127.0.0.1:8000\nproxySelector=installed\n\
existingDefaultContext=patched\nproxyProperties=set\nmessage=HTTP(S) traffic of this JVM now goes through Akev at 127.0.0.1:8000\n\
result=ok\nagentVersion=1.0.0\njavaVersion=21.0.5\n";
const RESTORED: &str = "action=restore\nrestored=true\nmessage=restored\nresult=ok\n";

struct Machine {
    _dir: tempfile::TempDir,
    root: PathBuf,
    runner: Arc<MockRunner>,
    sys: Arc<Sys>,
    jdk: PathBuf,
    ctx: InterceptContext,
}

fn machine() -> Machine {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let jdk = root.join("usr/lib/jvm/java-21");
    make_exe(&jdk.join("bin/java"), "");
    make_exe(&jdk.join("bin/jps"), "");
    std::fs::write(jdk.join("release"), "JAVA_VERSION=\"21.0.5\"\n").unwrap();
    std::fs::create_dir_all(root.join("usr/bin")).unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(jdk.join("bin/jps"), root.join("usr/bin/jps")).unwrap();
        std::fs::create_dir_all(root.join(format!("proc/{TARGET}"))).unwrap();
        std::os::unix::fs::symlink(
            jdk.join("bin/java"),
            root.join(format!("proc/{TARGET}/exe")),
        )
        .unwrap();
    }
    #[cfg(not(unix))]
    make_exe(&root.join("usr/bin/jps"), "");
    let runner = MockRunner::new();
    runner.processes.lock().extend([
        ProcessEntry {
            pid: std::process::id(),
            name: "akev-test".into(),
            ..Default::default()
        },
        ProcessEntry {
            pid: TARGET,
            name: "java".into(),
            cmd: vec![
                "java".into(),
                "-cp".into(),
                "app".into(),
                "org.acme.Main".into(),
            ],
            ..Default::default()
        },
    ]);
    runner.on_prefix(
        "jps -l",
        CommandOutput::ok(format!("{TARGET} org.acme.Main\n")),
    );
    let env = fake_env(Os::Linux, &root);
    let sys = Sys::new(runner.clone(), env);
    let ctx = ctx(&root.join("data"));
    Machine {
        _dir: dir,
        root,
        runner,
        sys,
        jdk,
        ctx,
    }
}

impl Machine {
    /// Answer attacher runs: `apply` for `--proxy`, `restore` for `--restore`.
    fn agent_answers(&self, apply: CommandOutput, restore: CommandOutput) {
        self.runner.on(move |spec| {
            let args = spec.args_lossy();
            if !args.iter().any(|a| a == "dev.akev.Attach") {
                return None;
            }
            Some(Ok(if args.iter().any(|a| a == "--restore") {
                restore.clone()
            } else {
                apply.clone()
            }))
        });
    }

    fn attach_calls(&self) -> Vec<String> {
        self.runner
            .call_lines()
            .into_iter()
            .filter(|l| l.contains("dev.akev.Attach"))
            .collect()
    }

    fn interceptor(&self) -> JvmInterceptor {
        JvmInterceptor::new(self.sys.clone())
    }

    async fn exit_target(&self) {
        self.runner.processes.lock().retain(|p| p.pid != TARGET);
        self.sys.invalidate_processes().await;
    }
}

fn ok(stdout: &str) -> CommandOutput {
    CommandOutput::ok(stdout)
}

fn processes(info: &InterceptorInfo) -> Vec<Value> {
    info.metadata["processes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

#[tokio::test]
async fn lists_running_jvms_with_details() {
    let m = machine();
    let i = m.interceptor();
    let info = i.info(&m.ctx).await;
    assert_eq!(info.id, "jvm");
    assert!(info.supported && info.available && info.needs_config && !info.active);
    assert_eq!(info.category, InterceptorCategory::Runtime);
    assert_eq!(info.icon, "java");
    assert_eq!(info.version.as_deref(), Some("21.0.5"));
    assert_eq!(info.metadata["discovery"], "jps", "{}", info.metadata);
    assert!(info.metadata["notes"].as_array().unwrap().len() >= 2);
    let procs = processes(&info);
    assert_eq!(procs.len(), 1);
    let p = &procs[0];
    assert_eq!(p["pid"], TARGET);
    assert_eq!(p["name"], "Main");
    assert_eq!(p["mainClass"], "org.acme.Main");
    assert_eq!(p["commandLine"], "java -cp app org.acme.Main");
    assert_eq!(p["attached"], false);
    assert_eq!(p["active"], false);
    #[cfg(unix)]
    assert_eq!(p["javaVersion"], "21.0.5");
    // Listing doesn't attach to anything.
    assert!(m.attach_calls().is_empty());
}

#[tokio::test]
async fn unavailable_without_java() {
    let dir = tempfile::tempdir().unwrap();
    let runner = MockRunner::new();
    let sys = Sys::new(runner.clone(), fake_env(Os::Linux, dir.path()));
    let i = JvmInterceptor::new(sys);
    let c = ctx(&dir.path().join("data"));
    let info = i.info(&c).await;
    assert!(info.supported && !info.available);
    assert!(info.unavailable_reason.unwrap().contains("install a JDK"));
    assert_eq!(info.metadata["discovery"], "process-table");
    let err = i.activate(&c, json!({"pid": 1234})).await.unwrap_err();
    assert!(matches!(err, InterceptError::Unavailable(_)), "{err}");

    let other = JvmInterceptor::new(Sys::new(runner, fake_env(Os::Other, dir.path())));
    assert!(!other.info(&c).await.supported);
}

#[tokio::test]
async fn attach_and_restore_round_trip() {
    let m = machine();
    m.agent_answers(
        ok(&format!(
            "{APPLIED}warning=OkHttp clients created before attaching keep connecting directly.\n"
        )),
        ok(RESTORED),
    );
    let i = m.interceptor();
    let res = i
        .activate(&m.ctx, json!({"pid": TARGET, "interceptLoopback": true}))
        .await
        .unwrap();
    assert!(res.success);
    let msg = res.message.unwrap();
    assert!(msg.starts_with("Attached to Main (PID 4242)"), "{msg}");
    assert!(
        msg.contains("OkHttp clients created before attaching"),
        "first warning surfaced: {msg}"
    );
    assert_eq!(res.metadata["agent"]["existingDefaultContext"], "patched");
    assert_eq!(
        res.metadata["agent"]["warnings"].as_array().unwrap().len(),
        1
    );

    let calls = m.attach_calls();
    assert_eq!(calls.len(), 1);
    let jar = ensure_agent_jar(&m.ctx.data_dir).unwrap();
    assert_eq!(
        calls[0],
        format!(
            "java -cp {} dev.akev.Attach {TARGET} --proxy 127.0.0.1:8000 --ca {} --intercept-loopback",
            jar.display(),
            m.ctx.ca_path.display()
        )
    );
    #[cfg(unix)]
    {
        // The target's own Java runs the attacher.
        let spec = m
            .runner
            .calls
            .lock()
            .iter()
            .find(|s| line_of(s).contains("dev.akev.Attach"))
            .cloned()
            .unwrap();
        assert_eq!(
            PathBuf::from(&spec.program),
            std::fs::canonicalize(&m.jdk).unwrap().join("bin/java")
        );
    }

    // Journaled and shown as active.
    let journal: Value =
        serde_json::from_slice(&std::fs::read(journal_path(&m.ctx.data_dir)).unwrap()).unwrap();
    assert_eq!(journal["attached"][0]["pid"], TARGET);
    assert_eq!(journal["attached"][0]["proxyPort"], 8000);
    assert_eq!(journal["attached"][0]["name"], "org.acme.Main");
    let info = i.info(&m.ctx).await;
    assert!(info.active);
    let p = &processes(&info)[0];
    assert_eq!(p["attached"], true);
    assert_eq!(p["active"], true);
    assert_eq!(p["attachedPort"], 8000);
    assert_eq!(p["interceptLoopback"], true);

    i.deactivate(&m.ctx, json!({"pid": TARGET})).await.unwrap();
    let calls = m.attach_calls();
    assert_eq!(calls.len(), 2);
    assert!(
        calls[1].ends_with(&format!("dev.akev.Attach {TARGET} --restore")),
        "{}",
        calls[1]
    );
    assert!(
        !journal_path(&m.ctx.data_dir).exists(),
        "empty journal removed"
    );
    let info = i.info(&m.ctx).await;
    assert!(!info.active);
    assert_eq!(processes(&info)[0]["attached"], false);
}

#[tokio::test]
async fn options_are_validated() {
    let m = machine();
    m.agent_answers(ok(APPLIED), ok(RESTORED));
    let i = m.interceptor();
    for bad in [
        json!({}),
        json!({"pid": "abc"}),
        json!({"pid": 0}),
        json!({"pid": -5}),
        json!({"pid": true}),
    ] {
        let err = i.activate(&m.ctx, bad.clone()).await.unwrap_err();
        assert!(matches!(err, InterceptError::Failed(_)), "{bad}: {err}");
    }
    let err = i
        .activate(
            &m.ctx,
            json!({"pid": TARGET, "proxyProperties": "sometimes"}),
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("proxyProperties"));
    let err = i
        .activate(&m.ctx, json!({"pid": std::process::id()}))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("Akev itself"));
    // Not a JVM we can see: never attached blindly.
    let err = i
        .activate(&m.ctx, json!({"pid": 999_999}))
        .await
        .unwrap_err();
    assert!(matches!(err, InterceptError::Unavailable(_)), "{err}");
    assert!(m.attach_calls().is_empty());

    // String pids and the proxyProperties override are accepted.
    i.activate(
        &m.ctx,
        json!({"pid": TARGET.to_string(), "proxyProperties": "never"}),
    )
    .await
    .unwrap();
    assert!(m.attach_calls()[0].ends_with("--proxy-properties never"));
}

#[tokio::test]
async fn next_java_is_tried_when_one_lacks_the_attach_api() {
    let m = machine();
    let other = m.root.join("opt/jdk-17");
    make_exe(&other.join("bin/java"), "");
    let env = fake_env(Os::Linux, &m.root).with_var("JAVA_HOME", other.to_string_lossy());
    let sys = Sys::new(m.runner.clone(), env);
    let target_java = std::fs::canonicalize(&m.jdk).unwrap().join("bin/java");
    let first = target_java.clone();
    m.runner.on(move |spec| {
        if !spec.args_lossy().iter().any(|a| a == "dev.akev.Attach") {
            return None;
        }
        Some(Ok(if Path::new(&spec.program) == first {
            CommandOutput {
                code: Some(3),
                stdout: "result=error\nstage=attach-api\nerror=java.lang.ClassNotFoundException: com.sun.tools.attach.VirtualMachine\n".into(),
                stderr: String::new(),
            }
        } else {
            CommandOutput::ok(APPLIED)
        }))
    });
    let i = JvmInterceptor::new(sys);
    let res = i.activate(&m.ctx, json!({"pid": TARGET})).await.unwrap();
    assert_eq!(res.metadata["java"], json!(other.join("bin/java")));
    let programs: Vec<PathBuf> = m
        .runner
        .calls
        .lock()
        .iter()
        .filter(|s| line_of(s).contains("dev.akev.Attach"))
        .map(|s| PathBuf::from(&s.program))
        .collect();
    #[cfg(unix)]
    assert_eq!(programs, vec![target_java, other.join("bin/java")]);
    #[cfg(not(unix))]
    assert_eq!(programs.last(), Some(&other.join("bin/java")));
}

#[tokio::test]
async fn attach_failures_are_reported_and_not_journaled() {
    let m = machine();
    m.agent_answers(
        CommandOutput {
            code: Some(4),
            stdout: "result=error\nstage=attach\nerror=java.io.IOException: Unable to open socket file\n".into(),
            stderr: String::new(),
        },
        ok(RESTORED),
    );
    let i = m.interceptor();
    let err = i
        .activate(&m.ctx, json!({"pid": TARGET}))
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains(
            "Couldn't attach to Main (PID 4242): java.io.IOException: Unable to open socket file"
        ),
        "{msg}"
    );
    assert_eq!(
        m.attach_calls().len(),
        1,
        "a target-side failure isn't retried with other Javas"
    );
    assert!(!journal_path(&m.ctx.data_dir).exists());
    assert!(!i.info(&m.ctx).await.active);
}

#[tokio::test]
async fn timeouts_are_explained() {
    let m = machine();
    m.runner.on(|spec| {
        spec.args_lossy()
            .iter()
            .any(|a| a == "dev.akev.Attach")
            .then(|| Err(io::Error::new(io::ErrorKind::TimedOut, "`java` timed out")))
    });
    let err = m
        .interceptor()
        .activate(&m.ctx, json!({"pid": TARGET}))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("timed out after 45 s"), "{err}");
}

#[tokio::test]
async fn journal_survives_restarts_and_tracks_the_proxy_port() {
    let m = machine();
    m.agent_answers(ok(APPLIED), ok(RESTORED));
    m.interceptor()
        .activate(&m.ctx, json!({"pid": TARGET}))
        .await
        .unwrap();

    // A new Akev (same data dir) on another port: attached, but not to this instance.
    let mut other_port = m.ctx.clone();
    other_port.proxy_port = 9000;
    let fresh = m.interceptor();
    let info = fresh.info(&other_port).await;
    assert!(!info.active);
    let p = &processes(&info)[0];
    assert_eq!(p["attached"], true);
    assert_eq!(p["active"], false);
    assert_eq!(p["attachedPort"], 8000);
    // Same port again: active.
    assert!(fresh.info(&m.ctx).await.active);

    // Shutdown restores every live attached JVM.
    fresh.deactivate_all(&m.ctx).await.unwrap();
    assert!(m.attach_calls().last().unwrap().ends_with("--restore"));
    assert!(!journal_path(&m.ctx.data_dir).exists());
}

#[tokio::test]
async fn exited_jvms_are_forgotten_without_attaching() {
    let m = machine();
    m.agent_answers(ok(APPLIED), ok(RESTORED));
    let i = m.interceptor();
    i.activate(&m.ctx, json!({"pid": TARGET})).await.unwrap();
    m.exit_target().await;
    m.runner.on_prefix("jps -l", CommandOutput::ok(""));
    i.scan.invalidate().await;
    // Restoring a JVM that's gone succeeds without running anything.
    i.deactivate(&m.ctx, json!({"pid": TARGET})).await.unwrap();
    assert_eq!(m.attach_calls().len(), 1, "only the attach");
    assert!(!journal_path(&m.ctx.data_dir).exists());
    // And the listing drops it.
    let info = i.info(&m.ctx).await;
    assert!(processes(&info).is_empty() && !info.active);
    // Restoring an unknown pid is an error (nothing to restore it with).
    assert!(matches!(
        i.deactivate(&m.ctx, json!({"pid": TARGET})).await,
        Err(InterceptError::Unavailable(_))
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn pid_reuse_is_detected_from_the_start_time() {
    let m = machine();
    m.agent_answers(ok(APPLIED), ok(RESTORED));
    let stat = |start: u64| {
        format!(
            "{TARGET} (java (x)) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 {start} 20 21\n"
        )
    };
    std::fs::write(m.root.join(format!("proc/{TARGET}/stat")), stat(1111)).unwrap();
    let i = m.interceptor();
    assert_eq!(start_marker(&m.sys, TARGET), Some(1111));
    i.activate(&m.ctx, json!({"pid": TARGET})).await.unwrap();
    assert!(i.info(&m.ctx).await.active);
    // Same pid, different process.
    std::fs::write(m.root.join(format!("proc/{TARGET}/stat")), stat(2222)).unwrap();
    let info = i.info(&m.ctx).await;
    assert!(!info.active);
    assert_eq!(processes(&info)[0]["attached"], false);
    assert!(!journal_path(&m.ctx.data_dir).exists());
}

#[tokio::test]
async fn restore_errors_keep_the_journal_entry() {
    let m = machine();
    m.agent_answers(
        ok(APPLIED),
        CommandOutput {
            code: Some(6),
            stdout: "result=error\nerror=boom\n".into(),
            stderr: String::new(),
        },
    );
    let i = m.interceptor();
    i.activate(&m.ctx, json!({"pid": TARGET})).await.unwrap();
    let err = i.deactivate(&m.ctx, json!({})).await.unwrap_err();
    assert!(err.to_string().contains("boom"), "{err}");
    assert!(journal_path(&m.ctx.data_dir).exists(), "still attached");
    assert!(i.info(&m.ctx).await.active);
    // Shutdown never fails, even when a restore does.
    i.deactivate_all(&m.ctx).await.unwrap();
}

#[test]
fn pid_options() {
    assert_eq!(parse_pid(&json!({"pid": 12})), Some(Ok(12)));
    assert_eq!(parse_pid(&json!({"targetPid": " 34 "})), Some(Ok(34)));
    assert!(matches!(
        parse_pid(&json!({"pid": 5_000_000_000u64})),
        Some(Err(_))
    ));
    assert_eq!(parse_pid(&json!({})), None);
    assert_eq!(parse_pid(&Value::Null), None);
}

/// The interceptor against a real JVM on this machine (skipped without `java`, `javac` and
/// `jps`): list → attach → restore, observed from inside the target.
mod real {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    const PROBE: &str = r#"
public class AkevProbe {
    public static void main(String[] args) throws Exception {
        while (true) {
            System.out.println(java.net.ProxySelector.getDefault().select(java.net.URI.create("https://example.com/")).get(0));
            System.out.flush();
            Thread.sleep(100);
        }
    }
}
"#;

    /// Any CA certificate: the agent only loads it.
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

    struct Probe {
        child: std::process::Child,
        lines: Arc<parking_lot::Mutex<Vec<String>>>,
    }

    impl Probe {
        fn last(&self) -> String {
            self.lines.lock().last().cloned().unwrap_or_default()
        }

        async fn wait(&self, what: &str, pred: impl Fn(&str) -> bool) {
            let deadline = Instant::now() + Duration::from_secs(20);
            while !pred(&self.last()) {
                assert!(
                    Instant::now() < deadline,
                    "timed out waiting for {what}; last line {:?}",
                    self.last()
                );
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }

    impl Drop for Probe {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    #[tokio::test]
    async fn attaches_to_and_restores_a_real_jvm() {
        let sys = Sys::system();
        let (Some(java), Some(javac)) =
            (sys.env.find_in_path("java"), sys.env.find_in_path("javac"))
        else {
            eprintln!("skipping: no java/javac on PATH");
            return;
        };
        if discover::find_jps(&sys.env).is_none() {
            eprintln!("skipping: no jps");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("AkevProbe.java"), PROBE).unwrap();
        let out = Command::new(&javac)
            .arg("-d")
            .arg(dir.path())
            .arg(dir.path().join("AkevProbe.java"))
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let mut child = Command::new(&java)
            .arg("-cp")
            .arg(dir.path())
            .arg("AkevProbe")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let lines = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let sink = lines.clone();
        std::thread::spawn(move || {
            for l in BufReader::new(stdout)
                .lines()
                .map_while(std::result::Result::ok)
            {
                sink.lock().push(l);
            }
        });
        let probe = Probe { child, lines };
        let pid = probe.child.id();
        probe.wait("the probe", |l| l == "DIRECT").await;

        let mut c = ctx(&dir.path().join("data"));
        std::fs::write(&c.ca_path, FIXTURE_CA).unwrap();
        c.ca_pem = FIXTURE_CA.into();
        c.proxy_port = 18_081;
        let i = JvmInterceptor::new(sys.clone());

        let info = i.info(&c).await;
        assert!(info.available, "{info:?}");
        let listed = processes(&info);
        let me = listed
            .iter()
            .find(|p| p["pid"] == pid)
            .unwrap_or_else(|| panic!("probe {pid} not listed: {}", info.metadata));
        assert_eq!(me["name"], "AkevProbe");
        assert_eq!(me["attached"], false);

        let res = i.activate(&c, json!({ "pid": pid })).await.unwrap();
        assert!(res.success, "{res:?}");
        assert_eq!(res.metadata["agent"]["result"], "ok");
        probe.wait("the proxy", |l| l.contains(":18081")).await;
        let info = i.info(&c).await;
        assert!(info.active);
        assert_eq!(
            processes(&info).iter().find(|p| p["pid"] == pid).unwrap()["active"],
            true
        );

        i.deactivate(&c, json!({ "pid": pid })).await.unwrap();
        probe.wait("direct again", |l| l == "DIRECT").await;
        let info = i.info(&c).await;
        assert!(!info.active);
        assert!(!journal_path(&c.data_dir).exists());
        assert_eq!(
            processes(&info).iter().find(|p| p["pid"] == pid).unwrap()["attached"],
            false
        );
    }
}
