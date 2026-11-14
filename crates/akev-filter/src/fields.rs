//! The registry of fields and flags understood by the filter language, plus the builtin value
//! enumerations used for validation, autocompletion and documentation.

use akev_core::{ContentKind, ErrorKind, HttpVersion};

/// Identifies a filterable field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum FieldId {
    Method,
    Status,
    Host,
    Path,
    Url,
    Query,
    Type,
    Size,
    Duration,
    Header,
    ReqHeader,
    ResHeader,
    Headers,
    Cookie,
    Body,
    ReqBody,
    ResBody,
    Contains,
    Port,
    Scheme,
    Version,
    ReqSize,
    TotalSize,
    Source,
    Rule,
    Id,
    Note,
    Color,
    Error,
    Ip,
    Messages,
    Model,
    Provider,
}

/// Units accepted by numeric fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Unit {
    /// A plain number (port, id, message count).
    Plain,
    /// Status codes: plain numbers plus classes (`4xx`, `40x`).
    Status,
    /// Bytes: `b`, `kb`, `mb`, `gb` (1024-based).
    Size,
    /// Milliseconds: `ms`, `s`, `m`, `h`.
    Duration,
}

/// What `:` means for a text field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Smart {
    /// Substring match (free-form text such as hosts and paths).
    Contains,
    /// Whole-value match (enumerations such as methods and colours).
    Exact,
}

/// The value type of a field, which decides the operators it accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FieldType {
    Text(Smart),
    Number(Unit),
}

/// Static description of a field.
#[derive(Debug)]
pub(crate) struct FieldSpec {
    pub id: FieldId,
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub ty: FieldType,
    /// What a `.subkey` names (e.g. "header name"), or `None` if the field takes no sub-key.
    pub subkey: Option<&'static str>,
    /// Short description for the suggestion dropdown.
    pub detail: &'static str,
    /// Longer description for the help popover.
    pub description: &'static str,
    pub examples: &'static [&'static str],
}

impl FieldSpec {
    /// Whether `name` (already lowercased) is this field's name or one of its aliases.
    pub(crate) fn is_named(&self, lower: &str) -> bool {
        self.name == lower || self.aliases.contains(&lower)
    }
}

const TEXT: FieldType = FieldType::Text(Smart::Contains);
const ENUM: FieldType = FieldType::Text(Smart::Exact);

/// Every field, ordered by how commonly it is used (this is also the suggestion order).
pub(crate) static FIELDS: &[FieldSpec] = &[
    FieldSpec {
        id: FieldId::Method,
        name: "method",
        aliases: &["verb"],
        ty: ENUM,
        subkey: None,
        detail: "Request method (GET, POST, …)",
        description: "HTTP request method. `:` matches the whole method, case-insensitively.",
        examples: &["method:post", "method:get,head", "-method:options"],
    },
    FieldSpec {
        id: FieldId::Status,
        name: "status",
        aliases: &["code", "statuscode"],
        ty: FieldType::Number(Unit::Status),
        subkey: None,
        detail: "Response status (404, 4xx, 400-499, >=500)",
        description: "Response status code: an exact code (404), a class (4xx, 50x), an inclusive \
                      range (400-499) or a comparison (>=500). Pending exchanges have no status.",
        examples: &["status:404", "status:4xx", "status:>=500", "status:200,3xx"],
    },
    FieldSpec {
        id: FieldId::Host,
        name: "host",
        aliases: &["domain", "hostname"],
        ty: TEXT,
        subkey: None,
        detail: "Request hostname (contains; * wildcards)",
        description: "Hostname the request was sent to (without the port). `:` matches a \
                      substring; a value with `*` is a wildcard pattern over the whole hostname.",
        examples: &[
            "host:example.com",
            "host:*.example.com",
            "-host:localhost",
            "host=api.example.com",
        ],
    },
    FieldSpec {
        id: FieldId::Path,
        name: "path",
        aliases: &["pathname"],
        ty: TEXT,
        subkey: None,
        detail: "URL path, without the query string",
        description: "URL path without the query string. `:` matches a substring; a value with \
                      `*` is a wildcard pattern over the whole path.",
        examples: &["path:/api", "path:/api/*", "path:*.js", "path~^/v[12]/"],
    },
    FieldSpec {
        id: FieldId::Url,
        name: "url",
        aliases: &["uri"],
        ty: TEXT,
        subkey: None,
        detail: "Full URL (scheme://host:port/path?query)",
        description: "The full URL, `scheme://host[:port]/path?query` (the port is shown only \
                      when it isn't the scheme's default).",
        examples: &[
            "url:example.com/api",
            "url^=https://api.",
            "url:/v\\d+\\/users/",
        ],
    },
    FieldSpec {
        id: FieldId::Query,
        name: "query",
        aliases: &["querystring", "params", "param"],
        ty: TEXT,
        subkey: Some("query parameter"),
        detail: "Query string; query.<name> matches one parameter",
        description: "The query string without the leading `?`. `query.<name>` matches the \
                      (URL-decoded) value of a single query parameter.",
        examples: &[
            "query:token=",
            "query.page:2",
            "query.q:\"hello world\"",
            "-has:query",
        ],
    },
    FieldSpec {
        id: FieldId::Type,
        name: "type",
        aliases: &["content-type", "contenttype", "mime", "category", "kind"],
        ty: TEXT,
        subkey: None,
        detail: "Response content kind (json, image, …) or MIME type",
        description: "Response content: a content kind (json, html, js, css, image, font, xml, \
                      data, media, …) or a substring of the response MIME type. HTTP Toolkit's \
                      state categories also work: mutative, websocket, incomplete, aborted.",
        examples: &[
            "type:json",
            "type:image,font",
            "type:application/grpc",
            "-type:js,css",
        ],
    },
    FieldSpec {
        id: FieldId::Size,
        name: "size",
        aliases: &["ressize", "responsesize"],
        ty: FieldType::Number(Unit::Size),
        subkey: None,
        detail: "Response body size (b, kb, mb, gb)",
        description: "Response body size on the wire, in bytes unless a unit is given: b, kb, \
                      mb, gb (1024-based). Supports comparisons and ranges.",
        examples: &["size:>1mb", "size:<1kb", "size:0", "size:10kb-1mb"],
    },
    FieldSpec {
        id: FieldId::Duration,
        name: "duration",
        aliases: &["time", "took", "latency", "elapsed"],
        ty: FieldType::Number(Unit::Duration),
        subkey: None,
        detail: "Total duration (ms by default; ms, s, m)",
        description: "Total exchange duration, in milliseconds unless a unit is given: ms, s, \
                      m, h. Unfinished exchanges have no duration.",
        examples: &["duration:>1s", "duration:<100ms", "time:500-2000"],
    },
    FieldSpec {
        id: FieldId::Header,
        name: "header",
        aliases: &["hdr"],
        ty: ENUM,
        subkey: Some("header name"),
        detail: "Header present; header.<name>:value matches its value",
        description: "`header:<name>` matches exchanges with that request or response header \
                      (wildcards allowed). `header.<name>:<value>` (or `header[<name>]:<value>`) \
                      matches the header's value.",
        examples: &[
            "header:x-request-id",
            "header.content-type:json",
            "header[x-cache]=hit",
            "-header:authorization",
        ],
    },
    FieldSpec {
        id: FieldId::ReqHeader,
        name: "reqheader",
        aliases: &["requestheader"],
        ty: ENUM,
        subkey: Some("header name"),
        detail: "Request header present / reqheader.<name>:value",
        description: "Like `header`, but only looks at request headers.",
        examples: &[
            "reqheader:authorization",
            "reqheader.user-agent:curl",
            "-reqheader:cookie",
        ],
    },
    FieldSpec {
        id: FieldId::ResHeader,
        name: "resheader",
        aliases: &["responseheader"],
        ty: ENUM,
        subkey: Some("header name"),
        detail: "Response header present / resheader.<name>:value",
        description: "Like `header`, but only looks at response headers.",
        examples: &[
            "resheader:set-cookie",
            "resheader.cache-control:no-store",
            "resheader.server:nginx*",
        ],
    },
    FieldSpec {
        id: FieldId::Headers,
        name: "headers",
        aliases: &["allheaders"],
        ty: TEXT,
        subkey: None,
        detail: "Any header value or \"Name: value\" line",
        description: "Searches every request and response header: each value, and each whole \
                      `Name: value` line.",
        examples: &[
            "headers:gzip",
            "headers:\"x-cache: hit\"",
            "headers~/^x-amz-/",
        ],
    },
    FieldSpec {
        id: FieldId::Cookie,
        name: "cookie",
        aliases: &["cookies"],
        ty: ENUM,
        subkey: Some("cookie name"),
        detail: "Cookie present; cookie.<name>:value matches its value",
        description: "`cookie:<name>` matches exchanges sending (Cookie) or setting \
                      (Set-Cookie) that cookie. `cookie.<name>:<value>` matches its value.",
        examples: &["cookie:session", "cookie.session:abc*", "has:cookie"],
    },
    FieldSpec {
        id: FieldId::Body,
        name: "body",
        aliases: &[],
        ty: TEXT,
        subkey: None,
        detail: "Request or response body text (slower)",
        description: "Decoded request or response body text. Body terms are evaluated last, \
                      only for exchanges that pass every cheaper term.",
        examples: &[
            "body:error",
            "body:\"user not found\"",
            "body:/\"id\":\\s*\\d+/",
            "-body:debug",
        ],
    },
    FieldSpec {
        id: FieldId::ReqBody,
        name: "reqbody",
        aliases: &["requestbody"],
        ty: TEXT,
        subkey: None,
        detail: "Request body text (slower)",
        description: "Decoded request body text.",
        examples: &[
            "reqbody:password",
            "reqbody:\"query {\"",
            "method:post reqbody~^\\{",
        ],
    },
    FieldSpec {
        id: FieldId::ResBody,
        name: "resbody",
        aliases: &["responsebody"],
        ty: TEXT,
        subkey: None,
        detail: "Response body text (slower)",
        description: "Decoded response body text.",
        examples: &[
            "resbody:\"\\\"ok\\\":false\"",
            "resbody:exception",
            "type:html resbody:<form",
        ],
    },
    FieldSpec {
        id: FieldId::Contains,
        name: "contains",
        aliases: &["any", "anywhere", "everywhere"],
        ty: TEXT,
        subkey: None,
        detail: "Anywhere: URL, method, status, headers, bodies",
        description: "Searches everything: URL, method, status, header lines and bodies \
                      (slower than plain text, which only searches the URL, method and status). \
                      `contains(any text)` searches for the literal text up to the first `)`.",
        examples: &[
            "contains:token",
            "contains:\"Bad Request\"",
            "contains(content-type: application/json)",
            "any:/[0-9a-f]{32}/",
        ],
    },
    FieldSpec {
        id: FieldId::Port,
        name: "port",
        aliases: &[],
        ty: FieldType::Number(Unit::Plain),
        subkey: None,
        detail: "Destination port",
        description: "Destination port number.",
        examples: &["port:8080", "port:>1024", "-port:443"],
    },
    FieldSpec {
        id: FieldId::Scheme,
        name: "scheme",
        aliases: &["protocol"],
        ty: ENUM,
        subkey: None,
        detail: "URL scheme (http, https, ws, wss)",
        description: "URL scheme: http, https, ws or wss.",
        examples: &["scheme:https", "scheme:ws,wss", "-scheme:https"],
    },
    FieldSpec {
        id: FieldId::Version,
        name: "version",
        aliases: &["httpversion", "http-version"],
        ty: ENUM,
        subkey: None,
        detail: "HTTP version (h1, h2, h3, 1.0, 1.1)",
        description: "HTTP protocol version: h1 (HTTP/1.x), 1.0, 1.1, h2 or h3.",
        examples: &["version:h2", "version:1.1", "-version:h1"],
    },
    FieldSpec {
        id: FieldId::ReqSize,
        name: "reqsize",
        aliases: &["requestsize"],
        ty: FieldType::Number(Unit::Size),
        subkey: None,
        detail: "Request body size (b, kb, mb, gb)",
        description: "Request body size on the wire (units b, kb, mb, gb; 1024-based).",
        examples: &["reqsize:>10kb", "reqsize:0", "method:post reqsize:>1mb"],
    },
    FieldSpec {
        id: FieldId::TotalSize,
        name: "totalsize",
        aliases: &["bodysize"],
        ty: FieldType::Number(Unit::Size),
        subkey: None,
        detail: "Request + response body size",
        description: "Request and response body sizes added together (units b, kb, mb, gb).",
        examples: &["totalsize:>5mb", "bodysize:>1000", "totalsize:<100b"],
    },
    FieldSpec {
        id: FieldId::Source,
        name: "source",
        aliases: &["process", "app", "client"],
        ty: TEXT,
        subkey: None,
        detail: "Client process or interceptor",
        description: "The client that sent the request: process name or interceptor.",
        examples: &["source:chrome", "source:curl,python", "-source:firefox"],
    },
    FieldSpec {
        id: FieldId::Rule,
        name: "rule",
        aliases: &[],
        ty: TEXT,
        subkey: None,
        detail: "Rule that handled the exchange (id or title)",
        description: "The rule that handled the exchange, matched against its id and title.",
        examples: &["rule:\"Mock users\"", "rule:mock*", "has:rule"],
    },
    FieldSpec {
        id: FieldId::Id,
        name: "id",
        aliases: &[],
        ty: FieldType::Number(Unit::Plain),
        subkey: None,
        detail: "Exchange id",
        description: "Exchange id (ids increase in capture order).",
        examples: &["id:42", "id:>1000", "id:10-20"],
    },
    FieldSpec {
        id: FieldId::Note,
        name: "note",
        aliases: &["comment"],
        ty: TEXT,
        subkey: None,
        detail: "User note",
        description: "The note attached to the exchange.",
        examples: &["note:bug", "note:\"look at this\"", "has:note"],
    },
    FieldSpec {
        id: FieldId::Color,
        name: "color",
        aliases: &["colour", "highlight", "tag"],
        ty: ENUM,
        subkey: None,
        detail: "Highlight colour",
        description: "Highlight colour: red, orange, yellow, green, blue, purple or grey.",
        examples: &["color:red", "color:red,orange", "is:highlighted"],
    },
    FieldSpec {
        id: FieldId::Error,
        name: "error",
        aliases: &["err", "failure"],
        ty: TEXT,
        subkey: None,
        detail: "Failure kind (timeout, dns-failed, …)",
        description: "Why the exchange failed (substring of the error kind, e.g. timeout, dns, \
                      tls, connection-refused).",
        examples: &["error:timeout", "error:dns", "error:upstream-cert-invalid"],
    },
    FieldSpec {
        id: FieldId::Ip,
        name: "ip",
        aliases: &["clientip", "client-ip", "remoteip"],
        ty: TEXT,
        subkey: None,
        detail: "Client IP address",
        description: "IP address of the client that sent the request.",
        examples: &["ip:127.0.0.1", "ip:192.168.*", "-ip:127.0.0.1"],
    },
    FieldSpec {
        id: FieldId::Messages,
        name: "messages",
        aliases: &["msgs", "messagecount"],
        ty: FieldType::Number(Unit::Plain),
        subkey: None,
        detail: "WebSocket / SSE message count",
        description: "Number of WebSocket or server-sent-event messages.",
        examples: &["messages:>100", "is:websocket messages:0"],
    },
    FieldSpec {
        id: FieldId::Model,
        name: "model",
        aliases: &["llm-model", "llmmodel"],
        ty: TEXT,
        subkey: None,
        detail: "LLM model of an AI API call",
        description: "The model an LLM API call asked for (OpenAI, Anthropic, Gemini, Ollama, \
                      OpenAI-compatible servers). `:` matches a substring; a value with `*` is a \
                      wildcard pattern over the whole name.",
        examples: &["model:gpt-4.1", "model:claude*", "is:llm -model:mini"],
    },
    FieldSpec {
        id: FieldId::Provider,
        name: "provider",
        aliases: &["vendor", "llm-provider"],
        ty: ENUM,
        subkey: None,
        detail: "LLM provider (openai, anthropic, google, ollama, …)",
        description: "Provider of an LLM API call: openai, anthropic, google, vertex-ai, \
                      azure-openai, openrouter, groq, mistral, deepseek, xai, ollama, lmstudio, \
                      openai-compatible…",
        examples: &[
            "provider:anthropic",
            "provider:openai,azure-openai",
            "provider:ollama",
        ],
    },
];

/// Find a field by (case-insensitive) name or alias.
pub(crate) fn field_by_name(name: &str) -> Option<&'static FieldSpec> {
    let lower = name.to_ascii_lowercase();
    FIELDS.iter().find(|f| f.is_named(&lower))
}

/// Identifies a boolean flag (`is:xxx` / `has:xxx`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum FlagId {
    Pending,
    Complete,
    Error,
    Aborted,
    Paused,
    Pinned,
    Mocked,
    Modified,
    WebSocket,
    Sse,
    Streaming,
    TlsFailure,
    Tunnel,
    Malformed,
    Http,
    Https,
    Highlighted,
    Mutative,
    Note,
    Body,
    ReqBody,
    ResBody,
    Query,
    Cookie,
    Auth,
    Rule,
    Response,
    Messages,
    Llm,
}

/// Static description of a flag.
#[derive(Debug)]
pub(crate) struct FlagSpec {
    pub id: FlagId,
    /// `is` or `has` — the prefix used in suggestions and docs (both prefixes are accepted).
    pub prefix: &'static str,
    pub name: &'static str,
    /// `prefix:name`, e.g. `is:pending`.
    pub full: &'static str,
    pub aliases: &'static [&'static str],
    pub description: &'static str,
    pub examples: &'static [&'static str],
}

impl FlagSpec {
    pub(crate) fn is_named(&self, lower: &str) -> bool {
        self.name == lower || self.aliases.contains(&lower)
    }
}

/// Every flag, in suggestion order.
pub(crate) static FLAGS: &[FlagSpec] = &[
    FlagSpec {
        id: FlagId::Error,
        prefix: "is",
        name: "error",
        full: "is:error",
        aliases: &["failed", "errored", "failure", "errors"],
        description: "Failed: connection/upstream errors, TLS failures, malformed requests",
        examples: &["is:error", "is:error host:example.com", "-is:error"],
    },
    FlagSpec {
        id: FlagId::Pending,
        prefix: "is",
        name: "pending",
        full: "is:pending",
        aliases: &["incomplete", "active", "inflight", "in-flight", "open"],
        description: "Not finished yet (in flight, streaming or paused)",
        examples: &["is:pending", "is:pending duration:>5s", "-is:pending"],
    },
    FlagSpec {
        id: FlagId::Complete,
        prefix: "is",
        name: "complete",
        full: "is:complete",
        aliases: &["completed", "done", "finished"],
        description: "Finished successfully",
        examples: &["is:complete", "is:complete status:5xx"],
    },
    FlagSpec {
        id: FlagId::Aborted,
        prefix: "is",
        name: "aborted",
        full: "is:aborted",
        aliases: &["cancelled", "canceled", "abort"],
        description: "Aborted before completing (including TLS handshake failures)",
        examples: &["is:aborted", "is:aborted -is:tls-failure"],
    },
    FlagSpec {
        id: FlagId::Paused,
        prefix: "is",
        name: "paused",
        full: "is:paused",
        aliases: &["breakpoint", "breakpointed", "intercepted"],
        description: "Paused at a breakpoint",
        examples: &["is:paused", "is:paused method:post"],
    },
    FlagSpec {
        id: FlagId::Pinned,
        prefix: "is",
        name: "pinned",
        full: "is:pinned",
        aliases: &["pin", "starred"],
        description: "Pinned (kept when clearing)",
        examples: &["is:pinned", "-is:pinned"],
    },
    FlagSpec {
        id: FlagId::Mocked,
        prefix: "is",
        name: "mocked",
        full: "is:mocked",
        aliases: &["mock", "stubbed", "fake"],
        description: "Response generated by a rule (mock / map local) instead of upstream",
        examples: &["is:mocked", "-is:mocked rule:api"],
    },
    FlagSpec {
        id: FlagId::Modified,
        prefix: "is",
        name: "modified",
        full: "is:modified",
        aliases: &["edited", "rewritten", "changed", "transformed"],
        description: "Request or response modified by a rule or breakpoint",
        examples: &["is:modified", "is:modified -is:mocked"],
    },
    FlagSpec {
        id: FlagId::WebSocket,
        prefix: "is",
        name: "websocket",
        full: "is:websocket",
        aliases: &["ws", "websockets"],
        description: "WebSocket connection",
        examples: &["is:websocket", "is:ws messages:>10"],
    },
    FlagSpec {
        id: FlagId::Llm,
        prefix: "is",
        name: "llm",
        full: "is:llm",
        aliases: &["ai", "genai", "llm-call"],
        description: "Call to an LLM API (OpenAI, Anthropic, Gemini, Ollama, OpenAI-compatible)",
        examples: &["is:llm", "is:llm status:>=400", "is:llm duration:>10s"],
    },
    FlagSpec {
        id: FlagId::Sse,
        prefix: "is",
        name: "sse",
        full: "is:sse",
        aliases: &["event-stream", "eventstream", "eventsource"],
        description: "Server-sent events stream",
        examples: &["is:sse", "is:sse is:streaming"],
    },
    FlagSpec {
        id: FlagId::Streaming,
        prefix: "is",
        name: "streaming",
        full: "is:streaming",
        aliases: &["stream", "live"],
        description: "WebSocket or SSE stream that is still open",
        examples: &["is:streaming", "is:websocket -is:streaming"],
    },
    FlagSpec {
        id: FlagId::TlsFailure,
        prefix: "is",
        name: "tls-failure",
        full: "is:tls-failure",
        aliases: &["tls-error", "tls-failed", "tlsfailure", "handshake-failed"],
        description: "TLS handshake rejected by the client (untrusted CA or certificate pinning)",
        examples: &["is:tls-failure", "is:tls-failure host:*.apple.com"],
    },
    FlagSpec {
        id: FlagId::Tunnel,
        prefix: "is",
        name: "tunnel",
        full: "is:tunnel",
        aliases: &["tunnelled", "tunneled", "passthrough"],
        description: "Connection tunnelled without interception",
        examples: &["is:tunnel", "-is:tunnel"],
    },
    FlagSpec {
        id: FlagId::Malformed,
        prefix: "is",
        name: "malformed",
        full: "is:malformed",
        aliases: &["client-error", "invalid", "unparseable"],
        description: "The client sent something that isn't valid HTTP",
        examples: &["is:malformed", "is:malformed ip:192.168.*"],
    },
    FlagSpec {
        id: FlagId::Http,
        prefix: "is",
        name: "http",
        full: "is:http",
        aliases: &["plaintext", "insecure", "unencrypted"],
        description: "Sent over plain-text HTTP (http:// or ws://)",
        examples: &["is:http", "is:http -host:localhost"],
    },
    FlagSpec {
        id: FlagId::Https,
        prefix: "is",
        name: "https",
        full: "is:https",
        aliases: &["secure", "encrypted", "tls"],
        description: "Sent over TLS (https:// or wss://)",
        examples: &["is:https", "-is:https"],
    },
    FlagSpec {
        id: FlagId::Highlighted,
        prefix: "is",
        name: "highlighted",
        full: "is:highlighted",
        aliases: &["colored", "coloured", "color", "colour", "tagged", "marked"],
        description: "Has a highlight colour",
        examples: &["is:highlighted", "-is:highlighted"],
    },
    FlagSpec {
        id: FlagId::Mutative,
        prefix: "is",
        name: "mutative",
        full: "is:mutative",
        aliases: &["mutating", "write", "unsafe"],
        description: "POST, PUT, PATCH or DELETE request",
        examples: &["is:mutative", "is:mutative status:2xx"],
    },
    FlagSpec {
        id: FlagId::Note,
        prefix: "has",
        name: "note",
        full: "has:note",
        aliases: &["notes", "noted", "comment"],
        description: "Has a user note",
        examples: &["has:note", "has:note is:pinned"],
    },
    FlagSpec {
        id: FlagId::Body,
        prefix: "has",
        name: "body",
        full: "has:body",
        aliases: &["bodies", "payload"],
        description: "Request or response has a body",
        examples: &["has:body", "-has:body"],
    },
    FlagSpec {
        id: FlagId::ReqBody,
        prefix: "has",
        name: "reqbody",
        full: "has:reqbody",
        aliases: &["requestbody", "req.body"],
        description: "Request has a body",
        examples: &["has:reqbody", "method:get has:reqbody"],
    },
    FlagSpec {
        id: FlagId::ResBody,
        prefix: "has",
        name: "resbody",
        full: "has:resbody",
        aliases: &["responsebody", "res.body"],
        description: "Response has a body",
        examples: &["has:resbody", "status:200 -has:resbody"],
    },
    FlagSpec {
        id: FlagId::Query,
        prefix: "has",
        name: "query",
        full: "has:query",
        aliases: &["querystring", "params"],
        description: "URL has a query string",
        examples: &["has:query", "-has:query"],
    },
    FlagSpec {
        id: FlagId::Cookie,
        prefix: "has",
        name: "cookie",
        full: "has:cookie",
        aliases: &["cookies"],
        description: "Sends a Cookie or sets one (Set-Cookie)",
        examples: &["has:cookie", "has:cookie -is:https"],
    },
    FlagSpec {
        id: FlagId::Auth,
        prefix: "has",
        name: "auth",
        full: "has:auth",
        aliases: &["authorization", "credentials", "authenticated"],
        description: "Request carries an Authorization or Proxy-Authorization header",
        examples: &["has:auth", "has:auth is:http"],
    },
    FlagSpec {
        id: FlagId::Rule,
        prefix: "has",
        name: "rule",
        full: "has:rule",
        aliases: &["rules", "handled"],
        description: "Handled by a rule (mock, rewrite, breakpoint, …)",
        examples: &["has:rule", "-has:rule"],
    },
    FlagSpec {
        id: FlagId::Response,
        prefix: "has",
        name: "response",
        full: "has:response",
        aliases: &["responded", "status"],
        description: "A response (status code) was received",
        examples: &["has:response", "-has:response is:complete"],
    },
    FlagSpec {
        id: FlagId::Messages,
        prefix: "has",
        name: "messages",
        full: "has:messages",
        aliases: &["message", "msgs"],
        description: "Has WebSocket or SSE messages",
        examples: &["has:messages", "is:websocket -has:messages"],
    },
];

/// Find a flag by (case-insensitive) name or alias.
pub(crate) fn flag_by_name(name: &str) -> Option<&'static FlagSpec> {
    let lower = name.to_ascii_lowercase();
    FLAGS.iter().find(|f| f.is_named(&lower))
}

/// LLM provider ids (`provider:` suggestions), most common first.
pub(crate) static PROVIDERS: &[(&str, &str)] = &[
    ("openai", "OpenAI"),
    ("anthropic", "Anthropic"),
    ("google", "Google Gemini (AI Studio)"),
    ("vertex-ai", "Google Vertex AI"),
    ("azure-openai", "Azure OpenAI"),
    ("aws-bedrock", "Amazon Bedrock"),
    ("openrouter", "OpenRouter"),
    ("ollama", "Ollama (local)"),
    ("lmstudio", "LM Studio (local)"),
    ("groq", "Groq"),
    ("mistral", "Mistral"),
    ("deepseek", "DeepSeek"),
    ("xai", "xAI"),
    ("together", "Together AI"),
    ("fireworks", "Fireworks AI"),
    ("perplexity", "Perplexity"),
    ("cerebras", "Cerebras"),
    ("huggingface", "Hugging Face"),
    ("openai-compatible", "Other OpenAI-compatible server"),
    ("anthropic-compatible", "Other Anthropic-compatible server"),
];

/// Standard HTTP methods, most common first.
pub(crate) static METHODS: &[(&str, &str)] = &[
    ("GET", "Retrieve a resource"),
    ("POST", "Submit data / create"),
    ("PUT", "Replace a resource"),
    ("PATCH", "Partially update a resource"),
    ("DELETE", "Delete a resource"),
    ("HEAD", "Headers only"),
    ("OPTIONS", "CORS preflight / capabilities"),
    ("CONNECT", "Open a tunnel"),
    ("TRACE", "Loop-back test"),
];

/// Status classes for suggestions.
pub(crate) static STATUS_CLASSES: &[(&str, &str)] = &[
    ("2xx", "Success"),
    ("3xx", "Redirection"),
    ("4xx", "Client error"),
    ("5xx", "Server error"),
    ("1xx", "Informational"),
];

/// Commonly seen status codes for suggestions (descriptions come from the canonical reason).
pub(crate) static COMMON_STATUSES: &[u16] = &[
    200, 201, 204, 206, 301, 302, 304, 307, 308, 400, 401, 403, 404, 405, 409, 410, 413, 415, 422,
    429, 500, 501, 502, 503, 504, 101,
];

/// Content kind names (the `type:` vocabulary) with the kinds they select.
pub(crate) static KIND_ALIASES: &[(&str, &[ContentKind], &str)] = &[
    ("json", &[ContentKind::Json, ContentKind::JsonLines], "JSON"),
    ("html", &[ContentKind::Html], "HTML documents"),
    ("js", &[ContentKind::Javascript], "JavaScript"),
    ("css", &[ContentKind::Css], "Stylesheets"),
    ("image", &[ContentKind::Image], "Images"),
    ("font", &[ContentKind::Font], "Fonts"),
    ("xml", &[ContentKind::Xml], "XML"),
    (
        "data",
        &[
            ContentKind::Json,
            ContentKind::JsonLines,
            ContentKind::Xml,
            ContentKind::Yaml,
            ContentKind::Csv,
            ContentKind::FormUrlEncoded,
            ContentKind::Multipart,
            ContentKind::Protobuf,
            ContentKind::Grpc,
            ContentKind::GrpcWeb,
            ContentKind::MsgPack,
            ContentKind::Cbor,
            ContentKind::GraphQl,
        ],
        "API data (JSON, XML, forms, protobuf, …)",
    ),
    (
        "media",
        &[ContentKind::Audio, ContentKind::Video],
        "Audio and video",
    ),
    ("text", &[ContentKind::Text], "Plain text"),
    (
        "form",
        &[ContentKind::FormUrlEncoded, ContentKind::Multipart],
        "Form submissions",
    ),
    ("grpc", &[ContentKind::Grpc, ContentKind::GrpcWeb], "gRPC"),
    ("protobuf", &[ContentKind::Protobuf], "Protocol buffers"),
    ("graphql", &[ContentKind::GraphQl], "GraphQL"),
    ("sse", &[ContentKind::EventStream], "Server-sent events"),
    ("audio", &[ContentKind::Audio], "Audio"),
    ("video", &[ContentKind::Video], "Video"),
    ("pdf", &[ContentKind::Pdf], "PDF documents"),
    ("wasm", &[ContentKind::Wasm], "WebAssembly"),
    ("yaml", &[ContentKind::Yaml], "YAML"),
    ("csv", &[ContentKind::Csv], "CSV"),
    ("markdown", &[ContentKind::Markdown], "Markdown"),
    ("multipart", &[ContentKind::Multipart], "Multipart bodies"),
    (
        "urlencoded",
        &[ContentKind::FormUrlEncoded],
        "URL-encoded forms",
    ),
    ("jsonl", &[ContentKind::JsonLines], "JSON lines / NDJSON"),
    ("grpc-web", &[ContentKind::GrpcWeb], "gRPC-Web"),
    ("msgpack", &[ContentKind::MsgPack], "MessagePack"),
    ("cbor", &[ContentKind::Cbor], "CBOR"),
    ("binary", &[ContentKind::Binary], "Other binary data"),
    ("unknown", &[ContentKind::Unknown], "Unknown / no content"),
    // Synonyms (not suggested separately).
    ("javascript", &[ContentKind::Javascript], ""),
    ("script", &[ContentKind::Javascript], ""),
    ("style", &[ContentKind::Css], ""),
    ("img", &[ContentKind::Image], ""),
    ("images", &[ContentKind::Image], ""),
    ("fonts", &[ContentKind::Font], ""),
    ("txt", &[ContentKind::Text], ""),
    ("plain", &[ContentKind::Text], ""),
    ("md", &[ContentKind::Markdown], ""),
    ("yml", &[ContentKind::Yaml], ""),
    ("ndjson", &[ContentKind::JsonLines], ""),
    ("json-lines", &[ContentKind::JsonLines], ""),
    ("proto", &[ContentKind::Protobuf], ""),
    ("gql", &[ContentKind::GraphQl], ""),
    ("event-stream", &[ContentKind::EventStream], ""),
    ("messagepack", &[ContentKind::MsgPack], ""),
    ("webassembly", &[ContentKind::Wasm], ""),
    ("bin", &[ContentKind::Binary], ""),
    ("other", &[ContentKind::Unknown], ""),
    ("form-urlencoded", &[ContentKind::FormUrlEncoded], ""),
];

/// Look up a content kind alias (case-insensitive) and return the selected kinds.
pub(crate) fn kinds_by_alias(name: &str) -> Option<&'static [ContentKind]> {
    KIND_ALIASES
        .iter()
        .find(|(alias, _, _)| alias.eq_ignore_ascii_case(name))
        .map(|(_, kinds, _)| *kinds)
}

/// HTTP Toolkit categories that describe an exchange's state rather than its content
/// (`category=mutative`, `category=incomplete`, …); under `type:` they select the matching flag.
pub(crate) static TYPE_FLAG_ALIASES: &[(&str, FlagId)] = &[
    ("mutative", FlagId::Mutative),
    ("websocket", FlagId::WebSocket),
    ("incomplete", FlagId::Pending),
    ("aborted", FlagId::Aborted),
];

/// Look up a state category (case-insensitive) usable as a `type:` value.
pub(crate) fn type_flag_by_alias(name: &str) -> Option<FlagId> {
    TYPE_FLAG_ALIASES
        .iter()
        .find(|(alias, _)| alias.eq_ignore_ascii_case(name))
        .map(|(_, flag)| *flag)
}

/// Bit for a content kind in a `u32` set.
pub(crate) fn kind_bit(kind: ContentKind) -> u32 {
    1u32 << (kind as u32)
}

/// The serialized (kebab-case) name of a content kind.
pub(crate) fn kind_name(kind: ContentKind) -> &'static str {
    match kind {
        ContentKind::Json => "json",
        ContentKind::JsonLines => "json-lines",
        ContentKind::Xml => "xml",
        ContentKind::Html => "html",
        ContentKind::Javascript => "javascript",
        ContentKind::Css => "css",
        ContentKind::Markdown => "markdown",
        ContentKind::Yaml => "yaml",
        ContentKind::Csv => "csv",
        ContentKind::Text => "text",
        ContentKind::FormUrlEncoded => "form-url-encoded",
        ContentKind::Multipart => "multipart",
        ContentKind::Image => "image",
        ContentKind::Audio => "audio",
        ContentKind::Video => "video",
        ContentKind::Font => "font",
        ContentKind::Pdf => "pdf",
        ContentKind::Wasm => "wasm",
        ContentKind::Protobuf => "protobuf",
        ContentKind::Grpc => "grpc",
        ContentKind::GrpcWeb => "grpc-web",
        ContentKind::MsgPack => "msg-pack",
        ContentKind::Cbor => "cbor",
        ContentKind::EventStream => "event-stream",
        ContentKind::GraphQl => "graph-ql",
        ContentKind::Binary => "binary",
        ContentKind::Unknown => "unknown",
    }
}

/// HTTP version vocabulary for `version:`.
pub(crate) static VERSION_ALIASES: &[(&str, &[HttpVersion], &str)] = &[
    (
        "h1",
        &[
            HttpVersion::Http09,
            HttpVersion::Http10,
            HttpVersion::Http11,
        ],
        "HTTP/1.x",
    ),
    ("h2", &[HttpVersion::H2], "HTTP/2"),
    ("h3", &[HttpVersion::H3], "HTTP/3 (QUIC)"),
    ("1.1", &[HttpVersion::Http11], "HTTP/1.1"),
    ("1.0", &[HttpVersion::Http10], "HTTP/1.0"),
    ("0.9", &[HttpVersion::Http09], ""),
    (
        "1",
        &[
            HttpVersion::Http09,
            HttpVersion::Http10,
            HttpVersion::Http11,
        ],
        "",
    ),
    (
        "1.x",
        &[
            HttpVersion::Http09,
            HttpVersion::Http10,
            HttpVersion::Http11,
        ],
        "",
    ),
    (
        "http1",
        &[
            HttpVersion::Http09,
            HttpVersion::Http10,
            HttpVersion::Http11,
        ],
        "",
    ),
    (
        "http/1",
        &[
            HttpVersion::Http09,
            HttpVersion::Http10,
            HttpVersion::Http11,
        ],
        "",
    ),
    (
        "http/1.x",
        &[
            HttpVersion::Http09,
            HttpVersion::Http10,
            HttpVersion::Http11,
        ],
        "",
    ),
    ("http/1.1", &[HttpVersion::Http11], ""),
    ("h1.1", &[HttpVersion::Http11], ""),
    ("http/1.0", &[HttpVersion::Http10], ""),
    ("h1.0", &[HttpVersion::Http10], ""),
    ("http/0.9", &[HttpVersion::Http09], ""),
    ("2", &[HttpVersion::H2], ""),
    ("2.0", &[HttpVersion::H2], ""),
    ("http2", &[HttpVersion::H2], ""),
    ("http/2", &[HttpVersion::H2], ""),
    ("http/2.0", &[HttpVersion::H2], ""),
    ("3", &[HttpVersion::H3], ""),
    ("http3", &[HttpVersion::H3], ""),
    ("http/3", &[HttpVersion::H3], ""),
    ("quic", &[HttpVersion::H3], ""),
];

/// Look up an HTTP version alias (case-insensitive).
pub(crate) fn versions_by_alias(name: &str) -> Option<&'static [HttpVersion]> {
    VERSION_ALIASES
        .iter()
        .find(|(alias, _, _)| alias.eq_ignore_ascii_case(name))
        .map(|(_, v, _)| *v)
}

/// Bit for an HTTP version in a `u8` set.
pub(crate) fn version_bit(v: HttpVersion) -> u8 {
    1u8 << (v as u8)
}

/// Highlight colours.
pub(crate) static COLORS: &[&str] = &["red", "orange", "yellow", "green", "blue", "purple", "grey"];

/// URL schemes.
pub(crate) static SCHEMES: &[(&str, &str)] = &[
    ("https", "HTTP over TLS"),
    ("http", "Plain-text HTTP"),
    ("wss", "WebSocket over TLS"),
    ("ws", "Plain-text WebSocket"),
];

/// Every error kind, with its serialized name and a short description.
pub(crate) static ERROR_KINDS: &[(ErrorKind, &str, &str)] = &[
    (
        ErrorKind::ClientAborted,
        "client-aborted",
        "Client closed the connection early",
    ),
    (
        ErrorKind::DnsFailed,
        "dns-failed",
        "DNS lookup for the upstream host failed",
    ),
    (
        ErrorKind::ConnectionRefused,
        "connection-refused",
        "Upstream refused the connection",
    ),
    (
        ErrorKind::ConnectionReset,
        "connection-reset",
        "Upstream reset the connection",
    ),
    (
        ErrorKind::HostUnreachable,
        "host-unreachable",
        "Upstream host unreachable",
    ),
    (
        ErrorKind::Timeout,
        "timeout",
        "Timed out connecting to / waiting for upstream",
    ),
    (
        ErrorKind::UpstreamCertInvalid,
        "upstream-cert-invalid",
        "Upstream certificate invalid or untrusted",
    ),
    (
        ErrorKind::UpstreamTlsFailed,
        "upstream-tls-failed",
        "Upstream TLS handshake failed",
    ),
    (
        ErrorKind::UpstreamProtocolError,
        "upstream-protocol-error",
        "Upstream sent malformed HTTP",
    ),
    (
        ErrorKind::ClientProtocolError,
        "client-protocol-error",
        "Client sent malformed HTTP",
    ),
    (
        ErrorKind::ClientTlsRejected,
        "client-tls-rejected",
        "Client rejected the Akev certificate",
    ),
    (
        ErrorKind::RuleClosed,
        "rule-closed",
        "A rule closed the connection",
    ),
    (
        ErrorKind::RuleReset,
        "rule-reset",
        "A rule reset the connection",
    ),
    (
        ErrorKind::RuleTimeout,
        "rule-timeout",
        "A rule timed the request out",
    ),
    (
        ErrorKind::BreakpointAborted,
        "breakpoint-aborted",
        "Aborted at a breakpoint",
    ),
    (
        ErrorKind::UpstreamProxyFailed,
        "upstream-proxy-failed",
        "Upstream proxy failed",
    ),
    (ErrorKind::Internal, "internal", "Internal error"),
];

/// The serialized (kebab-case) name of an error kind.
pub(crate) fn error_name(kind: ErrorKind) -> &'static str {
    ERROR_KINDS
        .iter()
        .find(|(k, _, _)| *k == kind)
        .map(|(_, name, _)| *name)
        .unwrap_or("internal")
}

/// Common header names, offered after the store's own list.
pub(crate) static COMMON_HEADERS: &[&str] = &[
    "content-type",
    "authorization",
    "cookie",
    "set-cookie",
    "user-agent",
    "accept",
    "accept-encoding",
    "accept-language",
    "cache-control",
    "content-encoding",
    "content-length",
    "location",
    "origin",
    "referer",
    "host",
    "etag",
    "if-none-match",
    "server",
    "vary",
    "x-request-id",
    "x-forwarded-for",
    "access-control-allow-origin",
    "content-security-policy",
    "strict-transport-security",
    "x-api-key",
];

/// Common MIME types, offered after the store's own list.
pub(crate) static COMMON_MIME_TYPES: &[&str] = &[
    "application/json",
    "text/html",
    "text/plain",
    "application/javascript",
    "text/css",
    "image/png",
    "image/jpeg",
    "image/svg+xml",
    "image/webp",
    "application/xml",
    "application/x-www-form-urlencoded",
    "multipart/form-data",
    "application/grpc",
    "text/event-stream",
    "application/octet-stream",
];

/// Well-known values for a few headers (`header.<name>:` value suggestions).
pub(crate) fn known_header_values(name: &str) -> &'static [&'static str] {
    match name.to_ascii_lowercase().as_str() {
        "content-encoding" | "accept-encoding" => &["gzip", "br", "deflate", "zstd", "identity"],
        "cache-control" => &[
            "no-cache",
            "no-store",
            "max-age=0",
            "public",
            "private",
            "immutable",
        ],
        "connection" => &["keep-alive", "close", "upgrade"],
        "upgrade" => &["websocket", "h2c"],
        "transfer-encoding" => &["chunked"],
        "x-cache" => &["hit", "miss"],
        "sec-fetch-dest" => &["document", "script", "style", "image", "font", "empty"],
        "sec-fetch-mode" => &["cors", "navigate", "no-cors", "same-origin", "websocket"],
        "sec-fetch-site" => &["same-origin", "same-site", "cross-site", "none"],
        "authorization" => &["Bearer*", "Basic*"],
        _ => &[],
    }
}

/// Longest input [`edit_distance`] considers (longer strings are never "typos").
const EDIT_MAX_LEN: usize = 48;

fn fill_lower(s: &str, buf: &mut [char; EDIT_MAX_LEN]) -> Option<usize> {
    let mut n = 0;
    for c in s.chars() {
        *buf.get_mut(n)? = c.to_ascii_lowercase();
        n += 1;
    }
    Some(n)
}

/// Optimal-string-alignment edit distance (Levenshtein + adjacent transpositions), on chars,
/// ASCII case-insensitive. Returns `None` when the distance exceeds `max` (checked early) or an
/// input is longer than 48 characters. Allocation-free.
pub(crate) fn edit_distance(a: &str, b: &str, max: usize) -> Option<usize> {
    let mut abuf = ['\0'; EDIT_MAX_LEN];
    let mut bbuf = ['\0'; EDIT_MAX_LEN];
    let la = fill_lower(a, &mut abuf)?;
    let lb = fill_lower(b, &mut bbuf)?;
    if la.abs_diff(lb) > max {
        return None;
    }
    let (a, b) = (&abuf[..la], &bbuf[..lb]);
    // Three rolling rows: i-2, i-1 and i (distances never exceed 48, so u8 suffices).
    let mut rows = [[0u8; EDIT_MAX_LEN + 1]; 3];
    for (j, cell) in rows[0].iter_mut().enumerate().take(lb + 1) {
        *cell = j as u8;
    }
    let max8 = max.min(EDIT_MAX_LEN) as u8;
    for i in 1..=la {
        let (cur, prev, prev2) = (i % 3, (i + 2) % 3, (i + 1) % 3);
        rows[cur][0] = i as u8;
        let mut row_min = i as u8;
        for j in 1..=lb {
            let cost = u8::from(a[i - 1] != b[j - 1]);
            let mut v = (rows[prev][j] + 1)
                .min(rows[cur][j - 1] + 1)
                .min(rows[prev][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                v = v.min(rows[prev2][j - 2] + 1);
            }
            rows[cur][j] = v;
            row_min = row_min.min(v);
        }
        if row_min > max8 {
            return None;
        }
    }
    let d = rows[la % 3][lb];
    (d <= max8).then_some(usize::from(d))
}

/// The closest candidate to a misspelt `name`, for "did you mean" hints.
pub(crate) fn closest<'a>(
    name: &str,
    candidates: impl IntoIterator<Item = &'a str>,
) -> Option<&'a str> {
    let len = name.chars().count();
    if len == 0 {
        return None;
    }
    let max = match len {
        0..=4 => 1,
        5..=7 => 2,
        _ => 3,
    };
    let mut best: Option<(usize, &str)> = None;
    for cand in candidates {
        let d = if len >= 3 && cand.len() > len && starts_with_ci(cand, name) {
            // A truncated field name ("stat") is as good as a one-letter typo.
            Some(1)
        } else {
            edit_distance(name, cand, max).filter(|&d| d < len.max(2))
        };
        if let Some(d) = d
            && best.is_none_or(|(bd, _)| d < bd)
        {
            best = Some((d, cand));
        }
    }
    best.map(|(_, c)| c)
}

/// ASCII case-insensitive `starts_with`.
pub(crate) fn starts_with_ci(haystack: &str, prefix: &str) -> bool {
    haystack.len() >= prefix.len()
        && haystack.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

/// All field names and aliases.
pub(crate) fn all_field_names() -> impl Iterator<Item = &'static str> {
    FIELDS
        .iter()
        .flat_map(|f| std::iter::once(f.name).chain(f.aliases.iter().copied()))
        .chain(["is", "has"])
}

/// All flag names and aliases.
pub(crate) fn all_flag_names() -> impl Iterator<Item = &'static str> {
    FLAGS
        .iter()
        .flat_map(|f| std::iter::once(f.name).chain(f.aliases.iter().copied()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn names_are_unique_and_lowercase() {
        let mut seen = HashSet::new();
        for name in all_field_names() {
            assert_eq!(name, name.to_ascii_lowercase());
            assert!(seen.insert(name), "duplicate field name {name}");
        }
        let mut seen = HashSet::new();
        for name in all_flag_names() {
            assert_eq!(name, name.to_ascii_lowercase());
            assert!(seen.insert(name), "duplicate flag name {name}");
        }
        let mut seen = HashSet::new();
        for (alias, _, _) in KIND_ALIASES {
            assert!(seen.insert(*alias), "duplicate kind alias {alias}");
        }
    }

    #[test]
    fn edit_distance_basics() {
        assert_eq!(edit_distance("status", "status", 2), Some(0));
        assert_eq!(edit_distance("stauts", "status", 2), Some(1));
        assert_eq!(edit_distance("mehtod", "method", 2), Some(1));
        assert_eq!(edit_distance("hst", "host", 2), Some(1));
        assert_eq!(edit_distance("abc", "xyz", 2), None);
        assert_eq!(edit_distance("", "ab", 2), Some(2));
        assert_eq!(edit_distance("STATUS", "status", 0), Some(0));
        assert_eq!(edit_distance("ü", "u", 1), Some(1));
        assert_eq!(edit_distance("ab", "ba", 1), Some(1));
        assert_eq!(edit_distance("kitten", "sitting", 3), Some(3));
        assert_eq!(edit_distance(&"a".repeat(60), &"a".repeat(60), 3), None);
    }

    #[test]
    fn closest_suggestions() {
        assert_eq!(closest("stauts", all_field_names()), Some("status"));
        assert_eq!(closest("mehtod", all_field_names()), Some("method"));
        assert_eq!(closest("stat", all_field_names()), Some("status"));
        assert_eq!(closest("pinnd", all_flag_names()), Some("pinned"));
        assert_eq!(closest("zzzzzzzz", all_field_names()), None);
        assert_eq!(closest("", all_field_names()), None);
    }

    #[test]
    fn kind_bits_fit() {
        assert!((ContentKind::Unknown as u32) < 32);
        assert!((HttpVersion::H3 as u8) < 8);
        assert_eq!(error_name(ErrorKind::DnsFailed), "dns-failed");
        assert_eq!(ERROR_KINDS.len(), 17);
    }
}
