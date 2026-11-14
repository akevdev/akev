//! .NET targets: C# `HttpClient` and PowerShell `Invoke-WebRequest`.

use super::{base64, non_ascii_header_note, note_line};
use crate::lit;
use crate::prepare::{Body, Prepared};

/// Headers that belong on `HttpContent.Headers` rather than the request in .NET.
const CONTENT_HEADERS: [&str; 9] = [
    "allow",
    "content-disposition",
    "content-language",
    "content-location",
    "content-md5",
    "content-range",
    "content-type",
    "expires",
    "last-modified",
];

fn http_method(method: &str) -> String {
    match method {
        "GET" => "HttpMethod.Get".into(),
        "POST" => "HttpMethod.Post".into(),
        "PUT" => "HttpMethod.Put".into(),
        "DELETE" => "HttpMethod.Delete".into(),
        "HEAD" => "HttpMethod.Head".into(),
        "OPTIONS" => "HttpMethod.Options".into(),
        "TRACE" => "HttpMethod.Trace".into(),
        "PATCH" => "HttpMethod.Patch".into(),
        other => format!("new HttpMethod({})", lit::csharp(other)),
    }
}

/// C# (.NET 7+, top-level statements). Headers use `TryAddWithoutValidation` (no format
/// exceptions for unusual values); content headers go on the content, `Content-Type` replaces
/// `StringContent`'s default (which is removed when the request had none). `Accept-Encoding`
/// switches on `AutomaticDecompression`, which sends a matching header itself; non-ASCII
/// header values (rejected by default) are sent as UTF-8 via `RequestHeaderEncodingSelector`.
pub(super) fn csharp(p: &Prepared<'_>) -> String {
    let is_content = |name: &str| CONTENT_HEADERS.iter().any(|h| h.eq_ignore_ascii_case(name));
    let content_type = p.content_type();
    let content_headers: Vec<(&str, &str)> = p
        .headers
        .iter()
        .copied()
        .filter(|(n, _)| is_content(n) && !n.eq_ignore_ascii_case("content-type"))
        .collect();
    let needs_content = !p.body.is_none() || content_type.is_some() || !content_headers.is_empty();

    let decompress = p.accept_encoding.is_some();
    let utf8_headers = !p.non_ascii_header_names().is_empty();

    let mut out = String::from("using System;\n");
    if decompress {
        out.push_str("using System.Net;\n");
    }
    out.push_str("using System.Net.Http;\n");
    if content_type.is_some() {
        out.push_str("using System.Net.Http.Headers;\n");
    }
    if utf8_headers {
        out.push_str("using System.Text;\n");
    }
    out.push('\n');
    if decompress || utf8_headers {
        out.push_str("using var client = new HttpClient(new SocketsHttpHandler\n{\n");
        if decompress {
            out.push_str("    AutomaticDecompression = DecompressionMethods.All,\n");
        }
        if utf8_headers {
            out.push_str(
                "    // Send non-ASCII header values as UTF-8 (by default .NET rejects them).\n\
                 \x20   RequestHeaderEncodingSelector = (_, _) => Encoding.UTF8,\n",
            );
        }
        out.push_str("});\n");
    } else {
        out.push_str("using var client = new HttpClient();\n");
    }
    out.push_str(&format!(
        "using var request = new HttpRequestMessage({}, {});\n",
        http_method(p.method),
        lit::csharp(p.url)
    ));
    for &(name, value) in &p.headers {
        if !is_content(name) {
            out.push_str(&format!(
                "request.Headers.TryAddWithoutValidation({}, {});\n",
                lit::csharp(name),
                lit::csharp(value)
            ));
        }
    }
    if needs_content {
        let content = match &p.body {
            Body::None => "new ByteArrayContent(Array.Empty<byte>())".to_string(),
            Body::Binary(bytes) => {
                format!(
                    "new ByteArrayContent(Convert.FromBase64String({}))",
                    lit::csharp(&base64(bytes))
                )
            }
            body => format!(
                "new StringContent({})",
                lit::csharp_text(body.text().unwrap_or_default(), "    ")
            ),
        };
        out.push_str(&format!("var content = {content};\n"));
        match content_type {
            Some(ct) => out.push_str(&format!(
                "content.Headers.ContentType = MediaTypeHeaderValue.Parse({});\n",
                lit::csharp(ct)
            )),
            // StringContent defaults to `text/plain; charset=utf-8`.
            None if p.body.text().is_some() => {
                out.push_str("content.Headers.ContentType = null;\n");
            }
            None => {}
        }
        for (name, value) in content_headers {
            out.push_str(&format!(
                "content.Headers.TryAddWithoutValidation({}, {});\n",
                lit::csharp(name),
                lit::csharp(value)
            ));
        }
        out.push_str("request.Content = content;\n");
    }
    out.push_str(
        "\nusing var response = await client.SendAsync(request);\n\n\
         Console.WriteLine((int)response.StatusCode);\n\
         Console.WriteLine(await response.Content.ReadAsStringAsync());",
    );
    out
}

/// PowerShell 7+ `Invoke-WebRequest`. `Content-Type` and `User-Agent` use their dedicated
/// parameters; other headers go in a hashtable (case-insensitive, so repeated names are
/// joined). Non-ASCII text is sent as UTF-8 bytes (older PowerShell versions default to
/// ISO-8859-1 for string bodies). Non-standard methods use `-CustomMethod`. .NET refuses to
/// send non-ASCII header values, which a note points out.
pub(super) fn powershell(p: &Prepared<'_>) -> String {
    let mut out = String::new();
    if let Some(note) = non_ascii_header_note(
        p,
        "Invoke-WebRequest",
        "refuses to send non-ASCII header values (\"Request headers must contain only ASCII \
         characters\")",
    ) {
        out.push_str(&note_line("# ", &note));
    }
    let mut content_type = None;
    let mut user_agent = None;
    let mut headers = Vec::new();
    for (name, value) in p.merged_headers() {
        if name.eq_ignore_ascii_case("content-type") {
            content_type.get_or_insert(value);
        } else if name.eq_ignore_ascii_case("user-agent") {
            user_agent.get_or_insert(value);
        } else {
            headers.push((name, value));
        }
    }
    if !headers.is_empty() {
        out.push_str("$headers = @{\n");
        for (name, value) in &headers {
            out.push_str(&format!("    {} = {}\n", lit::ps(name), lit::ps(value)));
        }
        out.push_str("}\n");
    }
    let body_arg = match &p.body {
        Body::None => None,
        Body::Binary(bytes) => {
            out.push_str(&format!(
                "$body = [System.Convert]::FromBase64String({})\n",
                lit::ps(&base64(bytes))
            ));
            Some("$body")
        }
        body => {
            let text = body.text().unwrap_or_default();
            let literal = lit::ps_here(text).unwrap_or_else(|| lit::ps(text));
            out.push_str(&format!("$body = {literal}\n"));
            Some(if text.is_ascii() {
                "$body"
            } else {
                "([System.Text.Encoding]::UTF8.GetBytes($body))"
            })
        }
    };
    if !out.is_empty() {
        out.push('\n');
    }

    let mut params = vec![format!(
        "$response = Invoke-WebRequest -Uri {}",
        lit::ps(p.url)
    )];
    match p.method {
        "GET" => {}
        "HEAD" | "POST" | "PUT" | "DELETE" | "TRACE" | "OPTIONS" | "PATCH" => {
            let mut method = p.method.to_ascii_lowercase();
            method[..1].make_ascii_uppercase();
            params.push(format!("-Method {method}"));
        }
        other => params.push(format!("-CustomMethod {}", lit::ps(other))),
    }
    if !headers.is_empty() {
        params.push("-Headers $headers".into());
    }
    if let Some(ct) = content_type {
        params.push(format!("-ContentType {}", lit::ps(&ct)));
    }
    if let Some(ua) = user_agent {
        params.push(format!("-UserAgent {}", lit::ps(&ua)));
    }
    if let Some(body) = body_arg {
        params.push(format!("-Body {body}"));
    }
    out.push_str(&params.join(" `\n    "));
    out.push_str("\n\n$response.StatusCode\n$response.Content");
    out
}
