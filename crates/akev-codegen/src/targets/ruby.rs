//! Ruby `Net::HTTP`.

use super::base64;
use crate::json::{self, Syntax};
use crate::lit;
use crate::prepare::{Body, Prepared};

const RUBY: Syntax = Syntax {
    indent: "  ",
    null: "nil",
    true_lit: "true",
    false_lit: "false",
    object_open: "{",
    object_close: "}",
    empty_object: "{}",
    array_open: "[",
    array_close: "]",
    key_sep: " => ",
    trailing_comma: false,
    string: lit::ruby,
    key: lit::ruby,
};

fn request_class(method: &str) -> Option<&'static str> {
    Some(match method {
        "GET" => "Get",
        "HEAD" => "Head",
        "POST" => "Post",
        "PUT" => "Put",
        "PATCH" => "Patch",
        "DELETE" => "Delete",
        "OPTIONS" => "Options",
        "TRACE" => "Trace",
        "COPY" => "Copy",
        "LOCK" => "Lock",
        "MKCOL" => "Mkcol",
        "MOVE" => "Move",
        "PROPFIND" => "Propfind",
        "PROPPATCH" => "Proppatch",
        "UNLOCK" => "Unlock",
        _ => return None,
    })
}

/// Ruby `Net::HTTP`. JSON bodies use `JSON.generate` on a hash literal, forms
/// `URI.encode_www_form` (order and repeated names preserved), binary bodies are decoded with
/// `unpack1("m")` (no `base64` gem needed on Ruby 3.4+). `Accept-Encoding` is dropped:
/// Net::HTTP only decodes responses when it negotiated compression itself.
pub(super) fn generate(p: &Prepared<'_>) -> String {
    let mut out = String::new();
    if matches!(p.body, Body::Json { .. }) {
        out.push_str("require 'json'\n");
    }
    out.push_str(&format!(
        "require 'net/http'\n\nuri = URI({})\n\n",
        lit::ruby(p.url)
    ));
    match request_class(p.method) {
        Some(class) => out.push_str(&format!("request = Net::HTTP::{class}.new(uri)\n")),
        None => out.push_str(&format!(
            "request = Net::HTTPGenericRequest.new({}, true, true, uri)\n",
            lit::ruby(p.method)
        )),
    }
    let mut seen: Vec<&str> = Vec::new();
    for &(name, value) in &p.headers {
        if seen.iter().any(|s| s.eq_ignore_ascii_case(name)) {
            out.push_str(&format!(
                "request.add_field({}, {})\n",
                lit::ruby(name),
                lit::ruby(value)
            ));
        } else {
            seen.push(name);
            out.push_str(&format!(
                "request[{}] = {}\n",
                lit::ruby(name),
                lit::ruby(value)
            ));
        }
    }
    match &p.body {
        Body::None => {}
        Body::Json { value, .. } => {
            out.push_str(&format!(
                "request.body = JSON.generate({})\n",
                json::render(value, &RUBY, "")
            ));
        }
        Body::Form { fields, .. } => {
            out.push_str("request.body = URI.encode_www_form([\n");
            let lines: Vec<String> = fields
                .iter()
                .map(|f| format!("  [{}, {}]", lit::ruby(&f.name), lit::ruby(&f.value)))
                .collect();
            out.push_str(&lines.join(",\n"));
            out.push_str("\n])\n");
        }
        Body::Text(text) => out.push_str(&format!("request.body = {}\n", lit::ruby_text(text))),
        Body::Binary(bytes) => out.push_str(&format!(
            "request.body = {}.unpack1('m')\n",
            lit::ruby(&base64(bytes))
        )),
    }
    out.push_str(
        "\nresponse = Net::HTTP.start(uri.hostname, uri.port, use_ssl: uri.scheme == 'https') do |http|\n\
         \x20 http.request(request)\nend\n\nputs response.code\nputs response.body",
    );
    out
}
