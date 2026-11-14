//! One module per language family; [`generate`] dispatches on the target.

mod dotnet;
mod go;
mod javascript;
mod jvm;
mod php;
mod python;
mod raw;
mod ruby;
mod rust;
mod shell;
mod swift_dart;

use crate::Target;
use crate::prepare::Prepared;
use base64::Engine as _;
use std::borrow::Cow;

pub(crate) fn generate(target: Target, p: &Prepared<'_>) -> String {
    match target {
        Target::Curl => shell::curl(p),
        Target::CurlWindows => shell::curl_windows(p),
        Target::Wget => shell::wget(p),
        Target::Httpie => shell::httpie(p),
        Target::Powershell => dotnet::powershell(p),
        Target::JsFetch => javascript::fetch(p, javascript::Runtime::Browser),
        Target::NodeFetch => javascript::fetch(p, javascript::Runtime::Node),
        Target::NodeAxios => javascript::axios(p),
        Target::PythonRequests => python::generate(p, python::Library::Requests),
        Target::PythonHttpx => python::generate(p, python::Library::Httpx),
        Target::GoNetHttp => go::generate(p),
        Target::RustReqwest => rust::generate(p),
        Target::JavaHttpClient => jvm::java(p),
        Target::KotlinOkhttp => jvm::kotlin(p),
        Target::CsharpHttpClient => dotnet::csharp(p),
        Target::PhpCurl => php::generate(p),
        Target::RubyNetHttp => ruby::generate(p),
        Target::SwiftUrlSession => swift_dart::swift(p),
        Target::DartHttp => swift_dart::dart(p),
        Target::RawHttp => raw::generate(p),
    }
}

/// Does `target` write JSON bodies as native literals (and so need the parsed value)?
pub(crate) fn uses_native_json(target: Target) -> bool {
    matches!(
        target,
        Target::JsFetch
            | Target::NodeFetch
            | Target::NodeAxios
            | Target::PythonRequests
            | Target::PythonHttpx
            | Target::RubyNetHttp
            | Target::PhpCurl
    )
}

/// Standard base64 (with padding).
pub(crate) fn base64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Does a form's field list repeat a name? (Linear: captured forms can be huge.)
pub(crate) fn has_duplicate_names(fields: &[crate::prepare::FormField<'_>]) -> bool {
    let mut seen = std::collections::HashSet::with_capacity(fields.len());
    !fields.iter().all(|f| seen.insert(f.name.as_str()))
}

/// A (non-empty) note telling the user a GET/HEAD body was left out because the client can't
/// send one.
pub(crate) fn omitted_body_note(client: &str, method: &str) -> String {
    format!("{client} does not allow a body with {method} requests; the request body was omitted.")
}

/// Characters that can't appear in a one-line comment: anything some target language treats
/// as a line break (`\n`, `\r`, VT, FF, NEL, U+2028, U+2029), every other control character,
/// and the invisible/bidi characters behind "Trojan Source" attacks.
fn breaks_comment(c: char) -> bool {
    c == '\n' || crate::lit::is_invisible(c)
}

/// `text` made safe to sit inside a one-line comment of every target language: characters
/// that could end the comment (or hide text) become spaces, and backslashes are doubled so a
/// `\u000a` can't become a line break in Java (which decodes Unicode escapes everywhere,
/// comments included, before lexing). Captured data — header names and values, methods —
/// must never end a comment and turn into code.
pub(crate) fn comment_safe(text: &str) -> Cow<'_, str> {
    if !text.chars().any(|c| c == '\\' || breaks_comment(c)) {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            c if breaks_comment(c) => out.push(' '),
            c => out.push(c),
        }
    }
    Cow::Owned(out)
}

/// A one-line `Note:` comment: `{lead}Note: {text}` plus a newline, where `lead` is the
/// indentation and comment marker (`"// "`, `"        // "`, `"# "`). `text` goes through
/// [`comment_safe`], so it may contain captured data.
pub(crate) fn note_line(lead: &str, text: &str) -> String {
    format!("{lead}Note: {}\n", comment_safe(text))
}

/// `A`, `A and B`, `A, B and C`.
pub(crate) fn join_names(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => (*one).to_string(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

/// A (non-empty) note for clients that can't send non-ASCII header values, or `None` when
/// every header value is ASCII. `problem` completes "<client> …", e.g. "rejects non-ASCII
/// header values".
pub(crate) fn non_ascii_header_note(
    p: &Prepared<'_>,
    client: &str,
    problem: &str,
) -> Option<String> {
    let names = p.non_ascii_header_names();
    if names.is_empty() {
        return None;
    }
    let (subject, verb) = if names.len() == 1 {
        ("value", "needs")
    } else {
        ("values", "need")
    };
    Some(format!(
        "{client} {problem}; the {} header {subject} {verb} to be ASCII for this to work as captured.",
        join_names(&names)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comment_text_cannot_break_out() {
        assert!(matches!(comment_safe("plain text"), Cow::Borrowed(_)));
        assert_eq!(comment_safe("a\nb\rc"), "a b c");
        for c in [
            '\u{85}', '\u{2028}', '\u{2029}', '\u{b}', '\u{c}', '\0', '\u{202e}',
        ] {
            assert_eq!(comment_safe(&format!("x{c}y")), "x y", "{c:?}");
        }
        assert_eq!(comment_safe(r"X\u000aY"), r"X\\u000aY");
        assert_eq!(comment_safe("tab\tok é"), "tab\tok é");
        assert_eq!(note_line("// ", "a\nb"), "// Note: a b\n");
    }

    #[test]
    fn joins_names() {
        assert_eq!(join_names(&[]), "");
        assert_eq!(join_names(&["A"]), "A");
        assert_eq!(join_names(&["A", "B"]), "A and B");
        assert_eq!(join_names(&["A", "B", "C"]), "A, B and C");
    }
}
