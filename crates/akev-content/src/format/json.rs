//! JSON formatting.
//!
//! [`pretty`] is a port of HTTP Toolkit's forgiving `formatJson`
//! (<https://github.com/httptoolkit/httptoolkit-ui>, by Tim Perry, AGPL-3.0-or-later like Akev):
//! a single pass over a JSONC-style token stream that re-indents with two spaces and copies every
//! token verbatim.
//! It is used for valid *and* invalid JSON alike: on valid JSON it produces exactly the canonical
//! 2-space layout (`"key": value`, empty containers inline), without ever re-serializing values —
//! so big integers, escapes, key order and duplicate keys are preserved byte-for-byte (unlike a
//! parse + stringify round trip). On broken JSON (comments, trailing or missing commas, unquoted
//! keys, single-quoted strings, truncated documents) it still produces a readable layout.
//!
//! Improvements over the original: whitespace between adjacent bare words is preserved instead
//! of being dropped (`hello world` no longer becomes `helloworld`), missing commas between
//! containers get a line break, single-quoted strings are tokens, and output size is bounded on
//! pathological nesting.

use crate::util::push_indent;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    OpenBrace,
    CloseBrace,
    OpenBracket,
    CloseBracket,
    Comma,
    Colon,
    Str,
    Num,
    Keyword,
    LineComment,
    BlockComment,
    RecordSep,
    Unknown,
}

#[derive(Clone, Copy, Debug)]
struct Tok {
    kind: Kind,
    start: usize,
    end: usize,
    /// Whitespace preceded the token.
    ws_before: bool,
    /// A line break preceded the token.
    nl_before: bool,
}

struct Scanner<'a> {
    s: &'a [u8],
    pos: usize,
}

impl<'a> Scanner<'a> {
    fn new(text: &'a str) -> Self {
        let s = text.as_bytes();
        let pos = if s.starts_with(b"\xEF\xBB\xBF") { 3 } else { 0 };
        Scanner { s, pos }
    }

    fn next(&mut self) -> Option<Tok> {
        let s = self.s;
        let ws_start = self.pos;
        let mut nl_before = false;
        while self.pos < s.len() {
            match s[self.pos] {
                b'\n' | b'\r' => nl_before = true,
                b' ' | b'\t' | 0x0b | 0x0c => {}
                _ => break,
            }
            self.pos += 1;
        }
        if self.pos >= s.len() {
            return None;
        }
        let ws_before = self.pos > ws_start;
        let start = self.pos;
        let b = s[start];
        let kind = match b {
            b'{' => Kind::OpenBrace,
            b'}' => Kind::CloseBrace,
            b'[' => Kind::OpenBracket,
            b']' => Kind::CloseBracket,
            b',' => Kind::Comma,
            b':' => Kind::Colon,
            0x1e => Kind::RecordSep,
            b'"' | b'\'' => {
                let mut i = start + 1;
                loop {
                    match memchr::memchr3(b, b'\\', b'\n', &s[i..]) {
                        None => {
                            i = s.len();
                            break;
                        }
                        Some(off) => {
                            i += off;
                            match s[i] {
                                b'\\' => i = (i + 2).min(s.len()),
                                b'\n' => {
                                    // unterminated string ends at the line break (exclude a CR)
                                    if i > start + 1 && s[i - 1] == b'\r' {
                                        i -= 1;
                                    }
                                    break;
                                }
                                _ => {
                                    i += 1;
                                    break;
                                }
                            }
                        }
                    }
                }
                self.pos = i;
                return Some(Tok {
                    kind: Kind::Str,
                    start,
                    end: i,
                    ws_before,
                    nl_before,
                });
            }
            b'/' if s.get(start + 1) == Some(&b'/') => {
                let end = memchr::memchr2(b'\n', b'\r', &s[start..]).map_or(s.len(), |e| start + e);
                self.pos = end;
                return Some(Tok {
                    kind: Kind::LineComment,
                    start,
                    end,
                    ws_before,
                    nl_before,
                });
            }
            b'/' if s.get(start + 1) == Some(&b'*') => {
                let end = memchr::memmem::find(&s[start + 2..], b"*/")
                    .map_or(s.len(), |e| start + 2 + e + 2);
                self.pos = end;
                return Some(Tok {
                    kind: Kind::BlockComment,
                    start,
                    end,
                    ws_before,
                    nl_before,
                });
            }
            _ => {
                let mut i = start + 1;
                if b != b'/' {
                    while i < s.len() && !is_delimiter(s[i]) {
                        i += 1;
                    }
                }
                self.pos = i;
                let word = &s[start..i];
                let kind = match word {
                    b"true" | b"false" | b"null" => Kind::Keyword,
                    _ if is_number(word) => Kind::Num,
                    _ => Kind::Unknown,
                };
                return Some(Tok {
                    kind,
                    start,
                    end: i,
                    ws_before,
                    nl_before,
                });
            }
        };
        self.pos = start + 1;
        Some(Tok {
            kind,
            start,
            end: start + 1,
            ws_before,
            nl_before,
        })
    }
}

fn is_delimiter(b: u8) -> bool {
    matches!(
        b,
        b'{' | b'}'
            | b'['
            | b']'
            | b','
            | b':'
            | b'"'
            | b'\''
            | b'/'
            | b' '
            | b'\t'
            | b'\n'
            | b'\r'
            | 0x0b
            | 0x0c
            | 0x1e
    )
}

/// JSON number grammar: `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?`.
fn is_number(w: &[u8]) -> bool {
    let mut i = 0;
    if w.first() == Some(&b'-') {
        i += 1;
    }
    match w.get(i) {
        Some(b'0') => i += 1,
        Some(b'1'..=b'9') => {
            while i < w.len() && w[i].is_ascii_digit() {
                i += 1;
            }
        }
        _ => return false,
    }
    if w.get(i) == Some(&b'.') {
        i += 1;
        let d = i;
        while i < w.len() && w[i].is_ascii_digit() {
            i += 1;
        }
        if i == d {
            return false;
        }
    }
    if matches!(w.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(w.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        let d = i;
        while i < w.len() && w[i].is_ascii_digit() {
            i += 1;
        }
        if i == d {
            return false;
        }
    }
    i == w.len()
}

fn is_value(k: Option<Kind>) -> bool {
    matches!(
        k,
        Some(Kind::Str | Kind::Num | Kind::Keyword | Kind::CloseBrace | Kind::CloseBracket)
    )
}

fn is_wordlike(k: Option<Kind>) -> bool {
    matches!(
        k,
        Some(Kind::Str | Kind::Num | Kind::Keyword | Kind::Unknown)
    )
}

/// Pretty-print JSON (or JSON-ish text). Returns `None` for empty input or when the first token
/// can't start a JSON document (e.g. an HTML error page).
pub(crate) fn pretty(text: &str) -> Option<String> {
    if !starts_like_json(text) {
        return None;
    }
    format_tolerant(text, false)
}

/// Pretty-print JSON records (JSON Lines / NDJSON / JSON text sequences / SignalR frames): every
/// top-level value is formatted and separated by a newline; RS (0x1E) separators are dropped.
pub(crate) fn pretty_records(text: &str) -> Option<String> {
    if !starts_like_json(text) {
        return None;
    }
    format_tolerant(text, true)
}

fn starts_like_json(text: &str) -> bool {
    let mut sc = Scanner::new(text);
    matches!(
        sc.next().map(|t| t.kind),
        Some(
            Kind::OpenBrace
                | Kind::OpenBracket
                | Kind::Str
                | Kind::Num
                | Kind::Keyword
                | Kind::LineComment
                | Kind::BlockComment
                | Kind::RecordSep
        )
    )
}

/// The forgiving formatter. `None` only if the output would exceed the size budget
/// ([`crate::util::output_budget`] — only reachable with pathological nesting, since
/// indentation is capped).
pub(crate) fn format_tolerant(text: &str, records: bool) -> Option<String> {
    let budget = crate::util::output_budget(text.len());
    let mut sc = Scanner::new(text);
    let mut out = String::with_capacity(text.len() + text.len() / 2 + 16);
    let mut indent: usize = 0;
    let mut needs_indent = false;
    let mut prev: Option<Kind> = None;
    let mut peeked: Option<Option<Tok>> = None;
    let mut lines = 0usize;
    let mut floor = 0usize;

    macro_rules! peek {
        () => {{
            if peeked.is_none() {
                peeked = Some(sc.next());
            }
            peeked.and_then(|t| t)
        }};
    }

    loop {
        let tok = match peeked.take() {
            Some(t) => t,
            None => sc.next(),
        };
        let Some(tok) = tok else { break };
        let between_records = records && indent == 0;
        if between_records && tok.kind == Kind::RecordSep {
            continue;
        }
        if between_records && !out.is_empty() && !out.ends_with('\n') && !is_value(Some(tok.kind)) {
            out.push('\n');
            needs_indent = indent > 0;
        }
        if needs_indent {
            push_indent(&mut out, indent);
            needs_indent = false;
        }
        let text_of = &text[tok.start..tok.end];
        match tok.kind {
            Kind::OpenBrace | Kind::OpenBracket => {
                // Missing comma between consecutive values: break the line first.
                if is_value(prev) && !out.ends_with('\n') && !out.is_empty() {
                    out.push('\n');
                    push_indent(&mut out, indent);
                }
                out.push_str(text_of);
                floor = out.len();
                indent += 1;
                let next = peek!();
                let closing = matches!(
                    next.map(|t| t.kind),
                    Some(Kind::CloseBrace | Kind::CloseBracket)
                );
                let inline_comment = matches!(next, Some(t) if matches!(t.kind, Kind::LineComment | Kind::BlockComment) && !t.nl_before);
                if !closing && next.is_some() && !inline_comment {
                    out.push('\n');
                    needs_indent = true;
                }
            }
            Kind::CloseBrace | Kind::CloseBracket => {
                let was_empty = matches!(prev, Some(Kind::OpenBrace | Kind::OpenBracket));
                let underflow = indent == 0;
                indent = indent.saturating_sub(1);
                if !was_empty {
                    trim_trailing_spaces(&mut out, floor);
                    if !out.ends_with('\n') && !out.is_empty() {
                        out.push('\n');
                    }
                    push_indent(&mut out, indent);
                }
                out.push_str(text_of);
                floor = out.len();
                if underflow {
                    out.push('\n');
                }
            }
            Kind::Comma => {
                out.push_str(text_of);
                floor = out.len();
                let next = peek!();
                let suppress = match next {
                    None => true,
                    Some(t) => match t.kind {
                        Kind::LineComment | Kind::BlockComment => !t.nl_before,
                        Kind::CloseBrace | Kind::CloseBracket | Kind::Comma => true,
                        _ => false,
                    },
                };
                if !suppress {
                    out.push('\n');
                    needs_indent = true;
                }
            }
            Kind::Colon => {
                out.push_str(": ");
            }
            Kind::LineComment => {
                let after_opener = matches!(prev, Some(Kind::OpenBrace | Kind::OpenBracket));
                if (after_opener || tok.nl_before) && !out.ends_with('\n') && !out.is_empty() {
                    trim_trailing_spaces(&mut out, floor);
                    out.push('\n');
                    push_indent(&mut out, indent);
                } else if !out.is_empty() && !out.ends_with('\n') {
                    trim_trailing_spaces(&mut out, floor);
                    out.push(' ');
                } else if out.ends_with('\n') {
                    push_indent(&mut out, indent);
                }
                out.push_str(text_of);
                floor = out.len();
                let next = peek!();
                if !matches!(
                    next.map(|t| t.kind),
                    None | Some(Kind::CloseBrace | Kind::CloseBracket)
                ) {
                    out.push('\n');
                    needs_indent = true;
                }
            }
            Kind::BlockComment => {
                let at_line_start = out.is_empty()
                    || out.ends_with('\n')
                    || out.ends_with(' ') && out.trim_end_matches(' ').ends_with('\n');
                if !at_line_start {
                    if tok.nl_before {
                        trim_trailing_spaces(&mut out, floor);
                        out.push('\n');
                        push_indent(&mut out, indent);
                    } else {
                        out.push(' ');
                    }
                }
                out.push_str(text_of);
                floor = out.len();
                let next = peek!();
                if !matches!(
                    next.map(|t| t.kind),
                    None | Some(Kind::Comma | Kind::CloseBrace | Kind::CloseBracket)
                ) {
                    out.push('\n');
                    needs_indent = true;
                }
            }
            _ => {
                let kind = Some(tok.kind);
                if is_value(prev) && is_value(kind) && !out.ends_with('\n') && !out.is_empty() {
                    // Missing comma between sequential values: break the line for readability.
                    out.push('\n');
                    push_indent(&mut out, indent);
                } else if is_wordlike(prev)
                    && is_wordlike(kind)
                    && tok.ws_before
                    && !out.ends_with('\n')
                {
                    out.push(' ');
                }
                out.push_str(text_of);
                floor = out.len();
            }
        }
        prev = Some(tok.kind);
        lines += 1;
        if lines & 0xfff == 0 && out.len() > budget {
            return None;
        }
    }
    if out.len() > budget {
        return None;
    }
    trim_trailing_spaces(&mut out, floor);
    while out.len() > floor && out.ends_with('\n') {
        out.pop();
    }
    Some(out)
}

/// Trim padding we emitted, never below `floor` (the end of the last token written, whose text
/// may legitimately end in spaces, e.g. an unterminated string).
fn trim_trailing_spaces(out: &mut String, floor: usize) {
    let trimmed = out.trim_end_matches([' ', '\t']).len().max(floor);
    out.truncate(trimmed);
}

// ---------------------------------------------------------------------------------------------
// Strict validation / minification

/// Strict RFC 8259 validation with an explicit stack (no recursion limit). When `out` is given,
/// the minified document is written to it. A leading UTF-8 BOM is tolerated.
pub(crate) fn validate(text: &str, mut out: Option<&mut String>) -> bool {
    #[derive(PartialEq, Clone, Copy)]
    enum State {
        Value,
        ValueOrClose,
        KeyOrClose,
        Key,
        Colon,
        CommaOrClose,
        Done,
    }
    let s = text.strip_prefix('\u{feff}').unwrap_or(text).as_bytes();
    let mut stack: Vec<bool> = Vec::new(); // true = object
    let mut state = State::Value;
    let mut i = 0;
    let mut emit = |bytes: &[u8]| {
        if let Some(o) = out.as_deref_mut() {
            // Tokens are delimited by ASCII bytes, so slices are valid UTF-8.
            o.push_str(std::str::from_utf8(bytes).unwrap_or(""));
        }
    };
    loop {
        while i < s.len() && matches!(s[i], b' ' | b'\t' | b'\n' | b'\r') {
            i += 1;
        }
        if i >= s.len() {
            return state == State::Done;
        }
        let b = s[i];
        let after_value = |stack: &Vec<bool>| {
            if stack.is_empty() {
                State::Done
            } else {
                State::CommaOrClose
            }
        };
        match (state, b) {
            (State::Done, _) => return false,
            (State::Value | State::ValueOrClose, b'{') => {
                stack.push(true);
                state = State::KeyOrClose;
                emit(b"{");
                i += 1;
            }
            (State::Value | State::ValueOrClose, b'[') => {
                stack.push(false);
                state = State::ValueOrClose;
                emit(b"[");
                i += 1;
            }
            (State::ValueOrClose, b']') | (State::CommaOrClose, b']')
                if stack.last() == Some(&false) =>
            {
                stack.pop();
                emit(b"]");
                i += 1;
                state = after_value(&stack);
            }
            (State::KeyOrClose, b'}') | (State::CommaOrClose, b'}')
                if stack.last() == Some(&true) =>
            {
                stack.pop();
                emit(b"}");
                i += 1;
                state = after_value(&stack);
            }
            (State::CommaOrClose, b',') => {
                emit(b",");
                i += 1;
                state = if stack.last() == Some(&true) {
                    State::Key
                } else {
                    State::Value
                };
            }
            (State::Colon, b':') => {
                emit(b":");
                i += 1;
                state = State::Value;
            }
            (State::Key | State::KeyOrClose, b'"') | (State::Value | State::ValueOrClose, b'"') => {
                let Some(end) = scan_strict_string(s, i) else {
                    return false;
                };
                emit(&s[i..end]);
                i = end;
                state = if matches!(state, State::Key | State::KeyOrClose) {
                    State::Colon
                } else {
                    after_value(&stack)
                };
            }
            (State::Value | State::ValueOrClose, _) => {
                let start = i;
                while i < s.len()
                    && !matches!(
                        s[i],
                        b',' | b']'
                            | b'}'
                            | b':'
                            | b' '
                            | b'\t'
                            | b'\n'
                            | b'\r'
                            | b'{'
                            | b'['
                            | b'"'
                    )
                {
                    i += 1;
                }
                let word = &s[start..i];
                if !(matches!(word, b"true" | b"false" | b"null") || is_number(word)) {
                    return false;
                }
                emit(word);
                state = after_value(&stack);
            }
            _ => return false,
        }
    }
}

/// Scan a strict JSON string starting at the opening quote; returns the index after the closing
/// quote. Rejects raw control characters and invalid escapes.
fn scan_strict_string(s: &[u8], start: usize) -> Option<usize> {
    let mut i = start + 1;
    loop {
        let off = s[i..]
            .iter()
            .position(|&c| c == b'"' || c == b'\\' || c < 0x20)?;
        i += off;
        match s[i] {
            b'"' => return Some(i + 1),
            b'\\' => match *s.get(i + 1)? {
                b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => i += 2,
                b'u' => {
                    let hex = s.get(i + 2..i + 6)?;
                    if !hex.iter().all(u8::is_ascii_hexdigit) {
                        return None;
                    }
                    i += 6;
                }
                _ => return None,
            },
            _ => return None, // control character
        }
    }
}

/// Minify strictly valid JSON; `None` if invalid.
pub(crate) fn minify(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    validate(text, Some(&mut out)).then_some(out)
}

/// Whether the text is strictly valid JSON.
pub(crate) fn is_valid(text: &str) -> bool {
    validate(text, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_json_matches_canonical_layout() {
        let input =
            r#"{"a":1,"b":[true,false,null],"c":{},"d":[],"e":{"f":"g\"h","i":[{"j":-1.5}]}}"#;
        let expected = "{\n  \"a\": 1,\n  \"b\": [\n    true,\n    false,\n    null\n  ],\n  \"c\": {},\n  \"d\": [],\n  \"e\": {\n    \"f\": \"g\\\"h\",\n    \"i\": [\n      {\n        \"j\": -1.5\n      }\n    ]\n  }\n}";
        assert_eq!(pretty(input).unwrap(), expected);
        // Same as serde_json's pretty printer on valid input.
        let v: serde_json::Value = serde_json::from_str(input).unwrap();
        assert_eq!(serde_json::to_string_pretty(&v).unwrap(), expected);
        // Idempotent.
        assert_eq!(pretty(expected).unwrap(), expected);
    }

    #[test]
    fn preserves_big_numbers_and_escapes() {
        let input = r#"{"id":123456789012345678901234567890,"u":"\u00e9\/","dup":1,"dup":2}"#;
        let out = pretty(input).unwrap();
        assert!(out.contains("123456789012345678901234567890"));
        assert!(out.contains(r#""\u00e9\/""#));
        assert_eq!(out.matches("\"dup\"").count(), 2);
    }

    #[test]
    fn tolerant_of_broken_json() {
        // comments, trailing comma, unquoted key, single quotes
        let input = "{// lead\n a: 'x y', \"b\": [1,2,], /* blk */ \"c\": 3,}";
        let out = pretty(input).unwrap();
        assert_eq!(
            out,
            "{\n  // lead\n  a: 'x y',\n  \"b\": [\n    1,\n    2,\n  ], /* blk */\n  \"c\": 3,\n}"
        );
        // missing commas
        assert_eq!(
            pretty("[1 2 {\"a\":1}{\"b\":2}]").unwrap(),
            "[\n  1\n  2\n  {\n    \"a\": 1\n  }\n  {\n    \"b\": 2\n  }\n]"
        );
        // truncated document
        assert_eq!(
            pretty("{\"a\": [1, 2").unwrap(),
            "{\n  \"a\": [\n    1,\n    2"
        );
        // extra closers (underflow) go on their own line
        assert_eq!(pretty("{}}").unwrap(), "{}\n}");
        // unterminated string stops at the line end
        assert_eq!(
            pretty("{\"a\": \"oops\n}").unwrap(),
            "{\n  \"a\": \"oops\n}"
        );
        // the tail of an unterminated string is token text, never trimmed
        assert_eq!(
            pretty("{\"a\": \"oops   ").unwrap(),
            "{\n  \"a\": \"oops   "
        );
        assert_eq!(pretty("[\"x  \n]").unwrap(), "[\n  \"x  \n]");
        // bare words keep their spacing (regression: HTK dropped whitespace between them)
        assert_eq!(
            pretty("{\"a\": hello world}").unwrap(),
            "{\n  \"a\": hello world\n}"
        );
        // trailing line comment stays on its line
        assert_eq!(
            pretty("{\"a\": 1, // one\n\"b\": 2}").unwrap(),
            "{\n  \"a\": 1, // one\n  \"b\": 2\n}"
        );
    }

    #[test]
    fn rejects_non_json_starts() {
        assert_eq!(pretty("<html><body>error</body></html>"), None);
        assert_eq!(pretty("   "), None);
        assert_eq!(pretty(""), None);
        assert!(pretty("\u{feff}{\"a\":1}").is_some());
        assert_eq!(pretty("42").as_deref(), Some("42"));
    }

    #[test]
    fn records() {
        let input = "{\"a\":1}\n{\"b\":[2]}\n";
        assert_eq!(
            pretty_records(input).unwrap(),
            "{\n  \"a\": 1\n}\n{\n  \"b\": [\n    2\n  ]\n}"
        );
        let seq = "\u{1e}{\"a\":1}\n\u{1e}[1]\n";
        assert_eq!(pretty_records(seq).unwrap(), "{\n  \"a\": 1\n}\n[\n  1\n]");
        let signalr = "{\"protocol\":\"json\",\"version\":1}\u{1e}{\"type\":6}\u{1e}";
        assert_eq!(
            pretty_records(signalr).unwrap(),
            "{\n  \"protocol\": \"json\",\n  \"version\": 1\n}\n{\n  \"type\": 6\n}"
        );
    }

    #[test]
    fn deep_nesting_is_bounded() {
        // Moderate depth formats, with indentation capped at MAX_INDENT_LEVELS.
        let deep = "[".repeat(1000) + &"]".repeat(1000);
        let out = pretty(&deep).unwrap();
        let widest = out.lines().map(|l| l.len()).max().unwrap();
        // `[` / `]` at the cap, or the innermost `[]`
        assert_eq!(widest, crate::util::MAX_INDENT_LEVELS * 2 + 2);
        // Pathological depth: no stack overflow, no runaway output.
        let deeper = "[".repeat(200_000) + &"]".repeat(200_000);
        assert!(
            pretty(&deeper).is_none_or(|o| o.len() <= crate::util::output_budget(deeper.len()))
        );
        // A 1 MB pile of unclosed brackets would expand ~100×: refused instead of allocating.
        assert_eq!(pretty(&"[".repeat(1 << 20)), None);
        let deep_obj = "{\"a\":".repeat(50_000) + "1" + &"}".repeat(50_000);
        let _ = pretty(&deep_obj);
    }

    #[test]
    fn strict_validation_and_minify() {
        assert_eq!(
            minify(" { \"a\" : [ 1 , 2.5e-3 , \"x y\" ] , \"b\" : { } } ").as_deref(),
            Some(r#"{"a":[1,2.5e-3,"x y"],"b":{}}"#)
        );
        assert_eq!(minify("\u{feff}[]").as_deref(), Some("[]"));
        assert_eq!(minify("\"\\u00e9\"").as_deref(), Some("\"\\u00e9\""));
        for bad in [
            "{,}",
            "[1,]",
            "{\"a\"}",
            "{\"a\":}",
            "[01]",
            "[1.]",
            "[.5]",
            "{'a':1}",
            "[1] x",
            "",
            "nul",
            "\"a\nb\"",
            "\"\\x\"",
            "[",
            "{\"a\":1,}",
            "[1 2]",
            "{\"a\" 1}",
        ] {
            assert!(minify(bad).is_none(), "{bad:?} must be invalid");
        }
        let deep = "[".repeat(100_000) + &"]".repeat(100_000);
        assert!(is_valid(&deep));
        assert!(is_valid("-0.0E+1"));
    }
}
