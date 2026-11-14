//! Swift `URLSession` and Dart `package:http`.

use super::{base64, non_ascii_header_note, note_line, omitted_body_note};
use crate::lit;
use crate::prepare::{Body, Prepared};

/// Swift `URLSession` with async/await (works as `main.swift` / a script; imports
/// FoundationNetworking on Linux). Text bodies use multi-line or raw string literals; binary
/// bodies `Data(base64Encoded:)`. URLSession rejects GET bodies, so those are omitted with a
/// note. `URL(string:)` rejects characters outside RFC 3986 on older systems, so those are
/// percent-encoded. `Host` is a header URLSession reserves (a note says so).
/// `Accept-Encoding` is dropped: URLSession negotiates and decodes it itself.
pub(super) fn swift(p: &Prepared<'_>) -> String {
    let mut out = String::from(
        "import Foundation\n#if canImport(FoundationNetworking)\nimport FoundationNetworking\n#endif\n\n",
    );
    let omit_body = !p.body.is_none() && p.is_get_or_head();
    match &p.body {
        _ if omit_body => out.push_str(&note_line(
            "// ",
            &omitted_body_note("URLSession", p.method),
        )),
        Body::None => {}
        Body::Binary(bytes) => out.push_str(&format!(
            "let body = Data(base64Encoded: {})!\n",
            lit::swift(&base64(bytes))
        )),
        body => out.push_str(&format!(
            "let body = {}\n",
            lit::swift_text(body.text().unwrap_or_default(), "    ")
        )),
    }
    if !p.body.is_none() && !omit_body {
        out.push('\n');
    }
    if p.header("host").is_some() {
        out.push_str(
            "// Note: URLSession manages the Host header itself and may ignore this value.\n",
        );
    }
    out.push_str(&format!(
        "var request = URLRequest(url: URL(string: {})!)\n",
        lit::swift(&p.strict_url())
    ));
    if p.method != "GET" {
        out.push_str(&format!("request.httpMethod = {}\n", lit::swift(p.method)));
    }
    let mut seen: Vec<&str> = Vec::new();
    for &(name, value) in &p.headers {
        let call = if seen.iter().any(|s| s.eq_ignore_ascii_case(name)) {
            "addValue"
        } else {
            seen.push(name);
            "setValue"
        };
        out.push_str(&format!(
            "request.{call}({}, forHTTPHeaderField: {})\n",
            lit::swift(value),
            lit::swift(name)
        ));
    }
    match &p.body {
        _ if omit_body => {}
        Body::None => {}
        Body::Binary(_) => out.push_str("request.httpBody = body\n"),
        _ => out.push_str("request.httpBody = Data(body.utf8)\n"),
    }
    out.push_str(
        "\nlet (data, response) = try await URLSession.shared.data(for: request)\n\n\
         print((response as? HTTPURLResponse)?.statusCode ?? 0)\n\
         print(String(decoding: data, as: UTF8.self))",
    );
    out
}

/// Dart `package:http`. Uses `http.Request` so any method can carry a body, and sets
/// `bodyBytes` (the `body` setter would append `; charset=utf-8` to `Content-Type`). Header
/// maps are case-insensitive, so repeated names are joined. The URL is percent-encoded to
/// RFC 3986 for `Uri.parse`. `dart:io` rejects non-ASCII header values, which a note points
/// out. `Accept-Encoding` is dropped: `dart:io` negotiates gzip
/// and decodes it itself.
pub(super) fn dart(p: &Prepared<'_>) -> String {
    let needs_convert = !p.body.is_none();
    let mut out = String::new();
    if needs_convert {
        out.push_str("import 'dart:convert';\n\n");
    }
    out.push_str("import 'package:http/http.dart' as http;\n\n");
    if let Some(note) = non_ascii_header_note(
        p,
        "dart:io",
        "rejects non-ASCII header values (FormatException)",
    ) {
        out.push_str(&note_line("// ", &note));
    }
    out.push_str("Future<void> main() async {\n");
    out.push_str(&format!(
        "  final request = http.Request({}, Uri.parse({}));\n",
        lit::dart(p.method),
        lit::dart(&p.strict_url())
    ));
    let merged = p.merged_headers();
    if !merged.is_empty() {
        out.push_str("  request.headers.addAll({\n");
        for (name, value) in &merged {
            out.push_str(&format!("    {}: {},\n", lit::dart(name), lit::dart(value)));
        }
        out.push_str("  });\n");
    }
    match &p.body {
        Body::None => {}
        Body::Binary(bytes) => out.push_str(&format!(
            "  request.bodyBytes = base64Decode({});\n",
            lit::dart(&base64(bytes))
        )),
        body => out.push_str(&format!(
            "  request.bodyBytes = utf8.encode({});\n",
            lit::dart_text(body.text().unwrap_or_default())
        )),
    }
    out.push_str(
        "\n  final response = await http.Response.fromStream(await request.send());\n\
         \x20 print(response.statusCode);\n  print(response.body);\n}",
    );
    out
}
