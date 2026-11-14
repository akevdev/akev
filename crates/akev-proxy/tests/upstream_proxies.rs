//! Upstream proxy tests: HTTP CONNECT, absolute-form, HTTPS proxies, SOCKS5, bypass lists and
//! PAC, against minimal in-process proxies.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/upstream.rs"]
mod upstream_support;

use akev_core::settings::{DnsOverride, ProxySettings, UpstreamProxy, UpstreamProxyMode};
use akev_core::{ErrorKind, HttpVersion};
use akev_proxy::upstream::RequestBody;
use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use upstream_support::*;

const ALPN_BOTH: &[&[u8]] = &[b"h2", b"http/1.1"];

fn manual(url: String, no_proxy: Vec<String>) -> ProxySettings {
    ProxySettings {
        upstream_proxy: UpstreamProxy {
            mode: UpstreamProxyMode::Manual,
            url,
            no_proxy,
        },
        ..Default::default()
    }
}

fn test_dns(settings: &mut ProxySettings, hosts: &[&str]) {
    settings.dns_overrides = hosts
        .iter()
        .map(|h| DnsOverride {
            host: (*h).into(),
            address: "127.0.0.1".into(),
        })
        .collect();
}

async fn dead_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

#[tokio::test]
async fn http_proxy_uses_connect_for_https_targets() {
    let ca = TestCa::new("CA");
    let dir = tempfile::tempdir().unwrap();
    let (chain, key) = ca.leaf(&["secure.test"], false);
    let (origin, _, _) = spawn_tls(server_config(chain, key, ALPN_BOTH, None)).await;
    let (proxy, seen) = spawn_http_proxy(None).await;
    let mut settings = manual(format!("http://127.0.0.1:{}", proxy.port()), vec![]);
    settings.trusted_ca_paths = vec![write_temp(&dir, "ca.pem", ca.pem().as_bytes())];
    let c = client(&settings);
    let resp = c
        .send(
            &head(
                "GET",
                &format!("https://secure.test:{}/via", origin.port()),
                &[],
            ),
            RequestBody::Empty,
            &opts(),
        )
        .await
        .unwrap();
    // HTTP/2 works through the tunnel.
    assert_eq!(resp.head.http_version, HttpVersion::H2);
    assert_eq!(
        resp.info.via_proxy.as_deref(),
        Some(format!("http://127.0.0.1:{}", proxy.port()).as_str())
    );
    assert_eq!(
        resp.info.remote_addr.as_deref(),
        Some(proxy.to_string().as_str())
    );
    assert_eq!(collect(resp.body).await.0, b"h2 GET /via");
    let reqs = seen.lock().requests.clone();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].method, "CONNECT");
    assert_eq!(reqs[0].target, format!("secure.test:{}", origin.port()));
}

#[tokio::test]
async fn http_proxy_uses_absolute_form_for_http_targets_and_shares_the_connection() {
    let (origin, _) = spawn_http1(|req, _| echo_response(req)).await;
    let (proxy, seen) = spawn_http_proxy(None).await;
    let c = client(&manual(
        format!("http://127.0.0.1:{}", proxy.port()),
        vec![],
    ));
    let first = c
        .send(
            &head(
                "GET",
                &format!("http://a.test:{}/one?x=1", origin.port()),
                &[("Host", "a.test")],
            ),
            RequestBody::Empty,
            &opts(),
        )
        .await
        .unwrap();
    assert!(!first.info.reused);
    let text = String::from_utf8(collect(first.body).await.0).unwrap();
    assert!(
        text.starts_with("GET /one HTTP/1.1\r\nHost: a.test\r\n"),
        "{text}"
    );
    let second = c
        .send(
            &head(
                "GET",
                &format!("http://b.test:{}/two", origin.port()),
                &[("Host", "b.test")],
            ),
            RequestBody::Empty,
            &opts(),
        )
        .await
        .unwrap();
    // Different origin, same proxy connection.
    assert!(second.info.reused);
    collect(second.body).await;
    let reqs = seen.lock().requests.clone();
    assert_eq!(
        reqs[0].target,
        format!("http://a.test:{}/one?x=1", origin.port())
    );
    assert_eq!(
        reqs[1].target,
        format!("http://b.test:{}/two", origin.port())
    );
}

#[tokio::test]
async fn http_proxy_credentials() {
    let (origin, _) = spawn_http1(|req, _| echo_response(req)).await;
    // base64("user:p@ss") = dXNlcjpwQHNz
    let (proxy, seen) = spawn_http_proxy(Some("Basic dXNlcjpwQHNz")).await;
    let c = client(&manual(
        format!("http://user:p%40ss@127.0.0.1:{}", proxy.port()),
        vec![],
    ));
    let resp = c
        .send(
            &head(
                "GET",
                &format!("http://origin.test:{}/", origin.port()),
                &[("Host", "origin.test")],
            ),
            RequestBody::Empty,
            &opts(),
        )
        .await
        .unwrap();
    assert_eq!(resp.head.status, 200);
    collect(resp.body).await;
    assert_eq!(
        seen.lock().requests[0].header("proxy-authorization"),
        Some("Basic dXNlcjpwQHNz")
    );
    // The via_proxy display never contains credentials.
    assert_eq!(
        resp.info.via_proxy.as_deref(),
        Some(format!("http://127.0.0.1:{}", proxy.port()).as_str())
    );

    // Wrong credentials on a CONNECT tunnel → UpstreamProxyFailed.
    let c = client(&manual(
        format!("http://user:wrong@127.0.0.1:{}", proxy.port()),
        vec![],
    ));
    let err = c
        .send(
            &head(
                "GET",
                &format!("https://origin.test:{}/", origin.port()),
                &[],
            ),
            RequestBody::Empty,
            &opts(),
        )
        .await
        .err()
        .unwrap();
    assert_eq!(err.kind, ErrorKind::UpstreamProxyFailed);
    assert_eq!(err.code.as_deref(), Some("PROXY_AUTH_FAILED"));
}

#[tokio::test]
async fn https_proxy_over_tls() {
    let ca = TestCa::new("CA");
    let dir = tempfile::tempdir().unwrap();
    let (proxy_chain, proxy_key) = ca.leaf(&["127.0.0.1"], false);
    let (proxy, seen) =
        spawn_https_proxy(server_config(proxy_chain, proxy_key, &[b"http/1.1"], None)).await;
    let (origin, _) = spawn_http1(|req, _| echo_response(req)).await;
    let mut settings = manual(format!("https://127.0.0.1:{}", proxy.port()), vec![]);
    settings.trusted_ca_paths = vec![write_temp(&dir, "ca.pem", ca.pem().as_bytes())];
    let c = client(&settings);
    let resp = c
        .send(
            &head(
                "GET",
                &format!("http://origin.test:{}/tls-proxy", origin.port()),
                &[("Host", "origin.test")],
            ),
            RequestBody::Empty,
            &opts(),
        )
        .await
        .unwrap();
    assert_eq!(resp.head.status, 200);
    collect(resp.body).await;
    assert_eq!(
        seen.lock().requests[0].target,
        format!("http://origin.test:{}/tls-proxy", origin.port())
    );
    assert_eq!(
        resp.info.via_proxy.as_deref(),
        Some(format!("https://127.0.0.1:{}", proxy.port()).as_str())
    );
}

#[tokio::test]
async fn socks5_proxy_remote_and_local_dns_with_auth() {
    let (origin, _) = spawn_http1(|req, _| echo_response(req)).await;
    let (socks, seen) = spawn_socks5(Some(("alice", "s3cret"))).await;

    // socks5h: the hostname goes to the proxy.
    let c = client(&manual(
        format!("socks5h://alice:s3cret@127.0.0.1:{}", socks.port()),
        vec![],
    ));
    let resp = c
        .send(
            &head(
                "GET",
                &format!("http://named.test:{}/s", origin.port()),
                &[("Host", "named.test")],
            ),
            RequestBody::Empty,
            &opts(),
        )
        .await
        .unwrap();
    assert_eq!(resp.head.status, 200);
    assert_eq!(
        resp.info.via_proxy.as_deref(),
        Some(format!("socks5h://127.0.0.1:{}", socks.port()).as_str())
    );
    collect(resp.body).await;

    // socks5: resolved locally (via a DNS override) and sent as an IP.
    let mut settings = manual(
        format!("socks5://alice:s3cret@127.0.0.1:{}", socks.port()),
        vec![],
    );
    test_dns(&mut settings, &["local.test"]);
    let c = client(&settings);
    let resp = c
        .send(
            &head(
                "GET",
                &format!("http://local.test:{}/s", origin.port()),
                &[("Host", "local.test")],
            ),
            RequestBody::Empty,
            &opts(),
        )
        .await
        .unwrap();
    collect(resp.body).await;

    let seen = seen.lock().clone();
    assert_eq!(
        seen.destinations,
        vec![
            format!("named.test:{}", origin.port()),
            format!("127.0.0.1:{}", origin.port())
        ]
    );
    assert_eq!(seen.users, vec!["alice".to_string(), "alice".to_string()]);

    // Wrong password → proxy failure.
    let c = client(&manual(
        format!("socks5h://alice:nope@127.0.0.1:{}", socks.port()),
        vec![],
    ));
    let err = c
        .send(
            &head("GET", &format!("http://named.test:{}/", origin.port()), &[]),
            RequestBody::Empty,
            &opts(),
        )
        .await
        .err()
        .unwrap();
    assert_eq!(err.kind, ErrorKind::UpstreamProxyFailed);
    assert_eq!(err.code.as_deref(), Some("SOCKS_AUTH_FAILED"));
}

#[tokio::test]
async fn socks5_carries_tls_and_h2() {
    let ca = TestCa::new("CA");
    let dir = tempfile::tempdir().unwrap();
    let (chain, key) = ca.leaf(&["tls.test"], false);
    let (origin, _, seen_tls) = spawn_tls(server_config(chain, key, ALPN_BOTH, None)).await;
    let (socks, _) = spawn_socks5(None).await;
    let mut settings = manual(format!("socks5h://127.0.0.1:{}", socks.port()), vec![]);
    settings.trusted_ca_paths = vec![write_temp(&dir, "ca.pem", ca.pem().as_bytes())];
    let c = client(&settings);
    let resp = c
        .send(
            &head("GET", &format!("https://tls.test:{}/x", origin.port()), &[]),
            RequestBody::Empty,
            &opts(),
        )
        .await
        .unwrap();
    assert_eq!(resp.head.http_version, HttpVersion::H2);
    assert_eq!(collect(resp.body).await.0, b"h2 GET /x");
    assert_eq!(seen_tls.lock()[0].sni.as_deref(), Some("tls.test"));
}

#[tokio::test]
async fn no_proxy_bypass_and_proxy_failures() {
    let (origin, _) = spawn_http1(|req, _| echo_response(req)).await;
    let dead = dead_port().await;
    let mut settings = manual(
        format!("http://127.0.0.1:{dead}"),
        vec!["*.bypass.test".into(), "10.0.0.0/8".into()],
    );
    test_dns(&mut settings, &["api.bypass.test", "proxied.test"]);
    let c = client(&settings);

    let resp = c
        .send(
            &head(
                "GET",
                &format!("http://api.bypass.test:{}/", origin.port()),
                &[],
            ),
            RequestBody::Empty,
            &opts(),
        )
        .await
        .unwrap();
    assert!(resp.info.via_proxy.is_none());
    collect(resp.body).await;

    // Loopback bypasses implicitly.
    let resp = c
        .send(
            &head("GET", &format!("http://127.0.0.1:{}/", origin.port()), &[]),
            RequestBody::Empty,
            &opts(),
        )
        .await
        .unwrap();
    assert!(resp.info.via_proxy.is_none());
    collect(resp.body).await;

    let err = c
        .send(
            &head(
                "GET",
                &format!("http://proxied.test:{}/", origin.port()),
                &[],
            ),
            RequestBody::Empty,
            &opts(),
        )
        .await
        .err()
        .unwrap();
    assert_eq!(err.kind, ErrorKind::UpstreamProxyFailed);
    assert_eq!(err.code.as_deref(), Some("ECONNREFUSED"));
    assert!(
        err.message.contains(&format!("127.0.0.1:{dead}")),
        "{}",
        err.message
    );

    // Invalid proxy URL → descriptive proxy failure (construction still succeeds).
    let c = client(&manual("gopher://x:70".into(), vec![]));
    let err = c
        .send(
            &head("GET", "http://example.test/", &[]),
            RequestBody::Empty,
            &opts(),
        )
        .await
        .err()
        .unwrap();
    assert_eq!(err.kind, ErrorKind::UpstreamProxyFailed);
}

async fn spawn_pac_server(script: String) -> SocketAddr {
    let (addr, _) = spawn_http1(move |_, _| {
        format!("HTTP/1.1 200 OK\r\nContent-Type: application/x-ns-proxy-autoconfig\r\nContent-Length: {}\r\n\r\n{script}", script.len())
            .into_bytes()
    })
    .await;
    addr
}

#[tokio::test]
async fn pac_selects_proxy_direct_and_fails_over() {
    let (origin, _) = spawn_http1(|req, _| echo_response(req)).await;
    let (proxy, seen) = spawn_http_proxy(None).await;
    let dead = dead_port().await;
    let script = format!(
        r#"function FindProxyForURL(url, host) {{
            if (dnsDomainIs(host, ".direct.test")) return "DIRECT";
            if (shExpMatch(host, "failover.*")) return "PROXY 127.0.0.1:{dead}; PROXY 127.0.0.1:{}";
            return "PROXY 127.0.0.1:{}";
        }}"#,
        proxy.port(),
        proxy.port()
    );
    let pac = spawn_pac_server(script).await;
    let mut settings = ProxySettings {
        upstream_proxy: UpstreamProxy {
            mode: UpstreamProxyMode::Pac,
            url: format!("http://127.0.0.1:{}/proxy.pac", pac.port()),
            no_proxy: vec![],
        },
        ..Default::default()
    };
    test_dns(&mut settings, &["www.direct.test"]);
    let c = client(&settings);

    let resp = c
        .send(
            &head(
                "GET",
                &format!("http://via.test:{}/p", origin.port()),
                &[("Host", "via.test")],
            ),
            RequestBody::Empty,
            &opts(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.info.via_proxy.as_deref(),
        Some(format!("http://127.0.0.1:{}", proxy.port()).as_str())
    );
    collect(resp.body).await;

    let resp = c
        .send(
            &head(
                "GET",
                &format!("http://www.direct.test:{}/d", origin.port()),
                &[],
            ),
            RequestBody::Empty,
            &opts(),
        )
        .await
        .unwrap();
    assert!(resp.info.via_proxy.is_none());
    collect(resp.body).await;

    let resp = c
        .send(
            &head(
                "GET",
                &format!("http://failover.test:{}/f", origin.port()),
                &[("Host", "failover.test")],
            ),
            RequestBody::Empty,
            &opts(),
        )
        .await
        .unwrap();
    assert_eq!(resp.head.status, 200);
    assert_eq!(
        resp.info.via_proxy.as_deref(),
        Some(format!("http://127.0.0.1:{}", proxy.port()).as_str())
    );
    collect(resp.body).await;

    let targets: Vec<String> = seen
        .lock()
        .requests
        .iter()
        .map(|r| r.target.clone())
        .collect();
    assert_eq!(
        targets,
        vec![
            format!("http://via.test:{}/p", origin.port()),
            format!("http://failover.test:{}/f", origin.port())
        ]
    );
}

#[tokio::test]
async fn pac_from_file_url_and_unavailable_pac_goes_direct() {
    let (origin, _) = spawn_http1(|req, _| echo_response(req)).await;
    let (socks, seen) = spawn_socks5(None).await;
    let dir = tempfile::tempdir().unwrap();
    let path = write_temp(
        &dir,
        "proxy.pac",
        format!(
            "function FindProxyForURL(url, host) {{ return 'SOCKS5 127.0.0.1:{}'; }}",
            socks.port()
        )
        .as_bytes(),
    );
    let c = client(&ProxySettings {
        upstream_proxy: UpstreamProxy {
            mode: UpstreamProxyMode::Pac,
            url: url::Url::from_file_path(&path).unwrap().to_string(),
            no_proxy: vec![],
        },
        ..Default::default()
    });
    let resp = c
        .send(
            &head(
                "GET",
                &format!("http://pac-file.test:{}/", origin.port()),
                &[],
            ),
            RequestBody::Empty,
            &opts(),
        )
        .await
        .unwrap();
    assert_eq!(resp.head.status, 200);
    collect(resp.body).await;
    assert_eq!(
        seen.lock().destinations,
        vec![format!("pac-file.test:{}", origin.port())]
    );

    // PAC server down → DIRECT (browser behaviour).
    let dead = dead_port().await;
    let c = client(&ProxySettings {
        upstream_proxy: UpstreamProxy {
            mode: UpstreamProxyMode::Pac,
            url: format!("http://127.0.0.1:{dead}/proxy.pac"),
            no_proxy: vec![],
        },
        ..Default::default()
    });
    let resp = c
        .send(
            &head("GET", &format!("http://127.0.0.1:{}/", origin.port()), &[]),
            RequestBody::Empty,
            &opts(),
        )
        .await
        .unwrap();
    assert!(resp.info.via_proxy.is_none());
    collect(resp.body).await;
}

#[tokio::test]
async fn websocket_through_http_proxy_uses_connect() {
    let (origin, _) = spawn_server(|mut s, _| async move {
        let mut buf = bytes::BytesMut::new();
        read_request(&mut s, &mut buf).await.unwrap();
        s.write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\nX").await.unwrap();
    })
    .await;
    let (proxy, seen) = spawn_http_proxy(None).await;
    let c = client(&manual(
        format!("http://127.0.0.1:{}", proxy.port()),
        vec![],
    ));
    let h = head(
        "GET",
        &format!("ws://ws.test:{}/", origin.port()),
        &[
            ("Host", "ws.test"),
            ("Upgrade", "websocket"),
            ("Connection", "Upgrade"),
        ],
    );
    let (resp_head, upgraded, _, info) = c.send_upgrade(&h, &opts()).await.unwrap();
    assert_eq!(resp_head.status, 101);
    assert_eq!(&upgraded.unwrap().leftover[..], b"X");
    assert!(info.via_proxy.is_some());
    assert_eq!(seen.lock().requests[0].method, "CONNECT");
}

#[tokio::test]
async fn connect_raw_through_proxies() {
    let (origin, _) = spawn_server(|mut s, _| async move {
        let mut b = [0u8; 4];
        s.read_exact(&mut b).await.unwrap();
        s.write_all(&b).await.unwrap();
    })
    .await;
    let (proxy, seen) = spawn_http_proxy(None).await;
    let c = client(&manual(
        format!("http://127.0.0.1:{}", proxy.port()),
        vec![],
    ));
    let (mut io, info, _) = c.connect_raw("raw.test", origin.port()).await.unwrap();
    assert!(info.via_proxy.is_some());
    io.write_all(b"ping").await.unwrap();
    let mut back = [0u8; 4];
    io.read_exact(&mut back).await.unwrap();
    assert_eq!(&back, b"ping");
    assert_eq!(
        seen.lock().requests[0].target,
        format!("raw.test:{}", origin.port())
    );

    let (socks, socks_seen) = spawn_socks5(None).await;
    let c = client(&manual(
        format!("socks5h://127.0.0.1:{}", socks.port()),
        vec![],
    ));
    let (mut io, _, _) = c.connect_raw("raw.test", origin.port()).await.unwrap();
    io.write_all(b"pong").await.unwrap();
    io.read_exact(&mut back).await.unwrap();
    assert_eq!(&back, b"pong");
    assert_eq!(
        socks_seen.lock().destinations,
        vec![format!("raw.test:{}", origin.port())]
    );
}
