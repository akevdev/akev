//! Docker & Podman containers.
//!
//! What this does — honestly: a running container's main process can't be re-pointed at a
//! proxy without recreating the container (its environment is fixed at start), and
//! recreating someone's container is too risky to do automatically. So Akev:
//! * lists running containers from every reachable engine and flags those started with the
//!   Akev environment (`AKEV_ACTIVE=1` pointing at this proxy port);
//! * generates ready-to-use `docker run` / `podman run` snippets and env files that proxy
//!   *new* containers (proxy variables + the CA bundle mounted read-only);
//! * on activation with a container id, copies the CA files into the container's `/tmp`
//!   (non-destructive) and returns a `docker exec` command that opens an intercepted shell
//!   inside it.

use crate::envvars::{EnvConfig, build_env_vars, ensure_ca_bundle, ensure_ca_file};
use crate::host::Os;
use crate::runner::CommandSpec;
use crate::util::{Descriptor, PROBE_TTL, ProbeCache, Sys, opt_str, tags, write_if_changed};
use crate::{InterceptContext, InterceptError, Interceptor, Result};
use akev_core::interceptor::{ActivationResult, InterceptorCategory, InterceptorInfo};
use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const LIST_TIMEOUT: Duration = Duration::from_secs(5);
const CP_TIMEOUT: Duration = Duration::from_secs(20);
/// Where CA files are mounted in containers started with the snippet.
const MOUNT_CA: &str = "/akev/ca.pem";
const MOUNT_BUNDLE: &str = "/akev/ca-bundle.pem";
/// Where CA files are copied into running containers.
const COPIED_CA: &str = "/tmp/akev-ca.pem";
const COPIED_BUNDLE: &str = "/tmp/akev-ca-bundle.pem";

/// Container engine flavour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum EngineKind {
    Docker,
    Podman,
}

impl EngineKind {
    fn as_str(self) -> &'static str {
        match self {
            EngineKind::Docker => "docker",
            EngineKind::Podman => "podman",
        }
    }

    /// Hostname containers use to reach the host.
    fn host_alias(self) -> &'static str {
        match self {
            EngineKind::Docker => "host.docker.internal",
            EngineKind::Podman => "host.containers.internal",
        }
    }
}

/// A container engine CLI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Engine {
    pub kind: EngineKind,
    pub path: PathBuf,
}

/// A running container.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Container {
    pub id: String,
    pub names: Vec<String>,
    pub image: String,
    pub state: String,
    pub status: String,
}

impl Container {
    fn display_name(&self) -> String {
        self.names
            .first()
            .map(|n| n.trim_start_matches('/').to_string())
            .unwrap_or_else(|| short_id(&self.id))
    }
}

fn short_id(id: &str) -> String {
    id.chars().take(12).collect()
}

/// Is this `docker` actually the podman-docker shim?
fn is_podman_shim(path: &Path) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    let head = &bytes[..bytes.len().min(4096)];
    head.starts_with(b"#!") && memchr::memmem::find(head, b"podman").is_some()
}

/// Engine CLIs on this machine.
pub(crate) fn detect_engines(env: &crate::host::HostEnv) -> Vec<Engine> {
    let mut out = Vec::new();
    let mut docker = env.find_in_path("docker");
    if docker.is_none() {
        let extra: &[&str] = match env.os {
            Os::MacOs => &[
                "/usr/local/bin/docker",
                "/opt/homebrew/bin/docker",
                "/Applications/Docker.app/Contents/Resources/bin/docker",
            ],
            Os::Linux => &["/usr/bin/docker", "/usr/local/bin/docker"],
            _ => &[],
        };
        docker = extra.iter().map(|p| env.sys_path(p)).find(|p| p.is_file());
        if docker.is_none() && env.os == Os::Windows {
            docker = env
                .var_path("PROGRAMFILES")
                .map(|p| {
                    p.join("Docker")
                        .join("Docker")
                        .join("resources")
                        .join("bin")
                        .join("docker.exe")
                })
                .filter(|p| p.is_file());
        }
    }
    if let Some(path) = docker.filter(|p| !is_podman_shim(p)) {
        out.push(Engine {
            kind: EngineKind::Docker,
            path,
        });
    }
    let mut podman = env.find_in_path("podman");
    if podman.is_none() && env.os == Os::Windows {
        podman = env
            .var_path("PROGRAMFILES")
            .map(|p| p.join("RedHat").join("Podman").join("podman.exe"))
            .filter(|p| p.is_file());
    }
    if podman.is_none() && env.os == Os::MacOs {
        podman = [
            "/opt/homebrew/bin/podman",
            "/usr/local/bin/podman",
            "/opt/podman/bin/podman",
        ]
        .iter()
        .map(|p| env.sys_path(p))
        .find(|p| p.is_file());
    }
    if let Some(path) = podman {
        out.push(Engine {
            kind: EngineKind::Podman,
            path,
        });
    }
    out
}

fn str_or_list(v: Option<&Value>, split_commas: bool) -> Vec<String> {
    match v {
        Some(Value::String(s)) if split_commas => s
            .split(',')
            .map(str::trim)
            .filter(|x| !x.is_empty())
            .map(String::from)
            .collect(),
        Some(Value::String(s)) if !s.is_empty() => vec![s.clone()],
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|x| x.as_str())
            .map(String::from)
            .collect(),
        _ => Vec::new(),
    }
}

/// Parse `<engine> ps --format '{{json .}}'` (one JSON object per line; Docker and Podman use
/// different shapes), also accepting Podman's `--format json` array.
pub(crate) fn parse_ps(output: &str) -> Vec<Container> {
    let trimmed = output.trim();
    let objects: Vec<Value> = if trimmed.starts_with('[') {
        serde_json::from_str::<Vec<Value>>(trimmed).unwrap_or_default()
    } else {
        trimmed
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l.trim()).ok())
            .collect()
    };
    objects
        .iter()
        .filter_map(|o| {
            let id = o
                .get("ID")
                .or_else(|| o.get("Id"))
                .and_then(Value::as_str)?
                .to_string();
            if id.is_empty() {
                return None;
            }
            let get = |k: &str| {
                o.get(k)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            Some(Container {
                id,
                names: str_or_list(o.get("Names"), true),
                image: get("Image"),
                state: get("State"),
                status: get("Status"),
            })
        })
        .collect()
}

/// Parse `inspect --format '{{.Id}} {{json .Config.Env}}'` output into the ids of containers
/// started with the Akev environment for `port`.
pub(crate) fn parse_intercepted(output: &str, port: u16) -> HashSet<String> {
    let needle = format!(":{port}");
    output
        .lines()
        .filter_map(|line| {
            let (id, env_json) = line.trim().split_once(' ')?;
            let env: Vec<String> = serde_json::from_str(env_json.trim()).ok()?;
            let active = env.iter().any(|e| e == "AKEV_ACTIVE=1");
            let ours = env.iter().any(|e| {
                e.starts_with("HTTP_PROXY=") && e.trim_end_matches('/').ends_with(&needle)
            });
            (active && ours).then(|| id.to_string())
        })
        .collect()
}

/// `KEY=value` lines for `--env-file` (values are taken literally, no quoting).
pub(crate) fn env_file_content(
    proxy_host: &str,
    port: u16,
    credentials: Option<&(String, String)>,
) -> String {
    let vars = build_env_vars(&EnvConfig {
        proxy_host: proxy_host.to_string(),
        proxy_port: port,
        ca_file: MOUNT_CA.into(),
        bundle_file: MOUNT_BUNDLE.into(),
        java_truststore: None,
        proxy_credentials: credentials.cloned(),
    });
    let mut out = String::from("# Generated by Akev: proxy settings for containers\n");
    for (k, v) in vars {
        out.push_str(&format!("{k}={v}\n"));
    }
    out
}

fn quote_path(p: &Path, os: Os) -> String {
    let s = p.to_string_lossy();
    if os == Os::Windows {
        format!("\"{s}\"")
    } else if s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-+:@".contains(c))
    {
        s.into_owned()
    } else {
        crate::scripts::posix_quote(&s)
    }
}

/// Files and snippets for proxying new containers.
#[derive(Clone, Debug)]
pub(crate) struct Snippets {
    pub files: Vec<(PathBuf, String)>,
    pub json: Value,
}

/// Generate env files (paths under `<data_dir>/docker`) and run snippets.
pub(crate) fn snippets(ctx: &InterceptContext, os: Os, ca: &Path, bundle: &Path) -> Snippets {
    let dir = ctx.data_dir.join("docker");
    let port = ctx.proxy_port;
    let linux = os == Os::Linux;
    let mount_opts = if linux { "ro,z" } else { "ro" };
    let mounts = format!(
        "-v {}:{MOUNT_CA}:{mount_opts} -v {}:{MOUNT_BUNDLE}:{mount_opts}",
        quote_path(ca, os),
        quote_path(bundle, os)
    );
    let mut files = Vec::new();
    let mut run = serde_json::Map::new();
    for kind in [EngineKind::Docker, EngineKind::Podman] {
        let file = dir.join(format!("akev-{}.env", kind.as_str()));
        // Bridge-networked containers connect from another address: they need the credentials.
        files.push((
            file.clone(),
            env_file_content(kind.host_alias(), port, ctx.proxy_credentials.as_ref()),
        ));
        // Docker on Linux has no host alias by default; Docker Desktop and Podman do.
        let add_host = if kind == EngineKind::Docker && linux {
            " --add-host=host.docker.internal:host-gateway"
        } else {
            ""
        };
        run.insert(
            kind.as_str().into(),
            json!(format!(
                "{} run --rm -it{add_host} --env-file {} {mounts} IMAGE",
                kind.as_str(),
                quote_path(&file, os)
            )),
        );
    }
    let mut out = json!({
        "envFiles": files.iter().map(|(p, c)| json!({"path": p, "content": c})).collect::<Vec<_>>(),
        "run": run,
        "compose": format!(
            "services:\n  app:\n    env_file: {}\n    extra_hosts: [\"host.docker.internal:host-gateway\"]\n    volumes:\n      - {}:{MOUNT_CA}:ro\n      - {}:{MOUNT_BUNDLE}:ro\n",
            dir.join("akev-docker.env").display(),
            ca.display(),
            bundle.display()
        ),
    });
    if linux {
        let file = dir.join("akev-hostnet.env");
        // Host networking reaches the proxy over loopback: no credentials needed.
        files.push((file.clone(), env_file_content("127.0.0.1", port, None)));
        out["runHostNetwork"] = json!(format!(
            "docker run --rm -it --network host --env-file {} {mounts} IMAGE",
            quote_path(&file, os)
        ));
        out["notes"] = json!([
            "Bridge-networked containers reach the host through host.docker.internal / host.containers.internal: the Akev proxy must listen on all interfaces (Settings → Proxy → Allow other devices).",
            "--network host works with the default 127.0.0.1-only proxy.",
            "Already-running containers keep their environment: recreate them with these settings, or open an intercepted shell inside one."
        ]);
    } else {
        out["notes"] = json!([
            "Docker Desktop forwards host.docker.internal to the host, so the default proxy settings work.",
            "Already-running containers keep their environment: recreate them with these settings, or open an intercepted shell inside one."
        ]);
    }
    Snippets { files, json: out }
}

/// Containers seen on one engine.
#[derive(Clone, Debug)]
struct EngineScan {
    engine: Engine,
    containers: Vec<Container>,
    intercepted: HashSet<String>,
    error: Option<String>,
}

/// `docker`.
pub(crate) struct DockerInterceptor {
    sys: Arc<Sys>,
    scan: ProbeCache<Vec<EngineScan>>,
    /// (engine, container id) we copied CA files into.
    touched: Mutex<Vec<(Engine, String)>>,
}

impl DockerInterceptor {
    const DESCRIPTOR: Descriptor = Descriptor {
        id: "docker",
        name: "Docker & Podman Containers",
        description: "Proxy new containers with a ready-made run snippet (env file + CA mounts), and open intercepted shells in running ones. Running containers' own processes can't be re-routed without recreating them.",
        category: InterceptorCategory::Container,
        tags: tags::DOCKER,
        icon: "docker",
        needs_config: true,
    };

    pub(crate) fn new(sys: Arc<Sys>) -> Self {
        DockerInterceptor {
            sys,
            scan: ProbeCache::new(PROBE_TTL),
            touched: Mutex::new(Vec::new()),
        }
    }

    async fn run(
        &self,
        engine: &Engine,
        args: &[&str],
        timeout: Duration,
    ) -> std::result::Result<String, String> {
        let spec = CommandSpec::new(&engine.path)
            .args(args.iter().copied())
            .timeout(timeout);
        match self.sys.runner.output(&spec).await {
            Ok(out) if out.success() => Ok(out.stdout),
            Ok(out) => Err(out.error_summary()),
            Err(e) => Err(e.to_string()),
        }
    }

    async fn scan_engines(&self, port: u16) -> Vec<EngineScan> {
        self.scan
            .get(|| async move {
                let engines = detect_engines(&self.sys.env);
                let scans = engines.into_iter().map(|engine| async move {
                    match self
                        .run(
                            &engine,
                            &["ps", "--no-trunc", "--format", "{{json .}}"],
                            LIST_TIMEOUT,
                        )
                        .await
                    {
                        Ok(out) => {
                            let containers = parse_ps(&out);
                            let mut intercepted = HashSet::new();
                            if !containers.is_empty() {
                                let mut args =
                                    vec!["inspect", "--format", "{{.Id}} {{json .Config.Env}}"];
                                args.extend(containers.iter().map(|c| c.id.as_str()));
                                if let Ok(out) = self.run(&engine, &args, LIST_TIMEOUT).await {
                                    intercepted = parse_intercepted(&out, port);
                                }
                            }
                            EngineScan {
                                engine,
                                containers,
                                intercepted,
                                error: None,
                            }
                        }
                        Err(e) => EngineScan {
                            engine,
                            containers: Vec::new(),
                            intercepted: HashSet::new(),
                            error: Some(e),
                        },
                    }
                });
                futures::future::join_all(scans).await
            })
            .await
    }

    /// Ensure CA files and env files exist; return the snippets.
    fn prepare_snippets(&self, ctx: &InterceptContext) -> Snippets {
        ensure_ca_file(ctx);
        let bundle = ensure_ca_bundle(ctx, &self.sys.env).unwrap_or_else(|_| ctx.ca_path.clone());
        let s = snippets(ctx, self.sys.env.os, &ctx.ca_path, &bundle);
        for (path, content) in &s.files {
            if let Err(e) = write_if_changed(path, content.as_bytes()) {
                tracing::warn!(path = %path.display(), "couldn't write container env file: {e}");
            }
        }
        s
    }
}

/// Proxy host for an existing container, from `inspect` output `<NetworkMode>|<gw>,<gw>,`.
fn container_proxy_host(kind: EngineKind, inspect: &str, os: Os) -> String {
    let (mode, gateways) = inspect
        .trim()
        .split_once('|')
        .unwrap_or((inspect.trim(), ""));
    if mode == "host" {
        return "127.0.0.1".into();
    }
    if kind == EngineKind::Docker && os == Os::Linux {
        // No host alias inside plain Linux Docker containers: use the bridge gateway.
        if let Some(gw) = gateways.split(',').map(str::trim).find(|g| !g.is_empty()) {
            return gw.to_string();
        }
        return "172.17.0.1".into();
    }
    kind.host_alias().into()
}

#[async_trait]
impl Interceptor for DockerInterceptor {
    fn id(&self) -> &str {
        Self::DESCRIPTOR.id
    }

    async fn info(&self, ctx: &InterceptContext) -> InterceptorInfo {
        let d = Self::DESCRIPTOR;
        let scans = self.scan_engines(ctx.proxy_port).await;
        if scans.is_empty() {
            return InterceptorInfo {
                unavailable_reason: Some("Neither docker nor podman is installed".into()),
                ..d.info()
            };
        }
        let reachable: Vec<&EngineScan> = scans.iter().filter(|s| s.error.is_none()).collect();
        let snippets = self.prepare_snippets(ctx);
        let mut containers = Vec::new();
        for s in &scans {
            for c in &s.containers {
                containers.push(json!({
                    "id": c.id,
                    "shortId": short_id(&c.id),
                    "name": c.display_name(),
                    "names": c.names,
                    "image": c.image,
                    "state": c.state,
                    "status": c.status,
                    "engine": s.engine.kind.as_str(),
                    "intercepted": s.intercepted.contains(&c.id),
                }));
            }
        }
        let active = scans.iter().any(|s| !s.intercepted.is_empty());
        let engines: Vec<Value> = scans
            .iter()
            .map(|s| json!({ "kind": s.engine.kind.as_str(), "path": s.engine.path, "available": s.error.is_none(), "error": s.error }))
            .collect();
        let reason = if reachable.is_empty() {
            Some(format!(
                "Container engine not reachable: {}",
                scans
                    .iter()
                    .filter_map(|s| s.error.as_deref())
                    .next()
                    .unwrap_or("unknown error")
            ))
        } else {
            None
        };
        InterceptorInfo {
            available: !reachable.is_empty(),
            active,
            unavailable_reason: reason,
            metadata: json!({ "engines": engines, "containers": containers, "snippets": snippets.json }),
            ..d.info()
        }
    }

    async fn activate(
        &self,
        ctx: &InterceptContext,
        options: serde_json::Value,
    ) -> Result<ActivationResult> {
        let snippets = self.prepare_snippets(ctx);
        let Some(id) = opt_str(&options, "containerId") else {
            return Ok(ActivationResult {
                success: true,
                message: Some(
                    "Start new containers with the snippet below to proxy them through Akev".into(),
                ),
                metadata: json!({ "snippets": snippets.json }),
            });
        };
        self.scan.invalidate().await;
        let scans = self.scan_engines(ctx.proxy_port).await;
        let want_engine = opt_str(&options, "engine");
        let (engine, container) = scans
            .iter()
            .filter(|s| want_engine.is_none_or(|e| e == s.engine.kind.as_str()))
            .find_map(|s| {
                s.containers
                    .iter()
                    .find(|c| {
                        c.id == id
                            || c.id.starts_with(id)
                            || c.names.iter().any(|n| n.trim_start_matches('/') == id)
                    })
                    .map(|c| (s.engine.clone(), c.clone()))
            })
            .ok_or_else(|| InterceptError::Unavailable(format!("container {id} isn't running")))?;

        let inspect = self
            .run(&engine, &["inspect", "--format", "{{.HostConfig.NetworkMode}}|{{range .NetworkSettings.Networks}}{{.Gateway}},{{end}}", &container.id], LIST_TIMEOUT)
            .await
            .unwrap_or_default();
        let host = container_proxy_host(engine.kind, &inspect, self.sys.env.os);

        let bundle = ensure_ca_bundle(ctx, &self.sys.env).unwrap_or_else(|_| ctx.ca_path.clone());
        let mut copied = true;
        for (src, dst) in [(ctx.ca_path.clone(), COPIED_CA), (bundle, COPIED_BUNDLE)] {
            let src_s = src.to_string_lossy().into_owned();
            let target = format!("{}:{dst}", container.id);
            if let Err(e) = self
                .run(&engine, &["cp", &src_s, &target], CP_TIMEOUT)
                .await
            {
                tracing::info!(container = %container.id, "couldn't copy CA into container: {e}");
                copied = false;
            }
        }
        if copied {
            self.touched
                .lock()
                .push((engine.clone(), container.id.clone()));
        }
        let vars = build_env_vars(&EnvConfig {
            proxy_host: host.clone(),
            proxy_port: ctx.proxy_port,
            ca_file: COPIED_CA.into(),
            bundle_file: COPIED_BUNDLE.into(),
            java_truststore: None,
            proxy_credentials: ctx.proxy_credentials.clone(),
        });
        let mut exec = format!("{} exec -it", engine.kind.as_str());
        for (k, v) in vars
            .iter()
            .filter(|(_, v)| copied || (v != COPIED_CA && v != COPIED_BUNDLE))
        {
            exec.push_str(&format!(
                " -e {}",
                crate::scripts::posix_quote(&format!("{k}={v}"))
            ));
        }
        exec.push_str(&format!(" {} sh", short_id(&container.id)));
        Ok(ActivationResult {
            success: true,
            message: Some(format!(
                "Container {} keeps its original environment (it would have to be recreated to proxy its main process). {} Run the command below for an intercepted shell inside it, or recreate it with the snippet.",
                container.display_name(),
                if copied {
                    "The Akev CA was copied to /tmp in the container."
                } else {
                    "Copying the CA into the container failed (read-only filesystem?), so only plain HTTP is intercepted from the shell."
                }
            )),
            metadata: json!({
                "containerId": container.id,
                "engine": engine.kind.as_str(),
                "proxyHost": host,
                "caCopied": copied,
                "execCommand": exec,
                "snippets": snippets.json,
            }),
        })
    }

    async fn deactivate(&self, _ctx: &InterceptContext, options: serde_json::Value) -> Result<()> {
        let want = opt_str(&options, "containerId").map(String::from);
        let targets: Vec<(Engine, String)> = {
            let mut touched = self.touched.lock();
            let (matching, rest): (Vec<_>, Vec<_>) = touched.drain(..).partition(|(_, id)| {
                want.as_ref()
                    .is_none_or(|w| id == w || id.starts_with(w.as_str()))
            });
            *touched = rest;
            matching
        };
        for (engine, id) in targets {
            // Best effort: the container may have stopped since.
            if let Err(e) = self
                .run(
                    &engine,
                    &["exec", &id, "rm", "-f", COPIED_CA, COPIED_BUNDLE],
                    LIST_TIMEOUT,
                )
                .await
            {
                tracing::debug!(container = %id, "couldn't remove copied CA files: {e}");
            }
        }
        self.scan.invalidate().await;
        Ok(())
    }

    async fn deactivate_all(&self, ctx: &InterceptContext) -> Result<()> {
        self.deactivate(ctx, json!({})).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::testutil::{fake_env, make_exe};
    use crate::runner::CommandOutput;
    use crate::runner::mock::MockRunner;
    use crate::util::testctx::ctx;

    const DOCKER_PS: &str = r#"{"Command":"\"nginx -g 'daemon off;'\"","CreatedAt":"2026-11-08 10:00:00 +0100 CET","ID":"3f2a9c1b5d7e8f901234567890abcdef1234567890abcdef1234567890abcdef","Image":"nginx:latest","Labels":"maintainer=NGINX","Names":"web","Networks":"bridge","Ports":"80/tcp","State":"running","Status":"Up 5 minutes"}
{"Command":"\"sleep 1d\"","ID":"aaaaaaaaaaaabbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","Image":"alpine","Names":"a,b","State":"running","Status":"Up 1 second"}
"#;
    const PODMAN_PS: &str = r#"{"AutoRemove":false,"Command":["sleep","1d"],"Created":"2026-11-08T10:00:00Z","Id":"c0ffee0000000000000000000000000000000000000000000000000000000000","Image":"docker.io/library/alpine:latest","Labels":{"a":"b"},"Names":["podtest"],"Networks":["podman"],"Pid":1234,"State":"running","Status":"Up 3 seconds"}
"#;

    #[test]
    fn parses_docker_and_podman_ps() {
        let d = parse_ps(DOCKER_PS);
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].names, vec!["web"]);
        assert_eq!(d[0].image, "nginx:latest");
        assert_eq!(d[1].names, vec!["a", "b"]);
        let p = parse_ps(PODMAN_PS);
        assert_eq!(
            p[0].id,
            "c0ffee0000000000000000000000000000000000000000000000000000000000"
        );
        assert_eq!(p[0].names, vec!["podtest"]);
        assert_eq!(p[0].state, "running");
        let arr = parse_ps(&format!("[{}]", PODMAN_PS.trim()));
        assert_eq!(arr.len(), 1);
        assert!(parse_ps("").is_empty());
        assert!(parse_ps("not json\n{}\n").is_empty());
    }

    #[test]
    fn detects_intercepted_containers() {
        let out = "abc [\"PATH=/usr/bin\",\"HTTP_PROXY=http://host.docker.internal:8000\",\"AKEV_ACTIVE=1\"]\n\
                   def [\"HTTP_PROXY=http://host.docker.internal:9000\",\"AKEV_ACTIVE=1\"]\n\
                   ghi [\"HTTP_PROXY=http://corp:8000\"]\njkl null\n";
        let ids = parse_intercepted(out, 8000);
        assert_eq!(ids, HashSet::from(["abc".to_string()]));
    }

    #[test]
    fn env_file_and_snippets() {
        let dir = tempfile::tempdir().unwrap();
        let c = ctx(dir.path());
        let content = env_file_content("host.docker.internal", 8000, None);
        assert!(content.contains("\nHTTP_PROXY=http://host.docker.internal:8000\n"));
        let creds = ("dev".to_string(), "p@ss word".to_string());
        let with_creds = env_file_content("host.docker.internal", 8000, Some(&creds));
        assert!(
            with_creds
                .contains("\nHTTP_PROXY=http://dev:p%40ss%20word@host.docker.internal:8000\n"),
            "{with_creds}"
        );
        assert!(content.contains("\nSSL_CERT_FILE=/akev/ca-bundle.pem\n"));
        assert!(content.contains("\nNODE_EXTRA_CA_CERTS=/akev/ca.pem\n"));
        assert!(content.contains("\nNO_PROXY=\n"));

        let s = snippets(
            &c,
            Os::Linux,
            Path::new("/d/ca.pem"),
            Path::new("/d/ca-bundle.pem"),
        );
        let docker = s.json["run"]["docker"].as_str().unwrap();
        assert!(docker.starts_with(
            "docker run --rm -it --add-host=host.docker.internal:host-gateway --env-file "
        ));
        assert!(docker.contains(
            "-v /d/ca.pem:/akev/ca.pem:ro,z -v /d/ca-bundle.pem:/akev/ca-bundle.pem:ro,z IMAGE"
        ));
        assert!(
            !s.json["run"]["podman"]
                .as_str()
                .unwrap()
                .contains("--add-host")
        );
        assert!(
            s.json["runHostNetwork"]
                .as_str()
                .unwrap()
                .contains("--network host")
        );
        assert_eq!(s.files.len(), 3);
        assert!(
            s.files
                .iter()
                .any(|(_, c)| c.contains("HTTP_PROXY=http://127.0.0.1:8000"))
        );
        assert!(
            s.files
                .iter()
                .any(|(_, c)| c.contains("HTTP_PROXY=http://host.containers.internal:8000"))
        );

        let mac = snippets(
            &c,
            Os::MacOs,
            Path::new("/Users/me/Library/Application Support/akev/ca.pem"),
            Path::new("/b.pem"),
        );
        let docker = mac.json["run"]["docker"].as_str().unwrap();
        assert!(!docker.contains("--add-host") && !docker.contains(",z"));
        assert!(
            docker
                .contains("-v '/Users/me/Library/Application Support/akev/ca.pem':/akev/ca.pem:ro")
        );
        assert!(mac.json.get("runHostNetwork").is_none());
    }

    #[test]
    fn podman_shim_is_not_docker() {
        let dir = tempfile::tempdir().unwrap();
        make_exe(
            &dir.path().join("usr/bin/docker"),
            "#!/bin/sh\nexec /usr/bin/podman \"$@\"\n",
        );
        make_exe(&dir.path().join("usr/bin/podman"), "\x7fELF");
        let engines = detect_engines(&fake_env(Os::Linux, dir.path()));
        assert_eq!(
            engines.iter().map(|e| e.kind).collect::<Vec<_>>(),
            vec![EngineKind::Podman]
        );
    }

    #[test]
    fn proxy_host_for_existing_containers() {
        assert_eq!(
            container_proxy_host(EngineKind::Docker, "host|", Os::Linux),
            "127.0.0.1"
        );
        assert_eq!(
            container_proxy_host(EngineKind::Docker, "bridge|172.18.0.1,", Os::Linux),
            "172.18.0.1"
        );
        assert_eq!(
            container_proxy_host(EngineKind::Docker, "bridge|", Os::Linux),
            "172.17.0.1"
        );
        assert_eq!(
            container_proxy_host(EngineKind::Docker, "bridge|172.18.0.1,", Os::MacOs),
            "host.docker.internal"
        );
        assert_eq!(
            container_proxy_host(EngineKind::Podman, "pasta|", Os::Linux),
            "host.containers.internal"
        );
    }

    fn setup() -> (
        tempfile::TempDir,
        Arc<MockRunner>,
        DockerInterceptor,
        InterceptContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        make_exe(&dir.path().join("usr/bin/docker"), "\x7fELF");
        make_exe(&dir.path().join("usr/bin/podman"), "\x7fELF");
        let runner = MockRunner::new();
        runner.on_prefix("docker ps", CommandOutput::ok(DOCKER_PS));
        runner.on_prefix(
            "podman ps",
            CommandOutput::failed("Cannot connect to Podman socket"),
        );
        runner.on_prefix(
            "docker inspect --format {{.Id}}",
            CommandOutput::ok("3f2a9c1b5d7e8f901234567890abcdef1234567890abcdef1234567890abcdef [\"HTTP_PROXY=http://host.docker.internal:8000\",\"AKEV_ACTIVE=1\"]\n"),
        );
        runner.on_prefix(
            "docker inspect --format {{.HostConfig.NetworkMode}}",
            CommandOutput::ok("bridge|172.17.0.1,\n"),
        );
        runner.on_prefix("docker cp", CommandOutput::ok(""));
        runner.on_prefix("docker exec", CommandOutput::ok(""));
        let i = DockerInterceptor::new(Sys::new(runner.clone(), fake_env(Os::Linux, dir.path())));
        let c = ctx(&dir.path().join("data"));
        (dir, runner, i, c)
    }

    #[tokio::test]
    async fn info_lists_containers_and_writes_env_files() {
        let (_dir, _runner, i, c) = setup();
        let info = i.info(&c).await;
        assert!(info.available && info.active && info.needs_config);
        let containers = info.metadata["containers"].as_array().unwrap();
        assert_eq!(containers.len(), 2);
        assert_eq!(containers[0]["name"], json!("web"));
        assert_eq!(containers[0]["intercepted"], json!(true));
        assert_eq!(containers[1]["intercepted"], json!(false));
        let engines = info.metadata["engines"].as_array().unwrap();
        assert_eq!(engines[1]["available"], json!(false));
        assert!(c.data_dir.join("docker/akev-docker.env").is_file());
        assert!(c.data_dir.join("ca-bundle.pem").is_file());
    }

    #[tokio::test]
    async fn activate_copies_ca_and_returns_exec_command() {
        let (_dir, runner, i, c) = setup();
        let res = i.activate(&c, json!({"containerId": "web"})).await.unwrap();
        assert!(res.success);
        assert_eq!(res.metadata["caCopied"], json!(true));
        assert_eq!(res.metadata["proxyHost"], json!("172.17.0.1"));
        let exec = res.metadata["execCommand"].as_str().unwrap();
        assert!(exec.starts_with("docker exec -it -e 'HTTP_PROXY=http://172.17.0.1:8000'"));
        assert!(exec.contains("-e 'SSL_CERT_FILE=/tmp/akev-ca-bundle.pem'"));
        assert!(exec.ends_with(" 3f2a9c1b5d7e sh"));
        let lines = runner.call_lines();
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("docker cp ") && l.ends_with(":/tmp/akev-ca.pem"))
        );

        i.deactivate_all(&c).await.unwrap();
        assert!(
            runner
                .call_lines()
                .iter()
                .any(|l| l.starts_with("docker exec 3f2a9c1b5d7e")
                    && l.contains("rm -f /tmp/akev-ca.pem"))
        );

        // No id: just the snippets. Unknown id: error.
        let res = i.activate(&c, json!({})).await.unwrap();
        assert!(res.metadata["snippets"]["run"]["docker"].is_string());
        assert!(matches!(
            i.activate(&c, json!({"containerId": "zzz"})).await,
            Err(InterceptError::Unavailable(_))
        ));
    }

    #[tokio::test]
    async fn unavailable_without_engines() {
        let dir = tempfile::tempdir().unwrap();
        let i =
            DockerInterceptor::new(Sys::new(MockRunner::new(), fake_env(Os::Linux, dir.path())));
        let info = i.info(&ctx(dir.path())).await;
        assert!(!info.available);
        assert!(
            info.unavailable_reason
                .unwrap()
                .contains("Neither docker nor podman")
        );
    }
}
