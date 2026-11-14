//! Rust reqwest (async, tokio).

use crate::lit;
use crate::prepare::{Body, Prepared};

/// rustfmt's `fn_call_width`: wider argument lists are laid out one per line.
const FN_CALL_WIDTH: usize = 60;

/// Display width as rustfmt measures it (East Asian wide characters count double).
fn display_width(s: &str) -> usize {
    s.chars()
        .map(|c| match c as u32 {
            0x1100..=0x115F
            | 0x2E80..=0x303E
            | 0x3041..=0x33FF
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xA000..=0xA4CF
            | 0xAC00..=0xD7A3
            | 0xF900..=0xFAFF
            | 0xFE30..=0xFE4F
            | 0xFF00..=0xFF60
            | 0xFFE0..=0xFFE6
            | 0x1F300..=0x1F64F
            | 0x1F900..=0x1F9FF
            | 0x20000..=0x3FFFD => 2,
            _ => 1,
        })
        .sum()
}

/// rustfmt's `chain_width`: shorter chains stay on one line.
const CHAIN_WIDTH: usize = 60;

/// One `.method(args)` link of the request builder chain, formatted like rustfmt.
fn push_call(out: &mut String, method: &str, args: &[String]) {
    let inline = args.join(", ");
    if args.len() > 1 && display_width(&inline) > FN_CALL_WIDTH {
        out.push_str(&format!("        .{method}(\n"));
        for arg in args {
            out.push_str(&format!("            {arg},\n"));
        }
        out.push_str("        )\n");
    } else {
        out.push_str(&format!("        .{method}({inline})\n"));
    }
}

/// `let response = client…send().await?;`, on one line when rustfmt would keep it there.
fn push_request_chain(out: &mut String, calls: &[(String, Vec<String>)]) {
    let mut one_line = String::from("client");
    for (method, args) in calls {
        one_line.push_str(&format!(".{method}({})", args.join(", ")));
    }
    one_line.push_str(".send().await?");
    if display_width(&one_line) < CHAIN_WIDTH {
        out.push_str(&format!("    let response = {one_line};\n"));
        return;
    }
    out.push_str("    let response = client\n");
    for (method, args) in calls {
        push_call(out, method, args);
    }
    out.push_str("        .send()\n        .await?;\n");
}

/// reqwest with tokio. Text bodies use raw strings where possible; binary bodies are byte
/// string literals (no extra dependency). `Accept-Encoding` is dropped: reqwest only decodes
/// responses with its compression features enabled.
pub(super) fn generate(p: &Prepared<'_>) -> String {
    let mut out = String::from(
        "// Cargo.toml:\n// [dependencies]\n// reqwest = \"0.12\"\n\
         // tokio = { version = \"1\", features = [\"full\"] }\n\n\
         #[tokio::main]\nasync fn main() -> Result<(), Box<dyn std::error::Error>> {\n",
    );
    match &p.body {
        Body::None => {}
        Body::Binary(bytes) => {
            out.push_str(&format!(
                "    let body: &[u8] = {};\n",
                lit::rust_bytes(bytes)
            ));
        }
        body => {
            out.push_str(&format!(
                "    let body = {};\n",
                lit::rust_text(body.text().unwrap_or_default())
            ));
        }
    }
    out.push_str("    let client = reqwest::Client::new();\n");
    let url = lit::rust(p.url);
    let mut calls: Vec<(String, Vec<String>)> = Vec::with_capacity(p.headers.len() + 2);
    match p.method {
        "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" => {
            calls.push((p.method.to_ascii_lowercase(), vec![url]));
        }
        "OPTIONS" | "CONNECT" | "TRACE" => {
            calls.push((
                "request".into(),
                vec![format!("reqwest::Method::{}", p.method), url],
            ));
        }
        other => {
            let method = format!(
                "reqwest::Method::from_bytes({})?",
                lit::rust_bytes(other.as_bytes())
            );
            calls.push(("request".into(), vec![method, url]));
        }
    }
    for &(name, value) in &p.headers {
        calls.push(("header".into(), vec![lit::rust(name), lit::rust(value)]));
    }
    if !p.body.is_none() {
        calls.push(("body".into(), vec!["body".into()]));
    }
    push_request_chain(&mut out, &calls);
    out.push_str(
        "\n\x20   println!(\"{}\", response.status());\n\
         \x20   println!(\"{}\", response.text().await?);\n\n\
         \x20   Ok(())\n}",
    );
    out
}
