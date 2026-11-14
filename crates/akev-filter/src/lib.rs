//! # akev-filter
//!
//! The traffic filter language used by the filter bar (and the CLI / MCP `query` tools).
//!
//! ## Syntax (case-insensitive field names)
//!
//! ```text
//! query      := or_expr
//! or_expr    := and_expr ( ("OR" | "|" | "||") and_expr )*
//! and_expr   := unary ( ["AND" | "&&"] unary )*          -- juxtaposition = AND
//! unary      := ("-" | "!" | "NOT ") unary | "(" query ")" | "or(" query ("," query)* ")"
//!             | term
//! term       := field op value | "is:" flag | "has:" thing | value      -- bare value = free text
//! op         := ":" (smart: contains / glob if value has * / regex if /.../)
//!             | "=" (exact) | "!=" | "^=" (starts) | "$=" (ends) | "*=" (contains)
//!             | "~" (regex) | ">" | ">=" | "<" | "<="
//! value      := bare-word | "double quoted" | 'single quoted' | /regex/flags
//!               values may be alternated with "," : method:get,post
//! ```
//!
//! Details:
//! * Precedence: `NOT` binds tightest, then `AND` (or juxtaposition), then `OR`:
//!   `a b | c` is `(a AND b) OR c`. `AND`/`OR`/`NOT` are case-insensitive keywords.
//! * Plain values match case-insensitively. For free-form text fields (host, path, url, query,
//!   header values, bodies, source, note, …) `:` means *contains*; for enumerated fields
//!   (method, scheme, version, type, color, header names, cookie names) `:` matches the whole
//!   value. A bare value containing `*` is a wildcard pattern over the whole field value
//!   (`host:*.example.com`, `path:/api/*`); quote it to search for a literal `*`.
//! * `/pattern/flags` is a regex (case-sensitive unless the `i` flag is given; `m`, `s`, `x`
//!   are supported). After `:` and in free text, a slash-delimited value is only treated as a
//!   regex when it uses regex syntax or has the `i` flag, so `/api/users` stays a path. After
//!   `~` the value is always a regex; unquoted/quoted patterns there are case-insensitive.
//!   Regex literals end at whitespace (use `\s`).
//! * Quoted strings support `\"`, `\'`, `\\`, `\n`, `\t`, `\r`; other backslashes are kept, so
//!   regex escapes like `"\d+"` work.
//! * Alternation: `method:get,post` means GET or POST. With `!=` it means "none of":
//!   `method!=get,head`. Per-alternative comparisons work after `:` (`size:<1kb,>1mb`).
//! * Negated terms match exchanges where the field is absent (`-note:x` matches exchanges
//!   without notes; `status!=200` matches pending exchanges). Comparisons on absent values
//!   (`status:>=400` on a pending exchange) never match.
//! * `word:value` with an unknown field is a parse error, except when it is clearly a URL
//!   fragment (`localhost:8080`, `http://…`, `api.example.com:443`), which is free text.
//! * HTTP Toolkit's filter strings (as its tags copy) work too: `hostname=x`, `header[x-trace]`,
//!   `header[x]=y`, `headers*=gzip`, `bodySize>1000`, `httpVersion=2`, `query^=?page`,
//!   `category=image` (and the state categories `mutative`, `websocket`, `incomplete`,
//!   `aborted`), `contains(any text)` (literal, up to the first `)`), `not(...)` and
//!   `or(a, b, …)`. Inside `or(…)` a comma separates filters; after another filter
//!   `x or(y, z)` is an `or(…)` call (AND-ed with `x`), while `x or(y)` is `x OR y`. Its bare
//!   keywords (`pending`, `errored`, …) stay free text here — write `is:pending`.
//!
//! ## Fields
//! `method`, `status` (supports `2xx`, `4xx`, `40x`, `400-499`, `>=500`), `host` (alias
//! `domain`), `port`, `path` (without the query), `url`, `query` (`query.page:2` matches one
//! parameter), `scheme`/`protocol`, `version` (`h1`,`h2`,`h3`,`1.1`), `type`/`content-type`
//! (matches ContentKind names like `json`, `image`, `data`, `media`, or mime substrings),
//! `size` (response size; units b/kb/mb/gb, e.g. `size:>1mb`), `reqsize`, `totalsize`,
//! `duration`/`time` (ms; units ms/s/m), `header` (`header:x-trace` → header present;
//! `header.x-trace:abc` or `header[x-trace]:abc` → value), `reqheader`, `resheader`,
//! `headers` (any header value or `Name: value` line), `cookie` (`cookie.session:abc`), `body` (request OR
//! response decoded body, substring), `reqbody`, `resbody`, `contains` (everything, bodies
//! included), `source` (process / interceptor), `rule` (rule id or title), `id`, `note`,
//! `color`, `error` (error kind), `ip` (client ip), `messages` (WebSocket/SSE message count),
//! `model` (model of an LLM API call), `provider` (LLM provider: `openai`, `anthropic`,
//! `google`, `ollama`, …).
//! See [`field_docs`] for descriptions and examples.
//!
//! ## Flags
//! `is:pending`, `is:complete`, `is:error`/`is:failed`, `is:aborted`, `is:paused`,
//! `is:pinned`, `is:mocked`, `is:modified`, `is:websocket`/`is:ws`, `is:sse`, `is:streaming`,
//! `is:tls-failure`, `is:tunnel`, `is:malformed`, `is:http`, `is:https`, `is:highlighted`,
//! `is:mutative`, `is:llm`; `has:note`, `has:body`, `has:reqbody`, `has:resbody`, `has:query`,
//! `has:cookie`, `has:auth`, `has:rule`, `has:response`, `has:messages`. `is:` and `has:` are
//! interchangeable, and flags alternate too: `is:error,aborted`.
//!
//! Free text (bare values) matches against the full URL (and method/status text), contains,
//! case-insensitive. Unknown fields are a parse error with a helpful message & span.
//!
//! ## Evaluation
//! Queries compile to a tree of precompiled matchers. `AND`/`OR` operands are reordered by cost
//! so cheap summary-only terms run first and body terms run last (and only when needed); bodies
//! and the full URL are computed at most once per exchange.

mod eval;
mod fields;
mod highlight;
mod matcher;
mod parse;
mod scan;
mod suggest;

#[cfg(test)]
mod tests;

pub use highlight::{HighlightToken, highlight};

use akev_core::{ExchangeSummary, Headers};
use serde::Serialize;
use std::sync::Arc;

/// Everything a filter may inspect. Implemented by the store over its records. Bodies are
/// accessed lazily (only evaluated when a body term is present and cheaper terms passed).
pub trait FilterTarget {
    /// The exchange's summary (all cheap fields live here).
    fn summary(&self) -> &ExchangeSummary;
    /// Request headers as received.
    fn request_headers(&self) -> &Headers;
    /// Response headers, once the response head has arrived.
    fn response_headers(&self) -> Option<&Headers>;
    /// Decoded request body as text (lossy UTF-8). `None` if absent/binary.
    fn request_body_text(&self) -> Option<std::borrow::Cow<'_, str>>;
    /// Decoded response body as text (lossy UTF-8). `None` if absent/binary.
    fn response_body_text(&self) -> Option<std::borrow::Cow<'_, str>>;
    /// Client IP address.
    fn client_ip(&self) -> Option<&str>;
    /// User note.
    fn note(&self) -> Option<&str>;
    /// Title of the rule that handled the exchange, if any.
    fn rule_title(&self) -> Option<&str>;
}

/// A query that failed to parse, with the offending byte range.
#[derive(Clone, Debug, PartialEq, Serialize, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[error("{message}")]
pub struct ParseError {
    /// Human readable explanation, e.g. "Unknown field 'stauts' — did you mean 'status'?".
    pub message: String,
    /// Byte range in the input that is wrong (start, inclusive).
    pub start: usize,
    /// End of the wrong range (exclusive).
    pub end: usize,
}

/// A compiled filter. Cheap to clone (Arc inside), Send + Sync.
#[derive(Clone, Debug)]
pub struct Filter {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    source: String,
    root: eval::Node,
    needs_body: bool,
}

impl Filter {
    /// Parse and compile. Empty/whitespace input → filter that matches everything.
    pub fn parse(query: &str) -> Result<Filter, ParseError> {
        let root = parse::parse(query)?.optimize();
        let needs_body = root.needs_body();
        Ok(Filter {
            inner: Arc::new(Inner {
                source: query.to_string(),
                root,
                needs_body,
            }),
        })
    }

    /// Matches everything.
    pub fn all() -> Filter {
        Filter {
            inner: Arc::new(Inner {
                source: String::new(),
                root: eval::Node::True,
                needs_body: false,
            }),
        }
    }

    /// True if this filter matches every exchange without inspecting it (empty query, or one
    /// that simplifies to "everything", such as `""`).
    pub fn is_match_all(&self) -> bool {
        matches!(self.inner.root, eval::Node::True)
    }

    /// Evaluate the filter against one exchange.
    pub fn matches(&self, target: &dyn FilterTarget) -> bool {
        match &self.inner.root {
            eval::Node::True => true,
            eval::Node::False => false,
            root => root.eval(&mut eval::Cx::new(target)),
        }
    }

    /// True if evaluating may require body access.
    pub fn needs_body(&self) -> bool {
        self.inner.needs_body
    }

    /// The original query string.
    pub fn source(&self) -> &str {
        &self.inner.source
    }

    /// The optimized evaluation tree as an S-expression (tests / debugging).
    #[cfg(test)]
    pub(crate) fn explain(&self) -> String {
        self.inner.root.explain()
    }
}

impl Default for Filter {
    fn default() -> Self {
        Filter::all()
    }
}

/// Values known to the store, used to power value completion.
#[derive(Clone, Debug, Default)]
pub struct SuggestContext {
    /// Hostnames seen in traffic (suggested in the given order, e.g. most frequent first).
    pub hosts: Vec<String>,
    /// Request methods seen in traffic (listed before the standard methods).
    pub methods: Vec<String>,
    /// Client sources (process / interceptor names).
    pub sources: Vec<String>,
    /// Response content types (MIME essences) seen in traffic.
    pub content_types: Vec<String>,
    /// Header names seen in traffic (any casing; suggested lowercased).
    pub header_names: Vec<String>,
    /// LLM models seen in traffic (`model:` suggestions).
    pub models: Vec<String>,
    /// LLM provider ids seen in traffic (suggested before the other known providers).
    pub providers: Vec<String>,
}

/// One autocompletion entry.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    /// What the dropdown shows, e.g. `status:` or `example.com`.
    pub label: String,
    /// Short description, e.g. "Response status code (e.g. 404, 4xx, >=500)".
    pub detail: String,
    /// Replacement text for the range `[replace_start, replace_end)`.
    pub insert: String,
    /// Byte offset where the replaced range starts.
    pub replace_start: usize,
    /// Byte offset where the replaced range ends (exclusive).
    pub replace_end: usize,
    /// `field`, `operator`, `value`, `flag`, `example`.
    pub kind: &'static str,
}

/// Autocomplete at byte offset `cursor` (field names, operators, flags, and known values).
/// Results sorted by relevance, at most 30.
///
/// `cursor` is clamped to the query length and to a character boundary. Replace ranges are
/// exact byte offsets into `query`, so `query[..replace_start] + insert + query[replace_end..]`
/// is the completed query.
pub fn suggest(query: &str, cursor: usize, ctx: &SuggestContext) -> Vec<Suggestion> {
    suggest::suggest(query, cursor, ctx)
}

/// Documentation of every field/flag for the help popover.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldDoc {
    /// Canonical field name (`status`) or flag (`is:pending`).
    pub name: &'static str,
    /// Alternative names accepted by the parser.
    pub aliases: &'static [&'static str],
    /// What the field matches and how.
    pub description: &'static str,
    /// 2–4 example queries (each one parses).
    pub examples: &'static [&'static str],
}

/// Every field (by canonical name, e.g. `status`) followed by every flag (as `is:pending`,
/// `has:body`, …), with aliases, a description and 2–4 examples each.
pub fn field_docs() -> Vec<FieldDoc> {
    let fields = fields::FIELDS.iter().map(|f| FieldDoc {
        name: f.name,
        aliases: f.aliases,
        description: f.description,
        examples: f.examples,
    });
    let flags = fields::FLAGS.iter().map(|f| FieldDoc {
        name: f.full,
        aliases: f.aliases,
        description: f.description,
        examples: f.examples,
    });
    fields.chain(flags).collect()
}
