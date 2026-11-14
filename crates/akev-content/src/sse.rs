//! `text/event-stream` parsing per the WHATWG HTML "server-sent events" interpretation rules.
//!
//! * A leading UTF-8 BOM is stripped; lines end with CRLF, LF or CR (a CRLF split across chunks
//!   is handled); bytes are decoded as UTF-8 with replacement.
//! * Lines starting with `:` are comments. `field:value` lines have one leading space stripped
//!   from the value; a line without `:` is a field with an empty value.
//! * `data` lines append to the data buffer joined by `\n`; `event` sets the type; `id` sets the
//!   id unless it contains NUL; `retry` is accepted only when it is all ASCII digits.
//! * A blank line dispatches the event — only if the data buffer is non-empty (per spec, blocks
//!   without any `data` field are not events).
//!
//! [`SseEvent::id`] / [`SseEvent::retry`] are the values set *within that event's block* (not
//! the persistent last-event-id), which is what a traffic viewer wants to show.
//! [`SseParser::finish`] dispatches a final event that lacks the terminating blank line
//! (truncated streams), unlike a browser.

use serde::Serialize;

/// One dispatched server-sent event.
#[derive(Clone, Debug, PartialEq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SseEvent {
    /// `event:` type (None = the default `message`).
    pub event: Option<String>,
    /// `id:` set in this event's block.
    pub id: Option<String>,
    /// `retry:` (milliseconds) set in this event's block.
    pub retry: Option<u64>,
    /// Data lines joined with `\n`.
    pub data: String,
}

/// Incremental text/event-stream parser (feed chunks as they arrive).
#[derive(Default)]
pub struct SseParser {
    /// Bytes of the current, not yet terminated line.
    line: Vec<u8>,
    /// Whether the BOM check at stream start is done.
    started: bool,
    /// The previous chunk ended with CR: skip a leading LF.
    skip_lf: bool,
    data: String,
    event: String,
    id: Option<String>,
    retry: Option<u64>,
}

const BOM: &[u8] = b"\xEF\xBB\xBF";

impl SseParser {
    /// A fresh parser.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed bytes, returning any complete events.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        let mut events = Vec::new();
        let mut input = chunk;
        if !self.started {
            // The BOM may itself be split across chunks: buffer its bytes in `line`.
            while !input.is_empty()
                && self.line.len() < BOM.len()
                && input[0] == BOM[self.line.len()]
            {
                self.line.push(input[0]);
                input = &input[1..];
            }
            if self.line.len() == BOM.len() {
                self.line.clear();
                self.started = true;
            } else if input.is_empty() {
                return events; // still undecided
            } else {
                self.started = true; // partial BOM bytes were real content; keep them in `line`
            }
        }
        while !input.is_empty() {
            if self.skip_lf {
                self.skip_lf = false;
                if input[0] == b'\n' {
                    input = &input[1..];
                    continue;
                }
            }
            match memchr::memchr2(b'\r', b'\n', input) {
                None => {
                    self.line.extend_from_slice(input);
                    break;
                }
                Some(i) => {
                    if self.line.is_empty() {
                        self.process_line(&input[..i], &mut events);
                    } else {
                        self.line.extend_from_slice(&input[..i]);
                        let line = std::mem::take(&mut self.line);
                        self.process_line(&line, &mut events);
                        self.line = line;
                        self.line.clear();
                    }
                    if input[i] == b'\r' {
                        match input.get(i + 1) {
                            Some(b'\n') => input = &input[i + 2..],
                            Some(_) => input = &input[i + 1..],
                            None => {
                                self.skip_lf = true;
                                input = &[];
                            }
                        }
                    } else {
                        input = &input[i + 1..];
                    }
                }
            }
        }
        events
    }

    /// Flush a trailing event without final blank line.
    pub fn finish(&mut self) -> Option<SseEvent> {
        if !self.line.is_empty() {
            let line = std::mem::take(&mut self.line);
            // A non-empty line never dispatches by itself, so nothing lands in the sink.
            let mut sink = Vec::new();
            self.process_line(&line, &mut sink);
        }
        let last = self.dispatch();
        *self = Self::default();
        last
    }

    fn process_line(&mut self, line: &[u8], events: &mut Vec<SseEvent>) {
        if line.is_empty() {
            if let Some(e) = self.dispatch() {
                events.push(e);
            }
            return;
        }
        if line[0] == b':' {
            return;
        }
        let (name, value) = match memchr::memchr(b':', line) {
            Some(i) => {
                let v = &line[i + 1..];
                (&line[..i], v.strip_prefix(b" ").unwrap_or(v))
            }
            None => (line, &[][..]),
        };
        match name {
            b"data" => {
                self.data.push_str(&String::from_utf8_lossy(value));
                self.data.push('\n');
            }
            b"event" => self.event = String::from_utf8_lossy(value).into_owned(),
            b"id" => {
                if !value.contains(&0) {
                    self.id = Some(String::from_utf8_lossy(value).into_owned());
                }
            }
            b"retry" if !value.is_empty() && value.iter().all(u8::is_ascii_digit) => {
                // all ASCII digits; overflowing values are ignored
                if let Ok(n) = std::str::from_utf8(value).unwrap_or("").parse::<u64>() {
                    self.retry = Some(n);
                }
            }
            _ => {}
        }
    }

    fn dispatch(&mut self) -> Option<SseEvent> {
        let event = std::mem::take(&mut self.event);
        let id = self.id.take();
        let retry = self.retry.take();
        if self.data.is_empty() {
            return None;
        }
        let mut data = std::mem::take(&mut self.data);
        if data.ends_with('\n') {
            data.pop();
        }
        Some(SseEvent {
            event: (!event.is_empty()).then_some(event),
            id,
            retry,
            data,
        })
    }
}

/// Parse a complete body.
pub fn parse_all(body: &[u8]) -> Vec<SseEvent> {
    let mut parser = SseParser::new();
    let mut events = parser.push(body);
    events.extend(parser.finish());
    events
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(event: Option<&str>, id: Option<&str>, retry: Option<u64>, data: &str) -> SseEvent {
        SseEvent {
            event: event.map(Into::into),
            id: id.map(Into::into),
            retry,
            data: data.into(),
        }
    }

    #[test]
    fn basic_fields() {
        let body = b"event: update\nid: 7\nretry: 1500\ndata: {\"a\":1}\n\ndata: second\n\n";
        assert_eq!(
            parse_all(body),
            vec![
                ev(Some("update"), Some("7"), Some(1500), "{\"a\":1}"),
                ev(None, None, None, "second")
            ]
        );
    }

    #[test]
    fn spec_examples() {
        // Multi-line data, comments, field without colon, no space after colon.
        let body = b": comment\ndata: YHOO\ndata: +2\ndata\ndata:10\n\n";
        assert_eq!(
            parse_all(body),
            vec![ev(None, None, None, "YHOO\n+2\n\n10")]
        );
        // Only one leading space is stripped.
        assert_eq!(parse_all(b"data:  two\n\n")[0].data, " two");
        // "data:" alone dispatches an empty-data event; a block without data doesn't.
        assert_eq!(
            parse_all(b"data:\n\nid: 1\n\nretry: 5\nevent: x\n\n"),
            vec![ev(None, None, None, "")]
        );
        // Unknown fields are ignored; invalid retry ignored; id with NUL ignored.
        assert_eq!(
            parse_all(b"foo: bar\nretry: 1s\nid: a\0b\ndata: x\n\n"),
            vec![ev(None, None, None, "x")]
        );
        // Empty id is a valid (resetting) id.
        assert_eq!(parse_all(b"id\ndata: x\n\n")[0].id.as_deref(), Some(""));
    }

    #[test]
    fn line_endings_and_bom() {
        let crlf = b"\xEF\xBB\xBFdata: a\r\ndata: b\r\n\r\ndata: c\r\rdata: d\n\n";
        assert_eq!(
            parse_all(crlf),
            vec![
                ev(None, None, None, "a\nb"),
                ev(None, None, None, "c"),
                ev(None, None, None, "d")
            ]
        );
        // A BOM in the middle is content (field name becomes "\u{feff}data", ignored).
        assert_eq!(parse_all(b"data: x\n\n\xEF\xBB\xBFdata: y\n\n").len(), 1);
    }

    #[test]
    fn incremental_chunking_matches_whole() {
        let body: &[u8] =
            b"\xEF\xBB\xBFevent: e\r\ndata: hello\r\n\r\ndata: w\xc3\xb6rld\r\rid: 9\ndata: z\n\n";
        let whole = parse_all(body);
        for size in 1..body.len() {
            let mut p = SseParser::new();
            let mut got = Vec::new();
            for chunk in body.chunks(size) {
                got.extend(p.push(chunk));
            }
            got.extend(p.finish());
            assert_eq!(got, whole, "chunk size {size}");
        }
        assert_eq!(whole.len(), 3);
        assert_eq!(whole[1].data, "wörld");
    }

    #[test]
    fn finish_flushes_truncated_stream() {
        let mut p = SseParser::new();
        assert!(p.push(b"data: one\n\ndata: two").len() == 1);
        assert_eq!(p.finish(), Some(ev(None, None, None, "two")));
        assert_eq!(p.finish(), None);
        let mut p = SseParser::new();
        assert!(p.push(b"data: three\n").is_empty());
        assert_eq!(p.finish().map(|e| e.data), Some("three".into()));
        // A partial BOM followed by finish is just content.
        let mut p = SseParser::new();
        assert!(p.push(b"\xEF\xBB").is_empty());
        assert_eq!(p.finish(), None);
    }

    #[test]
    fn serializes_camel_case() {
        let v = serde_json::to_value(ev(Some("x"), None, Some(3), "d")).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"event": "x", "id": null, "retry": 3, "data": "d"})
        );
    }
}
