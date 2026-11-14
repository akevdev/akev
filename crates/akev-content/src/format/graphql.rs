//! GraphQL document formatter (queries, mutations, fragments and SDL).
//!
//! Selection sets and type bodies get one field per line; arguments, variable definitions,
//! input objects and lists stay inline with normalized spacing (`(a: 1, b: $x)`); definitions
//! are separated by a blank line. Strings, block strings (descriptions) and comments are copied
//! verbatim. A JSON request envelope (`{"query": ...}`) passed here is pretty-printed as JSON.

use crate::util::push_indent;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum K {
    Name,
    Num,
    Str,
    BlockStr,
    Comment,
    Punct,
    Spread,
    Comma,
    Other,
}

#[derive(Clone, Copy, Debug)]
struct Tok {
    kind: K,
    start: usize,
    end: usize,
    /// A line break preceded the token in the source.
    nl: bool,
}

/// Streaming tokenizer (tokens are produced on demand, so memory stays O(1) in the input size).
struct Lexer<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Self {
        let pos = if src.starts_with('\u{feff}') { 3 } else { 0 };
        Lexer { src, pos }
    }
}

impl Iterator for Lexer<'_> {
    type Item = Tok;

    fn next(&mut self) -> Option<Tok> {
        let src = self.src;
        let s = src.as_bytes();
        let mut i = self.pos;
        let mut nl = false;
        while i < s.len() && matches!(s[i], b'\n' | b'\r' | b' ' | b'\t') {
            nl |= matches!(s[i], b'\n' | b'\r');
            i += 1;
        }
        if i >= s.len() {
            self.pos = i;
            return None;
        }
        let b = s[i];
        let start = i;
        let kind = match b {
            b'#' => {
                i = memchr::memchr2(b'\n', b'\r', &s[i..]).map_or(s.len(), |e| i + e);
                K::Comment
            }
            b'"' if s[i..].starts_with(b"\"\"\"") => {
                let mut j = i + 3;
                loop {
                    match memchr::memmem::find(&s[j..], b"\"\"\"") {
                        Some(e) if s[j + e - 1] == b'\\' => j += e + 3, // escaped \"""
                        Some(e) => {
                            j += e + 3;
                            break;
                        }
                        None => {
                            j = s.len();
                            break;
                        }
                    }
                }
                i = j;
                K::BlockStr
            }
            b'"' => {
                let mut j = i + 1;
                while j < s.len() {
                    match s[j] {
                        b'\\' => j += 2,
                        b'"' => {
                            j += 1;
                            break;
                        }
                        b'\n' | b'\r' => break,
                        _ => j += 1,
                    }
                }
                i = j.min(s.len());
                K::Str
            }
            b'.' if s[i..].starts_with(b"...") => {
                i += 3;
                K::Spread
            }
            b',' => {
                i += 1;
                K::Comma
            }
            b'{' | b'}' | b'(' | b')' | b'[' | b']' | b':' | b'=' | b'@' | b'$' | b'!' | b'|'
            | b'&' => {
                i += 1;
                K::Punct
            }
            b'_' | b'a'..=b'z' | b'A'..=b'Z' => {
                while i < s.len() && (s[i] == b'_' || s[i].is_ascii_alphanumeric()) {
                    i += 1;
                }
                K::Name
            }
            b'-' | b'0'..=b'9' => {
                i += 1;
                while i < s.len()
                    && (s[i].is_ascii_alphanumeric() || matches!(s[i], b'.' | b'+' | b'-'))
                {
                    i += 1;
                }
                K::Num
            }
            _ => {
                i += src[i..].chars().next().map_or(1, char::len_utf8);
                K::Other
            }
        };
        self.pos = i;
        Some(Tok {
            kind,
            start,
            end: i,
            nl,
        })
    }
}

const DEFINITION_KEYWORDS: &[&str] = &[
    "query",
    "mutation",
    "subscription",
    "fragment",
    "type",
    "interface",
    "union",
    "enum",
    "input",
    "scalar",
    "schema",
    "extend",
    "directive",
];

struct Out {
    buf: String,
    depth: usize,
    /// Nothing but indentation on the current line.
    line_empty: bool,
    /// A line break (with a blank line when `true`) is due before the next token.
    pending: Option<bool>,
    /// End of the last token text written; trimming never goes below it.
    floor: usize,
}

impl Out {
    fn newline(&mut self, blank: bool) {
        let trimmed = self.buf.trim_end_matches(' ').len().max(self.floor);
        self.buf.truncate(trimmed);
        if !self.buf.is_empty() {
            self.buf.push('\n');
            if blank {
                self.buf.push('\n');
            }
        }
        push_indent(&mut self.buf, self.depth);
        self.line_empty = true;
    }
    fn flush_pending(&mut self) {
        if let Some(blank) = self.pending.take() {
            self.newline(blank);
        }
    }
    fn space(&mut self) {
        if !self.line_empty && !self.buf.ends_with(' ') {
            self.buf.push(' ');
        }
    }
    fn push(&mut self, s: &str) {
        self.buf.push_str(s);
        self.line_empty = false;
        if !matches!(s, ": " | ", " | " ") {
            self.floor = self.buf.len();
        }
    }
}

/// Format a GraphQL document. `None` for empty input.
pub(crate) fn pretty(src: &str) -> Option<String> {
    let trimmed = crate::util::skip_bom_ws(src.as_bytes());
    match trimmed.first() {
        None => return None,
        Some(b'[') => return super::json::pretty(src),
        Some(b'{') if matches!(crate::util::skip_bom_ws(&trimmed[1..]).first(), Some(b'"')) => {
            return super::json::pretty(src);
        }
        _ => {}
    }
    let budget = crate::util::output_budget(src.len());
    let text = |t: &Tok| &src[t.start..t.end];
    let mut o = Out {
        buf: String::with_capacity(src.len() + src.len() / 2 + 16),
        depth: 0,
        line_empty: true,
        pending: None,
        floor: 0,
    };
    let mut inline = 0usize; // open ( [ and inline { (input objects)
    let mut braces: Vec<bool> = Vec::new(); // per open `{`: inline?
    let mut prev: Option<Tok> = None;
    let mut prev2: Option<Tok> = None;

    for tok in Lexer::new(src) {
        if o.buf.len() > budget {
            return None;
        }
        let tok = &tok;
        let t = text(tok);
        let ptext = prev.as_ref().map_or("", text);
        let pkind = prev.map(|p| p.kind);
        let completes = matches!(pkind, Some(K::Name | K::Num | K::Str | K::BlockStr))
            || matches!(ptext, ")" | "]" | "}" | "!");
        let after_binder = matches!(ptext, ":" | "=" | "@" | "$" | "|" | "&" | "extend")
            || pkind == Some(K::Spread);
        let after_on =
            pkind == Some(K::Name) && ptext == "on" && prev2.is_some_and(|q| q.kind == K::Spread);

        match (tok.kind, t) {
            (K::Comment, _) => {
                if tok.nl || o.pending.is_some() {
                    o.pending = None;
                    if !o.line_empty {
                        o.newline(false);
                    }
                } else {
                    o.space();
                }
                o.push(t);
                o.pending = Some(false);
            }
            (K::Punct, "{") => {
                if inline > 0 || matches!(ptext, ":" | "=") {
                    o.flush_pending();
                    if !matches!(ptext, "(" | "[" | "{") {
                        o.space();
                    }
                    o.push("{");
                    inline += 1;
                    braces.push(true);
                } else {
                    o.flush_pending();
                    o.space();
                    o.push("{");
                    o.depth += 1;
                    braces.push(false);
                    o.pending = Some(false);
                }
            }
            (K::Punct, "}") => {
                if braces.pop() == Some(true) {
                    inline = inline.saturating_sub(1);
                    o.push("}");
                } else {
                    o.depth = o.depth.saturating_sub(1);
                    o.pending = None;
                    o.newline(false);
                    o.push("}");
                    o.pending = Some(o.depth == 0);
                }
            }
            (K::Punct, "(" | "[") => {
                o.flush_pending();
                o.push(t);
                inline += 1;
            }
            (K::Punct, ")" | "]") => {
                inline = inline.saturating_sub(1);
                o.push(t);
            }
            (K::Punct, ":") => o.push(": "),
            (K::Punct, "=" | "|" | "&") => {
                o.flush_pending();
                o.space();
                o.push(t);
                o.push(" ");
            }
            (K::Punct, "!") => o.push("!"),
            (K::Punct, "@" | "$") => {
                o.flush_pending();
                if !matches!(ptext, "(" | "[" | "{") {
                    o.space();
                }
                o.push(t);
            }
            (K::Comma, _) => {
                if o.depth > 0 && inline == 0 {
                    o.push(",");
                    o.pending = Some(false);
                } else {
                    o.push(", ");
                }
            }
            _ => {
                let selection = o.depth > 0 && inline == 0;
                let starts_item = matches!(tok.kind, K::Name | K::Spread | K::Str | K::BlockStr);
                if o.pending.is_some() {
                    o.flush_pending();
                } else if !o.line_empty {
                    let top = o.depth == 0 && inline == 0;
                    if top && matches!(pkind, Some(K::Str | K::BlockStr)) && tok.kind == K::Name {
                        o.newline(false); // after a description
                    } else if top
                        && tok.kind == K::Name
                        && DEFINITION_KEYWORDS.contains(&t)
                        && completes
                        && !after_binder
                        && !after_on
                    {
                        o.newline(true);
                    } else if selection && starts_item && completes && !after_binder && !after_on {
                        o.newline(false);
                    } else {
                        let glue = matches!(ptext, "(" | "[" | "@" | "$")
                            || (ptext == "{" && inline > 0)
                            || (pkind == Some(K::Spread) && t != "on");
                        if !glue {
                            o.space();
                        }
                    }
                }
                o.push(t);
            }
        }
        prev2 = prev;
        prev = Some(*tok);
    }
    let trimmed = o.buf.trim_end().len().max(o.floor);
    o.buf.truncate(trimmed);
    (!o.buf.is_empty() && o.buf.len() <= budget).then_some(o.buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minified_query() {
        let q = "query GetUser($id: ID!, $n: Int = 10) @live { user(id: $id) { id name ...UserFields friends(first: $n, orderBy: {field: NAME, dir: ASC}) @include(if: true) { edges { node { id } } } ... on Admin { level } } }";
        let expected = "query GetUser($id: ID!, $n: Int = 10) @live {\n  user(id: $id) {\n    id\n    name\n    ...UserFields\n    friends(first: $n, orderBy: {field: NAME, dir: ASC}) @include(if: true) {\n      edges {\n        node {\n          id\n        }\n      }\n    }\n    ... on Admin {\n      level\n    }\n  }\n}";
        assert_eq!(pretty(q).unwrap(), expected);
        assert_eq!(pretty(expected).unwrap(), expected);
    }

    #[test]
    fn aliases_fragments_and_multiple_definitions() {
        let q = "{a: user(id:1){id} b: user(id:2){id}}\nfragment F on User{name, email}";
        assert_eq!(
            pretty(q).unwrap(),
            "{\n  a: user(id: 1) {\n    id\n  }\n  b: user(id: 2) {\n    id\n  }\n}\n\nfragment F on User {\n  name,\n  email\n}"
        );
    }

    #[test]
    fn sdl_and_comments() {
        let sdl = "\"\"\"A user\"\"\" type User implements Node & Entity { id: ID! # the id\n tags: [String!]! @deprecated(reason: \"x\") } scalar Date union U = A | B extend type User { age: Int }";
        assert_eq!(
            pretty(sdl).unwrap(),
            "\"\"\"A user\"\"\"\ntype User implements Node & Entity {\n  id: ID! # the id\n  tags: [String!]! @deprecated(reason: \"x\")\n}\n\nscalar Date\n\nunion U = A | B\n\nextend type User {\n  age: Int\n}"
        );
        // comment on its own line
        assert_eq!(
            pretty("{\n# leading\na}").unwrap(),
            "{\n  # leading\n  a\n}"
        );
        // escaped triple quote inside a block string
        assert_eq!(
            pretty("\"\"\"a \\\"\"\" b\"\"\" scalar X").unwrap(),
            "\"\"\"a \\\"\"\" b\"\"\"\nscalar X"
        );
    }

    #[test]
    fn lists_without_commas_and_mutations() {
        assert_eq!(
            pretty("mutation{add(ids:[1 2 3],input:{a:\"x\"}){ok}}").unwrap(),
            "mutation {\n  add(ids: [1 2 3], input: {a: \"x\"}) {\n    ok\n  }\n}"
        );
    }

    #[test]
    fn json_envelope_is_json() {
        let body = r#"{"query":"{ a }","variables":{"x":1}}"#;
        assert!(
            pretty(body)
                .unwrap()
                .starts_with("{\n  \"query\": \"{ a }\"")
        );
        assert!(
            pretty("[{\"query\":\"{a}\"}]")
                .unwrap()
                .starts_with("[\n  {")
        );
        assert_eq!(pretty("  "), None);
    }

    #[test]
    fn pathological_input_is_bounded() {
        // Deep selection sets: indentation is capped and the output budget refuses the ~100×
        // expansion instead of allocating hundreds of megabytes.
        assert_eq!(pretty(&"{a".repeat(1 << 20)), None);
        // Moderate depth still formats, and the streaming lexer handles many tiny tokens.
        assert!(pretty(&"{a".repeat(2_000)).is_some());
        assert!(pretty(&"a ".repeat(1 << 20)).is_some());
        // Unterminated strings / block strings / comments run to the end of input.
        for s in [
            "{a(b: \"open",
            "\"\"\"open",
            "# only a comment",
            "{a(b: \"x\\",
        ] {
            assert!(pretty(s).is_some(), "{s:?}");
        }
    }
}
