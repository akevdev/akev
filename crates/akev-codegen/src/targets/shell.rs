//! Command-line targets: cURL (bash and cmd), Wget and HTTPie.

use super::base64;
use crate::lit;
use crate::prepare::{Body, Prepared};

/// Header line as curl/wget take it; `Name;` is curl's syntax for an empty value.
fn curl_header(name: &str, value: &str) -> String {
    if value.is_empty() {
        format!("{name};")
    } else {
        format!("{name}: {value}")
    }
}

/// The `-X`/`--head` part of a curl command, if the method isn't implied.
fn curl_method(p: &Prepared<'_>, word: fn(&str) -> String) -> Option<String> {
    match (p.method, p.body.is_none()) {
        ("GET", true) | ("POST", false) => None,
        ("HEAD", true) => Some("--head".to_string()),
        (method, _) => Some(format!("-X {}", word(method))),
    }
}

/// Flags shared by both curl variants, after the body.
fn curl_trailing_flags(p: &Prepared<'_>, args: &mut Vec<String>) {
    if p.accept_encoding.is_some() {
        args.push("--compressed".into());
    }
    if p.url_needs_globoff() {
        args.push("--globoff".into());
    }
    if p.url_has_dot_segments() {
        args.push("--path-as-is".into());
    }
}

/// cURL for POSIX shells.
///
/// Method flags only when not implied; one `-H` per header; text bodies via `--data-raw`
/// (never interprets `@`), form bodies as one `--data-raw` per field; binary bodies piped in
/// through `base64 --decode` and `--data-binary @-`. `Accept-Encoding` becomes
/// `--compressed`; a body without `Content-Type` gets `-H 'Content-Type:'` so curl doesn't add
/// its form default.
pub(super) fn curl(p: &Prepared<'_>) -> String {
    let mut prelude = String::new();
    let mut args = vec![format!("curl {}", lit::sh(p.url))];
    args.extend(curl_method(p, lit::sh_word));
    for &(name, value) in &p.headers {
        args.push(format!("-H {}", lit::sh(&curl_header(name, value))));
    }
    if !p.body.is_none() && p.content_type().is_none() {
        args.push("-H 'Content-Type:'".into());
    }
    match &p.body {
        Body::None => {}
        Body::Form { fields, .. } => {
            args.extend(
                fields
                    .iter()
                    .map(|f| format!("--data-raw {}", lit::sh(f.raw))),
            );
        }
        Body::Json { text, .. } | Body::Text(text) => {
            args.push(format!("--data-raw {}", lit::sh(text)));
        }
        Body::Binary(bytes) => {
            prelude = format!("echo {} | base64 --decode | ", lit::sh(&base64(bytes)));
            args.push("--data-binary @-".into());
        }
    }
    curl_trailing_flags(p, &mut args);
    prelude + &args.join(" \\\n  ")
}

/// Bodies longer than this (or with characters cmd can't carry) go through a file in the cmd
/// variant: cmd.exe lines are limited to 8191 characters.
const CMD_INLINE_BODY_LIMIT: usize = 4000;

/// File the cmd variant decodes non-inlinable bodies into.
const CMD_BODY_FILE: &str = "akev-body.bin";

/// cURL for Windows `cmd.exe`: `^` continuations and cmd/MSVCRT-safe quoting (see
/// [`lit::cmd`]). Bodies that cmd can't carry (binary, `\r`, control characters, or longer than
/// a cmd line allows) are written as base64 lines, decoded with `certutil` into
/// `akev-body.bin` and sent with `--data-binary @akev-body.bin`. Control characters in the
/// URL, method or headers can't be passed at all: they become spaces, and a `rem` note says
/// so.
pub(super) fn curl_windows(p: &Prepared<'_>) -> String {
    let word = |s: &str| {
        if !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        {
            s.to_string()
        } else {
            lit::cmd(s)
        }
    };
    let mut prelude = String::new();
    let mut args = vec![format!("curl {}", lit::cmd(p.url))];
    args.extend(curl_method(p, word));
    for &(name, value) in &p.headers {
        args.push(format!("-H {}", lit::cmd(&curl_header(name, value))));
    }
    if !p.body.is_none() && p.content_type().is_none() {
        args.push("-H \"Content-Type:\"".into());
    }
    let inline_text = p.body.text().filter(|t| {
        t.len() <= CMD_INLINE_BODY_LIMIT && !t.chars().any(|c| c != '\t' && lit::is_invisible(c))
    });
    match (&p.body, inline_text) {
        (Body::None, _) => {}
        (Body::Form { fields, .. }, Some(_)) => {
            args.extend(
                fields
                    .iter()
                    .map(|f| format!("--data-raw {}", lit::cmd(f.raw))),
            );
        }
        (_, Some(text)) => args.push(format!("--data-raw {}", lit::cmd(text))),
        (body, None) => {
            let encoded = base64(body.bytes());
            prelude.push_str("(\n");
            for chunk in encoded.as_bytes().chunks(76) {
                prelude.push_str("echo ");
                prelude.push_str(&String::from_utf8_lossy(chunk));
                prelude.push('\n');
            }
            prelude.push_str(&format!(
                ") > akev-body.b64\ncertutil -f -decode akev-body.b64 {CMD_BODY_FILE} >nul\n"
            ));
            args.push(format!("--data-binary \"@{CMD_BODY_FILE}\""));
        }
    }
    curl_trailing_flags(p, &mut args);
    let lossy = [p.url, p.method]
        .into_iter()
        .chain(p.headers.iter().flat_map(|&(name, value)| [name, value]))
        .any(|s| s.chars().any(lit::cmd_unrepresentable));
    let note = if lossy {
        "rem Note: cmd.exe can't pass control characters such as CR in arguments, so those in \
         the URL, method or headers were replaced with spaces.\n"
    } else {
        ""
    };
    format!("{note}{prelude}{}", args.join(" ^\n  "))
}

/// GNU Wget (1.15+, Wget2). Prints the response body to stdout like curl. `Accept-Encoding`
/// is dropped (Wget doesn't reliably decode responses). Repeated headers are joined into one
/// `--header` (Wget replaces an earlier header of the same name); Wget2 ignores headers with
/// empty values, which a note points out. Binary bodies are decoded into a temporary file for
/// `--body-file` (Wget sizes the body from the file, so a pipe won't do).
pub(super) fn wget(p: &Prepared<'_>) -> String {
    let mut prelude = String::new();
    let mut args = vec!["wget --quiet --output-document=-".to_string()];
    if p.method != "GET" || !p.body.is_none() {
        args.push(format!("--method={}", lit::sh_word(p.method)));
    }
    let headers = p.merged_headers();
    let empty: Vec<&str> = headers
        .iter()
        .filter(|(_, value)| value.trim().is_empty())
        .map(|&(name, _)| name)
        .collect();
    if !empty.is_empty() {
        prelude.push_str(&super::note_line(
            "# ",
            &format!(
                "Wget2 doesn't send headers with empty values ({}).",
                super::join_names(&empty)
            ),
        ));
    }
    for (name, value) in &headers {
        args.push(format!("--header={}", lit::sh(&format!("{name}: {value}"))));
    }
    match &p.body {
        Body::None => {}
        Body::Binary(bytes) => {
            prelude.push_str(&format!(
                "body_file=$(mktemp)\necho {} | base64 --decode > \"$body_file\"\n",
                lit::sh(&base64(bytes))
            ));
            args.push("--body-file=\"$body_file\"".into());
        }
        body => args.push(format!(
            "--body-data={}",
            lit::sh(body.text().unwrap_or_default())
        )),
    }
    args.push(lit::sh(p.url));
    prelude + &args.join(" \\\n  ")
}

/// An HTTPie request item for a header. Values starting with `=`/`@` are escaped so they
/// aren't read as `:=` (raw JSON) or `:@` (file) items; an empty value uses `Name;`.
fn httpie_header(name: &str, value: &str) -> String {
    if value.is_empty() {
        return format!("{name};");
    }
    let escape = if value.starts_with(['=', '@']) {
        "\\"
    } else {
        ""
    };
    format!("{name}:{escape}{value}")
}

/// HTTPie. The body is piped in on stdin (`printf '%s'` keeps it byte-exact; binary goes
/// through `base64 --decode`). `Accept-Encoding` is dropped (HTTPie decodes responses itself).
/// A body without `Content-Type` unsets HTTPie's JSON default with `Content-Type:`.
pub(super) fn httpie(p: &Prepared<'_>) -> String {
    let prelude = match &p.body {
        Body::None => String::new(),
        Body::Binary(bytes) => format!("echo {} | base64 --decode | ", lit::sh(&base64(bytes))),
        body => format!(
            "printf '%s' {} | ",
            lit::sh(body.text().unwrap_or_default())
        ),
    };
    let mut args = vec![format!(
        "http {} {}",
        lit::sh_word(p.method),
        lit::sh(p.url)
    )];
    for &(name, value) in &p.headers {
        args.push(lit::sh(&httpie_header(name, value)));
    }
    if !p.body.is_none() && p.content_type().is_none() {
        args.push("'Content-Type:'".into());
    }
    prelude + &args.join(" \\\n  ")
}
