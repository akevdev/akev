//! Python targets: requests and HTTPX.

use super::{base64, has_duplicate_names};
use crate::json::{self, Syntax};
use crate::lit;
use crate::prepare::{Body, Prepared};
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Library {
    Requests,
    Httpx,
}

impl Library {
    fn module(self) -> &'static str {
        match self {
            Library::Requests => "requests",
            Library::Httpx => "httpx",
        }
    }
}

const PYTHON: Syntax = Syntax {
    indent: "    ",
    null: "None",
    true_lit: "True",
    false_lit: "False",
    object_open: "{",
    object_close: "}",
    empty_object: "{}",
    array_open: "[",
    array_close: "]",
    key_sep: ": ",
    trailing_comma: true,
    string: lit::py,
    key: lit::py,
};

/// requests / HTTPX. JSON bodies use `json=` with a dict literal, forms `data=` (a list of
/// tuples / dict of lists when names repeat), text `data=`/`content=` (requests needs bytes for
/// non-ASCII text, since `http.client` encodes `str` bodies as Latin-1), binary bodies
/// `base64.b64decode`. Non-ASCII header values are passed as UTF-8 `bytes` (requests would
/// encode a `str` as Latin-1, HTTPX as ASCII). Both clients upper-case the method, which a
/// note points out for mixed-case custom methods. `Accept-Encoding` is dropped: both clients
/// negotiate and decode it.
pub(super) fn generate(p: &Prepared<'_>, lib: Library) -> String {
    let mut out = String::new();
    if matches!(p.body, Body::Binary(_)) {
        out.push_str("import base64\n\n");
    }
    out.push_str(&format!(
        "import {}\n\nurl = {}\n",
        lib.module(),
        lit::py(p.url)
    ));

    let merged = p.merged_headers();
    if !merged.is_empty() {
        out.push_str("headers = {\n");
        for (name, value) in &merged {
            let encode = if value.is_ascii() { "" } else { ".encode()" };
            out.push_str(&format!(
                "    {}: {}{encode},\n",
                lit::py(name),
                lit::py(value)
            ));
        }
        out.push_str("}\n");
    }

    let body_kwarg = match &p.body {
        Body::None => None,
        Body::Json { value, .. } => {
            out.push_str(&format!("payload = {}\n", json::render(value, &PYTHON, "")));
            Some("json")
        }
        Body::Form { fields, .. } => {
            if !has_duplicate_names(fields) {
                out.push_str("payload = {\n");
                for f in fields {
                    out.push_str(&format!(
                        "    {}: {},\n",
                        lit::py(&f.name),
                        lit::py(&f.value)
                    ));
                }
                out.push_str("}\n");
            } else if lib == Library::Requests {
                out.push_str("payload = [\n");
                for f in fields {
                    out.push_str(&format!(
                        "    ({}, {}),\n",
                        lit::py(&f.name),
                        lit::py(&f.value)
                    ));
                }
                out.push_str("]\n");
            } else {
                // HTTPX takes repeated names as a list value.
                let mut grouped: Vec<(&str, Vec<&str>)> = Vec::new();
                let mut index: HashMap<&str, usize> = HashMap::with_capacity(fields.len());
                for f in fields {
                    match index.get(f.name.as_str()) {
                        Some(&i) => grouped[i].1.push(&f.value),
                        None => {
                            index.insert(&f.name, grouped.len());
                            grouped.push((&f.name, vec![&f.value]));
                        }
                    }
                }
                out.push_str("payload = {\n");
                for (name, values) in grouped {
                    let rendered: Vec<String> = values.iter().map(|v| lit::py(v)).collect();
                    let value = if rendered.len() == 1 {
                        rendered.concat()
                    } else {
                        format!("[{}]", rendered.join(", "))
                    };
                    out.push_str(&format!("    {}: {value},\n", lit::py(name)));
                }
                out.push_str("}\n");
            }
            Some("data")
        }
        Body::Text(text) => {
            let encode = if lib == Library::Requests && !text.is_ascii() {
                ".encode()"
            } else {
                ""
            };
            out.push_str(&format!("payload = {}{encode}\n", lit::py_text(text)));
            Some(if lib == Library::Requests {
                "data"
            } else {
                "content"
            })
        }
        Body::Binary(bytes) => {
            out.push_str(&format!(
                "payload = base64.b64decode({})\n",
                lit::py(&base64(bytes))
            ));
            Some(if lib == Library::Requests {
                "data"
            } else {
                "content"
            })
        }
    };

    let shortcut = matches!(p.method, "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS")
        // HTTPX's shortcut functions for these methods don't accept a body.
        && !(lib == Library::Httpx
            && body_kwarg.is_some()
            && matches!(p.method, "GET" | "HEAD" | "OPTIONS" | "DELETE"));
    let mut args = Vec::new();
    let call = if shortcut {
        format!("{}.{}", lib.module(), p.method.to_ascii_lowercase())
    } else {
        args.push(lit::py(p.method));
        format!("{}.request", lib.module())
    };
    args.push("url".to_string());
    if !merged.is_empty() {
        args.push("headers=headers".to_string());
    }
    if let Some(kwarg) = body_kwarg {
        args.push(format!("{kwarg}=payload"));
    }
    out.push('\n');
    if p.method.chars().any(|c| c.is_ascii_lowercase()) {
        out.push_str(&super::note_line(
            "# ",
            &format!(
                "{} upper-cases the method, so this sends {}.",
                lib.module(),
                p.method.to_ascii_uppercase()
            ),
        ));
    }
    out.push_str(&format!(
        "response = {call}({})\n\nprint(response.status_code)\nprint(response.text)",
        args.join(", ")
    ));
    out
}
