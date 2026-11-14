//! Request/response body plumbing: the client body abstraction, buffering, the capture tee that
//! records the request body while it streams upstream, and throttled replay of buffered bodies.

use crate::events::{Capture, ExchangeEvents};
use crate::throttle::TokenBucket;
use crate::upstream::{BodyChunk, BodyStream, UpstreamError};
use akev_core::Headers;
use bytes::{Bytes, BytesMut};
use futures::{Stream, StreamExt};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::task::{Context, Poll};
use tokio::time::Sleep;

/// Per-exchange cap for bodies that must be fully buffered (rules matching on bodies,
/// transforms, breakpoints, scripts). Larger bodies stream through untouched.
pub(crate) const MAX_BUFFERED_BODY: usize = 512 * 1024 * 1024;
/// Zip-bomb guard when decoding bodies for matching / editing.
pub(crate) const MAX_DECODED_BODY: usize = 256 * 1024 * 1024;
/// Shown with an exchange whose body didn't fit the rule-body budget.
pub(crate) const PASSED_THROUGH_NOTICE: &str = "Body too large to apply rules; passed through";

/// One exchange's share of the global rule-body budget (`proxy.ruleBodyBudgetBytes`): the
/// bytes it holds for rules, scripts and breakpoints (wire copies and decoded copies). The
/// share is returned to the budget when the reservation is dropped.
pub(crate) struct Reservation<'a> {
    used: &'a AtomicU64,
    held: u64,
}

impl<'a> Reservation<'a> {
    /// A reservation against `used` (the budget's in-use counter), holding nothing yet.
    pub(crate) fn new(used: &'a AtomicU64) -> Self {
        Reservation { used, held: 0 }
    }

    /// Take `n` more bytes if the budget (`limit`, shared by all exchanges) allows it.
    pub(crate) fn grow(&mut self, n: u64, limit: u64) -> bool {
        let mut current = self.used.load(Ordering::Acquire);
        loop {
            let Some(next) = current.checked_add(n).filter(|next| *next <= limit) else {
                return false;
            };
            match self.used.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    self.held += n;
                    return true;
                }
                Err(actual) => current = actual,
            }
        }
    }

    /// What the budget has left (for bounding a decode).
    pub(crate) fn available(&self, limit: u64) -> u64 {
        limit.saturating_sub(self.used.load(Ordering::Acquire))
    }

    /// Give back everything held so far.
    pub(crate) fn release(&mut self) {
        if self.held > 0 {
            self.used.fetch_sub(self.held, Ordering::AcqRel);
            self.held = 0;
        }
    }
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        self.release();
    }
}

/// A body read in full, or one that didn't fit.
pub(crate) enum Collected {
    Full(Buffered),
    /// Too large for the per-exchange cap or the shared budget: replays what was read so
    /// far, then the rest of the stream, untouched.
    TooLarge(BodyStream),
}

/// Buffer a body stream completely, as long as it fits `cap` and the reservation can grow
/// (budget `limit`). Otherwise the bytes read so far are handed back in front of the rest
/// of the stream (and the reservation released), so the body can still be forwarded.
pub(crate) async fn collect_within(
    mut stream: BodyStream,
    cap: usize,
    reservation: &mut Reservation<'_>,
    limit: u64,
) -> Result<Collected, UpstreamError> {
    let mut parts: Vec<Bytes> = Vec::new();
    let mut total = 0usize;
    let mut trailers = None;
    while let Some(chunk) = stream.next().await {
        match chunk? {
            BodyChunk::Data(b) => {
                total += b.len();
                let fits = total <= cap && reservation.grow(b.len() as u64, limit);
                parts.push(b);
                if !fits {
                    reservation.release();
                    let read =
                        futures::stream::iter(parts.into_iter().map(|p| Ok(BodyChunk::Data(p))));
                    return Ok(Collected::TooLarge(Box::pin(read.chain(stream))));
                }
            }
            BodyChunk::Trailers(t) => trailers = Some(t),
        }
    }
    let data = match parts.len() {
        0 => Bytes::new(),
        1 => parts.pop().unwrap_or_default(),
        _ => {
            let mut buf = BytesMut::with_capacity(total);
            for p in parts {
                buf.extend_from_slice(&p);
            }
            buf.freeze()
        }
    };
    Ok(Collected::Full(Buffered { data, trailers }))
}

/// A request body as received from the client.
pub(crate) enum ClientBody {
    Empty,
    Full(Bytes),
    /// Still arriving; `length` = declared Content-Length.
    Stream {
        stream: BodyStream,
        length: Option<u64>,
    },
}

impl ClientBody {
    pub(crate) fn is_empty(&self) -> bool {
        match self {
            ClientBody::Empty => true,
            ClientBody::Full(b) => b.is_empty(),
            ClientBody::Stream { length, .. } => *length == Some(0),
        }
    }
}

/// A fully buffered body plus trailers.
#[derive(Clone, Debug, Default)]
pub(crate) struct Buffered {
    pub data: Bytes,
    pub trailers: Option<Headers>,
}

/// A buffered body replayed as a stream in throttled slices.
pub(crate) fn throttled_replay(
    data: Bytes,
    trailers: Option<Headers>,
    mut bucket: TokenBucket,
) -> BodyStream {
    let slice = bucket.slice_size();
    let stream = futures::stream::unfold((data, trailers), move |(mut data, trailers)| {
        let mut bucket_wait = None;
        let piece = if data.is_empty() {
            None
        } else {
            let p = data.split_to(slice.min(data.len()));
            bucket_wait = Some(bucket.take(p.len()));
            Some(p)
        };
        async move {
            match piece {
                Some(p) => {
                    if let Some(d) = bucket_wait.filter(|d| !d.is_zero()) {
                        tokio::time::sleep(d).await;
                    }
                    Some((Ok(BodyChunk::Data(p)), (data, trailers)))
                }
                None => trailers.map(|t| (Ok(BodyChunk::Trailers(t)), (Bytes::new(), None))),
            }
        }
    });
    Box::pin(stream)
}

/// Wraps the client's request body on its way upstream: captures a bounded copy, applies the
/// upload throttle, and emits `RequestCompleted` when the body ends (or a partial completion if
/// the consumer drops it early, e.g. the server answered before reading everything).
pub(crate) struct TeeStream {
    inner: BodyStream,
    capture: Capture,
    trailers: Option<Headers>,
    events: Arc<ExchangeEvents>,
    throttle: Option<TokenBucket>,
    sleep: Option<Pin<Box<Sleep>>>,
    /// Data released after the current throttle sleep, then the not-yet-sent remainder.
    ready: Option<Bytes>,
    rest: Option<Bytes>,
    client_failed: Arc<AtomicBool>,
    /// Declared body length (Content-Length), used when the consumer stops early.
    declared: Option<u64>,
    done: bool,
}

impl TeeStream {
    pub(crate) fn new(
        inner: BodyStream,
        events: Arc<ExchangeEvents>,
        capture_limit: usize,
        throttle: Option<TokenBucket>,
        client_failed: Arc<AtomicBool>,
    ) -> Self {
        TeeStream {
            inner,
            capture: Capture::new(capture_limit),
            trailers: None,
            events,
            throttle,
            sleep: None,
            ready: None,
            rest: None,
            client_failed,
            declared: None,
            done: false,
        }
    }

    /// Set the declared body length (reported as the wire size if the consumer stops early).
    pub(crate) fn with_declared_length(mut self, length: Option<u64>) -> Self {
        self.declared = length;
        self
    }

    /// Hand out the next piece of `data`, honouring the throttle.
    fn emit_piece(
        &mut self,
        cx: &mut Context<'_>,
        mut data: Bytes,
    ) -> Poll<Option<Result<BodyChunk, UpstreamError>>> {
        let Some(bucket) = self.throttle.as_mut() else {
            return Poll::Ready(Some(Ok(BodyChunk::Data(data))));
        };
        let slice = bucket.slice_size();
        if data.len() > slice {
            let rest = data.split_off(slice);
            self.rest = Some(rest);
        }
        let wait = bucket.take(data.len());
        if wait.is_zero() {
            return Poll::Ready(Some(Ok(BodyChunk::Data(data))));
        }
        let mut sleep = Box::pin(tokio::time::sleep(wait));
        match sleep.as_mut().poll(cx) {
            Poll::Ready(()) => Poll::Ready(Some(Ok(BodyChunk::Data(data)))),
            Poll::Pending => {
                self.sleep = Some(sleep);
                self.ready = Some(data);
                Poll::Pending
            }
        }
    }

    /// Emit `RequestCompleted`. `finished` = the body really ended; otherwise only a prefix was
    /// seen (the consumer stopped early), so the capture is marked truncated.
    fn complete(&mut self, finished: bool) {
        if !self.done {
            self.done = true;
            let mut body = self.capture.finish();
            if !finished {
                body.truncated = true;
                if let Some(n) = self.declared {
                    body.wire_size = body.wire_size.max(n);
                }
            }
            self.events.request_completed(body, self.trailers.take());
        }
    }
}

use std::future::Future as _;

impl Stream for TeeStream {
    type Item = Result<BodyChunk, UpstreamError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = &mut *self;
        if let Some(sleep) = this.sleep.as_mut() {
            if sleep.as_mut().poll(cx).is_pending() {
                return Poll::Pending;
            }
            this.sleep = None;
            if let Some(data) = this.ready.take() {
                return Poll::Ready(Some(Ok(BodyChunk::Data(data))));
            }
        }
        if let Some(rest) = this.rest.take() {
            return this.emit_piece(cx, rest);
        }
        if this.done {
            return Poll::Ready(None);
        }
        match this.inner.as_mut().poll_next(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Some(Ok(BodyChunk::Data(data)))) => {
                this.capture.push(&data);
                this.emit_piece(cx, data)
            }
            Poll::Ready(Some(Ok(BodyChunk::Trailers(t)))) => {
                this.trailers = Some(t.clone());
                Poll::Ready(Some(Ok(BodyChunk::Trailers(t))))
            }
            Poll::Ready(Some(Err(e))) => {
                this.client_failed.store(true, Ordering::Relaxed);
                this.done = true;
                Poll::Ready(Some(Err(e)))
            }
            Poll::Ready(None) => {
                this.complete(true);
                Poll::Ready(None)
            }
        }
    }
}

impl Drop for TeeStream {
    fn drop(&mut self) {
        // Consumer stopped early (server answered without reading everything): record what we
        // saw so the exchange still gets its RequestCompleted before the response events.
        if !self.done && !self.client_failed.load(Ordering::Relaxed) {
            self.complete(false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::tests::channel_events;
    use akev_core::{ErrorKind, ProxyEvent};

    fn stream_of(chunks: Vec<Result<BodyChunk, UpstreamError>>) -> BodyStream {
        Box::pin(futures::stream::iter(chunks))
    }

    async fn drain(stream: BodyStream) -> Vec<u8> {
        let mut out = Vec::new();
        let chunks: Vec<_> = stream.collect().await;
        for c in chunks {
            if let Ok(BodyChunk::Data(d)) = c {
                out.extend_from_slice(&d);
            }
        }
        out
    }

    #[tokio::test]
    async fn collect_concatenates_and_keeps_trailers() {
        let used = AtomicU64::new(0);
        let mut t = Headers::new();
        t.append("X-Checksum", "1");
        let s = stream_of(vec![
            Ok(BodyChunk::Data(Bytes::from_static(b"ab"))),
            Ok(BodyChunk::Data(Bytes::from_static(b"cd"))),
            Ok(BodyChunk::Trailers(t.clone())),
        ]);
        let mut r = Reservation::new(&used);
        let Collected::Full(b) = collect_within(s, 100, &mut r, 1000).await.unwrap() else {
            panic!("fits");
        };
        assert_eq!(&b.data[..], b"abcd");
        assert_eq!(b.trailers, Some(t));
        assert_eq!(used.load(Ordering::Relaxed), 4, "held while the body is");
        drop(r);
        assert_eq!(used.load(Ordering::Relaxed), 0, "returned when done");
    }

    #[tokio::test]
    async fn bodies_over_the_cap_or_budget_stream_through_whole() {
        let used = AtomicU64::new(0);
        let chunks = || {
            stream_of(vec![
                Ok(BodyChunk::Data(Bytes::from_static(b"abc"))),
                Ok(BodyChunk::Data(Bytes::from_static(b"def"))),
                Ok(BodyChunk::Data(Bytes::from_static(b"ghi"))),
            ])
        };
        // Per-exchange cap.
        let mut r = Reservation::new(&used);
        let Collected::TooLarge(rest) = collect_within(chunks(), 4, &mut r, 1000).await.unwrap()
        else {
            panic!("over the cap");
        };
        assert_eq!(used.load(Ordering::Relaxed), 0, "released on overflow");
        assert_eq!(drain(rest).await, b"abcdefghi", "nothing lost or reordered");
        // Shared budget: another exchange already holds most of it.
        let mut other = Reservation::new(&used);
        assert!(other.grow(995, 1000));
        let mut r = Reservation::new(&used);
        let Collected::TooLarge(rest) = collect_within(chunks(), 100, &mut r, 1000).await.unwrap()
        else {
            panic!("over the budget");
        };
        assert_eq!(drain(rest).await, b"abcdefghi");
        assert_eq!(used.load(Ordering::Relaxed), 995);
        drop(other);
        assert_eq!(used.load(Ordering::Relaxed), 0);
        assert_eq!(r.available(1000), 1000);
    }

    #[tokio::test]
    async fn tee_captures_and_completes() {
        let (ev, mut rx) = channel_events(true);
        let failed = Arc::new(AtomicBool::new(false));
        let s = stream_of(vec![
            Ok(BodyChunk::Data(Bytes::from_static(b"hello "))),
            Ok(BodyChunk::Data(Bytes::from_static(b"world"))),
        ]);
        let tee = TeeStream::new(s, ev.clone(), 8, None, failed);
        let out: Vec<_> = tee.collect().await;
        assert_eq!(out.len(), 2);
        match rx.try_recv().unwrap() {
            ProxyEvent::RequestCompleted { body, .. } => {
                assert_eq!(&body.bytes[..], b"hello wo");
                assert_eq!(body.wire_size, 11);
                assert!(body.truncated);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn tee_dropped_early_still_completes_but_not_on_client_error() {
        let (ev, mut rx) = channel_events(true);
        let failed = Arc::new(AtomicBool::new(false));
        let s = stream_of(vec![Ok(BodyChunk::Data(Bytes::from_static(b"partial")))]);
        let mut tee = TeeStream::new(s, ev, 100, None, failed).with_declared_length(Some(1000));
        let _ = tee.next().await;
        drop(tee);
        match rx.try_recv() {
            Ok(ProxyEvent::RequestCompleted { body, .. }) => {
                assert_eq!(&body.bytes[..], b"partial");
                assert!(body.truncated, "only a prefix was seen");
                assert_eq!(body.wire_size, 1000);
            }
            other => panic!("{other:?}"),
        }

        let (ev, mut rx) = channel_events(true);
        let failed = Arc::new(AtomicBool::new(false));
        let err = UpstreamError {
            kind: ErrorKind::ClientAborted,
            message: "eof".into(),
            code: None,
        };
        let s = stream_of(vec![Err(err)]);
        let mut tee = TeeStream::new(s, ev, 100, None, failed.clone());
        assert!(tee.next().await.unwrap().is_err());
        drop(tee);
        assert!(failed.load(Ordering::Relaxed));
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn tee_throttles_and_splits() {
        let (ev, _rx) = channel_events(false);
        let failed = Arc::new(AtomicBool::new(false));
        let data = Bytes::from(vec![1u8; 20_000]);
        let s = stream_of(vec![Ok(BodyChunk::Data(data))]);
        let tee = TeeStream::new(s, ev, 100, TokenBucket::new(100_000), failed);
        let start = std::time::Instant::now();
        let out: Vec<_> = tee.collect().await;
        assert!(out.len() > 1, "large chunk should be split");
        let total: usize = out
            .iter()
            .map(|c| match c {
                Ok(BodyChunk::Data(d)) => d.len(),
                _ => 0,
            })
            .sum();
        assert_eq!(total, 20_000);
        // 20KB at 100KB/s with a 5KB burst ≈ 150ms.
        assert!(start.elapsed() >= std::time::Duration::from_millis(100));
    }

    #[tokio::test]
    async fn throttled_replay_emits_everything() {
        let mut t = Headers::new();
        t.append("a", "b");
        let s = throttled_replay(
            Bytes::from(vec![0u8; 3000]),
            Some(t),
            TokenBucket::new(1_000_000).unwrap(),
        );
        let out: Vec<_> = s.collect().await;
        assert!(matches!(out.last(), Some(Ok(BodyChunk::Trailers(_)))));
    }
}
