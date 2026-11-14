//! Shell-style glob → regex translation.
//!
//! Supported syntax (identical in every mode unless noted):
//!
//! | Glob | Meaning |
//! |---|---|
//! | `*` | any run of characters — in [`GlobMode::Path`] it stops at `/` (one segment) |
//! | `**` | any run of characters including `/`. In path mode `/**/` also matches a single `/` and a trailing `/**` also matches nothing (`/api/**` matches `/api`, `/api/` and `/api/a/b`) |
//! | `?` | exactly one character (not `/` in path mode) |
//! | `[abc]`, `[a-z]`, `[!a-z]` / `[^a-z]` | character classes (an unclosed `[` is literal) |
//! | `{a,b,c}` | alternatives, may nest and contain other glob syntax (an unclosed `{` is literal) |
//! | `\x` | the literal character `x` |
//!
//! Globs are always anchored (they must match the whole input). Alternatives may nest at most
//! [`MAX_BRACE_DEPTH`] levels deep. Translation takes time linear in the glob's length.

use std::collections::HashMap;

/// How `*` and `?` treat the `/` separator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GlobMode {
    /// URL paths: `*`/`?` never cross `/`, `**` does.
    Path,
    /// Hosts, URLs and free text: `*` matches anything (including `.` and `/`).
    Any,
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Literal(char),
    Star,
    DoubleStar,
    Question,
    /// Already-translated regex character class (including the brackets).
    Class(String),
    Alt(Vec<Vec<Token>>),
}

/// Deepest nesting of `{...}` alternatives accepted (deeper globs are rejected).
pub(crate) const MAX_BRACE_DEPTH: usize = 32;

/// Translate a glob into an anchored regex source string (without flags).
///
/// Runs in time linear in the glob's length. Fails only when `{...}` alternatives nest more
/// than [`MAX_BRACE_DEPTH`] levels deep.
pub(crate) fn glob_to_regex(glob: &str, mode: GlobMode) -> Result<String, String> {
    let chars: Vec<char> = glob.chars().collect();
    let structure = Structure::scan(&chars);
    let mut parser = Parser {
        chars: &chars,
        structure: &structure,
        pos: 0,
    };
    let tokens = parser.parse_seq(false, 0)?;
    let mut out = String::with_capacity(glob.len() * 2 + 2);
    out.push('^');
    emit(&tokens, mode, &mut out);
    out.push('$');
    Ok(out)
}

/// Where the classes and alternatives of a glob are, found in one left-to-right pass.
///
/// An unclosed `{` is a literal, so whether a `{` opens alternatives depends on everything
/// after it. Matching the braces up front (with a stack, skipping `\x` escapes and `[...]`
/// classes exactly as the parser does) lets the parser treat an unclosed `{` as a literal right
/// away. Re-parsing the rest of the glob for every unclosed `{` would take exponential time.
struct Structure {
    /// For every `{` that opens alternatives: the position of its closing `}`.
    closing: Vec<Option<usize>>,
    /// For every `[` that starts a class: the translated class and the position after `]`.
    classes: HashMap<usize, (String, usize)>,
}

impl Structure {
    fn scan(chars: &[char]) -> Structure {
        let mut closing = vec![None; chars.len()];
        let mut classes = HashMap::new();
        let mut open: Vec<usize> = Vec::new();
        // Once a `[` finds no closing `]`, no later `[` can find one either (see
        // `parse_class`), so the rest of the glob isn't scanned again for each `[`.
        let mut classes_possible = true;
        let mut pos = 0;
        while pos < chars.len() {
            match chars[pos] {
                '\\' => pos += 2,
                '[' => match classes_possible.then(|| parse_class(chars, pos)).flatten() {
                    Some((class, end)) => {
                        classes.insert(pos, (class, end));
                        pos = end;
                    }
                    None => {
                        classes_possible = false;
                        pos += 1;
                    }
                },
                '{' => {
                    open.push(pos);
                    pos += 1;
                }
                '}' => {
                    if let Some(start) = open.pop() {
                        closing[start] = Some(pos);
                    }
                    pos += 1;
                }
                _ => pos += 1,
            }
        }
        Structure { closing, classes }
    }
}

struct Parser<'a> {
    chars: &'a [char],
    structure: &'a Structure,
    pos: usize,
}

impl Parser<'_> {
    /// Parse a token sequence. Inside alternatives (`in_alt`), stops at an unnested `,` or `}`.
    /// `depth` is the number of enclosing alternatives.
    fn parse_seq(&mut self, in_alt: bool, depth: usize) -> Result<Vec<Token>, String> {
        let chars = self.chars;
        let mut out = Vec::new();
        while self.pos < chars.len() {
            let c = chars[self.pos];
            match c {
                ',' | '}' if in_alt => return Ok(out),
                '\\' => {
                    self.pos += 1;
                    match chars.get(self.pos) {
                        Some(&next) => {
                            out.push(Token::Literal(next));
                            self.pos += 1;
                        }
                        None => out.push(Token::Literal('\\')),
                    }
                }
                '*' => {
                    let start = self.pos;
                    while self.pos < chars.len() && chars[self.pos] == '*' {
                        self.pos += 1;
                    }
                    out.push(if self.pos - start >= 2 {
                        Token::DoubleStar
                    } else {
                        Token::Star
                    });
                }
                '?' => {
                    out.push(Token::Question);
                    self.pos += 1;
                }
                '[' => match self.structure.classes.get(&self.pos) {
                    Some((class, end)) => {
                        out.push(Token::Class(class.clone()));
                        self.pos = *end;
                    }
                    None => {
                        out.push(Token::Literal('['));
                        self.pos += 1;
                    }
                },
                '{' => match self.structure.closing[self.pos] {
                    Some(_) => {
                        let alts = self.parse_alt(depth + 1)?;
                        out.push(Token::Alt(alts));
                    }
                    None => {
                        out.push(Token::Literal('{'));
                        self.pos += 1;
                    }
                },
                other => {
                    out.push(Token::Literal(other));
                    self.pos += 1;
                }
            }
        }
        Ok(out)
    }

    /// Parse `{a,b}` at the current position (a `{` with a matching `}`), leaving the position
    /// after the closing `}`.
    fn parse_alt(&mut self, depth: usize) -> Result<Vec<Vec<Token>>, String> {
        if depth > MAX_BRACE_DEPTH {
            return Err(format!(
                "`{{...}}` alternatives are nested more than {MAX_BRACE_DEPTH} levels deep"
            ));
        }
        self.pos += 1;
        let mut alts = Vec::new();
        loop {
            alts.push(self.parse_seq(true, depth)?);
            // Every brace inside a matched pair is matched too, so the sequence stops at this
            // pair's own `,` or `}`.
            match self.chars.get(self.pos) {
                Some(',') => self.pos += 1,
                Some('}') => {
                    self.pos += 1;
                    return Ok(alts);
                }
                _ => return Err("unbalanced `{...}` alternatives".to_string()),
            }
        }
    }
}

/// Parse `[...]` starting at `start`. Returns the regex class and the position after `]`, or
/// `None` when the input ends before a closing `]`.
///
/// When the class starting at `p` is unclosed, so is any class starting at a later `q`: the
/// scan from `p` reads every position after `q` the same way the scan from `q` would (`q`
/// holds a `[`, so it never escapes its successor), and any `]` the scan from `q` would accept
/// as its leading member would have closed the class at `p`.
fn parse_class(chars: &[char], start: usize) -> Option<(String, usize)> {
    let mut pos = start + 1;
    let mut negated = false;
    if matches!(chars.get(pos), Some('!') | Some('^')) {
        negated = true;
        pos += 1;
    }
    let mut items: Vec<char> = Vec::new();
    // A `]` right after the (optional) negation is a literal member.
    if chars.get(pos) == Some(&']') {
        items.push(']');
        pos += 1;
    }
    loop {
        match chars.get(pos) {
            None => return None,
            Some(']') => break,
            Some('\\') => {
                pos += 1;
                items.push(*chars.get(pos)?);
                pos += 1;
            }
            Some(&c) => {
                items.push(c);
                pos += 1;
            }
        }
    }
    // `items` isn't empty: a `]` right after the `[` (or `[!`) was taken as a member above.
    let mut class = String::from(if negated { "[^" } else { "[" });
    let mut i = 0;
    while i < items.len() {
        let c = items[i];
        if i + 2 < items.len() && items[i + 1] == '-' && items[i + 2] >= c {
            push_class_char(&mut class, c);
            class.push('-');
            push_class_char(&mut class, items[i + 2]);
            i += 3;
        } else {
            push_class_char(&mut class, c);
            i += 1;
        }
    }
    class.push(']');
    Some((class, pos + 1))
}

fn push_class_char(out: &mut String, c: char) {
    if c.is_ascii_alphanumeric() || !c.is_ascii() {
        out.push(c);
    } else {
        // Every ASCII punctuation character may be escaped inside a regex class.
        out.push('\\');
        out.push(c);
    }
}

fn emit(tokens: &[Token], mode: GlobMode, out: &mut String) {
    let mut i = 0;
    while i < tokens.len() {
        let tok = &tokens[i];
        let prev_is_slash = i == 0 || tokens[i - 1] == Token::Literal('/');
        match tok {
            Token::Literal('/')
                if mode == GlobMode::Path
                    && tokens.get(i + 1) == Some(&Token::DoubleStar)
                    && i + 2 == tokens.len() =>
            {
                // Trailing `/**`: the slash and everything after it are optional.
                out.push_str("(?:/.*)?");
                i += 2;
                continue;
            }
            Token::Literal(c) => push_literal(out, *c),
            Token::Star => out.push_str(if mode == GlobMode::Path {
                "[^/]*"
            } else {
                ".*"
            }),
            Token::Question => out.push_str(if mode == GlobMode::Path { "[^/]" } else { "." }),
            Token::DoubleStar => {
                if mode == GlobMode::Path
                    && prev_is_slash
                    && tokens.get(i + 1) == Some(&Token::Literal('/'))
                {
                    // `/**/` (or a leading `**/`): zero or more whole segments.
                    out.push_str("(?:.*/)?");
                    i += 2;
                    continue;
                }
                out.push_str(".*");
            }
            Token::Class(class) => out.push_str(class),
            Token::Alt(alts) => {
                out.push_str("(?:");
                for (n, alt) in alts.iter().enumerate() {
                    if n > 0 {
                        out.push('|');
                    }
                    emit(alt, mode, out);
                }
                out.push(')');
            }
        }
        i += 1;
    }
}

fn push_literal(out: &mut String, c: char) {
    let mut buf = [0u8; 4];
    out.push_str(&regex::escape(c.encode_utf8(&mut buf)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use regex::Regex;

    fn m(glob: &str, mode: GlobMode, input: &str) -> bool {
        Regex::new(&glob_to_regex(glob, mode).unwrap())
            .unwrap()
            .is_match(input)
    }

    #[test]
    fn host_star_crosses_dots() {
        assert!(m("*.example.com", GlobMode::Any, "api.example.com"));
        assert!(m("*.example.com", GlobMode::Any, "a.b.example.com"));
        assert!(!m("*.example.com", GlobMode::Any, "example.com"));
        assert!(!m("*.example.com", GlobMode::Any, "example.com.evil.org"));
        assert!(m("{,*.}example.com", GlobMode::Any, "example.com"));
        assert!(m("{,*.}example.com", GlobMode::Any, "www.example.com"));
    }

    #[test]
    fn path_star_stays_in_segment() {
        assert!(m("/api/*", GlobMode::Path, "/api/users"));
        assert!(!m("/api/*", GlobMode::Path, "/api/users/1"));
        assert!(m("/api/*/1", GlobMode::Path, "/api/users/1"));
        assert!(m("/api/**", GlobMode::Path, "/api/users/1"));
        assert!(m("/api/**", GlobMode::Path, "/api"));
        assert!(m("/api/**", GlobMode::Path, "/api/"));
        assert!(!m("/api/**", GlobMode::Path, "/apix"));
        assert!(m("/a/**/b", GlobMode::Path, "/a/b"));
        assert!(m("/a/**/b", GlobMode::Path, "/a/x/y/b"));
        assert!(!m("/a/**/b", GlobMode::Path, "/a/xb"));
        assert!(m("**/*.js", GlobMode::Path, "/static/app.js"));
        assert!(m("**/*.js", GlobMode::Path, "app.js"));
        assert!(m("/file?.txt", GlobMode::Path, "/file1.txt"));
        assert!(!m("/file?.txt", GlobMode::Path, "/file/.txt"));
        assert!(m("/a**b", GlobMode::Path, "/a/x/b"));
    }

    #[test]
    fn classes_alternatives_and_escapes() {
        assert!(m("/v[0-9]/x", GlobMode::Path, "/v2/x"));
        assert!(!m("/v[!0-9]/x", GlobMode::Path, "/v2/x"));
        assert!(m("/v[^0-9]/x", GlobMode::Path, "/vb/x"));
        assert!(m("/{users,groups}/*", GlobMode::Path, "/groups/7"));
        assert!(!m("/{users,groups}/*", GlobMode::Path, "/teams/7"));
        assert!(m("/{a,b{c,d}}", GlobMode::Path, "/bd"));
        assert!(m(r"/literal\*", GlobMode::Path, "/literal*"));
        assert!(!m(r"/literal\*", GlobMode::Path, "/literalx"));
        // Unclosed constructs are literal.
        assert!(m("/a[b", GlobMode::Path, "/a[b"));
        assert!(m("/a{b", GlobMode::Path, "/a{b"));
        // Regex metacharacters are literal.
        assert!(m("/a.b+(c)$", GlobMode::Path, "/a.b+(c)$"));
        assert!(!m("/a.b", GlobMode::Path, "/axb"));
        assert!(m("[]]", GlobMode::Any, "]"));
        assert!(m("[-a]", GlobMode::Any, "-"));
    }

    #[test]
    fn brace_edge_cases() {
        // Unclosed `{` are literal; the braces that do close still form alternatives.
        assert!(m("{a,{b}", GlobMode::Any, "{a,b"));
        assert!(m("{a{,b}", GlobMode::Any, "{a"));
        assert!(m("{a{,b}", GlobMode::Any, "{ab"));
        assert!(m("{{a}", GlobMode::Any, "{a"));
        assert!(m("{a}}", GlobMode::Any, "a}"));
        assert!(m("x},y", GlobMode::Any, "x},y"));
        assert!(m("{a,b", GlobMode::Any, "{a,b"));
        // Classes and escapes hide braces and commas.
        assert!(m("{[}],x}", GlobMode::Any, "}"));
        assert!(m("{[}],x}", GlobMode::Any, "x"));
        assert!(m(r"{a\},b}", GlobMode::Any, "a}"));
        assert!(m(r"{a\,b,c}", GlobMode::Any, "a,b"));
        assert!(!m(r"{a\,b,c}", GlobMode::Any, "a"));
        assert!(m("[{]a}", GlobMode::Any, "{a}"));
    }

    /// An unclosed `{` used to make the parser re-parse the rest of the glob, doubling the work
    /// per brace (a rule save with 30 of them took minutes), and to recurse once per brace
    /// (about 20,000 of them overflowed the stack and aborted the process).
    #[test]
    fn pathological_globs_are_linear() {
        let start = std::time::Instant::now();
        let glob = format!("/api/{}", "{".repeat(64));
        assert!(m(&glob, GlobMode::Path, &glob));
        let glob = "{a,".repeat(5_000);
        assert!(m(&glob, GlobMode::Any, &glob));
        let glob = "{".repeat(100_000);
        assert!(glob_to_regex(&glob, GlobMode::Path).is_ok());
        let glob = format!("{}{}", "{".repeat(50_000), "}".repeat(10));
        assert!(glob_to_regex(&glob, GlobMode::Any).is_ok());
        let glob = format!("{}a", "[".repeat(100_000));
        assert!(glob_to_regex(&glob, GlobMode::Any).is_ok());
        let glob = "[!{".repeat(20_000);
        assert!(glob_to_regex(&glob, GlobMode::Any).is_ok());
        let glob = "{a,b}".repeat(2_000);
        assert!(glob_to_regex(&glob, GlobMode::Any).is_ok());
        let elapsed = start.elapsed();
        assert!(elapsed.as_secs_f64() < 5.0, "took {elapsed:?}");

        // Nesting is capped (the parser and the regex translation recurse per level).
        let nested = |n: usize| format!("{}a{}", "{".repeat(n), "}".repeat(n));
        assert!(m(&nested(MAX_BRACE_DEPTH), GlobMode::Any, "a"));
        let err = glob_to_regex(&nested(MAX_BRACE_DEPTH + 1), GlobMode::Any).unwrap_err();
        assert!(err.contains("nested"), "{err}");
        assert!(glob_to_regex(&nested(100_000), GlobMode::Any).is_err());
        // Unclosed braces around a deep but valid nesting don't count.
        let glob = format!("{}{}", "{".repeat(1_000), nested(MAX_BRACE_DEPTH));
        assert!(glob_to_regex(&glob, GlobMode::Any).is_ok());
    }

    /// The previous recursive parser, kept as the reference for the linear one (its results
    /// were right, only its running time and recursion depth weren't).
    mod reference {
        use super::super::{Token, parse_class};

        pub(super) fn parse(chars: &[char]) -> Vec<Token> {
            let mut pos = 0;
            parse_seq(chars, &mut pos, false)
        }

        fn parse_seq(chars: &[char], pos: &mut usize, in_alt: bool) -> Vec<Token> {
            let mut out = Vec::new();
            while *pos < chars.len() {
                let c = chars[*pos];
                match c {
                    ',' | '}' if in_alt => return out,
                    '\\' => {
                        *pos += 1;
                        match chars.get(*pos) {
                            Some(&next) => {
                                out.push(Token::Literal(next));
                                *pos += 1;
                            }
                            None => out.push(Token::Literal('\\')),
                        }
                    }
                    '*' => {
                        let start = *pos;
                        while *pos < chars.len() && chars[*pos] == '*' {
                            *pos += 1;
                        }
                        out.push(if *pos - start >= 2 {
                            Token::DoubleStar
                        } else {
                            Token::Star
                        });
                    }
                    '?' => {
                        out.push(Token::Question);
                        *pos += 1;
                    }
                    '[' => match parse_class(chars, *pos) {
                        Some((class, end)) => {
                            out.push(Token::Class(class));
                            *pos = end;
                        }
                        None => {
                            out.push(Token::Literal('['));
                            *pos += 1;
                        }
                    },
                    '{' => match parse_alt(chars, *pos) {
                        Some((alts, end)) => {
                            out.push(Token::Alt(alts));
                            *pos = end;
                        }
                        None => {
                            out.push(Token::Literal('{'));
                            *pos += 1;
                        }
                    },
                    other => {
                        out.push(Token::Literal(other));
                        *pos += 1;
                    }
                }
            }
            out
        }

        fn parse_alt(chars: &[char], start: usize) -> Option<(Vec<Vec<Token>>, usize)> {
            let mut pos = start + 1;
            let mut alts = Vec::new();
            loop {
                alts.push(parse_seq(chars, &mut pos, true));
                match chars.get(pos) {
                    Some(',') => pos += 1,
                    Some('}') => return Some((alts, pos + 1)),
                    _ => return None,
                }
            }
        }
    }

    #[test]
    fn linear_parser_agrees_with_the_reference() {
        const ALPHABET: &[char] = &['{', '}', ',', '[', ']', '!', '\\', '*', '?', 'a', '/'];
        let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..50_000 {
            let len = (next() % 14) as usize;
            let chars: Vec<char> = (0..len)
                .map(|_| ALPHABET[(next() % ALPHABET.len() as u64) as usize])
                .collect();
            let structure = Structure::scan(&chars);
            let mut parser = Parser {
                chars: &chars,
                structure: &structure,
                pos: 0,
            };
            let glob: String = chars.iter().collect();
            assert_eq!(
                parser.parse_seq(false, 0).unwrap(),
                reference::parse(&chars),
                "glob {glob:?}"
            );
        }
    }

    #[test]
    fn unicode_literals() {
        assert!(m("/caf\u{e9}/*", GlobMode::Path, "/caf\u{e9}/menu"));
        assert!(m(
            "*.\u{4f8b}\u{5b50}",
            GlobMode::Any,
            "www.\u{4f8b}\u{5b50}"
        ));
    }
}
