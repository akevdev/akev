//! QuickJS scripting tests.
#![cfg(feature = "scripting")]

use super::helpers::*;
use crate::script::{ScriptRequestOutcome, run_on_request, run_on_response};
use crate::*;
use akev_core::http::HttpVersion;
use std::time::Instant;

fn cont(outcome: ScriptRequestOutcome) -> (RequestHead, Bytes) {
    match outcome {
        ScriptRequestOutcome::Continue { head, body } => (head, body),
        ScriptRequestOutcome::Respond { head, .. } => panic!("unexpected response {}", head.status),
    }
}

fn respond(outcome: ScriptRequestOutcome) -> (ResponseHead, Bytes) {
    match outcome {
        ScriptRequestOutcome::Respond { head, body } => (head, body),
        ScriptRequestOutcome::Continue { head, .. } => {
            panic!("unexpected continue to {}", head.url)
        }
    }
}

fn response(status: u16) -> ResponseHead {
    ResponseHead {
        status,
        status_message: akev_core::http::canonical_reason(status).to_string(),
        http_version: HttpVersion::Http11,
        headers: Headers(vec![
            ("Content-Type".into(), "text/plain".into()),
            ("X-Up".into(), "1".into()),
        ]),
    }
}

#[test]
fn modifies_request() {
    let src = r#"
        function onRequest(req) {
          req.headers["X-Debug"] = "1";
          req.method = "put";
          req.body = req.body.toUpperCase();
          return req;
        }
    "#;
    let head = with_headers(
        req("POST", "http://example.com/a"),
        &[("Content-Type", "text/plain")],
    );
    let (out, log) = run_on_request(src, head, Bytes::from_static(b"hello")).unwrap();
    let (h, b) = cont(out);
    assert_eq!(h.method, "PUT");
    assert_eq!(h.headers.get("x-debug"), Some("1"));
    assert_eq!(h.headers.get("content-type"), Some("text/plain"));
    assert_eq!(&b[..], b"HELLO");
    assert!(log.lines.is_empty());
}

#[test]
fn url_change_updates_host_unless_preserved() {
    let src =
        r#"function onRequest(req) { req.url = "https://other.test:8443/new?x=1"; return req; }"#;
    let (out, _) = run_on_request(src, req("GET", "http://example.com/old"), Bytes::new()).unwrap();
    let (h, _) = cont(out);
    assert_eq!(
        (h.scheme.as_str(), h.host.as_str(), h.port, h.path.as_str()),
        ("https", "other.test", 8443, "/new?x=1")
    );
    assert_eq!(h.headers.get("host"), Some("other.test:8443"));

    let src =
        r#"function onRequest(req) { req.url = "http://other.test/"; req.preserveHost = true; }"#;
    let (out, _) = run_on_request(src, req("GET", "http://example.com/old"), Bytes::new()).unwrap();
    let (h, _) = cont(out);
    assert_eq!(
        (h.host.as_str(), h.headers.get("host")),
        ("other.test", Some("example.com"))
    );

    let src = r#"function onRequest(req) { req.url = "http://other.test/"; req.headers.Host = "custom.test"; return req; }"#;
    let (out, _) = run_on_request(src, req("GET", "http://example.com/old"), Bytes::new()).unwrap();
    assert_eq!(cont(out).0.headers.get("host"), Some("custom.test"));

    let src = r#"function onRequest(req) { req.url = "not a url"; return req; }"#;
    assert!(matches!(
        run_on_request(src, req("GET", "http://example.com/"), Bytes::new()),
        Err(RulesError::Script(_))
    ));
}

#[test]
fn short_circuit_response() {
    let src = r#"
        function onRequest(req) {
          if (req.url.indexOf("/blocked") >= 0) {
            return { status: 403, json: { error: "blocked", method: req.method } };
          }
        }
    "#;
    let (out, _) =
        run_on_request(src, req("DELETE", "https://x.test/blocked"), Bytes::new()).unwrap();
    let (h, b) = respond(out);
    assert_eq!((h.status, h.status_message.as_str()), (403, "Forbidden"));
    assert_eq!(h.headers.get("content-type"), Some("application/json"));
    assert_eq!(
        h.headers.get("content-length"),
        Some(b.len().to_string().as_str())
    );
    assert!(h.headers.contains("date"));
    assert_eq!(&b[..], br#"{"error":"blocked","method":"DELETE"}"#);
    // Not blocked → continue unchanged (returning nothing).
    let original = req("GET", "https://x.test/ok");
    let (out, _) = run_on_request(src, original.clone(), Bytes::from_static(b"\x00\xff")).unwrap();
    let (h, b) = cont(out);
    assert_eq!(h, original);
    assert_eq!(&b[..], b"\x00\xff", "binary body untouched");

    let src = r#"function onRequest(req) { return { status: 200, statusMessage: "Fine", headers: { "X-A": ["1", "2"] }, body: "abc" }; }"#;
    let (out, _) = run_on_request(src, req("HEAD", "http://x.test/"), Bytes::new()).unwrap();
    let (h, b) = respond(out);
    assert_eq!(h.status_message, "Fine");
    assert_eq!(h.headers.get_all("x-a").collect::<Vec<_>>(), vec!["1", "2"]);
    assert!(b.is_empty(), "HEAD → no body");
    assert_eq!(h.headers.get("content-length"), Some("3"));

    let src = r#"function onRequest(req) { return { status: 99999 }; }"#;
    assert!(run_on_request(src, req("GET", "http://x.test/"), Bytes::new()).is_err());
}

#[test]
fn modifies_response() {
    let src = r#"
        function onResponse(req, res) {
          console.log("seen", req.method, req.url, res.status);
          res.body = res.body.replace("foo", "bar");
          res.headers["X-Script"] = "yes";
          delete res.headers["X-Up"];
          if (req.url.indexOf("/missing") >= 0) res.status = 404;
          return res;
        }
    "#;
    let r = req("GET", "http://x.test/missing");
    let (h, b, log) =
        run_on_response(src, &r, response(200), Bytes::from_static(b"foo foo")).unwrap();
    assert_eq!((h.status, h.status_message.as_str()), (404, "Not Found"));
    assert_eq!(&b[..], b"bar foo");
    assert_eq!(h.headers.get("x-script"), Some("yes"));
    assert!(!h.headers.contains("x-up"));
    assert_eq!(h.headers.get("content-type"), Some("text/plain"));
    assert_eq!(log.lines, vec!["[log] seen GET http://x.test/missing 200"]);

    // Untouched response keeps exact headers and bytes.
    let src = r#"function onResponse(req, res) { return res; }"#;
    let original = response(200);
    let (h, b, _) =
        run_on_response(src, &r, original.clone(), Bytes::from_static(b"\xfe\xff")).unwrap();
    assert_eq!(h, original);
    assert_eq!(&b[..], b"\xfe\xff");

    // Explicit status message, JSON via `json`.
    let src = r#"function onResponse(req, res) { return { status: 201, statusMessage: "Made", json: [1, 2] }; }"#;
    let (h, b, _) = run_on_response(src, &r, response(200), Bytes::from_static(b"x")).unwrap();
    assert_eq!(
        (h.status, h.status_message.as_str(), &b[..]),
        (201, "Made", &b"[1,2]"[..])
    );
    assert_eq!(
        h.headers.get("content-type"),
        Some("text/plain"),
        "existing content type kept"
    );
}

#[test]
fn base64_bodies() {
    let src = r#"
        function onResponse(req, res) {
          if (res.bodyBase64 !== "AP8=") throw new Error("unexpected " + res.bodyBase64);
          res.bodyBase64 = "AQID";
          return res;
        }
    "#;
    let r = req("GET", "http://x.test/");
    let (_, b, _) = run_on_response(src, &r, response(200), Bytes::from_static(&[0, 255])).unwrap();
    assert_eq!(&b[..], &[1, 2, 3]);
    let src = r#"function onRequest(req) { req.bodyBase64 = "!!"; return req; }"#;
    assert!(run_on_request(src, req("POST", "http://x.test/"), Bytes::new()).is_err());
}

#[test]
fn console_capture() {
    let src = r#"
        function onRequest(req) {
          console.log("hello", 42, { a: [1, 2] }, null, undefined, true);
          console.info("info");
          console.warn("careful");
          console.error(new TypeError("bad"));
          console.debug(function named() {});
        }
    "#;
    let (_, log) = run_on_request(src, req("GET", "http://x.test/"), Bytes::new()).unwrap();
    assert_eq!(
        log.lines[0],
        r#"[log] hello 42 {"a":[1,2]} null undefined true"#
    );
    assert_eq!(log.lines[1], "[info] info");
    assert_eq!(log.lines[2], "[warn] careful");
    assert!(
        log.lines[3].starts_with("[error] TypeError: bad"),
        "{}",
        log.lines[3]
    );
    assert_eq!(log.lines[4], "[debug] [Function: named]");

    let src = r#"function onRequest(req) { for (var i = 0; i < 1500; i++) console.log(i); }"#;
    let (_, log) = run_on_request(src, req("GET", "http://x.test/"), Bytes::new()).unwrap();
    assert_eq!(log.lines.len(), 1001);
    assert!(log.lines[1000].contains("truncated"));
}

#[test]
fn timeout_is_enforced() {
    let start = Instant::now();
    let src = "function onRequest(req) { while (true) {} }";
    let err = run_on_request(src, req("GET", "http://x.test/"), Bytes::new()).unwrap_err();
    let elapsed = start.elapsed();
    assert!(err.to_string().contains("CPU time limit"), "{err}");
    assert!(elapsed.as_secs_f64() < 4.0, "took {elapsed:?}");
    assert!(elapsed.as_secs_f64() >= 1.9, "took {elapsed:?}");
    // try/catch can't swallow the interrupt.
    let src = "function onResponse(q, r) { while (true) { try { while (true) {} } catch (e) {} } }";
    let err = run_on_response(
        src,
        &req("GET", "http://x.test/"),
        response(200),
        Bytes::new(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("CPU time limit"), "{err}");
}

#[test]
fn memory_limit_is_enforced() {
    let src = "function onRequest(req) { var a = []; while (true) { a.push(new Array(1000000).fill(1)); } }";
    let err = run_on_request(src, req("GET", "http://x.test/"), Bytes::new()).unwrap_err();
    assert!(err.to_string().contains("memory"), "{err}");
    let src = "function onRequest(req) { var s = 'x'; while (true) { s = s + s; } }";
    let err = run_on_request(src, req("GET", "http://x.test/"), Bytes::new()).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("memory") || msg.contains("length"), "{msg}");
}

#[test]
fn errors_carry_line_numbers() {
    let src = "function onRequest(req) {\n  var x = 1;\n  return req.;\n}";
    let err = run_on_request(src, req("GET", "http://x.test/"), Bytes::new())
        .unwrap_err()
        .to_string();
    assert!(err.contains("SyntaxError"), "{err}");
    assert!(err.contains("line 3"), "{err}");

    let src = "function onRequest(req) {\n  var a = 1;\n  undefinedFunction();\n}";
    let err = run_on_request(src, req("GET", "http://x.test/"), Bytes::new())
        .unwrap_err()
        .to_string();
    assert!(err.contains("ReferenceError"), "{err}");
    assert!(err.contains("line 3"), "{err}");

    let src = "function onRequest(req) { throw { code: 7 }; }";
    let err = run_on_request(src, req("GET", "http://x.test/"), Bytes::new())
        .unwrap_err()
        .to_string();
    assert!(err.contains(r#"{"code":7}"#), "{err}");

    let src = "function onRequest(req) { return 5; }";
    assert!(run_on_request(src, req("GET", "http://x.test/"), Bytes::new()).is_err());
    let src = "function onRequest(r) { function f() { return f(); } return f(); }";
    let err = run_on_request(src, req("GET", "http://x.test/"), Bytes::new())
        .unwrap_err()
        .to_string();
    assert!(err.to_lowercase().contains("stack"), "{err}");
}

#[test]
fn hook_styles() {
    let r = req("GET", "http://x.test/");
    // Missing hooks → unchanged.
    let (out, _) = run_on_request(
        "function onResponse(q, r) { return r; }",
        r.clone(),
        Bytes::from_static(b"b"),
    )
    .unwrap();
    assert_eq!(cont(out).0, r);
    let (h, _, _) =
        run_on_response("function onRequest(q) {}", &r, response(200), Bytes::new()).unwrap();
    assert_eq!(h.status, 200);
    // const arrow functions.
    let src =
        r#"const onRequest = (req) => ({ ...req, headers: { ...req.headers, "x-arrow": "1" } });"#;
    let (out, _) = run_on_request(src, r.clone(), Bytes::new()).unwrap();
    assert_eq!(cont(out).0.headers.get("x-arrow"), Some("1"));
    // async hooks.
    let src = r#"async function onRequest(req) { await null; return { status: 202 }; }"#;
    let (out, _) = run_on_request(src, r.clone(), Bytes::new()).unwrap();
    assert_eq!(respond(out).0.status, 202);
    let src = r#"async function onRequest(req) { await new Promise(function () {}); }"#;
    let err = run_on_request(src, r.clone(), Bytes::new())
        .unwrap_err()
        .to_string();
    assert!(err.contains("never settled"), "{err}");
    // Each call is isolated.
    let src = "var counter = (typeof counter === 'number' ? counter : 0) + 1; function onRequest(req) { req.headers['x-count'] = String(counter); return req; }";
    for _ in 0..2 {
        let (out, _) = run_on_request(src, r.clone(), Bytes::new()).unwrap();
        assert_eq!(cont(out).0.headers.get("x-count"), Some("1"));
    }
}

#[test]
fn headers_round_trip() {
    let r = with_headers(
        req("GET", "http://x.test/"),
        &[("X-B", "1"), ("Accept", "a"), ("x-b", "2")],
    );
    // Touching the body only keeps header order and duplicates exactly.
    let src = r#"function onRequest(req) { req.body = "changed"; return req; }"#;
    let (out, _) = run_on_request(src, r.clone(), Bytes::new()).unwrap();
    let (h, b) = cont(out);
    assert_eq!(h.headers, r.headers);
    assert_eq!(&b[..], b"changed");
    // Repeated headers are exposed as arrays under the first spelling.
    let src = r#"function onRequest(req) { if (req.headers["X-B"].join() !== "1,2") throw new Error("bad"); req.headers["X-B"] = req.headers["X-B"].concat("3"); return req; }"#;
    let (out, _) = run_on_request(src, r.clone(), Bytes::new()).unwrap();
    assert_eq!(
        cont(out).0.headers.get_all("x-b").collect::<Vec<_>>(),
        vec!["1", "2", "3"]
    );
    let src = r#"function onRequest(req) { req.headers["Bad Header"] = "x"; return req; }"#;
    assert!(run_on_request(src, r.clone(), Bytes::new()).is_err());
}

#[test]
fn syntax_checked_at_compile_time() {
    let e = RulesEngine::new(set(vec![
        rule(
            "broken",
            vec![],
            Action::Script {
                source: "function onRequest(req) {\n  return req.;\n}".into(),
            },
        ),
        rule(
            "no-hooks",
            vec![],
            Action::Script {
                source: "var x = 1;".into(),
            },
        ),
        rule(
            "throws",
            vec![],
            Action::Script {
                source: "throw new Error('top');\nfunction onRequest(r) {}".into(),
            },
        ),
        rule(
            "slow-top",
            vec![],
            Action::Script {
                source: "while (true) {}\nfunction onRequest(r) {}".into(),
            },
        ),
        rule(
            "ok",
            vec![],
            Action::Script {
                source: "function onRequest(r) { return r; }".into(),
            },
        ),
    ]));
    let errors = e.errors();
    let ids: Vec<&str> = errors.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids, vec!["broken", "no-hooks", "throws"]);
    assert!(errors[0].1.contains("line 2"), "{}", errors[0].1);
    assert!(errors[2].1.contains("top"), "{}", errors[2].1);
}

#[test]
fn errors_include_console_output() {
    let src =
        "function onRequest(req) { console.log('step 1'); console.warn('about to fail'); null.x; }";
    let err = run_on_request(src, req("GET", "http://x.test/"), Bytes::new())
        .unwrap_err()
        .to_string();
    assert!(err.contains("TypeError"), "{err}");
    assert!(err.contains("[log] step 1\n[warn] about to fail"), "{err}");
}

#[test]
fn scripts_without_the_hook_skip_the_runtime() {
    // Even a script that would loop forever is never started for a hook it doesn't define.
    let src = "while (true) {} function onResponse(q, r) { return r; }";
    let start = Instant::now();
    let original = req("GET", "http://x.test/");
    let (out, log) = run_on_request(src, original.clone(), Bytes::new()).unwrap();
    assert_eq!(cont(out).0, original);
    assert!(log.lines.is_empty());
    let src = "function onRequest(q) { return q; }";
    let (h, _, _) = run_on_response(src, &original, response(204), Bytes::new()).unwrap();
    assert_eq!(h.status, 204);
    assert!(start.elapsed().as_millis() < 500);
}

fn post() -> RequestHead {
    req("POST", "http://x.test/")
}

fn nested_arrays(depth: usize) -> String {
    format!("{}{}", "[".repeat(depth), "]".repeat(depth))
}

/// A body nested ~10k levels deep used to abort the whole process: `JSON.parse` accepts it, but
/// QuickJS's native `JSON.stringify` (also used for console output, hook results and thrown
/// values) recursed without any stack check.
#[test]
fn deeply_nested_values_never_overflow_the_native_stack() {
    // The usual body rewrite on a hostile body: a catchable RangeError at the user's line.
    let rewrite = "function onRequest(req) {\n  const d = JSON.parse(req.body);\n  d.debug = true;\n  req.body = JSON.stringify(d);\n  return req;\n}";
    for depth in [10_000, 15_000, 30_000] {
        let err = run_on_request(rewrite, post(), Bytes::from(nested_arrays(depth)))
            .unwrap_err()
            .to_string();
        assert!(err.contains("RangeError"), "depth {depth}: {err}");
        assert!(
            err.contains("line 2") || err.contains("line 4"),
            "depth {depth}: {err}"
        );
    }
    // Ordinary nesting keeps working.
    let echo =
        "function onRequest(req) { req.body = JSON.stringify(JSON.parse(req.body)); return req; }";
    let (out, _) = run_on_request(echo, post(), Bytes::from(nested_arrays(1000))).unwrap();
    assert_eq!(cont(out).1, nested_arrays(1000));
    // The error can be caught.
    let src = "function onRequest(req) { try { JSON.stringify(JSON.parse(req.body)); } catch (e) { req.headers['x-error'] = e.name; } return req; }";
    let (out, _) = run_on_request(src, post(), Bytes::from(nested_arrays(10_000))).unwrap();
    assert_eq!(cont(out).0.headers.get("x-error"), Some("RangeError"));
    // console.log of such a value prints something instead of crashing. The script builds the
    // value itself: how deep `JSON.parse` gets before its RangeError depends on the platform's
    // native frame sizes (on macOS arm64 it already throws at this depth).
    let src = "function onRequest(req) { let a = []; for (let i = 0; i < 10000; i++) a = [a]; console.log('body', a); }";
    let (_, log) = run_on_request(src, post(), Bytes::new()).unwrap();
    assert!(log.lines[0].starts_with("[log] body "), "{:?}", log.lines);
    // The same from onResponse (an upstream body).
    let src = "function onResponse(req, res) { res.body = JSON.stringify(JSON.parse(res.body)); }";
    let err = run_on_response(
        src,
        &post(),
        response(200),
        Bytes::from(nested_arrays(10_000)),
    )
    .unwrap_err();
    assert!(err.to_string().contains("RangeError"), "{err}");

    // Values the script builds itself (no JSON.parse depth check), in every place a value gets
    // serialized: JSON.stringify with each kind of replacer, toJSON, console output, hook
    // results and thrown values.
    // 100k levels: unchecked native recursion that deep would need more than the script
    // thread's whole stack.
    let arrays = "let a = []; for (let i = 0; i < 100000; i++) a = [a];";
    let objects = "let a = {}; for (let i = 0; i < 100000; i++) a = { a };";
    let cases = [
        ("stringify", "JSON.stringify(a);"),
        ("indent", "JSON.stringify(a, null, 2);"),
        ("function replacer", "JSON.stringify(a, (k, v) => v);"),
        ("array replacer", "JSON.stringify(a, ['a', '0']);"),
        ("toJSON", "JSON.stringify({ toJSON() { return a; } });"),
        ("console", "console.log(a);"),
        ("json result", "return { status: 200, json: a };"),
        ("body result", "req.body = a; return req;"),
        ("returned request", "req.extra = a; return req;"),
        ("thrown", "throw a;"),
    ];
    std::thread::scope(|scope| {
        for build in [arrays, objects] {
            for (case, body) in cases {
                scope.spawn(move || {
                    let src = format!("function onRequest(req) {{ {build} {body} }}");
                    match run_on_request(&src, post(), Bytes::new()) {
                        Ok(_) => {
                            assert_eq!(case, "console", "{build} / {case}: unexpected success")
                        }
                        Err(e) => {
                            let e = e.to_string();
                            assert!(
                                e.contains("RangeError")
                                    || e.contains("uncaught exception")
                                    || e.contains("memory limit"),
                                "{build} / {case}: {e}"
                            );
                        }
                    }
                });
            }
        }
    });
}

/// `JSON.stringify` is replaced by a stack-safe version: its output must match QuickJS's native
/// implementation exactly.
#[test]
fn stringify_replacement_matches_native_behaviour() {
    let cases = [
        r#"JSON.stringify({a: [1, {b: 2}], c: "x", d: undefined, e: function () {}, f: null, g: NaN, h: -0, i: 1e21, j: true, k: Infinity})"#,
        r#"JSON.stringify([undefined, function () {}, Symbol('s'), new Number(3), new String('s'), new Boolean(false), new Date(0)])"#,
        r#"JSON.stringify({a: 1, b: [1, 2], c: {}}, null, 2)"#,
        r#"JSON.stringify({a: 1, b: [1, [2]]}, null, '\t')"#,
        r#"JSON.stringify({a: 1}, null, 20)"#,
        r#"JSON.stringify({a: 1}, null, new Number(3))"#,
        r#"JSON.stringify({a: 1}, null, 'abcdefghijklmnop')"#,
        r#"JSON.stringify({a: 1, b: 2}, function (k, v) { return typeof v === 'number' ? v * 10 : v; })"#,
        r#"JSON.stringify({a: 1, b: {c: 2}}, function (k, v) { return k === 'c' ? undefined : v; }, 1)"#,
        r#"JSON.stringify({b: 1, a: 2, 1: 3, c: {a: 5, b: 6}}, ['b', 1, 'c', 'a'])"#,
        r#"JSON.stringify({b: 1, a: 2, c: 3}, ['a', 'a', new String('b'), new Number(1), {}, null, true])"#,
        r#"JSON.stringify(Object.create({inherited: 1}), ['inherited'])"#,
        r#"JSON.stringify([{a: 1, b: 2}, {a: 3}, [{b: 4}]], ['a'], 1)"#,
        r#"JSON.stringify({d: new Date(0), n: new Number(5), s: new String('x'), b: new Boolean(true), t: {toJSON: function (k) { return 'key:' + k; }}}, ['d', 'n', 's', 'b', 't'])"#,
        r#"JSON.stringify({a: undefined, b: function () {}, c: Symbol()}, ['a', 'b', 'c'])"#,
        r#"JSON.stringify({a: 1}, [])"#,
        r#"JSON.stringify('str', ['a'])"#,
        r#"JSON.stringify(undefined, ['a'])"#,
        r#"JSON.stringify({a: [1, {a: 2, b: 3}], b: 4}, ['a'], '--')"#,
        r#"JSON.stringify(new Proxy({a: 1, b: 2}, {}), ['b'])"#,
        r#"JSON.stringify({a: {b: {c: 1}}, x: [NaN, -0, 'q']}, ['a', 'b', 'c', 'x'], 2)"#,
        r#"JSON.stringify({a: 1n}, ['a'])"#,
        r#"JSON.stringify({a: Object(1n)}, ['a'])"#,
        r#"(function () { var o = {}; o.o = o; return JSON.stringify(o, ['o']); })()"#,
        r#"(function () { var o = []; o.push(o); return JSON.stringify(o, ['x']); })()"#,
        r#"(function () { var o = {}; o.o = o; return JSON.stringify(o); })()"#,
        r#"(function () { var shared = {v: 1}; return JSON.stringify({a: shared, b: [shared]}, ['a', 'b', 'v']); })()"#,
        r#"JSON.stringify(1n)"#,
        r#"JSON.stringify({toJSON: function () { return {x: 1}; }})"#,
        r#"JSON.stringify({toJSON: function () { return {x: 1, y: 2}; }}, ['y'])"#,
        r#"JSON.stringify(' \ud800"\\\n\u0001')"#,
        r#"JSON.stringify(Symbol('x'))"#,
        r#"JSON.stringify(function () {})"#,
        r#"JSON.stringify.length + ' ' + JSON.stringify.name"#,
    ];
    let wrap = |expr: &str| {
        format!(
            "(function () {{ try {{ return String({expr}); }} catch (e) {{ return 'throws ' + e.name + ': ' + e.message; }} }})()"
        )
    };
    // Native results, from a bare QuickJS runtime.
    let rt = rquickjs::Runtime::new().unwrap();
    let ctx = rquickjs::Context::full(&rt).unwrap();
    let native: Vec<String> = ctx.with(|ctx| {
        cases
            .iter()
            .map(|c| ctx.eval::<String, _>(wrap(c)).unwrap())
            .collect()
    });
    // The same expressions inside a script.
    let mut src = String::from("function onRequest(req) {\n");
    for c in &cases {
        src.push_str(&format!("  console.log({});\n", wrap(c)));
    }
    src.push_str("}\n");
    let (_, log) = run_on_request(&src, post(), Bytes::new()).unwrap();
    assert_eq!(log.lines.len(), cases.len());
    for ((case, native), ours) in cases.iter().zip(&native).zip(&log.lines) {
        assert_eq!(ours, &format!("[log] {native}"), "case {case}");
    }
}

/// QuickJS's interrupt handler only runs between bytecode instructions. A backtracking regular
/// expression or a long built-in loop never reaches it, and used to block the caller (a proxied
/// request, a rule save) for as long as the native call took.
#[test]
fn uninterruptible_work_cannot_hang_the_caller() {
    // Each of these runs for tens of seconds natively.
    let regex = "/(a*)*b/.test('a'.repeat(28));";
    let builtin = "Array(2 ** 30).indexOf(1);";
    let cases = [
        ("regex", "onRequest", regex),
        ("built-in loop", "onRequest", builtin),
        ("onResponse regex", "onResponse", regex),
    ];
    std::thread::scope(|scope| {
        for (what, hook, code) in cases {
            scope.spawn(move || {
                let start = Instant::now();
                let result = if hook == "onRequest" {
                    let src = format!("function onRequest(req) {{ {code} return req; }}");
                    run_on_request(&src, req("GET", "http://x.test/"), Bytes::new()).map(drop)
                } else {
                    let src = format!("function onResponse(req, res) {{ {code} }}");
                    run_on_response(
                        &src,
                        &req("GET", "http://x.test/"),
                        response(200),
                        Bytes::new(),
                    )
                    .map(drop)
                };
                let err = result.unwrap_err().to_string();
                let elapsed = start.elapsed();
                assert!(elapsed.as_secs_f64() < 3.5, "{what}: took {elapsed:?}");
                assert!(err.contains("CPU time limit"), "{what}: {err}");
                assert!(err.contains("can't be interrupted"), "{what}: {err}");
            });
        }
    });

    // Rule compilation runs top-level code: a stuck script is rejected within about a second
    // instead of holding the rules lock until the regex finishes.
    let start = Instant::now();
    let e = RulesEngine::new(set(vec![
        rule(
            "stuck",
            vec![],
            Action::Script {
                source: "/(a*)*b/.test('a'.repeat(28));\nfunction onRequest(r) {}".into(),
            },
        ),
        rule(
            "ok",
            vec![],
            Action::Script {
                source: "function onRequest(r) { return r; }".into(),
            },
        ),
    ]));
    let elapsed = start.elapsed();
    assert!(elapsed.as_secs_f64() < 2.0, "compile took {elapsed:?}");
    let errors = e.errors();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].0.as_str(), "stuck");
    assert!(errors[0].1.contains("can't be interrupted"), "{errors:?}");
}
