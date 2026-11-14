//! Context-aware autocompletion.
//!
//! The query is scanned with the tolerant scanner, the token under the cursor is located, and
//! candidates are produced for that position: field names, flags and examples at the start of a
//! term; operators after a bare field name; values (from the [`SuggestContext`] and builtin
//! enumerations) after `field:`; header names after `header.`. Every suggestion carries the exact
//! byte range it replaces.

use crate::fields::{
    COLORS, COMMON_HEADERS, COMMON_MIME_TYPES, COMMON_STATUSES, ERROR_KINDS, FIELDS, FLAGS,
    FieldSpec, FieldType, KIND_ALIASES, METHODS, PROVIDERS, SCHEMES, STATUS_CLASSES, Smart,
    TYPE_FLAG_ALIASES, Unit, VERSION_ALIASES, edit_distance, field_by_name, known_header_values,
};
use crate::matcher::{find_ci, starts_with_ci};
use crate::scan::{AtomKind, FieldPart, RawTerm, Scanner, Span, Tok};
use crate::{SuggestContext, Suggestion};
use akev_core::canonical_reason;
use smallvec::SmallVec;
use std::borrow::Cow;
use std::collections::HashSet;

/// Maximum number of suggestions returned.
const MAX_SUGGESTIONS: usize = 30;

/// How many content kinds are listed before the MIME types seen in traffic.
const TOP_KINDS: usize = 9;

/// Candidate groups, in display order within a relevance tier.
const G_EXAMPLE: u8 = 0;
const G_OPERATOR: u8 = 1;
const G_FIELD: u8 = 2;
const G_FLAG: u8 = 3;
const G_VALUE: u8 = 4;
const G_KEYWORD: u8 = 5;
const G_SHORTCUT: u8 = 6;

type Text = Cow<'static, str>;

struct Cand {
    tier: u8,
    group: u8,
    order: usize,
    label: Text,
    detail: Text,
    insert: Text,
    range: Span,
    kind: &'static str,
}

/// Candidate collector. Texts stay borrowed until the final (at most 30) suggestions are built.
#[derive(Default)]
struct Out {
    cands: Vec<Cand>,
}

impl Out {
    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        tier: u8,
        group: u8,
        label: impl Into<Text>,
        detail: impl Into<Text>,
        insert: impl Into<Text>,
        range: Span,
        kind: &'static str,
    ) {
        let order = self.cands.len();
        self.cands.push(Cand {
            tier,
            group,
            order,
            label: label.into(),
            detail: detail.into(),
            insert: insert.into(),
            range,
            kind,
        });
    }

    fn finish(mut self) -> Vec<Suggestion> {
        self.cands.sort_by_key(|c| (c.tier, c.group, c.order));
        let mut out: Vec<Suggestion> = Vec::with_capacity(MAX_SUGGESTIONS.min(self.cands.len()));
        for c in self.cands {
            if out.len() == MAX_SUGGESTIONS {
                break;
            }
            let duplicate = out.iter().any(|s| {
                s.replace_start == c.range.start
                    && s.replace_end == c.range.end
                    && s.insert.eq_ignore_ascii_case(&c.insert)
            });
            if !duplicate {
                out.push(Suggestion {
                    label: c.label.into_owned(),
                    detail: c.detail.into_owned(),
                    insert: c.insert.into_owned(),
                    replace_start: c.range.start,
                    replace_end: c.range.end,
                    kind: c.kind,
                });
            }
        }
        out
    }
}

/// Relevance tier of `candidate` for the typed `prefix` (lower is better), or `None` if it
/// doesn't match at all: exact, prefix, word-boundary substring, substring, subsequence, typo.
/// Allocation-free for ASCII input (the common case).
fn rank(prefix: &str, candidate: &str) -> Option<u8> {
    rank_upto(prefix, candidate, 5)
}

/// [`rank`], skipping the (more expensive) fuzzy tiers when `max_tier` excludes them.
fn rank_upto(prefix: &str, candidate: &str, max_tier: u8) -> Option<u8> {
    if prefix.is_empty() {
        return Some(1);
    }
    // Numbers complete by prefix only: "5" means 5xx/500…, not 405.
    let max_tier = if prefix.bytes().all(|b| b.is_ascii_digit()) {
        max_tier.min(1)
    } else {
        max_tier
    };
    if !(prefix.is_ascii() && candidate.is_ascii()) {
        return rank_unicode(&prefix.to_lowercase(), &candidate.to_lowercase(), max_tier);
    }
    let p: SmallVec<[u8; 64]> = prefix.bytes().map(|b| b.to_ascii_lowercase()).collect();
    let c = candidate.as_bytes();
    if c.eq_ignore_ascii_case(&p) {
        return Some(0);
    }
    if starts_with_ci(c, &p) {
        return Some(1);
    }
    if let Some(i) = find_ci(c, &p) {
        let boundary = i > 0 && !c[i - 1].is_ascii_alphanumeric();
        return Some(if boundary { 2 } else { 3 }).filter(|t| *t <= max_tier);
    }
    if max_tier < 4 {
        return None;
    }
    fuzzy_tier(prefix, candidate)
}

fn rank_unicode(p: &str, c: &str, max_tier: u8) -> Option<u8> {
    if c == p {
        return Some(0);
    }
    if c.starts_with(p) {
        return Some(1);
    }
    if let Some(i) = c.find(p) {
        let boundary = c[..i]
            .chars()
            .next_back()
            .is_some_and(|ch| !ch.is_alphanumeric());
        return Some(if boundary { 2 } else { 3 }).filter(|t| *t <= max_tier);
    }
    if max_tier < 4 {
        return None;
    }
    fuzzy_tier(p, c)
}

/// Subsequence (tier 4) or small typo (tier 5) matches, for words only: "44" should not match
/// "404".
fn fuzzy_tier(p: &str, c: &str) -> Option<u8> {
    if p.bytes().any(|b| b.is_ascii_digit()) {
        return None;
    }
    // Long words are free text being typed, not misspelt names.
    let plen = p.chars().count();
    if !(2..=24).contains(&plen) {
        return None;
    }
    if is_subsequence(p, c) {
        return Some(4);
    }
    // Typos rarely change the first letter; requiring it keeps this cheap.
    let same_start = matches!((p.chars().next(), c.chars().next()), (Some(a), Some(b)) if a.eq_ignore_ascii_case(&b));
    if same_start && (4..=16).contains(&plen) {
        let max = if plen >= 6 { 2 } else { 1 };
        let head_end = c.char_indices().nth(plen).map_or(c.len(), |(i, _)| i);
        if edit_distance(p, &c[..head_end], max).is_some() || edit_distance(p, c, max).is_some() {
            return Some(5);
        }
    }
    None
}

fn is_subsequence(needle: &str, hay: &str) -> bool {
    let mut it = hay.chars().map(|c| c.to_ascii_lowercase());
    needle.chars().all(|n| {
        let n = n.to_ascii_lowercase();
        it.any(|h| h == n)
    })
}

/// Best tier over a name and its aliases. Inexact alias matches rank one tier below the same
/// match on the canonical name ("s" → `size:` before `is:pinned` via its alias "starred").
fn rank_names(prefix: &str, name: &str, aliases: &[&str]) -> Option<u8> {
    let canonical = rank(prefix, name);
    let alias = aliases
        .iter()
        .filter_map(|a| rank(prefix, a))
        .map(|t| if t == 0 { 0 } else { t + 1 })
        .min();
    match (canonical, alias) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// Quote a value when it contains characters that would end or change a bare word.
fn quote_value(v: &str, always: bool) -> String {
    let needs = always
        || v.is_empty()
        || v.chars()
            .any(|c| c.is_whitespace() || matches!(c, '(' | ')' | ',' | '|' | '"' | '\''));
    if !needs {
        return v.to_string();
    }
    let mut s = String::with_capacity(v.len() + 2);
    s.push('"');
    for c in v.chars() {
        if c == '"' || c == '\\' {
            s.push('\\');
        }
        s.push(c);
    }
    s.push('"');
    s
}

pub(crate) fn suggest(query: &str, cursor: usize, ctx: &SuggestContext) -> Vec<Suggestion> {
    let mut cursor = cursor.min(query.len());
    while !query.is_char_boundary(cursor) {
        cursor -= 1;
    }
    let tokens = Scanner::tokenize(query);
    let mut out = Out::default();
    let at = tokens
        .iter()
        .find(|t| t.span.start < cursor && cursor <= t.span.end);
    match at {
        Some(tok) => match &tok.tok {
            Tok::Term(term) => term_suggestions(query, cursor, term, ctx, &mut out),
            // A keyword the user may still be typing ("or" → "origin").
            Tok::Or | Tok::And | Tok::Not | Tok::OrCall
                if query
                    .as_bytes()
                    .get(tok.span.start)
                    .is_some_and(u8::is_ascii_alphabetic) =>
            {
                word_suggestions(query, cursor, tok.span, ctx, &mut out);
            }
            _ => term_start(query, cursor, ctx, &mut out),
        },
        None => term_start(query, cursor, ctx, &mut out),
    }
    out.finish()
}

/// Empty prefix at the start of a new term.
fn term_start(query: &str, cursor: usize, ctx: &SuggestContext, out: &mut Out) {
    let range = Span::new(cursor, cursor);
    if query.trim().is_empty() {
        if let Some(host) = ctx.hosts.first() {
            let ex = format!("host:{}", quote_value(host, false));
            out.push(
                1,
                G_EXAMPLE,
                ex.clone(),
                format!("Traffic to {host}"),
                ex,
                range,
                "example",
            );
        }
        for (ex, detail) in [
            ("status:4xx", "Client errors (400–499)"),
            ("is:error", "Failed exchanges"),
            ("method:post", "POST requests"),
            ("type:json", "JSON responses"),
            ("size:>1mb", "Responses larger than 1 MB"),
            ("duration:>1s", "Slower than one second"),
        ] {
            out.push(1, G_EXAMPLE, ex, detail, ex, range, "example");
        }
    }
    word_candidates("", range, ctx, out);
}

/// Completion for a bare word (a field name, flag or free text being typed).
fn word_suggestions(query: &str, cursor: usize, span: Span, ctx: &SuggestContext, out: &mut Out) {
    let word = &query[span.start..span.end];
    // `header.x-` / `header[x-` → header names.
    if let Some(sep) = word.find(['.', '[']) {
        let base = &word[..sep];
        if let Some(field) = field_by_name(base).filter(|f| f.subkey.is_some()) {
            let key_start = span.start + sep + 1;
            if cursor >= key_start {
                let bracket = word.as_bytes()[sep] == b'[';
                let prefix = &query[key_start..cursor];
                let suffix = if bracket { "]:" } else { ":" };
                subkey_candidates(
                    field,
                    prefix,
                    Span::new(key_start, span.end),
                    suffix,
                    ctx,
                    out,
                );
                return;
            }
        }
    }
    let prefix = &query[span.start..cursor];
    // A complete field name: offer its operators first — unless it is an alias the user is
    // probably still typing past ("err" → "error").
    if let Some(field) = field_by_name(prefix) {
        let still_typing = !field.name.eq_ignore_ascii_case(prefix)
            && field.name.len() > prefix.len()
            && field.name.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes());
        if !still_typing {
            operator_candidates(field, prefix, span, out);
        }
    }
    word_candidates(prefix, span, ctx, out);
}

/// Fields, `is:`/`has:`, flags, keywords and value shortcuts for a word prefix.
fn word_candidates(prefix: &str, range: Span, ctx: &SuggestContext, out: &mut Out) {
    for (i, field) in FIELDS.iter().enumerate() {
        // `is:` / `has:` go right after the three most used fields.
        if i == 3 {
            for (word, insert, detail) in [
                (
                    "is",
                    "is:",
                    "State flags: is:error, is:pending, is:pinned, …",
                ),
                (
                    "has",
                    "has:",
                    "Presence flags: has:body, has:note, has:cookie, …",
                ),
            ] {
                if let Some(tier) = rank(prefix, word) {
                    out.push(tier, G_FIELD, insert, detail, insert, range, "field");
                }
            }
        }
        if let Some(tier) = rank_names(prefix, field.name, field.aliases) {
            let insert = format!("{}:", field.name);
            out.push(
                tier,
                G_FIELD,
                insert.clone(),
                field.detail,
                insert,
                range,
                "field",
            );
            if let Some(what) = field.subkey {
                out.push(
                    tier,
                    G_FIELD,
                    format!(
                        "{}.<{}>:",
                        field.name,
                        what.split(' ').next_back().unwrap_or(what)
                    ),
                    format!("Match the value of one {what}"),
                    format!("{}.", field.name),
                    range,
                    "field",
                );
            }
        }
    }
    for flag in FLAGS {
        let tier = rank_names(prefix, flag.name, flag.aliases).or_else(|| rank(prefix, flag.full));
        if let Some(tier) = tier {
            out.push(
                tier,
                G_FLAG,
                flag.full,
                flag.description,
                flag.full,
                range,
                "flag",
            );
        }
    }
    if prefix.is_empty() {
        return;
    }
    for (kw, detail) in [
        ("OR", "Match either side: status:5xx OR is:error"),
        ("NOT", "Exclude matches (same as a leading -)"),
        ("AND", "Match both sides (same as a space)"),
    ] {
        if let Some(tier) = rank_upto(prefix, kw, 1) {
            out.push(tier, G_KEYWORD, kw, detail, kw, range, "operator");
        }
    }
    if prefix.chars().count() < 2 {
        return;
    }
    // Shortcuts: free text that looks like a known value becomes a field term.
    let mut shortcut = |tier: Option<u8>, field: &str, value: &str, detail: String| {
        if let Some(tier) = tier {
            let s = format!("{field}:{}", quote_value(value, false));
            out.push(tier, G_SHORTCUT, s.clone(), detail, s, range, "value");
        }
    };
    for host in &ctx.hosts {
        shortcut(rank_upto(prefix, host, 3), "host", host, "Host".to_string());
    }
    for (method, detail) in method_values(ctx) {
        shortcut(
            rank_upto(prefix, &method, 1),
            "method",
            &method.to_ascii_lowercase(),
            detail,
        );
    }
    for (alias, _, detail) in KIND_ALIASES.iter().filter(|(_, _, d)| !d.is_empty()) {
        shortcut(
            rank_upto(prefix, alias, 1),
            "type",
            alias,
            (*detail).to_string(),
        );
    }
    for source in &ctx.sources {
        shortcut(
            rank_upto(prefix, source, 3),
            "source",
            source,
            "Client".to_string(),
        );
    }
    if prefix.bytes().all(|b| b.is_ascii_digit()) {
        for (value, detail) in status_values() {
            shortcut(rank_upto(prefix, &value, 1), "status", &value, detail);
        }
    }
}

/// `status:`, `status>=`, … for a field name the user has typed.
fn operator_candidates(field: &FieldSpec, typed: &str, range: Span, out: &mut Out) {
    let ops: &[(&str, &str)] = match field.ty {
        FieldType::Number(unit) => match unit {
            Unit::Status => &[
                (":", "is — a code (404), class (4xx) or range (400-499)"),
                (">=", "at least"),
                ("<", "below"),
                (">", "above"),
                ("<=", "at most"),
                ("!=", "is not"),
            ],
            _ => &[
                (":", "equals, or a range like 10-20"),
                (">", "greater than"),
                ("<", "less than"),
                (">=", "at least"),
                ("<=", "at most"),
                ("!=", "is not"),
            ],
        },
        FieldType::Text(Smart::Exact) => &[
            (":", "is (whole value; * wildcards)"),
            ("!=", "is not"),
            ("^=", "starts with"),
            ("$=", "ends with"),
            ("*=", "contains"),
            ("~", "matches a regex"),
            ("=", "equals exactly"),
        ],
        FieldType::Text(Smart::Contains) => &[
            (":", "contains (* wildcards match the whole value)"),
            ("=", "equals exactly"),
            ("!=", "does not equal"),
            ("^=", "starts with"),
            ("$=", "ends with"),
            ("~", "matches a regex"),
            ("*=", "contains (literal)"),
        ],
    };
    for (op, detail) in ops {
        let s = format!("{typed}{op}");
        out.push(
            0,
            G_OPERATOR,
            s.clone(),
            format!("{} {detail}", field.name),
            s,
            range,
            "operator",
        );
    }
}

fn term_suggestions(
    query: &str,
    cursor: usize,
    term: &RawTerm,
    ctx: &SuggestContext,
    out: &mut Out,
) {
    let Some(fp) = &term.field else {
        if let Some(alt) = term.alts.first()
            && alt.atom.kind == AtomKind::Bare
        {
            word_suggestions(query, cursor, term.span, ctx, out);
        }
        return;
    };
    if cursor <= fp.name.end {
        let prefix = &query[fp.name.start..cursor];
        name_candidates(prefix, fp.name, out);
        return;
    }
    let name = &query[fp.name.start..fp.name.end];
    let lower = name.to_ascii_lowercase();
    let field = field_by_name(name);
    if let Some(key) = &fp.subkey
        && cursor <= key.span.end
    {
        if let Some(field) = field.filter(|f| f.subkey.is_some()) {
            let prefix = &query[key.span.start..cursor.max(key.span.start)];
            subkey_candidates(field, prefix, key.span, "", ctx, out);
        }
        return;
    }
    if cursor < fp.op_span.end {
        if let Some(field) = field {
            let typed = &query[fp.name.start..fp.op_span.start];
            operator_candidates(field, typed, Span::new(fp.name.start, fp.op_span.end), out);
        }
        return;
    }
    // Value position.
    let Some(alt) = term.alts.iter().find(|a| a.span.contains_cursor(cursor)) else {
        return;
    };
    let value_start = alt.cmp.map_or(alt.span.start, |(_, s)| s.end);
    if cursor < value_start {
        return;
    }
    let (prefix, range, always_quote) = match &alt.atom.kind {
        AtomKind::Regex { .. } | AtomKind::Paren { .. } => return,
        AtomKind::Quoted { .. } => {
            let start = (alt.atom.span.start + 1).min(cursor);
            (
                query[start..cursor].replace("\\\"", "\""),
                alt.atom.span,
                true,
            )
        }
        AtomKind::Bare => (
            query[value_start..cursor].to_string(),
            Span::new(value_start, alt.atom.span.end),
            false,
        ),
    };
    if lower == "is" || lower == "has" {
        flag_values(&lower, &prefix, range, out);
        return;
    }
    let Some(field) = field else {
        // Unknown field: offer corrections for the name instead.
        name_candidates(name, fp.name, out);
        return;
    };
    for (value, detail) in value_candidates(field, fp, alt.cmp.is_some(), &prefix, ctx) {
        if let Some(tier) = rank(&prefix, &value) {
            let insert = quote_value(&value, always_quote);
            out.push(tier, G_VALUE, value, detail, insert, range, "value");
        }
    }
}

/// Field-name completion for the name part of a `field:value` term.
fn name_candidates(prefix: &str, range: Span, out: &mut Out) {
    for word in ["is", "has"] {
        if let Some(tier) = rank(prefix, word) {
            out.push(tier, G_FIELD, word, "Flags", word, range, "field");
        }
    }
    for field in FIELDS {
        if let Some(tier) = rank_names(prefix, field.name, field.aliases) {
            out.push(
                tier,
                G_FIELD,
                field.name,
                field.detail,
                field.name,
                range,
                "field",
            );
        }
    }
}

fn flag_values(prefix_word: &str, prefix: &str, range: Span, out: &mut Out) {
    for flag in FLAGS {
        if let Some(tier) = rank_names(prefix, flag.name, flag.aliases) {
            let group = if flag.prefix == prefix_word {
                G_VALUE
            } else {
                G_VALUE + 1
            };
            out.push(
                tier,
                group,
                flag.name,
                flag.description,
                flag.name,
                range,
                "flag",
            );
        }
    }
}

fn header_names(ctx: &SuggestContext) -> Vec<String> {
    let mut seen = HashSet::new();
    ctx.header_names
        .iter()
        .map(|h| h.to_ascii_lowercase())
        .chain(COMMON_HEADERS.iter().map(|h| (*h).to_string()))
        .filter(|h| !h.is_empty() && seen.insert(h.clone()))
        .collect()
}

fn subkey_candidates(
    field: &FieldSpec,
    prefix: &str,
    range: Span,
    suffix: &str,
    ctx: &SuggestContext,
    out: &mut Out,
) {
    if !matches!(field.name, "header" | "reqheader" | "resheader") {
        return;
    }
    for name in header_names(ctx) {
        if let Some(tier) = rank(prefix, &name) {
            let insert = format!("{name}{suffix}");
            out.push(tier, G_VALUE, name, "Header", insert, range, "value");
        }
    }
}

fn method_values(ctx: &SuggestContext) -> Vec<(String, String)> {
    let mut seen = HashSet::new();
    let describe = |m: &str| {
        METHODS
            .iter()
            .find(|(n, _)| *n == m)
            .map_or("Seen in traffic", |(_, d)| *d)
            .to_string()
    };
    ctx.methods
        .iter()
        .map(|m| m.to_ascii_uppercase())
        .chain(METHODS.iter().map(|(m, _)| (*m).to_string()))
        .filter(|m| !m.is_empty() && seen.insert(m.clone()))
        .map(|m| {
            let d = describe(&m);
            (m, d)
        })
        .collect()
}

fn status_values() -> Vec<(String, String)> {
    status_classes().chain(status_codes()).collect()
}

fn status_classes() -> impl Iterator<Item = (String, String)> {
    STATUS_CLASSES
        .iter()
        .map(|(c, d)| ((*c).to_string(), (*d).to_string()))
}

fn status_codes() -> impl Iterator<Item = (String, String)> {
    COMMON_STATUSES
        .iter()
        .map(|code| (code.to_string(), canonical_reason(*code).to_string()))
}

fn unit_values(prefix: &str, units: &[(&str, &str)]) -> Vec<(String, String)> {
    let digits: String = prefix
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    if digits.is_empty() {
        return Vec::new();
    }
    units
        .iter()
        .map(|(u, d)| (format!("{digits}{u}"), (*d).to_string()))
        .collect()
}

fn owned(items: &[(&str, &str)]) -> Vec<(String, String)> {
    items
        .iter()
        .map(|(v, d)| ((*v).to_string(), (*d).to_string()))
        .collect()
}

/// Candidate values (unfiltered) for a field, given whether a comparison prefix was typed.
fn value_candidates(
    field: &FieldSpec,
    fp: &FieldPart,
    has_cmp: bool,
    prefix: &str,
    ctx: &SuggestContext,
) -> Vec<(String, String)> {
    let numeric_prefix = prefix
        .bytes()
        .next()
        .is_some_and(|b| b.is_ascii_digit() || b == b'.');
    let mut seen = HashSet::new();
    let mut v: Vec<(String, String)> = match field.name {
        "status" => {
            let mut v: Vec<(String, String)> = status_classes().collect();
            if !has_cmp && prefix.is_empty() {
                v.extend(owned(&[
                    (">=400", "All errors (4xx and 5xx)"),
                    ("400-499", "Inclusive range"),
                ]));
            }
            v.extend(status_codes());
            v
        }
        "size" | "reqsize" | "totalsize" => {
            if numeric_prefix {
                unit_values(
                    prefix,
                    &[
                        ("kb", "kilobytes"),
                        ("mb", "megabytes"),
                        ("b", "bytes"),
                        ("gb", "gigabytes"),
                    ],
                )
            } else if has_cmp {
                owned(&[
                    ("1kb", "1 KB"),
                    ("100kb", "100 KB"),
                    ("1mb", "1 MB"),
                    ("10mb", "10 MB"),
                ])
            } else {
                owned(&[
                    (">1mb", "Larger than 1 MB"),
                    (">100kb", "Larger than 100 KB"),
                    ("<1kb", "Smaller than 1 KB"),
                    ("0", "Empty"),
                    ("10kb-1mb", "Between 10 KB and 1 MB"),
                ])
            }
        }
        "duration" => {
            if numeric_prefix {
                unit_values(
                    prefix,
                    &[("ms", "milliseconds"), ("s", "seconds"), ("m", "minutes")],
                )
            } else if has_cmp {
                owned(&[
                    ("100ms", "100 ms"),
                    ("500ms", "500 ms"),
                    ("1s", "1 second"),
                    ("5s", "5 seconds"),
                ])
            } else {
                owned(&[
                    (">1s", "Slower than 1 second"),
                    (">500ms", "Slower than 500 ms"),
                    ("<100ms", "Faster than 100 ms"),
                    ("100ms-1s", "Between 100 ms and 1 s"),
                ])
            }
        }
        "port" => owned(&[
            ("443", "HTTPS"),
            ("80", "HTTP"),
            ("8080", "HTTP alternative"),
            ("8443", "HTTPS alternative"),
            ("3000", "Dev server"),
            ("5000", "Dev server"),
            ("8000", "Dev server"),
            ("9000", "Dev server"),
        ]),
        "method" => method_values(ctx),
        "host" => ctx
            .hosts
            .iter()
            .map(|h| (h.clone(), "Host".to_string()))
            .collect(),
        "scheme" => owned(SCHEMES),
        "version" => VERSION_ALIASES
            .iter()
            .filter(|(_, _, d)| !d.is_empty())
            .map(|(a, _, d)| ((*a).to_string(), (*d).to_string()))
            .collect(),
        "type" => {
            // The most common kinds, then MIME types seen in traffic, then the rest.
            let kinds: Vec<(String, String)> = KIND_ALIASES
                .iter()
                .filter(|(_, _, d)| !d.is_empty())
                .map(|(a, _, d)| ((*a).to_string(), (*d).to_string()))
                .collect();
            let (top, rest) = kinds.split_at(kinds.len().min(TOP_KINDS));
            let seen_mimes = ctx
                .content_types
                .iter()
                .map(|c| (c.clone(), "MIME type (seen in traffic)".to_string()));
            // HTTP Toolkit's state categories (`category=mutative`) select flags.
            let states = TYPE_FLAG_ALIASES.iter().map(|(name, id)| {
                let detail = FLAGS
                    .iter()
                    .find(|f| f.id == *id)
                    .map_or("", |f| f.description);
                ((*name).to_string(), detail.to_string())
            });
            top.iter()
                .cloned()
                .chain(seen_mimes)
                .chain(rest.iter().cloned())
                .chain(states)
                .chain(mime_values(ctx))
                .collect()
        }
        "header" | "reqheader" | "resheader" => match &fp.subkey {
            Some(key) if key.text.eq_ignore_ascii_case("content-type") => {
                mime_values(ctx).collect()
            }
            Some(key) => known_header_values(&key.text)
                .iter()
                .map(|v| ((*v).to_string(), String::new()))
                .collect(),
            None => header_names(ctx)
                .into_iter()
                .map(|h| (h, "Header".to_string()))
                .collect(),
        },
        "source" => ctx
            .sources
            .iter()
            .map(|s| (s.clone(), "Client".to_string()))
            .collect(),
        "color" => COLORS
            .iter()
            .map(|c| ((*c).to_string(), "Highlight colour".to_string()))
            .collect(),
        "error" => ERROR_KINDS
            .iter()
            .map(|(_, n, d)| ((*n).to_string(), (*d).to_string()))
            .collect(),
        "model" => ctx
            .models
            .iter()
            .map(|m| (m.clone(), "Model (seen in traffic)".to_string()))
            .collect(),
        "provider" => {
            let known = |id: &str| {
                PROVIDERS
                    .iter()
                    .find(|(p, _)| *p == id)
                    .map_or("LLM provider", |(_, d)| *d)
                    .to_string()
            };
            ctx.providers
                .iter()
                .map(|p| (p.clone(), format!("{} (seen in traffic)", known(p))))
                .chain(
                    PROVIDERS
                        .iter()
                        .map(|(p, d)| ((*p).to_string(), (*d).to_string())),
                )
                .collect()
        }
        _ => Vec::new(),
    };
    v.retain(|(value, _)| !value.is_empty() && seen.insert(value.to_ascii_lowercase()));
    v
}

fn mime_values(ctx: &SuggestContext) -> impl Iterator<Item = (String, String)> + '_ {
    ctx.content_types
        .iter()
        .map(|c| (c.clone(), "MIME type".to_string()))
        .chain(
            COMMON_MIME_TYPES
                .iter()
                .map(|c| ((*c).to_string(), "MIME type".to_string())),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranking() {
        assert_eq!(rank("status", "status"), Some(0));
        assert_eq!(rank("sta", "status"), Some(1));
        assert_eq!(rank("STA", "status"), Some(1));
        assert_eq!(rank("example", "api.example.com"), Some(2));
        assert_eq!(rank("xampl", "example.com"), Some(3));
        assert_eq!(rank("dur", "duration"), Some(1));
        assert_eq!(rank("drtn", "duration"), Some(4));
        assert_eq!(rank("stauts", "status"), Some(5));
        assert_eq!(rank("zzz", "status"), None);
        assert_eq!(rank("", "anything"), Some(1));
    }

    #[test]
    fn quoting() {
        assert_eq!(quote_value("example.com", false), "example.com");
        assert_eq!(quote_value("a b", false), "\"a b\"");
        assert_eq!(quote_value("a\"b", false), "\"a\\\"b\"");
        assert_eq!(quote_value("x", true), "\"x\"");
        assert_eq!(quote_value("", false), "\"\"");
    }
}
