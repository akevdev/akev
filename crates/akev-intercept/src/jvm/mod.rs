//! `jvm`: attach to a running Java/Kotlin/Scala app — no restart — and route its HTTP(S)
//! clients through Akev.
//!
//! Akev ships a small Java agent (`crates/akev-intercept/java`, Java 8 compatible, embedded
//! in the binary and written to `<data_dir>/jvm/`). Activation runs
//! `java -cp akev-agent.jar dev.akev.Attach <pid> --proxy … --ca …` with the target's own
//! Java where possible; the helper loads the agent through the JDK Attach API and prints the
//! agent's report. Inside the target, the agent installs a default `ProxySelector`, a default
//! `SSLContext` trusting the Akev CA *in addition to* the existing roots (never "trust all"),
//! and more — see `java/src/dev/akev/Interception.java`. Deactivation attaches again with
//! `--restore`, which puts the saved settings back.
//!
//! Attached JVMs are journaled in `<data_dir>/jvm/attached.json` so a restarted Akev still
//! shows (and can restore) them; quitting Akev restores every live attached JVM.

mod agent;
mod discover;
#[cfg(test)]
mod source_hash;

use crate::envvars::ensure_ca_file;
use crate::host::Os;
use crate::util::{
    Descriptor, PROBE_TTL, ProbeCache, Sys, atomic_write, local_proxy_addr, opt_bool, opt_str,
};
use crate::{InterceptContext, InterceptError, Interceptor, Result};
use agent::{
    AttachAction, AttachReport, attach_spec, ensure_agent_jar, explain_failure, try_next_java,
};
use akev_core::interceptor::{ActivationResult, InterceptorCategory, InterceptorInfo};
use async_trait::async_trait;
use discover::{Discovery, JvmProcess, discover, java_candidates, release_version};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const TAGS: &[&str] = &[
    "jvm",
    "java",
    "kotlin",
    "scala",
    "clojure",
    "groovy",
    "spring",
    "quarkus",
    "micronaut",
    "gradle",
    "maven",
    "attach",
    "agent",
    "process",
    "running",
];

/// Coverage notes shown with the process list (kept in sync with the JVM guide, akev.dev/docs/jvm).
const NOTES: &[&str] = &[
    "Intercepted after attaching: HttpURLConnection (and what's built on it: Spring RestTemplate's default, Feign, Jersey…), java.net.http.HttpClient, and clients created from then on with the JVM's default proxy & trust settings (OkHttp, Apache HttpClient, Netty with default trust).",
    "Clients created before attaching with their own trust settings (e.g. an OkHttpClient built at startup) keep connecting directly; Akev then leaves the JVM-wide proxy properties alone so they don't break.",
    "Requests to localhost stay direct unless \"Include localhost\" is on. Restore puts the JVM's original settings back; quitting Akev restores every attached JVM.",
];

/// One JVM the agent was loaded into (journal entry).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AttachedJvm {
    pid: u32,
    /// Proxy port the JVM was pointed at.
    proxy_port: u16,
    /// Linux: process start time (clock ticks since boot) — detects pid reuse.
    start_marker: Option<u64>,
    /// Unix time (ms).
    attached_at: u64,
    name: String,
    intercept_loopback: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct JournalFile {
    version: u32,
    attached: Vec<AttachedJvm>,
}

fn journal_path(data_dir: &Path) -> PathBuf {
    data_dir.join("jvm").join("attached.json")
}

/// Linux: the process start time from `/proc/<pid>/stat` (field 22).
fn start_marker(sys: &Sys, pid: u32) -> Option<u64> {
    if sys.env.os != Os::Linux {
        return None;
    }
    let stat = std::fs::read_to_string(sys.env.sys_path(&format!("/proc/{pid}/stat"))).ok()?;
    // `pid (comm) state ppid ...`: comm may contain spaces/parentheses, so split after the
    // last `)`; the start time is the 20th field after it.
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(19)?.parse().ok()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// `jvm` interceptor.
pub(crate) struct JvmInterceptor {
    sys: Arc<Sys>,
    scan: ProbeCache<Arc<Discovery>>,
    /// Journal (loaded lazily per data dir).
    journal: Mutex<Option<(PathBuf, Vec<AttachedJvm>)>>,
    /// Attach/restore operations run one at a time.
    ops: tokio::sync::Mutex<()>,
}

impl JvmInterceptor {
    const DESCRIPTOR: Descriptor = Descriptor {
        id: "jvm",
        name: "Running JVM",
        description: "Attach to a running Java, Kotlin or Scala app without restarting it: its HTTP(S) clients go through Akev and trust the Akev CA. Restore puts its original settings back.",
        category: InterceptorCategory::Runtime,
        tags: TAGS,
        icon: "java",
        needs_config: true,
    };

    pub(crate) fn new(sys: Arc<Sys>) -> Self {
        JvmInterceptor {
            sys,
            scan: ProbeCache::new(PROBE_TTL),
            journal: Mutex::new(None),
            ops: tokio::sync::Mutex::new(()),
        }
    }

    async fn discovery(&self) -> Arc<Discovery> {
        self.scan
            .get(|| async { Arc::new(discover(&self.sys).await) })
            .await
    }

    async fn fresh_discovery(&self) -> Arc<Discovery> {
        self.scan.invalidate().await;
        self.discovery().await
    }

    /// Run `f` on the journal for `data_dir` (loaded on first use); saves it if `f` says so.
    fn with_journal<T>(
        &self,
        data_dir: &Path,
        f: impl FnOnce(&mut Vec<AttachedJvm>) -> (T, bool),
    ) -> T {
        let mut slot = self.journal.lock();
        let (_, entries) = match slot.take() {
            Some((dir, entries)) if dir == data_dir => slot.insert((dir, entries)),
            _ => {
                let entries = std::fs::read(journal_path(data_dir))
                    .ok()
                    .and_then(|b| serde_json::from_slice::<JournalFile>(&b).ok())
                    .map(|j| j.attached)
                    .unwrap_or_default();
                slot.insert((data_dir.to_path_buf(), entries))
            }
        };
        let (out, changed) = f(entries);
        if changed {
            let file = JournalFile {
                version: 1,
                attached: entries.clone(),
            };
            let path = journal_path(data_dir);
            let result = if file.attached.is_empty() {
                match std::fs::remove_file(&path) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                    _ => Ok(()),
                }
            } else {
                serde_json::to_vec_pretty(&file)
                    .map_err(std::io::Error::other)
                    .and_then(|bytes| atomic_write(&path, &bytes))
            };
            if let Err(e) = result {
                tracing::warn!(path = %path.display(), "couldn't save the JVM attach journal: {e}");
            }
        }
        out
    }

    /// Is the journaled JVM still the same running process?
    async fn alive(&self, entry: &AttachedJvm) -> bool {
        let procs = self.sys.processes(false).await;
        if !procs.iter().any(|p| p.pid == entry.pid) {
            return false;
        }
        match (entry.start_marker, start_marker(&self.sys, entry.pid)) {
            (Some(a), Some(b)) => a == b,
            _ => true,
        }
    }

    /// Journal entries whose JVM still runs (dead ones are dropped).
    async fn live_attachments(&self, data_dir: &Path) -> Vec<AttachedJvm> {
        let entries = self.with_journal(data_dir, |e| (e.clone(), false));
        let mut live = Vec::new();
        for e in entries {
            if self.alive(&e).await {
                live.push(e);
            }
        }
        let keep: Vec<u32> = live.iter().map(|e| e.pid).collect();
        self.with_journal(data_dir, |e| {
            let before = e.len();
            e.retain(|x| keep.contains(&x.pid));
            ((), e.len() != before)
        });
        live
    }

    fn record(&self, data_dir: &Path, entry: AttachedJvm) {
        self.with_journal(data_dir, |e| {
            e.retain(|x| x.pid != entry.pid);
            e.push(entry);
            ((), true)
        });
    }

    fn forget(&self, data_dir: &Path, pid: u32) {
        self.with_journal(data_dir, |e| {
            let before = e.len();
            e.retain(|x| x.pid != pid);
            ((), e.len() != before)
        });
    }

    /// Run the attacher with the best Java available, falling back to the next candidate when
    /// one can't run it at all. Returns the report and the Java used.
    async fn run_attacher(
        &self,
        target: &JvmProcess,
        jar: &Path,
        action: &AttachAction,
    ) -> Result<(AttachReport, PathBuf)> {
        let label = format!("{} (PID {})", target.display_name(), target.pid);
        let candidates = java_candidates(&self.sys.env, target.java_home.as_deref());
        if candidates.is_empty() {
            return Err(InterceptError::Unavailable(
                "No Java found to run the attacher: install a JDK or set JAVA_HOME".into(),
            ));
        }
        let mut last = String::new();
        for java in candidates {
            let spec = attach_spec(&java, jar, target.pid, action);
            match self.sys.runner.output(&spec).await {
                Ok(out) => {
                    let report = AttachReport::parse(&out.stdout);
                    if out.success() && report.ok() {
                        return Ok((report, java));
                    }
                    if try_next_java(&out, &report) {
                        tracing::debug!(java = %java.display(), "can't attach with this Java: {}", out.error_summary());
                        last = explain_failure(&out, &report, &label);
                        continue;
                    }
                    return Err(InterceptError::Failed(explain_failure(
                        &out, &report, &label,
                    )));
                }
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {
                    return Err(InterceptError::Failed(format!(
                        "Attaching to {label} timed out after {} s: is the JVM suspended (debugger) or overloaded?",
                        agent::ATTACH_TIMEOUT.as_secs()
                    )));
                }
                Err(e) => {
                    tracing::debug!(java = %java.display(), "can't run Java: {e}");
                    last = format!("couldn't run {}: {e}", java.display());
                }
            }
        }
        Err(InterceptError::Failed(format!(
            "No Java on this machine could attach to {label}: {last}"
        )))
    }

    async fn restore_entry(
        &self,
        ctx: &InterceptContext,
        entry: &AttachedJvm,
    ) -> Result<Option<AttachReport>> {
        if !self.alive(entry).await {
            self.forget(&ctx.data_dir, entry.pid);
            return Ok(None);
        }
        let discovery = self.fresh_discovery().await;
        let target = discovery
            .processes
            .iter()
            .find(|p| p.pid == entry.pid)
            .cloned()
            .unwrap_or_else(|| JvmProcess {
                pid: entry.pid,
                main: entry.name.clone(),
                ..Default::default()
            });
        let jar = ensure_agent_jar(&ctx.data_dir)?;
        let result = self
            .run_attacher(&target, &jar, &AttachAction::Restore)
            .await;
        match result {
            Ok((report, _)) => {
                self.forget(&ctx.data_dir, entry.pid);
                self.scan.invalidate().await;
                Ok(Some(report))
            }
            Err(e) => {
                // It may have exited meanwhile.
                self.sys.invalidate_processes().await;
                if !self.alive(entry).await {
                    self.forget(&ctx.data_dir, entry.pid);
                    return Ok(None);
                }
                Err(e)
            }
        }
    }
}

fn parse_pid(options: &Value) -> Option<std::result::Result<u32, String>> {
    let v = options.get("pid").or_else(|| options.get("targetPid"))?;
    let parsed = match v {
        Value::Number(n) => n.as_u64().and_then(|n| u32::try_from(n).ok()),
        Value::String(s) => s.trim().parse::<u32>().ok(),
        _ => None,
    };
    Some(
        parsed
            .filter(|p| *p > 0)
            .ok_or_else(|| format!("invalid pid {v}")),
    )
}

#[async_trait]
impl Interceptor for JvmInterceptor {
    fn id(&self) -> &str {
        Self::DESCRIPTOR.id
    }

    async fn info(&self, ctx: &InterceptContext) -> InterceptorInfo {
        let d = Self::DESCRIPTOR;
        if self.sys.env.os == Os::Other {
            return d.unsupported("this operating system");
        }
        let discovery = self.discovery().await;
        let attached = self.live_attachments(&ctx.data_dir).await;
        let java = java_candidates(&self.sys.env, None).into_iter().next();
        let version = java
            .as_deref()
            .and_then(discover::home_of_java_exe)
            .and_then(|h| release_version(&h));

        let mut processes: Vec<Value> = discovery
            .processes
            .iter()
            .map(|p| {
                let a = attached.iter().find(|a| a.pid == p.pid);
                process_json(p, a, ctx.proxy_port)
            })
            .collect();
        // Attached JVMs the scan missed (e.g. jps hiccup) stay listed so they can be restored.
        for a in attached
            .iter()
            .filter(|a| !discovery.processes.iter().any(|p| p.pid == a.pid))
        {
            let p = JvmProcess {
                pid: a.pid,
                main: a.name.clone(),
                ..Default::default()
            };
            processes.push(process_json(&p, Some(a), ctx.proxy_port));
        }
        let active = attached.iter().any(|a| a.proxy_port == ctx.proxy_port);
        let agent = agent::agent_available();
        InterceptorInfo {
            available: java.is_some() && agent,
            active,
            unavailable_reason: if !agent {
                Some(
                    "This build of Akev has no JVM agent (it was built without a JDK). The downloads from akev.dev include it."
                        .to_string(),
                )
            } else if java.is_none() {
                Some(
                    "No Java found: install a JDK (or set JAVA_HOME) to attach to running JVMs"
                        .to_string(),
                )
            } else {
                None
            },
            version,
            metadata: json!({
                "processes": processes,
                "discovery": discovery.method.as_str(),
                "jps": discovery.jps,
                "jpsError": discovery.jps_error,
                "java": java,
                "notes": NOTES,
                "options": {
                    "pid": "process id of the JVM (from processes[].pid)",
                    "interceptLoopback": "also intercept requests to localhost (default false)",
                    "proxyProperties": "auto (default) | always | never: set the http(s).proxyHost/Port system properties",
                },
            }),
            ..d.info()
        }
    }

    async fn activate(&self, ctx: &InterceptContext, options: Value) -> Result<ActivationResult> {
        if self.sys.env.os == Os::Other {
            return Err(InterceptError::Unavailable(
                "Attaching to JVMs is not supported on this operating system".into(),
            ));
        }
        if !agent::agent_available() {
            return Err(InterceptError::Unavailable(
                "This build of Akev has no JVM agent (it was built without a JDK)".into(),
            ));
        }
        let pid = match parse_pid(&options) {
            Some(Ok(pid)) => pid,
            Some(Err(e)) => return Err(InterceptError::Failed(e)),
            None => {
                return Err(InterceptError::Failed(
                    "Choose a JVM: options.pid is required (see metadata.processes)".into(),
                ));
            }
        };
        let intercept_loopback = opt_bool(&options, "interceptLoopback").unwrap_or(false);
        let proxy_properties = match opt_str(&options, "proxyProperties") {
            None => None,
            Some(m @ ("auto" | "always" | "never")) => Some(m.to_string()),
            Some(other) => {
                return Err(InterceptError::Failed(format!(
                    "invalid proxyProperties {other:?} (expected auto, always or never)"
                )));
            }
        };
        if pid == std::process::id() {
            return Err(InterceptError::Failed("That's Akev itself".into()));
        }

        let _op = self.ops.lock().await;
        let discovery = self.fresh_discovery().await;
        let target = discovery
            .processes
            .iter()
            .find(|p| p.pid == pid)
            .cloned()
            .ok_or_else(|| {
                InterceptError::Unavailable(format!(
                    "No JVM with PID {pid} to attach to: it may have exited, run as another user or in a container"
                ))
            })?;
        ensure_ca_file(ctx);
        let jar = ensure_agent_jar(&ctx.data_dir)?;
        let action = AttachAction::Apply {
            proxy: local_proxy_addr(ctx),
            ca: ctx.ca_path.clone(),
            intercept_loopback,
            proxy_properties,
        };
        let (report, java) = self.run_attacher(&target, &jar, &action).await?;
        let name = target.display_name();
        self.record(
            &ctx.data_dir,
            AttachedJvm {
                pid,
                proxy_port: ctx.proxy_port,
                start_marker: start_marker(&self.sys, pid),
                attached_at: now_ms(),
                name: if target.main.is_empty() {
                    name.clone()
                } else {
                    target.main.clone()
                },
                intercept_loopback,
            },
        );
        self.scan.invalidate().await;
        let warnings = report.all("warning");
        let mut message =
            format!("Attached to {name} (PID {pid}): its HTTP(S) traffic now goes through Akev.");
        if let Some(first) = warnings.first() {
            message.push(' ');
            message.push_str(first);
        }
        Ok(ActivationResult {
            success: true,
            message: Some(message),
            metadata: json!({
                "pid": pid,
                "name": name,
                "java": java,
                "agent": report.to_json(),
            }),
        })
    }

    async fn deactivate(&self, ctx: &InterceptContext, options: Value) -> Result<()> {
        let pid = match parse_pid(&options) {
            Some(Ok(pid)) => Some(pid),
            Some(Err(e)) => return Err(InterceptError::Failed(e)),
            None => None,
        };
        let _op = self.ops.lock().await;
        let journal = self.with_journal(&ctx.data_dir, |e| (e.clone(), false));
        let targets: Vec<AttachedJvm> = match pid {
            Some(pid) => match journal.iter().find(|e| e.pid == pid) {
                Some(e) => vec![e.clone()],
                None => {
                    // Not attached by this data dir (e.g. its journal was lost): restore it
                    // anyway if it's a JVM we can see — the agent knows whether it's active.
                    let discovery = self.fresh_discovery().await;
                    let p = discovery
                        .processes
                        .iter()
                        .find(|p| p.pid == pid)
                        .ok_or_else(|| {
                            InterceptError::Unavailable(format!("No JVM with PID {pid} is running"))
                        })?;
                    vec![AttachedJvm {
                        pid,
                        proxy_port: ctx.proxy_port,
                        start_marker: None,
                        attached_at: 0,
                        name: p.main.clone(),
                        intercept_loopback: false,
                    }]
                }
            },
            None => journal,
        };
        let mut errors = Vec::new();
        for t in &targets {
            if let Err(e) = self.restore_entry(ctx, t).await {
                errors.push(match e {
                    InterceptError::Failed(m) | InterceptError::Unavailable(m) => m,
                    other => other.to_string(),
                });
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(InterceptError::Failed(errors.join("; ")))
        }
    }

    async fn deactivate_all(&self, ctx: &InterceptContext) -> Result<()> {
        let _op = self.ops.lock().await;
        let journal = self.with_journal(&ctx.data_dir, |e| (e.clone(), false));
        let results =
            futures::future::join_all(journal.iter().map(|t| self.restore_entry(ctx, t))).await;
        for (t, r) in journal.iter().zip(results) {
            if let Err(e) = r {
                tracing::warn!(pid = t.pid, "couldn't restore JVM on shutdown: {e}");
            }
        }
        Ok(())
    }
}

fn process_json(p: &JvmProcess, attached: Option<&AttachedJvm>, port: u16) -> Value {
    json!({
        "pid": p.pid,
        "name": p.display_name(),
        "mainClass": p.main,
        "commandLine": p.command_line,
        "javaVersion": p.java_version,
        "javaHome": p.java_home,
        "attached": attached.is_some(),
        // Attached to *this* Akev (same proxy port).
        "active": attached.is_some_and(|a| a.proxy_port == port),
        "attachedPort": attached.map(|a| a.proxy_port),
        "attachedAt": attached.map(|a| a.attached_at),
        "interceptLoopback": attached.map(|a| a.intercept_loopback),
    })
}

#[cfg(test)]
mod tests;
