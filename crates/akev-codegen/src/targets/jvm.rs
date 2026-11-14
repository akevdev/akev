//! JVM targets: Java `HttpClient` and Kotlin OkHttp.

use super::{base64, non_ascii_header_note, note_line, omitted_body_note};
use crate::lit;
use crate::prepare::{Body, Prepared};

/// Java 15+ `java.net.http.HttpClient`. Text bodies use text blocks; binary bodies are
/// base64-decoded. `Accept-Encoding` is dropped (HttpClient never decompresses). A differing
/// `Host` is allowed via `jdk.httpclient.allowRestrictedHeaders`. `java.net.URI` rejects
/// characters outside RFC 3986, so those are percent-encoded. HttpClient can't send non-ASCII
/// header values (they become `?`, or throw above U+00FF), which a note points out.
pub(super) fn java(p: &Prepared<'_>) -> String {
    let binary = matches!(p.body, Body::Binary(_));
    let mut out = String::from(
        "import java.net.URI;\nimport java.net.http.HttpClient;\nimport java.net.http.HttpRequest;\nimport java.net.http.HttpResponse;\n",
    );
    if binary {
        out.push_str("import java.util.Base64;\n");
    }
    out.push_str(
        "\npublic class Main {\n    public static void main(String[] args) throws Exception {\n",
    );
    if p.header("host").is_some() {
        out.push_str(
            "        // Host is a restricted header in java.net.http unless explicitly allowed.\n\
             \x20       System.setProperty(\"jdk.httpclient.allowRestrictedHeaders\", \"host\");\n\n",
        );
    }
    let publisher = match &p.body {
        Body::None => None,
        Body::Binary(bytes) => {
            out.push_str(&format!(
                "        byte[] body = Base64.getDecoder().decode({});\n\n",
                lit::java(&base64(bytes))
            ));
            Some("HttpRequest.BodyPublishers.ofByteArray(body)")
        }
        body => {
            out.push_str(&format!(
                "        String body = {};\n\n",
                lit::java_text(body.text().unwrap_or_default(), "                ")
            ));
            Some("HttpRequest.BodyPublishers.ofString(body)")
        }
    };
    if let Some(note) = non_ascii_header_note(
        p,
        "java.net.http",
        "sends non-ASCII header characters as '?' (and rejects those above U+00FF)",
    ) {
        out.push_str(&note_line("        // ", &note));
    }
    out.push_str(&format!(
        "        HttpRequest request = HttpRequest.newBuilder()\n                .uri(URI.create({}))\n",
        lit::java(&p.strict_url())
    ));
    for &(name, value) in &p.headers {
        out.push_str(&format!(
            "                .header({}, {})\n",
            lit::java(name),
            lit::java(value)
        ));
    }
    let method_call = match (p.method, publisher) {
        ("GET", None) => ".GET()".to_string(),
        ("DELETE", None) => ".DELETE()".to_string(),
        ("POST", Some(publisher)) => format!(".POST({publisher})"),
        ("PUT", Some(publisher)) => format!(".PUT({publisher})"),
        (method, publisher) => format!(
            ".method({}, {})",
            lit::java(method),
            publisher.unwrap_or("HttpRequest.BodyPublishers.noBody()")
        ),
    };
    out.push_str(&format!(
        "                {method_call}\n                .build();\n\n\
         \x20       HttpResponse<String> response = HttpClient.newHttpClient()\n\
         \x20               .send(request, HttpResponse.BodyHandlers.ofString());\n\n\
         \x20       System.out.println(response.statusCode());\n\
         \x20       System.out.println(response.body());\n    }}\n}}"
    ));
    out
}

/// Kotlin with OkHttp 4+. `Content-Type` becomes the body's media type (OkHttp would override
/// a header anyway). OkHttp forbids bodies on GET/HEAD (omitted with a note) and requires one
/// for POST/PUT/PATCH (an empty body is sent). Non-ASCII header values, which `addHeader`
/// rejects, go through `Headers.Builder.addUnsafeNonAscii` (sent as UTF-8). `Accept-Encoding`
/// is dropped: OkHttp only decompresses transparently when it negotiated gzip itself.
pub(super) fn kotlin(p: &Prepared<'_>) -> String {
    let content_type = p.content_type();
    let has_body = !p.body.is_none() && !p.is_get_or_head();
    let binary = has_body && matches!(p.body, Body::Binary(_));
    let headers: Vec<(&str, &str)> = p
        .headers
        .iter()
        .copied()
        .filter(|(name, _)| !(has_body && name.eq_ignore_ascii_case("content-type")))
        .collect();
    let unsafe_headers = headers.iter().any(|(_, value)| !value.is_ascii());

    let mut imports = vec!["okhttp3.OkHttpClient", "okhttp3.Request"];
    if unsafe_headers {
        imports.push("okhttp3.Headers");
    }
    if has_body || matches!(p.method, "POST" | "PUT" | "PATCH") {
        if content_type.is_some() && has_body {
            imports.push("okhttp3.MediaType.Companion.toMediaType");
        }
        imports.push("okhttp3.RequestBody.Companion.toRequestBody");
    }
    if binary {
        imports.push("okio.ByteString.Companion.decodeBase64");
    }
    imports.sort_unstable();

    let mut out = String::new();
    for import in &imports {
        out.push_str(&format!("import {import}\n"));
    }
    out.push_str("\nfun main() {\n    val client = OkHttpClient()\n\n");

    if !p.body.is_none() && p.is_get_or_head() {
        out.push_str(&note_line(
            "    // ",
            &omitted_body_note("OkHttp", p.method),
        ));
    }
    let media_type = match content_type {
        Some(ct) if has_body => format!("{}.toMediaType()", lit::kotlin(ct)),
        _ => String::new(),
    };
    let body_var = if has_body {
        let expr = match &p.body {
            Body::Binary(bytes) => format!("{}.decodeBase64()!!", lit::kotlin(&base64(bytes))),
            body => lit::kotlin_text(body.text().unwrap_or_default(), "        "),
        };
        out.push_str(&format!(
            "    val body = {expr}.toRequestBody({media_type})\n\n"
        ));
        Some("body")
    } else if matches!(p.method, "POST" | "PUT" | "PATCH") {
        out.push_str("    val body = \"\".toRequestBody()\n\n");
        Some("body")
    } else {
        None
    };

    if unsafe_headers {
        out.push_str("    val headers = Headers.Builder()\n");
        for &(name, value) in &headers {
            let add = if value.is_ascii() {
                "add"
            } else {
                "addUnsafeNonAscii"
            };
            out.push_str(&format!(
                "        .{add}({}, {})\n",
                lit::kotlin(name),
                lit::kotlin(value)
            ));
        }
        out.push_str("        .build()\n\n");
    }

    out.push_str(&format!(
        "    val request = Request.Builder()\n        .url({})\n",
        lit::kotlin(p.url)
    ));
    let method_call = match (p.method, body_var) {
        ("GET", None) => None,
        ("HEAD", None) => Some(".head()".to_string()),
        ("DELETE", None) => Some(".delete()".to_string()),
        ("POST" | "PUT" | "PATCH" | "DELETE", Some(body)) => {
            Some(format!(".{}({body})", p.method.to_ascii_lowercase()))
        }
        (method, body) => Some(format!(
            ".method({}, {})",
            lit::kotlin(method),
            body.unwrap_or("null")
        )),
    };
    if let Some(call) = method_call {
        out.push_str(&format!("        {call}\n"));
    }
    if unsafe_headers {
        out.push_str("        .headers(headers)\n");
    } else {
        for &(name, value) in &headers {
            out.push_str(&format!(
                "        .addHeader({}, {})\n",
                lit::kotlin(name),
                lit::kotlin(value)
            ));
        }
    }
    out.push_str(
        "        .build()\n\n    client.newCall(request).execute().use { response ->\n\
         \x20       println(response.code)\n        println(response.body?.string())\n    }\n}",
    );
    out
}
