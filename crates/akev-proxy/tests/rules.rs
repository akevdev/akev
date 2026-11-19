//! End-to-end: rule actions and breakpoints applied by the pipeline.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use akev_core::*;
use bytes::Bytes;
use common::*;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Breakpoint handler that edits requests and responses (or answers requests itself).
struct Editor {
    respond: bool,
}

#[async_trait::async_trait]
impl BreakpointHandler for Editor {
    async fn on_request(
        &self,
        _id: ExchangeId,
        head: &RequestHead,
        mut request: EditableRequest,
    ) -> RequestResume {
        assert_eq!(request.url, head.url);
        if self.respond {
            return RequestResume::Respond {
                response: EditableResponse {
                    status: 418,
                    status_message: String::new(),
                    headers: Headers(vec![("Content-Type".into(), "text/plain".into())]),
                    body: Bytes::from_static(b"answered at breakpoint"),
                },
            };
        }
        request.headers.set("X-Edited", "yes");
        request.body = Bytes::from_static(b"edited body");
        RequestResume::Continue { request }
    }

    async fn on_response(
        &self,
        _id: ExchangeId,
        _request: &RequestHead,
        _head: &ResponseHead,
        mut response: EditableResponse,
    ) -> ResponseResume {
        response.status = 203;
        response.headers.append("X-Edited-Response", "1");
        response.body = Bytes::from_static(b"edited response!");
        ResponseResume::Continue { response }
    }
}

#[tokio::test]
async fn mock_rule_responds_without_upstream() {
    let rules = rules(
        r#"{"items":[{"type":"rule","id":"mock-users","matchers":[{"type":"host","pattern":{"op":"equals","value":"api.example.test"}}],
            "action":{"type":"mock","status":201,"headers":[["X-Mocked","yes"]],"body":{"type":"json","value":{"ok":true}}}}]}"#,
    );
    let mut proxy = start_proxy(rules, Arc::new(NoBreakpoints)).await;
    let res = client(&proxy)
        .get("http://api.example.test/users")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    assert_eq!(res.headers()["x-mocked"], "yes");
    assert!(
        res.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("application/json")
    );
    assert_eq!(res.text().await.unwrap(), r#"{"ok":true}"#);

    let (_, events) = wait_exchange(&mut proxy, "/users", is_completed).await;
    let names: Vec<_> = events.iter().map(event_name).collect();
    assert_eq!(
        names,
        [
            "RequestStarted",
            "RequestCompleted",
            "RuleMatched",
            "ResponseStarted",
            "ResponseCompleted"
        ]
    );
    assert!(
        matches!(&events[2], ProxyEvent::RuleMatched { rule, mocked: true, .. } if rule == "mock-users")
    );
}

#[tokio::test]
async fn request_breakpoint_edits_reach_upstream() {
    let upstream = spawn_http_upstream().await;
    let rules = rules(
        r#"{"items":[{"type":"rule","id":"bp","matchers":[{"type":"path","pattern":{"op":"equals","value":"/echo"}}],"action":{"type":"breakpoint","request":true}}]}"#,
    );
    let mut proxy = start_proxy(rules, Arc::new(Editor { respond: false })).await;
    let echoed = client(&proxy)
        .post(format!("http://{upstream}/echo"))
        .body("original")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(echoed.contains("X-Edited: yes\r\n"), "{echoed}");
    assert!(
        echoed.contains("content-length: 11\r\n"),
        "Content-Length fixed for the edited body: {echoed}"
    );
    assert!(echoed.ends_with("\n---\nedited body"), "{echoed}");

    let (_, events) = wait_exchange(&mut proxy, "/echo", is_completed).await;
    let names: Vec<_> = events.iter().map(event_name).collect();
    assert_eq!(
        names,
        [
            "RequestStarted",
            "RequestCompleted",
            "RuleMatched",
            "Paused",
            "Resumed",
            "RequestModified",
            "Upstream",
            "ResponseStarted",
            "ResponseCompleted"
        ]
    );
    assert!(matches!(
        &events[3],
        ProxyEvent::Paused {
            stage: BreakpointStage::Request,
            ..
        }
    ));
    match &events[1] {
        ProxyEvent::RequestCompleted { body, .. } => assert_eq!(&body.bytes[..], b"original"),
        other => panic!("{other:?}"),
    }
    match &events[5] {
        ProxyEvent::RequestModified { head, body, .. } => {
            assert_eq!(head.headers.get("x-edited"), Some("yes"));
            assert_eq!(
                body.as_ref().map(|b| &b.bytes[..]),
                Some(&b"edited body"[..])
            );
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn request_breakpoint_can_answer_directly() {
    let rules = rules(
        r#"{"items":[{"type":"rule","id":"bp","action":{"type":"breakpoint","request":true}}]}"#,
    );
    let mut proxy = start_proxy(rules, Arc::new(Editor { respond: true })).await;
    let res = client(&proxy)
        .get("http://never-contacted.invalid/x")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 418);
    assert_eq!(res.text().await.unwrap(), "answered at breakpoint");
    let (_, events) = wait_exchange(&mut proxy, "/x", is_completed).await;
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, ProxyEvent::Upstream { .. }))
    );
    // The locally generated response is attributed to the breakpoint rule.
    assert!(
        events.iter().any(
            |e| matches!(e, ProxyEvent::RuleMatched { rule, mocked: true, .. } if rule == "bp")
        )
    );
}

#[tokio::test]
async fn response_breakpoint_edits_reach_client() {
    let upstream = spawn_http_upstream().await;
    let rules = rules(
        r#"{"items":[{"type":"rule","id":"bp","action":{"type":"breakpoint","request":false,"response":true}}]}"#,
    );
    let mut proxy = start_proxy(rules, Arc::new(Editor { respond: false })).await;
    let res = client(&proxy)
        .get(format!("http://{upstream}/hello"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 203);
    assert_eq!(res.headers()["x-edited-response"], "1");
    assert_eq!(res.headers()["content-length"], "16");
    assert_eq!(res.text().await.unwrap(), "edited response!");
    let (_, events) = wait_exchange(&mut proxy, "/hello", is_completed).await;
    assert!(events.iter().any(|e| matches!(
        e,
        ProxyEvent::Paused {
            stage: BreakpointStage::Response,
            ..
        }
    )));
    let modified = events.iter().find_map(|e| match e {
        ProxyEvent::ResponseModified {
            original_head,
            original_body,
            ..
        } => Some((original_head.status, original_body.clone())),
        _ => None,
    });
    let (status, body) = modified.expect("ResponseModified");
    assert_eq!(status, 200);
    assert_eq!(body.map(|b| b.bytes), Some(Bytes::from_static(b"hello")));
    match events.last() {
        Some(ProxyEvent::ResponseCompleted { body, .. }) => {
            assert_eq!(&body.bytes[..], b"edited response!")
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn transform_rules_stack_with_passthrough() {
    let upstream = spawn_http_upstream().await;
    let rules = rules(
        r#"{"items":[
            {"type":"rule","id":"add-header","action":{"type":"transform","request":{"setHeaders":[["X-Added","1"]]},"response":{"setHeaders":[["X-Resp","2"]]}}},
            {"type":"rule","id":"rewrite-body","matchers":[{"type":"path","pattern":{"op":"equals","value":"/hello"}}],
             "action":{"type":"passthrough","response":{"body":{"type":"match-replace","replacements":[{"find":"hello","replace":"howdy!"}]}}}}
        ]}"#,
    );
    let mut proxy = start_proxy(rules, Arc::new(NoBreakpoints)).await;
    let http = client(&proxy);
    let echoed = http
        .get(format!("http://{upstream}/echo"))
        .send()
        .await
        .unwrap();
    assert_eq!(echoed.headers()["x-resp"], "2");
    assert!(echoed.text().await.unwrap().contains("X-Added: 1\r\n"));
    let res = http
        .get(format!("http://{upstream}/hello"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.headers()["content-length"], "6");
    assert_eq!(res.text().await.unwrap(), "howdy!");

    let (_, events) = wait_exchange(&mut proxy, "/hello", is_completed).await;
    let rules_matched: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            ProxyEvent::RuleMatched { rule, mocked, .. } => Some((rule.as_str(), *mocked)),
            _ => None,
        })
        .collect();
    assert_eq!(
        rules_matched,
        [("add-header", false), ("rewrite-body", false)]
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ProxyEvent::RequestModified { .. }))
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ProxyEvent::ResponseModified { .. }))
    );
}

#[tokio::test]
async fn close_reset_and_timeout_rules() {
    let rules = rules(
        r#"{"items":[
            {"type":"rule","id":"close","matchers":[{"type":"path","pattern":{"op":"equals","value":"/close"}}],"action":{"type":"close"}},
            {"type":"rule","id":"reset","matchers":[{"type":"path","pattern":{"op":"equals","value":"/reset"}}],"action":{"type":"reset"}},
            {"type":"rule","id":"timeout","matchers":[{"type":"path","pattern":{"op":"equals","value":"/timeout"}}],"action":{"type":"timeout"}}
        ]}"#,
    );
    let mut proxy = start_proxy(rules, Arc::new(NoBreakpoints)).await;
    for (path, kind) in [
        ("/close", ErrorKind::RuleClosed),
        ("/reset", ErrorKind::RuleReset),
    ] {
        let mut tcp = tokio::net::TcpStream::connect(proxy.addr()).await.unwrap();
        tcp.write_all(
            format!("GET http://rules.invalid{path} HTTP/1.1\r\nHost: rules.invalid\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
        let mut out = Vec::new();
        let read = tokio::time::timeout(Duration::from_secs(5), tcp.read_to_end(&mut out))
            .await
            .expect("connection ended");
        assert!(out.is_empty(), "no response expected for {path}");
        if path == "/reset" {
            assert!(
                read.is_err_and(|e| e.kind() == std::io::ErrorKind::ConnectionReset),
                "expected a TCP RST"
            );
        }
        let (_, events) = wait_exchange(&mut proxy, path, is_completed).await;
        match events.last() {
            Some(ProxyEvent::Aborted { error, .. }) => assert_eq!(error.kind, kind),
            other => panic!("{other:?}"),
        }
    }

    let started = Instant::now();
    let short = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(proxy.url()).unwrap())
        .timeout(Duration::from_millis(300))
        .build()
        .unwrap();
    assert!(
        short
            .get("http://rules.invalid/timeout")
            .send()
            .await
            .is_err()
    );
    assert!(started.elapsed() >= Duration::from_millis(300));
    let (_, events) = wait_exchange(&mut proxy, "/timeout", is_completed).await;
    match events.last() {
        Some(ProxyEvent::Aborted { error, .. }) => assert_eq!(error.kind, ErrorKind::RuleTimeout),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn delay_and_forward_rules() {
    let upstream = spawn_http_upstream().await;
    let rules = rules(&format!(
        r#"{{"items":[
            {{"type":"rule","id":"slow","action":{{"type":"delay","ms":200}}}},
            {{"type":"rule","id":"remote","matchers":[{{"type":"host","pattern":{{"op":"equals","value":"prod.example.test"}}}}],
              "action":{{"type":"forward","target":"http://{upstream}"}}}}
        ]}}"#
    ));
    let mut proxy = start_proxy(rules, Arc::new(NoBreakpoints)).await;
    let started = Instant::now();
    let res = client(&proxy)
        .get("http://prod.example.test/hello")
        .send()
        .await
        .unwrap();
    assert!(started.elapsed() >= Duration::from_millis(200));
    assert_eq!(res.text().await.unwrap(), "hello");
    let (_, events) = wait_exchange(&mut proxy, "/hello", is_completed).await;
    let modified = events.iter().find_map(|e| match e {
        ProxyEvent::RequestModified { head, .. } => Some(head.clone()),
        _ => None,
    });
    let sent = modified.expect("forwarded request recorded as modified");
    assert_eq!(
        (sent.host.as_str(), sent.port),
        ("127.0.0.1", upstream.port())
    );
    assert_eq!(
        sent.headers.get("host"),
        Some(upstream.to_string().as_str())
    );
}

#[tokio::test]
async fn map_local_serves_files_or_falls_back() {
    let upstream = spawn_http_upstream().await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("hello"), "from disk").unwrap();
    let path = dir.path().to_string_lossy().replace('\\', "\\\\");
    let rules = rules(&format!(
        r#"{{"items":[{{"type":"rule","id":"local","action":{{"type":"map-local","path":"{path}","fallbackToUpstream":true}}}}]}}"#
    ));
    let mut proxy = start_proxy(rules, Arc::new(NoBreakpoints)).await;
    let http = client(&proxy);
    assert_eq!(
        http.get(format!("http://{upstream}/hello"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "from disk"
    );
    let (_, events) = wait_exchange(&mut proxy, "/hello", is_completed).await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ProxyEvent::RuleMatched { mocked: true, .. }))
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, ProxyEvent::Upstream { .. }))
    );

    // A large file is streamed from disk with its length, byte for byte.
    let big: Vec<u8> = (0..3 * 1024 * 1024 + 7).map(|i| (i % 253) as u8).collect();
    std::fs::write(dir.path().join("big.bin"), &big).unwrap();
    let res = http
        .get(format!("http://{upstream}/big.bin"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.content_length(), Some(big.len() as u64));
    assert_eq!(res.bytes().await.unwrap().as_ref(), big.as_slice());
    let (_, events) = wait_exchange(&mut proxy, "/big.bin", is_completed).await;
    let recorded = events.iter().find_map(|e| match e {
        ProxyEvent::ResponseCompleted { body, .. } => Some(body.wire_size),
        _ => None,
    });
    assert_eq!(recorded, Some(big.len() as u64));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ProxyEvent::RuleMatched { mocked: true, .. }))
    );

    // Missing file → forwarded upstream; not flagged as mocked.
    let res = http
        .get(format!("http://{upstream}/echo"))
        .send()
        .await
        .unwrap();
    let status = res.status();
    let text = res.text().await.unwrap();
    assert!(text.starts_with("GET /echo"), "{status} {text}");
    let (_, events) = wait_exchange(&mut proxy, "/echo", is_completed).await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ProxyEvent::RuleMatched { mocked: false, .. }))
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ProxyEvent::Upstream { .. }))
    );
}

#[tokio::test]
async fn rules_on_https_and_http2_traffic() {
    let upstream = spawn_tls_upstream().await;
    let rules = rules(
        r#"{"items":[{"type":"rule","id":"m","matchers":[{"type":"path","pattern":{"op":"equals","value":"/mocked"}}],"action":{"type":"mock","status":200,"body":{"type":"text","text":"mocked over h2"}}}]}"#,
    );
    let proxy = start_proxy(rules, Arc::new(NoBreakpoints)).await;
    let http = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(proxy.url()).unwrap())
        .tls_certs_only([reqwest::Certificate::from_pem(proxy.ca.cert_pem().as_bytes()).unwrap()])
        .build()
        .unwrap();
    let res = http
        .get(format!("https://localhost:{}/mocked", upstream.port()))
        .send()
        .await
        .unwrap();
    assert_eq!(res.version(), reqwest::Version::HTTP_2);
    assert_eq!(res.text().await.unwrap(), "mocked over h2");
    let res = http
        .get(format!("https://localhost:{}/hello", upstream.port()))
        .send()
        .await
        .unwrap();
    assert_eq!(res.text().await.unwrap(), "hello");
}

#[tokio::test]
async fn body_matchers_and_encoded_response_transforms() {
    let upstream = spawn_http_upstream().await;
    let rules = rules(
        r#"{"items":[
            {"type":"rule","id":"by-body","matchers":[{"type":"body","pattern":{"op":"contains","value":"secret"}}],
             "action":{"type":"mock","status":200,"body":{"type":"text","text":"matched on body"}}},
            {"type":"rule","id":"gz","matchers":[{"type":"path","pattern":{"op":"equals","value":"/gzip"}}],
             "action":{"type":"passthrough","response":{"body":{"type":"match-replace","replacements":[{"find":"hello","replace":"howdy"}]}}}}
        ]}"#,
    );
    let mut proxy = start_proxy(rules, Arc::new(NoBreakpoints)).await;
    let http = client(&proxy);
    // The body matcher sees the decoded request body (buffered before evaluation).
    let gz = akev_content::decode::encode_body(b"the secret word", &["gzip".to_string()]).unwrap();
    let res = http
        .post(format!("http://{upstream}/echo"))
        .header("content-encoding", "gzip")
        .body(gz.to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(res.text().await.unwrap(), "matched on body");
    let res = http
        .post(format!("http://{upstream}/echo"))
        .body("nothing to see")
        .send()
        .await
        .unwrap();
    assert!(res.text().await.unwrap().ends_with("\n---\nnothing to see"));

    // The gzip response is decoded, edited and re-encoded with a correct Content-Length.
    let raw = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(proxy.url()).unwrap())
        .no_gzip()
        .build()
        .unwrap();
    let res = raw
        .get(format!("http://{upstream}/gzip"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.headers()["content-encoding"], "gzip");
    let len: usize = res.headers()["content-length"]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    let wire = res.bytes().await.unwrap();
    assert_eq!(wire.len(), len);
    let decoded = akev_content::decode::decode_body(&wire, &["gzip".to_string()], 1 << 20).unwrap();
    assert_eq!(&decoded[..], b"howdy gzip world");
    let (_, events) = wait_exchange(&mut proxy, "/gzip", is_completed).await;
    let original = events.iter().find_map(|e| match e {
        ProxyEvent::ResponseModified { original_body, .. } => original_body.clone(),
        _ => None,
    });
    let original = original.expect("original (encoded) body recorded");
    assert_eq!(
        &akev_content::decode::decode_body(&original.bytes, &["gzip".to_string()], 1 << 20)
            .unwrap()[..],
        b"hello gzip world"
    );
}

/// Read from `s` until `buf` contains `needle` (5 s at most).
async fn read_until(s: &mut tokio::net::TcpStream, buf: &mut Vec<u8>, needle: &[u8]) {
    while !buf.windows(needle.len()).any(|w| w == needle) {
        let mut tmp = [0u8; 4096];
        let n = tokio::time::timeout(Duration::from_secs(5), s.read(&mut tmp))
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "timed out waiting for {:?}; got {:?}",
                    String::from_utf8_lossy(needle),
                    String::from_utf8_lossy(buf)
                )
            })
            .unwrap();
        assert!(n > 0, "closed; got {:?}", String::from_utf8_lossy(buf));
        buf.extend_from_slice(&tmp[..n]);
    }
}

fn script_rule(id: &str, source: &str) -> String {
    format!(
        r#"{{"type":"rule","id":"{id}","matchers":[],"action":{{"type":"script","source":{}}}}}"#,
        serde_json::to_string(source).unwrap()
    )
}

#[tokio::test]
async fn single_hook_scripts_do_not_buffer_the_other_direction() {
    // The upstream streams a chunked response and holds it open until the test has seen the
    // first chunk; for uploads it acknowledges the first request chunk before the client sends
    // the rest. Buffering the direction a script doesn't touch deadlocks (the test times out).
    let (release_tx, release_rx) = tokio::sync::watch::channel(false);
    let (upload_tx, upload_rx) = tokio::sync::watch::channel(String::new());
    let upload_tx = Arc::new(upload_tx);
    let upstream = spawn_raw(move |mut s| {
        let mut release = release_rx.clone();
        let upload_tx = upload_tx.clone();
        async move {
            let mut buf = Vec::new();
            read_until(&mut s, &mut buf, b"\r\n\r\n").await;
            let head = String::from_utf8_lossy(&buf).to_string();
            if head.starts_with("POST") {
                read_until(&mut s, &mut buf, b"first").await;
                let _ = upload_tx.send(String::from_utf8_lossy(&buf).to_string());
                read_until(&mut s, &mut buf, b"0\r\n\r\n").await;
                let _ = s
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                    .await;
                return;
            }
            let _ = s
                .write_all(format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nTransfer-Encoding: chunked\r\nX-Echo-Trace: {}\r\n\r\n8\r\n{{\"n\":1}}\n\r\n",
                    head.to_ascii_lowercase().contains("x-trace: 1")
                ).as_bytes())
                .await;
            let _ = s.flush().await;
            let _ = release.wait_for(|r| *r).await;
            let _ = s.write_all(b"8\r\n{\"n\":2}\n\r\n0\r\n\r\n").await;
        }
    })
    .await;

    // onRequest-only script: the response streams through (and the script did run).
    let proxy = start_proxy(rules_with_request_script(), Arc::new(NoBreakpoints)).await;
    let mut tcp = tokio::net::TcpStream::connect(proxy.addr()).await.unwrap();
    tcp.write_all(
        format!("GET http://{upstream}/stream HTTP/1.1\r\nHost: {upstream}\r\n\r\n").as_bytes(),
    )
    .await
    .unwrap();
    let mut buf = Vec::new();
    // The first chunk arrives while the upstream still holds the response open.
    read_until(&mut tcp, &mut buf, b"{\"n\":1}").await;
    release_tx.send(true).unwrap();
    read_until(&mut tcp, &mut buf, b"0\r\n\r\n").await;
    let text = String::from_utf8_lossy(&buf);
    assert!(text.contains("{\"n\":2}"), "{text}");
    assert!(text.contains("X-Echo-Trace: true"), "onRequest ran: {text}");

    // onResponse-only script: the upload streams through (and the script still runs).
    let proxy = start_proxy(rules_with_response_script(), Arc::new(NoBreakpoints)).await;
    let mut tcp = tokio::net::TcpStream::connect(proxy.addr()).await.unwrap();
    tcp.write_all(
        format!("POST http://{upstream}/upload HTTP/1.1\r\nHost: {upstream}\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nfirst\r\n")
            .as_bytes(),
    )
    .await
    .unwrap();
    let mut seen = upload_rx.clone();
    tokio::time::timeout(Duration::from_secs(5), seen.wait_for(|b| !b.is_empty()))
        .await
        .expect("the first request chunk reached the upstream before the rest was sent")
        .unwrap();
    tcp.write_all(b"4\r\nrest\r\n0\r\n\r\n").await.unwrap();
    let mut buf = Vec::new();
    let (head, body) = read_response(&mut tcp, &mut buf).await;
    assert_eq!(body, b"ok");
    assert!(head.to_ascii_lowercase().contains("x-seen: yes"), "{head}");
}

fn rules_with_request_script() -> akev_core::rules::RuleSet {
    rules(&format!(
        r#"{{"items":[{}]}}"#,
        script_rule(
            "req",
            "function onRequest(req) { req.headers['x-trace'] = '1'; return req; }"
        )
    ))
}

fn rules_with_response_script() -> akev_core::rules::RuleSet {
    rules(&format!(
        r#"{{"items":[{}]}}"#,
        script_rule(
            "res",
            "function onResponse(req, res) { res.headers['x-seen'] = 'yes'; return res; }"
        )
    ))
}

#[tokio::test]
async fn on_response_scripts_run_for_bodiless_and_event_stream_responses() {
    let upstream = spawn_raw(|mut s| async move {
        let mut buf = Vec::new();
        while let Some((head, _)) = read_request(&mut s, &mut buf).await {
            let head = String::from_utf8_lossy(&head).to_string();
            let response: &[u8] = if head.starts_with("OPTIONS") {
                b"HTTP/1.1 204 No Content\r\nAllow: GET\r\n\r\n"
            } else if head.starts_with("HEAD") {
                b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 5\r\n\r\n"
            } else if head.contains("/events") {
                let _ = s
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: one\n\n")
                    .await;
                return;
            } else if head.contains("/no-content") {
                b"HTTP/1.1 204 No Content\r\n\r\n"
            } else {
                b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello"
            };
            if s.write_all(response).await.is_err() {
                return;
            }
        }
    })
    .await;
    let rules = rules(&format!(
        r#"{{"items":[{},
            {{"type":"rule","id":"to-200","matchers":[{{"type":"path","pattern":{{"op":"equals","value":"/no-content"}}}}],
              "action":{{"type":"passthrough","response":{{"status":200,"body":{{"type":"replace","body":{{"type":"text","text":"now with a body"}}}}}}}}}}
        ]}}"#,
        script_rule(
            "cors",
            "function onResponse(req, res) { res.headers['Access-Control-Allow-Origin'] = '*'; res.headers['X-Script'] = 'ran'; return res; }"
        )
    ));
    let proxy = start_proxy(rules, Arc::new(NoBreakpoints)).await;
    let mut tcp = tokio::net::TcpStream::connect(proxy.addr()).await.unwrap();
    let mut buf = Vec::new();
    for method in ["OPTIONS", "HEAD", "GET"] {
        tcp.write_all(
            format!("{method} http://{upstream}/api HTTP/1.1\r\nHost: {upstream}\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
        let (head, body) = if method == "HEAD" {
            // No body follows a HEAD response, whatever its Content-Length says.
            read_until(&mut tcp, &mut buf, b"\r\n\r\n").await;
            let end = buf.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
            let head = String::from_utf8_lossy(&buf[..end]).to_string();
            buf.drain(..end);
            (head, Vec::new())
        } else {
            read_response(&mut tcp, &mut buf).await
        };
        let lower = head.to_ascii_lowercase();
        assert!(lower.contains("x-script: ran"), "{method}: {head}");
        assert!(
            lower.contains("access-control-allow-origin: *"),
            "{method}: {head}"
        );
        if method == "HEAD" {
            assert!(lower.contains("content-length: 5"), "{head}");
        }
        if method == "GET" {
            assert_eq!(body, b"hello");
        }
    }

    // Event stream: the head is modified, the stream still flows.
    let mut tcp = tokio::net::TcpStream::connect(proxy.addr()).await.unwrap();
    tcp.write_all(
        format!("GET http://{upstream}/events HTTP/1.1\r\nHost: {upstream}\r\n\r\n").as_bytes(),
    )
    .await
    .unwrap();
    let mut buf = Vec::new();
    read_until(&mut tcp, &mut buf, b"data: one").await;
    let text = String::from_utf8_lossy(&buf).to_ascii_lowercase();
    assert!(text.contains("x-script: ran"), "{text}");

    // A transform turning a 204 into a 200 with a body: the body is sent, correctly framed.
    let (head, body) = raw_exchange(
        &proxy,
        format!("GET http://{upstream}/no-content HTTP/1.1\r\nHost: {upstream}\r\n\r\n").as_bytes(),
    )
    .await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert!(
        head.to_ascii_lowercase().contains("content-length: 15"),
        "{head}"
    );
    assert_eq!(body, b"now with a body");
}

/// Rules are evaluated on the snapshot that decided whether to buffer the body: a rule set
/// swapped in while the body is still arriving doesn't apply to that exchange.
#[tokio::test]
async fn rule_set_swapped_mid_body_does_not_apply_to_the_exchange() {
    let engine = Arc::new(akev_rules::RulesEngine::new(rules(
        r#"{"items":[{"type":"rule","id":"by-body","matchers":[{"type":"body","pattern":{"op":"contains","value":"hello"}}],
            "action":{"type":"mock","status":201,"body":{"type":"text","text":"old rules"}}}]}"#,
    )));
    let mut proxy = start_proxy_engine(
        engine.clone(),
        Arc::new(NoBreakpoints),
        proxy_settings(),
        akev_core::settings::CaptureSettings {
            detect_processes: false,
            ..Default::default()
        },
    )
    .await;
    let mut tcp = tokio::net::TcpStream::connect(proxy.addr()).await.unwrap();
    tcp.write_all(
        b"POST http://api.example.test/swap HTTP/1.1\r\nHost: api.example.test\r\nContent-Length: 10\r\n\r\nhello",
    )
    .await
    .unwrap();
    // The proxy has the head and is buffering the body for the body matcher.
    proxy
        .wait_for(
            Duration::from_secs(10),
            |e| matches!(e, ProxyEvent::RequestStarted { head, .. } if head.path == "/swap"),
        )
        .await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    engine.set_rules(rules(
        r#"{"items":[{"type":"rule","id":"catch-all","matchers":[],
            "action":{"type":"mock","status":202,"body":{"type":"text","text":"new rules"}}}]}"#,
    ));
    tcp.write_all(b"world").await.unwrap();
    let mut buf = Vec::new();
    let (head, body) = read_response(&mut tcp, &mut buf).await;
    assert!(head.starts_with("HTTP/1.1 201"), "{head}");
    assert_eq!(body, b"old rules");

    // The next exchange sees the new rule set.
    let (head, body) = raw_exchange(
        &proxy,
        b"GET http://api.example.test/next HTTP/1.1\r\nHost: api.example.test\r\n\r\n",
    )
    .await;
    assert!(head.starts_with("HTTP/1.1 202"), "{head}");
    assert_eq!(body, b"new rules");
}

fn notices(events: &[ProxyEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            ProxyEvent::Notice { message, .. } => Some(message.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn bodies_too_large_for_the_rule_budget_pass_through_untouched() {
    let upstream = spawn_http_upstream().await;
    let rules = rules(
        r#"{"items":[
            {"type":"rule","id":"by-body","matchers":[{"type":"body","pattern":{"op":"contains","value":"secret"}}],
             "action":{"type":"mock","status":200,"body":{"type":"text","text":"matched on body"}}},
            {"type":"rule","id":"big","matchers":[{"type":"path","pattern":{"op":"equals","value":"/big"}}],
             "action":{"type":"passthrough","response":{"setHeaders":[["x-seen","1"]],"body":{"type":"match-replace","replacements":[{"find":"x","replace":"y"}]}}}}
        ]}"#,
    );
    let settings = akev_core::settings::ProxySettings {
        rule_body_budget_bytes: 50_000,
        ..proxy_settings()
    };
    let capture = akev_core::settings::CaptureSettings {
        detect_processes: false,
        ..Default::default()
    };
    let mut proxy = start_proxy_with(rules, Arc::new(NoBreakpoints), settings, capture).await;
    let http = client(&proxy);

    // A body that fits is matched as usual.
    let res = http
        .post(format!("http://{upstream}/hello"))
        .body("the secret word")
        .send()
        .await
        .unwrap();
    assert_eq!(res.text().await.unwrap(), "matched on body");
    let (_, events) = wait_exchange(&mut proxy, "/hello", is_completed).await;
    assert!(notices(&events).is_empty());

    // One over the budget streams to the server untouched (the rule can't see it) instead of
    // failing, and the exchange says why.
    let big = format!("{}secret", "a".repeat(80_000));
    let res = http
        .post(format!("http://{upstream}/echo"))
        .body(big.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert!(
        res.text()
            .await
            .unwrap()
            .ends_with(&format!("\n---\n{big}"))
    );
    let (_, events) = wait_exchange(&mut proxy, "/echo", is_completed).await;
    assert_eq!(
        notices(&events),
        vec!["Body too large to apply rules; passed through".to_string()]
    );
    let recorded = events.iter().find_map(|e| match e {
        ProxyEvent::RequestCompleted { body, .. } => Some(body.clone()),
        _ => None,
    });
    assert_eq!(
        recorded.unwrap().wire_size,
        big.len() as u64,
        "still captured"
    );

    // The response side too: the 100 KB response arrives unchanged (the transform that
    // needed its body is skipped as a whole, header edits included).
    let res = http
        .get(format!("http://{upstream}/big"))
        .send()
        .await
        .unwrap();
    assert!(res.headers().get("x-seen").is_none());
    let body = res.bytes().await.unwrap();
    assert_eq!(body.len(), 100_000);
    assert!(body.iter().all(|b| *b == b'x'));
    let (_, events) = wait_exchange(&mut proxy, "/big", is_completed).await;
    assert_eq!(
        notices(&events),
        vec!["Body too large to apply rules; passed through".to_string()]
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, ProxyEvent::Aborted { .. })),
        "never fails the exchange"
    );
    let stats = proxy.handle.stats();
    assert_eq!(stats.passed_through, 2);
    assert_eq!(
        stats.rule_body_bytes, 0,
        "budget returned after each exchange"
    );
}
