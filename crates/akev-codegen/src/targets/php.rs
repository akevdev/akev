//! PHP ext-curl.

use super::{base64, has_duplicate_names};
use crate::json::{self, Syntax};
use crate::lit;
use crate::prepare::{Body, Prepared};

const PHP: Syntax = Syntax {
    indent: "    ",
    null: "null",
    true_lit: "true",
    false_lit: "false",
    object_open: "[",
    object_close: "]",
    empty_object: "new \\stdClass()",
    array_open: "[",
    array_close: "]",
    key_sep: " => ",
    trailing_comma: true,
    string: lit::php,
    key: lit::php,
};

/// PHP ext-curl. JSON bodies become `json_encode([...])` (when every object maps losslessly to
/// a PHP array), forms with unique names `http_build_query([...])`, binary bodies
/// `base64_decode(...)`. HEAD uses `CURLOPT_NOBODY` (a custom HEAD request would wait for a
/// body). `Accept-Encoding` becomes `CURLOPT_ENCODING => ''` (negotiate and decode).
pub(super) fn generate(p: &Prepared<'_>) -> String {
    let mut opts = vec![
        format!("CURLOPT_URL => {}", lit::php(p.url)),
        "CURLOPT_RETURNTRANSFER => true".to_string(),
    ];
    match p.method {
        "GET" => {}
        "HEAD" => opts.push("CURLOPT_NOBODY => true".into()),
        method => opts.push(format!("CURLOPT_CUSTOMREQUEST => {}", lit::php(method))),
    }
    if p.accept_encoding.is_some() {
        opts.push("CURLOPT_ENCODING => ''".into());
    }
    if !p.headers.is_empty() {
        let lines: Vec<String> = p
            .headers
            .iter()
            // libcurl treats `Name:` as "remove this header"; `Name;` sends it empty.
            .map(|(name, value)| {
                let line = if value.is_empty() {
                    format!("{name};")
                } else {
                    format!("{name}: {value}")
                };
                format!("        {},\n", lit::php(&line))
            })
            .collect();
        opts.push(format!("CURLOPT_HTTPHEADER => [\n{}    ]", lines.concat()));
    }
    let body = match &p.body {
        Body::None => None,
        Body::Json { value, .. } if json::fits_php_arrays(value) => Some(format!(
            "json_encode({})",
            json::render(value, &PHP, "    ")
        )),
        Body::Form { fields, .. } if !has_duplicate_names(fields) => {
            let lines: Vec<String> = fields
                .iter()
                .map(|f| format!("        {} => {},\n", lit::php(&f.name), lit::php(&f.value)))
                .collect();
            Some(format!("http_build_query([\n{}    ])", lines.concat()))
        }
        Body::Binary(bytes) => Some(format!("base64_decode({})", lit::php(&base64(bytes)))),
        other => Some(lit::php(other.text().unwrap_or_default())),
    };
    if let Some(body) = body {
        opts.push(format!("CURLOPT_POSTFIELDS => {body}"));
    }
    let mut out = String::from("<?php\n\n$curl = curl_init();\n\ncurl_setopt_array($curl, [\n");
    for opt in opts {
        out.push_str("    ");
        out.push_str(&opt);
        out.push_str(",\n");
    }
    out.push_str(
        "]);\n\n$response = curl_exec($curl);\n\n\
         if ($response === false) {\n    echo 'cURL error: ' . curl_error($curl) . PHP_EOL;\n\
         } else {\n    echo curl_getinfo($curl, CURLINFO_RESPONSE_CODE) . PHP_EOL;\n    \
         echo $response . PHP_EOL;\n}",
    );
    out
}
