//! JavaScript targets: browser `fetch`, Node.js `fetch` and axios.

use super::{base64, has_duplicate_names, note_line, omitted_body_note};
use crate::json::{self, Syntax};
use crate::lit;
use crate::prepare::{Body, FormField, Prepared};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Runtime {
    Browser,
    Node,
}

const JS: Syntax = Syntax {
    indent: "  ",
    null: "null",
    true_lit: "true",
    false_lit: "false",
    object_open: "{",
    object_close: "}",
    empty_object: "{}",
    array_open: "[",
    array_close: "]",
    key_sep: ": ",
    trailing_comma: false,
    string: lit::js,
    key: lit::js_key,
};

/// A header value expression. Header values are byte strings in JavaScript HTTP clients
/// (written as Latin-1), so non-ASCII values are converted to a string of their UTF-8 bytes to
/// send exactly what other clients send.
fn header_value(value: &str, runtime: Runtime) -> String {
    if value.is_ascii() {
        return lit::js(value);
    }
    match runtime {
        Runtime::Browser => {
            format!(
                "String.fromCharCode(...new TextEncoder().encode({}))",
                lit::js(value)
            )
        }
        Runtime::Node => format!("Buffer.from({}).toString(\"latin1\")", lit::js(value)),
    }
}

/// `headers: {...}` at one level of indentation, or `None` when there are no headers.
fn headers_entry(p: &Prepared<'_>, runtime: Runtime) -> Option<String> {
    let merged = p.merged_headers();
    if merged.is_empty() {
        return None;
    }
    let lines: Vec<String> = merged
        .iter()
        .map(|(name, value)| {
            format!(
                "    {}: {}",
                lit::js_quoted_key(name),
                header_value(value, runtime)
            )
        })
        .collect();
    Some(format!("headers: {{\n{}\n  }}", lines.join(",\n")))
}

/// Comment lines explaining header values the client converts or can't send.
fn header_notes(p: &Prepared<'_>, client: &str, runtime: Runtime, is_fetch: bool) -> String {
    let mut out = String::new();
    if !p.non_ascii_header_names().is_empty() {
        out.push_str(&note_line(
            "// ",
            &format!(
                "{client} sends header values as Latin-1, so non-ASCII values are passed as \
                 their UTF-8 bytes."
            ),
        ));
    }
    if !is_fetch {
        return out;
    }
    match runtime {
        Runtime::Browser => {
            let forbidden: Vec<&str> = p
                .merged_headers()
                .into_iter()
                .map(|(name, _)| name)
                .filter(|name| is_forbidden_in_browsers(name))
                .collect();
            if !forbidden.is_empty() {
                let (noun, pronoun) = if forbidden.len() == 1 {
                    ("header", "it's")
                } else {
                    ("headers", "they're")
                };
                out.push_str(&note_line(
                    "// ",
                    &format!(
                        "browsers don't let fetch() set the {} {noun}; {pronoun} silently \
                         dropped.",
                        super::join_names(&forbidden)
                    ),
                ));
            }
            if p.header("cookie").is_some() {
                out.push_str(
                    "// The browser sends its own cookies instead (add credentials: \"include\" \
                     for cross-origin requests).\n",
                );
            }
        }
        Runtime::Node => {
            if let Some(host) = p.header("host") {
                out.push_str(&note_line(
                    "// ",
                    &format!(
                        "fetch() always derives Host from the URL, so \"Host: {host}\" is not \
                         sent."
                    ),
                ));
            }
        }
    }
    out
}

/// Forbidden request-header names of the Fetch standard (those still present after
/// sanitisation): browsers silently drop them from `fetch()` requests.
fn is_forbidden_in_browsers(name: &str) -> bool {
    const FORBIDDEN: [&str; 13] = [
        "accept-charset",
        "access-control-request-headers",
        "access-control-request-method",
        "access-control-request-private-network",
        "cookie",
        "cookie2",
        "date",
        "dnt",
        "host",
        "origin",
        "referer",
        "set-cookie",
        "via",
    ];
    let lower = name.to_ascii_lowercase();
    FORBIDDEN.contains(&lower.as_str()) || lower.starts_with("sec-") || lower.starts_with("proxy-")
}

/// Is `s` a canonical array index? Such keys are enumerated first in JS objects, which would
/// reorder form fields.
fn is_array_index(s: &str) -> bool {
    s.parse::<u32>()
        .is_ok_and(|n| n != u32::MAX && n.to_string() == s)
}

/// `new URLSearchParams(...)` for a form body (object form when names are unique and safe).
fn url_search_params(fields: &[FormField<'_>]) -> String {
    let as_object = !has_duplicate_names(fields) && !fields.iter().any(|f| is_array_index(&f.name));
    let lines: Vec<String> = fields
        .iter()
        .map(|f| {
            if as_object {
                format!("    {}: {}", lit::js_key(&f.name), lit::js(&f.value))
            } else {
                format!("    [{}, {}]", lit::js(&f.name), lit::js(&f.value))
            }
        })
        .collect();
    let (open, close) = if as_object { ("{", "}") } else { ("[", "]") };
    format!(
        "new URLSearchParams({open}\n{}\n  {close})",
        lines.join(",\n")
    )
}

/// The JS expression for the body (at one level of indentation).
fn body_expr(
    body: &Body<'_>,
    runtime: Runtime,
    native_json: impl Fn(&str) -> String,
) -> Option<String> {
    Some(match body {
        Body::None => return None,
        Body::Json { value, .. } if json::fits_js_numbers(value) => {
            native_json(&json::render(value, &JS, "  "))
        }
        Body::Form { fields, .. } => url_search_params(fields),
        Body::Binary(bytes) => match runtime {
            Runtime::Browser => format!(
                "Uint8Array.from(atob({}), (c) => c.charCodeAt(0))",
                lit::js(&base64(bytes))
            ),
            Runtime::Node => format!("Buffer.from({}, \"base64\")", lit::js(&base64(bytes))),
        },
        other => lit::js_text(other.text().unwrap_or_default()),
    })
}

/// `fetch` (browser, or Node.js 18+). JSON bodies become `JSON.stringify({...})`, forms
/// `URLSearchParams`, binary bodies `atob` (browser) / `Buffer.from` (Node). `fetch` rejects
/// GET/HEAD bodies, so those are omitted with a note.
pub(super) fn fetch(p: &Prepared<'_>, runtime: Runtime) -> String {
    let mut out = String::new();
    let mut opts = Vec::new();
    if p.method != "GET" {
        opts.push(format!("method: {}", lit::js(p.method)));
    }
    opts.extend(headers_entry(p, runtime));
    out.push_str(&header_notes(p, "fetch()", runtime, true));
    if !p.body.is_none() && p.is_get_or_head() {
        out.push_str(&note_line("// ", &omitted_body_note("fetch()", p.method)));
    } else if let Some(body) = body_expr(&p.body, runtime, |literal| {
        format!("JSON.stringify({literal})")
    }) {
        opts.push(format!("body: {body}"));
    }
    if opts.is_empty() {
        out.push_str(&format!(
            "const response = await fetch({});\n",
            lit::js(p.url)
        ));
    } else {
        out.push_str(&format!(
            "const response = await fetch({}, {{\n  {}\n}});\n",
            lit::js(p.url),
            opts.join(",\n  ")
        ));
    }
    out.push_str("\nconsole.log(response.status);\nconsole.log(await response.text());");
    out
}

/// axios (Node.js, ES module). JSON bodies are passed as objects (axios serialises them),
/// forms as `URLSearchParams`, binary bodies as a `Buffer`.
pub(super) fn axios(p: &Prepared<'_>) -> String {
    let mut opts = Vec::new();
    if p.method != "GET" {
        let method = if crate::prepare::is_standard_method(p.method) {
            p.method.to_ascii_lowercase()
        } else {
            p.method.to_string()
        };
        opts.push(format!("method: {}", lit::js(&method)));
    }
    opts.push(format!("url: {}", lit::js(p.url)));
    opts.extend(headers_entry(p, Runtime::Node));
    if let Some(body) = body_expr(&p.body, Runtime::Node, str::to_string) {
        opts.push(format!("data: {body}"));
    }
    let notes = header_notes(p, "Node.js", Runtime::Node, false);
    format!(
        "import axios from \"axios\";\n\n{notes}const response = await axios.request({{\n  {}\n}});\n\n\
         console.log(response.status);\nconsole.log(response.data);",
        opts.join(",\n  ")
    )
}
