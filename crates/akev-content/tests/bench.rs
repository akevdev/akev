//! Criterion-free micro-benchmarks. Run with:
//! `cargo test -p akev-content --release --test bench -- --ignored --nocapture`
//!
//! Each benchmark prints its throughput; in release builds it also asserts a generous upper
//! bound so performance regressions (e.g. accidentally quadratic code) fail loudly.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::{Duration, Instant};

use akev_content::{decode, detect, format, protobuf, view_body};
use akev_core::{ContentKind, Headers};

fn time<T>(label: &str, bytes: usize, limit: Duration, f: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let out = f();
    let elapsed = start.elapsed();
    let mbps = bytes as f64 / 1e6 / elapsed.as_secs_f64().max(1e-9);
    println!(
        "{label:<40} {:>8.1} MB in {:>8.2?}  ({mbps:>7.1} MB/s)",
        bytes as f64 / 1e6,
        elapsed
    );
    if !cfg!(debug_assertions) {
        assert!(
            elapsed < limit,
            "{label} took {elapsed:?} (limit {limit:?})"
        );
    }
    out
}

fn big_json(target: usize) -> String {
    let mut s = String::with_capacity(target + 256);
    s.push('[');
    let mut i = 0u64;
    while s.len() < target {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            r#"{{"id":{i},"name":"user \"{i}\"","tags":["a","b","c"],"score":{}.{},"active":{},"nested":{{"x":[1,2,{{"y":null}}],"z":"é\n"}}}}"#,
            i * 7 % 1000,
            i % 100,
            i.is_multiple_of(2)
        ));
        i += 1;
    }
    s.push(']');
    s
}

#[test]
#[ignore]
fn bench_json_pretty_10mb() {
    let json = big_json(10 << 20);
    let out = time(
        "json pretty (valid, 10 MB)",
        json.len(),
        Duration::from_secs(1),
        || format::pretty(ContentKind::Json, &json),
    );
    assert!(out.unwrap().len() > json.len());
    let broken = json
        .replacen("[", "[ // comment\n", 1)
        .replace(",\"active\"", ",'active'");
    let out = time(
        "json pretty (tolerant, 10 MB)",
        broken.len(),
        Duration::from_secs(1),
        || format::pretty(ContentKind::Json, &broken),
    );
    assert!(out.is_some());
    let min = time(
        "json minify (10 MB)",
        json.len(),
        Duration::from_secs(1),
        || format::minify_json(&json),
    );
    assert_eq!(min.unwrap().len(), json.len());
}

#[test]
#[ignore]
fn bench_other_formatters() {
    let js: String = (0..60_000)
        .map(|i| format!("function f{i}(a,b){{if(a>b){{return a/b/2}}var r=/x{{2}}[/]/g;return `t${{a}}`+{{k:[1,2,{i}]}}.k}}"))
        .collect();
    time(
        "javascript pretty",
        js.len(),
        Duration::from_secs(2),
        || format::pretty(ContentKind::Javascript, &js),
    )
    .unwrap();

    let css: String = (0..60_000).map(|i| format!(".c{i} a:hover,.d{i}>b{{color:red;margin:0 {i}px;background:url(x{i}.png)}}@media (max-width:{i}px){{.e{{top:0}}}}")).collect();
    time("css pretty", css.len(), Duration::from_secs(2), || {
        format::pretty(ContentKind::Css, &css)
    })
    .unwrap();

    let html: String = std::iter::once("<!doctype html><html><body>".to_string())
        .chain((0..60_000).map(|i| format!("<div class=\"r{i}\"><p>Hello <b>{i}</b> world<br><img src=a.png></p><ul><li>a<li>b</ul></div>")))
        .chain(std::iter::once("<script>if(a){b()}</script></body></html>".to_string()))
        .collect();
    time("html pretty", html.len(), Duration::from_secs(2), || {
        format::pretty(ContentKind::Html, &html)
    })
    .unwrap();

    let xml: String = std::iter::once("<?xml version=\"1.0\"?><root>".to_string())
        .chain((0..80_000).map(|i| {
            format!("<item id=\"{i}\"><name>n{i}</name><![CDATA[x<y]]><!-- c --><e/></item>")
        }))
        .chain(std::iter::once("</root>".to_string()))
        .collect();
    time("xml pretty", xml.len(), Duration::from_secs(2), || {
        format::pretty(ContentKind::Xml, &xml)
    })
    .unwrap();
}

#[test]
#[ignore]
fn bench_decode_detect_protobuf() {
    let json = big_json(32 << 20);
    for codec in ["gzip", "br", "zstd"] {
        let chain = vec![codec.to_string()];
        let wire = decode::encode_body(json.as_bytes(), &chain).unwrap();
        let out = time(
            &format!("decode {codec} (32 MB out)"),
            json.len(),
            Duration::from_secs(2),
            || decode::decode_body(&wire, &chain, 64 << 20).unwrap(),
        );
        assert_eq!(out.len(), json.len());
    }
    let kind = time(
        "detect_kind (32 MB body)",
        json.len(),
        Duration::from_millis(50),
        || detect::detect_kind(Some("application/octet-stream"), json.as_bytes(), "/data"),
    );
    assert_eq!(kind, ContentKind::Json);

    // ~4 MB protobuf: repeated nested messages with strings.
    let mut inner = vec![0x0a, 0x05];
    inner.extend_from_slice(b"hello");
    inner.extend_from_slice(&[0x10, 0x96, 0x01, 0x19]);
    inner.extend_from_slice(&1.5f64.to_le_bytes());
    let mut msg = Vec::new();
    while msg.len() < 4 << 20 {
        msg.push(0x12);
        msg.push(inner.len() as u8);
        msg.extend_from_slice(&inner);
    }
    let fields = time(
        "protobuf decode_raw (4 MB)",
        msg.len(),
        Duration::from_secs(1),
        || protobuf::decode_raw(&msg).unwrap(),
    );
    let _ = time(
        "protobuf to_json",
        msg.len(),
        Duration::from_secs(1),
        || protobuf::to_json(&fields),
    );
}

#[test]
#[ignore]
fn bench_view_body_gzip_json() {
    // under the default 8 MiB formatting limit
    let json = big_json(6 << 20);
    let wire = decode::encode_body(json.as_bytes(), &["gzip".to_string()]).unwrap();
    let mut h = Headers::new();
    h.append("Content-Type", "application/json");
    h.append("Content-Encoding", "gzip");
    let v = time(
        "view_body (6 MB gzip json)",
        json.len(),
        Duration::from_secs(1),
        || view_body(&wire, &h, "/api", 16 << 20),
    );
    assert!(v.formatted.is_some());
}

/// Adversarial shapes that used to be (or could easily become) quadratic or memory-hungry:
/// unmatched closers, unclosed-tag soup, pathological nesting. Each must finish fast, and
/// expansions beyond the output budget must be refused (`None`) rather than allocated.
#[test]
#[ignore]
fn bench_pathological_inputs() {
    let n = 100_000;
    let cases: Vec<(&str, ContentKind, String)> = vec![
        (
            "js ( then ]",
            ContentKind::Javascript,
            "(".repeat(n) + &"]".repeat(n),
        ),
        (
            "js [ then }",
            ContentKind::Javascript,
            "[".repeat(n) + &"}".repeat(n),
        ),
        ("js `${ nesting", ContentKind::Javascript, "`${".repeat(n)),
        (
            "css {( nesting",
            ContentKind::Css,
            "{(".repeat(n) + &"})".repeat(n),
        ),
        ("html unclosed div", ContentKind::Html, "<div>".repeat(n)),
        ("html ul/li", ContentKind::Html, "<ul><li>".repeat(n)),
        (
            "html stray end tags",
            ContentKind::Html,
            "<div>".repeat(n) + &"</span>".repeat(n),
        ),
        (
            "html script with </",
            ContentKind::Html,
            "<script>".to_string() + &"</".repeat(n),
        ),
        ("xml deep", ContentKind::Xml, "<a>".repeat(n)),
        ("graphql deep", ContentKind::GraphQl, "{a".repeat(n)),
        ("json deep", ContentKind::Json, "{\"a\":".repeat(n)),
        ("json records", ContentKind::JsonLines, "{}\n".repeat(n)),
    ];
    for (label, kind, input) in &cases {
        let out = time(label, input.len(), Duration::from_millis(500), || {
            format::pretty(*kind, input)
        });
        if let Some(o) = out {
            assert!(
                o.len() <= input.len() * 16 + (4 << 20),
                "{label}: output over budget"
            );
        }
    }
}
