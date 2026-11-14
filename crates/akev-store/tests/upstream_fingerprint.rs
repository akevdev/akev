//! The upstream TLS fingerprint (`UpstreamInfo.fingerprint`) is stored and survives HAR and
//! `.akev` session round trips (via the `_akev` extension).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use akev_core::*;
use common::*;

fn fingerprinted_store() -> std::sync::Arc<akev_store::Store> {
    let store = store();
    store.ingest(started(
        1,
        "GET",
        "https://shop.example/",
        &[("Host", "shop.example")],
    ));
    store.ingest(req_done(1, b"", 1.0));
    store.ingest(ProxyEvent::Upstream {
        id: ExchangeId(1),
        info: UpstreamInfo {
            remote_addr: Some("203.0.113.7:443".into()),
            tls_version: Some("TLSv1.3".into()),
            alpn: Some("h2".into()),
            fingerprint: Some(std::sync::Arc::new(UpstreamFingerprint {
                profile: "firefox".into(),
                requested: Some("mirror-client".into()),
                note: None,
                ja3: Some("6f7889b9fb1a62a9577e685c1fcfa919".into()),
                ja3_text: Some("771,4865-4867-4866,0-23-65281,4588-29,0".into()),
                ja4: Some("t13d1717h2_5b57614c22b0_3cbfd9057e0d".into()),
                http2: Some("1:65536;2:0;4:131072;5:16384|12517377|0|m,p,a,s".into()),
            })),
            ..UpstreamInfo::default()
        },
        dns_duration: Some(1.0),
        connect_duration: Some(2.0),
        tls_duration: Some(3.0),
        request_sent: Some(4.0),
    });
    store.ingest(resp_start(1, 200, &[("Content-Type", "text/plain")], 10.0));
    store.ingest(resp_done(1, b"ok", 11.0));
    store
}

#[test]
fn fingerprint_is_in_the_detail_and_its_json() {
    let store = fingerprinted_store();
    let detail = store.detail(ExchangeId(1)).unwrap();
    let fp = detail
        .upstream
        .as_ref()
        .unwrap()
        .fingerprint
        .as_ref()
        .unwrap();
    assert_eq!(fp.profile, "firefox");
    let json = serde_json::to_value(&detail).unwrap();
    let fp = &json["upstream"]["fingerprint"];
    assert_eq!(fp["profile"], "firefox");
    assert_eq!(fp["requested"], "mirror-client");
    assert_eq!(fp["ja3Text"], "771,4865-4867-4866,0-23-65281,4588-29,0");
    assert_eq!(
        fp["http2"],
        "1:65536;2:0;4:131072;5:16384|12517377|0|m,p,a,s"
    );
    assert!(fp.get("note").is_none(), "absent fields are omitted");
}

#[test]
fn fingerprint_survives_a_har_round_trip() {
    let source = fingerprinted_store();
    let har = serde_json::to_vec(&source.export_har(None)).unwrap();
    let target = store();
    let ids = target.import_har(&har).unwrap();
    assert_eq!(ids.len(), 1);
    assert_eq!(
        source.detail(ExchangeId(1)).unwrap().upstream,
        target.detail(ids[0]).unwrap().upstream
    );
}

#[test]
fn fingerprint_survives_a_session_round_trip() {
    let source = fingerprinted_store();
    let mut file = Vec::new();
    source.save_session(&mut file, None).unwrap();
    let target = store();
    let ids = target.load_session(&mut file.as_slice()).unwrap();
    assert_eq!(
        source.detail(ExchangeId(1)).unwrap().upstream,
        target.detail(ids[0]).unwrap().upstream
    );
}
