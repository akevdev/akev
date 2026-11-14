//! HTTP/1.1 upstream client tests against in-process raw servers.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/upstream.rs"]
mod upstream_support;

use akev_core::settings::{DnsOverride, ProxySettings};
use akev_core::{ErrorKind, Headers, HttpVersion};
use akev_proxy::upstream::{BodyChunk, BodyStream, RequestBody, RequestOptions, UpstreamError};
use bytes::{Bytes, BytesMut};
use futures::StreamExt;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use upstream_support::*;

fn stream_of(chunks: Vec<BodyChunk>) -> BodyStream {
    Box::pin(futures::stream::iter(
        chunks.into_iter().map(Ok::<_, UpstreamError>),
    ))
}

#[tokio::test]
async fn preserves_request_header_casing_order_and_duplicates() {
    let (addr, _) = spawn_http1(|req, _| echo_response(req)).await;
    let c = client(&ProxySettings::default());
    let url = format!("http://127.0.0.1:{}/path?q=1", addr.port());
    let h = head(
        "GET",
        &url,
        &[
            ("X-Custom-CASE", "1"),
            ("host", "example.test"),
            ("accept", "*/*"),
            ("X-Custom-CASE", "2"),
            ("x-ÜTF", "ok"),
        ],
    );
    // A non-token header name must be dropped rather than corrupting the request.
    let resp = c.send(&h, RequestBody::Empty, &opts()).await.unwrap();
    assert_eq!(resp.head.status, 200);
    let (body, _) = collect(resp.body).await;
    let text = String::from_utf8(body).unwrap();
    assert_eq!(
        text,
        "GET /path?q=1 HTTP/1.1\r\nX-Custom-CASE: 1\r\nhost: example.test\r\naccept: */*\r\nX-Custom-CASE: 2\r\n\r\n"
    );
}

#[tokio::test]
async fn inserts_missing_host_first_and_fixes_content_length() {
    let (addr, _) = spawn_http1(|req, _| echo_response(req)).await;
    let c = client(&ProxySettings::default());
    let url = format!("http://127.0.0.1:{}/submit", addr.port());
    let h = head(
        "POST",
        &url,
        &[
            ("content-length", "999"),
            ("X-A", "b"),
            ("Content-Length", "5"),
        ],
    );
    let resp = c
        .send(&h, RequestBody::Full(Bytes::from_static(b"hello")), &opts())
        .await
        .unwrap();
    let (body, _) = collect(resp.body).await;
    let text = String::from_utf8(body).unwrap();
    assert_eq!(
        text,
        format!(
            "POST /submit HTTP/1.1\r\nHost: 127.0.0.1:{}\r\ncontent-length: 5\r\nX-A: b\r\n\r\nhello",
            addr.port()
        )
    );

    // POST without a body still gets Content-Length: 0 (servers reply 411 otherwise).
    let h = head("POST", &url, &[("Host", "x")]);
    let resp = c.send(&h, RequestBody::Empty, &opts()).await.unwrap();
    let (body, _) = collect(resp.body).await;
    assert!(
        String::from_utf8(body)
            .unwrap()
            .contains("Content-Length: 0\r\n")
    );

    // GET without a body gets no framing headers at all.
    let h = head("GET", &url, &[("Host", "x")]);
    let resp = c.send(&h, RequestBody::Empty, &opts()).await.unwrap();
    let (body, _) = collect(resp.body).await;
    assert_eq!(
        String::from_utf8(body).unwrap(),
        "GET /submit HTTP/1.1\r\nHost: x\r\n\r\n"
    );
}

#[tokio::test]
async fn header_injection_is_dropped() {
    let (addr, _) = spawn_http1(|req, _| echo_response(req)).await;
    let c = client(&ProxySettings::default());
    let url = format!("http://127.0.0.1:{}/", addr.port());
    let h = head(
        "GET",
        &url,
        &[
            ("Host", "x"),
            ("X-Evil", "a\r\nInjected: yes"),
            ("X-Good", "ok"),
        ],
    );
    let resp = c.send(&h, RequestBody::Empty, &opts()).await.unwrap();
    let text = String::from_utf8(collect(resp.body).await.0).unwrap();
    assert!(!text.contains("Injected"));
    assert!(text.contains("X-Good: ok"));
}

#[tokio::test]
async fn title_cases_when_not_preserving_case() {
    let (addr, _) = spawn_http1(|req, _| echo_response(req)).await;
    let c = client(&ProxySettings {
        preserve_header_case: false,
        ..Default::default()
    });
    let url = format!("http://127.0.0.1:{}/", addr.port());
    let h = head(
        "GET",
        &url,
        &[("host", "x"), ("x-custom-header", "v"), ("ACCEPT", "*/*")],
    );
    let resp = c.send(&h, RequestBody::Empty, &opts()).await.unwrap();
    let text = String::from_utf8(collect(resp.body).await.0).unwrap();
    assert_eq!(
        text,
        "GET / HTTP/1.1\r\nHost: x\r\nX-Custom-Header: v\r\nAccept: */*\r\n\r\n"
    );
}

#[tokio::test]
async fn streams_unknown_length_body_as_chunked_with_trailers() {
    let (addr, _) = spawn_http1(|req, _| {
        let body = format!(
            "chunked={} body={} trailers={:?}",
            req.chunked,
            String::from_utf8_lossy(&req.body),
            req.trailers
        );
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    })
    .await;
    let c = client(&ProxySettings::default());
    let url = format!("http://127.0.0.1:{}/upload", addr.port());
    let h = head("POST", &url, &[("Host", "x")]);
    let mut trailers = Headers::new();
    trailers.append("X-Checksum", "abc");
    let body = stream_of(vec![
        BodyChunk::Data(Bytes::from_static(b"hello ")),
        BodyChunk::Data(Bytes::new()),
        BodyChunk::Data(Bytes::from_static(b"world")),
        BodyChunk::Trailers(trailers),
    ]);
    let resp = c
        .send(
            &h,
            RequestBody::Stream {
                stream: body,
                length: None,
            },
            &opts(),
        )
        .await
        .unwrap();
    let text = String::from_utf8(collect(resp.body).await.0).unwrap();
    assert_eq!(
        text,
        r#"chunked=true body=hello world trailers=[("X-Checksum", "abc")]"#
    );
    assert!(resp.request_sent_ms.is_some());
}

#[tokio::test]
async fn stream_with_known_length_uses_content_length() {
    let (addr, _) = spawn_http1(|req, _| echo_response(req)).await;
    let c = client(&ProxySettings::default());
    let url = format!("http://127.0.0.1:{}/", addr.port());
    let h = head("PUT", &url, &[("Host", "x")]);
    let body = stream_of(vec![
        BodyChunk::Data(Bytes::from_static(b"abc")),
        BodyChunk::Data(Bytes::from_static(b"de")),
    ]);
    let resp = c
        .send(
            &h,
            RequestBody::Stream {
                stream: body,
                length: Some(5),
            },
            &opts(),
        )
        .await
        .unwrap();
    let text = String::from_utf8(collect(resp.body).await.0).unwrap();
    assert_eq!(
        text,
        "PUT / HTTP/1.1\r\nHost: x\r\nContent-Length: 5\r\n\r\nabcde"
    );
}

#[tokio::test]
async fn large_full_body_is_written_concurrently() {
    let (addr, _) = spawn_http1(|req, _| {
        let body = format!("{}", req.body.len());
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    })
    .await;
    let c = client(&ProxySettings::default());
    let url = format!("http://127.0.0.1:{}/", addr.port());
    let payload = Bytes::from(vec![b'x'; 3 * 1024 * 1024]);
    let resp = c
        .send(
            &head("POST", &url, &[("Host", "x")]),
            RequestBody::Full(payload),
            &opts(),
        )
        .await
        .unwrap();
    assert_eq!(collect(resp.body).await.0, b"3145728");
}

#[tokio::test]
async fn response_casing_reason_chunked_body_and_trailers() {
    let (addr, _) = spawn_server(|mut s, _| async move {
        let mut buf = BytesMut::new();
        read_request(&mut s, &mut buf).await;
        s.write_all(
            b"HTTP/1.1 201 Created Here\r\nx-LoWeR: a\r\nX-UPPER: b\r\nx-lower: c\r\nTransfer-Encoding: chunked\r\nTrailer: X-Checksum\r\n\r\n5\r\nhello\r\n6;ext=1\r\n world\r\n0\r\nX-Checksum: abc\r\nX-Other: d\r\n\r\n",
        )
        .await
        .unwrap();
    })
    .await;
    let c = client(&ProxySettings::default());
    let resp = c
        .send(
            &head("GET", &format!("http://127.0.0.1:{}/", addr.port()), &[]),
            RequestBody::Empty,
            &opts(),
        )
        .await
        .unwrap();
    assert_eq!(resp.head.status, 201);
    assert_eq!(resp.head.status_message, "Created Here");
    assert_eq!(resp.head.http_version, HttpVersion::Http11);
    let names: Vec<&str> = resp.head.headers.iter().map(|(k, _)| k).collect();
    assert_eq!(
        names,
        [
            "x-LoWeR",
            "X-UPPER",
            "x-lower",
            "Transfer-Encoding",
            "Trailer"
        ]
    );
    let (body, trailers) = collect(resp.body).await;
    assert_eq!(body, b"hello world");
    let trailers = trailers.unwrap();
    assert_eq!(
        trailers.0,
        vec![
            ("X-Checksum".to_string(), "abc".to_string()),
            ("X-Other".to_string(), "d".to_string())
        ]
    );
}

#[tokio::test]
async fn chunked_response_split_into_single_bytes() {
    let (addr, _) = spawn_server(|mut s, _| async move {
        let mut buf = BytesMut::new();
        read_request(&mut s, &mut buf).await;
        let response = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\nA\r\n0123456789\r\n0\r\n\r\n";
        for b in response.iter() {
            s.write_all(&[*b]).await.unwrap();
            s.flush().await.unwrap();
            tokio::time::sleep(Duration::from_micros(200)).await;
        }
    })
    .await;
    let c = client(&ProxySettings::default());
    let resp = c
        .send(
            &head("GET", &format!("http://127.0.0.1:{}/", addr.port()), &[]),
            RequestBody::Empty,
            &opts(),
        )
        .await
        .unwrap();
    let (body, trailers) = collect(resp.body).await;
    assert_eq!(body, b"abc0123456789");
    assert!(trailers.is_none());
}

#[tokio::test]
async fn body_is_streamed_as_it_arrives() {
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let release_rx = Arc::new(parking_lot::Mutex::new(Some(release_rx)));
    let (addr, _) = spawn_server(move |mut s, _| {
        let release = release_rx.lock().take();
        async move {
            let mut buf = BytesMut::new();
            read_request(&mut s, &mut buf).await;
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nfirst")
                .await
                .unwrap();
            if let Some(rx) = release {
                let _ = rx.await;
            }
            s.write_all(b"-last").await.unwrap();
        }
    })
    .await;
    let c = client(&ProxySettings::default());
    let resp = c
        .send(
            &head("GET", &format!("http://127.0.0.1:{}/", addr.port()), &[]),
            RequestBody::Empty,
            &opts(),
        )
        .await
        .unwrap();
    let mut body = resp.body;
    let first = tokio::time::timeout(Duration::from_secs(2), body.next())
        .await
        .expect("first chunk streamed")
        .unwrap()
        .unwrap();
    assert!(matches!(first, BodyChunk::Data(ref d) if &d[..] == b"first"));
    release_tx.send(()).unwrap();
    let rest = body.next().await.unwrap().unwrap();
    assert!(matches!(rest, BodyChunk::Data(ref d) if &d[..] == b"-last"));
    assert!(body.next().await.is_none());
}

#[tokio::test]
async fn close_delimited_body_and_no_reuse() {
    let (addr, accepted) = spawn_server(|mut s, _| async move {
        let mut buf = BytesMut::new();
        read_request(&mut s, &mut buf).await;
        s.write_all(b"HTTP/1.1 200 OK\r\nX-Kind: close-delimited\r\n\r\nall of it")
            .await
            .unwrap();
    })
    .await;
    let c = client(&ProxySettings::default());
    let url = format!("http://127.0.0.1:{}/", addr.port());
    for _ in 0..2 {
        let resp = c
            .send(&head("GET", &url, &[]), RequestBody::Empty, &opts())
            .await
            .unwrap();
        assert!(!resp.info.reused);
        assert_eq!(collect(resp.body).await.0, b"all of it");
    }
    assert_eq!(accepted.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn head_204_304_have_no_body_and_keep_the_connection() {
    let (addr, accepted) = spawn_http1(|req, _| match req.target.as_str() {
        "/204" => b"HTTP/1.1 204 No Content\r\n\r\n".to_vec(),
        "/304" => b"HTTP/1.1 304 Not Modified\r\nContent-Length: 42\r\n\r\n".to_vec(),
        _ => b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\n".to_vec(),
    })
    .await;
    let c = client(&ProxySettings::default());
    let base = format!("http://127.0.0.1:{}", addr.port());
    let resp = c
        .send(
            &head("HEAD", &format!("{base}/"), &[]),
            RequestBody::Empty,
            &opts(),
        )
        .await
        .unwrap();
    assert_eq!(resp.head.headers.content_length(), Some(1000));
    assert!(collect(resp.body).await.0.is_empty());
    for path in ["/204", "/304"] {
        let resp = c
            .send(
                &head("GET", &format!("{base}{path}"), &[]),
                RequestBody::Empty,
                &opts(),
            )
            .await
            .unwrap();
        assert!(resp.info.reused);
        assert!(collect(resp.body).await.0.is_empty());
    }
    assert_eq!(accepted.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn pooled_connections_are_reused_with_flags_and_timings() {
    let (addr, accepted) = spawn_http1(|req, _| echo_response(req)).await;
    let c = client(&ProxySettings::default());
    let url = format!("http://127.0.0.1:{}/", addr.port());
    let first = c
        .send(&head("GET", &url, &[]), RequestBody::Empty, &opts())
        .await
        .unwrap();
    assert!(!first.info.reused);
    assert!(first.timings.dns_ms.is_some() && first.timings.connect_ms.is_some());
    assert!(first.timings.tls_ms.is_none());
    assert_eq!(
        first.info.remote_addr.as_deref(),
        Some(addr.to_string().as_str())
    );
    assert!(first.request_sent_ms.is_some());
    collect(first.body).await;
    for _ in 0..3 {
        let next = c
            .send(&head("GET", &url, &[]), RequestBody::Empty, &opts())
            .await
            .unwrap();
        assert!(next.info.reused);
        assert!(next.timings.dns_ms.is_none() && next.timings.connect_ms.is_none());
        collect(next.body).await;
    }
    assert_eq!(accepted.load(Ordering::SeqCst), 1);

    // An unread body keeps the connection busy: a concurrent request opens a new one.
    let busy = c
        .send(&head("GET", &url, &[]), RequestBody::Empty, &opts())
        .await
        .unwrap();
    let other = c
        .send(&head("GET", &url, &[]), RequestBody::Empty, &opts())
        .await
        .unwrap();
    assert!(!other.info.reused);
    drop(busy);
    collect(other.body).await;
    assert_eq!(accepted.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn connection_close_is_not_pooled() {
    let (addr, accepted) = spawn_http1(|_, _| {
        b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 2\r\n\r\nok".to_vec()
    })
    .await;
    let c = client(&ProxySettings::default());
    let url = format!("http://127.0.0.1:{}/", addr.port());
    for _ in 0..2 {
        let resp = c
            .send(&head("GET", &url, &[]), RequestBody::Empty, &opts())
            .await
            .unwrap();
        assert!(!resp.info.reused);
        collect(resp.body).await;
    }
    assert_eq!(accepted.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn stale_pooled_connection_is_retried_transparently() {
    // Connection 0 answers one request, then drops the next one without responding (an idle
    // timeout racing with our reuse). The client must retry on a fresh connection.
    let (addr, accepted) = spawn_server(|mut s, n| async move {
        let mut buf = BytesMut::new();
        let Some(req) = read_request(&mut s, &mut buf).await else {
            return;
        };
        s.write_all(&echo_response(&req)).await.unwrap();
        if n == 0 {
            let _ = read_request(&mut s, &mut buf).await;
            return;
        }
        while let Some(req) = read_request(&mut s, &mut buf).await {
            s.write_all(&echo_response(&req)).await.unwrap();
        }
    })
    .await;
    let c = client(&ProxySettings::default());
    let url = format!("http://127.0.0.1:{}/", addr.port());
    let first = c
        .send(&head("GET", &url, &[]), RequestBody::Empty, &opts())
        .await
        .unwrap();
    collect(first.body).await;
    let second = c
        .send(
            &head("POST", &url, &[]),
            RequestBody::Full(Bytes::from_static(b"replayed")),
            &opts(),
        )
        .await
        .unwrap();
    assert!(!second.info.reused);
    let text = String::from_utf8(collect(second.body).await.0).unwrap();
    assert!(text.ends_with("replayed"));
    assert_eq!(accepted.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn interim_responses_are_skipped_and_expect_continue_works() {
    let (addr, _) = spawn_server(|mut s, _| async move {
        let mut buf = BytesMut::new();
        let mut req = read_request_head(&mut s, &mut buf).await.unwrap();
        assert_eq!(req.header("expect"), Some("100-continue"));
        s.write_all(b"HTTP/1.1 100 Continue\r\n\r\n").await.unwrap();
        read_request_body(&mut s, &mut buf, &mut req).await;
        let body = String::from_utf8_lossy(&req.body).into_owned();
        s.write_all(
            format!("HTTP/1.1 103 Early Hints\r\nLink: </a.css>\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len())
                .as_bytes(),
        )
        .await
        .unwrap();
    })
    .await;
    let c = client(&ProxySettings::default());
    let h = head(
        "POST",
        &format!("http://127.0.0.1:{}/", addr.port()),
        &[("Host", "x"), ("Expect", "100-continue")],
    );
    let started = std::time::Instant::now();
    let resp = c
        .send(
            &h,
            RequestBody::Full(Bytes::from_static(b"payload")),
            &opts(),
        )
        .await
        .unwrap();
    assert_eq!(resp.head.status, 200);
    assert_eq!(collect(resp.body).await.0, b"payload");
    // The body was sent on 100 Continue, not after the 1 s fallback.
    assert!(started.elapsed() < Duration::from_millis(900));
}

#[tokio::test]
async fn expect_continue_rejection_skips_the_body() {
    let got_body = Arc::new(parking_lot::Mutex::new(None::<usize>));
    let seen = got_body.clone();
    let (addr, _) = spawn_server(move |mut s, _| {
        let seen = seen.clone();
        async move {
            let mut buf = BytesMut::new();
            read_request_head(&mut s, &mut buf).await.unwrap();
            s.write_all(b"HTTP/1.1 417 Expectation Failed\r\nContent-Length: 4\r\n\r\nnope")
                .await
                .unwrap();
            let mut rest = vec![0u8; 64];
            let n = tokio::time::timeout(Duration::from_millis(1500), s.read(&mut rest))
                .await
                .ok()
                .and_then(|r| r.ok());
            *seen.lock() = Some(n.unwrap_or(0) + buf.len());
        }
    })
    .await;
    let c = client(&ProxySettings::default());
    let h = head(
        "PUT",
        &format!("http://127.0.0.1:{}/", addr.port()),
        &[("Host", "x"), ("Expect", "100-continue")],
    );
    let resp = c
        .send(
            &h,
            RequestBody::Full(Bytes::from_static(b"never sent")),
            &opts(),
        )
        .await
        .unwrap();
    assert_eq!(resp.head.status, 417);
    assert_eq!(collect(resp.body).await.0, b"nope");
    tokio::time::sleep(Duration::from_millis(1700)).await;
    assert_eq!(*got_body.lock(), Some(0));
}

#[tokio::test]
async fn early_response_during_streaming_upload_is_delivered() {
    let (addr, _) = spawn_server(|mut s, _| async move {
        let mut buf = BytesMut::new();
        read_request_head(&mut s, &mut buf).await.unwrap();
        s.write_all(
            b"HTTP/1.1 413 Payload Too Large\r\nConnection: close\r\nContent-Length: 3\r\n\r\nbig",
        )
        .await
        .unwrap();
        // Close without reading the body.
    })
    .await;
    let c = client(&ProxySettings::default());
    let chunks = futures::stream::unfold(0u32, |i| async move {
        if i >= 200 {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
        Some((
            Ok::<_, UpstreamError>(BodyChunk::Data(Bytes::from(vec![b'z'; 64 * 1024]))),
            i + 1,
        ))
    });
    let h = head(
        "POST",
        &format!("http://127.0.0.1:{}/upload", addr.port()),
        &[("Host", "x")],
    );
    let resp = c
        .send(
            &h,
            RequestBody::Stream {
                stream: Box::pin(chunks),
                length: None,
            },
            &opts(),
        )
        .await
        .unwrap();
    assert_eq!(resp.head.status, 413);
    assert_eq!(collect(resp.body).await.0, b"big");
}

#[tokio::test]
async fn response_head_timeout() {
    let (addr, _) = spawn_server(|mut s, _| async move {
        let mut buf = BytesMut::new();
        read_request(&mut s, &mut buf).await;
        tokio::time::sleep(Duration::from_secs(10)).await;
    })
    .await;
    let c = client(&ProxySettings::default());
    let options = RequestOptions {
        timeout_ms: 200,
        ..Default::default()
    };
    let started = std::time::Instant::now();
    let err = c
        .send(
            &head("GET", &format!("http://127.0.0.1:{}/", addr.port()), &[]),
            RequestBody::Empty,
            &options,
        )
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Timeout);
    assert_eq!(err.code.as_deref(), Some("ETIMEDOUT"));
    assert!(started.elapsed() < Duration::from_secs(3));

    // settings.response_timeout_ms applies when the request doesn't override it.
    let c = client(&ProxySettings {
        response_timeout_ms: 200,
        ..Default::default()
    });
    let err = c
        .send(
            &head("GET", &format!("http://127.0.0.1:{}/", addr.port()), &[]),
            RequestBody::Empty,
            &opts(),
        )
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Timeout);
}

#[tokio::test]
async fn idle_body_timeout() {
    let (addr, _) = spawn_server(|mut s, _| async move {
        let mut buf = BytesMut::new();
        read_request(&mut s, &mut buf).await;
        s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\npartial")
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_secs(10)).await;
    })
    .await;
    let c = client(&ProxySettings {
        body_idle_timeout_ms: 300,
        ..Default::default()
    });
    let resp = c
        .send(
            &head("GET", &format!("http://127.0.0.1:{}/", addr.port()), &[]),
            RequestBody::Empty,
            &opts(),
        )
        .await
        .unwrap();
    let mut body = resp.body;
    assert!(matches!(body.next().await, Some(Ok(BodyChunk::Data(_)))));
    let err = tokio::time::timeout(Duration::from_secs(3), body.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Timeout);
}

#[tokio::test]
async fn streaming_bodies_outlive_the_response_timeout() {
    // The response timeout covers the wait for the head; a body that keeps arriving is only
    // subject to the (longer) idle timeout, however long it streams overall.
    let (addr, _) = spawn_server(|mut s, _| async move {
        let mut buf = BytesMut::new();
        read_request(&mut s, &mut buf).await;
        s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\n\r\n")
            .await
            .unwrap();
        for piece in [b"ab", b"cd", b"ef"] {
            tokio::time::sleep(Duration::from_millis(250)).await;
            s.write_all(piece).await.unwrap();
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    })
    .await;
    let c = client(&ProxySettings {
        response_timeout_ms: 200,
        body_idle_timeout_ms: 2_000,
        ..Default::default()
    });
    let resp = c
        .send(
            &head("GET", &format!("http://127.0.0.1:{}/", addr.port()), &[]),
            RequestBody::Empty,
            &opts(),
        )
        .await
        .unwrap();
    let mut body = resp.body;
    let mut got = Vec::new();
    while let Some(chunk) = tokio::time::timeout(Duration::from_secs(3), body.next())
        .await
        .unwrap()
    {
        if let BodyChunk::Data(d) = chunk.unwrap() {
            got.extend_from_slice(&d);
        }
    }
    assert_eq!(got, b"abcdef");
}

#[tokio::test]
async fn truncated_body_is_a_reset() {
    let (addr, _) = spawn_server(|mut s, _| async move {
        let mut buf = BytesMut::new();
        read_request(&mut s, &mut buf).await;
        s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nshort")
            .await
            .unwrap();
    })
    .await;
    let c = client(&ProxySettings::default());
    let resp = c
        .send(
            &head("GET", &format!("http://127.0.0.1:{}/", addr.port()), &[]),
            RequestBody::Empty,
            &opts(),
        )
        .await
        .unwrap();
    let mut body = resp.body;
    assert!(matches!(body.next().await, Some(Ok(BodyChunk::Data(_)))));
    let err = body.next().await.unwrap().unwrap_err();
    assert_eq!(err.kind, ErrorKind::ConnectionReset);
}

#[tokio::test]
async fn connection_refused_and_malformed_responses() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let c = client(&ProxySettings::default());
    let err = c
        .send(
            &head("GET", &format!("http://127.0.0.1:{port}/"), &[]),
            RequestBody::Empty,
            &opts(),
        )
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::ConnectionRefused);
    assert_eq!(err.code.as_deref(), Some("ECONNREFUSED"));
    assert_eq!(err.to_exchange_error().kind, ErrorKind::ConnectionRefused);

    let (addr, _) = spawn_server(|mut s, _| async move {
        let mut buf = BytesMut::new();
        read_request(&mut s, &mut buf).await;
        s.write_all(b"SPDY/9 what\r\n\r\n").await.unwrap();
    })
    .await;
    let err = c
        .send(
            &head("GET", &format!("http://127.0.0.1:{}/", addr.port()), &[]),
            RequestBody::Empty,
            &opts(),
        )
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::UpstreamProtocolError);

    let (addr, _) = spawn_server(|mut s, _| async move {
        let mut buf = BytesMut::new();
        read_request(&mut s, &mut buf).await;
        s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nContent-Length: 6\r\n\r\nhello!")
            .await
            .unwrap();
    })
    .await;
    let err = c
        .send(
            &head("GET", &format!("http://127.0.0.1:{}/", addr.port()), &[]),
            RequestBody::Empty,
            &opts(),
        )
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::UpstreamProtocolError);

    // Closed without a response on a fresh connection: a reset, not retried.
    let (addr, _) = spawn_server(|mut s, _| async move {
        let mut buf = BytesMut::new();
        read_request(&mut s, &mut buf).await;
    })
    .await;
    let err = c
        .send(
            &head("GET", &format!("http://127.0.0.1:{}/", addr.port()), &[]),
            RequestBody::Empty,
            &opts(),
        )
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::ConnectionReset);
}

#[tokio::test]
async fn dns_overrides_exact_and_glob_and_localhost() {
    let (addr, _) = spawn_http1(|req, _| echo_response(req)).await;
    let settings = ProxySettings {
        dns_overrides: vec![
            DnsOverride {
                host: "api.example.test".into(),
                address: "127.0.0.1".into(),
            },
            DnsOverride {
                host: "*.wild.test".into(),
                address: "127.0.0.1".into(),
            },
        ],
        ..Default::default()
    };
    let c = client(&settings);
    for host in ["api.example.test", "deep.sub.wild.test", "localhost"] {
        let url = format!("http://{host}:{}/x", addr.port());
        let resp = c
            .send(&head("GET", &url, &[]), RequestBody::Empty, &opts())
            .await
            .unwrap();
        let text = String::from_utf8(collect(resp.body).await.0).unwrap();
        assert!(
            text.contains(&format!("Host: {host}:{}", addr.port())),
            "{text}"
        );
        assert!(resp.info.remote_addr.unwrap().starts_with("127.0.0.1:"));
    }
    let err = c
        .send(
            &head("GET", "http://unresolvable-host.invalid/", &[]),
            RequestBody::Empty,
            &opts(),
        )
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::DnsFailed);
}

#[tokio::test]
async fn update_settings_swaps_dns_and_drops_pools() {
    let (addr, accepted) = spawn_http1(|req, _| echo_response(req)).await;
    let mut settings = ProxySettings::default();
    let c = client(&settings);
    let url = format!("http://127.0.0.1:{}/", addr.port());
    collect(
        c.send(&head("GET", &url, &[]), RequestBody::Empty, &opts())
            .await
            .unwrap()
            .body,
    )
    .await;

    // Timeout-only change: pool kept.
    settings.response_timeout_ms = 10_000;
    c.update_settings(&settings);
    let resp = c
        .send(&head("GET", &url, &[]), RequestBody::Empty, &opts())
        .await
        .unwrap();
    assert!(resp.info.reused);
    collect(resp.body).await;

    // DNS change: new generation, pool dropped, override effective.
    settings.dns_overrides = vec![DnsOverride {
        host: "late.test".into(),
        address: "127.0.0.1".into(),
    }];
    c.update_settings(&settings);
    let resp = c
        .send(&head("GET", &url, &[]), RequestBody::Empty, &opts())
        .await
        .unwrap();
    assert!(!resp.info.reused);
    collect(resp.body).await;
    let resp = c
        .send(
            &head("GET", &format!("http://late.test:{}/", addr.port()), &[]),
            RequestBody::Empty,
            &opts(),
        )
        .await
        .unwrap();
    assert_eq!(resp.head.status, 200);
    collect(resp.body).await;
    assert_eq!(accepted.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn websocket_upgrade_keeps_leftover_bytes() {
    let (addr, _) = spawn_server(|mut s, _| async move {
        let mut buf = BytesMut::new();
        let req = read_request(&mut s, &mut buf).await.unwrap();
        assert_eq!(req.header("upgrade"), Some("websocket"));
        assert!(req.raw_head.contains("Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n"));
        // Head and the first frame in a single write.
        s.write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\r\n\x81\x05hello")
            .await
            .unwrap();
        let mut echo = [0u8; 4];
        s.read_exact(&mut echo).await.unwrap();
        s.write_all(&echo).await.unwrap();
    })
    .await;
    let c = client(&ProxySettings::default());
    let h = head(
        "GET",
        &format!("ws://127.0.0.1:{}/chat", addr.port()),
        &[
            ("Host", "127.0.0.1"),
            ("Upgrade", "websocket"),
            ("Connection", "Upgrade"),
            ("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ=="),
            ("Sec-WebSocket-Version", "13"),
        ],
    );
    let (resp_head, upgraded, response, info) = c.send_upgrade(&h, &opts()).await.unwrap();
    assert_eq!(resp_head.status, 101);
    assert_eq!(resp_head.status_message, "Switching Protocols");
    assert!(response.is_none());
    assert!(info.remote_addr.is_some());
    let mut upgraded = upgraded.unwrap();
    assert_eq!(&upgraded.leftover[..], b"\x81\x05hello");
    upgraded.io.write_all(b"ping").await.unwrap();
    let mut back = [0u8; 4];
    upgraded.io.read_exact(&mut back).await.unwrap();
    assert_eq!(&back, b"ping");
}

#[tokio::test]
async fn rejected_upgrade_returns_a_normal_response() {
    let (addr, _) = spawn_http1(|_, _| {
        b"HTTP/1.1 400 Bad Request\r\nContent-Length: 7\r\n\r\nno ws!!".to_vec()
    })
    .await;
    let c = client(&ProxySettings::default());
    let h = head(
        "GET",
        &format!("ws://127.0.0.1:{}/", addr.port()),
        &[("Upgrade", "websocket"), ("Connection", "Upgrade")],
    );
    let (resp_head, upgraded, response, _) = c.send_upgrade(&h, &opts()).await.unwrap();
    assert_eq!(resp_head.status, 400);
    assert!(upgraded.is_none());
    let response = response.unwrap();
    assert_eq!(response.head.status, 400);
    assert_eq!(collect(response.body).await.0, b"no ws!!");
}

#[tokio::test]
async fn connect_raw_tunnels_bytes() {
    let (addr, _) = spawn_server(|mut s, _| async move {
        let mut buf = [0u8; 5];
        s.read_exact(&mut buf).await.unwrap();
        s.write_all(&buf).await.unwrap();
    })
    .await;
    let c = client(&ProxySettings::default());
    let (mut io, info, timings) = c.connect_raw("127.0.0.1", addr.port()).await.unwrap();
    assert_eq!(info.remote_addr.as_deref(), Some(addr.to_string().as_str()));
    assert!(timings.connect_ms.is_some());
    io.write_all(b"hello").await.unwrap();
    let mut back = [0u8; 5];
    io.read_exact(&mut back).await.unwrap();
    assert_eq!(&back, b"hello");
}

#[tokio::test]
async fn ipv6_literal_targets() {
    let Ok(listener) = std::net::TcpListener::bind("[::1]:0") else {
        eprintln!("IPv6 loopback unavailable; skipping");
        return;
    };
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let listener = tokio::net::TcpListener::bind(("::1", port)).await.unwrap();
    tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let mut buf = BytesMut::new();
        let req = read_request(&mut s, &mut buf).await.unwrap();
        s.write_all(&echo_response(&req)).await.unwrap();
    });
    let c = client(&ProxySettings::default());
    let resp = c
        .send(
            &head("GET", &format!("http://[::1]:{port}/v6"), &[]),
            RequestBody::Empty,
            &opts(),
        )
        .await
        .unwrap();
    let text = String::from_utf8(collect(resp.body).await.0).unwrap();
    assert!(text.contains(&format!("Host: [::1]:{port}")), "{text}");
    assert_eq!(resp.info.remote_addr, Some(format!("[::1]:{port}")));
}

#[tokio::test]
async fn proxy_loops_are_detectable_by_the_inbound_side() {
    let c = client(&ProxySettings::default());
    let checker = c.clone();
    // The "inbound side": reports whether each accepted connection came from our own
    // upstream client.
    let (addr, _) = spawn_server(move |mut s, _| {
        let checker = checker.clone();
        async move {
            let peer = s.peer_addr().unwrap();
            let looped = checker.is_own_connection(peer);
            let mut buf = BytesMut::new();
            while read_request(&mut s, &mut buf).await.is_some() {
                let body = if looped { "loop" } else { "no-loop" };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                if s.write_all(response.as_bytes()).await.is_err() {
                    break;
                }
            }
        }
    })
    .await;
    let url = format!("http://127.0.0.1:{}/", addr.port());
    let resp = c
        .send(&head("GET", &url, &[]), RequestBody::Empty, &opts())
        .await
        .unwrap();
    assert_eq!(collect(resp.body).await.0, b"loop");

    // A connection that isn't ours is not flagged.
    let mut foreign = tokio::net::TcpStream::connect(addr).await.unwrap();
    foreign
        .write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
        .await
        .unwrap();
    let mut response = vec![0u8; 256];
    let n = foreign.read(&mut response).await.unwrap();
    assert!(String::from_utf8_lossy(&response[..n]).ends_with("no-loop"));
    assert!(!c.is_own_connection(foreign.local_addr().unwrap()));
}

#[tokio::test]
async fn cookies_of_http2_clients_are_joined_for_http1_servers() {
    let (addr, _) = spawn_http1(|req, _| echo_response(req)).await;
    let c = client(&ProxySettings::default());
    let mut h = head(
        "GET",
        &format!("http://127.0.0.1:{}/", addr.port()),
        &[
            (":authority", "app.test"),
            ("cookie", "a=1"),
            ("cookie", "b=2"),
        ],
    );
    h.http_version = HttpVersion::H2;
    let resp = c.send(&h, RequestBody::Empty, &opts()).await.unwrap();
    let (body, _) = collect(resp.body).await;
    let text = String::from_utf8(body).unwrap();
    assert!(
        text.starts_with("GET / HTTP/1.1\r\nHost: app.test\r\ncookie: a=1; b=2\r\n\r\n"),
        "{text}"
    );
}

#[tokio::test]
async fn slow_streamed_upload_chunks_are_not_held_back() {
    // The server acknowledges the first chunk out of band; the client only produces the
    // second chunk after that. Buffering the first chunk would deadlock.
    let (seen_tx, seen_rx) = tokio::sync::oneshot::channel::<()>();
    let seen_tx = Arc::new(parking_lot::Mutex::new(Some(seen_tx)));
    let (addr, _) = spawn_server(move |mut s, _| {
        let seen_tx = seen_tx.clone();
        async move {
            let mut buf = Vec::new();
            let mut chunk = [0u8; 1024];
            let wait_for =
                |buf: &[u8], needle: &[u8]| buf.windows(needle.len()).any(|w| w == needle);
            while !wait_for(&buf, b"5\r\nhello\r\n") {
                let n = s.read(&mut chunk).await.unwrap();
                assert!(n > 0, "client closed early");
                buf.extend_from_slice(&chunk[..n]);
            }
            if let Some(tx) = seen_tx.lock().take() {
                let _ = tx.send(());
            }
            while !wait_for(&buf, b"0\r\n\r\n") {
                let n = s.read(&mut chunk).await.unwrap();
                assert!(n > 0, "client closed early");
                buf.extend_from_slice(&chunk[..n]);
            }
            let text = String::from_utf8_lossy(&buf).into_owned();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{text}",
                text.len()
            );
            s.write_all(response.as_bytes()).await.unwrap();
        }
    })
    .await;

    let (tx, rx) = tokio::sync::mpsc::channel::<BodyChunk>(4);
    let body: BodyStream = Box::pin(futures::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|c| (Ok::<_, UpstreamError>(c), rx))
    }));
    let producer = tokio::spawn(async move {
        tx.send(BodyChunk::Data(Bytes::from_static(b"hello")))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), seen_rx)
            .await
            .expect("first chunk was held back by the client")
            .unwrap();
        tx.send(BodyChunk::Data(Bytes::from_static(b"world")))
            .await
            .unwrap();
    });
    let c = client(&ProxySettings::default());
    let resp = c
        .send(
            &head("POST", &format!("http://127.0.0.1:{}/up", addr.port()), &[]),
            RequestBody::Stream {
                stream: body,
                length: None,
            },
            &opts(),
        )
        .await
        .unwrap();
    producer.await.unwrap();
    let (echo, _) = collect(resp.body).await;
    let echo = String::from_utf8(echo).unwrap();
    assert!(
        echo.ends_with("5\r\nhello\r\n5\r\nworld\r\n0\r\n\r\n"),
        "{echo}"
    );
}

/// A server that authenticates the TCP connection (like IIS with NTLM): after a request with
/// `Authorization`, the connection is served as that user. Answers `<user>` or `denied`.
async fn spawn_conn_auth_server() -> (std::net::SocketAddr, Arc<std::sync::atomic::AtomicUsize>) {
    spawn_server(|mut stream, _| async move {
        let mut buf = BytesMut::new();
        let mut user: Option<String> = None;
        while let Some(req) = read_request(&mut stream, &mut buf).await {
            if let Some(auth) = req.header("authorization") {
                user = Some(auth.to_string());
            }
            let response = match &user {
                Some(u) => format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{u}", u.len()),
                None => "HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: NTLM\r\nContent-Length: 6\r\n\r\ndenied".to_string(),
            };
            if stream.write_all(response.as_bytes()).await.is_err() {
                break;
            }
        }
    })
    .await
}

#[tokio::test]
async fn ntlm_connections_are_pinned_to_their_owner() {
    let (addr, accepted) = spawn_conn_auth_server().await;
    let c = client(&ProxySettings::default());
    let url = format!("http://127.0.0.1:{}/conn-auth", addr.port());
    let send = |auth: Option<&'static str>,
                owner: Option<akev_proxy::upstream::ConnectionOwner>| {
        let c = c.clone();
        let url = url.clone();
        async move {
            let headers: Vec<(&str, &str)> =
                auth.map(|a| ("Authorization", a)).into_iter().collect();
            let options = RequestOptions {
                owner,
                ..Default::default()
            };
            let resp = c
                .send(&head("GET", &url, &headers), RequestBody::Empty, &options)
                .await
                .unwrap();
            (resp.head.status, collect(resp.body).await.0)
        }
    };
    let alice = akev_proxy::upstream::ConnectionOwner::new();
    let bob = akev_proxy::upstream::ConnectionOwner::new();

    // Alice authenticates a connection; it is hers alone from now on.
    assert_eq!(
        send(Some("NTLM alice"), Some(alice.clone())).await,
        (200, b"NTLM alice".to_vec())
    );
    // Bob (and requests without an owner) never get it.
    assert_eq!(send(None, Some(bob.clone())).await.0, 401);
    assert_eq!(send(None, None).await.0, 401);
    // Alice's next request (next handshake leg) reuses it.
    assert_eq!(
        send(None, Some(alice.clone())).await,
        (200, b"NTLM alice".to_vec())
    );
    // Alice and Bob each got a connection (Bob's was pinned by the NTLM challenge), the
    // owner-less request a third one that wasn't pooled (it got a challenge too).
    assert_eq!(accepted.load(Ordering::SeqCst), 3);

    // Without an owner to pin to, an authenticated connection is never reused.
    assert_eq!(send(Some("NTLM anon"), None).await.0, 200);
    assert_eq!(send(None, None).await.0, 401);

    // Once Alice's client connection is gone, her pinned connection is closed, not shared.
    drop(alice);
    assert_eq!(send(None, Some(bob)).await.0, 401);
}

#[tokio::test]
async fn connect_failure_hands_back_the_unsent_body() {
    let closed = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap()
    };
    let c = client(&ProxySettings::default());
    let polled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = polled.clone();
    let body = RequestBody::Stream {
        stream: Box::pin(futures::stream::once(async move {
            flag.store(true, Ordering::SeqCst);
            Ok::<_, UpstreamError>(BodyChunk::Data(Bytes::from_static(b"payload")))
        })),
        length: None,
    };
    let url = format!("http://{closed}/refused");
    let failure = c
        .send_returning_body(&head("POST", &url, &[]), body, &opts())
        .await
        .expect_err("connection refused");
    assert_eq!(failure.error.kind, ErrorKind::ConnectionRefused);
    assert!(!polled.load(Ordering::SeqCst), "the body was never read");
    let Some(RequestBody::Stream { stream, .. }) = failure.unsent_body else {
        panic!("body not handed back: {:?}", failure.unsent_body)
    };
    assert_eq!(collect(stream).await.0, b"payload");

    // `send` keeps returning the plain error.
    let err = c
        .send(&head("GET", &url, &[]), RequestBody::Empty, &opts())
        .await
        .expect_err("connection refused");
    assert_eq!(err.kind, ErrorKind::ConnectionRefused);
}
