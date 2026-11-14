//! # akev-codegen
//!
//! Turn a captured request into ready-to-run code in many languages ("Copy as cURL", etc.)
//! and parse cURL commands back into requests ("Import from cURL" / paste into Compose).
//!
//! Rules for every generator:
//! * Omit hop-by-hop & auto-managed headers (`Host` when it matches the URL, `Content-Length`,
//!   `Connection`, `Transfer-Encoding`, HTTP/2 pseudo-headers, `Accept-Encoding` only when the
//!   target can't decompress — document per target).
//! * Text bodies are inlined with correct escaping for the target language; JSON bodies use
//!   the language's idiomatic JSON support where natural; binary bodies are base64-decoded at
//!   runtime or written as byte escapes.
//! * Output must be valid, runnable code (tests compile/parse where feasible).
//!
//! ## Request sanitisation (all targets)
//!
//! The request is expected to carry the *decoded* body, so the following headers are always
//! dropped: `Content-Length` (recomputed by every client), `Content-Encoding` (the body is no
//! longer encoded), HTTP/2 pseudo-headers (`:authority`, …), the hop-by-hop headers
//! `Connection`, `Keep-Alive`, `Proxy-Connection`, `Proxy-Authorization`, `Transfer-Encoding`,
//! `TE`, `Trailer`, `Upgrade` (plus any header named in `Connection`), and `Expect` (transport
//! negotiation the client performs itself). `Host` is dropped when it matches the URL's
//! authority and kept otherwise (Go sets `req.Host`, Java enables the restricted header).
//!
//! Unlike HTTP Toolkit, **`Cookie` headers are kept**: replaying a request without its session
//! cookies is rarely what the user wants. (Browser `fetch` silently ignores a `Cookie` header;
//! the snippet says so.)
//!
//! `Accept-Encoding`: the cURL targets emit `--compressed` instead (curl negotiates the
//! encodings it supports and transparently decodes the response); the raw HTTP target keeps
//! the header verbatim. Every other target drops it, because its client either negotiates
//! compression itself and decodes transparently (requests, httpx, Go, OkHttp, URLSession,
//! Net::HTTP, Dart, axios, Node fetch, HTTPie, browsers) or would otherwise hand back
//! undecoded bytes (Java `HttpClient`, wget, reqwest without features). C# enables
//! `AutomaticDecompression` and PHP sets `CURLOPT_ENCODING => ''` when the original request
//! asked for compression.
//!
//! ## Client limitations
//!
//! Where a client can't reproduce part of the request, the snippet still carries it and adds a
//! `Note:` comment rather than silently changing it:
//!
//! * Non-ASCII header values are sent as their UTF-8 bytes. That's natural for the shell
//!   tools, Go, reqwest, PHP and Ruby; JavaScript clients write header values as Latin-1, so
//!   the value is converted to a string of its UTF-8 bytes; Python passes `bytes`; OkHttp uses
//!   `addUnsafeNonAscii`; C# sets `RequestHeaderEncodingSelector`. Java's `HttpClient`, .NET's
//!   `Invoke-WebRequest` and `dart:io` can't send them at all (note).
//! * `fetch` can't set `Host` (and browsers drop every forbidden header, e.g. `Cookie`,
//!   `Origin`, `Sec-*`), URLSession may override `Host`, requests/HTTPX upper-case custom
//!   methods, Wget2 ignores empty header values, and `fetch`/OkHttp/URLSession can't send a
//!   body with GET or HEAD (it's omitted) — each with a note.
//! * URLs are emitted verbatim, except for `java.net.URI`, Foundation's `URL` and Dart's
//!   `Uri.parse`, which get characters RFC 3986 doesn't allow (`"`, `{`, `[` in the path, …)
//!   percent-encoded.
//!
//! ## Bodies
//!
//! * `None` or empty: no body.
//! * Valid UTF-8 without NUL bytes is *text* and is inlined with the target's best literal
//!   syntax (raw strings / text blocks / here-strings where they're safe, escaped strings
//!   otherwise). Invisible and bidi control characters are always written as escapes.
//! * JSON (`application/json`, `*/*+json`) that survives a parse → re-serialise round trip
//!   (no duplicate keys, numbers keep their exact spelling) is emitted as a native literal in
//!   JavaScript (`JSON.stringify({...})`, axios `data: {...}`), Python (`json=`), Ruby
//!   (`JSON.generate`) and PHP (`json_encode`). Those clients re-serialise it, so whitespace may
//!   differ while the JSON value is identical. All other targets inline the exact text.
//! * `application/x-www-form-urlencoded` bodies that decode cleanly use native form support in
//!   JavaScript (`URLSearchParams`), Python (`data=`), Ruby (`URI.encode_www_form`) and PHP
//!   (`http_build_query`, when keys are unique); cURL splits them into one `--data-raw` per
//!   field (byte-identical once curl joins them with `&`).
//! * Anything else (*binary*) is base64-decoded at runtime (`base64 --decode` piped into the
//!   shell tools, `Convert.FromBase64String`, `atob`/`Buffer.from`, `base64.b64decode`, …) or,
//!   in Rust, written as a byte-string literal. Windows `cmd` can't pipe binary data, so the
//!   cmd variant decodes it to `akev-body.bin` with PowerShell first. Bodies of any size are
//!   included; there are no placeholders.
//!
//! Generated snippets never end with a trailing newline (so pasting into a terminal doesn't run
//! the command immediately), except the raw HTTP target, which is the exact wire text.

mod curl;
mod json;
mod lit;
mod prepare;
mod targets;

use akev_core::Headers;
use bytes::Bytes;
use serde::Serialize;
use std::fmt;
use std::str::FromStr;

/// The request to generate code for (decoded body, i.e. no content-encoding).
#[derive(Clone, Debug, PartialEq)]
pub struct SnippetRequest {
    /// HTTP method. Standard methods are matched case-insensitively (like `fetch`); an empty
    /// method means `GET`.
    pub method: String,
    /// Absolute URL, emitted verbatim.
    pub url: String,
    /// Request headers in wire order (sanitised by [`generate`], see the crate docs).
    pub headers: Headers,
    /// Decoded body; `None` and an empty body both mean "no body".
    pub body: Option<Bytes>,
}

impl From<akev_core::EditableRequest> for SnippetRequest {
    fn from(req: akev_core::EditableRequest) -> Self {
        SnippetRequest {
            method: req.method,
            url: req.url,
            headers: req.headers,
            body: (!req.body.is_empty()).then_some(req.body),
        }
    }
}

impl From<SnippetRequest> for akev_core::EditableRequest {
    fn from(req: SnippetRequest) -> Self {
        akev_core::EditableRequest {
            method: req.method,
            url: req.url,
            headers: req.headers,
            body: req.body.unwrap_or_default(),
        }
    }
}

/// A snippet target (language + client library).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Target {
    /// cURL for bash/zsh (POSIX quoting, `\` continuations).
    Curl,
    /// cURL for Windows `cmd.exe` (`^` escaping and continuations).
    CurlWindows,
    /// GNU Wget (1.15+ / Wget2).
    Wget,
    /// HTTPie (`http` command).
    Httpie,
    /// PowerShell 7+ `Invoke-WebRequest`.
    Powershell,
    /// Browser `fetch`.
    JsFetch,
    /// Node.js 18+ built-in `fetch`.
    NodeFetch,
    /// Node.js with axios.
    NodeAxios,
    /// Python with requests.
    PythonRequests,
    /// Python with HTTPX.
    PythonHttpx,
    /// Go `net/http`.
    GoNetHttp,
    /// Rust reqwest (async, tokio).
    RustReqwest,
    /// Java 15+ `java.net.http.HttpClient`.
    JavaHttpClient,
    /// Kotlin with OkHttp 4+.
    KotlinOkhttp,
    /// C# (.NET 7+) `HttpClient`.
    CsharpHttpClient,
    /// PHP ext-curl.
    PhpCurl,
    /// Ruby `Net::HTTP`.
    RubyNetHttp,
    /// Swift `URLSession` (async/await).
    SwiftUrlSession,
    /// Dart `package:http`.
    DartHttp,
    /// Raw HTTP/1.1 request text.
    RawHttp,
}

impl Target {
    /// Every target, in display order.
    pub const ALL: [Target; 20] = [
        Target::Curl,
        Target::CurlWindows,
        Target::Wget,
        Target::Httpie,
        Target::Powershell,
        Target::JsFetch,
        Target::NodeFetch,
        Target::NodeAxios,
        Target::PythonRequests,
        Target::PythonHttpx,
        Target::GoNetHttp,
        Target::RustReqwest,
        Target::JavaHttpClient,
        Target::KotlinOkhttp,
        Target::CsharpHttpClient,
        Target::PhpCurl,
        Target::RubyNetHttp,
        Target::SwiftUrlSession,
        Target::DartHttp,
        Target::RawHttp,
    ];

    /// The serialised (kebab-case) id, e.g. `"python-requests"`, as used in `?target=`.
    pub fn id(self) -> &'static str {
        match self {
            Target::Curl => "curl",
            Target::CurlWindows => "curl-windows",
            Target::Wget => "wget",
            Target::Httpie => "httpie",
            Target::Powershell => "powershell",
            Target::JsFetch => "js-fetch",
            Target::NodeFetch => "node-fetch",
            Target::NodeAxios => "node-axios",
            Target::PythonRequests => "python-requests",
            Target::PythonHttpx => "python-httpx",
            Target::GoNetHttp => "go-net-http",
            Target::RustReqwest => "rust-reqwest",
            Target::JavaHttpClient => "java-http-client",
            Target::KotlinOkhttp => "kotlin-okhttp",
            Target::CsharpHttpClient => "csharp-http-client",
            Target::PhpCurl => "php-curl",
            Target::RubyNetHttp => "ruby-net-http",
            Target::SwiftUrlSession => "swift-url-session",
            Target::DartHttp => "dart-http",
            Target::RawHttp => "raw-http",
        }
    }

    /// Display metadata for this target.
    pub fn info(self) -> TargetInfo {
        let (name, language, group) = match self {
            Target::Curl => ("cURL", "shell", "Shell"),
            Target::CurlWindows => ("cURL (Windows cmd)", "shell", "Shell"),
            Target::Wget => ("Wget", "shell", "Shell"),
            Target::Httpie => ("HTTPie", "shell", "Shell"),
            Target::Powershell => ("PowerShell · Invoke-WebRequest", "powershell", "Shell"),
            Target::JsFetch => ("JavaScript · fetch", "javascript", "JavaScript"),
            Target::NodeFetch => ("Node.js · fetch", "javascript", "Node.js"),
            Target::NodeAxios => ("Node.js · Axios", "javascript", "Node.js"),
            Target::PythonRequests => ("Python · requests", "python", "Python"),
            Target::PythonHttpx => ("Python · HTTPX", "python", "Python"),
            Target::GoNetHttp => ("Go · net/http", "go", "Go"),
            Target::RustReqwest => ("Rust · reqwest", "rust", "Rust"),
            Target::JavaHttpClient => ("Java · HttpClient", "java", "Java"),
            Target::KotlinOkhttp => ("Kotlin · OkHttp", "kotlin", "Kotlin"),
            Target::CsharpHttpClient => ("C# · HttpClient", "csharp", "C#"),
            Target::PhpCurl => ("PHP · cURL", "php", "PHP"),
            Target::RubyNetHttp => ("Ruby · Net::HTTP", "ruby", "Ruby"),
            Target::SwiftUrlSession => ("Swift · URLSession", "swift", "Swift"),
            Target::DartHttp => ("Dart · http", "dart", "Dart"),
            Target::RawHttp => ("Raw HTTP/1.1", "http", "HTTP"),
        };
        TargetInfo {
            id: self,
            name,
            language,
            group,
        }
    }

    /// Conventional file extension (without the dot) for a snippet file, e.g. for ZIP export.
    pub fn file_extension(self) -> &'static str {
        match self {
            Target::Curl | Target::Wget | Target::Httpie => "sh",
            Target::CurlWindows => "cmd",
            Target::Powershell => "ps1",
            Target::JsFetch => "js",
            Target::NodeFetch | Target::NodeAxios => "mjs",
            Target::PythonRequests | Target::PythonHttpx => "py",
            Target::GoNetHttp => "go",
            Target::RustReqwest => "rs",
            Target::JavaHttpClient => "java",
            Target::KotlinOkhttp => "kt",
            Target::CsharpHttpClient => "cs",
            Target::PhpCurl => "php",
            Target::RubyNetHttp => "rb",
            Target::SwiftUrlSession => "swift",
            Target::DartHttp => "dart",
            Target::RawHttp => "http",
        }
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

/// Error returned when parsing an unknown target id.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("unknown snippet target: {0}")]
pub struct UnknownTarget(pub String);

impl FromStr for Target {
    type Err = UnknownTarget;

    /// Parses a kebab-case id (case-insensitive), e.g. `"curl"` or `"python-requests"`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        Target::ALL
            .into_iter()
            .find(|t| t.id().eq_ignore_ascii_case(s))
            .ok_or_else(|| UnknownTarget(s.to_string()))
    }
}

/// Display metadata for a [`Target`].
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetInfo {
    /// The target this describes.
    pub id: Target,
    /// Display name, e.g. "cURL", "Python · requests".
    pub name: &'static str,
    /// Language id for syntax highlighting: `shell`, `javascript`, `python`, `go`, `rust`,
    /// `java`, `kotlin`, `csharp`, `php`, `ruby`, `swift`, `dart`, `http`, `powershell`.
    pub language: &'static str,
    /// Grouping in the UI: "Shell", "JavaScript", "Python", ...
    pub group: &'static str,
}

/// All supported targets in display order.
pub fn targets() -> Vec<TargetInfo> {
    Target::ALL.into_iter().map(Target::info).collect()
}

/// Generate a snippet.
///
/// The request is sanitised first (see the crate docs); the result is runnable code for the
/// target without a trailing newline (except [`Target::RawHttp`], which is exact wire text).
pub fn generate(target: Target, req: &SnippetRequest) -> String {
    let prepared = prepare::prepare(req, targets::uses_native_json(target));
    let mut code = targets::generate(target, &prepared);
    if target != Target::RawHttp {
        let trimmed = code.trim_end_matches('\n').len();
        code.truncate(trimmed);
    }
    code
}

/// Error returned by [`parse_curl`].
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ParseError {
    /// The command doesn't invoke `curl`.
    #[error("not a curl command")]
    NotCurl,
    /// A quoted string isn't closed.
    #[error("unterminated quote")]
    UnterminatedQuote,
    /// No URL was given.
    #[error("missing URL")]
    MissingUrl,
    /// An option is malformed (e.g. missing its value) or options conflict.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
}

/// Parse a cURL command line (bash/zsh quoting incl. `$'...'`, line continuations `\`,
/// Windows `^` continuations; flags: -X/--request, -H/--header, -d/--data/--data-raw/
/// --data-binary/--data-urlencode (multiple → joined with &), --json, -F/--form, -u/--user,
/// -b/--cookie, -A/--user-agent, -e/--referer, -I/--head, -G/--get, --compressed, -k,
/// -L, --url, -x (ignored)...). `@file` data references are kept as literal text.
///
/// Details beyond the summary above:
/// * Three quoting dialects are recognised: POSIX shells (the default), Windows `cmd.exe`
///   (detected by `^` continuations or `^"` escapes; `^` escapes plus MSVCRT argument
///   splitting, as produced by Chrome's "Copy as cURL (cmd)"), and PowerShell (commands
///   starting with `curl.exe`, as produced by Firefox's "Copy as cURL (Windows)").
/// * Leading words before `curl` (`sudo`, a `$ ` prompt, …) are skipped, the command ends at
///   an unquoted `|`, `;`, `&` or `&&`, and redirections are ignored. `echo '…' | base64
///   --decode | curl --data-binary @-` (the binary form [`generate`] emits) is understood.
/// * Unknown options are ignored; options known to take a value consume it.
/// * `-d` without an explicit `Content-Type` adds `application/x-www-form-urlencoded` (as curl
///   does), `-H 'Name:'` removes a header, `-H 'Name;'` sends it empty, `-F` builds a
///   `multipart/form-data` body with a fixed boundary (file parts get an empty body),
///   `--compressed` adds `Accept-Encoding: gzip, deflate, br, zstd`, `-u` adds Basic auth
///   unless `Authorization` was given explicitly, and a URL without a scheme gets `http://`.
pub fn parse_curl(command: &str) -> Result<SnippetRequest, ParseError> {
    curl::parse(command)
}
