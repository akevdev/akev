//! Requests issued by Akev itself (Compose / Repeat) have no client connection: their
//! summary shows the protocol spoken upstream (HTTP/2, HTTP/3) once the response starts.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use akev_core::*;
use common::*;

fn with_version(mut ev: ProxyEvent, version: HttpVersion) -> ProxyEvent {
    if let ProxyEvent::ResponseStarted { head, .. } = &mut ev {
        head.http_version = version;
    }
    ev
}

fn via(mut ev: ProxyEvent, mode: ProxyMode) -> ProxyEvent {
    if let ProxyEvent::RequestStarted { client, .. } = &mut ev {
        client.via = mode;
    }
    ev
}

#[test]
fn composer_exchanges_show_the_upstream_protocol() {
    let store = store();
    store.ingest(via(
        started(1, "GET", "https://h3.example/x", &[]),
        ProxyMode::Composer,
    ));
    assert_eq!(
        store.summary(ExchangeId(1)).unwrap().http_version,
        HttpVersion::Http11
    );
    store.ingest(with_version(resp_start(1, 200, &[], 5.0), HttpVersion::H3));
    assert_eq!(
        store.summary(ExchangeId(1)).unwrap().http_version,
        HttpVersion::H3
    );

    // Proxied exchanges keep the client's protocol (here HTTP/1.1 to Akev, HTTP/3 upstream).
    store.ingest(via(
        started(2, "GET", "https://h3.example/y", &[]),
        ProxyMode::Connect,
    ));
    store.ingest(with_version(resp_start(2, 200, &[], 5.0), HttpVersion::H3));
    assert_eq!(
        store.summary(ExchangeId(2)).unwrap().http_version,
        HttpVersion::Http11
    );
}
