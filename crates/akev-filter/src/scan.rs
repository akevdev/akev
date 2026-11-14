//! A tolerant scanner for the filter language.
//!
//! The scanner never fails: it records what it saw (including unterminated strings and regex
//! literals) with exact byte spans, and leaves validation to the parser. The same scanner powers
//! autocompletion and syntax highlighting, which must cope with half-typed input.
//!
//! All delimiters are ASCII, so scanning byte-wise is UTF-8 safe: every slice boundary is either
//! the position of an ASCII byte or the end of the input, both of which are char boundaries.

/// A byte range `[start, end)` in the query.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub(crate) fn new(start: usize, end: usize) -> Span {
        Span { start, end }
    }

    pub(crate) fn contains_cursor(self, cursor: usize) -> bool {
        self.start <= cursor && cursor <= self.end
    }

    pub(crate) fn is_empty(self) -> bool {
        self.start == self.end
    }
}

/// The operator between a field and its value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Op {
    /// `:` — smart match (contains / equals / glob / regex, depending on field and value).
    Colon,
    /// `=` (or `==`) — exact.
    Eq,
    /// `!=` — not exact.
    Ne,
    /// `^=` — starts with.
    Prefix,
    /// `$=` — ends with.
    Suffix,
    /// `*=` — contains (literal).
    Contains,
    /// `~` — regex.
    Regex,
    Gt,
    Ge,
    Lt,
    Le,
    /// `header[name]` with no operator: presence (HTTP Toolkit compatible syntax).
    Present,
}

impl Op {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Op::Colon => ":",
            Op::Eq => "=",
            Op::Ne => "!=",
            Op::Prefix => "^=",
            Op::Suffix => "$=",
            Op::Contains => "*=",
            Op::Regex => "~",
            Op::Gt => ">",
            Op::Ge => ">=",
            Op::Lt => "<",
            Op::Le => "<=",
            Op::Present => "",
        }
    }
}

/// A comparison prefix inside a `:` value (`size:>1mb`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cmp {
    Gt,
    Ge,
    Lt,
    Le,
}

impl Cmp {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Cmp::Gt => ">",
            Cmp::Ge => ">=",
            Cmp::Lt => "<",
            Cmp::Le => "<=",
        }
    }
}

/// How a value was written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AtomKind {
    /// An unquoted word.
    Bare,
    /// `"..."` or `'...'`.
    Quoted { terminated: bool },
    /// `/pattern/flags`. `flags` holds whatever followed the closing slash (validated by the
    /// parser).
    Regex { flags: String, terminated: bool },
    /// `(text)` right after `contains` — HTTP Toolkit's `contains(text)` syntax. The text is
    /// literal (no escapes) and ends at the first `)`; the span includes both parentheses.
    Paren { terminated: bool },
}

/// One value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Atom {
    pub kind: AtomKind,
    /// Unescaped text (for regex literals: the raw pattern between the slashes).
    pub text: String,
    pub span: Span,
}

/// One comma-separated alternative of a value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Alt {
    pub cmp: Option<(Cmp, Span)>,
    pub atom: Atom,
    pub span: Span,
}

/// A `.subkey` / `[subkey]` after a field name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Subkey {
    pub text: String,
    /// Span of the key text itself.
    pub span: Span,
    pub bracket: bool,
    /// False for `[key` without a closing bracket.
    pub terminated: bool,
}

/// `field[.subkey] op`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FieldPart {
    pub name: Span,
    pub subkey: Option<Subkey>,
    pub op: Op,
    pub op_span: Span,
}

/// A term: either `field op values` or free text (one alternative, no field).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RawTerm {
    pub field: Option<FieldPart>,
    pub alts: Vec<Alt>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Tok {
    LParen,
    RParen,
    /// `|`, `||`, `OR`
    Or,
    /// `&&`, `AND`
    And,
    /// `-`, `!`, `NOT`
    Not,
    /// `or` immediately followed by `(` where a filter is expected: HTTP Toolkit's
    /// `or(a, b, …)`. The next token is always the `(`.
    OrCall,
    /// `,` separating the filters of an `or(…)` call.
    Comma,
    Term(RawTerm),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Token {
    pub tok: Tok,
    pub span: Span,
}

/// How a value starting with `/` is interpreted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RegexMode {
    /// After `=`, `^=`, …: a `/` is an ordinary character.
    Never,
    /// Free text and `:`: `/pattern/flags` is a regex only when it is well-formed and clearly a
    /// regex (uses regex syntax or has an `i` flag), so `/api/users` stays a path.
    Smart,
    /// After `~`: a leading `/` always starts a regex literal.
    Strict,
}

const REGEX_FLAGS: &str = "imsxug";

/// How far [`Scanner::group_has_comma`] looks for the comma of an `or(a, b)` call.
const CALL_LOOKAHEAD: usize = 4096;

/// Characters that make a slash-delimited value "look like" a regex in smart mode.
fn is_regex_meta(b: u8) -> bool {
    matches!(
        b,
        b'\\'
            | b'.'
            | b'+'
            | b'*'
            | b'?'
            | b'('
            | b')'
            | b'|'
            | b'['
            | b']'
            | b'{'
            | b'}'
            | b'^'
            | b'$'
    )
}

/// Characters allowed in a dotted sub-key (`header.x-trace`).
fn is_subkey_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || matches!(
            b,
            b'-' | b'_' | b'.' | b'+' | b'#' | b'%' | b'&' | b'\'' | b'`'
        )
}

/// Characters allowed in a field name after the first.
fn is_name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

/// Pull-based scanner.
///
/// Besides the position, the scanner tracks just enough context to recognise HTTP Toolkit's
/// `or(a, b)` calls: the parenthesis depth, the depths of open calls, and whether the next token
/// starts an operand. Inside a call (at its own depth) a `,` separates filters instead of value
/// alternatives.
pub(crate) struct Scanner<'a> {
    src: &'a str,
    b: &'a [u8],
    pos: usize,
    /// Current parenthesis depth.
    depth: usize,
    /// Depths of the open `or(` calls, innermost last.
    calls: Vec<usize>,
    /// The previous token leaves us where a filter is expected (input start, `(`, `,` or an
    /// operator), so `or(` is a call rather than a binary OR followed by a group.
    operand_start: bool,
    /// The previous token was [`Tok::OrCall`]: the next `(` opens the call.
    call_pending: bool,
}

impl<'a> Scanner<'a> {
    pub(crate) fn new(src: &'a str) -> Scanner<'a> {
        Scanner {
            src,
            b: src.as_bytes(),
            pos: 0,
            depth: 0,
            calls: Vec::new(),
            operand_start: true,
            call_pending: false,
        }
    }

    /// Directly inside the parentheses of an `or(…)` call (not in a nested group).
    fn in_call(&self) -> bool {
        self.calls.last() == Some(&self.depth)
    }

    /// Whether the group opened by the `(` at `open` has a `,` directly inside it (outside
    /// nested groups and quoted strings) before its closing `)`. Looks at most
    /// [`CALL_LOOKAHEAD`] bytes ahead so scanning stays linear on adversarial input.
    fn group_has_comma(&self, open: usize) -> bool {
        let b = &self.b[..self.b.len().min(open.saturating_add(CALL_LOOKAHEAD))];
        let mut depth = 0usize;
        let mut i = open;
        while i < b.len() {
            match b[i] {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return false;
                    }
                }
                b',' if depth == 1 => return true,
                q @ (b'"' | b'\'') => {
                    i += 1;
                    while i < b.len() && b[i] != q {
                        i += if b[i] == b'\\' { 2 } else { 1 };
                    }
                }
                _ => {}
            }
            i += 1;
        }
        false
    }

    /// Length in bytes of the whitespace character at `i`, or 0.
    fn ws_len(&self, i: usize) -> usize {
        let Some(&c) = self.b.get(i) else { return 0 };
        if c.is_ascii() {
            return usize::from(c.is_ascii_whitespace());
        }
        match self.src.get(i..).and_then(|s| s.chars().next()) {
            Some(ch) if ch.is_whitespace() => ch.len_utf8(),
            _ => 0,
        }
    }

    /// Whether position `i` ends a word.
    fn is_delim(&self, i: usize, in_value: bool) -> bool {
        match self.b.get(i) {
            None => true,
            Some(b'(' | b')' | b'|') => true,
            Some(b',') => in_value || self.in_call(),
            Some(_) => self.ws_len(i) > 0,
        }
    }

    fn skip_ws(&mut self) {
        loop {
            let n = self.ws_len(self.pos);
            if n == 0 {
                break;
            }
            self.pos += n;
        }
    }

    /// Scan every token (for autocompletion / highlighting).
    pub(crate) fn tokenize(src: &'a str) -> Vec<Token> {
        let mut s = Scanner::new(src);
        let mut out = Vec::new();
        while let Some(t) = s.next_token() {
            out.push(t);
        }
        out
    }

    /// Next token, or `None` at the end of input.
    pub(crate) fn next_token(&mut self) -> Option<Token> {
        let token = self.raw_token()?;
        match token.tok {
            Tok::LParen => {
                self.depth += 1;
                if std::mem::take(&mut self.call_pending) {
                    self.calls.push(self.depth);
                }
            }
            Tok::RParen => {
                if self.in_call() {
                    self.calls.pop();
                }
                self.depth = self.depth.saturating_sub(1);
            }
            Tok::OrCall => self.call_pending = true,
            _ => {}
        }
        self.operand_start = !matches!(token.tok, Tok::RParen | Tok::Term(_));
        Some(token)
    }

    fn raw_token(&mut self) -> Option<Token> {
        self.skip_ws();
        let start = self.pos;
        let &c = self.b.get(start)?;
        let single = |s: &mut Scanner<'_>, tok: Tok, len: usize| {
            s.pos = start + len;
            Some(Token {
                tok,
                span: Span::new(start, start + len),
            })
        };
        match c {
            b'(' => return single(self, Tok::LParen, 1),
            b')' => return single(self, Tok::RParen, 1),
            b',' if self.in_call() => return single(self, Tok::Comma, 1),
            b'|' => {
                let len = if self.b.get(start + 1) == Some(&b'|') {
                    2
                } else {
                    1
                };
                return single(self, Tok::Or, len);
            }
            b'&' if self.b.get(start + 1) == Some(&b'&') => return single(self, Tok::And, 2),
            b'-' | b'!' => return single(self, Tok::Not, 1),
            _ => {}
        }
        if c.is_ascii_alphabetic() {
            let mut end = start;
            while end < self.b.len() && self.b[end].is_ascii_alphabetic() {
                end += 1;
            }
            if end - start <= 3 && self.is_delim(end, false) {
                let word = &self.src[start..end];
                let tok = if word.eq_ignore_ascii_case("or") {
                    // `or(a, b)` is HTTP Toolkit's call syntax. Where a filter is expected it
                    // can't be anything else; after a filter (`status=404 or(a, b)`, as HTTP
                    // Toolkit copies tags) it is a call when it lists several filters, while
                    // `x or(y)` stays a binary OR followed by a group.
                    if self.b.get(end) == Some(&b'(')
                        && (self.operand_start || self.group_has_comma(end))
                    {
                        Some(Tok::OrCall)
                    } else {
                        Some(Tok::Or)
                    }
                } else if word.eq_ignore_ascii_case("and") {
                    Some(Tok::And)
                } else if word.eq_ignore_ascii_case("not") {
                    Some(Tok::Not)
                } else {
                    None
                };
                if let Some(tok) = tok {
                    return single(self, tok, end - start);
                }
            }
        }
        Some(self.scan_term())
    }

    /// Re-scan the term starting at `start` as free text (used when an unknown `word:value`
    /// turns out to be a URL such as `localhost:8080`).
    pub(crate) fn rescan_as_text(&mut self, start: usize) -> RawTerm {
        self.pos = start;
        let atom = self.scan_bare(false);
        let span = atom.span;
        RawTerm {
            field: None,
            alts: vec![Alt {
                cmp: None,
                atom,
                span,
            }],
            span,
        }
    }

    fn scan_term(&mut self) -> Token {
        let start = self.pos;
        if let Some(field) = self.try_field_prefix(start) {
            self.pos = field.op_span.end;
            let alts = match field.op {
                Op::Present => Vec::new(),
                // `contains(text)` (the field prefix ends right before the `(`).
                Op::Colon if field.op_span.is_empty() => {
                    let atom = self.scan_paren();
                    let span = atom.span;
                    vec![Alt {
                        cmp: None,
                        atom,
                        span,
                    }]
                }
                Op::Colon => self.scan_alts(RegexMode::Smart, true),
                Op::Regex => self.scan_alts(RegexMode::Strict, false),
                _ => self.scan_alts(RegexMode::Never, false),
            };
            let span = Span::new(start, self.pos);
            return Token {
                tok: Tok::Term(RawTerm {
                    field: Some(field),
                    alts,
                    span,
                }),
                span,
            };
        }
        let atom = self.scan_atom(RegexMode::Smart, false);
        let span = atom.span;
        Token {
            tok: Tok::Term(RawTerm {
                field: None,
                alts: vec![Alt {
                    cmp: None,
                    atom,
                    span,
                }],
                span,
            }),
            span,
        }
    }

    /// Recognize `name[.subkey|[subkey]] op` at `start`.
    fn try_field_prefix(&self, start: usize) -> Option<FieldPart> {
        let b = self.b;
        let first = *b.get(start)?;
        if !(first.is_ascii_alphabetic() || first == b'_') {
            return None;
        }
        let mut i = start + 1;
        while i < b.len() && is_name_byte(b[i]) {
            i += 1;
        }
        let name = Span::new(start, i);
        let mut subkey = None;
        match b.get(i) {
            Some(b'.') => {
                let ks = i + 1;
                let mut j = ks;
                while j < b.len() && is_subkey_byte(b[j]) {
                    j += 1;
                }
                subkey = Some(Subkey {
                    text: self.src[ks..j].to_string(),
                    span: Span::new(ks, j),
                    bracket: false,
                    terminated: true,
                });
                i = j;
            }
            Some(b'[') => {
                let ks = i + 1;
                let ke = ks + memchr::memchr(b']', &b[ks..])?;
                subkey = Some(Subkey {
                    text: self.src[ks..ke].to_string(),
                    span: Span::new(ks, ke),
                    bracket: true,
                    terminated: true,
                });
                i = ke + 1;
            }
            _ => {}
        }
        let rest = &b[i..];
        let (op, len) = match rest {
            [b'=', b'=', ..] => (Op::Eq, 2),
            [b'!', b'=', ..] => (Op::Ne, 2),
            [b'^', b'=', ..] => (Op::Prefix, 2),
            [b'$', b'=', ..] => (Op::Suffix, 2),
            [b'*', b'=', ..] => (Op::Contains, 2),
            [b'>', b'=', ..] => (Op::Ge, 2),
            [b'<', b'=', ..] => (Op::Le, 2),
            [b':', ..] => (Op::Colon, 1),
            [b'=', ..] => (Op::Eq, 1),
            [b'~', ..] => (Op::Regex, 1),
            [b'>', ..] => (Op::Gt, 1),
            [b'<', ..] => (Op::Lt, 1),
            [b'(', ..]
                if subkey.is_none()
                    && self.src[name.start..name.end].eq_ignore_ascii_case("contains") =>
            {
                (Op::Colon, 0)
            }
            _ => {
                // `header[x-trace]` on its own means "header present".
                let bracketed = subkey.as_ref().is_some_and(|k| k.bracket);
                if bracketed && self.is_delim(i, false) {
                    (Op::Present, 0)
                } else {
                    return None;
                }
            }
        };
        Some(FieldPart {
            name,
            subkey,
            op,
            op_span: Span::new(i, i + len),
        })
    }

    fn scan_alts(&mut self, mode: RegexMode, allow_cmp: bool) -> Vec<Alt> {
        let mut alts = Vec::new();
        loop {
            let start = self.pos;
            let cmp = if allow_cmp { self.scan_cmp() } else { None };
            let atom = self.scan_atom(mode, true);
            alts.push(Alt {
                cmp,
                atom,
                span: Span::new(start, self.pos),
            });
            // Inside `or(…)` a comma separates filters, not alternatives.
            if self.b.get(self.pos) == Some(&b',') && !self.in_call() {
                self.pos += 1;
            } else {
                break;
            }
        }
        alts
    }

    /// `(text)` after `contains`: everything up to the first `)`, literally.
    fn scan_paren(&mut self) -> Atom {
        let start = self.pos;
        let inner = start + 1;
        let (text_end, end, terminated) =
            match memchr::memchr(b')', &self.b[inner.min(self.b.len())..]) {
                Some(i) => (inner + i, inner + i + 1, true),
                None => (self.b.len(), self.b.len(), false),
            };
        self.pos = end;
        Atom {
            kind: AtomKind::Paren { terminated },
            text: self.src[inner.min(text_end)..text_end].to_string(),
            span: Span::new(start, end),
        }
    }

    fn scan_cmp(&mut self) -> Option<(Cmp, Span)> {
        let start = self.pos;
        let (cmp, len) = match &self.b[start..] {
            [b'>', b'=', ..] => (Cmp::Ge, 2),
            [b'<', b'=', ..] => (Cmp::Le, 2),
            [b'>', ..] => (Cmp::Gt, 1),
            [b'<', ..] => (Cmp::Lt, 1),
            _ => return None,
        };
        self.pos += len;
        Some((cmp, Span::new(start, start + len)))
    }

    fn scan_atom(&mut self, mode: RegexMode, in_value: bool) -> Atom {
        let start = self.pos;
        match self.b.get(start) {
            Some(b'"' | b'\'') => self.scan_quoted(),
            Some(b'/') if mode == RegexMode::Strict => self.scan_regex_strict(in_value),
            Some(b'/') if mode == RegexMode::Smart => match self.try_regex_smart(in_value) {
                Some(atom) => atom,
                None => self.scan_bare(in_value),
            },
            _ => self.scan_bare(in_value),
        }
    }

    fn scan_bare(&mut self, in_value: bool) -> Atom {
        let start = self.pos;
        let mut i = start;
        while !self.is_delim(i, in_value) {
            i += 1;
        }
        self.pos = i;
        Atom {
            kind: AtomKind::Bare,
            text: self.src[start..i].to_string(),
            span: Span::new(start, i),
        }
    }

    fn scan_quoted(&mut self) -> Atom {
        let start = self.pos;
        let q = self.b[start];
        let mut text = String::new();
        let mut i = start + 1;
        let mut seg = i;
        let mut terminated = false;
        while i < self.b.len() {
            let c = self.b[i];
            if c == b'\\' {
                // A trailing backslash (nothing after it) is kept literally.
                let Some(ch) = self.src.get(i + 1..).and_then(|s| s.chars().next()) else {
                    i += 1;
                    continue;
                };
                text.push_str(&self.src[seg..i]);
                match ch {
                    '\\' | '"' | '\'' => text.push(ch),
                    'n' => text.push('\n'),
                    't' => text.push('\t'),
                    'r' => text.push('\r'),
                    other => {
                        text.push('\\');
                        text.push(other);
                    }
                }
                i += 1 + ch.len_utf8();
                seg = i;
            } else if c == q {
                text.push_str(&self.src[seg..i]);
                i += 1;
                terminated = true;
                break;
            } else {
                i += 1;
            }
        }
        if !terminated {
            i = self.b.len();
            text.push_str(&self.src[seg..i]);
        }
        self.pos = i;
        Atom {
            kind: AtomKind::Quoted { terminated },
            text,
            span: Span::new(start, i),
        }
    }

    /// Find the closing `/` of a regex literal starting at `start`, stopping at unescaped
    /// whitespace. Returns the index of the closing slash.
    fn find_regex_close(&self, start: usize) -> Result<usize, usize> {
        let mut i = start + 1;
        while i < self.b.len() {
            match self.b[i] {
                b'\\' => {
                    // Skip the escaped character (which may be multi-byte or whitespace).
                    let next = self.src.get(i + 1..).and_then(|s| s.chars().next());
                    i += 1 + next.map_or(0, char::len_utf8);
                }
                b'/' => return Ok(i),
                _ if self.ws_len(i) > 0 => return Err(i),
                _ => i += 1,
            }
        }
        Err(self.b.len())
    }

    fn try_regex_smart(&mut self, in_value: bool) -> Option<Atom> {
        let start = self.pos;
        let close = self.find_regex_close(start).ok()?;
        if close == start + 1 {
            return None;
        }
        let mut j = close + 1;
        while j < self.b.len() && self.b[j].is_ascii_alphabetic() {
            j += 1;
        }
        if !self.is_delim(j, in_value) {
            return None;
        }
        let flags = &self.src[close + 1..j];
        if !flags.chars().all(|c| REGEX_FLAGS.contains(c)) {
            return None;
        }
        let body = &self.src[start + 1..close];
        if !(flags.contains('i') || body.bytes().any(is_regex_meta)) {
            return None;
        }
        self.pos = j;
        Some(Atom {
            kind: AtomKind::Regex {
                flags: flags.to_string(),
                terminated: true,
            },
            text: body.to_string(),
            span: Span::new(start, j),
        })
    }

    fn scan_regex_strict(&mut self, in_value: bool) -> Atom {
        let start = self.pos;
        match self.find_regex_close(start) {
            Ok(close) => {
                let mut j = close + 1;
                while !self.is_delim(j, in_value) {
                    j += 1;
                }
                self.pos = j;
                Atom {
                    kind: AtomKind::Regex {
                        flags: self.src[close + 1..j].to_string(),
                        terminated: true,
                    },
                    text: self.src[start + 1..close].to_string(),
                    span: Span::new(start, j),
                }
            }
            Err(end) => {
                self.pos = end;
                Atom {
                    kind: AtomKind::Regex {
                        flags: String::new(),
                        terminated: false,
                    },
                    text: self.src[start + 1..end].to_string(),
                    span: Span::new(start, end),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<String> {
        Scanner::tokenize(src)
            .into_iter()
            .map(|t| match t.tok {
                Tok::LParen => "(".to_string(),
                Tok::RParen => ")".to_string(),
                Tok::Or => "OR".to_string(),
                Tok::And => "AND".to_string(),
                Tok::Not => "NOT".to_string(),
                Tok::OrCall => "OR-CALL".to_string(),
                Tok::Comma => "COMMA".to_string(),
                Tok::Term(term) => {
                    let mut s = String::new();
                    if let Some(f) = &term.field {
                        s.push_str(&src[f.name.start..f.name.end]);
                        if let Some(k) = &f.subkey {
                            s.push('[');
                            s.push_str(&k.text);
                            s.push(']');
                        }
                        s.push_str(f.op.as_str());
                    }
                    let alts: Vec<String> = term
                        .alts
                        .iter()
                        .map(|a| {
                            let cmp = a.cmp.map_or("", |(c, _)| c.as_str());
                            match &a.atom.kind {
                                AtomKind::Bare => format!("{cmp}{}", a.atom.text),
                                AtomKind::Quoted { terminated } => {
                                    format!(
                                        "{cmp}q{}({})",
                                        if *terminated { "" } else { "!" },
                                        a.atom.text
                                    )
                                }
                                AtomKind::Regex { flags, terminated } => format!(
                                    "{cmp}re{}({})/{flags}",
                                    if *terminated { "" } else { "!" },
                                    a.atom.text
                                ),
                                AtomKind::Paren { terminated } => format!(
                                    "p{}({})",
                                    if *terminated { "" } else { "!" },
                                    a.atom.text
                                ),
                            }
                        })
                        .collect();
                    s.push_str(&alts.join(","));
                    s
                }
            })
            .collect()
    }

    #[test]
    fn basic_tokens() {
        assert_eq!(
            kinds("status:404 host:example.com"),
            ["status:404", "host:example.com"]
        );
        assert_eq!(
            kinds("(a | b) || c && d"),
            ["(", "a", "OR", "b", ")", "OR", "c", "AND", "d"]
        );
        assert_eq!(
            kinds("a OR b and c NOT d not(e)"),
            ["a", "OR", "b", "AND", "c", "NOT", "d", "NOT", "(", "e", ")"]
        );
        assert_eq!(kinds("-is:pending !x"), ["NOT", "is:pending", "NOT", "x"]);
        assert_eq!(
            kinds("order nothing android"),
            ["order", "nothing", "android"]
        );
        assert_eq!(kinds("a&b &&c"), ["a&b", "AND", "c"]);
    }

    #[test]
    fn operators() {
        assert_eq!(
            kinds("a:1 b=2 c==3 d!=4 e^=5 f$=6 g*=7 h~8 i>9 j>=10 k<11 l<=12"),
            [
                "a:1", "b=2", "c=3", "d!=4", "e^=5", "f$=6", "g*=7", "h~8", "i>9", "j>=10", "k<11",
                "l<=12"
            ]
        );
        assert_eq!(kinds("size:>1mb,<=2kb"), ["size:>1mb,<=2kb"]);
        assert_eq!(kinds("size>1mb,2"), ["size>1mb,2"]);
    }

    #[test]
    fn subkeys() {
        assert_eq!(kinds("header.x-trace:abc"), ["header[x-trace]:abc"]);
        assert_eq!(kinds("header[x weird]=1"), ["header[x weird]=1"]);
        assert_eq!(kinds("header[x-trace]"), ["header[x-trace]"]);
        assert_eq!(kinds("header[x-trace"), ["header[x-trace"]);
        assert_eq!(kinds("example.com"), ["example.com"]);
    }

    #[test]
    fn quoting() {
        assert_eq!(kinds(r#""a b" 'c d'"#), ["q(a b)", "q(c d)"]);
        assert_eq!(kinds(r#""a \"b\" \\ \n \d""#), ["q(a \"b\" \\ \n \\d)"]);
        assert_eq!(kinds(r#"host:"x y"#), ["host:q!(x y)"]);
        assert_eq!(kinds(r#""unterminated \"#), ["q!(unterminated \\)"]);
        assert_eq!(kinds(r#"path:"a,b",c"#), ["path:q(a,b),c"]);
    }

    #[test]
    fn regex_literals() {
        // Smart mode: only clear regexes.
        assert_eq!(kinds("/api/users"), ["/api/users"]);
        assert_eq!(kinds("/api/"), ["/api/"]);
        assert_eq!(kinds("/api/x"), ["/api/x"]);
        assert_eq!(kinds("/a.*b/"), ["re(a.*b)/"]);
        assert_eq!(kinds("/users/i"), ["re(users)/i"]);
        assert_eq!(kinds("path:/^\\/api/,/x+/"), ["path:re(^\\/api)/,re(x+)/"]);
        assert_eq!(kinds("/static/*.js"), ["/static/*.js"]);
        assert_eq!(kinds("/a.b c/"), ["/a.b", "c/"]);
        // Strict mode after `~`.
        assert_eq!(kinds("path~/api/users"), ["path~re(api)/users"]);
        assert_eq!(kinds("path~/abc"), ["path~re!(abc)/"]);
        assert_eq!(kinds("path~/a b/"), ["path~re!(a)/", "b/"]);
        assert_eq!(kinds("path~/a\\ b/i"), ["path~re(a\\ b)/i"]);
        assert_eq!(kinds("path~^/v1"), ["path~^/v1"]);
        // `/` is ordinary after `=`.
        assert_eq!(kinds("path=/a.*/"), ["path=/a.*/"]);
    }

    #[test]
    fn spans_are_exact() {
        let toks = Scanner::tokenize("  status:>=500  (x)");
        assert_eq!(toks[0].span, Span::new(2, 14));
        let Tok::Term(t) = &toks[0].tok else { panic!() };
        let f = t.field.as_ref().unwrap();
        assert_eq!(f.name, Span::new(2, 8));
        assert_eq!(f.op_span, Span::new(8, 9));
        assert_eq!(t.alts[0].cmp.unwrap().1, Span::new(9, 11));
        assert_eq!(t.alts[0].atom.span, Span::new(11, 14));
        assert_eq!(toks[1].span, Span::new(16, 17));
        assert_eq!(toks[3].span, Span::new(18, 19));
    }

    #[test]
    fn empty_alternatives() {
        assert_eq!(kinds("method:"), ["method:"]);
        assert_eq!(kinds("method:get, x"), ["method:get,", "x"]);
        let toks = Scanner::tokenize("method:get,");
        let Tok::Term(t) = &toks[0].tok else { panic!() };
        assert_eq!(t.alts.len(), 2);
        assert_eq!(t.alts[1].span, Span::new(11, 11));
    }

    #[test]
    fn or_calls() {
        assert_eq!(
            kinds("or(method=get, status=404)"),
            ["OR-CALL", "(", "method=get", "COMMA", "status=404", ")"]
        );
        // Commas separate filters (not alternatives) at the call's own depth only.
        assert_eq!(
            kinds("OR(a,b,(c,d)) e,f"),
            [
                "OR-CALL", "(", "a", "COMMA", "b", "COMMA", "(", "c,d", ")", ")", "e,f"
            ]
        );
        assert_eq!(
            kinds("or(status:4xx,5xx)"),
            ["OR-CALL", "(", "status:4xx", "COMMA", "5xx", ")"]
        );
        // After a filter, `or(` is a call only when it lists several filters (as HTTP Toolkit
        // copies tags: `status=404 or(a, b)`); otherwise it's a binary OR and a group.
        assert_eq!(
            kinds("a or(b, c)"),
            ["a", "OR-CALL", "(", "b", "COMMA", "c", ")"]
        );
        assert_eq!(kinds("a or(b)"), ["a", "OR", "(", "b", ")"]);
        assert_eq!(
            kinds("a or(b (c,d))"),
            ["a", "OR", "(", "b", "(", "c,d", ")", ")"]
        );
        assert_eq!(kinds("a or(\"x,y\")"), ["a", "OR", "(", "q(x,y)", ")"]);
        assert_eq!(kinds("a or('x\\',y')"), ["a", "OR", "(", "q(x',y)", ")"]);
        assert_eq!(kinds("a or(b"), ["a", "OR", "(", "b"]);
        assert_eq!(
            kinds("(or(a, b)) | or(c,d)"),
            [
                "(", "OR-CALL", "(", "a", "COMMA", "b", ")", ")", "OR", "OR-CALL", "(", "c",
                "COMMA", "d", ")"
            ]
        );
        assert_eq!(kinds("or (a,b)"), ["OR", "(", "a,b", ")"]);
        assert_eq!(
            kinds("not(or(a,b))"),
            ["NOT", "(", "OR-CALL", "(", "a", "COMMA", "b", ")", ")"]
        );
        // Stray closing parentheses don't underflow.
        assert_eq!(
            kinds(") ) or(a,b"),
            [")", ")", "OR-CALL", "(", "a", "COMMA", "b"]
        );
        assert_eq!(kinds(") ) or(a"), [")", ")", "OR", "(", "a"]);
    }

    #[test]
    fn contains_call() {
        assert_eq!(kinds("contains(foo bar)"), ["contains:p(foo bar)"]);
        assert_eq!(kinds("CONTAINS(a, b) x"), ["CONTAINS:p(a, b)", "x"]);
        assert_eq!(kinds("contains(x"), ["contains:p!(x)"]);
        assert_eq!(kinds("contains("), ["contains:p!()"]);
        assert_eq!(kinds("contains()"), ["contains:p()"]);
        assert_eq!(kinds("contains(a)b)"), ["contains:p(a)", "b", ")"]);
        // Only `contains` takes this form.
        assert_eq!(kinds("body(x)"), ["body", "(", "x", ")"]);
        assert_eq!(kinds("contains (x)"), ["contains", "(", "x", ")"]);
        let toks = Scanner::tokenize("contains(ab)");
        let Tok::Term(t) = &toks[0].tok else { panic!() };
        assert_eq!(t.alts[0].atom.span, Span::new(8, 12));
        assert_eq!(t.span, Span::new(0, 12));
    }

    #[test]
    fn unicode_whitespace_and_text() {
        assert_eq!(kinds("héllo\u{a0}wörld"), ["héllo", "wörld"]);
        assert_eq!(kinds("host:ü,ö"), ["host:ü,ö"]);
        assert_eq!(kinds("\"ü\\ü\""), ["q(ü\\ü)"]);
        assert_eq!(kinds("path~/a\\ü/"), ["path~re(a\\ü)/"]);
    }
}
