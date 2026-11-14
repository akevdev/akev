//! Content classification from the Content-Type header, the body bytes and the URL path.
//!
//! The declared type wins when it is specific. Sniffing applies when the type is missing,
//! generic (`application/octet-stream`, `text/plain` and friends) or unrecognized:
//! 1. magic numbers (PNG, JPEG, GIF, WebP, AVIF/HEIF, ICO/CUR, BMP, TIFF, PDF, WASM, ZIP, gzip,
//!    zstd, fonts, common audio/video containers, ...);
//! 2. for text-like bodies: protobuf first when the whole body validates and either the body is
//!    untyped with a protobuf-like first byte (a message whose first field is a ~100-character
//!    string looks like text) or it is non-UTF-8 "legacy" text; then JSON records
//!    (RS-prefixed sequences, SignalR frames, NDJSON), JSON (first character `{`/`[` and the
//!    prefix parses, allowing truncation), markup (`<?xml`, `<!doctype html`, HTML tags per the
//!    WHATWG sniffing table, `<svg` → image), server-sent events and urlencoded forms (untyped
//!    bodies only), then the path extension;
//! 3. for binary bodies: gRPC framing and protobuf (validated), then the path extension.
//!
//! Fixes over HTTP Toolkit's rules: a declared JSON type is respected (JSON records are only
//! chosen when the body really is a sequence of values), only `{`/`[` count as JSON starts
//! (a number at the start of a text body no longer means JSON), protobuf/gRPC checks run on a
//! prefix for large bodies instead of being skipped above 100 KB, and an HTML-labelled body that
//! is actually JSON is detected as JSON.
//!
//! All checks are bounded to small prefixes (≤ 64 KB) except protobuf validation of bodies up to
//! 1 MB, so detection is cheap on huge bodies.

use akev_core::ContentKind;

use crate::protobuf;
use crate::util::{essence, path_extension, path_only, skip_bom_ws};

/// Classify content. `content_type` is the raw Content-Type header (may include params);
/// `body` is the DECODED body (may be a prefix); `path` is the URL path (extension hints).
/// Header wins when specific; sniffing is used for missing/generic types
/// (`application/octet-stream`, `text/plain`).
pub fn detect_kind(content_type: Option<&str>, body: &[u8], path: &str) -> ContentKind {
    let ess = content_type.map(essence).unwrap_or_default();
    let declared = if ess.is_empty() {
        ContentKind::Unknown
    } else {
        kind_from_essence(&ess)
    };
    let untyped = ess.is_empty()
        || (is_generic(&ess) && ess != "text/plain")
        || declared == ContentKind::Unknown;
    let kind = if untyped {
        sniff(body, path, Mode::Untyped)
    } else {
        match declared {
            ContentKind::Text => sniff(body, path, Mode::PlainText),
            ContentKind::Html => refine_html(body),
            ContentKind::Json => refine_json(body),
            ContentKind::FormUrlEncoded => refine_form(body),
            other => other,
        }
    };
    if kind == ContentKind::Json && is_graphql_path(path) && has_graphql_query(body) {
        return ContentKind::GraphQl;
    }
    kind
}

/// Best-effort kind from Content-Type alone (used for list rows before the body arrives).
pub fn kind_from_content_type(content_type: &str) -> ContentKind {
    kind_from_essence(&essence(content_type))
}

/// True when the content kind is displayable as text.
///
/// Note: SVG images are text too but share [`ContentKind::Image`]; use [`crate::image::is_svg`]
/// on the body for that case.
pub fn is_textual(kind: ContentKind) -> bool {
    matches!(
        kind,
        ContentKind::Json
            | ContentKind::JsonLines
            | ContentKind::Xml
            | ContentKind::Html
            | ContentKind::Javascript
            | ContentKind::Css
            | ContentKind::Markdown
            | ContentKind::Yaml
            | ContentKind::Csv
            | ContentKind::Text
            | ContentKind::FormUrlEncoded
            | ContentKind::EventStream
            | ContentKind::GraphQl
    )
}

/// Types that carry no real information about the body.
fn is_generic(ess: &str) -> bool {
    matches!(
        ess,
        "application/octet-stream"
            | "binary/octet-stream"
            | "application/binary"
            | "application/x-binary"
            | "application/unknown"
            | "application/x-unknown"
            | "unknown/unknown"
            | "content/unknown"
            | "application/download"
            | "application/x-download"
            | "application/force-download"
            | "application/x-octet-stream"
            | "*/*"
            | "text/plain"
    )
}

fn kind_from_essence(e: &str) -> ContentKind {
    use ContentKind as C;
    let Some((ty, sub)) = e.split_once('/') else {
        return C::Unknown;
    };
    if ty.is_empty() || sub.is_empty() {
        return C::Unknown;
    }
    match ty {
        "image" => return C::Image,
        "audio" => return C::Audio,
        "video" => return C::Video,
        "font" => return C::Font,
        "multipart" => return C::Multipart,
        _ => {}
    }
    let exact = match e {
        "application/json"
        | "text/json"
        | "application/x-json"
        | "application/csp-report"
        | "application/x-amz-json-1.0"
        | "application/x-amz-json-1.1"
        | "application/jsonrequest"
        | "text/x-json" => Some(C::Json),
        "application/jsonlines"
        | "application/json-lines"
        | "application/x-jsonlines"
        | "application/jsonl"
        | "application/x-jsonl"
        | "application/x-ndjson"
        | "application/ndjson"
        | "application/json-seq"
        | "application/stream+json"
        | "application/x-json-stream"
        | "text/x-ndjson"
        | "text/ndjson" => Some(C::JsonLines),
        "application/xml"
        | "text/xml"
        | "application/rss"
        | "application/x-xml"
        | "application/xml-dtd"
        | "application/xml-external-parsed-entity"
        | "text/xml-external-parsed-entity" => Some(C::Xml),
        "text/html"
        | "application/xhtml+xml"
        | "application/xhtml"
        | "application/vnd.wap.xhtml+xml" => Some(C::Html),
        "application/javascript"
        | "application/x-javascript"
        | "application/ecmascript"
        | "application/x-ecmascript"
        | "text/javascript"
        | "text/ecmascript"
        | "text/x-javascript"
        | "text/x-ecmascript"
        | "text/jscript"
        | "text/livescript"
        | "application/node"
        | "text/javascript1.0"
        | "text/javascript1.1"
        | "text/javascript1.2"
        | "text/javascript1.3"
        | "text/javascript1.4"
        | "text/javascript1.5"
        | "application/typescript"
        | "text/typescript"
        | "application/x-typescript"
        | "text/jsx"
        | "text/babel" => Some(C::Javascript),
        "text/css" => Some(C::Css),
        "text/markdown" | "text/x-markdown" => Some(C::Markdown),
        "text/yaml" | "text/x-yaml" | "application/yaml" | "application/x-yaml"
        | "text/vnd.yaml" => Some(C::Yaml),
        "text/csv"
        | "text/x-csv"
        | "application/csv"
        | "application/x-csv"
        | "text/comma-separated-values"
        | "text/tab-separated-values"
        | "text/tsv" => Some(C::Csv),
        "application/x-www-form-urlencoded" => Some(C::FormUrlEncoded),
        "application/pdf" | "application/x-pdf" => Some(C::Pdf),
        "application/wasm" => Some(C::Wasm),
        "application/x-protobuf"
        | "application/protobuf"
        | "application/vnd.google.protobuf"
        | "application/x-google-protobuf"
        | "application/x-protobuffer"
        | "application/proto"
        | "application/x-proto" => Some(C::Protobuf),
        "application/msgpack"
        | "application/x-msgpack"
        | "application/vnd.msgpack"
        | "application/x-messagepack"
        | "application/vnd.messagepack" => Some(C::MsgPack),
        "application/cbor" | "application/cbor-seq" => Some(C::Cbor),
        "text/event-stream" => Some(C::EventStream),
        "application/graphql" => Some(C::GraphQl),
        "application/font-woff"
        | "application/font-woff2"
        | "application/x-font-woff"
        | "application/x-font-woff2"
        | "application/x-font-ttf"
        | "application/x-font-truetype"
        | "application/x-font-opentype"
        | "application/x-font-otf"
        | "application/font-sfnt"
        | "application/vnd.ms-fontobject"
        | "application/font-ttf"
        | "application/font-otf" => Some(C::Font),
        "application/ogg" => Some(C::Audio),
        "application/mp4" => Some(C::Video),
        "application/x-sh"
        | "application/sql"
        | "application/toml"
        | "application/x-httpd-php"
        | "application/vnd.apple.mpegurl"
        | "application/x-mpegurl"
        | "application/x-subrip"
        | "application/x-pem-file"
        | "application/x-perl"
        | "application/x-python"
        | "application/x-ruby"
        | "application/rtf"
        | "application/x-tex" => Some(C::Text),
        "application/octet-stream"
        | "binary/octet-stream"
        | "application/binary"
        | "application/x-binary"
        | "application/zip"
        | "application/gzip"
        | "application/x-gzip"
        | "application/x-tar"
        | "application/x-bzip2"
        | "application/x-7z-compressed"
        | "application/x-rar-compressed"
        | "application/vnd.rar"
        | "application/zstd"
        | "application/x-xz"
        | "application/java-archive"
        | "application/vnd.android.package-archive"
        | "application/x-msdownload"
        | "application/x-apple-diskimage"
        | "application/x-iso9660-image"
        | "application/vnd.ms-cab-compressed"
        | "application/x-shockwave-flash"
        | "application/x-sqlite3"
        | "application/vnd.sqlite3" => Some(C::Binary),
        _ => None,
    };
    if let Some(k) = exact {
        return k;
    }
    if ty == "application" {
        if let Some(rest) = sub.strip_prefix("grpc-web")
            && (rest.is_empty() || rest.starts_with('+') || rest.starts_with("-text"))
        {
            return C::GrpcWeb;
        }
        if let Some(rest) = sub.strip_prefix("grpc")
            && (rest.is_empty() || rest.starts_with('+') || rest == "-proto" || rest == "-protobuf")
        {
            return C::Grpc;
        }
    }
    if let Some((_, suffix)) = sub.rsplit_once('+') {
        match suffix {
            "json" => return C::Json,
            "json-seq" | "jsonl" | "ndjson" => return C::JsonLines,
            "xml" => return C::Xml,
            "yaml" => return C::Yaml,
            "proto" | "protobuf" => return C::Protobuf,
            "msgpack" => return C::MsgPack,
            "cbor" | "cbor-seq" => return C::Cbor,
            "zip" | "gzip" | "zstd" => return C::Binary,
            _ => {}
        }
    }
    if ty == "text" {
        return C::Text;
    }
    C::Unknown
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// No / generic / unrecognized declared type.
    Untyped,
    /// A `text/*` type without a more specific kind.
    PlainText,
}

const TEXT_SNIFF: usize = 8 * 1024;
const JSON_SNIFF: usize = 64 * 1024;
const RECORDS_SNIFF: usize = 16 * 1024;

fn sniff(body: &[u8], path: &str, mode: Mode) -> ContentKind {
    let fallback_text = if mode == Mode::PlainText {
        ContentKind::Text
    } else {
        ContentKind::Unknown
    };
    if body.is_empty() {
        return ext_hint(path).unwrap_or(fallback_text);
    }
    if let Some(k) = sniff_magic(body) {
        return k;
    }
    // UTF-16 text (with BOM): analyze a transcoded prefix.
    if body.starts_with(&[0xFF, 0xFE]) || body.starts_with(&[0xFE, 0xFF]) {
        let enc = if body[0] == 0xFF {
            encoding_rs::UTF_16LE
        } else {
            encoding_rs::UTF_16BE
        };
        let prefix = &body[2..body.len().min(TEXT_SNIFF * 2)];
        let (text, _) = enc.decode_without_bom_handling(prefix);
        return sniff_text(text.as_bytes(), path, mode);
    }
    let class = text_class(body);
    if class != TextClass::Binary {
        // Text-looking bytes can still be protobuf: a message whose first field is a string of
        // ~100 printable characters (`0a 64 <text>`) is valid UTF-8 with at most one control
        // character. Like HTTP Toolkit (which prefers protobuf for generic types when the whole
        // body validates), check untyped UTF-8 bodies with a protobuf-like first byte, and any
        // legacy 8-bit "text", before settling on text.
        let maybe_protobuf =
            class == TextClass::Legacy || (mode == Mode::Untyped && protobuf::quick_check(body));
        if !(maybe_protobuf && looks_like_protobuf(body)) {
            return sniff_text(body, path, mode);
        }
        return ContentKind::Protobuf;
    }
    if looks_like_grpc(body) {
        return ContentKind::Grpc;
    }
    if looks_like_protobuf(body) {
        return ContentKind::Protobuf;
    }
    // A binary body: only binary-format extensions are believable (`.svg` is text).
    match ext_hint(path) {
        Some(k) if !is_textual(k) && path_extension(path).as_deref() != Some("svg") => k,
        _ => ContentKind::Binary,
    }
}

fn sniff_text(body: &[u8], path: &str, mode: Mode) -> ContentKind {
    let t = skip_bom_ws(body);
    if t.is_empty() {
        return ext_hint(path)
            .filter(|k| is_textual(*k))
            .unwrap_or(ContentKind::Text);
    }
    if is_probably_json_records(body) {
        return ContentKind::JsonLines;
    }
    if matches!(t[0], b'{' | b'[')
        && let Some(k) = sniff_json(t)
    {
        return k;
    }
    if t[0] == b'<'
        && let Some(k) = sniff_markup(t)
    {
        return k;
    }
    if mode == Mode::Untyped && looks_like_sse(t) {
        return ContentKind::EventStream;
    }
    if let Some(k) = ext_hint(path)
        && is_textual(k)
    {
        // Don't trust a .json/.html/.xml extension the body contradicts: those were
        // already checked above; still prefer it for e.g. JSON with comments.
        return k;
    }
    if mode == Mode::Untyped && looks_like_form(t) {
        return ContentKind::FormUrlEncoded;
    }
    ContentKind::Text
}

/// Declared HTML: keep unless the body is clearly not markup.
fn refine_html(body: &[u8]) -> ContentKind {
    let t = skip_bom_ws(body);
    if t.is_empty() || t[0] == b'<' {
        return ContentKind::Html;
    }
    if matches!(t[0], b'{' | b'[') && sniff_json(t).is_some() {
        return ContentKind::Json;
    }
    // An HTML fragment that starts with text but contains tags stays HTML.
    let head = &t[..t.len().min(1024)];
    let has_tag = memchr::memchr_iter(b'<', head).any(|i| {
        head.get(i + 1)
            .is_some_and(|c| c.is_ascii_alphabetic() || *c == b'/' || *c == b'!')
    });
    if has_tag {
        ContentKind::Html
    } else {
        ContentKind::Text
    }
}

/// Declared JSON: respect it, except for sequences of values (records) and markup error pages.
fn refine_json(body: &[u8]) -> ContentKind {
    let t = skip_bom_ws(body);
    match t.first() {
        None => ContentKind::Json,
        Some(0x1e) => ContentKind::JsonLines,
        Some(b'<') => sniff_markup(t).unwrap_or(ContentKind::Json),
        Some(b'{' | b'[') => {
            // A valid JSON document can never contain `}`/`]` + newline + `{`/`[` outside a
            // string (strings can't hold raw newlines), so this pattern means several values.
            if has_record_boundary(&t[..t.len().min(RECORDS_SNIFF)]) {
                ContentKind::JsonLines
            } else {
                ContentKind::Json
            }
        }
        _ => ContentKind::Json,
    }
}

fn refine_form(body: &[u8]) -> ContentKind {
    let t = skip_bom_ws(body);
    match t.first() {
        Some(b'{' | b'[') if sniff_json(t).is_some() => ContentKind::Json,
        Some(b'<') => sniff_markup(t).unwrap_or(ContentKind::FormUrlEncoded),
        _ => ContentKind::FormUrlEncoded,
    }
}

/// `t` starts with `{` or `[`. Parses the prefix leniently: a first value that parses, or a
/// parse that only fails because the prefix ends, counts. A second value after a newline
/// means JSON lines.
fn sniff_json(t: &[u8]) -> Option<ContentKind> {
    let prefix = &t[..t.len().min(JSON_SNIFF)];
    let mut it = serde_json::Deserializer::from_slice(prefix).into_iter::<serde::de::IgnoredAny>();
    match it.next() {
        Some(Ok(_)) => {
            let offset = it.byte_offset();
            let rest = &prefix[offset..];
            let gap_end = rest
                .iter()
                .position(|c| !c.is_ascii_whitespace())
                .unwrap_or(rest.len());
            let newline_gap = rest[..gap_end].contains(&b'\n');
            if newline_gap && matches!(rest.get(gap_end), Some(b'{' | b'[')) {
                Some(ContentKind::JsonLines)
            } else {
                Some(ContentKind::Json)
            }
        }
        Some(Err(e)) if e.is_eof() => Some(ContentKind::Json),
        Some(Err(e)) if e.to_string().starts_with("recursion limit exceeded") => {
            Some(ContentKind::Json)
        }
        _ => None,
    }
}

/// HTTP Toolkit's JSON-records heuristics (RFC 7464 text sequences, SignalR frames, NDJSON).
fn is_probably_json_records(body: &[u8]) -> bool {
    if body.len() < 3 {
        return false;
    }
    let start = &body[..body.len().min(6)];
    let end = &body[body.len().saturating_sub(6)..];
    let st = skip_ascii_ws(start);
    if start[0] == 0x1e && matches!(skip_ascii_ws(&start[1..]).first(), Some(b'{' | b'[')) {
        return true;
    }
    let end_trim = trim_ascii_ws_end(end);
    if st.first() == Some(&b'{')
        && end_trim.last() == Some(&0x1e)
        && trim_ascii_ws_end(&end_trim[..end_trim.len() - 1]).last() == Some(&b'}')
    {
        return true; // SignalR
    }
    if matches!(st.first(), Some(b'{' | b'[')) {
        // ends with `}`/`]` followed by newline(s)
        let ends_record = {
            let mut i = end.len();
            let mut saw_nl = false;
            while i > 0 && matches!(end[i - 1], b'\n' | b'\r') {
                saw_nl |= end[i - 1] == b'\n';
                i -= 1;
            }
            saw_nl && i > 0 && matches!(end[i - 1], b'}' | b']')
        };
        if ends_record && has_record_boundary(&body[..body.len().min(RECORDS_SNIFF)]) {
            return true;
        }
    }
    false
}

/// `[}\]](\r?\n)+[{\[]` somewhere in `b`.
fn has_record_boundary(b: &[u8]) -> bool {
    for i in memchr::memchr_iter(b'\n', b) {
        // walk back over \r\n runs
        let mut j = i;
        while j > 0 && matches!(b[j - 1], b'\r' | b'\n') {
            j -= 1;
        }
        if j == 0 || !matches!(b[j - 1], b'}' | b']') {
            continue;
        }
        let mut k = i + 1;
        while k < b.len() && matches!(b[k], b'\r' | b'\n') {
            k += 1;
        }
        if matches!(b.get(k), Some(b'{' | b'[')) {
            return true;
        }
    }
    false
}

fn skip_ascii_ws(b: &[u8]) -> &[u8] {
    let i = b
        .iter()
        .position(|c| !c.is_ascii_whitespace())
        .unwrap_or(b.len());
    &b[i..]
}

fn trim_ascii_ws_end(b: &[u8]) -> &[u8] {
    let i = b
        .iter()
        .rposition(|c| !c.is_ascii_whitespace())
        .map_or(0, |i| i + 1);
    &b[..i]
}

/// WHATWG MIME sniffing "HTML" patterns (plus a few common ones), each followed by a
/// tag-terminating byte.
const HTML_TAGS: &[&[u8]] = &[
    b"html",
    b"head",
    b"script",
    b"iframe",
    b"h1",
    b"div",
    b"font",
    b"table",
    b"a",
    b"style",
    b"title",
    b"b",
    b"body",
    b"br",
    b"p",
    b"meta",
    b"link",
    b"span",
    b"form",
    b"img",
    b"ul",
    b"ol",
    b"li",
    b"section",
    b"header",
    b"footer",
    b"main",
    b"nav",
    b"article",
    b"h2",
    b"h3",
    b"button",
    b"input",
    b"noscript",
    b"template",
    b"center",
];

/// Classify markup starting with `<`: Html, Xml, Image (svg) or None (not markup).
fn sniff_markup(t: &[u8]) -> Option<ContentKind> {
    let head: Vec<u8> = t[..t.len().min(4096)]
        .iter()
        .map(u8::to_ascii_lowercase)
        .collect();
    let mut i = 0;
    let mut xml_decl = false;
    loop {
        while i < head.len() && head[i].is_ascii_whitespace() {
            i += 1;
        }
        let rest = &head[i..];
        if rest.is_empty() {
            return Some(if xml_decl {
                ContentKind::Xml
            } else {
                ContentKind::Html
            });
        }
        if rest[0] != b'<' {
            return Some(if xml_decl {
                ContentKind::Xml
            } else {
                ContentKind::Html
            });
        }
        if rest.starts_with(b"<?xml") {
            xml_decl = true;
            i += memchr::memmem::find(rest, b"?>").map_or(rest.len(), |e| e + 2);
            continue;
        }
        if rest.starts_with(b"<?") {
            i += memchr::memmem::find(rest, b"?>").map_or(rest.len(), |e| e + 2);
            continue;
        }
        if rest.starts_with(b"<!--") {
            match memchr::memmem::find(&rest[4..], b"-->") {
                Some(e) => {
                    i += 4 + e + 3;
                    continue;
                }
                None => {
                    return Some(if xml_decl {
                        ContentKind::Xml
                    } else {
                        ContentKind::Html
                    });
                }
            }
        }
        if rest.starts_with(b"<!doctype") {
            let name = skip_ascii_ws(&rest[9..]);
            if name.starts_with(b"html") {
                return Some(ContentKind::Html);
            }
            if name.starts_with(b"svg") {
                return Some(ContentKind::Image);
            }
            i += memchr::memchr(b'>', rest).map_or(rest.len(), |e| e + 1);
            continue;
        }
        let name_end = rest[1..]
            .iter()
            .position(|c| !(c.is_ascii_alphanumeric() || matches!(c, b':' | b'-' | b'_' | b'.')))
            .map_or(rest.len(), |p| p + 1);
        let name = &rest[1..name_end];
        if name.is_empty() || !name[0].is_ascii_alphabetic() {
            return None;
        }
        let terminated = matches!(
            rest.get(name_end),
            None | Some(b' ' | b'>' | b'/' | b'\t' | b'\n' | b'\r' | 0x0c)
        );
        if name == b"svg" && terminated {
            return Some(ContentKind::Image);
        }
        if !xml_decl && terminated && HTML_TAGS.contains(&name) {
            return Some(ContentKind::Html);
        }
        return Some(if name == b"html" {
            ContentKind::Html
        } else {
            ContentKind::Xml
        });
    }
}

/// Valid UTF-8 (allowing a sequence cut at the end of the prefix) or legacy 8-bit text, with
/// no NUL and few control characters.
pub(crate) fn looks_like_text(body: &[u8]) -> bool {
    text_class(body) != TextClass::Binary
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TextClass {
    /// Valid UTF-8 (a sequence cut at the end of the sniffed prefix is fine), no NUL, at most
    /// 1% control characters.
    Utf8,
    /// Not UTF-8, but no NUL or control characters at all: probably a legacy 8-bit charset.
    Legacy,
    Binary,
}

/// Classify the first [`TEXT_SNIFF`] bytes.
fn text_class(body: &[u8]) -> TextClass {
    let p = &body[..body.len().min(TEXT_SNIFF)];
    if p.is_empty() {
        return TextClass::Utf8;
    }
    let mut control = 0usize;
    for &b in p {
        match b {
            0 => return TextClass::Binary,
            0x01..=0x08 | 0x0b | 0x0e..=0x1a | 0x1c..=0x1f | 0x7f => control += 1,
            _ => {}
        }
    }
    if control * 100 > p.len() {
        return TextClass::Binary;
    }
    match std::str::from_utf8(p) {
        Ok(_) => TextClass::Utf8,
        // truncated sequence at the end of the prefix
        Err(e) if e.error_len().is_none() => TextClass::Utf8,
        Err(_) if control == 0 => TextClass::Legacy,
        Err(_) => TextClass::Binary,
    }
}

fn looks_like_sse(t: &[u8]) -> bool {
    let first_line_end = memchr::memchr(b'\n', t).unwrap_or(t.len());
    let first = &t[..first_line_end];
    let field_ok = [&b"data:"[..], b"event:", b"id:", b"retry:"]
        .iter()
        .any(|f| first.starts_with(f));
    let prefix = &t[..t.len().min(TEXT_SNIFF)];
    field_ok
        && (memchr::memmem::find(prefix, b"\n\n").is_some()
            || memchr::memmem::find(prefix, b"\r\n\r\n").is_some())
}

fn looks_like_form(t: &[u8]) -> bool {
    let p = &t[..t.len().min(TEXT_SNIFF)];
    p.len() >= 3
        && p[0] != b'='
        && p[0] != b'&'
        && p.contains(&b'=')
        && p.iter()
            .all(|&c| c.is_ascii_alphanumeric() || b"%+.-_~*=&;,:/!$'()@[]".contains(&c))
}

/// Bytes of top-level protobuf field headers examined when sniffing (every length is still
/// checked against the whole message, see [`protobuf::wire_valid`]).
const PROTOBUF_SCAN: usize = 1 << 20;

fn looks_like_grpc(body: &[u8]) -> bool {
    if body.len() < 7 {
        return false;
    }
    let mut off = 0usize;
    let mut first = true;
    while off < body.len() {
        let Some(h) = body.get(off..off + 5) else {
            return false;
        };
        let flags = h[0];
        if flags != 0 && flags != 0x80 {
            return false;
        }
        let len = u32::from_be_bytes([h[1], h[2], h[3], h[4]]) as usize;
        let start = off + 5;
        let Some(msg) = start.checked_add(len).and_then(|e| body.get(start..e)) else {
            return false;
        };
        if first {
            if flags != 0 || len < 2 || !protobuf::quick_check(msg) {
                return false;
            }
            if !protobuf::wire_valid(msg, PROTOBUF_SCAN) {
                return false;
            }
            first = false;
        }
        off = start + len;
    }
    true
}

fn looks_like_protobuf(body: &[u8]) -> bool {
    protobuf::quick_check(body) && protobuf::wire_valid(body, PROTOBUF_SCAN)
}

/// Magic-number sniffing for binary formats.
fn sniff_magic(b: &[u8]) -> Option<ContentKind> {
    use ContentKind as C;
    let starts = |m: &[u8]| b.starts_with(m);
    if starts(b"\x89PNG\r\n\x1a\n")
        || starts(&[0xFF, 0xD8, 0xFF])
        || starts(b"GIF87a")
        || starts(b"GIF89a")
    {
        return Some(C::Image);
    }
    if starts(b"II*\0")
        || starts(b"MM\0*")
        || starts(&[0xFF, 0x0A])
        || starts(b"\0\0\0\x0cJXL \r\n\x87\n")
    {
        return Some(C::Image);
    }
    if starts(b"RIFF") && b.len() >= 12 {
        return match &b[8..12] {
            b"WEBP" => Some(C::Image),
            b"WAVE" => Some(C::Audio),
            b"AVI " => Some(C::Video),
            _ => Some(C::Binary),
        };
    }
    if b.len() >= 12 && &b[4..8] == b"ftyp" {
        let brand = &b[8..12];
        if crate::image::info(b).is_some() {
            return Some(C::Image);
        }
        return Some(match brand {
            b"M4A " | b"M4B " | b"M4P " | b"F4A " | b"F4B " => C::Audio,
            _ => C::Video,
        });
    }
    if b.len() >= 6 && b[0] == 0 && b[1] == 0 && (b[2] == 1 || b[2] == 2) && b[3] == 0 {
        let count = u16::from_le_bytes([b[4], b[5]]);
        if count > 0 && count <= 512 && b.get(9).is_none_or(|&r| r == 0) {
            return Some(C::Image);
        }
    }
    if starts(b"BM") && crate::image::info(b).is_some() {
        return Some(C::Image);
    }
    if starts(b"%PDF-") {
        return Some(C::Pdf);
    }
    if starts(b"\0asm") {
        return Some(C::Wasm);
    }
    if starts(b"wOFF") || starts(b"wOF2") || starts(b"OTTO") || starts(b"ttcf") {
        return Some(C::Font);
    }
    if starts(&[0, 1, 0, 0]) && b.len() >= 12 {
        let tables = u16::from_be_bytes([b[4], b[5]]);
        if (1..=64).contains(&tables) {
            return Some(C::Font);
        }
    }
    if starts(b"ID3") || starts(b"OggS") || starts(b"fLaC") || starts(b"#!AMR") || starts(b"MThd") {
        return Some(C::Audio);
    }
    if b.len() >= 2 && b[0] == 0xFF && matches!(b[1], 0xFB | 0xF3 | 0xF2 | 0xFA | 0xF1 | 0xF9) {
        return Some(C::Audio); // MPEG audio frame sync / ADTS
    }
    if starts(&[0x1A, 0x45, 0xDF, 0xA3]) || starts(b"FLV\x01") || starts(&[0, 0, 1, 0xBA]) {
        return Some(C::Video);
    }
    if b.len() > 376 && b[0] == 0x47 && b[188] == 0x47 && b[376] == 0x47 {
        return Some(C::Video); // MPEG-TS
    }
    if starts(b"PK\x03\x04")
        || starts(b"PK\x05\x06")
        || starts(b"PK\x07\x08")
        || starts(&[0x1f, 0x8b])
        || starts(&[0x28, 0xb5, 0x2f, 0xfd])
        || starts(b"\xFD7zXZ\0")
        || starts(b"7z\xBC\xAF\x27\x1C")
        || starts(b"Rar!\x1a\x07")
        || starts(b"\x7fELF")
        || starts(&[0xCA, 0xFE, 0xBA, 0xBE])
        || starts(b"SQLite format 3\0")
        || (starts(b"BZh") && b.get(3).is_some_and(|c| (b'1'..=b'9').contains(c)))
    {
        return Some(C::Binary);
    }
    None
}

/// Kind suggested by the URL path's file extension.
fn ext_hint(path: &str) -> Option<ContentKind> {
    use ContentKind as C;
    let ext = path_extension(path)?;
    Some(match ext.as_str() {
        "json" | "map" | "har" | "geojson" | "webmanifest" | "jsonc" | "json5" => C::Json,
        "ndjson" | "jsonl" => C::JsonLines,
        "xml" | "rss" | "atom" | "xsd" | "xsl" | "xslt" | "wsdl" | "kml" | "gpx" | "plist"
        | "xliff" => C::Xml,
        "html" | "htm" | "xhtml" | "shtml" => C::Html,
        "js" | "mjs" | "cjs" | "jsx" | "ts" | "tsx" | "mts" | "cts" => C::Javascript,
        "css" => C::Css,
        "md" | "markdown" => C::Markdown,
        "yaml" | "yml" => C::Yaml,
        "csv" | "tsv" => C::Csv,
        "txt" | "log" | "text" | "ini" | "conf" | "cfg" | "toml" | "sh" | "py" | "rb" | "go"
        | "rs" | "java" | "c" | "h" | "cpp" | "m3u8" | "m3u" | "vtt" | "srt" | "sql" | "pem"
        | "crt" => C::Text,
        "graphql" | "gql" => C::GraphQl,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "avif" | "ico" | "bmp" | "svg" | "tif"
        | "tiff" | "heic" | "heif" | "jxl" | "cur" | "apng" => C::Image,
        "mp3" | "wav" | "ogg" | "oga" | "opus" | "flac" | "aac" | "m4a" | "weba" | "mid"
        | "midi" => C::Audio,
        "mp4" | "m4v" | "webm" | "mkv" | "mov" | "avi" | "ogv" | "m4s" | "flv" | "3gp" => C::Video,
        "woff" | "woff2" | "ttf" | "otf" | "eot" | "ttc" => C::Font,
        "pdf" => C::Pdf,
        "wasm" => C::Wasm,
        "pb" | "protobuf" => C::Protobuf,
        "msgpack" | "mpk" => C::MsgPack,
        "cbor" => C::Cbor,
        "zip" | "gz" | "tgz" | "tar" | "7z" | "rar" | "zst" | "br" | "bz2" | "xz" | "exe"
        | "dll" | "so" | "dmg" | "apk" | "jar" | "deb" | "rpm" | "iso" | "bin" => C::Binary,
        _ => return None,
    })
}

/// `/graphql`, `/api/graphql`, `/v1/graphql/...`, `/gql`.
fn is_graphql_path(path: &str) -> bool {
    path_only(path)
        .split('/')
        .any(|seg| seg.eq_ignore_ascii_case("graphql") || seg.eq_ignore_ascii_case("gql"))
}

/// JSON body with a non-empty string `query` field (or a batch array of such operations).
fn has_graphql_query(body: &[u8]) -> bool {
    #[derive(serde::Deserialize)]
    struct Op {
        #[serde(default)]
        query: Option<serde_json::Value>,
    }
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum Req {
        One(Op),
        Batch(Vec<Op>),
    }
    let has = |op: &Op| {
        op.query
            .as_ref()
            .and_then(|q| q.as_str())
            .is_some_and(|q| !q.trim().is_empty())
    };
    if body.len() > 4 << 20 {
        let head = &body[..4096];
        return memchr::memmem::find(head, b"\"query\"").is_some();
    }
    match serde_json::from_slice::<Req>(body) {
        Ok(Req::One(op)) => has(&op),
        Ok(Req::Batch(ops)) => !ops.is_empty() && ops.iter().any(has),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ContentKind as C;

    fn d(ct: Option<&str>, body: &[u8], path: &str) -> ContentKind {
        detect_kind(ct, body, path)
    }

    #[test]
    fn content_type_table() {
        let table: &[(&str, ContentKind)] = &[
            ("application/json", C::Json),
            ("application/json; charset=utf-8", C::Json),
            ("Application/JSON", C::Json),
            ("text/json", C::Json),
            ("application/problem+json", C::Json),
            ("application/vnd.api+json", C::Json),
            ("application/ld+json", C::Json),
            ("application/x-amz-json-1.1", C::Json),
            ("application/csp-report", C::Json),
            ("application/x-ndjson", C::JsonLines),
            ("application/jsonl", C::JsonLines),
            ("application/json-seq", C::JsonLines),
            ("application/stream+json", C::JsonLines),
            ("application/xml", C::Xml),
            ("text/xml", C::Xml),
            ("application/rss+xml", C::Xml),
            ("application/atom+xml", C::Xml),
            ("application/soap+xml", C::Xml),
            ("text/html", C::Html),
            ("application/xhtml+xml", C::Html),
            ("application/javascript", C::Javascript),
            ("text/javascript", C::Javascript),
            ("application/x-javascript", C::Javascript),
            ("application/ecmascript", C::Javascript),
            ("text/css", C::Css),
            ("text/markdown", C::Markdown),
            ("application/yaml", C::Yaml),
            ("text/x-yaml", C::Yaml),
            ("application/vnd.foo+yaml", C::Yaml),
            ("text/csv", C::Csv),
            ("text/plain", C::Text),
            ("text/x-python", C::Text),
            ("text/vtt", C::Text),
            ("application/x-www-form-urlencoded", C::FormUrlEncoded),
            ("multipart/form-data; boundary=x", C::Multipart),
            ("multipart/mixed", C::Multipart),
            ("image/png", C::Image),
            ("image/svg+xml", C::Image),
            ("audio/mpeg", C::Audio),
            ("video/mp4", C::Video),
            ("font/woff2", C::Font),
            ("application/font-woff", C::Font),
            ("application/vnd.ms-fontobject", C::Font),
            ("application/pdf", C::Pdf),
            ("application/wasm", C::Wasm),
            ("application/x-protobuf", C::Protobuf),
            ("application/protobuf", C::Protobuf),
            ("application/vnd.google.protobuf", C::Protobuf),
            ("application/vnd.foo+proto", C::Protobuf),
            ("application/grpc", C::Grpc),
            ("application/grpc+proto", C::Grpc),
            ("application/grpc+json", C::Grpc),
            ("application/grpc-web", C::GrpcWeb),
            ("application/grpc-web+proto", C::GrpcWeb),
            ("application/grpc-web-text", C::GrpcWeb),
            ("application/grpc-web-text+proto", C::GrpcWeb),
            ("application/msgpack", C::MsgPack),
            ("application/x-msgpack", C::MsgPack),
            ("application/cbor", C::Cbor),
            ("text/event-stream", C::EventStream),
            ("application/graphql", C::GraphQl),
            ("application/octet-stream", C::Binary),
            ("application/zip", C::Binary),
            ("application/vnd.ms-excel", C::Unknown),
            ("garbage", C::Unknown),
            ("", C::Unknown),
        ];
        for (ct, kind) in table {
            assert_eq!(kind_from_content_type(ct), *kind, "{ct}");
        }
    }

    #[test]
    fn header_wins_when_specific() {
        assert_eq!(d(Some("text/css"), b"{\"a\":1}", "/x"), C::Css);
        assert_eq!(d(Some("image/png"), b"not really", "/x"), C::Image);
        assert_eq!(
            d(Some("application/javascript"), b"{\"a\":1}", "/x"),
            C::Javascript
        );
        assert_eq!(
            d(Some("application/x-protobuf"), b"anything", "/x"),
            C::Protobuf
        );
        assert_eq!(d(Some("text/csv"), b"a,b\n1,2", "/x.json"), C::Csv);
    }

    #[test]
    fn declared_json_is_respected() {
        // Regression (HTK): JSON records were preferred even when declared json.
        assert_eq!(
            d(Some("application/json"), b"[{\"a\":1},\n{\"b\":2}]\n", "/"),
            C::Json
        );
        assert_eq!(d(Some("application/json"), b"{\"a\":1}\n", "/"), C::Json);
        // ...but a real NDJSON body labelled json is JSON lines.
        assert_eq!(
            d(Some("application/json"), b"{\"a\":1}\n{\"b\":2}\n", "/"),
            C::JsonLines
        );
        assert_eq!(
            d(Some("application/json"), b"\x1e{\"a\":1}\n", "/"),
            C::JsonLines
        );
        // An HTML error page labelled JSON.
        assert_eq!(
            d(
                Some("application/json"),
                b"<!DOCTYPE html><html></html>",
                "/"
            ),
            C::Html
        );
        assert_eq!(d(Some("application/json"), b"", "/"), C::Json);
    }

    #[test]
    fn html_refinement() {
        assert_eq!(d(Some("text/html"), b"  <div>x</div>", "/"), C::Html);
        assert_eq!(
            d(
                Some("text/html; charset=utf-8"),
                b"{\"error\":\"nope\"}",
                "/"
            ),
            C::Json
        );
        assert_eq!(d(Some("text/html"), b"plain words only", "/"), C::Text);
        assert_eq!(d(Some("text/html"), b"Hello <b>world</b>", "/"), C::Html);
        assert_eq!(d(Some("text/html"), b"", "/"), C::Html);
    }

    #[test]
    fn sniffing_generic_types() {
        for ct in [
            None,
            Some("application/octet-stream"),
            Some("text/plain"),
            Some("binary/octet-stream"),
            Some("application/x-unknown-thing"),
        ] {
            assert_eq!(d(ct, b" {\"a\": [1, 2]}", "/"), C::Json, "{ct:?}");
            assert_eq!(d(ct, b"[1,2,3", "/"), C::Json, "{ct:?} truncated json");
            assert_eq!(d(ct, b"{\"a\":1}\n{\"a\":2}", "/"), C::JsonLines, "{ct:?}");
            assert_eq!(
                d(ct, b"<?xml version=\"1.0\"?><feed/>", "/"),
                C::Xml,
                "{ct:?}"
            );
            assert_eq!(
                d(ct, b"<!doctype html><title>x</title>", "/"),
                C::Html,
                "{ct:?}"
            );
            assert_eq!(d(ct, b"<HTML><BODY>", "/"), C::Html, "{ct:?}");
            assert_eq!(
                d(ct, b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>", "/"),
                C::Image,
                "{ct:?}"
            );
            assert_eq!(
                d(ct, b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR", "/"),
                C::Image,
                "{ct:?}"
            );
            assert_eq!(d(ct, b"%PDF-1.7\n", "/"), C::Pdf, "{ct:?}");
        }
        // Regression (HTK): a text body starting with a digit is not JSON.
        assert_eq!(d(Some("text/plain"), b"200 OK all good", "/"), C::Text);
        assert_eq!(d(Some("text/plain"), b"[INFO] started", "/"), C::Text);
        assert_eq!(d(None, b"hello world", "/"), C::Text);
        assert_eq!(d(None, b"<3 you", "/"), C::Text);
        assert_eq!(d(None, b"<rss version=\"2.0\">", "/"), C::Xml);
        assert_eq!(d(None, b"<!-- c --><html>", "/"), C::Html);
        assert_eq!(d(None, b"<soap:Envelope>", "/"), C::Xml);
    }

    #[test]
    fn magic_numbers() {
        let cases: &[(&[u8], ContentKind)] = &[
            (b"\xFF\xD8\xFF\xE0\0\x10JFIF", C::Image),
            (b"GIF89a\x01\0\x01\0", C::Image),
            (b"RIFF\0\0\0\0WEBPVP8 ", C::Image),
            (b"\0\0\0\x1cftypavif\0\0\0\0avifmif1miaf", C::Image),
            (b"\0\0\0\x18ftypisom\0\0\x02\0isomiso2", C::Video),
            (b"\0\0\0\x18ftypM4A \0\0\x02\0isomiso2", C::Audio),
            (b"\0\0\x01\0\x01\0\x10\x10\0\0\x01\0\x20\0", C::Image),
            (
                b"BM\0\0\0\0\0\0\0\0\0\0\0\0\x28\0\0\0\x10\0\0\0\x10\0\0\0",
                C::Image,
            ),
            (b"%PDF-1.4", C::Pdf),
            (b"\0asm\x01\0\0\0", C::Wasm),
            (b"PK\x03\x04\x14\0", C::Binary),
            (b"\x1f\x8b\x08\0\0\0", C::Binary),
            (b"\x28\xb5\x2f\xfd\0", C::Binary),
            (b"wOF2\0\x01\0\0", C::Font),
            (b"\0\x01\0\0\0\x0c\0\x80\0\x03\0\x40", C::Font),
            (b"ID3\x04\0\0", C::Audio),
            (b"OggS\0\x02", C::Audio),
            (b"\x1A\x45\xDF\xA3\x01", C::Video),
            (b"RIFF\0\0\0\0WAVEfmt ", C::Audio),
            (b"\x7fELF\x02\x01", C::Binary),
        ];
        for (body, kind) in cases {
            assert_eq!(d(None, body, "/"), *kind, "{body:?}");
            assert_eq!(
                d(Some("application/octet-stream"), body, "/"),
                *kind,
                "{body:?}"
            );
        }
    }

    #[test]
    fn binary_protobuf_and_grpc_sniffing() {
        // {1: "hello", 2: 150}
        let pb = [0x0a, 0x05, b'h', b'e', b'l', b'l', b'o', 0x10, 0x96, 0x01];
        assert_eq!(d(Some("application/octet-stream"), &pb, "/"), C::Protobuf);
        let mut grpc = vec![0, 0, 0, 0, pb.len() as u8];
        grpc.extend_from_slice(&pb);
        assert_eq!(d(None, &grpc, "/"), C::Grpc);
        // Random binary isn't protobuf.
        assert_eq!(
            d(None, &[0x0a, 0xff, 0x00, 0x13, 0x88, 0x01, 0x02], "/"),
            C::Binary
        );
        assert_eq!(
            d(None, &[0x00, 0x01, 0x02, 0x03, 0xfe, 0xff], "/"),
            C::Binary
        );
        // A message whose single field is a ~120-char string is valid UTF-8 with one control
        // character: still protobuf when untyped, text when declared text/plain.
        let mut text_pb = vec![0x0a, 120];
        text_pb.extend(std::iter::repeat_n(b'a', 120));
        assert_eq!(d(None, &text_pb, "/"), C::Protobuf);
        assert_eq!(
            d(Some("application/octet-stream"), &text_pb, "/"),
            C::Protobuf
        );
        assert_eq!(d(Some("text/plain"), &text_pb, "/"), C::Text);
        let mut field2 = vec![0x12, 110];
        field2.extend(std::iter::repeat_n(b'z', 110));
        assert_eq!(d(None, &field2, "/"), C::Protobuf);
        // Text that merely starts with a line break / tab stays text.
        assert_eq!(d(None, b"\nhello world\n", "/"), C::Text);
        assert_eq!(d(None, b"\tindented text", "/"), C::Text);
        assert_eq!(d(None, b"\r\nline", "/"), C::Text);
        // Legacy 8-bit bytes are checked for protobuf before being called text: {1: "é…"}
        // with a 2-byte length varint (no control characters at all).
        let mut legacy = vec![0x0a, 0xa0, 0x20];
        legacy.extend(std::iter::repeat_n(b'q', 0x20 << 7 | 0x20));
        assert_eq!(d(None, &legacy, "/"), C::Protobuf);
        assert_eq!(d(None, b"caf\xe9 au lait", "/"), C::Text);
        // Regression: large binary data whose first bytes happen to look like a field followed by
        // an oversized length field used to pass the prefix check (the length was only checked
        // against a 64 KB prefix, and a "truncated tail" was accepted).
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let mut noise = vec![0x08, 0x01, 0x12, 0xff, 0xff, 0x7f];
        while noise.len() < 2 << 20 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            noise.extend_from_slice(&state.to_le_bytes());
        }
        assert_eq!(d(None, &noise, "/blob"), C::Binary);
        // Large protobuf body: headers of the first 1 MB scanned, lengths checked against all.
        let mut big = Vec::new();
        for _ in 0..200_000 {
            big.extend_from_slice(&pb);
        }
        assert_eq!(d(None, &big, "/"), C::Protobuf);
    }

    #[test]
    fn path_extension_hints() {
        assert_eq!(
            d(None, b"function f(){}", "/static/app.js?v=3"),
            C::Javascript
        );
        assert_eq!(
            d(Some("text/plain"), b"function f(){}", "/raw/app.mjs"),
            C::Javascript
        );
        assert_eq!(
            d(Some("text/plain"), b"body { color: red }", "/a/site.css"),
            C::Css
        );
        assert_eq!(
            d(Some("text/plain"), b"# Title\n\ntext", "/README.md"),
            C::Markdown
        );
        assert_eq!(d(None, b"key: value", "/conf.yaml"), C::Yaml);
        assert_eq!(d(None, b"", "/font.woff2"), C::Font);
        assert_eq!(d(None, b"", "/x"), C::Unknown);
        assert_eq!(d(Some("text/plain"), b"", "/x"), C::Text);
        assert_eq!(
            d(
                None,
                b"\x01\x02\x03\x04binary\x05\x06\x07\x08\x09\x0b",
                "/f.woff"
            ),
            C::Font
        );
        // A binary body doesn't become JS because of a .ts (MPEG-TS) path.
        assert_eq!(
            d(
                None,
                b"\x01\x02\x03\x04\x05\x06\x07\x08\x0e\x0f",
                "/seg1.ts"
            ),
            C::Binary
        );
        // A text body ignores binary extensions.
        assert_eq!(d(None, b"plain text here", "/x.png"), C::Text);
        assert_eq!(d(None, b"{ // comment\n}", "/tsconfig.json"), C::Json);
    }

    #[test]
    fn untyped_text_shapes() {
        assert_eq!(
            d(None, b"data: {\"a\":1}\n\ndata: x\n\n", "/stream"),
            C::EventStream
        );
        assert_eq!(
            d(None, b"user=alice&pass=s3cr3t%21", "/login"),
            C::FormUrlEncoded
        );
        assert_eq!(
            d(Some("text/plain"), b"user=alice&pass=x", "/login"),
            C::Text
        );
        assert_eq!(
            d(Some("application/x-www-form-urlencoded"), b"{\"a\":1}", "/"),
            C::Json
        );
        assert_eq!(
            d(Some("application/x-www-form-urlencoded"), b"a=1", "/"),
            C::FormUrlEncoded
        );
    }

    #[test]
    fn charsets_and_utf16() {
        // latin1 text without a charset is still text
        assert_eq!(d(None, b"caf\xe9 cr\xe8me", "/"), C::Text);
        let utf16: Vec<u8> = [0xFF, 0xFE]
            .into_iter()
            .chain("{\"a\":1}".encode_utf16().flat_map(|u| u.to_le_bytes()))
            .collect();
        assert_eq!(d(None, &utf16, "/"), C::Json);
        // A multi-byte char cut at the sniff boundary is still text.
        let mut s = "é".repeat(TEXT_SNIFF);
        s.insert(0, 'a');
        assert_eq!(d(None, s.as_bytes(), "/"), C::Text);
    }

    #[test]
    fn graphql_detection() {
        let q = br#"{"query":"query { me { id } }","variables":{}}"#;
        assert_eq!(d(Some("application/json"), q, "/graphql"), C::GraphQl);
        assert_eq!(
            d(Some("application/json"), q, "/api/graphql?op=Me"),
            C::GraphQl
        );
        assert_eq!(d(Some("application/json"), q, "/gql"), C::GraphQl);
        assert_eq!(d(Some("application/json"), q, "/api/users"), C::Json);
        let batch = br#"[{"query":"{a}"},{"query":"{b}"}]"#;
        assert_eq!(d(Some("application/json"), batch, "/graphql"), C::GraphQl);
        // responses and persisted queries stay JSON
        assert_eq!(
            d(
                Some("application/json"),
                br#"{"data":{"me":null}}"#,
                "/graphql"
            ),
            C::Json
        );
        assert_eq!(
            d(
                Some("application/json"),
                br#"{"extensions":{"persistedQuery":{}}}"#,
                "/graphql"
            ),
            C::Json
        );
        assert_eq!(
            d(Some("application/graphql"), b"{ me { id } }", "/x"),
            C::GraphQl
        );
        assert_eq!(d(None, q, "/graphql"), C::GraphQl);
    }

    #[test]
    fn textual_kinds() {
        for k in [
            C::Json,
            C::JsonLines,
            C::Xml,
            C::Html,
            C::Javascript,
            C::Css,
            C::Markdown,
            C::Yaml,
            C::Csv,
            C::Text,
            C::FormUrlEncoded,
            C::EventStream,
            C::GraphQl,
        ] {
            assert!(is_textual(k), "{k:?}");
        }
        for k in [
            C::Image,
            C::Audio,
            C::Video,
            C::Font,
            C::Pdf,
            C::Wasm,
            C::Protobuf,
            C::Grpc,
            C::GrpcWeb,
            C::MsgPack,
            C::Cbor,
            C::Multipart,
            C::Binary,
            C::Unknown,
        ] {
            assert!(!is_textual(k), "{k:?}");
        }
    }
}
