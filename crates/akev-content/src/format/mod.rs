//! Pretty-printers. All formatters are single-pass, linear-time and use explicit stacks (no
//! recursion), so multi-megabyte and deeply nested inputs are safe. Indentation is two spaces
//! (capped at 100 levels). Output size is bounded too: a formatter that would produce more than
//! 16× its input + 4 MiB (only reachable with pathological nesting) returns `None` instead
//! (JavaScript: 8× + 1 MiB).
//!
//! | Kind | Behaviour |
//! |---|---|
//! | `Json` | tolerant re-indenter (see [`pretty`]); exact canonical layout for valid JSON, readable output for broken JSON |
//! | `JsonLines` | each record formatted, records separated by newlines |
//! | `Xml` | `quick-xml` events → indented, markup copied verbatim |
//! | `Html` | tag-aware indenter; `script`/`style` contents beautified |
//! | `Css` | one declaration per line, nested blocks indented |
//! | `Javascript` | token-based beautifier that never changes semantics |
//! | `GraphQl` | query/SDL formatter; JSON request envelopes are formatted as JSON |
//! | `FormUrlEncoded` | one decoded `key=value` per line |
//! | `EventStream` | events as a formatted JSON array (`event`/`id`/`retry`/`data`); JSON `data` is inlined verbatim (like HTTP Toolkit's `formatEventStreamAsJson`) |
//! | `Image` | SVG documents are formatted as XML |
//! | `Yaml`, `Markdown`, `Csv`, `Text`, binary kinds | `None` (shown as-is) |

pub(crate) mod css;
pub(crate) mod graphql;
pub(crate) mod html;
pub(crate) mod js;
pub(crate) mod json;
pub(crate) mod xml;

use akev_core::ContentKind;

/// Pretty-print content of the given kind. Returns `None` if the kind isn't formattable, the
/// content fails to parse, or the output would exceed the size budget (callers then show raw
/// text). Must be fast on multi-MB inputs (linear time; no recursion-depth blowups).
///
/// JSON is never rejected for being invalid: it is re-indented by a tolerant token-based
/// formatter (a port of HTTP Toolkit's `formatJson`) so slightly broken JSON — comments,
/// trailing commas, truncated captures — still formats; only text that can't start a JSON
/// document (e.g. an HTML error page) yields `None`. YAML and Markdown are returned as `None`
/// (they're already human-oriented; the UI highlights the raw text).
pub fn pretty(kind: ContentKind, text: &str) -> Option<String> {
    match kind {
        ContentKind::Json => json::pretty(text),
        ContentKind::JsonLines => json::pretty_records(text),
        ContentKind::Xml => xml::pretty(text),
        ContentKind::Html => html::pretty(text),
        ContentKind::Css => css::pretty(text, 0),
        ContentKind::Javascript => js::pretty(text, 0),
        ContentKind::GraphQl => graphql::pretty(text),
        ContentKind::FormUrlEncoded => form(text),
        ContentKind::EventStream => event_stream(text),
        ContentKind::Image if crate::image::is_svg(text.as_bytes()) => xml::pretty(text),
        _ => None,
    }
}

/// Server-sent events as a formatted JSON array. `data` that is valid JSON is inserted
/// verbatim (so big numbers and key order survive); anything else becomes a JSON string.
/// `None` when the stream holds no events. Events are written as they are parsed (never
/// collected), so memory stays proportional to the text.
fn event_stream(text: &str) -> Option<String> {
    /// Bytes fed to the parser at a time.
    const CHUNK: usize = 64 * 1024;
    fn write_event(out: &mut String, e: &crate::sse::SseEvent) -> Option<()> {
        let quoted = |s: &str| serde_json::to_string(s).ok();
        if out.len() > 1 {
            out.push(',');
        }
        out.push('{');
        if let Some(ev) = &e.event {
            out.push_str("\"event\":");
            out.push_str(&quoted(ev)?);
            out.push(',');
        }
        if let Some(id) = &e.id {
            out.push_str("\"id\":");
            out.push_str(&quoted(id)?);
            out.push(',');
        }
        if let Some(retry) = e.retry {
            out.push_str(&format!("\"retry\":{retry},"));
        }
        out.push_str("\"data\":");
        let data = e.data.trim();
        if !data.is_empty() && json::is_valid(data) {
            out.push_str(data);
        } else {
            out.push_str(&quoted(&e.data)?);
        }
        out.push('}');
        Some(())
    }

    let mut parser = crate::sse::SseParser::new();
    let mut out = String::with_capacity(text.len() + 2);
    out.push('[');
    for chunk in text.as_bytes().chunks(CHUNK) {
        for e in parser.push(chunk) {
            write_event(&mut out, &e)?;
        }
    }
    if let Some(e) = parser.finish() {
        write_event(&mut out, &e)?;
    }
    if out.len() == 1 {
        return None;
    }
    out.push(']');
    json::pretty(&out)
}

/// Minify JSON (used by "copy as compact JSON"). Returns `None` unless the input is strictly
/// valid JSON (RFC 8259; a leading BOM is tolerated). Tokens are copied verbatim, so numbers
/// and escapes are preserved exactly.
pub fn minify_json(text: &str) -> Option<String> {
    json::minify(text)
}

/// Whether `text` is strictly valid JSON (RFC 8259), with no nesting limit.
pub fn is_valid_json(text: &str) -> bool {
    json::is_valid(text)
}

/// One decoded `key=value` per line; pairs are decoded one at a time (never collected).
fn form(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut any = false;
    for (k, v) in crate::form::urlencoded_pairs(text.trim()) {
        if any {
            out.push('\n');
        }
        any = true;
        out.push_str(&k);
        out.push('=');
        out.push_str(&v);
    }
    any.then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_streams_render_as_json() {
        let sse = "event: tick\nid: 7\nretry: 500\ndata: {\"n\":12345678901234567890}\n\n: comment\ndata: plain \"text\"\ndata: line 2\n\n";
        assert_eq!(
            pretty(ContentKind::EventStream, sse).as_deref(),
            Some(
                "[\n  {\n    \"event\": \"tick\",\n    \"id\": \"7\",\n    \"retry\": 500,\n    \"data\": {\n      \"n\": 12345678901234567890\n    }\n  },\n  {\n    \"data\": \"plain \\\"text\\\"\\nline 2\"\n  }\n]"
            )
        );
        assert_eq!(
            pretty(ContentKind::EventStream, ": only a comment\n\n"),
            None
        );
        // empty / whitespace data stays a string
        assert_eq!(
            pretty(ContentKind::EventStream, "data:\n\n").as_deref(),
            Some("[\n  {\n    \"data\": \"\"\n  }\n]")
        );
    }

    #[test]
    fn dispatch() {
        assert_eq!(
            pretty(ContentKind::Json, "{\"a\":1}").as_deref(),
            Some("{\n  \"a\": 1\n}")
        );
        assert_eq!(
            pretty(ContentKind::FormUrlEncoded, "a=1&b=x+y%21").as_deref(),
            Some("a=1\nb=x y!")
        );
        assert_eq!(pretty(ContentKind::FormUrlEncoded, ""), None);
        assert_eq!(pretty(ContentKind::Yaml, "a: 1"), None);
        assert_eq!(pretty(ContentKind::Markdown, "# hi"), None);
        assert_eq!(pretty(ContentKind::Text, "x"), None);
        assert_eq!(pretty(ContentKind::Binary, "x"), None);
        assert_eq!(
            pretty(ContentKind::Image, "<svg><g/></svg>").as_deref(),
            Some("<svg>\n  <g/>\n</svg>")
        );
        assert_eq!(pretty(ContentKind::Image, "GIF89a"), None);
        assert!(pretty(ContentKind::JsonLines, "{\"a\":1}\n{\"b\":2}\n").is_some());
        assert!(pretty(ContentKind::GraphQl, "{a{b}}").is_some());
        assert_eq!(minify_json("{ \"a\" : 1 }").as_deref(), Some("{\"a\":1}"));
        assert_eq!(minify_json("{a:1}"), None);
        assert!(is_valid_json("[1,2]"));
    }
}
