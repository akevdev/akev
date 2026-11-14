//! Manual setup helpers:
//! * `manual` ("Anything"): proxy addresses (local & LAN), the PAC URL, the device setup page,
//!   CA download links, the environment variables and per-tool instructions;
//! * `ios`: Wi-Fi proxy + configuration profile + certificate trust steps for iPhone/iPad,
//!   with the LAN setup page URL as the QR code payload.
//!
//! Neither changes anything on this machine; activation just returns the instructions.

use crate::envvars::{ca_bundle_path, java_truststore_path, local_env_vars};
use crate::host::Os;
use crate::scripts::{posix_quote, ps_quote};
use crate::util::{Descriptor, Sys, host_for_url, local_proxy_host, local_proxy_url, tags};
use crate::{InterceptContext, Interceptor, Result};
use akev_core::interceptor::{ActivationResult, InterceptorCategory, InterceptorInfo};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;

/// Host the proxy answers setup requests on when reached *through* the proxy.
pub(crate) const MAGIC_HOST: &str = "akev.cert";

/// Base URL (`http://host:port`) for a host/port pair.
fn base_url(host: &str, port: u16) -> String {
    format!("http://{}:{port}", host_for_url(host))
}

/// URLs the proxy serves for one address.
fn endpoint_json(host: &str, port: u16) -> Value {
    let base = base_url(host, port);
    json!({
        "host": host,
        "port": port,
        "proxyUrl": base,
        "setupUrl": format!("{base}/"),
        "pacUrl": format!("{base}/proxy.pac"),
        "caPemUrl": format!("{base}/ca.pem"),
        "caDerUrl": format!("{base}/ca.crt"),
        "mobileconfigUrl": format!("{base}/ca.mobileconfig"),
    })
}

/// Preferred LAN endpoint for other devices.
fn primary_lan(ctx: &InterceptContext) -> Option<(String, u16)> {
    let host = ctx.lan_addresses.first()?;
    Some((host.clone(), ctx.proxy_port))
}

/// A path as a command-line *argument* (`curl --cacert <here>`).
fn quote_path(path: &str, os: Os) -> String {
    if os == Os::Windows {
        format!("\"{path}\"")
    } else {
        posix_quote(path)
    }
}

/// Environment variable assignments for the platform's shell, to paste into a terminal:
/// `export K=V` lines (values that are paths POSIX-quoted), or for Windows cmd `set "K=V"`
/// (the quotes around the whole assignment keep them *out* of the value, and no space before
/// `&&` sneaks into it).
fn env_snippet(pairs: &[(&str, &str, bool)], os: Os) -> String {
    if os == Os::Windows {
        pairs
            .iter()
            .map(|(k, v, _)| format!("set \"{k}={v}\""))
            .collect::<Vec<_>>()
            .join(" && ")
    } else {
        pairs
            .iter()
            .map(|(k, v, is_path)| {
                if *is_path {
                    format!("export {k}={}", posix_quote(v))
                } else {
                    format!("export {k}={v}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// The same assignments for PowerShell (`$env:K = 'V'`; Windows only).
fn env_snippet_powershell(pairs: &[(&str, &str, bool)]) -> String {
    pairs
        .iter()
        .map(|(k, v, _)| format!("$env:{k} = {}", ps_quote(v)))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Per-tool setup instructions.
pub(crate) fn tool_instructions(ctx: &InterceptContext, os: Os) -> Vec<Value> {
    let proxy = local_proxy_url(ctx);
    let host = local_proxy_host(ctx);
    let port = ctx.proxy_port;
    let ca = ctx.ca_path.to_string_lossy().into_owned();
    let bundle = ca_bundle_path(&ctx.data_dir).to_string_lossy().into_owned();
    let truststore = java_truststore_path(&ctx.data_dir)
        .to_string_lossy()
        .into_owned();
    let (ca_q, ts_q) = (quote_path(&ca, os), quote_path(&truststore, os));
    let lan_host = ctx
        .lan_addresses
        .first()
        .cloned()
        .unwrap_or_else(|| "<your computer's LAN IP>".into());
    let lan_port = port;
    let lan_base = base_url(&lan_host, lan_port);
    // `(name, value, is_path)` → the snippet object fields: `snippet` for the platform's
    // default shell (cmd on Windows) and, on Windows, `snippetPowerShell`.
    let env_tool = |id: &str, name: &str, pairs: &[(&str, &str, bool)], notes: Value| {
        let mut tool = json!({
            "id": id,
            "name": name,
            "snippet": env_snippet(pairs, os),
            "shell": if os == Os::Windows { "cmd" } else { "posix" },
            "notes": notes,
        });
        if os == Os::Windows {
            tool["snippetPowerShell"] = json!(env_snippet_powershell(pairs));
        }
        tool
    };
    vec![
        json!({
            "id": "curl",
            "name": "curl",
            "snippet": format!("curl --proxy {proxy} --cacert {ca_q} https://example.com"),
            "notes": ["Or export the environment variables below: curl reads http_proxy / https_proxy and CURL_CA_BUNDLE."],
        }),
        env_tool(
            "node",
            "Node.js",
            &[
                ("HTTPS_PROXY", &proxy, false),
                ("HTTP_PROXY", &proxy, false),
                ("NODE_USE_ENV_PROXY", "1", false),
                ("NODE_EXTRA_CA_CERTS", &ca, true),
            ],
            json!([
                "Recent Node.js releases honour HTTP(S)_PROXY for fetch and http(s) when NODE_USE_ENV_PROXY=1 is set.",
                "Older Node: `npm i global-agent` and run `node -r global-agent/bootstrap app.js` with GLOBAL_AGENT_HTTP_PROXY set, or pass an undici ProxyAgent to fetch.",
                "NODE_EXTRA_CA_CERTS adds the Akev CA to Node's built-in roots.",
            ]),
        ),
        env_tool(
            "python",
            "Python",
            &[
                ("HTTPS_PROXY", &proxy, false),
                ("HTTP_PROXY", &proxy, false),
                ("REQUESTS_CA_BUNDLE", &bundle, true),
                ("SSL_CERT_FILE", &bundle, true),
            ],
            json!([
                "requests and urllib read the proxy variables; requests trusts REQUESTS_CA_BUNDLE, httpx/urllib/aiohttp trust SSL_CERT_FILE.",
                "aiohttp needs ClientSession(trust_env=True) to use the proxy variables.",
                "The bundle contains your system roots plus the Akev CA (it's written when an interceptor first needs it).",
            ]),
        ),
        json!({
            "id": "java",
            "name": "Java / JVM",
            "snippet": format!("java -Dhttp.proxyHost={host} -Dhttp.proxyPort={port} -Dhttps.proxyHost={host} -Dhttps.proxyPort={port} -Dhttp.nonProxyHosts= -Djavax.net.ssl.trustStore={ts_q} -Djavax.net.ssl.trustStorePassword=changeit -jar app.jar"),
            "notes": [
                "Or set JAVA_TOOL_OPTIONS to the same flags so every JVM (Gradle, Maven, Kotlin, Scala...) picks them up.",
                format!("Akev generates the truststore (JVM default roots + Akev CA) with keytool when a terminal interceptor is used. To build one yourself: keytool -importcert -noprompt -alias akev-ca -file {ca_q} -keystore {ts_q} -storetype PKCS12 -storepass changeit"),
            ],
        }),
        env_tool(
            "go",
            "Go",
            &[
                ("HTTPS_PROXY", &proxy, false),
                ("HTTP_PROXY", &proxy, false),
                ("SSL_CERT_FILE", &bundle, true),
            ],
            json!([
                "net/http's default transport uses http.ProxyFromEnvironment; on Linux, crypto/x509 reads SSL_CERT_FILE.",
                "Go never proxies requests to localhost / 127.0.0.1: use your machine's hostname or LAN IP to capture local services.",
                "On macOS and Windows Go uses the OS trust store: trust the CA system-wide (see System Proxy) or add it to a custom tls.Config.",
            ]),
        ),
        env_tool(
            "rust",
            "Rust",
            &[
                ("HTTPS_PROXY", &proxy, false),
                ("HTTP_PROXY", &proxy, false),
                ("SSL_CERT_FILE", &bundle, true),
            ],
            json!([
                "reqwest uses the proxy variables by default; with rustls-native-certs or native-tls (OpenSSL) it trusts SSL_CERT_FILE.",
                // A Rust string literal (escaped), not a shell-quoted path.
                format!(
                    "Builds using webpki-roots need the CA in code: reqwest::Client::builder().add_root_certificate(reqwest::Certificate::from_pem(&std::fs::read({ca:?})?)?)"
                ),
                "cargo itself honours HTTPS_PROXY and CARGO_HTTP_CAINFO (both set by the terminal interceptors).",
            ]),
        ),
        json!({
            "id": "ios",
            "name": "iPhone / iPad",
            "snippet": format!("{lan_base}/ca.mobileconfig"),
            "notes": [
                format!("Settings → Wi-Fi → (i) → Configure Proxy → Manual: server {lan_host}, port {lan_port}."),
                format!("Open {lan_base}/ in Safari, install the configuration profile (Settings → Profile Downloaded → Install)."),
                "Settings → General → About → Certificate Trust Settings → enable full trust for the Akev CA.",
            ],
        }),
        json!({
            "id": "android",
            "name": "Android",
            "snippet": format!("{lan_base}/ca.crt"),
            "notes": [
                format!("Settings → Network & internet → Wi-Fi → your network → Edit → Proxy: Manual, host {lan_host}, port {lan_port} (or use the Android via ADB interceptor over USB)."),
                format!("Download {lan_base}/ca.crt in the browser, then Settings → Security → Encryption & credentials → Install a certificate → CA certificate."),
                "Apps targeting Android 7+ only trust user CAs if their network_security_config allows it, e.g. <debug-overrides><trust-anchors><certificates src=\"user\"/></trust-anchors></debug-overrides> for debug builds.",
            ],
        }),
    ]
}

/// Everything needed to configure a client by hand.
pub(crate) fn manual_metadata(ctx: &InterceptContext, sys: &Sys) -> Value {
    let host = local_proxy_host(ctx);
    let port = ctx.proxy_port;
    let local = endpoint_json(&host, port);
    let lan: Vec<Value> = ctx
        .lan_addresses
        .iter()
        .map(|h| endpoint_json(h, port))
        .collect();
    let env: Vec<Value> = local_env_vars(ctx, &sys.env)
        .into_iter()
        .map(|(name, value)| json!({ "name": name, "value": value }))
        .collect();
    json!({
        "proxy": {
            "host": host,
            "port": port,
            "url": local_proxy_url(ctx),
            "protocols": ["http", "https (CONNECT)", "socks4", "socks5"],
        },
        "local": local,
        "lan": lan,
        "pacUrl": local["pacUrl"],
        "setupUrl": local["setupUrl"],
        "setupUrlViaProxy": format!("http://{MAGIC_HOST}/"),
        "ca": {
            "path": ctx.ca_path,
            "bundlePath": ca_bundle_path(&ctx.data_dir),
            "spkiFingerprint": ctx.ca_spki_fingerprint,
            "pemUrl": local["caPemUrl"],
            "derUrl": local["caDerUrl"],
            "mobileconfigUrl": local["mobileconfigUrl"],
        },
        "envVars": env,
        "tools": tool_instructions(ctx, sys.env.os),
        "steps": [
            "Point the client at the proxy (address above, or the PAC URL for automatic configuration).",
            "Make the client trust the Akev CA (download links above) — needed for HTTPS.",
        ],
        "note": "Other devices can only connect if the proxy accepts connections from the network (bind it to all interfaces in Settings).",
    })
}

/// `manual` ("Anything").
pub(crate) struct ManualInterceptor {
    sys: Arc<Sys>,
}

impl ManualInterceptor {
    const DESCRIPTOR: Descriptor = Descriptor {
        id: "manual",
        name: "Anything",
        description: "Manually configure any source using the proxy settings and CA certificate",
        category: InterceptorCategory::Manual,
        tags: tags::MANUAL,
        icon: "manual",
        needs_config: false,
    };

    pub(crate) fn new(sys: Arc<Sys>) -> Self {
        ManualInterceptor { sys }
    }
}

#[async_trait]
impl Interceptor for ManualInterceptor {
    fn id(&self) -> &str {
        Self::DESCRIPTOR.id
    }

    async fn info(&self, ctx: &InterceptContext) -> InterceptorInfo {
        InterceptorInfo {
            available: true,
            metadata: manual_metadata(ctx, &self.sys),
            ..Self::DESCRIPTOR.info()
        }
    }

    async fn activate(&self, ctx: &InterceptContext, _options: Value) -> Result<ActivationResult> {
        Ok(ActivationResult {
            success: true,
            message: Some(format!(
                "Configure your client to use the proxy at {} and trust the Akev CA",
                local_proxy_url(ctx)
            )),
            metadata: manual_metadata(ctx, &self.sys),
        })
    }

    async fn deactivate(&self, _ctx: &InterceptContext, _options: Value) -> Result<()> {
        Ok(())
    }

    async fn deactivate_all(&self, _ctx: &InterceptContext) -> Result<()> {
        Ok(())
    }
}

/// Setup data for an iPhone/iPad, or `None` without a LAN address.
pub(crate) fn ios_metadata(ctx: &InterceptContext) -> Option<Value> {
    let (host, port) = primary_lan(ctx)?;
    let base = base_url(&host, port);
    let setup_url = format!("{base}/");
    let addresses: Vec<Value> = ctx
        .lan_addresses
        .iter()
        .map(|h| endpoint_json(h, port))
        .collect();
    Some(json!({
        "host": host,
        "port": port,
        "setupUrl": setup_url,
        "qrPayload": setup_url,
        "mobileconfigUrl": format!("{base}/ca.mobileconfig"),
        "addresses": addresses,
        "steps": [
            {
                "title": "Connect to the same network",
                "detail": "Join the Wi-Fi network this computer is on.",
            },
            {
                "title": "Set the Wi-Fi proxy",
                "detail": format!("Settings → Wi-Fi → tap (i) next to the network → Configure Proxy → Manual. Server: {host}, Port: {port}, Authentication off. Save."),
            },
            {
                "title": "Install the Akev profile",
                "detail": format!("Scan the QR code (or open {setup_url} in Safari) and tap the iPhone/iPad button, or open {base}/ca.mobileconfig directly. Allow the download, then Settings → Profile Downloaded → Install."),
            },
            {
                "title": "Trust the certificate",
                "detail": "Settings → General → About → Certificate Trust Settings → turn on full trust for the Akev CA. Without this, HTTPS fails.",
            },
            {
                "title": "Undo when done",
                "detail": "Set Configure Proxy back to Off, and remove the profile in Settings → General → VPN & Device Management.",
            },
        ],
        "notes": [
            "The proxy must accept connections from other devices (bind it to all interfaces in Settings) and your firewall must allow the port.",
            "Apps that pin certificates won't work through any proxy.",
        ],
    }))
}

/// `ios`.
pub(crate) struct IosInterceptor;

impl IosInterceptor {
    const DESCRIPTOR: Descriptor = Descriptor {
        id: "ios",
        name: "iOS via Manual Setup",
        description: "Manually intercept all HTTP and HTTPS traffic from any iPhone or iPad on your network",
        category: InterceptorCategory::Mobile,
        tags: tags::IOS,
        icon: "ios",
        needs_config: true,
    };
}

const NO_LAN: &str =
    "No network address found: connect this computer to the same Wi-Fi network as the device";

#[async_trait]
impl Interceptor for IosInterceptor {
    fn id(&self) -> &str {
        Self::DESCRIPTOR.id
    }

    async fn info(&self, ctx: &InterceptContext) -> InterceptorInfo {
        let d = Self::DESCRIPTOR;
        match ios_metadata(ctx) {
            Some(metadata) => InterceptorInfo {
                available: true,
                metadata,
                ..d.info()
            },
            None => InterceptorInfo {
                unavailable_reason: Some(NO_LAN.into()),
                ..d.info()
            },
        }
    }

    async fn activate(&self, ctx: &InterceptContext, _options: Value) -> Result<ActivationResult> {
        let metadata =
            ios_metadata(ctx).ok_or_else(|| crate::InterceptError::Unavailable(NO_LAN.into()))?;
        Ok(ActivationResult {
            success: true,
            message: Some(format!(
                "On your iPhone/iPad: set the Wi-Fi proxy to {}:{}, then install and trust the Akev profile",
                metadata["host"].as_str().unwrap_or_default(),
                metadata["port"]
            )),
            metadata,
        })
    }

    async fn deactivate(&self, _ctx: &InterceptContext, _options: Value) -> Result<()> {
        Ok(())
    }

    async fn deactivate_all(&self, _ctx: &InterceptContext) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::testutil::fake_env;
    use crate::runner::mock::MockRunner;
    use crate::util::testctx::ctx;

    #[tokio::test]
    async fn manual_setup_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let c = ctx(&dir.path().join("data"));
        let runner = MockRunner::new();
        let i = ManualInterceptor::new(Sys::new(runner.clone(), fake_env(Os::Linux, dir.path())));
        let info = i.info(&c).await;
        assert!(info.available && !info.needs_config && !info.active);
        let m = &info.metadata;
        assert_eq!(m["proxy"]["url"], json!("http://127.0.0.1:8000"));
        assert_eq!(m["pacUrl"], json!("http://127.0.0.1:8000/proxy.pac"));
        assert_eq!(m["setupUrl"], json!("http://127.0.0.1:8000/"));
        assert_eq!(m["setupUrlViaProxy"], json!("http://akev.cert/"));
        assert_eq!(m["lan"][0]["proxyUrl"], json!("http://192.168.1.20:8000"));
        assert_eq!(
            m["lan"][0]["pacUrl"],
            json!("http://192.168.1.20:8000/proxy.pac")
        );
        assert_eq!(m["ca"]["derUrl"], json!("http://127.0.0.1:8000/ca.crt"));
        assert_eq!(m["ca"]["spkiFingerprint"], json!(c.ca_spki_fingerprint));
        let env = m["envVars"].as_array().unwrap();
        assert!(
            env.iter()
                .any(|v| v["name"] == "HTTPS_PROXY" && v["value"] == "http://127.0.0.1:8000")
        );
        let tools: Vec<&str> = m["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["id"].as_str().unwrap())
            .collect();
        assert_eq!(
            tools,
            vec![
                "curl", "node", "python", "java", "go", "rust", "ios", "android"
            ]
        );
        let curl = m["tools"][0]["snippet"].as_str().unwrap();
        assert!(
            curl.starts_with("curl --proxy http://127.0.0.1:8000 --cacert '"),
            "{curl}"
        );
        assert!(
            m["tools"][6]["notes"][0]
                .as_str()
                .unwrap()
                .contains("server 192.168.1.20, port 8000")
        );
        assert!(runner.call_lines().is_empty(), "manual setup runs nothing");

        let res = i.activate(&c, json!({})).await.unwrap();
        assert!(res.success && res.metadata["tools"].is_array());
        i.deactivate_all(&c).await.unwrap();
    }

    /// Variables an interactive cmd.exe line of `set` commands joined by `&&` defines (what cmd
    /// does with `set "K=V"` and `set K=V`, including the space before `&&` ending up in the
    /// value).
    fn cmd_vars(line: &str) -> Vec<(String, String)> {
        line.split("&&")
            .enumerate()
            .map(|(i, cmd)| {
                let cmd = if i == 0 {
                    cmd
                } else {
                    cmd.strip_prefix(' ').unwrap()
                };
                let arg = cmd.strip_prefix("set ").unwrap();
                let assignment = match arg.strip_prefix('"') {
                    Some(quoted) => &quoted[..quoted.rfind('"').unwrap()],
                    None => arg,
                };
                let (k, v) = assignment.split_once('=').unwrap();
                (k.to_string(), v.to_string())
            })
            .collect()
    }

    #[test]
    fn windows_snippets_use_set() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = ctx(dir.path());
        c.ca_path = "C:\\Users\\John Doe\\AppData\\Roaming\\akev\\ca.pem".into();
        let tools = tool_instructions(&c, Os::Windows);
        let node = tools[1]["snippet"].as_str().unwrap();
        assert_eq!(tools[1]["shell"], json!("cmd"));
        assert_eq!(
            cmd_vars(node),
            vec![
                (
                    "HTTPS_PROXY".to_string(),
                    "http://127.0.0.1:8000".to_string()
                ),
                (
                    "HTTP_PROXY".to_string(),
                    "http://127.0.0.1:8000".to_string()
                ),
                ("NODE_USE_ENV_PROXY".to_string(), "1".to_string()),
                (
                    "NODE_EXTRA_CA_CERTS".to_string(),
                    "C:\\Users\\John Doe\\AppData\\Roaming\\akev\\ca.pem".to_string()
                ),
            ],
            "{node}"
        );
        for tool in &tools[1..=5] {
            if let Some(snippet) = tool["snippet"].as_str().filter(|s| s.starts_with("set ")) {
                for (k, v) in cmd_vars(snippet) {
                    assert!(
                        !v.contains('"') && !v.ends_with(' '),
                        "{k}={v:?} in {snippet}"
                    );
                }
            }
        }
        let ps = tools[1]["snippetPowerShell"].as_str().unwrap();
        assert!(
            ps.starts_with("$env:HTTPS_PROXY = 'http://127.0.0.1:8000'; "),
            "{ps}"
        );
        assert!(ps.ends_with(
            "$env:NODE_EXTRA_CA_CERTS = 'C:\\Users\\John Doe\\AppData\\Roaming\\akev\\ca.pem'"
        ));
        // Command arguments stay quoted.
        assert!(
            tools[0]["snippet"]
                .as_str()
                .unwrap()
                .contains("--cacert \"C:\\Users\\John Doe\\AppData\\Roaming\\akev\\ca.pem\"")
        );
        // The Rust note is Rust code: an escaped string literal.
        let rust = tools[5]["notes"][1].as_str().unwrap();
        assert!(
            rust.contains(
                "std::fs::read(\"C:\\\\Users\\\\John Doe\\\\AppData\\\\Roaming\\\\akev\\\\ca.pem\")"
            ),
            "{rust}"
        );

        // POSIX shells are unchanged: export lines, paths quoted.
        let posix = tool_instructions(&c, Os::Linux);
        let node = posix[1]["snippet"].as_str().unwrap();
        assert!(node.starts_with("export HTTPS_PROXY=http://127.0.0.1:8000\nexport HTTP_PROXY="));
        assert!(node.contains("export NODE_EXTRA_CA_CERTS='C:\\Users\\John Doe"));
        assert!(posix[1].get("snippetPowerShell").is_none());
    }

    #[tokio::test]
    async fn ios_setup() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = ctx(dir.path());
        c.lan_addresses = vec!["10.0.0.7".into(), "192.168.1.9".into()];
        let info = IosInterceptor.info(&c).await;
        assert!(info.available && info.needs_config);
        let m = &info.metadata;
        assert_eq!(m["setupUrl"], json!("http://10.0.0.7:8000/"));
        assert_eq!(m["qrPayload"], m["setupUrl"]);
        assert_eq!(
            m["mobileconfigUrl"],
            json!("http://10.0.0.7:8000/ca.mobileconfig")
        );
        assert_eq!(m["addresses"].as_array().unwrap().len(), 2);
        assert_eq!(
            m["addresses"][1]["setupUrl"],
            json!("http://192.168.1.9:8000/")
        );
        let steps = m["steps"].as_array().unwrap();
        assert!(
            steps[1]["detail"]
                .as_str()
                .unwrap()
                .contains("Server: 10.0.0.7, Port: 8000")
        );
        assert!(
            steps[3]["detail"]
                .as_str()
                .unwrap()
                .contains("Certificate Trust Settings")
        );
        let res = IosInterceptor.activate(&c, json!({})).await.unwrap();
        assert!(res.message.unwrap().contains("10.0.0.7:8000"));

        c.lan_addresses.clear();
        let info = IosInterceptor.info(&c).await;
        assert!(!info.available && info.unavailable_reason.is_some());
        assert!(matches!(
            IosInterceptor.activate(&c, json!({})).await,
            Err(crate::InterceptError::Unavailable(_))
        ));
    }
}
