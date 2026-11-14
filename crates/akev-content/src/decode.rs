//! `Content-Encoding` decoding and re-encoding.
//!
//! Supported codings (case-insensitive): `gzip` / `x-gzip` (multi-member streams; trailing
//! garbage after the last member is ignored like browsers do), `deflate` / `x-deflate` (zlib-wrapped
//! *or* raw DEFLATE, autodetected), `br`, `zstd` (multi-frame), `base64`, `identity` and
//! `amz-1.0` (an identity alias sent by AWS SDKs).
//!
//! Decoding streams through a size cap so a small compressed body can't expand into gigabytes
//! (zip-bomb protection).

use std::borrow::Cow;
use std::io::{self, Read, Write};

use bytes::Bytes;

use crate::{ContentError, Result};

/// A single supported content coding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Coding {
    Identity,
    Gzip,
    Deflate,
    Brotli,
    Zstd,
    Base64,
}

impl Coding {
    pub(crate) fn parse(name: &str) -> Option<Coding> {
        let n = name.trim();
        let is = |s: &str| n.eq_ignore_ascii_case(s);
        Some(if is("gzip") || is("x-gzip") {
            Coding::Gzip
        } else if is("deflate") || is("x-deflate") {
            Coding::Deflate
        } else if is("br") {
            Coding::Brotli
        } else if is("zstd") {
            Coding::Zstd
        } else if is("identity") || is("amz-1.0") || n.is_empty() {
            Coding::Identity
        } else if is("base64") {
            Coding::Base64
        } else {
            return None;
        })
    }
}

/// Decode a body given its `Content-Encoding` chain as listed in the header (applied
/// left-to-right by the sender, so decoded right-to-left). Unknown encodings → error.
/// `max_output` caps decompressed size (zip-bomb protection); exceeding it → error.
///
/// An empty body decodes to an empty body for every known coding (HEAD / 204 responses often
/// carry `Content-Encoding` without a body). A chain made only of `identity` returns the input
/// unchanged and is not subject to `max_output` (nothing is decompressed).
pub fn decode_body(bytes: &[u8], encodings: &[String], max_output: usize) -> Result<Bytes> {
    decode_chain(bytes, encodings, max_output, false).map(|(b, _)| b)
}

/// Like [`decode_body`] but tolerant of truncated input (a capture that only holds a prefix of
/// the body, or an aborted transfer): when a decoder fails after producing output, the output
/// decoded so far is returned with `complete = false` instead of an error. Unknown encodings and
/// exceeding `max_output` are still errors.
pub fn decode_body_partial(
    bytes: &[u8],
    encodings: &[String],
    max_output: usize,
) -> Result<(Bytes, bool)> {
    decode_chain(bytes, encodings, max_output, true)
}

/// Re-apply an encoding chain (used after editing a decoded body).
///
/// Levels are tuned for interactive use (edited bodies are re-encoded on every keystroke batch):
/// gzip/deflate level 6, brotli quality 5 (window 22), zstd level 3. `deflate` produces the
/// zlib-wrapped format that RFC 9110 specifies.
pub fn encode_body(bytes: &[u8], encodings: &[String]) -> Result<Bytes> {
    let codings = parse_chain(encodings)?;
    let mut current: Cow<'_, [u8]> = Cow::Borrowed(bytes);
    for (coding, name) in codings {
        let encoded = encode_one(coding, &current).map_err(|e| ContentError::Decode {
            encoding: name.to_string(),
            message: format!("encoding failed: {e}"),
        })?;
        if let Some(v) = encoded {
            current = Cow::Owned(v);
        }
    }
    Ok(match current {
        Cow::Borrowed(b) => Bytes::copy_from_slice(b),
        Cow::Owned(v) => Bytes::from(v),
    })
}

/// Whether we can decode this encoding name.
pub fn is_supported(encoding: &str) -> bool {
    Coding::parse(encoding).is_some()
}

/// Validate a chain, dropping identity codings. Order is preserved (as listed in the header).
fn parse_chain(encodings: &[String]) -> Result<Vec<(Coding, &str)>> {
    let mut out = Vec::with_capacity(encodings.len());
    for name in encodings {
        let coding = Coding::parse(name)
            .ok_or_else(|| ContentError::UnsupportedEncoding(name.trim().to_string()))?;
        if coding != Coding::Identity {
            out.push((coding, name.trim()));
        }
    }
    Ok(out)
}

fn decode_chain(
    bytes: &[u8],
    encodings: &[String],
    max_output: usize,
    lenient: bool,
) -> Result<(Bytes, bool)> {
    let codings = parse_chain(encodings)?;
    if codings.is_empty() || bytes.is_empty() {
        return Ok((Bytes::copy_from_slice(bytes), true));
    }
    let mut current: Cow<'_, [u8]> = Cow::Borrowed(bytes);
    let mut complete = true;
    for (coding, name) in codings.into_iter().rev() {
        let (out, ok) = decode_one(coding, &current, max_output, lenient).map_err(|e| {
            ContentError::Decode {
                encoding: name.to_string(),
                message: e.to_string(),
            }
        })?;
        complete &= ok;
        current = Cow::Owned(out);
    }
    Ok((Bytes::from(current.into_owned()), complete))
}

/// Why a single decode step failed.
#[derive(Debug)]
pub(crate) enum DecodeFailure {
    TooLarge(usize),
    Io(String),
}

impl From<&str> for DecodeFailure {
    fn from(msg: &str) -> Self {
        DecodeFailure::Io(msg.to_string())
    }
}

impl From<String> for DecodeFailure {
    fn from(msg: String) -> Self {
        DecodeFailure::Io(msg)
    }
}

impl std::fmt::Display for DecodeFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeFailure::TooLarge(max) => write!(f, "decoded body exceeds {max} bytes"),
            DecodeFailure::Io(msg) => f.write_str(msg),
        }
    }
}

/// Read `r` to the end into `out`, failing once `out` would exceed `max` bytes.
fn read_capped<R: Read>(
    r: &mut R,
    out: &mut Vec<u8>,
    max: usize,
) -> std::result::Result<(), ReadError> {
    let allowance = max.saturating_sub(out.len()) as u64;
    r.take(allowance.saturating_add(1))
        .read_to_end(out)
        .map_err(ReadError::Io)?;
    if out.len() > max {
        return Err(ReadError::TooLarge);
    }
    Ok(())
}

enum ReadError {
    TooLarge,
    Io(io::Error),
}

fn initial_capacity(input_len: usize, max: usize) -> usize {
    input_len
        .saturating_mul(4)
        .clamp(256, 4 << 20)
        .min(max.saturating_add(1))
}

/// Run one decompressor over `input`. On I/O failure: lenient mode keeps whatever was produced
/// (if anything), strict mode reports the error.
fn run<R: Read>(
    mut reader: R,
    out: &mut Vec<u8>,
    max: usize,
    lenient: bool,
) -> std::result::Result<bool, DecodeFailure> {
    let before = out.len();
    match read_capped(&mut reader, out, max) {
        Ok(()) => Ok(true),
        Err(ReadError::TooLarge) => Err(DecodeFailure::TooLarge(max)),
        Err(ReadError::Io(e)) if lenient && out.len() > before => {
            let _ = e;
            Ok(false)
        }
        Err(ReadError::Io(e)) => Err(DecodeFailure::Io(e.to_string())),
    }
}

/// Decode one coding. Returns the decoded bytes and whether the stream was complete.
pub(crate) fn decode_one(
    coding: Coding,
    input: &[u8],
    max: usize,
    lenient: bool,
) -> std::result::Result<(Vec<u8>, bool), DecodeFailure> {
    match coding {
        Coding::Identity => {
            if input.len() > max {
                return Err(DecodeFailure::TooLarge(max));
            }
            Ok((input.to_vec(), true))
        }
        Coding::Gzip => decode_gzip(input, max, lenient),
        Coding::Deflate => decode_deflate(input, max, lenient),
        Coding::Brotli => {
            let mut out = Vec::with_capacity(initial_capacity(input.len(), max));
            let complete = run(
                brotli::Decompressor::new(input, 8192),
                &mut out,
                max,
                lenient,
            )?;
            Ok((out, complete))
        }
        Coding::Zstd => {
            let mut out = Vec::with_capacity(initial_capacity(input.len(), max));
            let decoder = zstd::stream::read::Decoder::with_buffer(input)
                .map_err(|e| DecodeFailure::Io(e.to_string()))?;
            let complete = run(decoder, &mut out, max, lenient)?;
            Ok((out, complete))
        }
        Coding::Base64 => decode_base64(input, max),
    }
}

fn decode_gzip(
    input: &[u8],
    max: usize,
    lenient: bool,
) -> std::result::Result<(Vec<u8>, bool), DecodeFailure> {
    let mut out = Vec::with_capacity(initial_capacity(input.len(), max));
    let mut rest = input;
    loop {
        let mut decoder = flate2::bufread::GzDecoder::new(rest);
        let before = out.len();
        match read_capped(&mut decoder, &mut out, max) {
            Ok(()) => {}
            Err(ReadError::TooLarge) => return Err(DecodeFailure::TooLarge(max)),
            Err(ReadError::Io(e)) => {
                let later_member = rest.len() != input.len();
                if later_member && out.len() == before {
                    // Something after a complete member that merely starts with the gzip magic
                    // but doesn't decode: trailing junk, ignored like browsers do. In lenient
                    // mode an unexpected EOF there means a capture cut inside the next member's
                    // header, so the result is reported as incomplete.
                    let cut = lenient && e.kind() == io::ErrorKind::UnexpectedEof;
                    return Ok((out, !cut));
                }
                if lenient && !out.is_empty() {
                    return Ok((out, false));
                }
                return Err(DecodeFailure::Io(e.to_string()));
            }
        }
        rest = decoder.into_inner();
        // Another member follows only if it starts with the gzip magic; anything else (padding,
        // junk) is ignored.
        if rest.len() >= 2 && rest[0] == 0x1f && rest[1] == 0x8b {
            continue;
        }
        return Ok((out, true));
    }
}

/// zlib header check (RFC 1950): CM = 8, CINFO <= 7, FCHECK makes the header a multiple of 31.
fn looks_like_zlib(input: &[u8]) -> bool {
    input.len() >= 2
        && input[0] & 0x0f == 8
        && input[0] >> 4 <= 7
        && (u16::from(input[0]) << 8 | u16::from(input[1])) % 31 == 0
}

fn decode_deflate(
    input: &[u8],
    max: usize,
    lenient: bool,
) -> std::result::Result<(Vec<u8>, bool), DecodeFailure> {
    let cap = initial_capacity(input.len(), max);
    if looks_like_zlib(input) {
        let mut out = Vec::with_capacity(cap);
        match run(
            flate2::bufread::ZlibDecoder::new(input),
            &mut out,
            max,
            false,
        ) {
            Ok(_) => return Ok((out, true)),
            Err(DecodeFailure::TooLarge(m)) => return Err(DecodeFailure::TooLarge(m)),
            Err(zlib_err) => {
                // Rare: a raw stream whose first two bytes happen to form a valid zlib header.
                let mut raw = Vec::with_capacity(cap);
                match run(
                    flate2::bufread::DeflateDecoder::new(input),
                    &mut raw,
                    max,
                    false,
                ) {
                    Ok(_) => return Ok((raw, true)),
                    Err(DecodeFailure::TooLarge(m)) => return Err(DecodeFailure::TooLarge(m)),
                    Err(_) if lenient && !out.is_empty() => return Ok((out, false)),
                    Err(_) => return Err(zlib_err),
                }
            }
        }
    }
    let mut out = Vec::with_capacity(cap);
    let complete = run(
        flate2::bufread::DeflateDecoder::new(input),
        &mut out,
        max,
        lenient,
    )?;
    Ok((out, complete))
}

fn decode_base64(input: &[u8], max: usize) -> std::result::Result<(Vec<u8>, bool), DecodeFailure> {
    use base64::Engine as _;
    let cleaned: Vec<u8> = input
        .iter()
        .copied()
        .filter(|b| !b.is_ascii_whitespace())
        .collect();
    if cleaned.len() / 4 * 3 > max.saturating_add(3) {
        return Err(DecodeFailure::TooLarge(max));
    }
    let engine = if cleaned.iter().any(|&b| b == b'-' || b == b'_') {
        crate::util::B64_URL_LENIENT
    } else {
        crate::util::B64_LENIENT
    };
    let out = engine
        .decode(&cleaned)
        .map_err(|e| DecodeFailure::Io(format!("invalid base64: {e}")))?;
    if out.len() > max {
        return Err(DecodeFailure::TooLarge(max));
    }
    Ok((out, true))
}

/// Encode one coding; `None` means "unchanged" (identity).
fn encode_one(coding: Coding, input: &[u8]) -> io::Result<Option<Vec<u8>>> {
    let capacity = input.len() / 2 + 64;
    Ok(Some(match coding {
        Coding::Identity => return Ok(None),
        Coding::Gzip => {
            let mut enc = flate2::write::GzEncoder::new(
                Vec::with_capacity(capacity),
                flate2::Compression::new(6),
            );
            enc.write_all(input)?;
            enc.finish()?
        }
        Coding::Deflate => {
            let mut enc = flate2::write::ZlibEncoder::new(
                Vec::with_capacity(capacity),
                flate2::Compression::new(6),
            );
            enc.write_all(input)?;
            enc.finish()?
        }
        Coding::Brotli => {
            let mut enc = brotli::CompressorWriter::new(Vec::with_capacity(capacity), 8192, 5, 22);
            enc.write_all(input)?;
            enc.flush()?;
            enc.into_inner()
        }
        Coding::Zstd => zstd::stream::encode_all(input, 3)?,
        Coding::Base64 => crate::util::b64(input).into_bytes(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    const MAX: usize = 64 << 20;

    fn sample() -> Vec<u8> {
        let mut v = Vec::new();
        for i in 0..2000 {
            v.extend_from_slice(
                format!("line {i}: the quick brown fox jumps over the lazy dog\n").as_bytes(),
            );
        }
        v
    }

    #[test]
    fn round_trips_every_coding() {
        let body = sample();
        for enc in [
            "gzip",
            "x-gzip",
            "deflate",
            "x-deflate",
            "br",
            "zstd",
            "base64",
            "identity",
            "amz-1.0",
        ] {
            let chain = s(&[enc]);
            let encoded = encode_body(&body, &chain).unwrap();
            if !matches!(enc, "identity" | "amz-1.0" | "base64") {
                assert!(encoded.len() < body.len() / 4, "{enc} should compress");
            }
            let decoded = decode_body(&encoded, &chain, MAX).unwrap();
            assert_eq!(&decoded[..], &body[..], "{enc}");
        }
    }

    #[test]
    fn chains_decode_right_to_left() {
        let body = sample();
        let chain = s(&["gzip", "br", "zstd"]);
        let encoded = encode_body(&body, &chain).unwrap();
        // outermost is zstd
        assert_eq!(&encoded[..4], &[0x28, 0xb5, 0x2f, 0xfd]);
        assert_eq!(&decode_body(&encoded, &chain, MAX).unwrap()[..], &body[..]);
        // Wrong order fails.
        assert!(decode_body(&encoded, &s(&["zstd", "br", "gzip"]), MAX).is_err());
        // Identity entries are skipped anywhere in the chain; names are case-insensitive.
        let chain2 = s(&["identity", " GZIP ", "identity"]);
        let enc2 = encode_body(&body, &chain2).unwrap();
        assert_eq!(&decode_body(&enc2, &chain2, MAX).unwrap()[..], &body[..]);
    }

    #[test]
    fn raw_deflate_is_autodetected() {
        let body = sample();
        let mut enc =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(&body).unwrap();
        let raw = enc.finish().unwrap();
        assert!(!looks_like_zlib(&raw));
        assert_eq!(
            &decode_body(&raw, &s(&["deflate"]), MAX).unwrap()[..],
            &body[..]
        );
        let zlib = encode_body(&body, &s(&["deflate"])).unwrap();
        assert!(looks_like_zlib(&zlib));
        assert_eq!(
            &decode_body(&zlib, &s(&["deflate"]), MAX).unwrap()[..],
            &body[..]
        );
    }

    #[test]
    fn multi_member_gzip_and_trailing_garbage() {
        let a = encode_body(b"hello ", &s(&["gzip"])).unwrap();
        let b = encode_body(b"world", &s(&["gzip"])).unwrap();
        let mut both = a.to_vec();
        both.extend_from_slice(&b);
        assert_eq!(
            &decode_body(&both, &s(&["gzip"]), MAX).unwrap()[..],
            b"hello world"
        );
        both.extend_from_slice(b"\0\0\0junk");
        assert_eq!(
            &decode_body(&both, &s(&["x-gzip"]), MAX).unwrap()[..],
            b"hello world"
        );
    }

    #[test]
    fn capture_cut_inside_a_later_gzip_member_is_incomplete() {
        let a = encode_body(b"first ", &s(&["gzip"])).unwrap();
        let b = encode_body(b"second", &s(&["gzip"])).unwrap();
        let mut cut = a.to_vec();
        cut.extend_from_slice(&b[..6]); // inside the second member's header
        let (out, complete) = decode_body_partial(&cut, &s(&["gzip"]), MAX).unwrap();
        assert_eq!(&out[..], b"first ");
        assert!(!complete);
        // Strict decoding still treats it as trailing junk after a complete member.
        assert_eq!(
            &decode_body(&cut, &s(&["gzip"]), MAX).unwrap()[..],
            b"first "
        );
    }

    #[test]
    fn zstd_multi_frame() {
        let mut both = encode_body(b"abc", &s(&["zstd"])).unwrap().to_vec();
        both.extend_from_slice(&encode_body(b"def", &s(&["zstd"])).unwrap());
        assert_eq!(
            &decode_body(&both, &s(&["zstd"]), MAX).unwrap()[..],
            b"abcdef"
        );
    }

    #[test]
    fn zip_bomb_is_capped() {
        let zeros = vec![0u8; 10 << 20];
        for enc in ["gzip", "deflate", "br", "zstd"] {
            let chain = s(&[enc]);
            let bomb = encode_body(&zeros, &chain).unwrap();
            assert!(bomb.len() < 64 << 10, "{enc} bomb is small: {}", bomb.len());
            let err = decode_body(&bomb, &chain, 1 << 20).unwrap_err();
            assert_eq!(
                err,
                ContentError::Decode {
                    encoding: enc.to_string(),
                    message: "decoded body exceeds 1048576 bytes".into()
                }
            );
            assert!(err.to_string().contains("exceeds 1048576 bytes"));
            // Exactly at the cap is fine.
            assert_eq!(
                decode_body(&bomb, &chain, 10 << 20).unwrap().len(),
                10 << 20
            );
            // Partial decoding still enforces the cap.
            assert!(decode_body_partial(&bomb, &chain, 1 << 20).is_err());
        }
    }

    #[test]
    fn errors() {
        assert_eq!(
            decode_body(b"x", &s(&["compress"]), MAX).unwrap_err(),
            ContentError::UnsupportedEncoding("compress".into())
        );
        assert!(encode_body(b"x", &s(&["gzip", "lzma"])).is_err());
        let err = decode_body(b"not gzip at all", &s(&["gzip"]), MAX).unwrap_err();
        assert!(matches!(err, ContentError::Decode { ref encoding, .. } if encoding == "gzip"));
        assert!(decode_body(b"\xff\xff\xff\xff", &s(&["br"]), MAX).is_err());
        assert!(decode_body(b"garbage!", &s(&["zstd"]), MAX).is_err());
        assert!(decode_body(b"garbage!", &s(&["deflate"]), MAX).is_err());
        // Empty bodies decode to empty for known codings, but unknown ones still error.
        assert_eq!(decode_body(b"", &s(&["gzip", "br"]), MAX).unwrap().len(), 0);
        assert!(decode_body(b"", &s(&["bogus"]), MAX).is_err());
        assert_eq!(&decode_body(b"plain", &[], MAX).unwrap()[..], b"plain");
    }

    #[test]
    fn truncated_streams() {
        // Large enough to span several zstd blocks / brotli meta-blocks (a decoder can only
        // emit output for complete blocks).
        let body: Vec<u8> = (0..20_000u32)
            .flat_map(|i| format!("{i}:{}\n", i.wrapping_mul(2_654_435_761)).into_bytes())
            .collect();
        for enc in ["gzip", "deflate", "br", "zstd"] {
            let chain = s(&[enc]);
            let full = encode_body(&body, &chain).unwrap();
            let cut = &full[..full.len() * 2 / 3];
            assert!(
                decode_body(cut, &chain, MAX).is_err(),
                "{enc} strict must fail on truncation"
            );
            let (partial, complete) = decode_body_partial(cut, &chain, MAX).unwrap();
            assert!(!complete, "{enc}");
            assert!(!partial.is_empty(), "{enc}");
            assert!(
                body.starts_with(&partial),
                "{enc} partial output is a prefix"
            );
            let (all, complete) = decode_body_partial(&full, &chain, MAX).unwrap();
            assert!(complete);
            assert_eq!(&all[..], &body[..]);
        }
    }

    #[test]
    fn base64_coding() {
        assert_eq!(
            &decode_body(b"aGVs\nbG8=", &s(&["base64"]), MAX).unwrap()[..],
            b"hello"
        );
        assert_eq!(
            &decode_body(b"aGVsbG8", &s(&["base64"]), MAX).unwrap()[..],
            b"hello"
        );
        assert_eq!(
            &decode_body(b"-_8", &s(&["base64"]), MAX).unwrap()[..],
            &[0xfb, 0xff]
        );
        assert!(decode_body(b"!!!", &s(&["base64"]), MAX).is_err());
    }

    #[test]
    fn supported_names() {
        for n in [
            "gzip",
            "X-GZIP",
            "deflate",
            "x-deflate",
            "br",
            "zstd",
            "identity",
            " Br ",
        ] {
            assert!(is_supported(n), "{n}");
        }
        for n in ["compress", "lzma", "snappy", "brotli"] {
            assert!(!is_supported(n), "{n}");
        }
    }
}
