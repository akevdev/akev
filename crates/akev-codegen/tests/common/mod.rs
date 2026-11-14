//! Shared fixtures for the integration tests.
#![allow(dead_code)]

use akev_codegen::{SnippetRequest, Target};
use akev_core::Headers;
use bytes::Bytes;
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct Case {
    pub name: &'static str,
    pub request: SnippetRequest,
}

pub fn request(
    method: &str,
    url: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
) -> SnippetRequest {
    SnippetRequest {
        method: method.into(),
        url: url.into(),
        headers: Headers(
            headers
                .iter()
                .map(|(n, v)| (n.to_string(), v.to_string()))
                .collect(),
        ),
        body: body.map(Bytes::copy_from_slice),
    }
}

/// A PNG header plus bytes that are invalid UTF-8 and include NUL.
pub const PNG_BYTES: &[u8] = &[
    0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n', 0, 0, 0, 0x0d, b'I', b'H', b'D', b'R', 0, 0,
    0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1f, 0x15, 0xc4, 0x89, 0xff, 0xfe, b'"', b'\'', b'\\',
];

pub const TRICKY_TEXT: &str = "Line 1: it's \"quoted\" and 'single'\n\
Line 2: $HOME ${HOME} `whoami` $(id) %PATH% 100% #{x} #@y !!\n\
\tTabbed: back\\slash \\n \\\\ \"\"\" ''' ``` --%\n\
Unicode: héllo — 日本語 🚀 ‘smart’ “quotes”\n\
'@ at line start, and a trailing backslash \\";

/// The canonical golden cases (also syntax-checked and, with `--ignored`, executed).
pub fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "get_headers",
            request: request(
                "GET",
                "https://api.example.com/v1/items?page=2&sort=-created",
                &[
                    (":authority", "api.example.com"),
                    ("Host", "api.example.com"),
                    ("Accept", "application/json"),
                    ("Authorization", "Bearer eyJhbGciOiJIUzI1NiJ9.e30.abc"),
                    ("Accept-Encoding", "gzip, deflate, br"),
                    ("User-Agent", "Akev/1.0"),
                    ("X-Request-Id", "42"),
                    ("Connection", "keep-alive"),
                ],
                None,
            ),
        },
        Case {
            name: "post_json",
            request: request(
                "POST",
                "https://api.example.com/v1/users",
                &[
                    ("Content-Type", "application/json"),
                    ("Content-Length", "121"),
                    ("Accept", "application/json"),
                ],
                Some(
                    r#"{"name":"Ann O'Neil","age":34,"admin":false,"tags":["a","b"],"address":{"city":"Zürich","zip":null},"score":9.5,"meta":{}}"#
                        .as_bytes(),
                ),
            ),
        },
        Case {
            name: "post_form",
            request: request(
                "POST",
                "https://example.com/login",
                &[
                    ("Content-Type", "application/x-www-form-urlencoded"),
                    ("Cookie", "session=abc123; theme=dark"),
                    ("Origin", "https://example.com"),
                ],
                Some(b"user=ann%40example.com&password=p%40ss+w0rd%21&remember=on"),
            ),
        },
        Case {
            name: "binary",
            request: request(
                "PUT",
                "https://upload.example.com/files/logo.png",
                &[("Content-Type", "image/png"), ("X-Upload-Token", "t0k3n")],
                Some(PNG_BYTES),
            ),
        },
        Case {
            name: "tricky",
            request: request(
                "PATCH",
                "https://example.com/api/notes/1?q=a%20b&x=$y&z='\"",
                &[
                    ("Content-Type", "text/plain; charset=utf-8"),
                    ("X-Quote", "it's \"quoted\" `cmd` $HOME %PATH% \\back\\"),
                    ("X-Unicode", "héllo 日本 ✓"),
                    ("X-Eq", "=starts-with-equals"),
                ],
                Some(TRICKY_TEXT.as_bytes()),
            ),
        },
        Case {
            name: "json_edge",
            request: request(
                "POST",
                "https://example.com/graphql",
                &[("Content-Type", "application/json; charset=utf-8")],
                Some(
                    "{\n  \"__proto__\": {\"polluted\": true},\n  \"0\": \"zero\",\n  \"big\": 9007199254740993,\n  \"text\": \"line\\nbreak \\\"q\\\" $x #{y} \\u00e9\",\n  \"empty\": [],\n  \"nested\": [[1, 2], {\"a\": null}],\n  \"neg\": -1.5e-7\n}"
                        .as_bytes(),
                ),
            ),
        },
        Case {
            name: "crlf_text",
            request: request(
                "POST",
                "https://example.com/upload",
                &[("Content-Type", "multipart/form-data; boundary=XyZ")],
                Some(b"--XyZ\r\nContent-Disposition: form-data; name=\"note\"\r\n\r\nhi \x1b[1m\"there\"\r\n--XyZ--\r\n"),
            ),
        },
        Case {
            name: "custom_method",
            request: request(
                "purge",
                "http://cache.local:8080/a/../b/[1]?x={y}",
                &[
                    ("Host", "example.com"),
                    ("X-Dup", "one"),
                    ("x-dup", "two"),
                    ("X-Empty", ""),
                    ("Transfer-Encoding", "chunked"),
                    ("Expect", "100-continue"),
                ],
                Some(b"plain body without a content type"),
            ),
        },
    ]
}

/// The golden file for a case/target.
pub fn golden_path(case: &str, target: Target) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(case)
        .join(format!("{}.{}", target.id(), target.file_extension()))
}

/// Is `program` runnable (exits successfully with `args`)?
pub fn have(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// A fresh temporary directory under the target dir.
pub fn temp_dir(label: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "{label}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}
