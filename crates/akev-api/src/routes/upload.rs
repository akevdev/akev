//! Large request bodies (imports) read without holding the upload twice.
//!
//! Axum's `Bytes` extractor collects the chunks of a body and then concatenates them: a
//! 500 MB HAR briefly costs 1 GB. Imports read their body here instead:
//!
//! * [`read_whole`] appends each chunk to one buffer, sized up front from `Content-Length`,
//!   and drops the chunk: one copy, for parsers that need the whole input (HAR, Charles,
//!   mitmproxy).
//! * [`BodyReader`] hands chunks to a blocking parser as they arrive, for formats that
//!   stream (`.akev` sessions): the upload is never in memory as a whole.
//!
//! Both stop at `limit` bytes.

use crate::error::{ApiError, ApiResult};
use axum::body::Body;
use bytes::{Bytes, BytesMut};
use futures::StreamExt;
use http::HeaderMap;
use std::io;

/// The `Content-Length` of a request, if it declared one.
pub(crate) fn content_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(http::header::CONTENT_LENGTH)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn too_large(limit: usize) -> ApiError {
    ApiError::payload_too_large(format!(
        "the upload is larger than {} MB",
        limit / (1024 * 1024)
    ))
}

/// Read a whole request body into a single buffer (at most `limit` bytes).
pub(crate) async fn read_whole(
    body: Body,
    size_hint: Option<u64>,
    limit: usize,
) -> ApiResult<Bytes> {
    if size_hint.is_some_and(|n| n > limit as u64) {
        return Err(too_large(limit));
    }
    let capacity = size_hint
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(0)
        .min(limit);
    let mut buf = BytesMut::with_capacity(capacity);
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| ApiError::bad_request(format!("upload failed: {e}")))?;
        if buf.len() + chunk.len() > limit {
            return Err(too_large(limit));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf.freeze())
}

/// An [`io::Read`] over a request body for a parser on a blocking thread. The body is read
/// by a task that hands over a few chunks at a time, so memory stays at a few chunks.
pub(crate) struct BodyReader {
    rx: tokio::sync::mpsc::Receiver<io::Result<Bytes>>,
    current: Bytes,
}

/// Chunks read ahead of the parser.
const READ_AHEAD: usize = 8;

impl BodyReader {
    /// Start reading `body` (at most `limit` bytes; more fails the read). Call from a Tokio
    /// context; read from a blocking thread.
    pub(crate) fn new(body: Body, limit: usize) -> Self {
        let (tx, rx) = tokio::sync::mpsc::channel(READ_AHEAD);
        tokio::spawn(async move {
            let mut stream = body.into_data_stream();
            let mut total = 0usize;
            while let Some(chunk) = stream.next().await {
                let item = match chunk {
                    Ok(b) => {
                        total += b.len();
                        if total > limit {
                            Err(io::Error::other(format!(
                                "the upload is larger than {} MB",
                                limit / (1024 * 1024)
                            )))
                        } else {
                            Ok(b)
                        }
                    }
                    Err(e) => Err(io::Error::other(format!("upload failed: {e}"))),
                };
                let failed = item.is_err();
                if tx.send(item).await.is_err() || failed {
                    return;
                }
            }
        });
        BodyReader {
            rx,
            current: Bytes::new(),
        }
    }

    /// Whether the body is empty (waits for the first chunk; blocking thread only).
    pub(crate) fn is_empty(&mut self) -> io::Result<bool> {
        self.fill()?;
        Ok(self.current.is_empty())
    }

    fn fill(&mut self) -> io::Result<()> {
        while self.current.is_empty() {
            match self.rx.blocking_recv() {
                Some(Ok(b)) => self.current = b,
                Some(Err(e)) => return Err(e),
                None => break,
            }
        }
        Ok(())
    }
}

impl io::Read for BodyReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        self.fill()?;
        let n = out.len().min(self.current.len());
        out[..n].copy_from_slice(&self.current[..n]);
        let _ = self.current.split_to(n);
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn chunked(parts: &[&'static [u8]]) -> Body {
        let parts: Vec<Result<Bytes, io::Error>> =
            parts.iter().map(|p| Ok(Bytes::from_static(p))).collect();
        Body::from_stream(futures::stream::iter(parts))
    }

    #[tokio::test]
    async fn whole_bodies_are_read_into_one_buffer() {
        let body = read_whole(chunked(&[b"ab", b"cd", b"e"]), Some(5), 100)
            .await
            .unwrap();
        assert_eq!(&body[..], b"abcde");
        assert!(
            read_whole(chunked(&[b"abc", b"def"]), None, 4)
                .await
                .is_err()
        );
        assert!(read_whole(chunked(&[b"a"]), Some(500), 100).await.is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_reader_streams_chunks_to_a_blocking_parser() {
        let mut reader = BodyReader::new(chunked(&[b"hello ", b"", b"streamed ", b"world"]), 100);
        let text = tokio::task::spawn_blocking(move || {
            assert!(!reader.is_empty().unwrap());
            let mut s = String::new();
            reader.read_to_string(&mut s).unwrap();
            s
        })
        .await
        .unwrap();
        assert_eq!(text, "hello streamed world");

        let mut reader = BodyReader::new(chunked(&[b"0123456789", b"0123456789"]), 15);
        let err = tokio::task::spawn_blocking(move || {
            let mut v = Vec::new();
            reader.read_to_end(&mut v).unwrap_err()
        })
        .await
        .unwrap();
        assert!(err.to_string().contains("larger than"));

        let mut empty = BodyReader::new(Body::empty(), 10);
        assert!(
            tokio::task::spawn_blocking(move || empty.is_empty().unwrap())
                .await
                .unwrap()
        );
    }
}
