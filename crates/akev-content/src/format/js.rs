//! Token-based JavaScript beautifier.
//!
//! Safety rules — the output must mean exactly what the input meant:
//! * Tokens (strings, template literals including their `${}` substitutions, regex literals,
//!   comments, numbers, identifiers) are copied verbatim; only the whitespace *between* tokens
//!   changes.
//! * Existing line breaks are never removed (so ASI behaves identically); runs of blank lines
//!   collapse to one.
//! * New line breaks are only inserted after `{`, `;` (outside `for(...)` headers), `,` inside
//!   object literals, `case x:` labels, before `}` and between `}` and a following statement
//!   keyword/identifier. None of these can split a restricted production (`return`/`throw`/
//!   `break`/`continue`/`yield`/`async` + operand, postfix `++`/`--`, `=>`).
//! * Whitespace between two tokens is only dropped where tokens can't merge (before `,` `;`
//!   `)` `]`, after `(` `[`, around `.`), so `a + +b` never becomes `a++b`.
//!
//! Regex-vs-division is decided like most hand-written JS lexers: a `/` starts a regex where an
//! operand is expected — after operators, `(`/`[`/`,`, expression keywords (`return`,
//! `typeof`, ...), the `)` of an `if`/`for`/`while`/`with` header and the `}` of a block.
//! Template literals are tracked with an explicit stack so nested `${ `...` }` work.

use crate::util::push_indent;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum K {
    Word,
    Num,
    Str,
    /// A complete template literal (substitutions included, collapsed by [`Tokens`]).
    Template,
    TemplateHead,
    TemplateMiddle,
    TemplateTail,
    Regex,
    Punct,
    LineComment,
    BlockComment,
}

#[derive(Clone, Copy, Debug)]
struct Tok {
    kind: K,
    start: usize,
    end: usize,
    /// Line breaks before the token (capped at 2).
    nl: u8,
    /// Whitespace before the token.
    ws: bool,
    /// The lexer expected an operand here (`+`/`-` are unary, `/` starts a regex).
    expr: bool,
    /// For `{`: the brace opens a block (not an object literal).
    block: bool,
    /// A word that follows `.` / `?.` (a property name, never a keyword).
    property: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Open {
    Paren { control: bool },
    Bracket,
    Brace { block: bool },
    TemplateSub,
}

const REGEX_AFTER_WORDS: &[&str] = &[
    "return",
    "typeof",
    "instanceof",
    "in",
    "of",
    "new",
    "delete",
    "void",
    "throw",
    "case",
    "do",
    "else",
    "yield",
    "await",
];
const EXPR_WORDS_BEFORE_BRACE: &[&str] = &[
    "return",
    "typeof",
    "instanceof",
    "in",
    "of",
    "new",
    "delete",
    "void",
    "throw",
    "case",
    "yield",
    "await",
];
const CONTROL_WORDS: &[&str] = &["if", "for", "while", "switch", "catch", "with"];
const SPACE_AFTER_WORDS: &[&str] = &[
    "return",
    "typeof",
    "case",
    "throw",
    "else",
    "do",
    "in",
    "of",
    "new",
    "delete",
    "void",
    "yield",
    "await",
    "instanceof",
    "export",
    "import",
    "from",
    "as",
    "extends",
    "let",
    "const",
    "var",
    "async",
    "static",
];
const KEYWORDS: &[&str] = &[
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "export",
    "extends",
    "finally",
    "for",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "new",
    "return",
    "switch",
    "throw",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "yield",
    "let",
    "static",
    "await",
    "async",
    "of",
];

struct Lexer<'a> {
    src: &'a str,
    s: &'a [u8],
    pos: usize,
    stack: Vec<Open>,
    regex_ok: bool,
    /// Previous significant token (kind, start, end).
    prev: Option<(K, usize, usize)>,
    /// The previous significant token is `.` / `?.`.
    prev_dot: bool,
    /// The previous significant token is a property name.
    prev_property: bool,
    /// Inside `case ...` / `default` before its `:` (with the `?` nesting seen so far).
    case_label: Option<u32>,
    /// The previous significant token is the `:` ending a case label.
    prev_case_colon: bool,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Self {
        Lexer {
            src,
            s: src.as_bytes(),
            pos: 0,
            stack: Vec::new(),
            regex_ok: true,
            prev: None,
            prev_dot: false,
            prev_property: false,
            case_label: None,
            prev_case_colon: false,
        }
    }

    fn prev_text(&self) -> &'a str {
        self.prev.map_or("", |(_, a, b)| &self.src[a..b])
    }

    fn next(&mut self) -> Option<Tok> {
        let s = self.s;
        let ws_start = self.pos;
        let mut nl: u8 = 0;
        while self.pos < s.len() {
            let b = s[self.pos];
            match b {
                b'\n' => {
                    nl = nl.saturating_add(1);
                    self.pos += 1;
                }
                b'\r' => {
                    if s.get(self.pos + 1) != Some(&b'\n') {
                        nl = nl.saturating_add(1);
                    }
                    self.pos += 1;
                }
                b' ' | b'\t' | 0x0b | 0x0c => self.pos += 1,
                0x80.. => {
                    let c = self.src[self.pos..].chars().next().unwrap_or('\0');
                    if c == '\u{2028}' || c == '\u{2029}' {
                        nl = nl.saturating_add(1);
                    } else if !(c.is_whitespace() || c == '\u{feff}') {
                        break;
                    }
                    self.pos += c.len_utf8();
                }
                _ => break,
            }
        }
        if self.pos >= s.len() {
            return None;
        }
        let ws = self.pos > ws_start;
        let start = self.pos;
        let expr = self.regex_ok;
        let at_line_start = nl > 0 || self.prev.is_none();
        let b = s[start];
        let mut tok = Tok {
            kind: K::Punct,
            start,
            end: start + 1,
            nl: nl.min(2),
            ws,
            expr,
            block: false,
            property: false,
        };

        let rest = &s[start..];
        if rest.starts_with(b"//")
            || rest.starts_with(b"<!--")
            || (start == 0 && rest.starts_with(b"#!"))
            || (at_line_start && rest.starts_with(b"-->"))
        {
            tok.kind = K::LineComment;
            tok.end = memchr::memchr2(b'\n', b'\r', rest).map_or(s.len(), |e| start + e);
            self.pos = tok.end;
            return Some(tok);
        }
        if rest.starts_with(b"/*") {
            tok.kind = K::BlockComment;
            tok.end =
                memchr::memmem::find(&rest[2..], b"*/").map_or(s.len(), |e| start + 2 + e + 2);
            self.pos = tok.end;
            return Some(tok);
        }

        match b {
            b'\'' | b'"' => {
                tok.kind = K::Str;
                tok.end = scan_string(s, start);
            }
            b'`' => {
                let (end, sub) = scan_template(s, start + 1);
                tok.end = end;
                if sub {
                    tok.kind = K::TemplateHead;
                    self.stack.push(Open::TemplateSub);
                } else {
                    tok.kind = K::Template;
                }
            }
            b'}' if self.stack.last() == Some(&Open::TemplateSub) => {
                self.stack.pop();
                let (end, sub) = scan_template(s, start + 1);
                tok.end = end;
                if sub {
                    tok.kind = K::TemplateMiddle;
                    self.stack.push(Open::TemplateSub);
                } else {
                    tok.kind = K::TemplateTail;
                }
            }
            b'/' if self.regex_ok => {
                if let Some(end) = scan_regex(s, start) {
                    tok.kind = K::Regex;
                    tok.end = end;
                } else {
                    tok.end = start + punct_len(rest);
                }
            }
            b'0'..=b'9' => {
                tok.kind = K::Num;
                tok.end = scan_number(s, start);
            }
            b'.' if matches!(s.get(start + 1), Some(b'0'..=b'9')) => {
                tok.kind = K::Num;
                tok.end = scan_number(s, start);
            }
            b'a'..=b'z' | b'A'..=b'Z' | b'_' | b'$' | b'\\' | b'#' | 0x80.. => {
                tok.kind = K::Word;
                tok.end = scan_word(self.src, start);
                tok.property = self.prev_dot;
            }
            _ => {
                tok.end = start + punct_len(rest);
            }
        }
        self.pos = tok.end;
        self.after_token(&mut tok);
        Some(tok)
    }

    /// Update the bracket stack and the regex/operand expectation after a significant token.
    fn after_token(&mut self, tok: &mut Tok) {
        let text = &self.src[tok.start..tok.end];
        match tok.kind {
            K::Word => {
                self.regex_ok = !tok.property && REGEX_AFTER_WORDS.contains(&text);
                if !tok.property
                    && (text == "case" || (text == "default" && self.next_non_ws() == Some(b':')))
                {
                    self.case_label = Some(0);
                }
            }
            K::Num | K::Str | K::Regex | K::Template | K::TemplateTail => self.regex_ok = false,
            K::TemplateHead | K::TemplateMiddle => self.regex_ok = true,
            K::Punct => match text {
                "(" => {
                    let control = matches!(self.prev, Some((K::Word, ..)))
                        && !self.prev_property
                        && CONTROL_WORDS.contains(&self.prev_text());
                    self.stack.push(Open::Paren { control });
                    self.regex_ok = true;
                }
                ")" => {
                    let control = match self.stack.last() {
                        Some(Open::Paren { control }) => {
                            let c = *control;
                            self.stack.pop();
                            c
                        }
                        _ => false,
                    };
                    self.regex_ok = control;
                }
                "[" => {
                    self.stack.push(Open::Bracket);
                    self.regex_ok = true;
                }
                "]" => {
                    if self.stack.last() == Some(&Open::Bracket) {
                        self.stack.pop();
                    }
                    self.regex_ok = false;
                }
                "{" => {
                    let block = self.brace_is_block();
                    tok.block = block;
                    self.stack.push(Open::Brace { block });
                    self.regex_ok = true;
                }
                "}" => {
                    let block = match self.stack.last() {
                        Some(Open::Brace { block }) => {
                            let b = *block;
                            self.stack.pop();
                            b
                        }
                        _ => true,
                    };
                    tok.block = block;
                    self.regex_ok = block;
                }
                "++" | "--" => self.regex_ok = false,
                _ => self.regex_ok = true,
            },
            K::LineComment | K::BlockComment => return,
        }
        let mut case_colon = false;
        if tok.kind == K::Punct {
            match (text, self.case_label.as_mut()) {
                ("?", Some(depth)) => *depth += 1,
                (":", Some(0)) => {
                    case_colon = true;
                    self.case_label = None;
                }
                (":", Some(depth)) => *depth -= 1,
                _ => {}
            }
        }
        self.prev_case_colon = case_colon;
        self.prev_dot = tok.kind == K::Punct && (text == "." || text == "?.");
        self.prev = Some((tok.kind, tok.start, tok.end));
        self.prev_property = tok.kind == K::Word && tok.property;
    }

    fn next_non_ws(&self) -> Option<u8> {
        self.s[self.pos..]
            .iter()
            .copied()
            .find(|c| !c.is_ascii_whitespace())
    }

    fn brace_is_block(&self) -> bool {
        if self.prev_case_colon {
            return true;
        }
        match self.prev {
            None => true,
            Some((K::Punct, a, b)) => matches!(&self.src[a..b], ")" | ";" | "{" | "}" | "=>"),
            Some((K::Word, a, b)) => {
                self.prev_property || !EXPR_WORDS_BEFORE_BRACE.contains(&&self.src[a..b])
            }
            Some(_) => false,
        }
    }
}

fn scan_string(s: &[u8], start: usize) -> usize {
    let q = s[start];
    let mut i = start + 1;
    while i < s.len() {
        match s[i] {
            b'\\' => {
                // escape, including line continuations (\ CR LF)
                i += if s.get(i + 1) == Some(&b'\r') && s.get(i + 2) == Some(&b'\n') {
                    3
                } else {
                    2
                };
            }
            b'\n' | b'\r' => return i, // unterminated
            c if c == q => return i + 1,
            _ => i += 1,
        }
    }
    s.len()
}

/// Scan template characters from `i` (just after `` ` `` or `}`); returns the end index and
/// whether it stopped at a `${` substitution.
fn scan_template(s: &[u8], mut i: usize) -> (usize, bool) {
    while i < s.len() {
        match s[i] {
            b'\\' => i += 2,
            b'`' => return (i + 1, false),
            b'$' if s.get(i + 1) == Some(&b'{') => return (i + 2, true),
            _ => i += 1,
        }
    }
    (s.len(), false)
}

fn scan_regex(s: &[u8], start: usize) -> Option<usize> {
    let mut i = start + 1;
    let mut in_class = false;
    if matches!(s.get(i), Some(b'*' | b'/')) {
        return None;
    }
    loop {
        match *s.get(i)? {
            b'\\' => i += 2,
            b'\n' | b'\r' => return None,
            b'[' => {
                in_class = true;
                i += 1;
            }
            b']' => {
                in_class = false;
                i += 1;
            }
            b'/' if !in_class => {
                i += 1;
                while i < s.len() && (s[i].is_ascii_alphanumeric() || s[i] == b'_' || s[i] == b'$')
                {
                    i += 1;
                }
                return Some(i);
            }
            _ => i += 1,
        }
    }
}

fn scan_number(s: &[u8], start: usize) -> usize {
    let radix_prefixed = s[start] == b'0'
        && matches!(
            s.get(start + 1),
            Some(b'x' | b'X' | b'b' | b'B' | b'o' | b'O')
        );
    let mut i = start + 1;
    while i < s.len() {
        let c = s[i];
        let exponent_sign =
            (c == b'+' || c == b'-') && !radix_prefixed && matches!(s[i - 1], b'e' | b'E');
        if c.is_ascii_alphanumeric() || c == b'_' || c == b'.' || exponent_sign {
            i += 1;
        } else {
            break;
        }
    }
    i
}

fn scan_word(src: &str, start: usize) -> usize {
    let s = src.as_bytes();
    // the first character may be multi-byte (`é = 1`)
    let mut i = start + src[start..].chars().next().map_or(1, char::len_utf8);
    while i < s.len() {
        let c = s[i];
        if c.is_ascii_alphanumeric() || c == b'_' || c == b'$' || c == b'\\' {
            i += 1;
        } else if c == b'{' && s[i - 1] == b'u' && i >= 2 && s[i - 2] == b'\\' {
            // \u{XXXX} escape
            i = memchr::memchr(b'}', &s[i..]).map_or(s.len(), |e| i + e + 1);
        } else if c >= 0x80 {
            let ch = src[i..].chars().next().unwrap_or(' ');
            if ch.is_whitespace() || ch == '\u{feff}' {
                break;
            }
            i += ch.len_utf8();
        } else {
            break;
        }
    }
    i
}

fn punct_len(rest: &[u8]) -> usize {
    const P3: &[&[u8]] = &[
        b"===", b"!==", b"**=", b"<<=", b">>=", b">>>", b"...", b"&&=", b"||=", b"??=",
    ];
    const P2: &[&[u8]] = &[
        b"=>", b"==", b"!=", b"<=", b">=", b"&&", b"||", b"??", b"++", b"--", b"+=", b"-=", b"*=",
        b"/=", b"%=", b"&=", b"|=", b"^=", b"**", b"<<", b">>",
    ];
    if rest.starts_with(b">>>=") {
        return 4;
    }
    if P3.iter().any(|p| rest.starts_with(p)) {
        return 3;
    }
    if rest.starts_with(b"?.") && !matches!(rest.get(2), Some(b'0'..=b'9')) {
        return 2;
    }
    if P2.iter().any(|p| rest.starts_with(p)) {
        return 2;
    }
    // A single character (multi-byte safe: never split a UTF-8 sequence).
    match rest[0] {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2.min(rest.len()),
        0xe0..=0xef => 3.min(rest.len()),
        _ => 4.min(rest.len()),
    }
}

/// Token stream with template literals collapsed into single verbatim tokens.
struct Tokens<'a> {
    lx: Lexer<'a>,
}

impl Tokens<'_> {
    fn next(&mut self) -> Option<Tok> {
        let mut t = self.lx.next()?;
        if t.kind == K::TemplateHead {
            let mut depth = 1usize;
            let mut end = t.end;
            while depth > 0 {
                match self.lx.next() {
                    Some(inner) => {
                        match inner.kind {
                            K::TemplateHead => depth += 1,
                            K::TemplateTail => depth -= 1,
                            _ => {}
                        }
                        end = inner.end;
                    }
                    None => {
                        end = self.lx.s.len();
                        break;
                    }
                }
            }
            t.kind = K::Template;
            t.end = end;
        }
        Some(t)
    }
}

#[derive(Clone, Copy, Debug)]
struct Frame {
    open: u8,
    block: bool,
    /// Indentation of lines inside.
    inner: usize,
    /// Indentation of the line holding the opener (used for the closer line).
    outer: usize,
    switch: bool,
    /// For `(`: the parenthesis is a `switch (...)` head.
    switch_head: bool,
    is_do: bool,
    ternary: u32,
    /// A line break follows the opener.
    expanded: bool,
}

/// Index into [`Fmt::open_counts`] for an opener byte.
fn opener_slot(open: u8) -> usize {
    match open {
        b'{' => 0,
        b'(' => 1,
        _ => 2,
    }
}

struct Fmt<'a> {
    src: &'a str,
    out: String,
    frames: Vec<Frame>,
    /// Number of open `{` / `(` / `[` frames (see [`opener_slot`]), so a closer without a
    /// matching opener is rejected in O(1) instead of scanning the whole stack.
    open_counts: [usize; 3],
    top_ternary: u32,
    line_indent: usize,
    force_nl: bool,
    case_pending: bool,
    last_paren_switch: bool,
    prev: Option<Tok>,
    prev_was_comment: bool,
}

fn text<'a>(src: &'a str, t: &Tok) -> &'a str {
    &src[t.start..t.end]
}

fn is_binary(src: &str, t: &Tok, ternary_colon: bool) -> bool {
    if t.kind != K::Punct {
        return false;
    }
    match text(src, t) {
        "+" | "-" => !t.expr,
        ":" => ternary_colon,
        "*" | "/" | "%" | "**" | "==" | "===" | "!=" | "!==" | "<" | ">" | "<=" | ">=" | "&&"
        | "||" | "??" | "&" | "|" | "^" | "<<" | ">>" | ">>>" | "=" | "+=" | "-=" | "*=" | "/="
        | "%=" | "**=" | "<<=" | ">>=" | ">>>=" | "&=" | "|=" | "^=" | "&&=" | "||=" | "??="
        | "=>" | "?" => true,
        _ => false,
    }
}

impl<'a> Fmt<'a> {
    fn ternary_mut(&mut self) -> &mut u32 {
        match self.frames.last_mut() {
            Some(f) => &mut f.ternary,
            None => &mut self.top_ternary,
        }
    }

    fn push_frame(&mut self, frame: Frame) {
        self.open_counts[opener_slot(frame.open)] += 1;
        self.frames.push(frame);
    }

    /// Close the innermost frame opened by the matching bracket, implicitly closing any frames
    /// opened inside it. Amortized O(1): a closer without any matching opener is rejected via
    /// [`Fmt::open_counts`], and every frame scanned on success is removed.
    fn pop_frame(&mut self, closer: &str) -> Option<Frame> {
        let want = match closer {
            "}" => b'{',
            ")" => b'(',
            _ => b'[',
        };
        if self.open_counts[opener_slot(want)] == 0 {
            return None;
        }
        let idx = self.frames.iter().rposition(|f| f.open == want)?;
        let frame = self.frames[idx];
        for f in &self.frames[idx..] {
            self.open_counts[opener_slot(f.open)] -= 1;
        }
        self.frames.truncate(idx);
        if want == b'(' {
            self.last_paren_switch = frame.switch_head;
        }
        Some(frame)
    }

    fn indent_for(&self, tok: &Tok) -> usize {
        let t = text(self.src, tok);
        let mut indent = 0;
        if let Some(f) = self.frames.last() {
            indent = f.inner;
            if f.switch {
                let label = tok.kind == K::Word && !tok.property && (t == "case" || t == "default");
                if !label {
                    indent += 1;
                }
            }
        }
        if tok.kind == K::Punct && (t == "." || t == "?.") {
            indent += 1;
        }
        indent
    }

    /// Whether to put a space between `p` and `t` on the same line. A space present in the
    /// source is only dropped when the two tokens can't fuse into different tokens.
    fn space_between(&self, p: &Tok, t: &Tok) -> bool {
        self.space_policy(p, t) || (t.ws && risky_join(text(self.src, p), text(self.src, t)))
    }

    fn space_policy(&self, p: &Tok, t: &Tok) -> bool {
        let pt = text(self.src, p);
        let tt = text(self.src, t);
        let ternary_colon = tt == ":" && self.current_ternary() > 0;
        if matches!(t.kind, K::LineComment) {
            return true;
        }
        if matches!(t.kind, K::BlockComment) || self.prev_was_comment {
            return t.ws;
        }
        if t.kind == K::Punct && matches!(tt, "," | ";" | ")" | "]") {
            return false;
        }
        if p.kind == K::Punct && matches!(pt, "(" | "[") {
            return false;
        }
        if t.kind == K::Punct && (tt == "." || tt == "?.") {
            return t.ws && p.kind == K::Num;
        }
        if p.kind == K::Punct && matches!(pt, "." | "?." | "..." | "!" | "~") {
            return false;
        }
        // prefix ++/-- and unary +/- before an operand; postfix ++/-- after one
        if p.kind == K::Punct
            && matches!(pt, "++" | "--" | "+" | "-")
            && p.expr
            && matches!(t.kind, K::Word | K::Num)
        {
            return false;
        }
        if t.kind == K::Punct && matches!(tt, "++" | "--") && !t.expr {
            return false;
        }
        // generators: keep `function*` / `yield*` as written
        if t.kind == K::Punct
            && tt == "*"
            && p.kind == K::Word
            && matches!(pt, "function" | "yield")
        {
            return t.ws;
        }
        if p.kind == K::Word && !p.property && t.kind == K::Punct && tt == "(" {
            if CONTROL_WORDS.contains(&pt) {
                return true;
            }
            return t.ws && KEYWORDS.contains(&pt);
        }
        if t.kind == K::Punct && tt == ":" {
            return ternary_colon;
        }
        // After any colon (ternary, object key, label) comes a space.
        if p.kind == K::Punct && pt == ":" {
            return true;
        }
        if is_binary(self.src, p, false) || is_binary(self.src, t, ternary_colon) {
            return true;
        }
        if p.kind == K::Punct && (pt == "," || pt == ";") {
            return true;
        }
        if t.kind == K::Punct && tt == "{" {
            return matches!(p.kind, K::Word | K::Num | K::Str | K::Template)
                || matches!(pt, ")" | "]" | "=>")
                || t.ws;
        }
        if p.kind == K::Word
            && !p.property
            && SPACE_AFTER_WORDS.contains(&pt)
            && !(t.kind == K::Punct && tt == "(")
        {
            return true;
        }
        if p.kind == K::Punct
            && (pt == ")" || pt == "}")
            && matches!(t.kind, K::Word | K::Num | K::Str | K::Template | K::Regex)
        {
            return true;
        }
        t.ws
    }

    fn current_ternary(&self) -> u32 {
        self.frames.last().map_or(self.top_ternary, |f| f.ternary)
    }
}

/// Beautify JavaScript with every line indented by `base` extra levels. `None` for empty input
/// or if the output would exceed the size budget.
pub(crate) fn pretty(src: &str, base: usize) -> Option<String> {
    if src.trim().is_empty() {
        return None;
    }
    let budget = src.len().saturating_mul(8).saturating_add(1 << 20);
    let mut toks = Tokens {
        lx: Lexer::new(src),
    };
    let mut f = Fmt {
        src,
        out: String::with_capacity(src.len() + src.len() / 3 + 64),
        frames: Vec::new(),
        open_counts: [0; 3],
        top_ternary: 0,
        line_indent: 0,
        force_nl: false,
        case_pending: false,
        last_paren_switch: false,
        prev: None,
        prev_was_comment: false,
    };
    let mut next = toks.next();
    let mut count = 0usize;
    while let Some(tok) = next {
        next = toks.next();
        let tt = text(src, &tok);
        let is_closer = tok.kind == K::Punct && matches!(tt, "}" | ")" | "]");
        let prev_is_opener = f
            .prev
            .is_some_and(|p| p.kind == K::Punct && matches!(text(src, &p), "{" | "(" | "["));

        // ---- line breaks ----
        let mut nl = usize::from(tok.nl);
        if f.force_nl {
            nl = nl.max(1);
        }
        if tok.kind == K::Punct
            && tt == "}"
            && let Some(top) = f.frames.last()
            && top.open == b'{'
            && top.expanded
            && !prev_is_opener
        {
            nl = nl.max(1);
        }
        if nl == 2 && (is_closer || prev_is_opener) {
            nl = 1;
        }
        f.force_nl = false;

        // ---- closers pop before computing the line's indentation ----
        let closed = if is_closer { f.pop_frame(tt) } else { None };

        // ---- whitespace ----
        let ternary_colon = tok.kind == K::Punct && tt == ":" && f.current_ternary() > 0;
        if f.out.is_empty() {
            f.line_indent = f.indent_for(&tok);
            push_indent(&mut f.out, base + f.line_indent);
        } else if nl > 0 {
            for _ in 0..nl {
                f.out.push('\n');
            }
            f.line_indent = match &closed {
                Some(c) => c.outer,
                None => f.indent_for(&tok),
            };
            push_indent(&mut f.out, base + f.line_indent);
        } else if let Some(p) = f.prev
            && f.space_between(&p, &tok)
        {
            f.out.push(' ');
        }

        // ---- the token itself ----
        if tok.kind == K::BlockComment {
            write_block_comment(&mut f.out, tt, base + f.line_indent);
        } else {
            f.out.push_str(tt);
        }

        // ---- state updates & forced breaks ----
        let next_same_line_comment =
            next.is_some_and(|n| matches!(n.kind, K::LineComment | K::BlockComment) && n.nl == 0);
        let next_text = next.map_or("", |n| text(src, &n));
        match tok.kind {
            K::Punct => match tt {
                "{" => {
                    let switch = f
                        .prev
                        .is_some_and(|p| p.kind == K::Punct && text(src, &p) == ")")
                        && f.last_paren_switch;
                    let is_do = f
                        .prev
                        .is_some_and(|p| p.kind == K::Word && !p.property && text(src, &p) == "do");
                    let empty = next.is_some_and(|n| n.kind == K::Punct && next_text == "}");
                    let expanded = !empty && next.is_some();
                    f.push_frame(Frame {
                        open: b'{',
                        block: tok.block,
                        inner: f.line_indent + 1,
                        outer: f.line_indent,
                        switch,
                        switch_head: false,
                        is_do,
                        ternary: 0,
                        expanded,
                    });
                    if expanded && !next_same_line_comment {
                        f.force_nl = true;
                    }
                }
                "(" | "[" => {
                    let open = tt.as_bytes()[0];
                    let switch_head = open == b'('
                        && f.prev.is_some_and(|p| {
                            p.kind == K::Word && !p.property && text(src, &p) == "switch"
                        });
                    f.push_frame(Frame {
                        open,
                        block: false,
                        inner: f.line_indent + 1,
                        outer: f.line_indent,
                        switch: false,
                        switch_head,
                        is_do: false,
                        ternary: 0,
                        expanded: next.is_some_and(|n| n.nl > 0),
                    });
                }
                "}" => {
                    let statement_ctx = f.frames.last().is_none_or(|t| t.open == b'{' && t.block);
                    if let (Some(c), Some(n)) = (closed, next) {
                        let stays = match next_text {
                            "else" | "catch" | "finally" | "as" | "from" | "of" | "in"
                            | "instanceof" => true,
                            "while" => c.is_do,
                            _ => false,
                        };
                        if statement_ctx && c.block && n.kind == K::Word && !stays && n.nl == 0 {
                            f.force_nl = true;
                        }
                    }
                }
                ";" => {
                    let in_parens = f.frames.last().is_some_and(|t| t.open != b'{');
                    if !in_parens && !next_same_line_comment {
                        f.force_nl = true;
                    }
                }
                "," => {
                    let in_object = f.frames.last().is_some_and(|t| t.open == b'{' && !t.block);
                    let closer_next = next.is_some_and(|n| n.kind == K::Punct && next_text == "}");
                    if in_object && !next_same_line_comment && !closer_next {
                        f.force_nl = true;
                    }
                }
                "?" => *f.ternary_mut() += 1,
                ":" => {
                    if ternary_colon {
                        *f.ternary_mut() -= 1;
                    } else if f.case_pending {
                        f.case_pending = false;
                        let brace_next =
                            next.is_some_and(|n| n.kind == K::Punct && next_text == "{");
                        if !brace_next && !next_same_line_comment {
                            f.force_nl = true;
                        }
                    }
                }
                _ => {}
            },
            K::Word if !tok.property => {
                let in_switch = f.frames.last().is_some_and(|t| t.switch);
                if in_switch && (tt == "case" || (tt == "default" && next_text == ":")) {
                    f.case_pending = true;
                }
            }
            K::LineComment => f.force_nl = true,
            _ => {}
        }
        f.prev_was_comment = matches!(tok.kind, K::LineComment | K::BlockComment);
        if !f.prev_was_comment {
            f.prev = Some(tok);
        }
        count += 1;
        if count & 0x3ff == 0 && f.out.len() > budget {
            return None;
        }
    }
    if f.out.len() > budget {
        return None;
    }
    // No trailing trim: the formatter never emits trailing whitespace itself, and whitespace at
    // the very end belongs to a token (e.g. an unterminated template literal).
    Some(f.out)
}

/// Joining `a` and `b` without whitespace could change how they lex (`a b`, `+ +`, `?. 5`,
/// `1 .x`, `! =`, `/ /`...).
fn risky_join(a: &str, b: &str) -> bool {
    let (Some(x), Some(y)) = (a.chars().next_back(), b.chars().next()) else {
        return false;
    };
    let word =
        |c: char| c.is_alphanumeric() || matches!(c, '_' | '$' | '\\' | '#') || !c.is_ascii();
    let op = |c: char| "+-*/%=<>!&|^~?:.".contains(c);
    (word(x) && word(y))
        || (op(x) && op(y))
        || (matches!(x, '.' | '?') && y.is_ascii_digit())
        || (x.is_ascii_digit() && y == '.')
        // a number ending in `e` would absorb a following sign as its exponent (`1e -1`)
        || (matches!(x, 'e' | 'E')
            && matches!(y, '+' | '-')
            && a.starts_with(|c: char| c.is_ascii_digit() || c == '.'))
}

fn write_block_comment(out: &mut String, comment: &str, indent: usize) {
    let mut lines = comment.split('\n');
    if let Some(first) = lines.next() {
        out.push_str(first.trim_end_matches('\r'));
    }
    for line in lines {
        out.push('\n');
        let line = line.trim_end_matches('\r');
        let trimmed = line.trim_start();
        if trimmed.starts_with('*') {
            push_indent(out, indent);
            out.push(' ');
            out.push_str(trimmed);
        } else {
            out.push_str(line);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(s: &str) -> String {
        pretty(s, 0).expect("formats")
    }

    /// Re-lex both texts and check the significant token sequences are identical, i.e. the
    /// formatter only changed whitespace.
    fn tokens(s: &str) -> Vec<String> {
        let mut t = Tokens { lx: Lexer::new(s) };
        let mut v = Vec::new();
        while let Some(tok) = t.next() {
            if tok.kind == K::BlockComment {
                // continuation lines of block comments are re-indented
                v.push(
                    s[tok.start..tok.end]
                        .lines()
                        .map(str::trim)
                        .collect::<Vec<_>>()
                        .join("\n"),
                );
            } else {
                v.push(s[tok.start..tok.end].to_string());
            }
        }
        v
    }

    /// Per token: whether a line break preceded it.
    fn line_breaks(s: &str) -> Vec<bool> {
        let mut t = Tokens { lx: Lexer::new(s) };
        let mut v = Vec::new();
        while let Some(tok) = t.next() {
            v.push(tok.nl > 0);
        }
        v
    }

    fn assert_same_tokens(input: &str) -> String {
        let out = fmt(input);
        assert_eq!(tokens(input), tokens(&out), "token stream changed:\n{out}");
        out
    }

    #[test]
    fn minified_code() {
        let input = "function f(a,b){if(a>b){return a-b}else{return b?a:-b}}var o={x:1,y:[1,2,3],z:function(){return this.x}};for(var i=0;i<10;i++){f(i,2)}";
        let expected = "function f(a, b) {\n  if (a > b) {\n    return a - b\n  } else {\n    return b ? a : -b\n  }\n}\nvar o = {\n  x: 1,\n  y: [1, 2, 3],\n  z: function() {\n    return this.x\n  }\n};\nfor (var i = 0; i < 10; i++) {\n  f(i, 2)\n}";
        assert_eq!(fmt(input), expected);
        assert_eq!(fmt(expected), expected, "idempotent");
        assert_same_tokens(input);
    }

    #[test]
    fn regex_vs_division() {
        let out =
            assert_same_tokens("a=b/c/d;e=x.replace(/[/;{]+/g,'');if(t)/re;/.test(s);y=(1)/2/g");
        assert!(out.contains("a = b / c / d;"), "{out}");
        assert!(out.contains("x.replace(/[/;{]+/g, '')"), "{out}");
        assert!(out.contains("if (t) /re;/.test(s);"), "{out}");
        assert!(out.contains("y = (1) / 2 / g"), "{out}");
        // regex after return / typeof / operators, and after a block
        let out = assert_same_tokens("function g(){return /a{2}/.test(x)}\n{}/b;c/.exec(y)");
        assert!(out.contains("return /a{2}/.test(x)"), "{out}");
        assert!(out.contains("/b;c/.exec(y)"), "{out}");
        // `/=` is an operator after an operand
        assert!(fmt("x/=2").contains("x /= 2"));
    }

    #[test]
    fn template_literals_are_verbatim() {
        let input = "const q=`a;{b}\n  ${x?`in;{ner}${y}`:'z'}  ;end`;f({a:1})";
        let out = assert_same_tokens(input);
        assert!(
            out.contains("`a;{b}\n  ${x?`in;{ner}${y}`:'z'}  ;end`"),
            "{out}"
        );
        assert!(out.ends_with("f({\n  a: 1\n})"), "{out}");
        // unterminated template doesn't hang
        assert!(pretty("x = `oops ${a", 0).is_some());
    }

    #[test]
    fn asi_sensitive_code_keeps_line_breaks() {
        let input = "let a = 1\nlet b = a\n(function(){})()\nreturn\nx\ni\n++j\nconst f = async\nfunction g(){}";
        let out = assert_same_tokens(input);
        assert_eq!(out.lines().count(), input.lines().count(), "{out}");
        assert!(out.contains("return\nx"), "{out}");
        assert!(out.contains("i\n++j"), "{out}");
        assert!(out.contains("async\nfunction"), "{out}");
    }

    #[test]
    fn never_merges_operators() {
        let out = assert_same_tokens("a = b + +c - -d + ++e - --f; g = h++ + i; j = k - - l");
        assert!(out.contains("b + +c - -d + ++e - --f"), "{out}");
        assert!(out.contains("h++ + i"), "{out}");
        assert!(out.contains("k - -l"), "{out}"); // unary minus hugs its operand
        assert_same_tokens("x = 1 .toString(); y = a ?.5:1; z = a?.b ?? c");
        let out = fmt("x=a?.5:1");
        assert!(out.contains("a ? .5 : 1"), "{out}");
    }

    #[test]
    fn strings_comments_and_switch() {
        let input = "switch(x){case 1:a();break;case\"two\":{b()}default:c()}// tail\n/**\n   * doc\n   */\nvar s='a;{b}',t=\"c\\\"d\";";
        let out = assert_same_tokens(input);
        assert_eq!(
            out,
            "switch (x) {\n  case 1:\n    a();\n    break;\n  case \"two\": {\n    b()\n  }\n  default:\n    c()\n} // tail\n/**\n * doc\n */\nvar s = 'a;{b}', t = \"c\\\"d\";"
        );
    }

    #[test]
    fn do_while_else_catch() {
        let out = fmt("do{x()}while(y);try{a()}catch(e){b()}finally{c()}if(a){}else if(b){d()}");
        assert_eq!(
            out,
            "do {\n  x()\n} while (y);\ntry {\n  a()\n} catch (e) {\n  b()\n} finally {\n  c()\n}\nif (a) {} else if (b) {\n  d()\n}"
        );
    }

    #[test]
    fn arrows_classes_and_chains() {
        let out = assert_same_tokens(
            "const f=(a,b)=>{return a*b};class A extends B{constructor(){super();this.x=1}get y(){return 2}}p.then(r=>r.json())\n.catch(e=>log(e))",
        );
        assert!(
            out.contains("const f = (a, b) => {\n  return a * b\n};"),
            "{out}"
        );
        assert!(out.contains("class A extends B {\n  constructor() {\n    super();\n    this.x = 1\n  }\n  get y() {\n    return 2\n  }\n}"), "{out}");
        assert!(out.contains("\n  .catch(e => log(e))"), "{out}");
    }

    #[test]
    fn base_indent_and_edge_cases() {
        assert_eq!(pretty("a();b()", 2).unwrap(), "    a();\n    b()");
        assert_eq!(pretty("   ", 0), None);
        // unbalanced brackets and unterminated constructs don't panic
        for s in [
            "}}}",
            "((((",
            "'unterminated",
            "/* open",
            "a = /unterminated",
            "x = {",
            "<!-- html comment\nx()",
            "#!/usr/bin/env node\nx()",
        ] {
            let out = pretty(s, 0).unwrap();
            assert_eq!(tokens(s), tokens(&out), "{s:?} → {out:?}");
        }
        // non-ASCII identifiers and whitespace
        assert_same_tokens("const café\u{00a0}= 'ü';\u{2028}x=1");
    }

    #[test]
    fn random_soup_keeps_token_stream() {
        // No bare `\` fragment: outside literals a backslash only occurs inside identifier
        // escapes in valid JS; a stray one would let an inserted space change regex scanning
        // (`/\/=` vs `/ \ /=`), which only affects invalid input.
        const FRAGMENTS: &[&str] = &[
            "{", "}", "(", ")", "[", "]", ";", ",", ".", "?.", ":", "?", "!", "=", "=>", "+", "++",
            "-", "--", "*", "/", "/=", "<", ">", "&&", "'", "\"", "`", "${", " ", "\n", "\t",
            "\\u0061", "a", "x1", "0", ".5", "1e-3", "return", "if", "else", "case", "default",
            "switch", "do", "while", "function", "typeof", "async", "//c\n", "/*", "*/", "<!--",
            "-->", "é", "\u{a0}", "/re/g", "`t${a}`",
        ];
        let env = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<u64>().ok());
        let mut state: u64 = 0x2545_f491_4f6c_dd1d
            ^ env("AKEV_FUZZ_SEED")
                .unwrap_or(0)
                .wrapping_mul(0x9e37_79b9_7f4a_7c15)
            | 1;
        let iterations = env("AKEV_FUZZ_ITERS").unwrap_or(20_000);
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..iterations {
            let n = (next() % 40) as usize;
            let s: String = (0..n)
                .map(|_| FRAGMENTS[(next() % FRAGMENTS.len() as u64) as usize])
                .collect();
            if let Some(out) = pretty(&s, 0) {
                assert_eq!(tokens(&s), tokens(&out), "{s:?} → {out:?}");
                // a line break between two tokens in the source is never removed
                let a = line_breaks(&s);
                let b = line_breaks(&out);
                for i in 1..a.len() {
                    assert!(
                        !a[i] || b[i],
                        "line break before token {i} lost: {s:?} → {out:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn deep_nesting_is_bounded() {
        let deep = "[".repeat(100_000) + &"]".repeat(100_000);
        let _ = pretty(&deep, 0);
        let blocks = "{".repeat(50_000) + &"}".repeat(50_000);
        let _ = pretty(&blocks, 0);
    }

    #[test]
    fn unmatched_closers_stay_linear() {
        // Regression: every closer used to scan the whole open-bracket stack for its opener,
        // so 100k `(` followed by 100k `]` took ~10^10 steps (seconds in release).
        let start = std::time::Instant::now();
        for (open, close) in [("(", "]"), ("[", "}"), ("(", "}")] {
            let s = open.repeat(100_000) + &close.repeat(100_000);
            let out = pretty(&s, 0).unwrap();
            assert_eq!(tokens(&s), tokens(&out));
        }
        // One opener deep down the stack is found once, then the frames above it are gone.
        let s = "[".to_string() + &"(".repeat(100_000) + &"]".repeat(100_000);
        assert!(pretty(&s, 0).is_some());
        assert!(
            start.elapsed() < std::time::Duration::from_secs(20),
            "took {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn switch_head_survives_implicitly_closed_parens() {
        // `]` implicitly closes the inner `(`; the `)` that follows still closes the switch
        // head, so the body is laid out as a switch.
        let out = assert_same_tokens("switch(a[(b]){case 1:x();break;default:y()}");
        assert!(
            out.contains("case 1:\n    x();\n    break;\n  default:\n    y()"),
            "{out}"
        );
    }
}
