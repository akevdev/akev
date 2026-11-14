//! Syntax validation for editor error markers (HTTP Toolkit's `content-validation.ts`).
//!
//! * **JSON** — strict RFC 8259 (via `serde_json`, with its precise messages); documents nested
//!   deeper than `serde_json`'s recursion limit fall back to this crate's iterative validator.
//! * **JSON records** (NDJSON / JSON Lines / RFC 7464 sequences / SignalR frames) — values are
//!   parsed one after another, skipping whitespace and RS (`0x1E`) separators; the first error
//!   of each record is reported and validation resumes on the next line (at most
//!   [`MAX_ISSUES`] issues).
//! * **XML** — well-formedness via `quick-xml` (matching end tags, comments, attribute syntax,
//!   duplicate attributes, entity reference syntax, a single root element, no text outside it,
//!   unclosed elements at the end). Valueless ("boolean") attributes are allowed, like HTTP
//!   Toolkit's validator configuration.
//!
//! Positions are 1-based lines and columns, columns counted in characters.

use akev_core::ContentKind;
use quick_xml::Reader;
use quick_xml::errors::IllFormedError;
use quick_xml::events::Event;
use quick_xml::events::attributes::AttrError;
use serde::Serialize;

/// Maximum number of issues reported for JSON records.
pub const MAX_ISSUES: usize = 100;

/// A syntax problem at a position in the text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyntaxIssue {
    /// 1-based line.
    pub line: usize,
    /// 1-based column, in characters.
    pub column: usize,
    /// What is wrong.
    pub message: String,
}

/// Validate text of the given kind: JSON (and GraphQL JSON envelopes), JSON records, XML and
/// SVG images. Other kinds have no validator and yield no issues.
pub fn validate(kind: ContentKind, text: &str) -> Vec<SyntaxIssue> {
    match kind {
        ContentKind::Json => json(text).into_iter().collect(),
        ContentKind::GraphQl if is_json_envelope(text) => json(text).into_iter().collect(),
        ContentKind::JsonLines => json_records(text),
        ContentKind::Xml => xml(text).into_iter().collect(),
        ContentKind::Image if crate::image::is_svg(text.as_bytes()) => {
            xml(text).into_iter().collect()
        }
        _ => Vec::new(),
    }
}

/// A GraphQL-over-HTTP JSON request (`{"query": ...}` or a batch array), as opposed to a bare
/// GraphQL document, which also starts with `{`.
fn is_json_envelope(text: &str) -> bool {
    let t = crate::util::skip_bom_ws(text.as_bytes());
    match t.first() {
        Some(b'[') => true,
        Some(b'{') => matches!(crate::util::skip_bom_ws(&t[1..]).first(), Some(b'"' | b'}')),
        _ => false,
    }
}

/// The first syntax error of a JSON document, if any. Empty (or whitespace-only) text is an
/// error ("EOF while parsing a value").
pub fn json(text: &str) -> Option<SyntaxIssue> {
    let body = text.strip_prefix('\u{feff}').unwrap_or(text);
    let bom = text.len() - body.len();
    match parse_one(body) {
        Ok(_) => None,
        Err(JsonError::RecursionLimit) => {
            if crate::format::is_valid_json(body) {
                None
            } else {
                Some(issue_at(text, bom, "invalid JSON (deeply nested)".into()))
            }
        }
        Err(JsonError::At { offset, message }) => Some(issue_at(text, bom + offset, message)),
    }
}

enum JsonError {
    RecursionLimit,
    /// Byte offset (relative to the parsed slice) and message.
    At {
        offset: usize,
        message: String,
    },
}

/// Parse exactly one JSON document filling `s` (surrounding whitespace allowed).
fn parse_one(s: &str) -> Result<(), JsonError> {
    use serde::Deserialize as _;
    let mut de = serde_json::Deserializer::from_str(s);
    serde::de::IgnoredAny::deserialize(&mut de)
        .and_then(|_| de.end())
        .map_err(|e| json_error(s, &e))
}

fn json_error(s: &str, e: &serde_json::Error) -> JsonError {
    let full = e.to_string();
    if full.starts_with("recursion limit exceeded") {
        return JsonError::RecursionLimit;
    }
    // serde_json appends " at line L column C"; keep just the description.
    let message = full
        .rfind(" at line ")
        .map_or(full.as_str(), |i| &full[..i])
        .to_string();
    JsonError::At {
        offset: offset_of(s, e.line(), e.column()),
        message,
    }
}

/// Byte offset of serde_json's (1-based line, byte column) position; column 0 (reported for
/// some EOF errors) maps to the line start.
fn offset_of(s: &str, line: usize, column: usize) -> usize {
    let line_start = if line <= 1 {
        0
    } else {
        memchr::memchr_iter(b'\n', s.as_bytes())
            .nth(line - 2)
            .map_or(s.len(), |i| i + 1)
    };
    (line_start + column.saturating_sub(1)).min(s.len())
}

/// Issue at byte offset `offset` of `text` (1-based line and character column).
fn issue_at(text: &str, offset: usize, message: String) -> SyntaxIssue {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    let before = &text[..offset];
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    SyntaxIssue {
        line: before.matches('\n').count() + 1,
        column: before[line_start..].chars().count() + 1,
        message,
    }
}

/// Validate a sequence of JSON values (one issue per broken record, at most [`MAX_ISSUES`]).
pub fn json_records(text: &str) -> Vec<SyntaxIssue> {
    let s = text.as_bytes();
    let mut issues = Vec::new();
    let mut pos = if text.starts_with('\u{feff}') { 3 } else { 0 };
    loop {
        while pos < s.len() && (s[pos].is_ascii_whitespace() || s[pos] == 0x1e) {
            pos += 1;
        }
        if pos >= s.len() || issues.len() >= MAX_ISSUES {
            break;
        }
        let rest = &text[pos..];
        let mut values =
            serde_json::Deserializer::from_str(rest).into_iter::<serde::de::IgnoredAny>();
        match values.next() {
            Some(Ok(_)) => {
                pos += values.byte_offset().max(1);
                continue;
            }
            None => break,
            Some(Err(e)) => {
                let err_at = match json_error(rest, &e) {
                    JsonError::At { offset, message } => {
                        issues.push(issue_at(text, pos + offset, message));
                        pos + offset
                    }
                    // Valid but deeper than serde_json's limit: not reported.
                    JsonError::RecursionLimit => pos,
                };
                // Resume at the next line (a record's first error is the only one reported).
                let err_at = err_at.min(s.len());
                pos = memchr::memchr(b'\n', &s[err_at..]).map_or(s.len(), |i| err_at + i + 1);
            }
        }
    }
    issues
}

/// The first well-formedness error of an XML document, if any. Blank text yields `None`.
pub fn xml(text: &str) -> Option<SyntaxIssue> {
    let body = text.strip_prefix('\u{feff}').unwrap_or(text);
    let bom = text.len() - body.len();
    if body.trim().is_empty() {
        return None;
    }
    let at = |offset: usize, message: String| Some(issue_at(text, bom + offset, message));
    let mut reader = Reader::from_str(body);
    {
        let cfg = reader.config_mut();
        cfg.trim_text(false);
        cfg.check_end_names = true;
        cfg.allow_unmatched_ends = false;
        cfg.check_comments = true;
        cfg.expand_empty_elements = false;
    }
    // Byte offsets of the currently open start tags.
    let mut open: Vec<usize> = Vec::new();
    let mut roots = 0usize;
    loop {
        let start = reader.buffer_position() as usize;
        let event = match reader.read_event() {
            Ok(e) => e,
            Err(quick_xml::Error::IllFormed(IllFormedError::UnclosedReference)) => {
                let message = "invalid entity or character reference".to_string();
                return at(reader.error_position() as usize, message);
            }
            Err(e) => return at(reader.error_position() as usize, e.to_string()),
        };
        match event {
            Event::Start(ref e) | Event::Empty(ref e) => {
                if open.is_empty() {
                    roots += 1;
                    if roots > 1 {
                        return at(start, "multiple root elements".into());
                    }
                }
                let mut attrs = e.attributes();
                attrs.with_checks(true);
                for a in attrs {
                    match a {
                        Ok(_) | Err(AttrError::ExpectedEq(_)) => {} // boolean attributes are fine
                        Err(err) => return at(start + 1 + attr_offset(&err), err.to_string()),
                    }
                }
                if matches!(event, Event::Start(_)) {
                    open.push(start);
                }
            }
            Event::End(_) => {
                open.pop();
            }
            // Character and entity references (`&amp;`) are events of their own; both are
            // checked on the source text.
            Event::Text(_) | Event::GeneralRef(_) => {
                let end = (reader.buffer_position() as usize).min(body.len());
                let raw = body.get(start..end).unwrap_or("");
                if open.is_empty() && !raw.trim().is_empty() {
                    let lead = raw.len() - raw.trim_start().len();
                    return at(start + lead, "text outside the root element".into());
                }
                if let Some(i) = bad_reference(raw) {
                    return at(start + i, "invalid entity or character reference".into());
                }
            }
            Event::CData(_) if open.is_empty() => {
                return at(start, "CDATA outside the root element".into());
            }
            Event::Eof => {
                if let Some(&p) = open.last() {
                    let name_end = body[p + 1..]
                        .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
                        .map_or(body.len(), |i| p + 1 + i);
                    return at(p, format!("unclosed element <{}>", &body[p + 1..name_end]));
                }
                if roots == 0 {
                    return at(body.len(), "no root element".into());
                }
                return None;
            }
            _ => {}
        }
    }
}

fn attr_offset(e: &AttrError) -> usize {
    match *e {
        AttrError::ExpectedEq(p)
        | AttrError::ExpectedValue(p)
        | AttrError::UnquotedValue(p)
        | AttrError::ExpectedQuote(p, _)
        | AttrError::Duplicated(p, _) => p,
    }
}

/// Offset of the first malformed `&` reference in text content (`&name;`, `&#123;`, `&#x1F;`).
fn bad_reference(t: &str) -> Option<usize> {
    let b = t.as_bytes();
    for i in memchr::memchr_iter(b'&', b) {
        let rest = &b[i + 1..];
        let body_len = if let Some(hex) = rest.strip_prefix(b"#x") {
            let n = hex.iter().take_while(|c| c.is_ascii_hexdigit()).count();
            (n > 0).then_some(2 + n)
        } else if let Some(dec) = rest.strip_prefix(b"#") {
            let n = dec.iter().take_while(|c| c.is_ascii_digit()).count();
            (n > 0).then_some(1 + n)
        } else {
            let name_start = rest
                .first()
                .is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, b'_' | b':'));
            let n = rest
                .iter()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.' | b':'))
                .count();
            name_start.then_some(n)
        };
        match body_len {
            Some(n) if rest.get(n) == Some(&b';') => {}
            _ => return Some(i),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(line: usize, column: usize, message: &str) -> SyntaxIssue {
        SyntaxIssue {
            line,
            column,
            message: message.into(),
        }
    }

    #[test]
    fn json_documents() {
        assert_eq!(json("{\"a\": [1, 2]}"), None);
        assert_eq!(json("\u{feff} [] "), None);
        let e = json("{\n  \"a\": 1,\n  \"b\" 2\n}").unwrap();
        assert_eq!((e.line, e.column), (3, 7));
        assert_eq!(e.message, "expected `:`");
        let e = json("[1, 2,]").unwrap();
        assert_eq!(
            (e.line, e.column, e.message.as_str()),
            (1, 7, "expected value")
        );
        // Columns count characters, not bytes.
        let e = json("{\"é😀\": x}").unwrap();
        assert_eq!((e.line, e.column), (1, 8));
        assert!(json("").is_some());
        assert!(json("{} {}").is_some());
        // Deeper than serde_json's recursion limit: still validated (iteratively).
        let deep = "[".repeat(10_000) + &"]".repeat(10_000);
        assert_eq!(json(&deep), None);
        let deep_bad = "[".repeat(10_000) + &"]".repeat(9_999);
        assert!(json(&deep_bad).is_some());
    }

    #[test]
    fn json_record_streams() {
        assert_eq!(json_records("{\"a\":1}\n{\"b\":2}\n"), vec![]);
        assert_eq!(json_records("\u{1e}{\"a\":1}\n\u{1e}[1]\n"), vec![]);
        assert_eq!(json_records("{\"a\":1}\u{1e}{\"b\":2}\u{1e}"), vec![]);
        // multi-line records are fine
        assert_eq!(json_records("{\n  \"a\": 1\n}\n{\n  \"b\": 2\n}"), vec![]);
        let issues = json_records("{\"a\":1}\n{\"b\":}\n{\"c\":3}\n[1,,2]\n");
        assert_eq!(
            issues,
            vec![issue(2, 6, "expected value"), issue(4, 4, "expected value")]
        );
        // One issue per record, capped overall.
        let many = "{x}\n".repeat(500);
        assert_eq!(json_records(&many).len(), MAX_ISSUES);
        assert_eq!(json_records(""), vec![]);
    }

    #[test]
    fn xml_documents() {
        assert_eq!(
            xml("<?xml version=\"1.0\"?>\n<a x=\"1\"><b/><c>t &amp; &#38; &#x26;</c></a>"),
            None
        );
        assert_eq!(xml("<a><input disabled/></a>"), None, "boolean attribute");
        assert_eq!(xml("<!-- c --><a/><!-- after -->\n"), None);
        assert_eq!(xml("  "), None);
        let e = xml("<a>\n  <b></c>\n</a>").unwrap();
        assert_eq!(e.line, 2);
        assert!(e.message.contains("expected `</b>`"), "{e:?}");
        let e = xml("<a><b>").unwrap();
        assert_eq!(e, issue(1, 4, "unclosed element <b>"));
        assert_eq!(
            xml("<a/><b/>").unwrap(),
            issue(1, 5, "multiple root elements")
        );
        assert_eq!(
            xml("<a/>\n trailing").unwrap(),
            issue(2, 2, "text outside the root element")
        );
        assert_eq!(
            xml("<a>x & y</a>").unwrap(),
            issue(1, 6, "invalid entity or character reference")
        );
        assert!(xml("<a x=\"1\" x=\"2\"/>").is_some(), "duplicate attribute");
        assert!(xml("<a x=1/>").is_some(), "unquoted attribute");
        assert!(xml("<a><!-- bad -- comment --></a>").is_some());
        assert!(xml("</a>").is_some());
        assert_eq!(
            xml("<?xml version=\"1.0\"?>").unwrap().message,
            "no root element"
        );
    }

    #[test]
    fn dispatch_by_kind() {
        assert_eq!(validate(ContentKind::Json, "[1]"), vec![]);
        assert_eq!(validate(ContentKind::Json, "[1").len(), 1);
        assert_eq!(validate(ContentKind::JsonLines, "{}\n{").len(), 1);
        assert_eq!(validate(ContentKind::Xml, "<a>").len(), 1);
        assert_eq!(validate(ContentKind::Image, "<svg><g></svg>").len(), 1);
        assert_eq!(validate(ContentKind::GraphQl, "{\"query\": }").len(), 1);
        assert_eq!(validate(ContentKind::GraphQl, "{ me { id } }"), vec![]);
        assert_eq!(validate(ContentKind::Html, "<p>"), vec![]);
        assert_eq!(validate(ContentKind::Text, "{"), vec![]);
    }
}
