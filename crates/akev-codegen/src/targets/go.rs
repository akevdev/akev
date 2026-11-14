//! Go `net/http`.

use super::base64;
use crate::lit;
use crate::prepare::{Body, Prepared};

fn method_expr(method: &str) -> String {
    let constant = match method {
        "GET" => "MethodGet",
        "HEAD" => "MethodHead",
        "POST" => "MethodPost",
        "PUT" => "MethodPut",
        "PATCH" => "MethodPatch",
        "DELETE" => "MethodDelete",
        "CONNECT" => "MethodConnect",
        "OPTIONS" => "MethodOptions",
        "TRACE" => "MethodTrace",
        other => return lit::go(other),
    };
    format!("http.{constant}")
}

/// Go `net/http` (gofmt-formatted). Text bodies use raw strings where possible; binary bodies
/// are base64-decoded. `Accept-Encoding` is dropped: Go's transport only decompresses
/// transparently when it set that header itself. A differing `Host` sets `req.Host`.
pub(super) fn generate(p: &Prepared<'_>) -> String {
    let mut imports = vec!["fmt", "io", "net/http"];
    let mut body_setup = String::new();
    let body_arg = match &p.body {
        Body::None => "nil",
        Body::Binary(bytes) => {
            imports.extend(["bytes", "encoding/base64"]);
            body_setup = format!(
                "\tdata, err := base64.StdEncoding.DecodeString({})\n\tif err != nil {{\n\t\tpanic(err)\n\t}}\n\tbody := bytes.NewReader(data)\n\n",
                lit::go(&base64(bytes))
            );
            "body"
        }
        body => {
            imports.push("strings");
            body_setup = format!(
                "\tbody := strings.NewReader({})\n\n",
                lit::go_text(body.text().unwrap_or_default())
            );
            "body"
        }
    };
    imports.sort_unstable();

    let mut out = String::from("package main\n\nimport (\n");
    for import in imports {
        out.push_str(&format!("\t\"{import}\"\n"));
    }
    out.push_str(")\n\nfunc main() {\n");
    out.push_str(&body_setup);
    out.push_str(&format!(
        "\treq, err := http.NewRequest({}, {}, {body_arg})\n\tif err != nil {{\n\t\tpanic(err)\n\t}}\n",
        method_expr(p.method),
        lit::go(p.url)
    ));
    let mut host = None;
    for &(name, value) in &p.headers {
        if name.eq_ignore_ascii_case("host") {
            host.get_or_insert(value);
        } else {
            out.push_str(&format!(
                "\treq.Header.Add({}, {})\n",
                lit::go(name),
                lit::go(value)
            ));
        }
    }
    if let Some(host) = host {
        out.push_str(&format!("\treq.Host = {}\n", lit::go(host)));
    }
    out.push_str(
        "\n\tres, err := http.DefaultClient.Do(req)\n\tif err != nil {\n\t\tpanic(err)\n\t}\n\
         \tdefer res.Body.Close()\n\n\
         \tresBody, err := io.ReadAll(res.Body)\n\tif err != nil {\n\t\tpanic(err)\n\t}\n\n\
         \tfmt.Println(res.Status)\n\tfmt.Println(string(resBody))\n}",
    );
    out
}
