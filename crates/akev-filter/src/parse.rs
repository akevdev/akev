//! Recursive-descent parser: tokens → compiled [`Node`] tree, with precise error spans.
//!
//! Precedence, loosest first: `OR` (`|`, `||`), `AND` (`&&`, or juxtaposition), `NOT` (`-`, `!`),
//! then parentheses and terms. Values are compiled (regexes built, numbers and units resolved)
//! while parsing so errors are reported left to right.

use crate::ParseError;
use crate::eval::{Node, NumField, Pred, Range, Sides, StrField};
use crate::fields::{
    FIELDS, FieldId, FieldSpec, FieldType, FlagSpec, Smart, Unit, all_field_names, all_flag_names,
    closest, field_by_name, flag_by_name, kind_bit, kinds_by_alias, type_flag_by_alias,
    version_bit, versions_by_alias,
};
use crate::matcher::{LitOp, RegexFlags, TextMatcher};
use crate::scan::{Alt, Atom, AtomKind, Cmp, FieldPart, Op, RawTerm, Scanner, Span, Tok, Token};

/// Maximum nesting of parentheses / negations.
pub(crate) const MAX_DEPTH: usize = 64;

fn err(message: impl Into<String>, span: Span) -> ParseError {
    ParseError {
        message: message.into(),
        start: span.start,
        end: span.end,
    }
}

/// Parse a query into an (unoptimized) tree. Empty input yields [`Node::True`].
pub(crate) fn parse(src: &str) -> Result<Node, ParseError> {
    let mut p = Parser {
        src,
        sc: Scanner::new(src),
        peeked: None,
        depth: 0,
    };
    if p.peek().is_none() {
        return Ok(Node::True);
    }
    let node = p.parse_or()?;
    match p.next() {
        None => Ok(node),
        Some(Token {
            tok: Tok::RParen,
            span,
        }) => Err(err("Unmatched ')' — there is no '(' for it to close", span)),
        Some(Token { span, .. }) => Err(err("Unexpected input", span)),
    }
}

struct Parser<'a> {
    src: &'a str,
    sc: Scanner<'a>,
    peeked: Option<Token>,
    depth: usize,
}

impl<'a> Parser<'a> {
    fn peek(&mut self) -> Option<&Token> {
        if self.peeked.is_none() {
            self.peeked = self.sc.next_token();
        }
        self.peeked.as_ref()
    }

    fn next(&mut self) -> Option<Token> {
        match self.peeked.take() {
            Some(t) => Some(t),
            None => self.sc.next_token(),
        }
    }

    fn text(&self, span: Span) -> &'a str {
        self.src.get(span.start..span.end).unwrap_or("")
    }

    fn starts_unary(&mut self) -> bool {
        matches!(
            self.peek(),
            Some(Token {
                tok: Tok::Not | Tok::LParen | Tok::OrCall | Tok::Term(_),
                ..
            })
        )
    }

    fn parse_or(&mut self) -> Result<Node, ParseError> {
        let mut items = vec![self.parse_and()?];
        while let Some(Token { tok: Tok::Or, span }) = self.peek() {
            let span = *span;
            self.next();
            if !self.starts_unary() {
                return Err(err(
                    format!("Expected a filter after '{}'", self.text(span)),
                    span,
                ));
            }
            items.push(self.parse_and()?);
        }
        Ok(if items.len() == 1 {
            items.swap_remove(0)
        } else {
            Node::Or(items.into_boxed_slice())
        })
    }

    fn parse_and(&mut self) -> Result<Node, ParseError> {
        match self.peek() {
            Some(Token {
                tok: Tok::Or | Tok::And | Tok::Comma,
                span,
            }) => {
                let span = *span;
                return Err(err(
                    format!("Expected a filter before '{}'", self.text(span)),
                    span,
                ));
            }
            Some(Token {
                tok: Tok::RParen,
                span,
            }) => {
                let span = *span;
                return Err(err("Unmatched ')' — there is no '(' for it to close", span));
            }
            None => {
                return Err(err(
                    "Expected a filter",
                    Span::new(self.src.len(), self.src.len()),
                ));
            }
            _ => {}
        }
        let mut items = vec![self.parse_unary()?];
        loop {
            match self.peek() {
                Some(Token {
                    tok: Tok::And,
                    span,
                }) => {
                    let span = *span;
                    self.next();
                    if !self.starts_unary() {
                        return Err(err(
                            format!("Expected a filter after '{}'", self.text(span)),
                            span,
                        ));
                    }
                    items.push(self.parse_unary()?);
                }
                Some(Token {
                    tok: Tok::Not | Tok::LParen | Tok::OrCall | Tok::Term(_),
                    ..
                }) => {
                    items.push(self.parse_unary()?);
                }
                _ => break,
            }
        }
        Ok(if items.len() == 1 {
            items.swap_remove(0)
        } else {
            Node::And(items.into_boxed_slice())
        })
    }

    fn enter(&mut self, span: Span) -> Result<(), ParseError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(err("Filter is nested too deeply", span));
        }
        Ok(())
    }

    fn parse_unary(&mut self) -> Result<Node, ParseError> {
        let Some(tok) = self.next() else {
            return Err(err(
                "Expected a filter",
                Span::new(self.src.len(), self.src.len()),
            ));
        };
        match tok.tok {
            Tok::Not => {
                self.enter(tok.span)?;
                if !self.starts_unary() {
                    return Err(err(
                        format!("Expected a filter after '{}'", self.text(tok.span)),
                        tok.span,
                    ));
                }
                let inner = self.parse_unary()?;
                self.depth -= 1;
                Ok(Node::Not(Box::new(inner)))
            }
            Tok::LParen => {
                self.enter(tok.span)?;
                match self.peek() {
                    Some(Token {
                        tok: Tok::RParen,
                        span,
                    }) => {
                        let span = Span::new(tok.span.start, span.end);
                        return Err(err("Empty parentheses — put a filter inside", span));
                    }
                    None => return Err(err("Missing ')' to close this '('", tok.span)),
                    _ => {}
                }
                let inner = self.parse_or()?;
                match self.next() {
                    Some(Token {
                        tok: Tok::RParen, ..
                    }) => {
                        self.depth -= 1;
                        Ok(inner)
                    }
                    _ => Err(err("Missing ')' to close this '('", tok.span)),
                }
            }
            Tok::OrCall => {
                self.enter(tok.span)?;
                let node = self.or_call(tok.span)?;
                self.depth -= 1;
                Ok(node)
            }
            Tok::Term(term) => self.term(term),
            Tok::RParen => Err(err(
                "Unmatched ')' — there is no '(' for it to close",
                tok.span,
            )),
            Tok::Or | Tok::And | Tok::Comma => Err(err(
                format!("Expected a filter before '{}'", self.text(tok.span)),
                tok.span,
            )),
        }
    }

    /// HTTP Toolkit's `or(a, b, …)`: matches when any of the comma-separated filters does.
    fn or_call(&mut self, name: Span) -> Result<Node, ParseError> {
        // The scanner only emits `OrCall` immediately before a `(`.
        let open = match self.next() {
            Some(Token {
                tok: Tok::LParen,
                span,
            }) => span,
            _ => return Err(err("Expected '(' after 'or'", name)),
        };
        let call = Span::new(name.start, open.end);
        if let Some(Token {
            tok: Tok::RParen,
            span,
        }) = self.peek()
        {
            let span = Span::new(call.start, span.end);
            return Err(err(
                "Empty or() — list filters inside, separated by commas: or(status:404, is:error)",
                span,
            ));
        }
        let mut items = Vec::new();
        loop {
            if self.peek().is_none() {
                return Err(err("Missing ')' to close this 'or('", call));
            }
            items.push(self.parse_or()?);
            match self.next() {
                Some(Token {
                    tok: Tok::Comma,
                    span,
                }) => {
                    if !self.starts_unary() {
                        return Err(err("Expected a filter after ','", span));
                    }
                }
                Some(Token {
                    tok: Tok::RParen, ..
                }) => break,
                _ => return Err(err("Missing ')' to close this 'or('", call)),
            }
        }
        Ok(if items.len() == 1 {
            items.swap_remove(0)
        } else {
            Node::Or(items.into_boxed_slice())
        })
    }

    fn term(&mut self, term: RawTerm) -> Result<Node, ParseError> {
        let Some(fp) = &term.field else {
            return match term.alts.first() {
                Some(alt) => free_text(&alt.atom, self.src),
                None => Ok(Node::True),
            };
        };
        let name = self.text(fp.name);
        let lower = name.to_ascii_lowercase();
        if lower == "is" || lower == "has" {
            return self.flags(&lower, fp, &term);
        }
        let Some(field) = field_by_name(name) else {
            if self.url_like_unknown(&lower, fp, &term) {
                return self.fallback_text(term.span.start);
            }
            return Err(unknown_field(name, fp, self.src));
        };
        match (&fp.subkey, field.subkey) {
            (Some(key), None) => {
                // `host.docker.internal:8080`, `items[0]` — hostnames and text, not sub-keys.
                if (fp.op == Op::Colon && !key.bracket && single_bare(&term))
                    || fp.op == Op::Present
                {
                    return self.fallback_text(term.span.start);
                }
                let span = Span::new(fp.name.end, key.span.end + usize::from(key.bracket));
                return Err(err(
                    format!(
                        "'{}' doesn't take a sub-key (only header, reqheader, resheader, cookie and query do)",
                        field.name
                    ),
                    span,
                ));
            }
            (Some(key), Some(what)) if key.text.trim().is_empty() => {
                let span = Span::new(fp.name.end, key.span.end + usize::from(key.bracket));
                return Err(err(
                    format!("Expected a {what} after '{}.'", self.text(fp.name)),
                    span,
                ));
            }
            _ => {}
        }
        compile_field(field, fp, &term, self.src)
    }

    /// Whether an unknown `word:value` is really free text such as `localhost:8080`,
    /// `http://x` or `api.example.com:443`.
    fn url_like_unknown(&self, lower: &str, fp: &FieldPart, term: &RawTerm) -> bool {
        if fp.op == Op::Present {
            return true;
        }
        if fp.op != Op::Colon || !single_bare(term) {
            return false;
        }
        let value = term.alts.first().map_or("", |a| a.atom.text.as_str());
        if value.starts_with("//") {
            return true;
        }
        match &fp.subkey {
            Some(key) if !key.bracket => {
                // A dotted name is a hostname unless it is a typo of a sub-key field
                // (`heder.x-foo:bar`).
                let subkey_fields = FIELDS
                    .iter()
                    .filter(|f| f.subkey.is_some())
                    .flat_map(|f| std::iter::once(f.name).chain(f.aliases.iter().copied()));
                closest(lower, subkey_fields).is_none()
            }
            Some(_) => false,
            None => {
                value.as_bytes().first().is_some_and(u8::is_ascii_digit)
                    && closest(lower, all_field_names()).is_none()
            }
        }
    }

    fn fallback_text(&mut self, start: usize) -> Result<Node, ParseError> {
        debug_assert!(self.peeked.is_none());
        let term = self.sc.rescan_as_text(start);
        match term.alts.first() {
            Some(alt) => free_text(&alt.atom, self.src),
            None => Ok(Node::True),
        }
    }

    fn flags(&self, prefix: &str, fp: &FieldPart, term: &RawTerm) -> Result<Node, ParseError> {
        if let Some(key) = &fp.subkey {
            let span = Span::new(fp.name.end, key.span.end + usize::from(key.bracket));
            return Err(err(
                format!("Write flags as {prefix}:<flag>, e.g. is:error or has:body"),
                span,
            ));
        }
        if !matches!(fp.op, Op::Colon | Op::Eq | Op::Ne) {
            return Err(err(
                format!("Write flags as {prefix}:<flag>, e.g. is:error or has:body"),
                fp.op_span,
            ));
        }
        let mut nodes = Vec::with_capacity(term.alts.len());
        for (i, alt) in term.alts.iter().enumerate() {
            if i == 0
                && alt.cmp.is_none()
                && alt.atom.kind == AtomKind::Bare
                && alt.atom.text.is_empty()
            {
                let span = Span::new(fp.name.start, fp.op_span.end);
                return Err(err(
                    format!(
                        "Expected a flag after '{}' — e.g. is:error, is:pending, has:body",
                        self.text(span)
                    ),
                    span,
                ));
            }
            check_atom(alt, i, fp, self.src, false)?;
            if let Some((_, span)) = alt.cmp {
                return Err(err(
                    format!("Write flags as {prefix}:<flag>, e.g. is:error or has:body"),
                    span,
                ));
            }
            let text = alt.atom.text.trim();
            let flag: &FlagSpec = match flag_by_name(text) {
                Some(f) => f,
                None => {
                    let message = match closest(text, all_flag_names()) {
                        Some(s) => format!("Unknown flag '{text}' — did you mean '{s}'?"),
                        None => format!(
                            "Unknown flag '{text}' (try is:error, is:pending, is:pinned, has:body…; \
                             for a header use header:{text})"
                        ),
                    };
                    return Err(err(message, alt.atom.span));
                }
            };
            nodes.push(Node::Pred(Pred::Flag(flag.id)));
        }
        let node = if nodes.len() == 1 {
            nodes.swap_remove(0)
        } else {
            Node::Or(nodes.into_boxed_slice())
        };
        Ok(if fp.op == Op::Ne {
            Node::Not(Box::new(node))
        } else {
            node
        })
    }
}

/// Every alternative is an unquoted word without a comparison (so the term can be re-read as
/// free text).
fn single_bare(term: &RawTerm) -> bool {
    !term.alts.is_empty()
        && term
            .alts
            .iter()
            .all(|a| a.cmp.is_none() && a.atom.kind == AtomKind::Bare)
}

fn unknown_field(name: &str, fp: &FieldPart, src: &str) -> ParseError {
    let message = match closest(name, all_field_names()) {
        Some(s) => format!("Unknown field '{name}' — did you mean '{s}'?"),
        None => {
            let text = src
                .get(fp.name.start..)
                .unwrap_or("")
                .split_whitespace()
                .next()
                .unwrap_or(name);
            format!(
                "Unknown field '{name}'. To search for text containing '{}', quote it: \"{text}\"",
                fp.op.as_str()
            )
        }
    };
    err(message, fp.name)
}

/// Validate the shape of a value: present, terminated.
fn check_atom(
    alt: &Alt,
    index: usize,
    fp: &FieldPart,
    src: &str,
    numeric: bool,
) -> Result<(), ParseError> {
    let atom = &alt.atom;
    match &atom.kind {
        AtomKind::Quoted { terminated: false } => {
            let q = src
                .get(atom.span.start..atom.span.start + 1)
                .unwrap_or("\"");
            return Err(err(
                format!("Unterminated string — add a closing {q}"),
                atom.span,
            ));
        }
        AtomKind::Regex {
            terminated: false, ..
        } => {
            return Err(err(
                "Unterminated regex — add a closing '/' (use \\s to match spaces)",
                atom.span,
            ));
        }
        AtomKind::Paren { terminated: false } => {
            let call = Span::new(fp.name.start, atom.span.end);
            let name = src.get(fp.name.start..fp.name.end).unwrap_or("contains");
            return Err(err(format!("Missing ')' to close '{name}('"), call));
        }
        AtomKind::Bare if atom.text.is_empty() => {
            if let Some((cmp, span)) = alt.cmp {
                let what = if numeric { "a number" } else { "a value" };
                return Err(err(
                    format!("Expected {what} after '{}'", cmp.as_str()),
                    span,
                ));
            }
            if index > 0 {
                let comma = alt.span.start.saturating_sub(1);
                return Err(err(
                    "Expected a value after ','",
                    Span::new(comma, alt.span.start),
                ));
            }
            if numeric && matches!(fp.op, Op::Gt | Op::Ge | Op::Lt | Op::Le) {
                return Err(err(
                    format!("Expected a number after '{}'", fp.op.as_str()),
                    fp.op_span,
                ));
            }
            let field = src.get(fp.name.start..fp.op_span.end).unwrap_or("");
            let span = Span::new(fp.name.start, fp.op_span.end);
            return Err(err(format!("Expected a value after '{field}'"), span));
        }
        _ => {}
    }
    Ok(())
}

/// Compile a free-text term (URL, method and status).
fn free_text(atom: &Atom, src: &str) -> Result<Node, ParseError> {
    let m = match &atom.kind {
        AtomKind::Quoted { terminated: false } => {
            let q = src
                .get(atom.span.start..atom.span.start + 1)
                .unwrap_or("\"");
            return Err(err(
                format!("Unterminated string — add a closing {q}"),
                atom.span,
            ));
        }
        AtomKind::Quoted { .. } | AtomKind::Paren { .. } => {
            TextMatcher::literal(LitOp::Contains, &atom.text, false)
        }
        AtomKind::Bare if atom.text.contains('*') => TextMatcher::glob(&atom.text, false),
        AtomKind::Bare => TextMatcher::literal(LitOp::Contains, &atom.text, false),
        AtomKind::Regex { flags, terminated } => {
            if !terminated {
                return Err(err(
                    "Unterminated regex — add a closing '/' (use \\s to match spaces)",
                    atom.span,
                ));
            }
            regex_matcher(&atom.text, flags, false, atom)?
        }
    };
    Ok(match m {
        TextMatcher::Any => Node::True,
        m => Node::Pred(Pred::Text(m)),
    })
}

fn regex_matcher(
    pattern: &str,
    flags: &str,
    default_ci: bool,
    atom: &Atom,
) -> Result<TextMatcher, ParseError> {
    let mut parsed = RegexFlags::parse(flags).map_err(|c| {
        err(
            format!(
                "Unknown regex flag '{c}' (supported: i, m, s, x). To match a literal '/', \
                 escape it as \\/ or use ':' instead of '~'"
            ),
            atom.span,
        )
    })?;
    parsed.case_insensitive |= default_ci;
    TextMatcher::regex(pattern, parsed).map_err(|e| err(format!("Invalid regex: {e}"), atom.span))
}

/// Build the matcher for one text value.
fn text_matcher(smart: Smart, op: Op, alt: &Alt, long: bool) -> Result<TextMatcher, ParseError> {
    let atom = &alt.atom;
    let text = atom.text.as_str();
    Ok(match op {
        Op::Colon => match &atom.kind {
            AtomKind::Regex { flags, .. } => regex_matcher(text, flags, false, atom)?,
            AtomKind::Bare if text.contains('*') => TextMatcher::glob(text, true),
            _ => {
                let lit = if smart == Smart::Contains {
                    LitOp::Contains
                } else {
                    LitOp::Equals
                };
                TextMatcher::literal(lit, text, long)
            }
        },
        Op::Eq | Op::Ne => TextMatcher::literal(LitOp::Equals, text, long),
        Op::Prefix => TextMatcher::literal(LitOp::Prefix, text, long),
        Op::Suffix => TextMatcher::literal(LitOp::Suffix, text, long),
        Op::Contains => TextMatcher::literal(LitOp::Contains, text, long),
        Op::Regex => match &atom.kind {
            AtomKind::Regex { flags, .. } => regex_matcher(text, flags, false, atom)?,
            _ => regex_matcher(text, "", true, atom)?,
        },
        Op::Gt | Op::Ge | Op::Lt | Op::Le | Op::Present => TextMatcher::Any,
    })
}

fn numeric_hint(field: &FieldSpec, unit: Unit) -> String {
    let n = field.name;
    match unit {
        Unit::Status => format!("{n}:4xx, {n}>=400 or {n}:400-499"),
        Unit::Size => format!("{n}:>1mb, {n}<10kb or {n}:1kb-1mb"),
        Unit::Duration => format!("{n}:>1s, {n}<100ms or {n}:100-500"),
        Unit::Plain => format!("{n}:80, {n}>1000 or {n}:10-20"),
    }
}

fn compile_field(
    field: &'static FieldSpec,
    fp: &FieldPart,
    term: &RawTerm,
    src: &str,
) -> Result<Node, ParseError> {
    let op = fp.op;
    let name = field.name;
    if let FieldType::Number(unit) = field.ty {
        if !matches!(
            op,
            Op::Colon | Op::Eq | Op::Ne | Op::Gt | Op::Ge | Op::Lt | Op::Le
        ) {
            return Err(err(
                format!(
                    "'{}' works on text, but '{name}' is a number — try {}",
                    op.as_str(),
                    numeric_hint(field, unit)
                ),
                fp.op_span,
            ));
        }
        let mut ranges = Vec::with_capacity(term.alts.len());
        for (i, alt) in term.alts.iter().enumerate() {
            check_atom(alt, i, fp, src, true)?;
            ranges.push(parse_number(field, unit, op, alt)?);
        }
        let num = match field.id {
            FieldId::Status => NumField::Status,
            FieldId::Port => NumField::Port,
            FieldId::Size => NumField::Size,
            FieldId::ReqSize => NumField::ReqSize,
            FieldId::TotalSize => NumField::TotalSize,
            FieldId::Duration => NumField::Duration,
            FieldId::Id => NumField::Id,
            _ => NumField::Messages,
        };
        let node = Node::Pred(Pred::Num(num, ranges.into_boxed_slice()));
        return Ok(if op == Op::Ne {
            Node::Not(Box::new(node))
        } else {
            node
        });
    }

    if op == Op::Present {
        let key: Box<str> = fp.subkey.as_ref().map_or("", |k| k.text.trim()).into();
        let any: Box<[TextMatcher]> = Box::new([TextMatcher::Any]);
        return Ok(Node::Pred(match field.id {
            FieldId::Cookie => Pred::CookieValue(key, any),
            FieldId::Query => Pred::QueryParam(key, any),
            id => Pred::HeaderValue(header_sides(id), key, any),
        }));
    }
    if matches!(op, Op::Gt | Op::Ge | Op::Lt | Op::Le) {
        let smart_desc = match field.ty {
            FieldType::Text(Smart::Exact) if fp.subkey.is_none() => "is",
            _ => "contains",
        };
        return Err(err(
            format!(
                "'{}' compares numbers, but '{name}' is text — use {name}: ({smart_desc}), {name}= (equals), \
                 {name}^= (starts with), {name}$= (ends with) or {name}~ (regex)",
                op.as_str()
            ),
            fp.op_span,
        ));
    }

    let smart = match (field.ty, &fp.subkey) {
        (_, Some(_)) => Smart::Contains,
        (FieldType::Text(s), None) => s,
        (FieldType::Number(_), None) => Smart::Exact,
    };
    let long = matches!(
        field.id,
        FieldId::Body | FieldId::ReqBody | FieldId::ResBody
    );
    let mut matchers = Vec::with_capacity(term.alts.len());
    let mut kinds = 0u32;
    let mut versions = 0u8;
    let mut type_flags = Vec::new();
    let mut long_matchers = Vec::new();
    for (i, alt) in term.alts.iter().enumerate() {
        check_atom(alt, i, fp, src, false)?;
        // Text fields don't compare: `resbody:<form` searches for "<form".
        let literal_alt;
        let alt = match (alt.cmp, &alt.atom.kind) {
            (None, _) => alt,
            (Some(_), AtomKind::Bare) => {
                literal_alt = Alt {
                    cmp: None,
                    atom: Atom {
                        kind: AtomKind::Bare,
                        text: src
                            .get(alt.span.start..alt.atom.span.end)
                            .unwrap_or_default()
                            .to_string(),
                        span: Span::new(alt.span.start, alt.atom.span.end),
                    },
                    span: alt.span,
                };
                &literal_alt
            }
            (Some((cmp, span)), _) => {
                return Err(err(
                    format!(
                        "'{}' compares numbers, but '{name}' is text — quote the whole value to search for it",
                        cmp.as_str()
                    ),
                    span,
                ));
            }
        };
        // Whole-value comparisons resolve vocabulary aliases (`type:js`, `version!=h1`).
        let plain = matches!(op, Op::Colon | Op::Eq | Op::Ne)
            && !matches!(alt.atom.kind, AtomKind::Regex { .. })
            && !(alt.atom.kind == AtomKind::Bare && alt.atom.text.contains('*'));
        match field.id {
            FieldId::Type if plain => {
                let name = alt.atom.text.trim();
                if let Some(ks) = kinds_by_alias(name) {
                    kinds |= ks.iter().fold(0, |acc, k| acc | kind_bit(*k));
                    continue;
                }
                if let Some(flag) = type_flag_by_alias(name) {
                    type_flags.push(flag);
                    continue;
                }
            }
            // HTTP Toolkit's query values include the leading `?` (`query^=?page=`).
            FieldId::Query
                if fp.subkey.is_none()
                    && !matches!(alt.atom.kind, AtomKind::Regex { .. })
                    && alt.atom.text.len() > 1
                    && alt.atom.text.starts_with('?') =>
            {
                let mut alt = alt.clone();
                alt.atom.text.remove(0);
                matchers.push(text_matcher(smart, op, &alt, long)?);
                continue;
            }
            FieldId::Version if plain => {
                if let Some(vs) = versions_by_alias(alt.atom.text.trim()) {
                    versions |= vs.iter().fold(0, |acc, v| acc | version_bit(*v));
                    continue;
                }
            }
            FieldId::Color if plain && alt.atom.text.trim().eq_ignore_ascii_case("gray") => {
                let mut alt = alt.clone();
                alt.atom.text = "grey".to_string();
                matchers.push(text_matcher(smart, op, &alt, long)?);
                continue;
            }
            FieldId::Contains => {
                long_matchers.push(text_matcher(smart, op, alt, true)?);
            }
            _ => {}
        }
        matchers.push(text_matcher(smart, op, alt, long)?);
    }
    // `type:` values that name a state (`category=mutative`) select flags alongside kinds.
    let mut extra: Vec<Node> = type_flags
        .into_iter()
        .map(|f| Node::Pred(Pred::Flag(f)))
        .collect();
    if !extra.is_empty() && kinds == 0 && matchers.is_empty() {
        let node = if extra.len() == 1 {
            extra.swap_remove(0)
        } else {
            Node::Or(extra.into_boxed_slice())
        };
        return Ok(if op == Op::Ne {
            Node::Not(Box::new(node))
        } else {
            node
        });
    }
    let ms = matchers.into_boxed_slice();
    let sub = fp.subkey.as_ref().map(|k| Box::<str>::from(k.text.trim()));
    let pred = match field.id {
        FieldId::Method => Pred::Str(StrField::Method, ms),
        FieldId::Host => Pred::Str(StrField::Host, ms),
        FieldId::Path => Pred::Str(StrField::Path, ms),
        FieldId::Url => Pred::Str(StrField::Url, ms),
        FieldId::Scheme => Pred::Str(StrField::Scheme, ms),
        FieldId::Source => Pred::Str(StrField::Source, ms),
        FieldId::Rule => Pred::Str(StrField::Rule, ms),
        FieldId::Note => Pred::Str(StrField::Note, ms),
        FieldId::Color => Pred::Str(StrField::Color, ms),
        FieldId::Error => Pred::Str(StrField::Error, ms),
        FieldId::Ip => Pred::Str(StrField::Ip, ms),
        FieldId::Model => Pred::Str(StrField::Model, ms),
        FieldId::Provider => Pred::Str(StrField::Provider, ms),
        FieldId::Query => match sub {
            Some(key) => Pred::QueryParam(key, ms),
            None => Pred::Str(StrField::Query, ms),
        },
        FieldId::Type => Pred::Type {
            kinds,
            matchers: ms,
        },
        FieldId::Version => Pred::Version {
            versions,
            matchers: ms,
        },
        FieldId::Header | FieldId::ReqHeader | FieldId::ResHeader => {
            let sides = header_sides(field.id);
            match sub {
                Some(key) => Pred::HeaderValue(sides, key, ms),
                None => Pred::HeaderName(sides, ms),
            }
        }
        FieldId::Headers => Pred::HeaderLine(Sides::Both, ms),
        FieldId::Cookie => match sub {
            Some(key) => Pred::CookieValue(key, ms),
            None => Pred::CookieName(ms),
        },
        FieldId::Body => Pred::Body(Sides::Both, ms),
        FieldId::ReqBody => Pred::Body(Sides::Request, ms),
        FieldId::ResBody => Pred::Body(Sides::Response, ms),
        FieldId::Contains => Pred::Deep {
            short: ms,
            long: long_matchers.into_boxed_slice(),
        },
        // Numeric fields returned above; this arm is unreachable.
        FieldId::Status
        | FieldId::Port
        | FieldId::Size
        | FieldId::ReqSize
        | FieldId::TotalSize
        | FieldId::Duration
        | FieldId::Id
        | FieldId::Messages => return Ok(Node::False),
    };
    let node = if extra.is_empty() {
        Node::Pred(pred)
    } else {
        extra.insert(0, Node::Pred(pred));
        Node::Or(extra.into_boxed_slice())
    };
    Ok(if op == Op::Ne {
        Node::Not(Box::new(node))
    } else {
        node
    })
}

fn header_sides(id: FieldId) -> Sides {
    match id {
        FieldId::ReqHeader => Sides::Request,
        FieldId::ResHeader => Sides::Response,
        _ => Sides::Both,
    }
}

/// Split `12.5kb` into (12.5, "kb").
fn split_num_unit(text: &str) -> Option<(f64, &str)> {
    let idx = text
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(text.len());
    let (num, unit) = text.split_at(idx);
    if num.is_empty() || num == "." {
        return None;
    }
    let v: f64 = num.parse().ok()?;
    v.is_finite().then_some((v, unit.trim()))
}

fn unit_multiplier(unit: Unit, u: &str) -> Option<f64> {
    let u = u.to_ascii_lowercase();
    match unit {
        Unit::Plain | Unit::Status => u.is_empty().then_some(1.0),
        Unit::Size => match u.as_str() {
            "" | "b" | "byte" | "bytes" => Some(1.0),
            "k" | "kb" | "kib" => Some(1024.0),
            "m" | "mb" | "mib" => Some(1024.0 * 1024.0),
            "g" | "gb" | "gib" => Some(1024.0 * 1024.0 * 1024.0),
            "t" | "tb" | "tib" => Some(1024.0 * 1024.0 * 1024.0 * 1024.0),
            _ => None,
        },
        Unit::Duration => match u.as_str() {
            "" | "ms" | "msec" | "msecs" | "millis" => Some(1.0),
            "s" | "sec" | "secs" | "second" | "seconds" => Some(1000.0),
            "m" | "min" | "mins" | "minute" | "minutes" => Some(60_000.0),
            "h" | "hr" | "hrs" | "hour" | "hours" => Some(3_600_000.0),
            _ => None,
        },
    }
}

/// A single point value: a number (with optional unit) or a status class, as an inclusive
/// `(lo, hi)` pair. `inherit` supplies a unit for bare numbers (`1-2mb`).
fn parse_point(unit: Unit, text: &str, inherit: Option<&str>) -> Result<(f64, f64), PointError> {
    let text = text.trim();
    if unit == Unit::Status {
        let b = text.as_bytes();
        let is_x = |c: u8| c == b'x' || c == b'X';
        if b.len() == 3 && b[0].is_ascii_digit() {
            if is_x(b[1]) && is_x(b[2]) {
                let lo = f64::from(b[0] - b'0') * 100.0;
                return Ok((lo, lo + 99.0));
            }
            if b[1].is_ascii_digit() && is_x(b[2]) {
                let lo = f64::from(b[0] - b'0') * 100.0 + f64::from(b[1] - b'0') * 10.0;
                return Ok((lo, lo + 9.0));
            }
        }
    }
    let (v, u) = split_num_unit(text).ok_or(PointError::NotANumber)?;
    let u = if u.is_empty() {
        inherit.unwrap_or("")
    } else {
        u
    };
    let mult = unit_multiplier(unit, u).ok_or_else(|| PointError::BadUnit(u.to_string()))?;
    let v = v * mult;
    Ok((v, v))
}

enum PointError {
    NotANumber,
    BadUnit(String),
}

fn point_error(
    field: &FieldSpec,
    unit: Unit,
    text: &str,
    e: PointError,
    after: Option<&str>,
    span: Span,
) -> ParseError {
    let message = match e {
        PointError::BadUnit(u) => match unit {
            Unit::Size => format!("Unknown size unit '{u}' — use b, kb, mb or gb"),
            Unit::Duration => format!("Unknown duration unit '{u}' — use ms, s, m or h"),
            Unit::Plain | Unit::Status => {
                format!(
                    "'{}' takes a plain number, not '{text}' — e.g. {}",
                    field.name,
                    numeric_hint(field, unit)
                )
            }
        },
        PointError::NotANumber => match after {
            Some(cmp) => format!("Expected a number after '{cmp}'"),
            None => match unit {
                Unit::Status => format!(
                    "Invalid status '{text}' — use a code (404), a class (4xx), a range (400-499) or a comparison (>=500)"
                ),
                Unit::Size => format!(
                    "Invalid size '{text}' — use a number with an optional unit (b, kb, mb, gb), e.g. 500, 10kb, >1mb"
                ),
                Unit::Duration => format!(
                    "Invalid duration '{text}' — use a number with an optional unit (ms, s, m), e.g. 250, 1.5s, >2s"
                ),
                Unit::Plain => format!(
                    "Expected a number for '{}', e.g. {}",
                    field.name,
                    numeric_hint(field, unit)
                ),
            },
        },
    };
    err(message, span)
}

fn parse_number(field: &FieldSpec, unit: Unit, op: Op, alt: &Alt) -> Result<Range, ParseError> {
    let atom = &alt.atom;
    if let AtomKind::Regex { .. } = atom.kind {
        return Err(err(
            format!(
                "'{}' is a number, so regexes don't apply — try {}",
                field.name,
                numeric_hint(field, unit)
            ),
            atom.span,
        ));
    }
    let cmp = match op {
        Op::Gt => Some(Cmp::Gt),
        Op::Ge => Some(Cmp::Ge),
        Op::Lt => Some(Cmp::Lt),
        Op::Le => Some(Cmp::Le),
        _ => alt.cmp.map(|(c, _)| c),
    };
    let text = atom.text.trim();
    if let Some(cmp) = cmp {
        let (lo, hi) = parse_point(unit, text, None)
            .map_err(|e| point_error(field, unit, text, e, Some(cmp.as_str()), atom.span))?;
        return Ok(match cmp {
            Cmp::Gt => Range {
                lo: hi,
                hi: f64::INFINITY,
                lo_incl: false,
                hi_incl: true,
            },
            Cmp::Ge => Range {
                lo,
                hi: f64::INFINITY,
                lo_incl: true,
                hi_incl: true,
            },
            Cmp::Lt => Range {
                lo: f64::NEG_INFINITY,
                hi: lo,
                lo_incl: true,
                hi_incl: false,
            },
            Cmp::Le => Range {
                lo: f64::NEG_INFINITY,
                hi,
                lo_incl: true,
                hi_incl: true,
            },
        });
    }
    // `a-b` inclusive range (a leading '-' is not a range).
    if let Some(dash) = text.get(1..).and_then(|t| t.find('-')).map(|i| i + 1) {
        let (a, b) = (&text[..dash], &text[dash + 1..]);
        if b.trim().is_empty() {
            return Err(err("Expected a number after '-'", atom.span));
        }
        // `1-2mb`: a bare start inherits the end's unit.
        let b_unit = split_num_unit(b.trim())
            .map(|(_, u)| u)
            .filter(|u| !u.is_empty());
        let (lo, _) = parse_point(unit, a, b_unit)
            .map_err(|e| point_error(field, unit, text, e, None, atom.span))?;
        let (_, hi) = parse_point(unit, b, None)
            .map_err(|e| point_error(field, unit, text, e, None, atom.span))?;
        let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
        return Ok(Range::between(lo, hi));
    }
    let (lo, hi) = parse_point(unit, text, None)
        .map_err(|e| point_error(field, unit, text, e, None, atom.span))?;
    Ok(Range::between(lo, hi))
}
