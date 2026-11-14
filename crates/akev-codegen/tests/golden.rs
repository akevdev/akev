//! Golden-output tests: every target × every fixture in `tests/common`.
//!
//! Regenerate with `AKEV_UPDATE_GOLDEN=1 cargo test -p akev-codegen --test golden` and
//! review the diff.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use akev_codegen::{Target, generate, targets};

#[test]
fn golden_outputs() {
    let update = std::env::var_os("AKEV_UPDATE_GOLDEN").is_some();
    let mut failures = Vec::new();
    for case in common::cases() {
        for target in Target::ALL {
            let code = generate(target, &case.request);
            let path = common::golden_path(case.name, target);
            if update {
                std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
                std::fs::write(&path, &code).expect("write golden");
                continue;
            }
            match std::fs::read_to_string(&path) {
                Ok(expected) if expected == code => {}
                Ok(expected) => failures.push(format!(
                    "{}/{}: output changed\n--- expected\n{expected}\n--- actual\n{code}\n",
                    case.name,
                    target.id()
                )),
                Err(_) => failures.push(format!(
                    "{}/{}: missing golden file {}",
                    case.name,
                    target.id(),
                    path.display()
                )),
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} golden mismatches:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn targets_metadata() {
    let list = targets();
    assert_eq!(list.len(), 20);
    assert_eq!(list[0].id, Target::Curl);
    assert_eq!(list[0].name, "cURL");
    for info in &list {
        assert!(!info.name.is_empty() && !info.group.is_empty());
        assert!(
            [
                "shell",
                "javascript",
                "python",
                "go",
                "rust",
                "java",
                "kotlin",
                "csharp",
                "php",
                "ruby",
                "swift",
                "dart",
                "http",
                "powershell"
            ]
            .contains(&info.language),
            "{}",
            info.language
        );
        // ids round-trip through serde and FromStr
        let json = serde_json::to_string(&info.id).unwrap();
        assert_eq!(json, format!("\"{}\"", info.id.id()));
        assert_eq!(info.id.id().parse::<Target>().unwrap(), info.id);
    }
    let json = serde_json::to_value(&list[8]).unwrap();
    assert_eq!(
        json,
        serde_json::json!({"id": "python-requests", "name": "Python · requests", "language": "python", "group": "Python"})
    );
    assert!("nope".parse::<Target>().is_err());
}

#[test]
fn no_trailing_newline_except_raw() {
    for case in common::cases() {
        for target in Target::ALL {
            let code = generate(target, &case.request);
            if target != Target::RawHttp {
                assert!(!code.ends_with('\n'), "{}/{}", case.name, target.id());
            }
            assert!(!code.is_empty());
        }
    }
}

/// Bodies of any size are included in full (no placeholders), and large form bodies don't hit
/// quadratic paths (this would take minutes if they did).
#[test]
fn large_bodies_are_included() {
    use base64::Engine as _;
    let text = "row 'quoted' \"double\" $x\n".repeat(60_000); // ~1.5 MB
    let form: String = (0..40_000)
        .map(|i| format!("f{}=v{i}", i % 20_000))
        .collect::<Vec<_>>()
        .join("&");
    let binary: Vec<u8> = (0..1_200_000u32).map(|i| (i % 251) as u8).collect();
    let cases = [
        ("text/plain", text.as_bytes().to_vec()),
        ("application/x-www-form-urlencoded", form.into_bytes()),
        ("application/octet-stream", binary.clone()),
    ];
    for (content_type, body) in cases {
        let req = common::request(
            "POST",
            "https://x.test/",
            &[("Content-Type", content_type)],
            Some(&body),
        );
        for target in Target::ALL {
            let code = generate(target, &req);
            assert!(
                code.len() >= body.len(),
                "{content_type}/{}: body missing",
                target.id()
            );
        }
    }
    let req = common::request("POST", "https://x.test/", &[], Some(&binary));
    let b64 = base64::engine::general_purpose::STANDARD.encode(&binary);
    for target in [
        Target::Curl,
        Target::PythonRequests,
        Target::NodeFetch,
        Target::GoNetHttp,
    ] {
        assert!(generate(target, &req).contains(&b64), "{}", target.id());
    }
}

/// Seeded randomized inputs (odd methods, malformed URLs, strange header names and values,
/// random bodies under every content type) never make a generator panic.
#[test]
fn generators_never_panic() {
    const METHODS: [&str; 8] = [
        "", " get ", "post", "PROPFIND", "M-SEARCH", "GET\n", "délète", "HEAD",
    ];
    const URLS: [&str; 10] = [
        "",
        "::",
        "http://",
        "http://[::1",
        "https://user:pa%ss@例え.jp:99999/ü?q=%zz#a#b",
        "http://x.test",
        "http://x.test?only=query",
        "ftp://x.test/a b",
        "relative/path?x=1",
        "http://x.test/a/./b/../c/%2e%2E/d?e=[f]&g={h}",
    ];
    const NAMES: [&str; 10] = [
        "",
        ":path",
        "Host",
        "content-type",
        "X-A",
        "x a",
        "Cookie",
        "Ä",
        "Content-Length",
        "__proto__",
    ];
    const VALUES: [&str; 8] = [
        "",
        " ",
        "v",
        "a\r\nb",
        "日本",
        "\u{0}\u{1b}",
        "=@x",
        "multipart/form-data; boundary=\"q\"",
    ];
    const TYPES: [&str; 6] = [
        "application/json",
        "application/x-www-form-urlencoded",
        "text/plain",
        "application/octet-stream",
        "application/vnd.x+json; charset=utf-8",
        "",
    ];
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state as usize
    };
    for _ in 0..300 {
        let mut headers: Vec<(String, String)> = (0..next() % 6)
            .map(|_| {
                (
                    NAMES[next() % NAMES.len()].to_string(),
                    VALUES[next() % VALUES.len()].to_string(),
                )
            })
            .collect();
        let content_type = TYPES[next() % TYPES.len()];
        if !content_type.is_empty() {
            headers.push(("Content-Type".into(), content_type.into()));
        }
        let body: Option<Vec<u8>> = match next() % 5 {
            0 => None,
            1 => Some(Vec::new()),
            2 => Some((0..next() % 64).map(|_| (next() % 256) as u8).collect()),
            3 => Some(br#"{"a":[1,2.50,{"b":null}],"c":"d\u0000","a":1}"#.to_vec()),
            _ => Some(b"a=1&b=%ZZ&&c=d+e%20f&=x&g".to_vec()),
        };
        let req = akev_codegen::SnippetRequest {
            method: METHODS[next() % METHODS.len()].into(),
            url: URLS[next() % URLS.len()].into(),
            headers: akev_core::Headers(headers),
            body: body.map(Into::into),
        };
        for target in Target::ALL {
            let _ = generate(target, &req);
        }
    }
}
