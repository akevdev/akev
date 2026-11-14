//! Randomized robustness tests: every public entry point is fed random bytes and
//! "grammar soup" (random concatenations of syntax fragments). Nothing may panic or hang, and
//! the formatters must only ever change whitespace (output == input modulo whitespace).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use akev_content::{
    cbor, decode, detect, form, format, grpc, image, jwt, msgpack, multipart, protobuf, sse, text,
    validate, view_body,
};
use akev_core::{ContentKind, Headers};

/// xorshift64* — deterministic, dependency-free.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn bytes(&mut self, max: usize) -> Vec<u8> {
        let n = self.below(max + 1);
        (0..n).map(|_| self.next() as u8).collect()
    }
    fn soup(&mut self, fragments: &[&str], max_parts: usize) -> String {
        let n = self.below(max_parts + 1);
        (0..n)
            .map(|_| fragments[self.below(fragments.len())])
            .collect()
    }
}

const JSON: &[&str] = &[
    "{", "}", "[", "]", ",", ":", "\"", "\"k\"", "'s'", "\\", "\\\"", " ", "\n", "\r\n", "\t", "1",
    "-2.5e3", "true", "null", "//c\n", "/*", "*/", "\u{1e}", "é", "😀", "x", "\u{feff}",
];
const JS: &[&str] = &[
    "{", "}", "(", ")", "[", "]", ";", ",", ".", "?.", ":", "?", "!", "=", "==", "=>", "+", "++",
    "-", "--", "*", "/", "/=", "%", "<", ">", "&&", "||", "??", "'", "\"", "`", "${", "\\", " ",
    "\n", "\r", "\t", "a", "x1", "0", "1.5e-3", ".5", "return", "if", "else", "case", "default",
    "switch", "do", "while", "for", "function", "typeof", "async", "yield", "//c\n", "/*", "*/",
    "<!--", "-->", "#!", "é", "\u{2028}", "\u{a0}", "\u{feff}", "/re/g", "`t${a}`",
];
const HTML: &[&str] = &[
    "<",
    ">",
    "/",
    "!",
    "-",
    "=",
    "\"",
    "'",
    " ",
    "\n",
    "a",
    "div",
    "p",
    "li",
    "td",
    "tr",
    "<p>",
    "</p>",
    "<li>",
    "<ul>",
    "</ul>",
    "<br>",
    "<br/>",
    "<script>",
    "</script>",
    "<style>",
    "</style>",
    "<pre>",
    "</pre>",
    "<textarea>",
    "<!--",
    "-->",
    "<![CDATA[",
    "]]>",
    "<!DOCTYPE html>",
    "<b>",
    "</b>",
    "<html>",
    "</html>",
    "<table>",
    "<script type=\"x\">",
    "if(a){b()}",
    "a{b:c}",
    "é",
    "&amp;",
];
const XML: &[&str] = &[
    "<",
    ">",
    "/",
    "<a>",
    "</a>",
    "<b x='1'>",
    "</b>",
    "<c/>",
    "<?xml version=\"1.0\"?>",
    "<?pi x?>",
    "<!-- c -->",
    "<![CDATA[x<y]]>",
    "<!DOCTYPE r [<!ENTITY e \"v\">]>",
    "text",
    " ",
    "\n",
    "&amp;",
    "&",
    "\"",
    "é",
    "ns:t",
];
const CSS: &[&str] = &[
    "{",
    "}",
    "(",
    ")",
    ";",
    ":",
    ",",
    "\"",
    "'",
    "/*",
    "*/",
    " ",
    "\n",
    "a",
    ".c",
    "#i",
    "@media",
    "url(",
    "url(x)",
    "color",
    "red",
    "!important",
    "&",
    ">",
    "é",
    "\\",
    "\u{c}",
];
const GRAPHQL: &[&str] = &[
    "{", "}", "(", ")", "[", "]", ":", "=", "@", "$", "!", "|", "&", ",", "...", "\"", "\"\"\"",
    "\\", "#c\n", " ", "\n", "a", "on", "query", "type", "fragment", "1", "-2", "é", "\u{b}",
];
const SSE: &[&str] = &[
    "data:",
    "data: x",
    "event: e",
    "id: 1",
    "id",
    "retry: 10",
    "retry: x",
    ":",
    ": c",
    "\r",
    "\n",
    "\r\n",
    " ",
    "\u{feff}",
    "\0",
    "é",
];
const MULTIPART: &[&str] = &[
    "--b",
    "--b--",
    "\r\n",
    "\n",
    " ",
    "Content-Disposition: form-data; name=\"x\"",
    "; filename=\"f.txt\"",
    "; filename*=UTF-8''%E2%82%AC",
    "Content-Type: text/plain",
    ":",
    "data",
    "\0",
    "é",
];

/// Remove all whitespace (incl. BOM) for the "formatting only changes whitespace" invariant.
fn strip_ws(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace() && *c != '\u{feff}')
        .collect()
}

fn check_format(kind: ContentKind, input: &str) {
    if let Some(out) = format::pretty(kind, input) {
        assert_eq!(
            strip_ws(&out),
            strip_ws(input),
            "{kind:?} changed non-whitespace content of {input:?}\n→ {out:?}"
        );
    }
}

/// Iteration count; override with `AKEV_FUZZ_ITERS` for longer campaigns.
fn iterations() -> usize {
    std::env::var("AKEV_FUZZ_ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4000)
}

/// Seed offset; override with `AKEV_FUZZ_SEED`.
fn seed(base: u64) -> Rng {
    let extra: u64 = std::env::var("AKEV_FUZZ_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    Rng((base ^ extra.wrapping_mul(0x9e37_79b9_7f4a_7c15)) | 1)
}

#[test]
fn formatters_only_change_whitespace() {
    let mut rng = seed(0x9e37_79b9_7f4a_7c15);
    for _ in 0..iterations() {
        check_format(ContentKind::Json, &rng.soup(JSON, 40));
        check_format(ContentKind::Javascript, &rng.soup(JS, 40));
        check_format(ContentKind::Html, &rng.soup(HTML, 30));
        check_format(ContentKind::Xml, &rng.soup(XML, 30));
        check_format(ContentKind::Css, &rng.soup(CSS, 40));
        check_format(ContentKind::GraphQl, &rng.soup(GRAPHQL, 40));
        let lossy = String::from_utf8_lossy(&rng.bytes(200)).into_owned();
        for kind in [
            ContentKind::Json,
            ContentKind::Javascript,
            ContentKind::Html,
            ContentKind::Xml,
            ContentKind::Css,
            ContentKind::GraphQl,
        ] {
            check_format(kind, &lossy);
        }
        // records mode drops RS separators; just make sure it doesn't panic
        let _ = format::pretty(ContentKind::JsonLines, &rng.soup(JSON, 40));
        let _ = format::pretty(ContentKind::FormUrlEncoded, &lossy);
        let _ = format::minify_json(&rng.soup(JSON, 40));
    }
}

#[test]
fn validators_never_panic_and_agree() {
    let mut rng = seed(0x0bad_cafe_1234_5678);
    for _ in 0..iterations() {
        let s = rng.soup(JSON, 30);
        // The serde_json-based validator and the crate's own iterative strict validator agree.
        assert_eq!(
            validate::json(&s).is_none(),
            format::is_valid_json(&s),
            "{s:?}: {:?}",
            validate::json(&s)
        );
        let _ = validate::json_records(&s);
        let _ = validate::xml(&rng.soup(XML, 30));
        let lossy = String::from_utf8_lossy(&rng.bytes(200)).into_owned();
        for kind in [ContentKind::Json, ContentKind::JsonLines, ContentKind::Xml] {
            for issue in validate::validate(kind, &lossy) {
                assert!(issue.line >= 1 && issue.column >= 1);
            }
        }
    }
}

#[test]
fn decoders_never_panic() {
    let mut rng = seed(0x1234_5678_9abc_def1);
    let codings = ["gzip", "deflate", "br", "zstd", "base64"];
    let samples: Vec<(String, Vec<u8>)> = codings
        .iter()
        .map(|c| {
            let chain = vec![c.to_string()];
            (
                c.to_string(),
                decode::encode_body(b"hello hello hello world, a sample body", &chain)
                    .unwrap()
                    .to_vec(),
            )
        })
        .collect();
    for _ in 0..iterations() {
        let junk = rng.bytes(300);
        for (coding, sample) in &samples {
            let chain = vec![coding.clone()];
            let _ = decode::decode_body(&junk, &chain, 1 << 20);
            // mutate a valid stream
            let mut m = sample.clone();
            if !m.is_empty() {
                let i = rng.below(m.len());
                m[i] ^= 1 << rng.below(8);
                m.truncate(m.len() - rng.below(m.len()));
            }
            let _ = decode::decode_body(&m, &chain, 1 << 20);
            let _ = decode::decode_body_partial(&m, &chain, 1 << 20);
        }
        let _ = protobuf::decode_raw(&junk).map(|f| protobuf::to_json(&f));
        let _ = grpc::parse_messages(&junk, Some("gzip"), false);
        // junk inside a well-formed compressed frame reaches every decompressor
        let frame = |payload: &[u8]| {
            let mut f = vec![1u8];
            f.extend_from_slice(&(payload.len() as u32).to_be_bytes());
            f.extend_from_slice(payload);
            f
        };
        for enc in ["snappy", "gzip", "deflate", "zstd"] {
            let _ = grpc::parse_messages(&frame(&junk), Some(enc), false);
        }
        let mut snappy_stream = b"\xff\x06\x00\x00sNaPpY".to_vec();
        snappy_stream.extend_from_slice(&junk);
        let _ = grpc::parse_messages(&frame(&snappy_stream), Some("snappy"), false);
        let _ = grpc::parse_messages(&junk, None, true);
        let _ = msgpack::to_json(&junk);
        let _ = cbor::to_json(&junk);
        let _ = image::info(&junk);
        let _ = image::exif(&junk);
        // EXIF parsing behind real container / TIFF headers (junk IFDs, offsets, counts).
        for prefix in [
            &b"II*\0\x08\0\0\0"[..],
            b"MM\0*\0\0\0\x08",
            b"\xFF\xD8\xFF\xE1\x01\x00Exif\0\0II*\0\x08\0\0\0",
            b"\x89PNG\r\n\x1a\n\0\0\x01\0eXIfMM\0*\0\0\0\x08",
        ] {
            let mut b = prefix.to_vec();
            b.extend_from_slice(&junk);
            let _ = image::exif(&b);
            let _ = image::info(&b);
        }
        let _ = sse::parse_all(&junk);
        let _ = text::decode_text(&junk, Some("text/html"));
        let _ =
            text::decode_text_prefix(&junk, Some("text/plain; charset=shift_jis"), rng.below(64));
        let s = String::from_utf8_lossy(&junk);
        let _ = form::parse_urlencoded(&s);
        let _ = jwt::decode(&s);
        let _ = multipart::parse(&junk, "multipart/form-data; boundary=b");
    }
}

#[test]
fn structured_soups_never_panic() {
    let mut rng = seed(0xdead_beef_cafe_f00d);
    for _ in 0..iterations() {
        let sse_body = rng.soup(SSE, 30);
        let events = sse::parse_all(sse_body.as_bytes());
        // incremental parsing agrees with whole-body parsing
        let mut p = sse::SseParser::new();
        let mut got = Vec::new();
        let step = 1 + rng.below(7);
        for chunk in sse_body.as_bytes().chunks(step) {
            got.extend(p.push(chunk));
        }
        got.extend(p.finish());
        assert_eq!(got, events, "{sse_body:?}");

        let mp = rng.soup(MULTIPART, 30);
        if let Ok(parts) = multipart::parse(mp.as_bytes(), "multipart/form-data; boundary=b") {
            for part in parts {
                assert_eq!(part.size, part.data.len());
            }
        }

        let jsonish = rng.soup(JSON, 30);
        for ct in [
            None,
            Some("application/json"),
            Some("text/plain"),
            Some("application/octet-stream"),
            Some("text/html"),
        ] {
            let _ = detect::detect_kind(ct, jsonish.as_bytes(), "/graphql");
        }
        let junk = rng.bytes(200);
        let mut h = Headers::new();
        let cts = [
            "application/json",
            "text/html",
            "multipart/form-data; boundary=b",
            "application/grpc-web-text",
            "application/grpc",
            "image/svg+xml",
            "text/event-stream",
            "application/x-www-form-urlencoded",
            "application/msgpack",
            "application/cbor",
            "application/x-protobuf",
        ];
        h.append("content-type", cts[rng.below(cts.len())]);
        if rng.below(3) == 0 {
            h.append(
                "content-encoding",
                ["gzip", "br", "deflate", "zstd", "x-unknown"][rng.below(5)],
            );
        }
        let _ = view_body(&junk, &h, "/x", 1 + rng.below(100));
        let _ = view_body(mp.as_bytes(), &h, "/x", 1 + rng.below(100));
    }
}
