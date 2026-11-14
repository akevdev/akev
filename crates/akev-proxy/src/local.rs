//! Locally served pages: requests to the magic host `akev.cert` (through the proxy) or
//! addressed directly to the proxy port get a self-contained, mobile-friendly setup page with
//! CA certificate downloads (PEM, DER, iOS configuration profile), a PAC file and
//! per-platform instructions.

use akev_ca::CertificateAuthority;
use akev_core::{Headers, HttpVersion, ProxyMode, RequestHead, ResponseHead, canonical_reason};
use base64::Engine;
use bytes::Bytes;
use sha2::{Digest, Sha256};
use std::net::{IpAddr, SocketAddr};

/// Magic hostname that always reaches the setup page through the proxy.
pub const MAGIC_HOST: &str = "akev.cert";

/// Whether a request should be answered by the proxy itself instead of being forwarded.
///
/// * any request for `akev.cert`;
/// * direct (origin-form, untunnelled) requests whose `Host` port is the proxy's own port;
/// * proxy/tunnel requests whose destination is the proxy's own address (prevents loops).
pub fn is_local_target(host: &str, port: u16, mode: ProxyMode, local: SocketAddr) -> bool {
    let host = host.trim_end_matches('.');
    if host.eq_ignore_ascii_case(MAGIC_HOST) {
        return true;
    }
    if port != local.port() {
        return false;
    }
    mode == ProxyMode::Direct || is_self_host(host, local)
}

fn is_self_host(host: &str, local: SocketAddr) -> bool {
    if host.eq_ignore_ascii_case("localhost") || host.to_ascii_lowercase().ends_with(".localhost") {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(ip) => ip.is_loopback() || ip.is_unspecified() || canonical(ip) == canonical(local.ip()),
        Err(_) => false,
    }
}

/// `host:port` to show users for a socket address (IPv4-mapped IPv6 shown as plain IPv4, as
/// seen on dual-stack `::` listeners).
pub fn display_addr(addr: SocketAddr) -> String {
    SocketAddr::new(canonical(addr.ip()), addr.port()).to_string()
}

fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(v6)),
        v4 => v4,
    }
}

use akev_core::time::html_escape;

/// A deterministic UUID derived from `seed` (so re-downloading the same CA gives the same
/// iOS profile identity instead of duplicate profiles).
fn uuid_from(seed: &[u8], salt: &str) -> String {
    let mut h = Sha256::new();
    h.update(salt.as_bytes());
    h.update(seed);
    let d = h.finalize();
    let mut b = [0u8; 16];
    b.copy_from_slice(&d[..16]);
    b[6] = (b[6] & 0x0f) | 0x50; // version 5 style
    b[8] = (b[8] & 0x3f) | 0x80; // RFC 4122 variant
    let x = hex::encode_upper(b);
    format!(
        "{}-{}-{}-{}-{}",
        &x[..8],
        &x[8..12],
        &x[12..16],
        &x[16..20],
        &x[20..]
    )
}

/// iOS / macOS configuration profile installing the CA as a trusted root.
pub fn mobileconfig(ca: &CertificateAuthority) -> String {
    let der = ca.cert_der();
    let b64 = base64::engine::general_purpose::STANDARD.encode(der);
    let wrapped: Vec<&str> = b64
        .as_bytes()
        .chunks(64)
        .filter_map(|c| std::str::from_utf8(c).ok())
        .collect();
    let cert_uuid = uuid_from(der, "akev-ca-cert");
    let profile_uuid = uuid_from(der, "akev-ca-profile");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>PayloadContent</key>
	<array>
		<dict>
			<key>PayloadCertificateFileName</key>
			<string>akev-ca.cer</string>
			<key>PayloadContent</key>
			<data>
			{data}
			</data>
			<key>PayloadDescription</key>
			<string>Adds the Akev CA root certificate.</string>
			<key>PayloadDisplayName</key>
			<string>Akev CA</string>
			<key>PayloadIdentifier</key>
			<string>dev.akev.ca.cert.{cert_uuid}</string>
			<key>PayloadType</key>
			<string>com.apple.security.root</string>
			<key>PayloadUUID</key>
			<string>{cert_uuid}</string>
			<key>PayloadVersion</key>
			<integer>1</integer>
		</dict>
	</array>
	<key>PayloadDescription</key>
	<string>Installs the Akev certificate authority so HTTPS traffic from this device can be inspected with Akev. Remove this profile when you are done debugging.</string>
	<key>PayloadDisplayName</key>
	<string>Akev CA</string>
	<key>PayloadIdentifier</key>
	<string>dev.akev.ca.{profile_uuid}</string>
	<key>PayloadOrganization</key>
	<string>Akev</string>
	<key>PayloadRemovalDisallowed</key>
	<false/>
	<key>PayloadType</key>
	<string>Configuration</string>
	<key>PayloadUUID</key>
	<string>{profile_uuid}</string>
	<key>PayloadVersion</key>
	<integer>1</integer>
</dict>
</plist>
"#,
        data = wrapped.join("\n\t\t\t"),
    )
}

/// Proxy auto-config script pointing at this proxy (falls back to DIRECT if it's down).
pub fn pac(proxy_addr: &str) -> String {
    format!(
        "// Akev proxy auto-configuration\nfunction FindProxyForURL(url, host) {{\n  return \"PROXY {proxy_addr}; DIRECT\";\n}}\n"
    )
}

fn response(
    status: u16,
    content_type: &str,
    body: Bytes,
    extra: &[(&str, &str)],
) -> (ResponseHead, Bytes) {
    let mut headers = Headers::with_capacity(4 + extra.len());
    headers.append("Content-Type", content_type);
    headers.append("Content-Length", body.len().to_string());
    headers.append("Cache-Control", "no-store");
    for (k, v) in extra {
        headers.append(*k, *v);
    }
    let head = ResponseHead {
        status,
        status_message: canonical_reason(status).to_string(),
        http_version: HttpVersion::Http11,
        headers,
    };
    (head, body)
}

/// Answer a local request. `proxy_addr` is the `host:port` clients should use for the proxy.
pub fn respond(
    ca: &CertificateAuthority,
    request: &RequestHead,
    proxy_addr: &str,
) -> (ResponseHead, Bytes) {
    let path = request.path_only();
    match path {
        "/" | "/index.html" | "/setup" => {
            let via_proxy = request
                .host
                .trim_end_matches('.')
                .eq_ignore_ascii_case(MAGIC_HOST);
            let page = setup_page(ca, proxy_addr, via_proxy, request.is_tls());
            response(200, "text/html; charset=utf-8", Bytes::from(page), &[])
        }
        "/ca.pem" => response(
            200,
            "application/x-pem-file",
            Bytes::copy_from_slice(ca.cert_pem().as_bytes()),
            &[(
                "Content-Disposition",
                "attachment; filename=\"akev-ca.pem\"",
            )],
        ),
        // Served inline: Firefox and Android offer to trust a CA served with this type.
        "/ca.crt" | "/ca.cer" | "/ca.der" => response(
            200,
            "application/x-x509-ca-cert",
            Bytes::copy_from_slice(ca.cert_der()),
            &[],
        ),
        "/ca.mobileconfig" => response(
            200,
            "application/x-apple-aspen-config",
            Bytes::from(mobileconfig(ca)),
            &[(
                "Content-Disposition",
                "attachment; filename=\"akev-ca.mobileconfig\"",
            )],
        ),
        "/proxy.pac" | "/wpad.dat" => response(
            200,
            "application/x-ns-proxy-autoconfig",
            Bytes::from(pac(proxy_addr)),
            &[],
        ),
        "/favicon.ico" | "/favicon.svg" => response(
            200,
            "image/svg+xml",
            Bytes::from_static(LOGO_SVG.as_bytes()),
            &[],
        ),
        _ => {
            let body = format!(
                "<!doctype html><meta charset=utf-8><meta name=viewport content=\"width=device-width,initial-scale=1\"><title>Not found &middot; Akev</title>\
                 <body style=\"background:#0d0d12;color:#e7e7ee;font:16px system-ui,sans-serif;display:grid;place-items:center;min-height:100vh;margin:0\">\
                 <p>Nothing at <code>{}</code>. <a style=\"color:#a48bff\" href=\"/\">Open the Akev setup page</a></p>",
                html_escape(path)
            );
            response(404, "text/html; charset=utf-8", Bytes::from(body), &[])
        }
    }
}

const LOGO_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><rect x=".5" y=".5" width="63" height="63" rx="15.5" fill="#15161c" stroke="#fff" stroke-opacity=".12"/><g transform="translate(-4 -4) scale(.0703125)"><path d="M512 280 748 744H656L512 460.9 368 744H276Z" fill="#fff" stroke="#fff" stroke-width="24" stroke-linejoin="round"/><circle cx="512" cy="684" r="62" fill="#8a70ff"/></g></svg>"##;

fn setup_page(
    ca: &CertificateAuthority,
    proxy_addr: &str,
    via_proxy: bool,
    secure: bool,
) -> String {
    let info = ca.info();
    let (proxy_host, proxy_port) = match proxy_addr.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => (
            h.trim_matches(|c| c == '[' || c == ']').to_string(),
            p.to_string(),
        ),
        _ => (proxy_addr.to_string(), String::new()),
    };
    let status = if via_proxy {
        let note = if secure {
            "over HTTPS, so the Akev CA is already trusted here"
        } else {
            "over plain HTTP"
        };
        format!(
            "<div class=\"status ok\"><span class=\"dot\"></span>This device is connected through Akev ({note}).</div>"
        )
    } else {
        "<div class=\"status warn\"><span class=\"dot\"></span>You reached Akev directly. Configure the proxy below, then open <b>http://akev.cert</b> to check.</div>".to_string()
    };
    let pac_url = format!("http://{proxy_addr}/proxy.pac");
    format!(
        r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1,viewport-fit=cover">
<meta name="color-scheme" content="dark">
<meta name="theme-color" content="#0d0d12">
<title>Akev &middot; Device setup</title>
<link rel="icon" href="/favicon.svg" type="image/svg+xml">
<style>
:root{{--bg:#0d0d12;--panel:#15151d;--panel2:#1c1c27;--line:#262634;--text:#e7e7ee;--muted:#9a9aae;--accent:#7c5cff;--accent2:#a48bff;--ok:#3ecf8e;--warn:#f5b84b}}
*{{box-sizing:border-box}}
html{{-webkit-text-size-adjust:100%}}
body{{margin:0;background:radial-gradient(1200px 600px at 10% -10%,#7c5cff22,transparent 60%),var(--bg);color:var(--text);font:16px/1.55 system-ui,-apple-system,"Segoe UI",Roboto,"Helvetica Neue",sans-serif;min-height:100vh}}
main{{max-width:760px;margin:0 auto;padding:28px 16px 48px}}
header{{display:flex;align-items:center;gap:14px;margin-bottom:22px}}
header svg{{width:44px;height:44px;flex:none;filter:drop-shadow(0 6px 16px #0008)}}
h1{{font-size:24px;margin:0;letter-spacing:-.01em}}
header p{{margin:2px 0 0;color:var(--muted);font-size:14px}}
.status{{display:flex;gap:10px;align-items:flex-start;padding:12px 14px;border-radius:12px;font-size:14px;margin-bottom:18px;border:1px solid var(--line);background:var(--panel)}}
.status .dot{{width:9px;height:9px;border-radius:50%;margin-top:6px;flex:none}}
.ok .dot{{background:var(--ok);box-shadow:0 0 0 4px #3ecf8e22}} .warn .dot{{background:var(--warn);box-shadow:0 0 0 4px #f5b84b22}}
section{{background:var(--panel);border:1px solid var(--line);border-radius:16px;padding:18px;margin-bottom:14px}}
h2{{font-size:13px;text-transform:uppercase;letter-spacing:.08em;color:var(--accent2);margin:0 0 12px;display:flex;align-items:center;gap:8px}}
h2 .n{{display:inline-grid;place-items:center;width:22px;height:22px;border-radius:7px;background:#7c5cff26;color:var(--accent2);font-size:12px}}
.grid{{display:grid;grid-template-columns:repeat(auto-fit,minmax(150px,1fr));gap:10px}}
.kv{{background:var(--panel2);border-radius:10px;padding:10px 12px}}
.kv span{{display:block;font-size:12px;color:var(--muted)}}
code,.mono{{font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;font-size:14px;word-break:break-all}}
.btns{{display:flex;flex-wrap:wrap;gap:10px}}
a.btn{{flex:1 1 180px;display:flex;flex-direction:column;gap:2px;text-decoration:none;color:var(--text);background:var(--panel2);border:1px solid var(--line);border-radius:12px;padding:12px 14px;transition:border-color .15s,transform .15s}}
a.btn:hover,a.btn:focus-visible{{border-color:var(--accent);transform:translateY(-1px);outline:none}}
a.btn.primary{{background:linear-gradient(135deg,#7c5cff,#5b3df5);border-color:transparent}}
a.btn b{{font-size:15px}} a.btn small{{color:#d6d0ff;opacity:.85;font-size:12px}}
a.btn:not(.primary) small{{color:var(--muted)}}
details{{border-top:1px solid var(--line);padding:12px 0}}
details:first-of-type{{border-top:0;padding-top:0}}
summary{{cursor:pointer;font-weight:600;list-style:none;display:flex;justify-content:space-between;align-items:center}}
summary::-webkit-details-marker{{display:none}}
summary::after{{content:"+";color:var(--accent2);font-size:20px;line-height:1}}
details[open] summary::after{{content:"\2212"}}
ol{{margin:10px 0 0;padding-left:20px;color:#cfcfe0}} li{{margin:4px 0}}
.muted{{color:var(--muted);font-size:13px}}
footer{{text-align:center;color:var(--muted);font-size:12px;margin-top:22px}}
footer a{{color:var(--accent2)}}
</style>
</head>
<body>
<main>
<header>{logo}<div><h1>Set up this device</h1><p>Route traffic through Akev and trust its certificate to inspect HTTPS.</p></div></header>
{status}
<section>
<h2><span class="n">1</span>Proxy settings</h2>
<div class="grid">
<div class="kv"><span>Server / host</span><code>{proxy_host}</code></div>
<div class="kv"><span>Port</span><code>{proxy_port}</code></div>
<div class="kv"><span>Auto-config (PAC) URL</span><code>{pac_url}</code></div>
</div>
<p class="muted">Use these values as a manual HTTP proxy in your Wi-Fi or system network settings. SOCKS5 works on the same port.</p>
</section>
<section>
<h2><span class="n">2</span>Install the certificate</h2>
<div class="btns">
<a class="btn primary" href="/ca.mobileconfig"><b>iPhone / iPad / Mac</b><small>Configuration profile</small></a>
<a class="btn" href="/ca.crt"><b>Android / Firefox</b><small>DER certificate (.crt)</small></a>
<a class="btn" href="/ca.pem"><b>Other</b><small>PEM certificate (.pem)</small></a>
</div>
</section>
<section>
<h2><span class="n">3</span>Trust it</h2>
<details open><summary>iOS &amp; iPadOS</summary><ol>
<li>Open this page in <b>Safari</b> and tap <b>iPhone / iPad / Mac</b>, then <b>Allow</b>.</li>
<li>Open <b>Settings &rarr; Profile Downloaded</b> and tap <b>Install</b>.</li>
<li>Go to <b>Settings &rarr; General &rarr; About &rarr; Certificate Trust Settings</b> and enable full trust for <b>{subject}</b>.</li>
</ol></details>
<details><summary>Android</summary><ol>
<li>Tap <b>Android / Firefox</b> to download the certificate.</li>
<li>Open <b>Settings &rarr; Security &amp; privacy &rarr; More security settings &rarr; Encryption &amp; credentials &rarr; Install a certificate &rarr; CA certificate</b> and pick <code>ca.crt</code>.</li>
<li>Since Android 7, apps only trust user CAs if they opt in (<code>network_security_config</code>); browsers like Chrome do. Rooted devices and emulators can be intercepted fully with Akev's Android interceptor.</li>
</ol></details>
<details><summary>macOS</summary><ol>
<li>Download the configuration profile (or the PEM) and open it.</li>
<li>Install it from <b>System Settings &rarr; Privacy &amp; Security &rarr; Profiles</b>, or import the PEM into <b>Keychain Access &rarr; System</b>.</li>
<li>Double-click <b>{subject}</b> and set <b>When using this certificate</b> to <b>Always Trust</b>.</li>
</ol></details>
<details><summary>Windows</summary><ol>
<li>Download <b>ca.crt</b> and open it, then choose <b>Install Certificate</b>.</li>
<li>Pick <b>Local Machine</b> (or Current User) and place it in <b>Trusted Root Certification Authorities</b>.</li>
</ol></details>
<details><summary>Linux</summary><ol>
<li>Debian / Ubuntu: <code>sudo cp akev-ca.pem /usr/local/share/ca-certificates/akev-ca.crt &amp;&amp; sudo update-ca-certificates</code></li>
<li>Fedora / RHEL / Arch: <code>sudo trust anchor akev-ca.pem</code></li>
<li>Chrome and Firefox keep their own stores: import the PEM under <b>Certificates &rarr; Authorities</b>.</li>
</ol></details>
<details><summary>Firefox (any platform)</summary><ol>
<li>Tap <b>Android / Firefox</b>; Firefox offers to trust the CA. Tick <b>Trust this CA to identify websites</b>.</li>
</ol></details>
</section>
<section>
<h2>Certificate</h2>
<div class="grid">
<div class="kv"><span>Subject</span><span class="mono" style="color:var(--text)">{subject}</span></div>
<div class="kv"><span>Valid until</span><span class="mono" style="color:var(--text)">{not_after} ({days} days)</span></div>
</div>
<div class="kv" style="margin-top:10px"><span>SHA-256 fingerprint</span><code>{fingerprint}</code></div>
<p class="muted">Only install this certificate on devices you control, and remove it when you're done. Anyone holding Akev's CA key could intercept this device's HTTPS traffic.</p>
</section>
<footer>Akev {version} &middot; <a href="http://{magic}/">{magic}</a></footer>
</main>
</body>
</html>
"##,
        logo = LOGO_SVG,
        status = status,
        proxy_host = html_escape(&proxy_host),
        proxy_port = html_escape(&proxy_port),
        pac_url = html_escape(&pac_url),
        subject = html_escape(&info.subject),
        not_after = html_escape(&info.not_after),
        days = info.days_remaining,
        fingerprint = html_escape(&info.sha256_fingerprint),
        version = akev_core::VERSION,
        magic = MAGIC_HOST,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_target_detection() {
        let local: SocketAddr = "192.168.1.5:8000".parse().unwrap();
        assert!(is_local_target(
            "akev.cert",
            80,
            ProxyMode::HttpProxy,
            local
        ));
        assert!(is_local_target(
            "AKEV.CERT.",
            443,
            ProxyMode::Connect,
            local
        ));
        assert!(is_local_target(
            "anything.example",
            8000,
            ProxyMode::Direct,
            local
        ));
        assert!(!is_local_target(
            "example.com",
            80,
            ProxyMode::Direct,
            local
        ));
        assert!(is_local_target(
            "192.168.1.5",
            8000,
            ProxyMode::HttpProxy,
            local
        ));
        assert!(is_local_target("127.0.0.1", 8000, ProxyMode::Socks, local));
        assert!(is_local_target(
            "localhost",
            8000,
            ProxyMode::HttpProxy,
            local
        ));
        assert!(!is_local_target(
            "example.com",
            8000,
            ProxyMode::HttpProxy,
            local
        ));
        assert!(!is_local_target(
            "127.0.0.1",
            8001,
            ProxyMode::HttpProxy,
            local
        ));
        let mapped: SocketAddr = "[::ffff:10.0.0.2]:8000".parse().unwrap();
        assert!(is_local_target(
            "10.0.0.2",
            8000,
            ProxyMode::Connect,
            mapped
        ));
    }

    fn req(host: &str, path: &str, scheme: &str) -> RequestHead {
        RequestHead {
            method: "GET".into(),
            url: format!("{scheme}://{host}{path}"),
            scheme: scheme.into(),
            host: host.into(),
            port: if scheme == "https" { 443 } else { 80 },
            path: path.into(),
            http_version: HttpVersion::Http11,
            headers: Headers::new(),
        }
    }

    fn body_text(b: &Bytes) -> &str {
        std::str::from_utf8(b).unwrap()
    }

    #[test]
    fn setup_routes() {
        let ca = CertificateAuthority::generate().unwrap();
        let addr = "192.168.1.5:8000";

        let (h, b) = respond(&ca, &req(MAGIC_HOST, "/", "http"), addr);
        assert_eq!((h.status, h.status_message.as_str()), (200, "OK"));
        assert_eq!(
            h.headers.get("content-type"),
            Some("text/html; charset=utf-8")
        );
        assert_eq!(h.headers.content_length(), Some(b.len() as u64));
        let page = body_text(&b);
        for needle in [
            "<code>192.168.1.5</code>",
            "<code>8000</code>",
            "/ca.mobileconfig",
            "/ca.crt",
            "/ca.pem",
            "#7c5cff",
            "width=device-width",
            "http://192.168.1.5:8000/proxy.pac",
        ] {
            assert!(page.contains(needle), "setup page lacks {needle}");
        }
        assert!(page.contains("connected through Akev"));
        // Reached directly (not through the proxy): tells the user to configure it first.
        let (_, b) = respond(&ca, &req("192.168.1.5", "/index.html", "http"), addr);
        assert!(body_text(&b).contains("reached Akev directly"));

        let (h, b) = respond(&ca, &req(MAGIC_HOST, "/ca.pem?download=1", "http"), addr);
        assert_eq!(
            h.headers.get("content-type"),
            Some("application/x-pem-file")
        );
        assert_eq!(body_text(&b), ca.cert_pem());
        assert!(
            h.headers
                .get("content-disposition")
                .unwrap()
                .contains("akev-ca.pem")
        );

        let (h, b) = respond(&ca, &req(MAGIC_HOST, "/ca.crt", "https"), addr);
        assert_eq!(
            h.headers.get("content-type"),
            Some("application/x-x509-ca-cert")
        );
        assert_eq!(&b[..], ca.cert_der());
        // Served inline so Firefox / Android offer to trust it.
        assert!(!h.headers.contains("content-disposition"));

        let (h, b) = respond(&ca, &req(MAGIC_HOST, "/ca.mobileconfig", "http"), addr);
        assert_eq!(
            h.headers.get("content-type"),
            Some("application/x-apple-aspen-config")
        );
        let profile = body_text(&b);
        let b64 = base64::engine::general_purpose::STANDARD.encode(ca.cert_der());
        assert!(
            profile.contains(&b64[..64]),
            "profile embeds the CA certificate"
        );
        assert!(profile.contains("<string>com.apple.security.root</string>"));
        assert!(profile.starts_with("<?xml") && profile.trim_end().ends_with("</plist>"));
        // Re-downloading yields the same profile identity (no duplicate profiles on iOS).
        assert_eq!(
            b,
            respond(&ca, &req(MAGIC_HOST, "/ca.mobileconfig", "http"), addr).1
        );

        let (h, b) = respond(&ca, &req(MAGIC_HOST, "/proxy.pac", "http"), addr);
        assert_eq!(
            h.headers.get("content-type"),
            Some("application/x-ns-proxy-autoconfig")
        );
        assert!(body_text(&b).contains("PROXY 192.168.1.5:8000; DIRECT"));

        let (h, b) = respond(&ca, &req(MAGIC_HOST, "/<script>x", "http"), addr);
        assert_eq!(h.status, 404);
        assert!(body_text(&b).contains("&lt;script&gt;") && !body_text(&b).contains("<script>"));
    }

    #[test]
    fn display_addr_unmaps_ipv4() {
        assert_eq!(
            display_addr("[::ffff:192.168.1.5]:8000".parse().unwrap()),
            "192.168.1.5:8000"
        );
        assert_eq!(display_addr("[::1]:8000".parse().unwrap()), "[::1]:8000");
        assert_eq!(display_addr("10.0.0.1:1".parse().unwrap()), "10.0.0.1:1");
    }

    #[test]
    fn deterministic_uuids_and_pac() {
        let a = uuid_from(b"cert", "x");
        assert_eq!(a, uuid_from(b"cert", "x"));
        assert_ne!(a, uuid_from(b"cert", "y"));
        assert_eq!(a.len(), 36);
        assert_eq!(&a[14..15], "5");
        assert!(pac("10.0.0.1:8000").contains("PROXY 10.0.0.1:8000; DIRECT"));
    }
}
