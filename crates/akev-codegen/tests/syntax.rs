//! Syntax validation of generated snippets with real tools. Each check is skipped (with a note
//! on stderr) when its tool isn't installed: `bash -n`, Python's `ast.parse`, `node --check`,
//! `javac`, `gofmt`, `rustfmt`, `php -l`, `ruby -c`, `swiftc -parse`, and PowerShell's parser.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use akev_codegen::{SnippetRequest, Target, generate};
use std::path::Path;
use std::process::Command;

/// Extra requests beyond the golden fixtures that stress the escapers.
fn extra_requests() -> Vec<(&'static str, SnippetRequest)> {
    let r = common::request;
    let all_controls: String = (1u8..0x20)
        .map(char::from)
        .chain(['\u{7f}', '\u{85}', '\u{2028}', '\u{202e}', '\u{feff}'])
        .collect();
    vec![
        (
            "controls",
            r(
                "POST",
                "https://x.test/",
                &[("Content-Type", "text/plain")],
                Some(format!("a{all_controls}b").as_bytes()),
            ),
        ),
        (
            "quotes_only",
            r("POST", "https://x.test/", &[], Some(br#""""''''"""```"#)),
        ),
        (
            "trailing_ws",
            r(
                "POST",
                "https://x.test/",
                &[],
                Some(b"line with trailing space \nline with tab\t\n   \nend\n"),
            ),
        ),
        (
            "hashes",
            r(
                "POST",
                "https://x.test/",
                &[],
                Some(br###""#"## \#( #{x} ${x} $x"###),
            ),
        ),
        (
            "only_newlines",
            r("POST", "https://x.test/", &[], Some(b"\n\n")),
        ),
        (
            "leading_quote",
            r("POST", "https://x.test/", &[], Some(b"\"start and end\"")),
        ),
        (
            "dollar_brace",
            r(
                "PUT",
                "https://x.test/$x?a=${b}",
                &[("X-A", "${c} $(d) `e`")],
                Some(b"${x}\n$y `z` \\${w}"),
            ),
        ),
        (
            "get_with_body",
            r(
                "GET",
                "https://x.test/",
                &[("Content-Type", "application/json")],
                Some(b"{\"a\":1}"),
            ),
        ),
        ("head", r("HEAD", "https://x.test/", &[], None)),
        (
            "options",
            r(
                "OPTIONS",
                "https://x.test/",
                &[("Content-Language", "en")],
                None,
            ),
        ),
        (
            "post_empty",
            r(
                "POST",
                "https://x.test/",
                &[("Content-Type", "application/json")],
                None,
            ),
        ),
        (
            "form_dupes",
            r(
                "POST",
                "https://x.test/",
                &[("Content-Type", "application/x-www-form-urlencoded")],
                Some(b"a=1&a=2&1=x&__proto__=y"),
            ),
        ),
        (
            "json_proto",
            r(
                "POST",
                "https://x.test/",
                &[("Content-Type", "application/json")],
                Some(br##"{"__proto__":{"x":1},"a-b":[{}],"$c":"#{d}"}"##),
            ),
        ),
        (
            "json_array",
            r(
                "POST",
                "https://x.test/",
                &[("Content-Type", "application/json")],
                Some(br#"[1,"two",[3],{"four":4},[],{}]"#),
            ),
        ),
        (
            "windows_path",
            r(
                "POST",
                "https://x.test/",
                &[("X-Path", "C:\\dir\\")],
                Some(b"C:\\Program Files\\\"x\"\\"),
            ),
        ),
        (
            "header_limits",
            r(
                "delete",
                "http://10.0.0.1:8080/a b/{c}?d=\"e\"#f#g",
                &[
                    ("Host", "api.example.com"),
                    ("Accept-Encoding", "gzip"),
                    ("Cookie", "a=1"),
                    ("Sec-Fetch-Mode", "cors"),
                    ("X-Name", "résumé 日本.pdf"),
                    ("x-name", "ü"),
                    ("X-Empty", ""),
                ],
                None,
            ),
        ),
    ]
}

fn all_requests() -> Vec<(String, SnippetRequest)> {
    common::cases()
        .into_iter()
        .map(|c| (c.name.to_string(), c.request))
        .chain(
            extra_requests()
                .into_iter()
                .map(|(n, r)| (n.to_string(), r)),
        )
        .collect()
}

fn run(cmd: &mut Command) -> Result<(), String> {
    let output = cmd.output().map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

/// Write every snippet for `targets` to a file and run `check(path)` on it.
/// Checks run in parallel (one thread per request) since compilers are slow to start.
fn check_targets(
    label: &str,
    targets: &[Target],
    file_name: impl Fn(&str, Target) -> String + Sync,
    check: impl Fn(&Path) -> Result<(), String> + Sync,
) {
    let dir = common::temp_dir(label);
    let failures = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for (name, request) in all_requests() {
            let (dir, failures, file_name, check) = (&dir, &failures, &file_name, &check);
            scope.spawn(move || {
                for &target in targets {
                    let code = generate(target, &request);
                    let path = dir.join(file_name(&name, target));
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent).expect("mkdir");
                    }
                    std::fs::write(&path, &code).expect("write snippet");
                    if let Err(e) = check(&path) {
                        failures
                            .lock()
                            .unwrap()
                            .push(format!("{name}/{}: {e}\n{code}\n", target.id()));
                    }
                }
            });
        }
    });
    let failures = failures.into_inner().unwrap();
    assert!(
        failures.is_empty(),
        "{} invalid snippets:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

fn skip(tool: &str) {
    eprintln!("skipping: {tool} not available");
}

#[test]
fn shell_snippets_parse_with_bash() {
    if !common::have("bash", &["-c", "true"]) {
        return skip("bash");
    }
    check_targets(
        "bash",
        &[Target::Curl, Target::Wget, Target::Httpie],
        |n, t| format!("{n}-{}.sh", t.id()),
        |p| run(Command::new("bash").arg("-n").arg(p)),
    );
}

#[test]
fn python_snippets_parse() {
    if !common::have("python3", &["-c", "import ast"]) {
        return skip("python3");
    }
    check_targets(
        "python",
        &[Target::PythonRequests, Target::PythonHttpx],
        |n, t| format!("{n}-{}.py", t.id()),
        |p| {
            run(Command::new("python3")
                .args([
                    "-I",
                    "-c",
                    "import ast,sys; ast.parse(open(sys.argv[1], encoding='utf-8').read())",
                ])
                .arg(p))
        },
    );
}

#[test]
fn javascript_snippets_parse_with_node() {
    if !common::have("node", &["--version"]) {
        return skip("node");
    }
    check_targets(
        "node",
        &[Target::JsFetch, Target::NodeFetch, Target::NodeAxios],
        // Top-level await needs module syntax.
        |n, t| format!("{n}-{}.mjs", t.id()),
        |p| run(Command::new("node").arg("--check").arg(p)),
    );
}

#[test]
fn java_snippets_compile() {
    if !common::have("javac", &["-version"]) {
        return skip("javac");
    }
    check_targets(
        "java",
        &[Target::JavaHttpClient],
        |n, _| format!("{n}/Main.java"),
        |p| {
            let out = p.parent().expect("parent").join("classes");
            run(Command::new("javac")
                .arg("-Xlint:none")
                .arg("-d")
                .arg(out)
                .arg(p))
        },
    );
}

#[test]
fn go_snippets_are_gofmt_clean() {
    // `gofmt` has no version flag; probe it with a trivial file.
    let probe = common::temp_dir("gofmt-probe").join("probe.go");
    std::fs::write(&probe, "package main\n").expect("write probe");
    if !common::have("gofmt", &["-l", &probe.to_string_lossy()]) {
        return skip("gofmt");
    }
    check_targets(
        "go",
        &[Target::GoNetHttp],
        |n, _| format!("{n}.go"),
        |p| {
            // Snippets deliberately lack the final newline gofmt adds to files.
            let mut code = std::fs::read_to_string(p).map_err(|e| e.to_string())?;
            code.push('\n');
            std::fs::write(p, code).map_err(|e| e.to_string())?;
            let output = Command::new("gofmt")
                .arg("-l")
                .arg("-e")
                .arg(p)
                .output()
                .map_err(|e| e.to_string())?;
            if output.status.success() && output.stdout.is_empty() {
                Ok(())
            } else {
                Err(format!(
                    "not gofmt-clean: {}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                ))
            }
        },
    );
}

#[test]
fn rust_snippets_parse_with_rustfmt() {
    if !common::have("rustfmt", &["--version"]) {
        return skip("rustfmt");
    }
    // Strict: the snippet must already be rustfmt-formatted (modulo the final newline).
    check_targets(
        "rust",
        &[Target::RustReqwest],
        |n, _| format!("{n}.rs"),
        |p| {
            let code = std::fs::read_to_string(p).map_err(|e| e.to_string())?;
            std::fs::write(p, format!("{code}\n")).map_err(|e| e.to_string())?;
            run(Command::new("rustfmt")
                .args(["--edition", "2021", "--check"])
                .arg(p))
        },
    );
}

#[test]
fn php_snippets_lint() {
    if !common::have("php", &["--version"]) {
        return skip("php");
    }
    check_targets(
        "php",
        &[Target::PhpCurl],
        |n, _| format!("{n}.php"),
        |p| run(Command::new("php").arg("-l").arg(p)),
    );
}

#[test]
fn ruby_snippets_parse() {
    if !common::have("ruby", &["--version"]) {
        return skip("ruby");
    }
    check_targets(
        "ruby",
        &[Target::RubyNetHttp],
        |n, _| format!("{n}.rb"),
        |p| run(Command::new("ruby").arg("-c").arg(p)),
    );
}

#[test]
fn swift_snippets_parse() {
    if !common::have("swiftc", &["--version"]) {
        return skip("swiftc");
    }
    check_targets(
        "swift",
        &[Target::SwiftUrlSession],
        |n, _| format!("{n}.swift"),
        |p| run(Command::new("swiftc").arg("-parse").arg(p)),
    );
}

#[test]
fn powershell_snippets_parse() {
    if !common::have("pwsh", &["-NoProfile", "-Command", "exit 0"]) {
        return skip("pwsh");
    }
    check_targets(
        "pwsh",
        &[Target::Powershell],
        |n, _| format!("{n}.ps1"),
        |p| {
            // `-Command` joins any further arguments into the command text (they never reach
            // `$args`), so the path goes into the script as a single-quoted literal.
            let path = p.display().to_string().replace('\'', "''");
            let script = format!(
                "$errors = $null; [System.Management.Automation.Language.Parser]::ParseFile('{path}', [ref]$null, [ref]$errors) | Out-Null; if ($errors.Count) {{ $errors | ForEach-Object {{ $_.ToString() }}; exit 1 }}"
            );
            run(Command::new("pwsh").args(["-NoProfile", "-NonInteractive", "-Command", &script]))
        },
    );
}
