//! End-to-end: requests issued by Akev itself (`ProxyHandle::send`) and proxy lifecycle
//! (port fallback, rebind, shutdown).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use akev_core::settings::CaptureSettings;
use akev_core::*;
use akev_proxy::SendOptions;
use bytes::Bytes;
use common::*;
use std::sync::Arc;
use std::time::Duration;

fn editable(method: &str, url: String, body: &'static [u8]) -> EditableRequest {
    EditableRequest {
        method: method.into(),
        url,
        headers: Headers(vec![("X-From".into(), "compose".into())]),
        body: Bytes::from_static(body),
    }
}

#[tokio::test]
async fn send_records_a_composer_exchange() {
    let upstream = spawn_http_upstream().await;
    let mut proxy = start_proxy(
        akev_core::rules::RuleSet::default(),
        Arc::new(NoBreakpoints),
    )
    .await;
    let result = proxy
        .handle
        .send(
            editable("POST", format!("http://{upstream}/echo"), b"abc"),
            SendOptions::default(),
        )
        .await;
    assert!(result.error.is_none(), "{:?}", result.error);
    assert_eq!(result.response.as_ref().map(|r| r.status), Some(200));
    let body = String::from_utf8_lossy(&result.body);
    assert!(
        body.contains("X-From: compose\r\n") && body.contains("Content-Length: 3\r\n"),
        "{body}"
    );
    assert!(body.ends_with("\n---\nabc"));
    assert!(result.timing.response_headers.is_some() && result.timing.response_body.is_some());

    let (id, events) = wait_exchange(&mut proxy, "/echo", is_completed).await;
    assert_eq!(id, result.id);
    let ProxyEvent::RequestStarted { client, head, .. } = &events[0] else {
        panic!()
    };
    assert_eq!(client.via, ProxyMode::Composer);
    assert_eq!(head.method, "POST");
}

#[tokio::test]
async fn send_follows_redirects_as_separate_exchanges() {
    let upstream = spawn_http_upstream().await;
    let mut proxy = start_proxy(
        akev_core::rules::RuleSet::default(),
        Arc::new(NoBreakpoints),
    )
    .await;
    let options = SendOptions {
        follow_redirects: true,
        ..Default::default()
    };
    let result = proxy
        .handle
        .send(
            editable("GET", format!("http://{upstream}/redirect"), b""),
            options,
        )
        .await;
    assert_eq!(result.response.as_ref().map(|r| r.status), Some(200));
    assert_eq!(&result.body[..], b"hello");
    let (first, _) = wait_exchange(&mut proxy, "/redirect", is_completed).await;
    let (second, _) = wait_exchange(&mut proxy, "/hello", is_completed).await;
    assert!(second > first);
    assert_eq!(second, result.id);

    // Without following, the redirect itself is returned.
    let result = proxy
        .handle
        .send(
            editable("GET", format!("http://{upstream}/redirect"), b""),
            SendOptions::default(),
        )
        .await;
    assert_eq!(result.response.as_ref().map(|r| r.status), Some(302));
}

#[tokio::test]
async fn send_reports_errors_and_can_apply_rules() {
    let closed = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let rules = rules(
        r#"{"items":[{"type":"rule","id":"m","matchers":[{"type":"path","pattern":{"op":"equals","value":"/mocked"}}],"action":{"type":"mock","status":202,"body":{"type":"text","text":"rule says hi"}}}]}"#,
    );
    let mut proxy = start_proxy(rules, Arc::new(NoBreakpoints)).await;
    let result = proxy
        .handle
        .send(
            editable("GET", format!("http://{closed}/down"), b""),
            SendOptions::default(),
        )
        .await;
    assert_eq!(
        result.error.as_ref().map(|e| e.kind),
        Some(ErrorKind::ConnectionRefused)
    );
    assert!(result.response.is_none() && result.body.is_empty());
    let (_, events) = wait_exchange(&mut proxy, "/down", is_completed).await;
    assert!(matches!(events.last(), Some(ProxyEvent::Aborted { .. })));

    let invalid = proxy
        .handle
        .send(
            editable("GET", "not a url".into(), b""),
            SendOptions::default(),
        )
        .await;
    assert_eq!(
        invalid.error.as_ref().map(|e| e.kind),
        Some(ErrorKind::ClientProtocolError)
    );

    let mocked = proxy
        .handle
        .send(
            editable("GET", format!("http://{closed}/mocked"), b""),
            SendOptions {
                apply_rules: true,
                ..Default::default()
            },
        )
        .await;
    assert_eq!(mocked.response.as_ref().map(|r| r.status), Some(202));
    assert_eq!(&mocked.body[..], b"rule says hi");
    // Rules are opt-in for Compose.
    let unmocked = proxy
        .handle
        .send(
            editable("GET", format!("http://{closed}/mocked"), b""),
            SendOptions::default(),
        )
        .await;
    assert!(unmocked.error.is_some());
}

#[tokio::test]
async fn busy_port_falls_back_to_the_next_one() {
    let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = busy.local_addr().unwrap().port();
    if port > u16::MAX - 100 {
        return;
    }
    let mut settings = proxy_settings();
    settings.port = port;
    let proxy = start_proxy_with(
        akev_core::rules::RuleSet::default(),
        Arc::new(NoBreakpoints),
        settings,
        CaptureSettings::default(),
    )
    .await;
    assert!(
        proxy.handle.port() > port && proxy.handle.port() <= port + 100,
        "{port} → {}",
        proxy.handle.port()
    );
}

#[tokio::test]
async fn rebind_and_shutdown() {
    let upstream = spawn_http_upstream().await;
    let mut proxy = start_proxy(
        akev_core::rules::RuleSet::default(),
        Arc::new(NoBreakpoints),
    )
    .await;
    let old = proxy.addr();
    let new = proxy.handle.rebind(0, "127.0.0.1").await.expect("rebind");
    assert_ne!(old, new);
    assert_eq!(proxy.addr(), new);
    let http = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(format!("http://{new}")).unwrap())
        .build()
        .unwrap();
    assert_eq!(
        http.get(format!("http://{upstream}/hello"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "hello"
    );
    // The old listener is gone.
    assert!(tokio::net::TcpStream::connect(old).await.is_err());
    wait_exchange(&mut proxy, "/hello", is_completed).await;

    // Rebinding to the current address is a no-op.
    assert_eq!(
        proxy.handle.rebind(new.port(), "127.0.0.1").await.unwrap(),
        new
    );

    // An idle keep-alive connection doesn't hold up shutdown.
    let idle = tokio::net::TcpStream::connect(new).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), proxy.handle.shutdown())
        .await
        .expect("shutdown completes promptly");
    assert!(tokio::net::TcpStream::connect(new).await.is_err());
    drop(idle);
    assert_eq!(proxy.handle.stats().active_connections, 0);
}
