//! The client side of an exchange as seen by the pipeline: an HTTP/1 connection, an HTTP/2
//! stream, or an in-process capture (Compose / Repeat). The pipeline only talks to this enum.

use crate::h2::H2Responder;
use crate::http1::server::H1Responder;
pub(crate) use crate::http1::write::BodyKind;
use crate::http3::H3Responder;
use akev_core::{Headers, ResponseHead};
use bytes::{Bytes, BytesMut};
use std::io;

/// How to terminate a response abnormally.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AbortKind {
    /// Close the connection (FIN) / end the stream without a response.
    Close,
    /// TCP RST / HTTP/2 RST_STREAM.
    Reset,
    /// The response was already started but can't be completed.
    Incomplete,
}

pub(crate) enum Responder<'a> {
    H1(&'a mut H1Responder),
    H2(&'a mut H2Responder),
    H3(&'a mut H3Responder),
    Capture(&'a mut CaptureResponder),
}

impl Responder<'_> {
    pub(crate) fn head_sent(&self) -> bool {
        match self {
            Responder::H1(r) => r.head_sent(),
            Responder::H2(r) => r.head_sent(),
            Responder::H3(r) => r.head_sent(),
            Responder::Capture(r) => r.head.is_some(),
        }
    }

    /// Send `100 Continue` if the client asked for it and nothing was sent yet.
    pub(crate) async fn send_continue(&mut self) {
        match self {
            Responder::H1(r) => r.send_continue().await,
            Responder::H2(r) => r.send_continue(),
            Responder::H3(r) => r.send_continue().await,
            Responder::Capture(_) => {}
        }
    }

    pub(crate) async fn send_head(
        &mut self,
        head: &ResponseHead,
        body: BodyKind,
    ) -> io::Result<()> {
        match self {
            Responder::H1(r) => r.send_head(head, body).await,
            Responder::H2(r) => r.send_head(head, body),
            Responder::H3(r) => r.send_head(head, body).await,
            Responder::Capture(r) => {
                r.head = Some(head.clone());
                Ok(())
            }
        }
    }

    pub(crate) async fn send_data(&mut self, data: Bytes) -> io::Result<()> {
        match self {
            Responder::H1(r) => r.send_data(data).await,
            Responder::H2(r) => r.send_data(data).await,
            Responder::H3(r) => r.send_data(data).await,
            Responder::Capture(r) => {
                r.push(&data);
                Ok(())
            }
        }
    }

    pub(crate) async fn finish(&mut self, trailers: Option<&Headers>) -> io::Result<()> {
        match self {
            Responder::H1(r) => r.finish(trailers).await,
            Responder::H2(r) => r.finish(trailers),
            Responder::H3(r) => r.finish(trailers).await,
            Responder::Capture(r) => {
                r.trailers = trailers.cloned();
                Ok(())
            }
        }
    }

    pub(crate) async fn abort(&mut self, kind: AbortKind) {
        match self {
            Responder::H1(r) => r.abort(kind).await,
            Responder::H2(r) => r.abort(kind),
            Responder::H3(r) => r.abort(kind),
            Responder::Capture(r) => r.aborted = true,
        }
    }

    /// Resolves when the client has gone away (connection closed / stream reset).
    pub(crate) async fn client_gone(&mut self) {
        match self {
            Responder::H1(r) => r.client_gone().await,
            Responder::H2(r) => r.client_gone().await,
            Responder::H3(r) => r.client_gone().await,
            Responder::Capture(_) => std::future::pending().await,
        }
    }

    /// Whether the connection can serve another request afterwards.
    pub(crate) fn keep_alive(&self) -> bool {
        match self {
            Responder::H1(r) => r.keep_alive(),
            Responder::H2(_) | Responder::H3(_) | Responder::Capture(_) => true,
        }
    }
}

/// Collects a response in memory (requests issued by Akev itself).
#[derive(Default)]
pub(crate) struct CaptureResponder {
    pub head: Option<ResponseHead>,
    pub body: BytesMut,
    pub trailers: Option<Headers>,
    pub aborted: bool,
    pub limit: usize,
    pub truncated: bool,
}

impl CaptureResponder {
    pub(crate) fn new(limit: usize) -> Self {
        CaptureResponder {
            limit,
            ..Default::default()
        }
    }

    fn push(&mut self, data: &Bytes) {
        let room = self.limit.saturating_sub(self.body.len());
        if data.len() > room {
            self.truncated = true;
        }
        self.body.extend_from_slice(&data[..data.len().min(room)]);
    }
}
