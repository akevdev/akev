//! The LLM conversation view of an exchange (`GET /api/exchanges/:id/llm`, MCP
//! `akev_llm_conversation`): gathers the decoded bodies, the timed SSE events and the timing
//! of a captured call and hands them to [`akev_content::llm::parse`].

use crate::app::App;
use akev_content::llm::{self, LlmExchange, LlmInput, StreamEvent};
use akev_core::{CapturedBody, Direction, ExchangeId, ExchangeState, Headers, MessageKind, Side};
use bytes::Bytes;

/// Bodies are decoded up to this size for the conversation view; larger ones are cut there
/// (the view then says the response was only partly captured).
const MAX_LLM_BODY: usize = 64 << 20;

/// Result of [`conversation`].
pub enum Lookup {
    /// No such exchange.
    NotFound,
    /// The exchange isn't a recognised LLM API call.
    NotLlm,
    Found(Box<LlmExchange>),
}

/// Decoded body bytes (content-encoding removed; a capture cut short decodes leniently) and
/// whether they are only a prefix of the body.
fn decoded(body: Option<CapturedBody>, headers: &Headers) -> (Bytes, bool) {
    let Some(body) = body else {
        return (Bytes::new(), false);
    };
    let encodings = headers.content_encodings();
    let (bytes, cut) = if encodings.is_empty() {
        (body.bytes, false)
    } else if body.truncated {
        match akev_content::decode::decode_body_partial(&body.bytes, &encodings, MAX_LLM_BODY) {
            Ok((b, complete)) => (b, !complete),
            Err(_) => (body.bytes, false),
        }
    } else {
        match akev_content::decode::decode_body(&body.bytes, &encodings, MAX_LLM_BODY) {
            Ok(b) => (b, false),
            Err(_) => (body.bytes, false),
        }
    };
    if bytes.len() > MAX_LLM_BODY {
        return (bytes.slice(..MAX_LLM_BODY), true);
    }
    (bytes, cut || body.truncated)
}

/// Build the conversation view of exchange `id`. CPU heavy for large prompts: run it on the
/// blocking pool (under a body-view permit).
pub fn conversation(app: &App, id: ExchangeId) -> Lookup {
    let Some(detail) = app.store.detail(id) else {
        return Lookup::NotFound;
    };
    let s = &detail.summary;
    if llm::route(&s.host, s.port, &s.path).is_none() {
        return Lookup::NotLlm;
    }
    let (request_body, _) = decoded(app.store.body(id, Side::Request), &detail.request.headers);
    let response_headers = detail
        .response
        .as_ref()
        .map(|r| r.headers.clone())
        .unwrap_or_default();
    let (response_body, response_truncated) = match &detail.response {
        Some(_) => decoded(app.store.body(id, Side::Response), &response_headers),
        None => (Bytes::new(), false),
    };
    // The SSE events as the proxy recorded them, with their arrival times.
    let start = detail.timing.start.0;
    let events: Vec<StreamEvent> = if s.message_count > 0 {
        app.store
            .messages(id)
            .into_iter()
            .filter(|m| {
                m.message.kind == MessageKind::Event && m.message.direction == Direction::Received
            })
            .map(|m| StreamEvent {
                event: m.message.event,
                data: String::from_utf8_lossy(&m.payload).into_owned(),
                at_ms: Some((m.message.at.0 - start) as f64 / 1000.0),
            })
            .collect()
    } else {
        Vec::new()
    };
    let timing = &detail.timing;
    let finished = matches!(s.state, ExchangeState::Complete | ExchangeState::Aborted);
    let input = LlmInput {
        host: &s.host,
        port: s.port,
        path: &s.path,
        request_body: &request_body,
        response_status: detail.response.as_ref().map(|r| r.status),
        response_content_type: response_headers.get("content-type"),
        response_body: &response_body,
        response_truncated,
        events: &events,
        sent_ms: timing.request_sent.or(timing.request_body),
        done_ms: timing.response_body.or(timing.aborted),
        finished,
    };
    match llm::parse(&input) {
        Some(x) => Lookup::Found(Box::new(x)),
        None => Lookup::NotLlm,
    }
}
