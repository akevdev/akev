//! Incremental HTTP/1 body decoding (Content-Length and chunked with trailers), operating
//! zero-copy on the connection's read buffer.

use super::parse::Framing;
use akev_core::{Headers, header_value_to_string};
use bytes::{Buf, Bytes, BytesMut};

const MAX_CHUNK_LINE: usize = 4096;
const MAX_TRAILERS: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Chunked {
    Size,
    Data(u64),
    DataEnd,
    Trailers,
}

/// Body decoder state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decoder {
    Length(u64),
    Chunked(ChunkedState),
    Done,
}

/// Opaque chunked-decoder state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChunkedState(Chunked);

/// One decoding step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decoded {
    Data(Bytes),
    Trailers(Headers),
    /// Body complete.
    Done,
    /// Need more bytes in the buffer.
    NeedMore,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct DecodeError(pub &'static str);

impl Decoder {
    pub fn new(framing: Framing) -> Self {
        match framing {
            Framing::Empty | Framing::Length(0) => Decoder::Done,
            Framing::Length(n) => Decoder::Length(n),
            Framing::Chunked => Decoder::Chunked(ChunkedState(Chunked::Size)),
        }
    }

    pub fn is_done(&self) -> bool {
        matches!(self, Decoder::Done)
    }

    /// Decode the next piece from `buf`, consuming what was used.
    pub fn decode(&mut self, buf: &mut BytesMut) -> Result<Decoded, DecodeError> {
        loop {
            match self {
                Decoder::Done => return Ok(Decoded::Done),
                Decoder::Length(remaining) => {
                    if buf.is_empty() {
                        return Ok(Decoded::NeedMore);
                    }
                    let n = (*remaining).min(buf.len() as u64) as usize;
                    let data = buf.split_to(n).freeze();
                    *remaining -= n as u64;
                    if *remaining == 0 {
                        *self = Decoder::Done;
                    }
                    return Ok(Decoded::Data(data));
                }
                Decoder::Chunked(ChunkedState(state)) => match state {
                    Chunked::Size => match parse_chunk_size(buf)? {
                        Some((used, size)) => {
                            buf.advance(used);
                            *state = if size == 0 {
                                Chunked::Trailers
                            } else {
                                Chunked::Data(size)
                            };
                        }
                        None => return Ok(Decoded::NeedMore),
                    },
                    Chunked::Data(remaining) => {
                        if buf.is_empty() {
                            return Ok(Decoded::NeedMore);
                        }
                        let n = (*remaining).min(buf.len() as u64) as usize;
                        let data = buf.split_to(n).freeze();
                        *remaining -= n as u64;
                        if *remaining == 0 {
                            *state = Chunked::DataEnd;
                        }
                        return Ok(Decoded::Data(data));
                    }
                    Chunked::DataEnd => {
                        if buf.starts_with(b"\r\n") {
                            buf.advance(2);
                        } else if buf.starts_with(b"\n") {
                            buf.advance(1);
                        } else if buf.is_empty() || buf[..] == b"\r"[..] {
                            return Ok(Decoded::NeedMore);
                        } else {
                            return Err(DecodeError("missing CRLF after chunk data"));
                        }
                        *state = Chunked::Size;
                    }
                    Chunked::Trailers => {
                        if buf.starts_with(b"\r\n") {
                            buf.advance(2);
                            *self = Decoder::Done;
                            return Ok(Decoded::Done);
                        }
                        if buf.starts_with(b"\n") {
                            buf.advance(1);
                            *self = Decoder::Done;
                            return Ok(Decoded::Done);
                        }
                        if buf.is_empty() || buf[..] == b"\r"[..] {
                            return Ok(Decoded::NeedMore);
                        }
                        let mut slots = [httparse::EMPTY_HEADER; 64];
                        match httparse::parse_headers(buf, &mut slots) {
                            Ok(httparse::Status::Complete((used, parsed))) => {
                                let mut trailers = Headers::with_capacity(parsed.len());
                                for h in parsed {
                                    trailers.append(
                                        h.name.to_string(),
                                        header_value_to_string(h.value),
                                    );
                                }
                                buf.advance(used);
                                *self = Decoder::Done;
                                return Ok(Decoded::Trailers(trailers));
                            }
                            Ok(httparse::Status::Partial) => {
                                return if buf.len() > MAX_TRAILERS {
                                    Err(DecodeError("trailers too large"))
                                } else {
                                    Ok(Decoded::NeedMore)
                                };
                            }
                            Err(_) => return Err(DecodeError("invalid trailers")),
                        }
                    }
                },
            }
        }
    }
}

/// Parse a chunk-size line (`HEX[;ext]CRLF`, bare LF tolerated). `Ok(None)` = incomplete.
fn parse_chunk_size(buf: &[u8]) -> Result<Option<(usize, u64)>, DecodeError> {
    let Some(nl) = buf
        .iter()
        .take(MAX_CHUNK_LINE + 1)
        .position(|&b| b == b'\n')
    else {
        return if buf.len() > MAX_CHUNK_LINE {
            Err(DecodeError("chunk size line too long"))
        } else {
            Ok(None)
        };
    };
    let line = &buf[..nl];
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    let digits_end = line
        .iter()
        .position(|b| !b.is_ascii_hexdigit())
        .unwrap_or(line.len());
    let (digits, rest) = line.split_at(digits_end);
    if digits.is_empty() || digits.len() > 16 {
        return Err(DecodeError("invalid chunk size"));
    }
    // Only whitespace and extensions may follow the size.
    let rest_ok = rest
        .iter()
        .position(|&b| b == b';')
        .map_or(rest, |i| &rest[..i])
        .iter()
        .all(|&b| b == b' ' || b == b'\t');
    if !rest_ok {
        return Err(DecodeError("invalid chunk size"));
    }
    let digits = std::str::from_utf8(digits).map_err(|_| DecodeError("invalid chunk size"))?;
    let size = u64::from_str_radix(digits, 16).map_err(|_| DecodeError("invalid chunk size"))?;
    Ok(Some((nl + 1, size)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_all(
        framing: Framing,
        input: &[u8],
        step: usize,
    ) -> (Vec<u8>, Option<Headers>, Vec<u8>, bool) {
        let mut d = Decoder::new(framing);
        let mut buf = BytesMut::new();
        let mut body = Vec::new();
        let mut trailers = None;
        let mut fed = 0;
        let mut done = false;
        loop {
            match d.decode(&mut buf).unwrap() {
                Decoded::Data(b) => body.extend_from_slice(&b),
                Decoded::Trailers(t) => trailers = Some(t),
                Decoded::Done => {
                    done = true;
                    break;
                }
                Decoded::NeedMore => {
                    if fed >= input.len() {
                        break;
                    }
                    let end = (fed + step).min(input.len());
                    buf.extend_from_slice(&input[fed..end]);
                    fed = end;
                }
            }
        }
        let mut rest = buf.to_vec();
        rest.extend_from_slice(&input[fed..]);
        (body, trailers, rest, done)
    }

    #[test]
    fn content_length_leaves_pipelined_bytes() {
        let (body, _, rest, done) = decode_all(Framing::Length(5), b"helloGET / HTTP/1.1", 3);
        assert_eq!(body, b"hello");
        assert_eq!(rest, b"GET / HTTP/1.1");
        assert!(done);
    }

    #[test]
    fn chunked_with_extensions_and_trailers_any_split() {
        let input = b"4;ext=1\r\nWiki\r\n5\r\npedia\r\nE\r\n in\r\n\r\nchunks.\r\n0\r\nX-Trailer: a\r\nX-Other: b\r\n\r\nNEXT";
        for step in [1, 2, 3, 7, 64] {
            let (body, trailers, rest, done) = decode_all(Framing::Chunked, input, step);
            assert_eq!(body, b"Wikipedia in\r\n\r\nchunks.", "step {step}");
            let t = trailers.unwrap();
            assert_eq!(t.get("x-trailer"), Some("a"));
            assert_eq!(t.0[1].0, "X-Other");
            assert_eq!(rest, b"NEXT");
            assert!(done);
        }
    }

    #[test]
    fn chunked_without_trailers_and_bare_lf() {
        let (body, trailers, rest, done) = decode_all(Framing::Chunked, b"3\nabc\n0\n\n", 2);
        assert_eq!(body, b"abc");
        assert!(trailers.is_none());
        assert!(rest.is_empty() && done);
    }

    #[test]
    fn chunked_errors() {
        let mut d = Decoder::new(Framing::Chunked);
        let mut buf = BytesMut::from(&b"zz\r\n"[..]);
        assert!(d.decode(&mut buf).is_err());
        let mut d = Decoder::new(Framing::Chunked);
        let mut buf = BytesMut::from(&b"3\r\nabcXX"[..]);
        assert_eq!(
            d.decode(&mut buf).unwrap(),
            Decoded::Data(Bytes::from_static(b"abc"))
        );
        assert!(d.decode(&mut buf).is_err());
        let mut d = Decoder::new(Framing::Chunked);
        let mut buf = BytesMut::from(&vec![b'1'; MAX_CHUNK_LINE + 10][..]);
        assert!(d.decode(&mut buf).is_err());
    }

    #[test]
    fn incomplete_body_is_not_done() {
        let (body, _, _, done) = decode_all(Framing::Length(10), b"abc", 10);
        assert_eq!(body, b"abc");
        assert!(!done);
        assert!(Decoder::new(Framing::Empty).is_done());
        assert_eq!(Decoder::new(Framing::Length(4)), Decoder::Length(4));
    }
}
