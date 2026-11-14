//! Token classification for syntax highlighting in the filter bar.

use crate::scan::{AtomKind, Scanner, Tok};
use serde::Serialize;

/// A highlighted region of a query.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HighlightToken {
    /// `field`, `operator`, `value`, `string`, `regex`, `flag`, `text`, `keyword` (OR/AND),
    /// `negation` (`-`, `!`, `NOT`) or `paren`.
    pub kind: &'static str,
    /// Byte offset where the token starts.
    pub start: usize,
    /// Byte offset where the token ends (exclusive).
    pub end: usize,
}

/// Classify every token of `query` for syntax highlighting. Never fails (half-typed input is
/// classified as well as possible); spans are ordered, non-overlapping byte ranges and
/// whitespace is not covered. Combine with [`crate::Filter::parse`] errors for squiggles.
pub fn highlight(query: &str) -> Vec<HighlightToken> {
    let mut out = Vec::new();
    let mut push = |kind: &'static str, start: usize, end: usize| {
        if end > start {
            out.push(HighlightToken { kind, start, end });
        }
    };
    for token in Scanner::tokenize(query) {
        let span = token.span;
        match token.tok {
            Tok::LParen | Tok::RParen => push("paren", span.start, span.end),
            Tok::Or | Tok::And | Tok::OrCall => push("keyword", span.start, span.end),
            Tok::Comma => push("operator", span.start, span.end),
            Tok::Not => push("negation", span.start, span.end),
            Tok::Term(term) => {
                let flag = term.field.as_ref().is_some_and(|f| {
                    let name = query.get(f.name.start..f.name.end).unwrap_or("");
                    name.eq_ignore_ascii_case("is") || name.eq_ignore_ascii_case("has")
                });
                if let Some(f) = &term.field {
                    push("field", f.name.start, f.op_span.start);
                    push("operator", f.op_span.start, f.op_span.end);
                }
                let mut prev_end = term.field.as_ref().map(|f| f.op_span.end);
                for alt in &term.alts {
                    // The comma separating alternatives.
                    if let Some(prev) = prev_end
                        && alt.span.start > prev
                    {
                        push("operator", prev, alt.span.start);
                    }
                    if let Some((_, cmp)) = alt.cmp {
                        push("operator", cmp.start, cmp.end);
                    }
                    let kind = match (&alt.atom.kind, term.field.is_some(), flag) {
                        (_, _, true) => "flag",
                        (AtomKind::Regex { .. }, _, _) => "regex",
                        (AtomKind::Quoted { .. } | AtomKind::Paren { .. }, _, _) => "string",
                        (AtomKind::Bare, true, _) => "value",
                        (AtomKind::Bare, false, _) => "text",
                    };
                    push(kind, alt.atom.span.start, alt.atom.span.end);
                    prev_end = Some(alt.span.end);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(q: &str) -> Vec<(&'static str, &str)> {
        highlight(q)
            .into_iter()
            .map(|t| (t.kind, &q[t.start..t.end]))
            .collect()
    }

    #[test]
    fn classifies_tokens() {
        assert_eq!(
            kinds(r#"-status:>=500,4xx (host:"a b" | is:error) path~/^x/i free"#),
            [
                ("negation", "-"),
                ("field", "status"),
                ("operator", ":"),
                ("operator", ">="),
                ("value", "500"),
                ("operator", ","),
                ("value", "4xx"),
                ("paren", "("),
                ("field", "host"),
                ("operator", ":"),
                ("string", "\"a b\""),
                ("keyword", "|"),
                ("field", "is"),
                ("operator", ":"),
                ("flag", "error"),
                ("paren", ")"),
                ("field", "path"),
                ("operator", "~"),
                ("regex", "/^x/i"),
                ("text", "free"),
            ]
        );
        assert_eq!(
            kinds("header.x-a:b NOT c"),
            [
                ("field", "header.x-a"),
                ("operator", ":"),
                ("value", "b"),
                ("negation", "NOT"),
                ("text", "c")
            ]
        );
        assert_eq!(kinds("header[x]"), [("field", "header[x]")]);
        assert_eq!(
            kinds("or(a, b) contains(x y)"),
            [
                ("keyword", "or"),
                ("paren", "("),
                ("text", "a"),
                ("operator", ","),
                ("text", "b"),
                ("paren", ")"),
                ("field", "contains"),
                ("string", "(x y)"),
            ]
        );
        assert!(highlight("").is_empty());
        assert_eq!(kinds("status:"), [("field", "status"), ("operator", ":")]);
    }

    #[test]
    fn spans_are_ordered_and_serialize() {
        let q = "a  OR (b:c,d) && \"e";
        let toks = highlight(q);
        let mut pos = 0;
        for t in &toks {
            assert!(t.start >= pos && t.end > t.start && t.end <= q.len());
            pos = t.end;
        }
        let json = serde_json::to_value(&toks[0]).unwrap();
        assert_eq!(json["kind"], "text");
        assert_eq!(json["start"], 0);
    }
}
