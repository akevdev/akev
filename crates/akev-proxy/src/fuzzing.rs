//! Entry points for the fuzz targets in `fuzz/` (cargo feature `fuzzing`). Each runs one
//! internal parser on arbitrary bytes and discards the result: the targets only look for
//! panics, hangs and excessive memory use. Not a stable API.

use bytes::BytesMut;
use std::sync::OnceLock;

/// HTTP/1 request heads ([`crate::http1::parse`]): parse, then the framing rules and the
/// body decoder over whatever follows the head.
pub fn http1_request(data: &[u8]) {
    use crate::http1::decode::Decoder;
    use crate::http1::parse::{
        client_wants_close, expects_continue, has_head_end, parse_request, request_framing,
        upgrade_request,
    };
    let _ = has_head_end(data, 0);
    let Ok(Some(raw)) = parse_request(data) else {
        return;
    };
    let _ = upgrade_request(&raw.headers);
    let _ = client_wants_close(&raw.headers, raw.version);
    let _ = expects_continue(&raw.headers);
    let _ = crate::target::RequestTarget::parse(&raw.target);
    if let Ok(framing) = request_framing(&raw.headers) {
        let mut body = BytesMut::from(data.get(raw.head_len..).unwrap_or_default());
        let mut decoder = Decoder::new(framing);
        // Bounded: every successful step consumes input or ends the body.
        for _ in 0..data.len() + 2 {
            match decoder.decode(&mut body) {
                Ok(
                    crate::http1::decode::Decoded::Data(_)
                    | crate::http1::decode::Decoded::Trailers(_),
                ) => {}
                _ => break,
            }
        }
    }
}

/// HTTP/1 response heads from upstream servers ([`crate::upstream`]'s parser) and the
/// RFC 9112 framing rules.
pub fn http1_response(data: &[u8]) {
    crate::upstream::fuzz_response_head(data);
}

/// PROXY protocol v1 / v2 headers.
pub fn proxy_protocol(data: &[u8]) {
    let _ = crate::proxy_protocol::parse_v1(data);
    let _ = crate::proxy_protocol::parse_v2(data);
}

/// TLS ClientHello parsing (SNI, ALPN, JA3/JA4 inputs).
pub fn client_hello(data: &[u8]) {
    let _ = crate::client_hello::parse(data);
}

/// SOCKS4/4a/5 server handshakes on a stream that sends `data` (the first byte picks whether
/// proxy credentials are required, so both paths are covered).
pub fn socks_handshake(data: &[u8]) {
    static RUNTIME: OnceLock<Option<tokio::runtime::Runtime>> = OnceLock::new();
    let Some(runtime) = RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .ok()
    }) else {
        return;
    };
    let Some((&selector, input)) = data.split_first() else {
        return;
    };
    let access = akev_core::access::ClientAccess::new(&akev_core::settings::ProxyAccessSettings {
        username: "user".into(),
        password: "password".into(),
        ..Default::default()
    });
    let auth = (selector & 1 == 1).then_some(&access);
    runtime.block_on(async {
        let mut io = tokio::io::join(input, tokio::io::sink());
        let mut buf = BytesMut::new();
        let _ = crate::socks::handshake(&mut io, &mut buf, auth).await;
    });
}
