//! Token-based CSS beautifier.
//!
//! Output style (close to js-beautify's defaults): one declaration per line, `prop: value`,
//! `selector {` with each comma-separated selector of a rule on its own line, nested blocks
//! (`@media`, `@supports`, CSS nesting) indented, a blank line between top-level rules.
//! Strings, `url(...)` contents and comments are copied verbatim; runs of whitespace elsewhere
//! collapse to a single space (never removed between two tokens, so selectors like `a :hover`
//! keep their meaning). Linear time, no recursion.

use crate::util::push_indent;

/// One statement being accumulated (text between `{`, `}` and `;`).
#[derive(Default)]
struct Stmt {
    text: String,
    /// Byte offset of the first top-level `:` in `text`.
    colon: Option<usize>,
    /// Byte offsets of top-level commas in `text`.
    commas: Vec<usize>,
    pending_space: bool,
}

impl Stmt {
    fn push(&mut self, s: &str) {
        if self.pending_space && !self.text.is_empty() {
            self.text.push(' ');
        }
        self.pending_space = false;
        self.text.push_str(s);
    }
    fn is_blank(&self) -> bool {
        self.text.trim().is_empty()
    }
    fn clear(&mut self) {
        self.text.clear();
        self.colon = None;
        self.commas.clear();
        self.pending_space = false;
    }
}

pub(crate) fn pretty(text: &str, base: usize) -> Option<String> {
    let s = text.as_bytes();
    if text.trim().is_empty() {
        return None;
    }
    let mut out = String::with_capacity(text.len() + text.len() / 2 + 16);
    let mut depth = 0usize;
    let mut paren = 0usize;
    let mut stmt = Stmt::default();
    let mut blank_before_next = false;
    let mut i = if s.starts_with(b"\xEF\xBB\xBF") { 3 } else { 0 };
    let budget = crate::util::output_budget(text.len());

    let emit_line = |out: &mut String, depth: usize, line: &str, blank: &mut bool| {
        if !out.is_empty() {
            out.push('\n');
            if *blank {
                out.push('\n');
            }
        }
        *blank = false;
        push_indent(out, base + depth);
        out.push_str(line);
    };

    while i < s.len() {
        if out.len() > budget {
            return None;
        }
        let b = s[i];
        match b {
            b' ' | b'\t' | b'\n' | b'\r' | 0x0c => {
                stmt.pending_space = true;
                i += 1;
            }
            b'/' if s.get(i + 1) == Some(&b'*') => {
                let end =
                    memchr::memmem::find(&s[i + 2..], b"*/").map_or(s.len(), |e| i + 2 + e + 2);
                let comment = &text[i..end];
                if stmt.is_blank() {
                    stmt.clear();
                    emit_line(&mut out, depth, comment, &mut blank_before_next);
                } else {
                    stmt.push(comment);
                }
                i = end;
            }
            b'"' | b'\'' => {
                let end = scan_string(s, i);
                stmt.push(&text[i..end]);
                i = end;
            }
            b'(' => {
                // url( with an unquoted argument: copy verbatim up to the closing paren.
                let is_url = stmt.text.len() >= 3
                    && !stmt.pending_space
                    && stmt.text.as_bytes()[stmt.text.len() - 3..].eq_ignore_ascii_case(b"url");
                if is_url {
                    let mut j = i + 1;
                    while j < s.len() && s[j].is_ascii_whitespace() {
                        j += 1;
                    }
                    if j < s.len() && s[j] != b'"' && s[j] != b'\'' {
                        let end = memchr::memchr(b')', &s[i..]).map_or(s.len(), |e| i + e + 1);
                        stmt.push(&text[i..end]);
                        i = end;
                        continue;
                    }
                }
                paren += 1;
                stmt.push("(");
                i += 1;
            }
            b')' => {
                paren = paren.saturating_sub(1);
                stmt.pending_space = false;
                stmt.push(")");
                i += 1;
            }
            b'{' if paren == 0 => {
                let line = format_prelude(&stmt, base + depth);
                emit_line(
                    &mut out,
                    depth,
                    &format!("{line} {{"),
                    &mut blank_before_next,
                );
                stmt.clear();
                depth += 1;
                i += 1;
            }
            b'}' if paren == 0 => {
                if !stmt.is_blank() {
                    emit_line(
                        &mut out,
                        depth,
                        &format_declaration(&stmt),
                        &mut blank_before_next,
                    );
                }
                stmt.clear();
                depth = depth.saturating_sub(1);
                emit_line(&mut out, depth, "}", &mut blank_before_next);
                blank_before_next = depth == 0;
                i += 1;
            }
            b';' if paren == 0 => {
                if !stmt.is_blank() {
                    let decl = format_declaration(&stmt);
                    emit_line(&mut out, depth, &format!("{decl};"), &mut blank_before_next);
                } else if out.is_empty() {
                    emit_line(&mut out, depth, ";", &mut blank_before_next);
                } else {
                    out.push(';'); // keep stray empty statements (lossless)
                }
                stmt.clear();
                i += 1;
            }
            b':' if paren == 0 => {
                stmt.push(":");
                if stmt.colon.is_none() {
                    stmt.colon = Some(stmt.text.len() - 1);
                }
                i += 1;
            }
            b',' if paren == 0 => {
                stmt.pending_space = false;
                stmt.push(",");
                stmt.commas.push(stmt.text.len() - 1);
                i += 1;
            }
            _ => {
                // A run of ordinary characters (stop at anything handled above).
                let start = i;
                i += 1;
                while i < s.len()
                    && !matches!(
                        s[i],
                        b' ' | b'\t'
                            | b'\n'
                            | b'\r'
                            | 0x0c
                            | b'"'
                            | b'\''
                            | b'('
                            | b')'
                            | b'{'
                            | b'}'
                            | b';'
                            | b':'
                            | b','
                    )
                    && !(s[i] == b'/' && s.get(i + 1) == Some(&b'*'))
                {
                    i += 1;
                }
                stmt.push(&text[start..i]);
            }
        }
    }
    if !stmt.is_blank() {
        emit_line(&mut out, depth, stmt.text.trim(), &mut blank_before_next);
    }
    (out.len() <= budget).then_some(out)
}

fn scan_string(s: &[u8], start: usize) -> usize {
    let q = s[start];
    let mut i = start + 1;
    while i < s.len() {
        match s[i] {
            b'\\' => i += 2,
            b'\n' => return i,
            c if c == q => return i + 1,
            _ => i += 1,
        }
    }
    s.len()
}

/// `prop: value` normalization for a declaration (statement ended by `;` or `}`).
fn format_declaration(stmt: &Stmt) -> String {
    let t = &stmt.text;
    match stmt.colon {
        Some(c) if !t.trim_start().starts_with('@') => {
            let prop = t[..c].trim();
            let value = t[c + 1..].trim();
            if value.is_empty() {
                format!("{prop}:")
            } else {
                format!("{prop}: {value}")
            }
        }
        _ => t.trim().to_string(),
    }
}

/// Selector list (one per line) or at-rule prelude (kept on one line).
fn format_prelude(stmt: &Stmt, indent: usize) -> String {
    let t = &stmt.text;
    if t.trim_start().starts_with('@') || stmt.commas.is_empty() {
        return t.trim().to_string();
    }
    let mut out = String::with_capacity(t.len() + stmt.commas.len() * (indent * 2 + 2));
    let mut prev = 0;
    for &c in &stmt.commas {
        out.push_str(t[prev..c].trim());
        out.push_str(",\n");
        push_indent(&mut out, indent);
        prev = c + 1;
    }
    out.push_str(t[prev..].trim());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minified_rules() {
        let input = "body{margin:0;padding:0 auto}a:hover,a:focus{color:red!important;background:url(data:image/png;base64,AAA=)}";
        let expected = "body {\n  margin: 0;\n  padding: 0 auto\n}\n\na:hover,\na:focus {\n  color: red!important;\n  background: url(data:image/png;base64,AAA=)\n}";
        assert_eq!(pretty(input, 0).unwrap(), expected);
        assert_eq!(pretty(expected, 0).unwrap(), expected);
    }

    #[test]
    fn media_queries_and_nesting() {
        let input = "@media screen and (max-width:600px){.a{color:blue}.b .c{font:12px/1.5 \"Helvetica Neue\",sans-serif}}@import url(\"x.css\");";
        let expected = "@media screen and (max-width:600px) {\n  .a {\n    color: blue\n  }\n  .b .c {\n    font: 12px/1.5 \"Helvetica Neue\",sans-serif\n  }\n}\n\n@import url(\"x.css\");";
        assert_eq!(pretty(input, 0).unwrap(), expected);
        let nested = ".card{color:red;&:hover{color:blue}}";
        assert_eq!(
            pretty(nested, 0).unwrap(),
            ".card {\n  color: red;\n  &:hover {\n    color: blue\n  }\n}"
        );
    }

    #[test]
    fn comments_strings_and_whitespace() {
        let input = "/* header */\na  :hover   >  b{content:\"a;b{c}\"; /* inline */ color : red}";
        assert_eq!(
            pretty(input, 0).unwrap(),
            "/* header */\na :hover > b {\n  content: \"a;b{c}\";\n  /* inline */\n  color: red\n}"
        );
    }

    #[test]
    fn base_indent_and_edge_cases() {
        assert_eq!(pretty("a{b:c}", 2).unwrap(), "    a {\n      b: c\n    }");
        assert_eq!(pretty("  ", 0), None);
        // unbalanced braces don't panic
        assert_eq!(pretty("}}a{", 0).unwrap(), "}\n\n}\n\na {");
        // truncated string
        assert!(pretty("a{content:\"oops", 0).is_some());
        assert_eq!(
            pretty("@charset \"utf-8\";", 0).unwrap(),
            "@charset \"utf-8\";"
        );
        // multi-byte text right before `(` (regression: byte-slicing inside a character)
        assert_eq!(pretty("a{b:éé(x)}", 0).unwrap(), "a {\n  b: éé(x)\n}");
        // Output is bounded on pathological nesting (each `a{` would become a ~200-byte line).
        assert_eq!(pretty(&"a{".repeat(1 << 20), 0), None);
    }
}
