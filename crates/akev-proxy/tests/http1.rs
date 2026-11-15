//! End-to-end: plain HTTP/1 proxying (absolute-form and transparent), header fidelity,
//! framing, pipelining, error responses, SSE, capture limits and the local setup page.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use akev_core::settings::CaptureSettings;
use akev_core::*;
use common::*;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn no_rules() -> akev_core::rules::RuleSet {
    akev_core::rules::RuleSet::default()
}

#[tokio::test]
async fn proxies_http_and_emits_events_in_causal_order() {
    let upstream = spawn_http_upstream().await;
    let mut proxy = start_proxy(no_rules(), Arc::new(NoBreakpoints)).await;
    let res = client(&proxy)
        .get(format!("http://{upstream}/hello?x=1"))
        .send()
        .await
        .expect("request");
    assert_eq!(res.status(), 200);
    assert_eq!(
        res.headers()
            .get("x-upstream")
            .and_then(|v| v.to_str().ok()),
        Some("test")
    );
    assert_eq!(res.text().await.unwrap(), "hello");

    let (_, events) = wait_exchange(&mut proxy, "/hello?x=1", is_completed).await;
    let names: Vec<_> = events.iter().map(event_name).collect();
    assert_eq!(
        names,
        [
            "RequestStarted",
            "RequestCompleted",
            "Upstream",
            "ResponseStarted",
            "ResponseCompleted"
        ]
    );
    match &events[0] {
        ProxyEvent::RequestStarted {
            head,
            client,
            kind,
            timing,
            ..
        } => {
            assert_eq!(*kind, ExchangeKind::Http);
            assert_eq!(head.method, "GET");
            assert_eq!(head.url, format!("http://{upstream}/hello?x=1"));
            assert_eq!(
                (head.scheme.as_str(), head.host.as_str(), head.port),
                ("http", "127.0.0.1", upstream.port())
            );
            assert_eq!(head.http_version, HttpVersion::Http11);
            assert_eq!(client.via, ProxyMode::HttpProxy);
            assert!(client.tls.is_none());
            assert!(timing.request_headers.is_some());
        }
        other => panic!("unexpected {other:?}"),
    }
    match &events[3] {
        ProxyEvent::ResponseStarted { head, .. } => {
            assert_eq!((head.status, head.status_message.as_str()), (200, "OK"));
            // Upstream header casing is preserved.
            assert_eq!(
                head.headers
                    .0
                    .iter()
                    .find(|(k, _)| k == "X-Upstream")
                    .map(|(_, v)| v.as_str()),
                Some("test")
            );
        }
        other => panic!("unexpected {other:?}"),
    }
    match &events[4] {
        ProxyEvent::ResponseCompleted { body, .. } => {
            assert_eq!(&body.bytes[..], b"hello");
            assert_eq!(body.wire_size, 5);
            assert!(!body.truncated);
        }
        other => panic!("unexpected {other:?}"),
    }
    let stats = proxy.handle.stats();
    assert!(
        stats.total_connections >= 1 && stats.bytes_in > 0 && stats.bytes_out > 0,
        "{stats:?}"
    );
}

#[tokio::test]
async fn preserves_header_casing_order_and_duplicates() {
    let upstream = spawn_http_upstream().await;
    let mut proxy = start_proxy(no_rules(), Arc::new(NoBreakpoints)).await;
    let request = format!(
        "GET http://{upstream}/echo HTTP/1.1\r\nHost: {upstream}\r\nX-MiXeD-Case: 1\r\nx-dup: a\r\nAccept: */*\r\nX-Dup: b\r\n\
         Proxy-Connection: keep-alive\r\nCookie: a=1\r\nCookie: b=2\r\n\r\n"
    );
    let (head, body) = raw_exchange(&proxy, request.as_bytes()).await;
    assert!(head.starts_with("HTTP/1.1 200 OK\r\n"), "{head}");
    let received = String::from_utf8_lossy(&body);
    let expected = format!(
        "GET /echo HTTP/1.1\r\nHost: {upstream}\r\nX-MiXeD-Case: 1\r\nx-dup: a\r\nAccept: */*\r\nX-Dup: b\r\nCookie: a=1\r\nCookie: b=2\r\n\r\n"
    );
    assert!(
        received.starts_with(&expected),
        "upstream received:\n{received}"
    );
    assert!(!received.contains("Proxy-Connection"));

    let (_, events) = wait_exchange(&mut proxy, "/echo", is_completed).await;
    let ProxyEvent::RequestStarted { head, .. } = &events[0] else {
        panic!()
    };
    let names: Vec<&str> = head.headers.iter().map(|(k, _)| k).collect();
    assert_eq!(
        names,
        [
            "Host",
            "X-MiXeD-Case",
            "x-dup",
            "Accept",
            "X-Dup",
            "Cookie",
            "Cookie"
        ]
    );
}

#[tokio::test]
async fn chunked_request_with_trailers_then_pipelined_request() {
    let upstream = spawn_http_upstream().await;
    let mut proxy = start_proxy(no_rules(), Arc::new(NoBreakpoints)).await;
    let mut tcp = tokio::net::TcpStream::connect(proxy.addr()).await.unwrap();
    let pipelined = format!(
        "POST http://{upstream}/echo HTTP/1.1\r\nHost: {upstream}\r\nTransfer-Encoding: chunked\r\n\r\n\
         4\r\nWiki\r\n5;ext=1\r\npedia\r\n0\r\nX-Trailer: t1\r\n\r\n\
         GET http://{upstream}/hello HTTP/1.1\r\nHost: {upstream}\r\n\r\n"
    );
    tcp.write_all(pipelined.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    let (head1, body1) = read_response(&mut tcp, &mut buf).await;
    assert!(head1.starts_with("HTTP/1.1 200"), "{head1}");
    let echoed = String::from_utf8_lossy(&body1);
    assert!(echoed.ends_with("\n---\nWikipedia"), "{echoed}");
    let (head2, body2) = read_response(&mut tcp, &mut buf).await;
    assert!(head2.starts_with("HTTP/1.1 200"), "{head2}");
    assert_eq!(body2, b"hello");

    let (_, events) = wait_exchange(&mut proxy, "/echo", is_completed).await;
    let completed = events.iter().find_map(|e| match e {
        ProxyEvent::RequestCompleted { body, trailers, .. } => {
            Some((body.clone(), trailers.clone()))
        }
        _ => None,
    });
    let (body, trailers) = completed.expect("RequestCompleted");
    assert_eq!(&body.bytes[..], b"Wikipedia");
    assert_eq!(
        trailers.as_ref().and_then(|t| t.get("x-trailer")),
        Some("t1")
    );
    // Both requests went over the same client connection.
    let seen = proxy
        .wait_for(Duration::from_secs(5), |e| {
            matches!(e, ProxyEvent::ResponseCompleted { .. })
        })
        .await;
    let conns: Vec<ConnectionId> = events
        .iter()
        .chain(seen.iter())
        .filter_map(|e| match e {
            ProxyEvent::RequestStarted { client, .. } => Some(client.connection_id),
            _ => None,
        })
        .collect();
    assert!(conns.windows(2).all(|w| w[0] == w[1]), "{conns:?}");
}

#[tokio::test]
async fn chunked_response_trailers_are_relayed() {
    let upstream = spawn_http_upstream().await;
    let mut proxy = start_proxy(no_rules(), Arc::new(NoBreakpoints)).await;
    let mut tcp = tokio::net::TcpStream::connect(proxy.addr()).await.unwrap();
    tcp.write_all(format!("GET http://{upstream}/chunked HTTP/1.1\r\nHost: {upstream}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    tcp.read_to_end(&mut raw).await.unwrap();
    let text = String::from_utf8_lossy(&raw);
    assert!(text.contains("Transfer-Encoding: chunked\r\n"), "{text}");
    assert!(text.contains("hello ") && text.contains("world"), "{text}");
    assert!(text.ends_with("0\r\nX-Checksum: abc\r\n\r\n"), "{text}");
    let (_, events) = wait_exchange(&mut proxy, "/chunked", is_completed).await;
    let Some(ProxyEvent::ResponseCompleted { body, trailers, .. }) = events.last() else {
        panic!("{events:?}")
    };
    assert_eq!(&body.bytes[..], b"hello world");
    assert_eq!(
        trailers.as_ref().and_then(|t| t.get("x-checksum")),
        Some("abc")
    );
}

#[tokio::test]
async fn expect_100_continue_is_answered_before_the_body() {
    let upstream = spawn_http_upstream().await;
    let proxy = start_proxy(no_rules(), Arc::new(NoBreakpoints)).await;
    let mut tcp = tokio::net::TcpStream::connect(proxy.addr()).await.unwrap();
    tcp.write_all(format!("POST http://{upstream}/echo HTTP/1.1\r\nHost: {upstream}\r\nContent-Length: 5\r\nExpect: 100-continue\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut interim = vec![0u8; 25];
    tokio::time::timeout(Duration::from_secs(5), tcp.read_exact(&mut interim))
        .await
        .expect("100 Continue in time")
        .unwrap();
    assert_eq!(&interim[..], b"HTTP/1.1 100 Continue\r\n\r\n");
    tcp.write_all(b"abcde").await.unwrap();
    let mut buf = Vec::new();
    let (head, body) = read_response(&mut tcp, &mut buf).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert!(String::from_utf8_lossy(&body).ends_with("\n---\nabcde"));
}

#[tokio::test]
async fn upstream_failure_returns_explanatory_502() {
    let closed = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap()
    };
    let mut proxy = start_proxy(no_rules(), Arc::new(NoBreakpoints)).await;
    let res = client(&proxy)
        .get(format!("http://{closed}/x"))
        .header("accept", "text/html")
        .send()
        .await
        .expect("response");
    assert_eq!(res.status(), 502);
    assert_eq!(
        res.headers()
            .get("x-akev-error")
            .and_then(|v| v.to_str().ok()),
        Some("connection-refused")
    );
    let page = res.text().await.unwrap();
    assert!(page.contains("refused") && page.contains("Akev"), "{page}");
    let (_, events) = wait_exchange(&mut proxy, "/x", is_completed).await;
    match events.last() {
        Some(ProxyEvent::Aborted { error, .. }) => {
            assert_eq!(error.kind, ErrorKind::ConnectionRefused)
        }
        other => panic!("expected Aborted, got {other:?}"),
    }
}

#[tokio::test]
async fn garbage_is_reported_as_client_error() {
    let mut proxy = start_proxy(no_rules(), Arc::new(NoBreakpoints)).await;
    let mut tcp = tokio::net::TcpStream::connect(proxy.addr()).await.unwrap();
    tcp.write_all(b"\x00\x01\x02 definitely not http\r\n\r\n")
        .await
        .unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), tcp.read_to_end(&mut out)).await;
    assert!(
        String::from_utf8_lossy(&out).starts_with("HTTP/1.1 400"),
        "{:?}",
        String::from_utf8_lossy(&out)
    );
    let seen = proxy
        .wait_for(Duration::from_secs(5), |e| {
            matches!(e, ProxyEvent::ClientError { .. })
        })
        .await;
    let Some(ProxyEvent::ClientError {
        error,
        raw_prefix,
        client,
        ..
    }) = seen.last()
    else {
        panic!()
    };
    assert_eq!(error.kind, ErrorKind::ClientProtocolError);
    assert!(raw_prefix.starts_with(b"\x00\x01\x02"));
    assert!(!client.remote_addr.is_empty());

    // A malformed request line gets a 400 and a ClientError with the parser code.
    let (head, _) = raw_exchange(&proxy, b"GET / HTTP/9.9\r\n\r\n").await;
    assert!(head.starts_with("HTTP/1.1 400"), "{head}");
    let seen = proxy
        .wait_for(Duration::from_secs(5), |e| {
            matches!(e, ProxyEvent::ClientError { .. })
        })
        .await;
    let Some(ProxyEvent::ClientError { error, .. }) = seen.last() else {
        panic!()
    };
    assert_eq!(error.code.as_deref(), Some("HPE_INVALID_VERSION"));
}

#[tokio::test]
async fn server_sent_events_stream_through_as_messages() {
    let upstream = spawn_http_upstream().await;
    let mut proxy = start_proxy(no_rules(), Arc::new(NoBreakpoints)).await;
    let mut res = client(&proxy)
        .get(format!("http://{upstream}/sse"))
        .send()
        .await
        .expect("response");
    // The first event arrives before the stream ends (no buffering in the proxy).
    let first = tokio::time::timeout(Duration::from_secs(5), res.chunk())
        .await
        .expect("first chunk")
        .unwrap()
        .unwrap();
    assert_eq!(&first[..], b"data: one\n\n");
    let mut rest = Vec::new();
    while let Some(c) = res.chunk().await.unwrap() {
        rest.extend_from_slice(&c);
    }
    assert_eq!(rest, b"event: tick\nid: 2\ndata: two\n\n");

    let (_, events) = wait_exchange(&mut proxy, "/sse", is_completed).await;
    assert!(events.iter().any(|e| matches!(
        e,
        ProxyEvent::StreamOpened {
            kind: ExchangeKind::EventStream,
            ..
        }
    )));
    let msgs: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            ProxyEvent::StreamMessage {
                message, payload, ..
            } => Some((message.clone(), payload.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(msgs.len(), 2, "{msgs:?}");
    assert_eq!(
        (msgs[0].0.kind, msgs[0].0.direction, &msgs[0].1[..]),
        (MessageKind::Event, Direction::Received, &b"one"[..])
    );
    assert_eq!(
        (
            msgs[1].0.event.as_deref(),
            msgs[1].0.event_id.as_deref(),
            &msgs[1].1[..]
        ),
        (Some("tick"), Some("2"), &b"two"[..])
    );
    assert_eq!((msgs[0].0.seq, msgs[1].0.seq), (0, 1));
}

#[tokio::test]
async fn capture_is_truncated_but_body_streams_in_full() {
    let upstream = spawn_http_upstream().await;
    let capture = CaptureSettings {
        max_body_bytes: 1000,
        detect_processes: false,
        ..Default::default()
    };
    let mut proxy = start_proxy_with(
        no_rules(),
        Arc::new(NoBreakpoints),
        proxy_settings(),
        capture,
    )
    .await;
    let body = client(&proxy)
        .get(format!("http://{upstream}/big"))
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(body.len(), 100_000);
    let (_, events) = wait_exchange(&mut proxy, "/big", is_completed).await;
    let Some(ProxyEvent::ResponseCompleted { body, .. }) = events.last() else {
        panic!()
    };
    assert_eq!(
        (body.bytes.len(), body.wire_size, body.truncated),
        (1000, 100_000, true)
    );
}

#[tokio::test]
async fn ignored_hosts_are_proxied_but_not_recorded() {
    let upstream = spawn_http_upstream().await;
    let capture = CaptureSettings {
        ignore_hosts: vec!["127.0.0.*".into()],
        detect_processes: false,
        ..Default::default()
    };
    let mut proxy = start_proxy_with(
        no_rules(),
        Arc::new(NoBreakpoints),
        proxy_settings(),
        capture,
    )
    .await;
    let res = client(&proxy)
        .get(format!("http://{upstream}/hello"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.text().await.unwrap(), "hello");
    let events = proxy.drain(Duration::from_millis(300)).await;
    assert!(
        events.is_empty(),
        "{:?}",
        events.iter().map(event_name).collect::<Vec<_>>()
    );

    // Live settings update: recording resumes without restarting.
    let settings = proxy_settings();
    proxy.handle.update_settings(
        settings,
        CaptureSettings {
            detect_processes: false,
            ..Default::default()
        },
    );
    let res = client(&proxy)
        .get(format!("http://{upstream}/hello"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.text().await.unwrap(), "hello");
    wait_exchange(&mut proxy, "/hello", is_completed).await;
}

#[tokio::test]
async fn transparent_request_uses_host_header() {
    let upstream = spawn_http_upstream().await;
    let mut proxy = start_proxy(no_rules(), Arc::new(NoBreakpoints)).await;
    let (head, body) = raw_exchange(
        &proxy,
        format!("GET /hello HTTP/1.1\r\nHost: {upstream}\r\n\r\n").as_bytes(),
    )
    .await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(body, b"hello");
    let (_, events) = wait_exchange(&mut proxy, "/hello", is_completed).await;
    let ProxyEvent::RequestStarted { head, client, .. } = &events[0] else {
        panic!()
    };
    assert_eq!(head.url, format!("http://{upstream}/hello"));
    assert_eq!(client.via, ProxyMode::Direct);
}

#[tokio::test]
async fn setup_page_is_served_locally() {
    let mut proxy = start_proxy(no_rules(), Arc::new(NoBreakpoints)).await;
    let http = client(&proxy);
    let page = http.get("http://akev.cert/").send().await.unwrap();
    assert_eq!(page.status(), 200);
    assert!(
        page.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );
    let html = page.text().await.unwrap();
    assert!(html.contains("Akev") && html.contains("/ca.mobileconfig"));
    let pem = http.get("http://akev.cert/ca.pem").send().await.unwrap();
    assert_eq!(pem.headers()["content-type"], "application/x-pem-file");
    assert_eq!(pem.text().await.unwrap(), proxy.ca.cert_pem());

    // Directly addressed to the proxy port (a phone opening http://<ip>:<port>/).
    let direct = reqwest::Client::builder().no_proxy().build().unwrap();
    let der = direct
        .get(format!("http://{}/ca.crt", proxy.addr()))
        .send()
        .await
        .unwrap();
    assert_eq!(der.headers()["content-type"], "application/x-x509-ca-cert");
    assert_eq!(&der.bytes().await.unwrap()[..], proxy.ca.cert_der());
    let pac = direct
        .get(format!("http://{}/proxy.pac", proxy.addr()))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(pac.contains(&format!("PROXY {}", proxy.addr())), "{pac}");

    // Local responses are recorded like any other exchange (no upstream involved).
    let (_, events) = wait_exchange(&mut proxy, "/ca.pem", is_completed).await;
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, ProxyEvent::Upstream { .. }))
    );
}

#[tokio::test]
async fn http10_request_gets_http10_semantics() {
    let upstream = spawn_http_upstream().await;
    let proxy = start_proxy(no_rules(), Arc::new(NoBreakpoints)).await;
    let mut tcp = tokio::net::TcpStream::connect(proxy.addr()).await.unwrap();
    tcp.write_all(format!("GET http://{upstream}/chunked HTTP/1.0\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut raw = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), tcp.read_to_end(&mut raw))
        .await
        .expect("connection closed after response")
        .unwrap();
    let text = String::from_utf8_lossy(&raw);
    // HTTP/1.0 clients can't parse chunked: the body is close-delimited instead.
    assert!(
        !text.to_ascii_lowercase().contains("transfer-encoding"),
        "{text}"
    );
    assert!(text.ends_with("\r\n\r\nhello world"), "{text}");

    // A Host-less HTTP/1.0 proxy request reaches the upstream with exactly one Host header.
    let (_, body) = raw_exchange(
        &proxy,
        format!("GET http://{upstream}/echo HTTP/1.0\r\n\r\n").as_bytes(),
    )
    .await;
    let echoed = String::from_utf8_lossy(&body);
    assert_eq!(echoed.matches("Host: ").count(), 1, "{echoed}");
    assert!(
        echoed.contains(&format!("Host: {upstream}\r\n")),
        "{echoed}"
    );
}

// ---------------------------------------------------------------------------------------
// Upstream misbehaviour: truncated bodies, ambiguous framing, failed connections,
// connection-oriented authentication.
// ---------------------------------------------------------------------------------------

/// Read everything the proxy sends until the connection ends (5 s at most).
async fn read_until_closed(tcp: &mut tokio::net::TcpStream) -> (Vec<u8>, std::io::Result<usize>) {
    let mut raw = Vec::new();
    let res = tokio::time::timeout(Duration::from_secs(5), tcp.read_to_end(&mut raw))
        .await
        .expect("the proxy ended the connection");
    (raw, res)
}

#[tokio::test]
async fn upstream_failure_mid_body_resets_the_client_connection() {
    // A close-delimited body cut short by a TCP RST, and a chunked body cut short the same way.
    let upstream = spawn_raw(|mut s| async move {
        let mut buf = Vec::new();
        let Some((head, _)) = read_request(&mut s, &mut buf).await else {
            return;
        };
        let response: &[u8] = if String::from_utf8_lossy(&head).contains("/chunked-rst") {
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n"
        } else {
            b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\nhello"
        };
        let _ = s.write_all(response).await;
        let _ = s.flush().await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        let _ = s.set_zero_linger();
    })
    .await;
    let proxy = start_proxy(no_rules(), Arc::new(NoBreakpoints)).await;
    for (path, version) in [
        ("/close-rst", "HTTP/1.1"),
        ("/close-rst", "HTTP/1.0"),
        ("/chunked-rst", "HTTP/1.0"),
    ] {
        let mut tcp = tokio::net::TcpStream::connect(proxy.addr()).await.unwrap();
        tcp.write_all(
            format!("GET http://{upstream}{path} {version}\r\nHost: {upstream}\r\n\r\n").as_bytes(),
        )
        .await
        .unwrap();
        let (raw, res) = read_until_closed(&mut tcp).await;
        // Close-delimited framing: a clean EOF would tell the client the body is complete.
        match res {
            Err(e) => assert_eq!(
                e.kind(),
                std::io::ErrorKind::ConnectionReset,
                "{path} {version}"
            ),
            Ok(_) => panic!(
                "{path} {version}: truncated body ended with a graceful close: {:?}",
                String::from_utf8_lossy(&raw)
            ),
        }
    }
}

#[tokio::test]
async fn content_length_is_dropped_when_transfer_encoding_frames_the_response() {
    let upstream = spawn_raw(|mut s| async move {
        let mut buf = Vec::new();
        while read_request(&mut s, &mut buf).await.is_some() {
            let response: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n";
            if s.write_all(response).await.is_err() {
                return;
            }
        }
    })
    .await;
    let mut proxy = start_proxy(no_rules(), Arc::new(NoBreakpoints)).await;

    // HTTP/1.0 client: close-delimited without the stale length (which made it keep "hel").
    let mut tcp = tokio::net::TcpStream::connect(proxy.addr()).await.unwrap();
    tcp.write_all(format!("GET http://{upstream}/te-cl HTTP/1.0\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let (raw, res) = read_until_closed(&mut tcp).await;
    res.unwrap();
    let text = String::from_utf8_lossy(&raw).to_ascii_lowercase();
    assert!(!text.contains("content-length"), "{text}");
    assert!(!text.contains("transfer-encoding"), "{text}");
    assert!(text.ends_with("\r\n\r\nhello"), "{text}");

    // HTTP/1.1 client: chunked, and the conflicting Content-Length is removed (RFC 9112 §6.3).
    let mut tcp = tokio::net::TcpStream::connect(proxy.addr()).await.unwrap();
    tcp.write_all(
        format!(
            "GET http://{upstream}/te-cl HTTP/1.1\r\nHost: {upstream}\r\nConnection: close\r\n\r\n"
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    let (raw, res) = read_until_closed(&mut tcp).await;
    res.unwrap();
    let text = String::from_utf8_lossy(&raw);
    let lower = text.to_ascii_lowercase();
    assert!(!lower.contains("content-length"), "{text}");
    assert!(lower.contains("transfer-encoding: chunked"), "{text}");
    assert!(text.ends_with("\r\n\r\n5\r\nhello\r\n0\r\n\r\n"), "{text}");

    // What's recorded matches what the client got.
    let (_, events) = wait_exchange(&mut proxy, "/te-cl", is_completed).await;
    for e in &events {
        if let ProxyEvent::ResponseStarted { head, .. } = e {
            assert!(!head.headers.contains("content-length"), "{head:?}");
        }
        if let ProxyEvent::ResponseCompleted { body, .. } = e {
            assert_eq!(&body.bytes[..], b"hello");
        }
    }
}

#[tokio::test]
async fn request_body_is_recorded_when_the_upstream_connection_fails() {
    let closed = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap()
    };
    let mut proxy = start_proxy(no_rules(), Arc::new(NoBreakpoints)).await;
    let payload = r#"{"important":"payload"}"#;
    let mut tcp = tokio::net::TcpStream::connect(proxy.addr()).await.unwrap();
    // A Content-Length body, then a chunked one on the same (still usable) connection.
    tcp.write_all(
        format!(
            "POST http://{closed}/refused-post HTTP/1.1\r\nHost: {closed}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{payload}",
            payload.len()
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    let mut buf = Vec::new();
    let (head, _) = read_response(&mut tcp, &mut buf).await;
    assert!(head.starts_with("HTTP/1.1 502"), "{head}");
    tcp.write_all(
        format!(
            "POST http://{closed}/refused-chunked HTTP/1.1\r\nHost: {closed}\r\nTransfer-Encoding: chunked\r\n\r\n6\r\nchunk-\r\n3\r\nend\r\n0\r\n\r\n"
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    let (head, _) = read_response(&mut tcp, &mut buf).await;
    assert!(head.starts_with("HTTP/1.1 502"), "{head}");

    for (path, expected) in [
        ("/refused-post", payload.as_bytes()),
        ("/refused-chunked", &b"chunk-end"[..]),
    ] {
        let (_, events) = wait_exchange(&mut proxy, path, |e| {
            matches!(e, ProxyEvent::Aborted { .. })
        })
        .await;
        let body = events
            .iter()
            .find_map(|e| match e {
                ProxyEvent::RequestCompleted { body, .. } => Some(body.clone()),
                _ => None,
            })
            .expect("RequestCompleted");
        assert_eq!(&body.bytes[..], expected, "{path}");
        assert!(!body.truncated, "{path}: {body:?}");
        assert_eq!(body.wire_size, expected.len() as u64, "{path}");
        match events.last() {
            Some(ProxyEvent::Aborted { error, .. }) => {
                assert_eq!(error.kind, ErrorKind::ConnectionRefused)
            }
            other => panic!("expected Aborted, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn partial_response_body_is_kept_when_an_exchange_aborts_mid_body() {
    let upstream = spawn_raw(|mut s| async move {
        let mut buf = Vec::new();
        let Some((head, _)) = read_request(&mut s, &mut buf).await else {
            return;
        };
        let _ = s
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\n0123456789")
            .await;
        let _ = s.flush().await;
        if String::from_utf8_lossy(&head).contains("/short-cl") {
            // The upstream gives up with 990 bytes missing.
            tokio::time::sleep(Duration::from_millis(50)).await;
        } else {
            // `/stalled`: the client gives up first.
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    })
    .await;
    let mut proxy = start_proxy(no_rules(), Arc::new(NoBreakpoints)).await;

    // Upstream closes before the declared length.
    let mut tcp = tokio::net::TcpStream::connect(proxy.addr()).await.unwrap();
    tcp.write_all(
        format!("GET http://{upstream}/short-cl HTTP/1.1\r\nHost: {upstream}\r\n\r\n").as_bytes(),
    )
    .await
    .unwrap();
    let _ = read_until_closed(&mut tcp).await;
    let (id, events) = wait_exchange(&mut proxy, "/short-cl", |e| {
        matches!(e, ProxyEvent::Aborted { .. })
    })
    .await;
    assert_eq!(
        names_for(&events, id)[events.len() - 2..],
        ["ResponseCompleted", "Aborted"]
    );
    let body = events
        .iter()
        .find_map(|e| match e {
            ProxyEvent::ResponseCompleted { body, .. } => Some(body.clone()),
            _ => None,
        })
        .expect("partial body recorded");
    assert_eq!(&body.bytes[..], b"0123456789");
    assert_eq!(body.wire_size, 10);
    assert!(body.truncated, "a partial body is marked truncated");

    // The client goes away mid-body.
    let mut tcp = tokio::net::TcpStream::connect(proxy.addr()).await.unwrap();
    tcp.write_all(
        format!("GET http://{upstream}/stalled HTTP/1.1\r\nHost: {upstream}\r\n\r\n").as_bytes(),
    )
    .await
    .unwrap();
    let mut raw = Vec::new();
    while !raw.ends_with(b"0123456789") {
        let mut tmp = [0u8; 1024];
        let n = tokio::time::timeout(Duration::from_secs(5), tcp.read(&mut tmp))
            .await
            .expect("first body bytes")
            .unwrap();
        assert!(n > 0, "closed early: {:?}", String::from_utf8_lossy(&raw));
        raw.extend_from_slice(&tmp[..n]);
    }
    drop(tcp);
    let (_, events) = wait_exchange(&mut proxy, "/stalled", |e| {
        matches!(e, ProxyEvent::Aborted { .. })
    })
    .await;
    let body = events
        .iter()
        .find_map(|e| match e {
            ProxyEvent::ResponseCompleted { body, .. } => Some(body.clone()),
            _ => None,
        })
        .expect("partial body recorded");
    assert_eq!(&body.bytes[..], b"0123456789");
    match events.last() {
        Some(ProxyEvent::Aborted { error, .. }) => {
            assert_eq!(error.kind, ErrorKind::ClientAborted)
        }
        other => panic!("expected Aborted, got {other:?}"),
    }
}

#[tokio::test]
async fn ntlm_authenticated_upstream_connections_are_never_shared_between_clients() {
    // An origin that authenticates the TCP connection (like IIS with NTLM): once a request on a
    // connection carried credentials, later requests on it are served without them.
    let upstream = spawn_raw(|mut s| async move {
        let mut buf = Vec::new();
        let mut user: Option<String> = None;
        while let Some((head, _)) = read_request(&mut s, &mut buf).await {
            let head = String::from_utf8_lossy(&head).to_string();
            if let Some(auth) = head
                .lines()
                .find_map(|l| l.strip_prefix("Authorization: "))
            {
                user = Some(auth.to_string());
            }
            let response = match &user {
                Some(u) => {
                    let body = format!("secret data for {u}");
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    )
                }
                None => "HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: NTLM\r\nContent-Length: 6\r\n\r\ndenied"
                    .to_string(),
            };
            if s.write_all(response.as_bytes()).await.is_err() {
                return;
            }
        }
    })
    .await;
    let proxy = start_proxy(no_rules(), Arc::new(NoBreakpoints)).await;
    let get = |auth: Option<&str>| {
        let auth = auth
            .map(|a| format!("Authorization: {a}\r\n"))
            .unwrap_or_default();
        format!("GET http://{upstream}/conn-auth HTTP/1.1\r\nHost: {upstream}\r\n{auth}\r\n")
    };

    // Client A authenticates its upstream connection.
    let mut alice = tokio::net::TcpStream::connect(proxy.addr()).await.unwrap();
    let mut alice_buf = Vec::new();
    alice
        .write_all(get(Some("NTLM alice-token")).as_bytes())
        .await
        .unwrap();
    let (head, body) = read_response(&mut alice, &mut alice_buf).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(body, b"secret data for NTLM alice-token");

    // Another client without credentials must not ride on A's authenticated connection.
    let mut bob = tokio::net::TcpStream::connect(proxy.addr()).await.unwrap();
    let mut bob_buf = Vec::new();
    bob.write_all(get(None).as_bytes()).await.unwrap();
    let (head, body) = read_response(&mut bob, &mut bob_buf).await;
    assert!(
        head.starts_with("HTTP/1.1 401"),
        "another client got A's session: {head}{}",
        String::from_utf8_lossy(&body)
    );

    // A itself keeps its authenticated connection (the next handshake leg / request).
    alice.write_all(get(None).as_bytes()).await.unwrap();
    let (head, body) = read_response(&mut alice, &mut alice_buf).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(body, b"secret data for NTLM alice-token");
}

/// CA regeneration restarts the proxy on the old one's listening socket: the address can't
/// be lost to another program meanwhile, and a client connecting during the restart waits in
/// the backlog and is served by the new proxy instead of being refused.
#[tokio::test]
async fn a_restarted_proxy_takes_over_the_listening_socket() {
    let upstream = spawn_http_upstream().await;
    let proxy = start_proxy(no_rules(), Arc::new(NoBreakpoints)).await;
    let addr = proxy.addr();
    let listener = proxy
        .handle
        .shutdown_keep_listener()
        .await
        .expect("the listening socket is kept");
    assert_eq!(listener.local_addr().unwrap(), addr);
    assert!(
        proxy.handle.shutdown_keep_listener().await.is_none(),
        "a stopped proxy has no socket left to give"
    );

    let mut conn = tokio::net::TcpStream::connect(addr)
        .await
        .expect("connecting while no proxy runs is queued, not refused");
    conn.write_all(
        format!(
            "GET http://{upstream}/hello HTTP/1.1\r\nHost: {upstream}\r\nConnection: close\r\n\r\n"
        )
        .as_bytes(),
    )
    .await
    .unwrap();

    let restarted = start_proxy_on(listener).await;
    assert_eq!(restarted.addr(), addr);
    let mut buf = Vec::new();
    let (head, body) =
        tokio::time::timeout(Duration::from_secs(10), read_response(&mut conn, &mut buf))
            .await
            .expect("the queued client is served");
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(body, b"hello");
    restarted.handle.shutdown().await;

    // A plain shutdown closes the socket: new clients are refused.
    assert!(tokio::net::TcpStream::connect(addr).await.is_err());
}
