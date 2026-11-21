//! Proxy & CA-trust environment variables for CLI tools and runtimes, plus the files they
//! point at: a combined CA bundle (system roots + Akev CA) and an optional Java truststore.
//!
//! Variables that *replace* a tool's trust store (`SSL_CERT_FILE`, `REQUESTS_CA_BUNDLE`,
//! `GIT_SSL_CAINFO`, ...) point at the combined bundle so hosts that bypass the proxy keep
//! working; variables that *add* to the default store (`NODE_EXTRA_CA_CERTS`) point at the
//! Akev CA alone.
//!
//! `ALL_PROXY` is deliberately **not** set: many tools treat it as a SOCKS proxy or apply it
//! to non-HTTP protocols (git's `git://`, rsync, ftp), which breaks them.

use crate::InterceptContext;
use crate::host::{HostEnv, Os};
use crate::runner::CommandSpec;
use crate::util::{
    Sys, atomic_write, host_for_url, local_proxy_host, short_hash, write_if_changed,
};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

/// Every variable any Akev setup script may set. Reset scripts restore/unset exactly these,
/// so the list is static (reset scripts are generated without a context).
pub const ENV_VAR_NAMES: &[&str] = &[
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "http_proxy",
    "https_proxy",
    "WS_PROXY",
    "WSS_PROXY",
    "NO_PROXY",
    "no_proxy",
    "GLOBAL_AGENT_HTTP_PROXY",
    "CGI_HTTP_PROXY",
    "npm_config_proxy",
    "npm_config_https_proxy",
    "YARN_HTTP_PROXY",
    "YARN_HTTPS_PROXY",
    "NODE_USE_ENV_PROXY",
    "ELECTRON_GET_USE_PROXY",
    "SSL_CERT_FILE",
    "REQUESTS_CA_BUNDLE",
    "CURL_CA_BUNDLE",
    "GIT_SSL_CAINFO",
    "AWS_CA_BUNDLE",
    "CARGO_HTTP_CAINFO",
    "PIP_CERT",
    "BUNDLE_SSL_CA_CERT",
    "DENO_CERT",
    "HEX_CACERTS_PATH",
    "PERL_LWP_SSL_CA_FILE",
    "CLOUDSDK_CORE_CUSTOM_CA_CERTS_FILE",
    "GRPC_DEFAULT_SSL_ROOTS_FILE_PATH",
    "NIX_SSL_CERT_FILE",
    "HTTPLIB2_CA_CERTS",
    "NODE_EXTRA_CA_CERTS",
    "JAVA_TOOL_OPTIONS",
    "AKEV_ACTIVE",
    "AKEV_PROXY",
];

/// Variables whose previous value is kept and extended (space-separated) instead of
/// replaced, so user flags like `-Xmx` survive.
pub const APPEND_VARS: &[&str] = &["JAVA_TOOL_OPTIONS"];

/// Inputs for [`build_env_vars`].
#[derive(Clone, Debug)]
pub struct EnvConfig {
    /// Proxy host as seen by the client (no brackets).
    pub proxy_host: String,
    /// Proxy port.
    pub proxy_port: u16,
    /// Path of the Akev CA PEM (as seen by the client).
    pub ca_file: String,
    /// Path of the combined bundle (as seen by the client).
    pub bundle_file: String,
    /// PKCS#12 Java truststore (system roots + Akev CA), if one exists.
    pub java_truststore: Option<String>,
    /// Proxy credentials to put in the proxy URLs (`http://user:pass@host:port`), for clients
    /// that connect from another address.
    pub proxy_credentials: Option<(String, String)>,
}

/// Build the variable list (deterministic order).
pub fn build_env_vars(cfg: &EnvConfig) -> Vec<(String, String)> {
    let userinfo = cfg
        .proxy_credentials
        .as_ref()
        .map(|(user, pass)| format!("{}:{}@", userinfo_escape(user), userinfo_escape(pass)))
        .unwrap_or_default();
    let proxy_url = format!(
        "http://{userinfo}{}:{}",
        host_for_url(&cfg.proxy_host),
        cfg.proxy_port
    );
    let bundle = cfg.bundle_file.as_str();
    let vars: Vec<(&str, String)> = vec![
        // The conventional variables, both cases (curl only reads lowercase http_proxy).
        ("HTTP_PROXY", proxy_url.clone()),
        ("HTTPS_PROXY", proxy_url.clone()),
        ("http_proxy", proxy_url.clone()),
        ("https_proxy", proxy_url.clone()),
        // Some WebSocket clients look these up separately.
        ("WS_PROXY", proxy_url.clone()),
        ("WSS_PROXY", proxy_url.clone()),
        // Clear inherited exclusions so everything (even localhost) is captured.
        ("NO_PROXY", String::new()),
        ("no_proxy", String::new()),
        // Node: global-agent bootstrap, CGI (httpoxy-safe name), npm/yarn, Node ≥ 24 native.
        ("GLOBAL_AGENT_HTTP_PROXY", proxy_url.clone()),
        ("CGI_HTTP_PROXY", proxy_url.clone()),
        ("npm_config_proxy", proxy_url.clone()),
        ("npm_config_https_proxy", proxy_url.clone()),
        ("YARN_HTTP_PROXY", proxy_url.clone()),
        ("YARN_HTTPS_PROXY", proxy_url.clone()),
        ("NODE_USE_ENV_PROXY", "1".into()),
        ("ELECTRON_GET_USE_PROXY", "1".into()),
        // Trust-store replacements → combined bundle.
        ("SSL_CERT_FILE", bundle.into()), // OpenSSL, Ruby, Go (Linux), httpx, uv, rustls-native-certs
        ("REQUESTS_CA_BUNDLE", bundle.into()), // Python requests, Azure CLI, conda
        ("CURL_CA_BUNDLE", bundle.into()), // curl, PHP
        ("GIT_SSL_CAINFO", bundle.into()), // git
        ("AWS_CA_BUNDLE", bundle.into()), // AWS CLI & SDKs
        ("CARGO_HTTP_CAINFO", bundle.into()), // cargo
        ("PIP_CERT", bundle.into()),      // pip
        ("BUNDLE_SSL_CA_CERT", bundle.into()), // Ruby bundler
        ("DENO_CERT", bundle.into()),     // Deno
        ("HEX_CACERTS_PATH", bundle.into()), // Elixir hex
        ("PERL_LWP_SSL_CA_FILE", bundle.into()), // Perl LWP
        ("CLOUDSDK_CORE_CUSTOM_CA_CERTS_FILE", bundle.into()), // gcloud
        ("GRPC_DEFAULT_SSL_ROOTS_FILE_PATH", bundle.into()), // gRPC C-core: grpcio, Ruby, PHP, C++
        ("NIX_SSL_CERT_FILE", bundle.into()), // Nix-built OpenSSL and curl
        ("HTTPLIB2_CA_CERTS", bundle.into()), // Python httplib2 (older Google API clients)
        // Additive: Node, npm, Bun, Electron main process.
        ("NODE_EXTRA_CA_CERTS", cfg.ca_file.clone()),
        ("JAVA_TOOL_OPTIONS", java_tool_options(cfg)),
        ("AKEV_ACTIVE", "1".into()),
        ("AKEV_PROXY", proxy_url.clone()),
    ];
    vars.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

/// Percent-encode a URL user-info part (RFC 3986 unreserved characters stay as they are).
fn userinfo_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn java_tool_options(cfg: &EnvConfig) -> String {
    let host = cfg.proxy_host.trim_start_matches('[').trim_end_matches(']');
    let port = cfg.proxy_port;
    let mut opts = vec![
        format!("-Dhttp.proxyHost={host}"),
        format!("-Dhttp.proxyPort={port}"),
        format!("-Dhttps.proxyHost={host}"),
        format!("-Dhttps.proxyPort={port}"),
        // Empty = no exclusions (the JVM default skips localhost).
        "-Dhttp.nonProxyHosts=".to_string(),
        // Allow Basic auth through CONNECT tunnels (disabled by default since 8u111).
        "-Djdk.http.auth.tunneling.disabledSchemes=".to_string(),
    ];
    if let Some(ts) = &cfg.java_truststore {
        opts.push(format!("-Djavax.net.ssl.trustStore={}", java_quote(ts)));
        opts.push("-Djavax.net.ssl.trustStorePassword=changeit".to_string());
        opts.push("-Djavax.net.ssl.trustStoreType=PKCS12".to_string());
    }
    opts.join(" ")
}

/// The JVM splits `JAVA_TOOL_OPTIONS` on whitespace but honours double quotes.
fn java_quote(value: &str) -> String {
    if value.chars().any(char::is_whitespace) {
        format!("\"{value}\"")
    } else {
        value.to_string()
    }
}

/// Env vars for a local client of `ctx` (paths on this machine). Creates the combined bundle
/// on demand; falls back to the bare CA file if the bundle can't be written.
pub(crate) fn local_env_vars(ctx: &InterceptContext, env: &HostEnv) -> Vec<(String, String)> {
    ensure_ca_file(ctx);
    let bundle = match ensure_ca_bundle(ctx, env) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("couldn't write the combined CA bundle: {e}");
            ctx.ca_path.clone()
        }
    };
    build_env_vars(&EnvConfig {
        proxy_host: local_proxy_host(ctx),
        proxy_port: ctx.proxy_port,
        ca_file: ctx.ca_path.to_string_lossy().into_owned(),
        bundle_file: bundle.to_string_lossy().into_owned(),
        java_truststore: java_truststore_if_ready(ctx).map(|p| p.to_string_lossy().into_owned()),
        // This machine's own clients connect over loopback, which never needs credentials.
        proxy_credentials: None,
    })
}

/// Make sure the CA PEM exists at `ctx.ca_path` (the API normally writes it; this guards
/// against a missing file making every env var point at nothing).
pub(crate) fn ensure_ca_file(ctx: &InterceptContext) {
    if !ctx.ca_path.exists()
        && !ctx.ca_pem.is_empty()
        && let Err(e) = atomic_write(&ctx.ca_path, ctx.ca_pem.as_bytes())
    {
        tracing::warn!(path = %ctx.ca_path.display(), "couldn't write CA file: {e}");
    }
}

/// `<data_dir>/ca-bundle.pem`.
pub fn ca_bundle_path(data_dir: &Path) -> PathBuf {
    data_dir.join("ca-bundle.pem")
}

/// Candidate system CA bundles (PEM), most specific first.
pub(crate) fn system_ca_bundle(env: &HostEnv, exclude: &Path) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    // A bundle the user already relies on (e.g. a corporate one) wins.
    for key in ["SSL_CERT_FILE", "REQUESTS_CA_BUNDLE", "CURL_CA_BUNDLE"] {
        if let Some(p) = env.var_path(key) {
            candidates.push(p);
        }
    }
    match env.os {
        Os::Windows => {
            for base in ["PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA"] {
                if let Some(b) = env.var_path(base) {
                    let git = if base == "LOCALAPPDATA" {
                        b.join("Programs").join("Git")
                    } else {
                        b.join("Git")
                    };
                    candidates.push(
                        git.join("mingw64")
                            .join("etc")
                            .join("ssl")
                            .join("certs")
                            .join("ca-bundle.crt"),
                    );
                    candidates.push(
                        git.join("mingw64")
                            .join("ssl")
                            .join("certs")
                            .join("ca-bundle.crt"),
                    );
                }
            }
        }
        _ => {
            for p in [
                "/etc/ssl/certs/ca-certificates.crt", // Debian, Ubuntu, Arch, Gentoo, Fedora (compat)
                "/etc/pki/tls/certs/ca-bundle.crt",   // Fedora, RHEL
                "/etc/pki/ca-trust/extracted/pem/tls-ca-bundle.pem", // RHEL/CentOS 7+
                "/etc/ssl/ca-bundle.pem",             // openSUSE
                "/etc/ssl/cert.pem",                  // macOS, Alpine, BSDs
                "/usr/local/etc/openssl/cert.pem",    // Homebrew OpenSSL
            ] {
                candidates.push(env.sys_path(p));
            }
        }
    }
    candidates.into_iter().find(|p| p != exclude && p.is_file())
}

fn bundle_memo() -> &'static Mutex<HashMap<PathBuf, String>> {
    static MEMO: OnceLock<Mutex<HashMap<PathBuf, String>>> = OnceLock::new();
    MEMO.get_or_init(Default::default)
}

/// Write `<data_dir>/ca-bundle.pem` = system bundle + Akev CA if it's missing or stale.
/// Memoised per (CA, system bundle version), so repeated calls cost a `stat`.
pub fn ensure_ca_bundle(ctx: &InterceptContext, env: &HostEnv) -> io::Result<PathBuf> {
    let path = ca_bundle_path(&ctx.data_dir);
    let system = system_ca_bundle(env, &path);
    let system_stamp = system
        .as_ref()
        .and_then(|p| {
            std::fs::metadata(p)
                .ok()
                .map(|m| (p.clone(), m.len(), m.modified().ok()))
        })
        .map(|(p, len, mtime)| format!("{}:{len}:{mtime:?}", p.display()))
        .unwrap_or_default();
    let key = format!("{}|{system_stamp}", short_hash(ctx.ca_pem.as_bytes()));
    {
        let memo = bundle_memo().lock();
        if memo.get(&path) == Some(&key) && path.is_file() {
            return Ok(path);
        }
    }

    let mut contents = match &system {
        Some(p) => std::fs::read(p).unwrap_or_default(),
        None => Vec::new(),
    };
    if !contents.is_empty() && !contents.ends_with(b"\n") {
        contents.push(b'\n');
    }
    contents.extend_from_slice(b"# Akev CA\n");
    contents.extend_from_slice(ctx.ca_pem.trim_end().as_bytes());
    contents.push(b'\n');
    write_if_changed(&path, &contents)?;
    bundle_memo().lock().insert(path.clone(), key);
    Ok(path)
}

/// `<data_dir>/java/akev-truststore.p12`.
pub fn java_truststore_path(data_dir: &Path) -> PathBuf {
    data_dir.join("java").join("akev-truststore.p12")
}

fn truststore_marker(data_dir: &Path) -> PathBuf {
    data_dir.join("java").join("akev-truststore.ca")
}

/// The truststore, if it exists and was built for the current CA.
pub(crate) fn java_truststore_if_ready(ctx: &InterceptContext) -> Option<PathBuf> {
    let path = java_truststore_path(&ctx.data_dir);
    let marker = std::fs::read_to_string(truststore_marker(&ctx.data_dir)).ok()?;
    (marker.trim() == short_hash(ctx.ca_pem.as_bytes()) && path.is_file()).then_some(path)
}

/// Locate `keytool` and the JVM's default `cacerts`.
pub(crate) fn find_java_tools(env: &HostEnv) -> Option<(PathBuf, PathBuf)> {
    let exe = if env.os == Os::Windows {
        "keytool.exe"
    } else {
        "keytool"
    };
    let mut homes: Vec<PathBuf> = Vec::new();
    if let Some(h) = env.var_path("JAVA_HOME") {
        homes.push(h);
    }
    let keytool = homes
        .iter()
        .map(|h| h.join("bin").join(exe))
        .find(|p| p.is_file())
        .or_else(|| env.find_in_path("keytool"))?;
    if let Ok(real) = std::fs::canonicalize(&keytool)
        && let Some(home) = real.parent().and_then(Path::parent)
    {
        homes.push(home.to_path_buf());
    }
    let cacerts = homes.iter().find_map(|h| {
        [
            h.join("lib").join("security").join("cacerts"),
            h.join("jre").join("lib").join("security").join("cacerts"),
        ]
        .into_iter()
        .find(|p| p.is_file())
    })?;
    Some((keytool, cacerts))
}

fn truststore_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// Build the Java truststore (JVM default roots + Akev CA) with `keytool`, if available.
/// Returns the path when a truststore for the current CA exists afterwards.
pub async fn ensure_java_truststore(ctx: &InterceptContext, sys: &Sys) -> Option<PathBuf> {
    if let Some(p) = java_truststore_if_ready(ctx) {
        return Some(p);
    }
    let _guard = truststore_lock().lock().await;
    if let Some(p) = java_truststore_if_ready(ctx) {
        return Some(p);
    }
    let (keytool, cacerts) = find_java_tools(&sys.env)?;
    ensure_ca_file(ctx);
    let path = java_truststore_path(&ctx.data_dir);
    let tmp = path.with_extension("p12.tmp");
    if let Some(dir) = path.parent()
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        tracing::debug!("can't create java truststore dir: {e}");
        return None;
    }
    let _ = std::fs::remove_file(&tmp);
    let timeout = Duration::from_secs(30);
    let copy_roots = CommandSpec::new(&keytool)
        .args(["-importkeystore", "-noprompt", "-srckeystore"])
        .arg(&cacerts)
        .args(["-srcstorepass", "changeit", "-destkeystore"])
        .arg(&tmp)
        .args(["-deststoretype", "PKCS12", "-deststorepass", "changeit"])
        .timeout(timeout);
    let add_ca = CommandSpec::new(&keytool)
        .args(["-importcert", "-noprompt", "-alias", "akev-ca", "-file"])
        .arg(&ctx.ca_path)
        .arg("-keystore")
        .arg(&tmp)
        .args(["-storetype", "PKCS12", "-storepass", "changeit"])
        .timeout(timeout);
    for spec in [copy_roots, add_ca] {
        match sys.runner.output(&spec).await {
            Ok(out) if out.success() => {}
            Ok(out) => {
                tracing::info!(
                    "keytool failed ({}); Java trust store options disabled",
                    out.error_summary()
                );
                let _ = std::fs::remove_file(&tmp);
                return None;
            }
            Err(e) => {
                tracing::info!("couldn't run keytool: {e}");
                let _ = std::fs::remove_file(&tmp);
                return None;
            }
        }
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        tracing::info!("couldn't install java truststore: {e}");
        return None;
    }
    if let Err(e) = atomic_write(
        &truststore_marker(&ctx.data_dir),
        short_hash(ctx.ca_pem.as_bytes()).as_bytes(),
    ) {
        tracing::info!("couldn't write truststore marker: {e}");
        return None;
    }
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::testutil::{fake_env, make_exe, native};
    use crate::runner::CommandOutput;
    use crate::runner::mock::MockRunner;
    use crate::util::testctx::ctx;

    fn cfg() -> EnvConfig {
        EnvConfig {
            proxy_host: "127.0.0.1".into(),
            proxy_port: 8000,
            ca_file: "/data/ca.pem".into(),
            bundle_file: "/data/ca-bundle.pem".into(),
            java_truststore: None,
            proxy_credentials: None,
        }
    }

    fn get<'a>(vars: &'a [(String, String)], k: &str) -> Option<&'a str> {
        vars.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
    }

    #[test]
    fn env_vars_cover_proxy_and_trust() {
        let vars = build_env_vars(&cfg());
        for k in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "http_proxy",
            "https_proxy",
            "npm_config_proxy",
            "AKEV_PROXY",
        ] {
            assert_eq!(get(&vars, k), Some("http://127.0.0.1:8000"), "{k}");
        }
        for k in [
            "SSL_CERT_FILE",
            "REQUESTS_CA_BUNDLE",
            "CURL_CA_BUNDLE",
            "GIT_SSL_CAINFO",
            "AWS_CA_BUNDLE",
            "CARGO_HTTP_CAINFO",
            "PIP_CERT",
            "BUNDLE_SSL_CA_CERT",
            "DENO_CERT",
            "HEX_CACERTS_PATH",
            "PERL_LWP_SSL_CA_FILE",
            "CLOUDSDK_CORE_CUSTOM_CA_CERTS_FILE",
            "GRPC_DEFAULT_SSL_ROOTS_FILE_PATH",
            "NIX_SSL_CERT_FILE",
            "HTTPLIB2_CA_CERTS",
        ] {
            assert_eq!(get(&vars, k), Some("/data/ca-bundle.pem"), "{k}");
        }
        assert_eq!(get(&vars, "NODE_EXTRA_CA_CERTS"), Some("/data/ca.pem"));
        assert_eq!(get(&vars, "NO_PROXY"), Some(""));
        assert_eq!(get(&vars, "no_proxy"), Some(""));
        assert_eq!(get(&vars, "NODE_USE_ENV_PROXY"), Some("1"));
        assert_eq!(get(&vars, "ELECTRON_GET_USE_PROXY"), Some("1"));
        assert_eq!(get(&vars, "AKEV_ACTIVE"), Some("1"));
        // ALL_PROXY breaks SOCKS-assuming tools: never set.
        assert_eq!(get(&vars, "ALL_PROXY"), None);
        assert_eq!(get(&vars, "all_proxy"), None);
        // No duplicates, and everything is resettable.
        let mut names: Vec<_> = vars.iter().map(|(k, _)| k.as_str()).collect();
        assert!(names.iter().all(|n| ENV_VAR_NAMES.contains(n)));
        names.sort();
        names.dedup();
        assert_eq!(names.len(), vars.len());
        assert_eq!(vars.len(), ENV_VAR_NAMES.len());
    }

    #[test]
    fn java_options() {
        let vars = build_env_vars(&cfg());
        let jto = get(&vars, "JAVA_TOOL_OPTIONS").unwrap();
        assert!(jto.contains("-Dhttp.proxyHost=127.0.0.1 -Dhttp.proxyPort=8000"));
        assert!(jto.contains("-Dhttps.proxyHost=127.0.0.1 -Dhttps.proxyPort=8000"));
        assert!(jto.contains("-Djdk.http.auth.tunneling.disabledSchemes="));
        assert!(!jto.contains("trustStore"));

        let mut c = cfg();
        c.java_truststore = Some("/Users/me/Library/Application Support/akev/java/ts.p12".into());
        c.proxy_host = "::1".into();
        let vars = build_env_vars(&c);
        let jto = get(&vars, "JAVA_TOOL_OPTIONS").unwrap();
        assert!(jto.contains(
            "-Djavax.net.ssl.trustStore=\"/Users/me/Library/Application Support/akev/java/ts.p12\""
        ));
        assert!(jto.contains("-Djavax.net.ssl.trustStoreType=PKCS12"));
        assert!(jto.contains("-Dhttp.proxyHost=::1 "));
        assert_eq!(get(&vars, "HTTP_PROXY"), Some("http://[::1]:8000"));
    }

    #[test]
    fn combined_bundle_written_and_refreshed() {
        let dir = tempfile::tempdir().unwrap();
        let env = fake_env(Os::Linux, dir.path());
        let sys_bundle = env.sys_path("/etc/pki/tls/certs/ca-bundle.crt");
        std::fs::create_dir_all(sys_bundle.parent().unwrap()).unwrap();
        std::fs::write(
            &sys_bundle,
            "-----BEGIN CERTIFICATE-----\nSYSTEM\n-----END CERTIFICATE-----",
        )
        .unwrap();

        let mut c = ctx(&dir.path().join("data"));
        let path = ensure_ca_bundle(&c, &env).unwrap();
        assert_eq!(path, dir.path().join("data/ca-bundle.pem"));
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.starts_with(
            "-----BEGIN CERTIFICATE-----\nSYSTEM\n-----END CERTIFICATE-----\n# Akev CA\n"
        ));
        assert!(content.ends_with(&c.ca_pem));

        // A new CA rewrites the bundle.
        c.ca_pem = "-----BEGIN CERTIFICATE-----\nNEWCA\n-----END CERTIFICATE-----\n".into();
        ensure_ca_bundle(&c, &env).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("NEWCA"));

        // A user-provided SSL_CERT_FILE (corporate bundle) takes precedence.
        let corp = dir.path().join("corp.pem");
        std::fs::write(&corp, "CORP\n").unwrap();
        let env = env.with_var("SSL_CERT_FILE", corp.to_string_lossy());
        ensure_ca_bundle(&c, &env).unwrap();
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .starts_with("CORP\n# Akev CA")
        );

        // ...but never our own bundle (re-running inside an intercepted shell).
        let env = env.with_var("SSL_CERT_FILE", path.to_string_lossy());
        assert_eq!(system_ca_bundle(&env, &path), Some(sys_bundle));
    }

    #[test]
    fn bundle_without_system_roots_still_has_ca() {
        let dir = tempfile::tempdir().unwrap();
        let env = fake_env(Os::Linux, dir.path());
        let c = ctx(&dir.path().join("data2"));
        let path = ensure_ca_bundle(&c, &env).unwrap();
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            format!("# Akev CA\n{}", c.ca_pem)
        );
    }

    #[test]
    fn local_env_vars_use_bundle_and_rewrite_missing_ca() {
        let dir = tempfile::tempdir().unwrap();
        let env = fake_env(Os::Linux, dir.path());
        let c = ctx(&dir.path().join("data3"));
        std::fs::remove_file(&c.ca_path).unwrap();
        let vars = local_env_vars(&c, &env);
        assert!(c.ca_path.exists());
        assert_eq!(
            get(&vars, "SSL_CERT_FILE").map(PathBuf::from),
            Some(ca_bundle_path(&c.data_dir))
        );
        assert_eq!(
            get(&vars, "NODE_EXTRA_CA_CERTS").map(PathBuf::from),
            Some(c.ca_path.clone())
        );
    }

    #[tokio::test]
    async fn java_truststore_generation_with_keytool() {
        let dir = tempfile::tempdir().unwrap();
        let jdk = dir.path().join("jdk");
        make_exe(&jdk.join("bin/keytool"), "#!/bin/sh\n");
        std::fs::create_dir_all(jdk.join("lib/security")).unwrap();
        std::fs::write(jdk.join("lib/security/cacerts"), "x").unwrap();
        let env = fake_env(Os::Linux, dir.path()).with_var("JAVA_HOME", jdk.to_string_lossy());
        let runner = MockRunner::new();
        // keytool "creates" the destination file.
        runner.on(|spec| {
            if spec.program_name() != "keytool" {
                return None;
            }
            let args = spec.args_lossy();
            let dest = args
                .iter()
                .position(|a| a == "-destkeystore" || a == "-keystore")
                .and_then(|i| args.get(i + 1))
                .unwrap();
            std::fs::write(dest, "p12").unwrap();
            Some(Ok(CommandOutput::ok("")))
        });
        let sys = Sys::new(runner.clone(), env);
        let c = ctx(&dir.path().join("data"));
        assert_eq!(java_truststore_if_ready(&c), None);
        let ts = ensure_java_truststore(&c, &sys).await.unwrap();
        assert_eq!(ts, java_truststore_path(&c.data_dir));
        assert_eq!(java_truststore_if_ready(&c), Some(ts.clone()));
        let lines = runner.call_lines();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("keytool -importkeystore -noprompt -srckeystore"));
        assert!(lines[0].contains(&*native(&jdk, "lib/security/cacerts").to_string_lossy()));
        assert!(lines[1].starts_with("keytool -importcert -noprompt -alias akev-ca"));

        // Cached: no more keytool runs; env vars now carry the trust store.
        ensure_java_truststore(&c, &sys).await.unwrap();
        assert_eq!(runner.call_lines().len(), 2);
        let vars = local_env_vars(&c, &sys.env);
        assert!(
            get(&vars, "JAVA_TOOL_OPTIONS")
                .unwrap()
                .contains("-Djavax.net.ssl.trustStore=")
        );

        // A different CA invalidates it.
        let mut c2 = c.clone();
        c2.ca_pem = "other".into();
        assert_eq!(java_truststore_if_ready(&c2), None);
    }

    #[tokio::test]
    async fn java_truststore_skipped_without_keytool_or_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        let runner = MockRunner::new();
        let sys = Sys::new(runner.clone(), fake_env(Os::Linux, dir.path()));
        let c = ctx(&dir.path().join("data"));
        assert_eq!(ensure_java_truststore(&c, &sys).await, None);
        assert!(runner.call_lines().is_empty());

        let jdk = dir.path().join("jdk");
        make_exe(&jdk.join("bin/keytool"), "");
        std::fs::create_dir_all(jdk.join("jre/lib/security")).unwrap();
        std::fs::write(jdk.join("jre/lib/security/cacerts"), "x").unwrap();
        let runner = MockRunner::new();
        runner.on_prefix(
            "keytool",
            CommandOutput::failed("keytool error: java.io.IOException"),
        );
        let sys = Sys::new(
            runner.clone(),
            fake_env(Os::Linux, dir.path()).with_var("JAVA_HOME", jdk.to_string_lossy()),
        );
        assert_eq!(ensure_java_truststore(&c, &sys).await, None);
        assert!(
            !java_truststore_path(&c.data_dir)
                .with_extension("p12.tmp")
                .exists()
        );
    }
}
