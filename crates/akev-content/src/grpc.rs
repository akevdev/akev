//! gRPC / gRPC-Web message framing.
//!
//! Frames are `[flags: u8][length: u32 BE][payload]`. Flag bit `0x01` = compressed with the
//! `grpc-encoding` algorithm (gzip, deflate (zlib or raw), zstd, snappy (raw block or framed
//! stream); `identity` / missing header with a compressed frame is an error). Flag bit `0x80` marks a
//! gRPC-Web trailer frame whose payload is an HTTP/1-style header block (`name: value\r\n`).
//! gRPC-Web-text bodies are base64, possibly a concatenation of separately padded chunks.

use bytes::Bytes;
use serde::Serialize;

use crate::budget::Budget;
use crate::decode::{Coding, DecodeFailure, decode_one};
use crate::{ContentError, Result};

/// Maximum decompressed size of a single message.
pub const MAX_MESSAGE_SIZE: usize = 64 << 20;

/// Default cap on the decoded size of all messages of a body together (compressed frames count
/// with their decompressed size), used by [`parse_messages`] / [`parse_lenient`].
pub const MAX_TOTAL_SIZE: usize = 256 << 20;

/// One gRPC frame.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrpcMessage {
    /// The frame's compressed flag (bit 0x01) was set.
    pub compressed: bool,
    /// Decompressed message bytes (not serialized; the API exposes `decoded` / `size`).
    #[serde(skip)]
    pub data: Bytes,
    /// Schemaless decode, if it parses.
    pub decoded: Option<serde_json::Value>,
    /// gRPC-Web trailer frame (flag 0x80) parsed as headers.
    pub trailers: Option<Vec<(String, String)>>,
}

/// Split a gRPC / gRPC-Web body into length-prefixed messages. `grpc_encoding` is the
/// `grpc-encoding` header (gzip/deflate/zstd/snappy/identity). `text_mode` = grpc-web-text
/// (base64 body).
///
/// `decoded` holds the schemaless protobuf decode ([`crate::protobuf::to_json`]) when the
/// payload is valid protobuf, else the parsed JSON when the payload is JSON (`application/grpc+json`),
/// else `None`. Errors carry HTTP Toolkit-style context: `gRPC message #i @offset: ...`.
/// The messages may decode to at most [`MAX_TOTAL_SIZE`] bytes together.
pub fn parse_messages(
    body: &[u8],
    grpc_encoding: Option<&str>,
    text_mode: bool,
) -> Result<Vec<GrpcMessage>> {
    let (messages, error) = parse_lenient(body, grpc_encoding, text_mode);
    match error {
        Some(e) => Err(e),
        None => Ok(messages),
    }
}

/// Parse as many frames as possible; returns the messages decoded before the first error, and
/// that error (e.g. a truncated capture). The messages may decode to at most
/// [`MAX_TOTAL_SIZE`] bytes together.
pub fn parse_lenient(
    body: &[u8],
    grpc_encoding: Option<&str>,
    text_mode: bool,
) -> (Vec<GrpcMessage>, Option<ContentError>) {
    parse_lenient_limited(body, grpc_encoding, text_mode, MAX_TOTAL_SIZE)
}

/// [`parse_lenient`] with an explicit cap on the decoded size of all messages together: one
/// decompression budget for the whole body (no single message may exceed
/// [`MAX_MESSAGE_SIZE`] either). Once a message would exceed what is left, parsing stops with
/// an error and the messages before it are returned.
pub fn parse_lenient_limited(
    body: &[u8],
    grpc_encoding: Option<&str>,
    text_mode: bool,
    max_total: usize,
) -> (Vec<GrpcMessage>, Option<ContentError>) {
    let parsed = parse_budget(
        body,
        grpc_encoding,
        text_mode,
        max_total,
        &mut Budget::unlimited(),
    );
    (parsed.messages, parsed.error)
}

/// What [`parse_budget`] produced.
pub(crate) struct Parsed {
    pub(crate) messages: Vec<GrpcMessage>,
    pub(crate) error: Option<ContentError>,
    /// Parsing stopped at a size limit (decoded bytes or output budget), not at an error in the
    /// data: only a prefix of the messages is present.
    pub(crate) limited: bool,
}

/// Parse frames until the end, an error, `max_total` decoded payload bytes, or the output
/// budget runs out (each message and its schemaless decode are charged to `budget`).
pub(crate) fn parse_budget(
    body: &[u8],
    grpc_encoding: Option<&str>,
    text_mode: bool,
    max_total: usize,
    budget: &mut Budget,
) -> Parsed {
    let mut parsed = Parsed {
        messages: Vec::new(),
        error: None,
        limited: false,
    };
    let decoded_text;
    let data: &[u8] = if text_mode {
        match decode_grpc_web_text(body) {
            Ok(d) => {
                decoded_text = d;
                &decoded_text
            }
            Err(e) => {
                parsed.error = Some(e);
                return parsed;
            }
        }
    } else {
        body
    };

    // Decoded payload bytes still allowed (compressed frames count decompressed).
    let mut remaining = max_total;
    let mut offset = 0usize;
    let mut index = 0usize;
    while offset < data.len() {
        let err = |msg: String| ContentError::Parse {
            what: "gRPC",
            message: format!("gRPC message #{index} @{offset}: {msg}"),
        };
        let total_exceeded = || {
            err(format!(
                "decoded messages exceed {max_total} bytes in total"
            ))
        };
        let Some(header) = data.get(offset..offset + 5) else {
            parsed.error = Some(err("truncated frame header".into()));
            return parsed;
        };
        let flags = header[0];
        let len = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;
        let start = offset + 5;
        let Some(payload) = start.checked_add(len).and_then(|end| data.get(start..end)) else {
            parsed.error = Some(err("length of message is corrupted".into()));
            return parsed;
        };
        if flags & !0x81 != 0 {
            parsed.error = Some(err(format!("unsupported compression flag (0x{flags:02x})")));
            return parsed;
        }
        // The message object with `compressed`, `decoded`, `trailers` and `size`.
        if !budget.charge(5, 0) {
            parsed.limited = true;
            return parsed;
        }
        let compressed = flags & 0x01 != 0;
        let trailer = flags & 0x80 != 0;
        let payload: Bytes = if compressed {
            let limit = remaining.min(MAX_MESSAGE_SIZE);
            match decompress(payload, grpc_encoding, limit) {
                Ok(p) => Bytes::from(p),
                Err(Failure::TooLarge) if limit < MAX_MESSAGE_SIZE => {
                    parsed.error = Some(total_exceeded());
                    parsed.limited = true;
                    return parsed;
                }
                Err(Failure::TooLarge) => {
                    parsed.error = Some(err(format!(
                        "failed decompression (from {}): decoded body exceeds \
                         {MAX_MESSAGE_SIZE} bytes",
                        encoding_name(grpc_encoding)
                    )));
                    return parsed;
                }
                Err(Failure::Other(msg)) => {
                    parsed.error = Some(err(msg));
                    return parsed;
                }
            }
        } else if payload.len() > remaining {
            parsed.error = Some(total_exceeded());
            parsed.limited = true;
            return parsed;
        } else {
            Bytes::copy_from_slice(payload)
        };
        remaining -= payload.len();
        let message = if trailer {
            GrpcMessage {
                compressed,
                decoded: None,
                trailers: Some(parse_trailers(&payload, budget)),
                data: payload,
            }
        } else {
            GrpcMessage {
                compressed,
                decoded: decode_payload(&payload, budget),
                trailers: None,
                data: payload,
            }
        };
        parsed.messages.push(message);
        if budget.exhausted() {
            parsed.limited = true;
            return parsed;
        }
        offset = start + len;
        index += 1;
    }
    parsed
}

enum Failure {
    /// The output exceeded the limit passed in.
    TooLarge,
    Other(String),
}

fn encoding_name(grpc_encoding: Option<&str>) -> &str {
    grpc_encoding
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or("identity")
}

fn decompress(
    payload: &[u8],
    grpc_encoding: Option<&str>,
    limit: usize,
) -> std::result::Result<Vec<u8>, Failure> {
    let name = encoding_name(grpc_encoding);
    let failed = |e: DecodeFailure| match e {
        DecodeFailure::TooLarge(_) => Failure::TooLarge,
        e => Failure::Other(format!("failed decompression (from {name}): {e}")),
    };
    if name.eq_ignore_ascii_case("snappy") {
        return crate::snappy::decompress(payload, limit).map_err(failed);
    }
    let coding = match Coding::parse(name) {
        Some(Coding::Identity) => {
            return Err(Failure::Other(
                "not expected to be compressed (no grpc-encoding)".into(),
            ));
        }
        Some(c @ (Coding::Gzip | Coding::Deflate | Coding::Zstd)) => c,
        _ => {
            return Err(Failure::Other(format!(
                "unsupported grpc-encoding '{name}'"
            )));
        }
    };
    decode_one(coding, payload, limit, false)
        .map(|(v, _)| v)
        .map_err(failed)
}

/// Schemaless decode: protobuf first, then JSON (gRPC with a JSON codec), within `budget`.
/// `None` when neither parses, or when the budget can't hold the result.
pub(crate) fn decode_payload(data: &[u8], budget: &mut Budget) -> Option<serde_json::Value> {
    let before = *budget;
    match crate::protobuf::decode_raw_budget(data, budget) {
        Ok(fields) if fields.is_empty() && budget.exhausted() => return None,
        Ok(fields) => return Some(crate::protobuf::to_json(&fields)),
        // Not protobuf: give back what the attempt charged.
        Err(_) => *budget = before,
    }
    let t = crate::util::skip_bom_ws(data);
    if matches!(t.first(), Some(b'{' | b'[')) {
        // Check the size of the tree before building it.
        if !budget.charge(crate::budget::json_value_bound(t), t.len()) {
            return None;
        }
        return serde_json::from_slice(t).ok();
    }
    None
}

/// Parse a trailer block, keeping the pairs that fit `budget` (three values each: the pair and
/// its two strings).
fn parse_trailers(payload: &[u8], budget: &mut Budget) -> Vec<(String, String)> {
    String::from_utf8_lossy(payload)
        .split('\n')
        .filter_map(|line| {
            let line = line.trim_end_matches('\r');
            let (name, value) = line.split_once(':')?;
            let name = name.trim();
            (!name.is_empty()).then_some((name, value.trim()))
        })
        .take_while(|(name, value)| budget.charge(3, name.len() + value.len()))
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

/// Decode a grpc-web-text body: base64 where each chunk (one per streamed message) may carry
/// its own `=` padding, so padding can appear mid-stream. Whitespace is ignored.
fn decode_grpc_web_text(body: &[u8]) -> Result<Vec<u8>> {
    use base64::Engine as _;
    let mut out = Vec::with_capacity(body.len() / 4 * 3 + 3);
    let mut chunk: Vec<u8> = Vec::with_capacity(body.len().min(1 << 16));
    let mut i = 0;
    let flush = |chunk: &mut Vec<u8>, out: &mut Vec<u8>| -> Result<()> {
        if !chunk.is_empty() {
            crate::util::B64_LENIENT
                .decode_vec(&chunk[..], out)
                .map_err(|e| ContentError::Parse {
                    what: "gRPC-Web text",
                    message: format!("invalid base64: {e}"),
                })?;
            chunk.clear();
        }
        Ok(())
    };
    while i < body.len() {
        let b = body[i];
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        chunk.push(b);
        i += 1;
        if b == b'=' {
            // consume the rest of the padding run, then end the chunk
            while i < body.len() && (body[i] == b'=' || body[i].is_ascii_whitespace()) {
                if body[i] == b'=' {
                    chunk.push(b'=');
                }
                i += 1;
            }
            flush(&mut chunk, &mut out)?;
        }
    }
    // A final quantum cut down to a single character (a truncated capture) carries no complete
    // byte: drop it so the frames before the cut still decode (frame parsing then reports the
    // truncation).
    if chunk.len() % 4 == 1 {
        chunk.pop();
    }
    flush(&mut chunk, &mut out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use serde_json::json;

    fn frame(flags: u8, payload: &[u8]) -> Vec<u8> {
        let mut v = vec![flags];
        v.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        v.extend_from_slice(payload);
        v
    }

    // {1: "hi", 2: 150}
    const MSG: &[u8] = &[0x0a, 0x02, b'h', b'i', 0x10, 0x96, 0x01];

    #[test]
    fn plain_frames() {
        let mut body = frame(0, MSG);
        body.extend(frame(0, &[]));
        let msgs = parse_messages(&body, None, false).unwrap();
        assert_eq!(msgs.len(), 2);
        assert!(!msgs[0].compressed);
        assert_eq!(&msgs[0].data[..], MSG);
        assert_eq!(msgs[0].decoded, Some(json!({"1": "hi", "2": 150})));
        assert_eq!(msgs[1].decoded, Some(json!({})));
        assert_eq!(
            serde_json::to_value(&msgs[0]).unwrap(),
            json!({"compressed": false, "decoded": {"1": "hi", "2": 150}, "trailers": null})
        );
    }

    #[test]
    fn compressed_frames() {
        for enc in ["gzip", "deflate", "zstd"] {
            let c = crate::decode::encode_body(MSG, &[enc.to_string()]).unwrap();
            let body = frame(1, &c);
            let msgs = parse_messages(&body, Some(enc), false).unwrap();
            assert!(msgs[0].compressed);
            assert_eq!(&msgs[0].data[..], MSG, "{enc}");
            assert_eq!(msgs[0].decoded, Some(json!({"1": "hi", "2": 150})));
        }
        // Flag 0 is allowed even with a grpc-encoding header.
        assert!(parse_messages(&frame(0, MSG), Some("gzip"), false).is_ok());
        let c = crate::decode::encode_body(MSG, &["gzip".to_string()]).unwrap();
        let e = parse_messages(&frame(1, &c), None, false).unwrap_err();
        assert!(
            e.to_string().contains("not expected to be compressed"),
            "{e}"
        );
        let e = parse_messages(&frame(1, &c), Some("lz4"), false).unwrap_err();
        assert!(
            e.to_string().contains("unsupported grpc-encoding 'lz4'"),
            "{e}"
        );
        // snappy (raw block): literal of the whole message
        let mut snappy = vec![MSG.len() as u8, ((MSG.len() - 1) << 2) as u8];
        snappy.extend_from_slice(MSG);
        let msgs = parse_messages(&frame(1, &snappy), Some("Snappy"), false).unwrap();
        assert_eq!(msgs[0].decoded, Some(json!({"1": "hi", "2": 150})));
        let e = parse_messages(&frame(1, b"\x05junk"), Some("snappy"), false).unwrap_err();
        assert!(
            e.to_string().contains("failed decompression (from snappy)"),
            "{e}"
        );
        let e = parse_messages(&frame(1, b"junk"), Some("gzip"), false).unwrap_err();
        assert!(
            e.to_string().contains("failed decompression (from gzip)"),
            "{e}"
        );
    }

    #[test]
    fn errors_and_lenient() {
        let mut body = frame(0, MSG);
        body.extend_from_slice(&[0, 0, 0, 0, 9, 1, 2]);
        let e = parse_messages(&body, None, false).unwrap_err();
        assert_eq!(
            e.to_string(),
            "invalid gRPC: gRPC message #1 @12: length of message is corrupted"
        );
        let (msgs, err) = parse_lenient(&body, None, false);
        assert_eq!(msgs.len(), 1);
        assert!(err.is_some());
        let e = parse_messages(&frame(2, MSG), None, false).unwrap_err();
        assert!(
            e.to_string()
                .contains("unsupported compression flag (0x02)")
        );
        let e = parse_messages(&[0, 0, 0], None, false).unwrap_err();
        assert!(e.to_string().contains("truncated frame header"));
        assert_eq!(parse_messages(b"", None, false).unwrap(), vec![]);
    }

    #[test]
    fn grpc_web_trailers_and_json_codec() {
        let mut body = frame(0, br#"{"greeting":"hi"}"#);
        body.extend(frame(0x80, b"grpc-status: 0\r\ngrpc-message: OK done\r\n"));
        let msgs = parse_messages(&body, None, false).unwrap();
        assert_eq!(msgs[0].decoded, Some(json!({"greeting": "hi"})));
        assert_eq!(
            msgs[1].trailers,
            Some(vec![
                ("grpc-status".into(), "0".into()),
                ("grpc-message".into(), "OK done".into())
            ])
        );
        assert_eq!(msgs[1].decoded, None);
    }

    #[test]
    fn grpc_web_text_with_concatenated_padded_chunks() {
        // 9-byte payload → 14-byte frame → base64 with padding in the middle of the stream
        let mut payload = MSG.to_vec();
        payload.extend_from_slice(&[0x18, 0x01]);
        let a = frame(0, &payload);
        let t = frame(0x80, b"grpc-status:0\r\n");
        let mut text = crate::util::B64.encode(&a);
        assert!(text.ends_with('='), "test needs padding mid-stream: {text}");
        text.push_str("\r\n");
        text.push_str(&crate::util::B64.encode(&t));
        let msgs = parse_messages(text.as_bytes(), None, true).unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].decoded, Some(json!({"1": "hi", "2": 150, "3": 1})));
        assert_eq!(
            msgs[1].trailers.as_ref().unwrap()[0],
            ("grpc-status".into(), "0".into())
        );
        assert!(parse_messages(b"!!notbase64!!", None, true).is_err());
    }

    #[test]
    fn truncated_grpc_web_text_keeps_complete_frames() {
        let mut body = frame(0, MSG);
        body.extend(frame(0, MSG));
        let text = crate::util::B64.encode(&body);
        // Cut so that the last base64 quantum holds a single character.
        let cut_len = (0..text.len())
            .rev()
            .find(|n| n % 4 == 1 && *n > 16)
            .unwrap();
        let (msgs, err) = parse_lenient(&text.as_bytes()[..cut_len], None, true);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].decoded, Some(json!({"1": "hi", "2": 150})));
        assert!(
            err.unwrap()
                .to_string()
                .contains("length of message is corrupted")
        );
        assert!(parse_messages(&text.as_bytes()[..cut_len], None, true).is_err());
    }
}
