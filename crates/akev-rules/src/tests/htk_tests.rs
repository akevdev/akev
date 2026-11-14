//! HTTP Toolkit import tests, using exports shaped exactly like `rule-serialization.ts` output.

use super::helpers::*;
use crate::htk_import::{convert, looks_like_htk};
use crate::*;
use akev_core::rules::*;
use serde_json::{Value, json};

/// A realistic `.htkrules` export (HTTP Toolkit 1.2x UI, Mockttp 4).
fn sample() -> Value {
    json!({
        "id": "root",
        "title": "HTTP Toolkit Rules",
        "isRoot": true,
        "items": [
            {
                "id": "group-api",
                "title": "API mocks",
                "items": [
                    {
                        "id": "mock-login", "type": "http", "activated": true, "title": "Mock login",
                        "matchers": [
                            {"method": 1, "uiType": "POST", "type": "method"},
                            {"type": "simple-path", "path": "/login"},
                            {"type": "json-body-matching", "body": {"user": "admin"}}
                        ],
                        "steps": [{"type": "simple", "status": 200, "data": "{\"ok\":true}",
                                   "headers": {"content-type": "application/json", "set-cookie": ["a=1", "b=2"]}}],
                        "completionChecker": {"type": "always"}
                    },
                    {
                        "id": "slow-api", "type": "http", "activated": true,
                        "matchers": [
                            {"method": 0, "uiType": "GET", "type": "method"},
                            {"type": "host", "host": "api.example.com:8443"}
                        ],
                        "steps": [{"type": "delay", "delayMs": 500}, {"type": "passthrough"}],
                        "completionChecker": {"type": "always"}
                    },
                    {
                        "id": "fwd", "type": "http", "activated": true,
                        "matchers": [{"type": "wildcard"}, {"type": "host", "host": "localhost:3000"}],
                        "steps": [{"uiType": "forward-to-host", "type": "passthrough",
                                   "transformRequest": {"replaceHost": {"targetHost": "staging.example.com:8443", "updateHostHeader": false},
                                                        "setProtocol": "https"}}],
                        "completionChecker": {"type": "always"}
                    },
                    {
                        "id": "transformer", "type": "http", "activated": true, "title": "Rewrite API",
                        "matchers": [
                            {"method": 0, "uiType": "GET", "type": "method"},
                            {"type": "regex-path", "regexSource": "^\\/api\\/", "regexFlags": ""}
                        ],
                        "steps": [{
                            "uiType": "req-res-transformer", "type": "passthrough",
                            "transformRequest": {
                                "matchReplacePath": [[{"source": "^\\/api\\/v1\\/", "flags": "g"}, "/api/v2/"]],
                                "updateHeaders": "{\"x-debug\":\"1\",\"cookie\":\"__http_toolkit_undefined_placeholder__\"}",
                                "updateJsonBody": "{\"debug\":true,\"secret\":\"__http_toolkit_undefined_placeholder__\"}"
                            },
                            "transformResponse": {
                                "replaceStatus": 201,
                                "updateHeaders": "{\"x-powered-by\":\"__http_toolkit_undefined_placeholder__\"}",
                                "matchReplaceBody": [[{"source": "(\\w+)@example\\.com", "flags": "gi"}, "$1@test.dev"]]
                            }
                        }],
                        "completionChecker": {"type": "always"}
                    },
                    {
                        "id": "replace-body", "type": "http", "activated": false,
                        "matchers": [{"type": "wildcard"}, {"type": "header", "headers": {"x-test": "yes"}}],
                        "steps": [{"uiType": "req-res-transformer", "type": "passthrough",
                                   "transformRequest": {"replaceBody": "aGVsbG8="},
                                   "transformResponse": {"patchJsonBody": [{"op": "replace", "path": "/a", "value": 1}]}}],
                        "completionChecker": {"type": "always"}
                    }
                ]
            },
            {
                "id": "bp", "type": "http", "activated": true,
                "matchers": [{"method": 2, "uiType": "PUT", "type": "method"}, {"type": "query", "queryObject": {"debug": "1", "tag": ["a", "b"]}}],
                "steps": [{"uiType": "request-and-response-breakpoint", "type": "passthrough"}],
                "completionChecker": {"type": "always"}
            },
            {
                "id": "hook", "type": "http", "activated": true,
                "matchers": [{"type": "wildcard"}, {"type": "exact-query-string", "query": "?a=1"}],
                "steps": [{"type": "webhook", "url": "https://hooks.example.com", "events": ["request", "response"]}, {"type": "passthrough"}],
                "completionChecker": {"type": "always"}
            },
            {
                "id": "cb", "type": "http", "activated": true,
                "matchers": [{"type": "wildcard"}, {"type": "callback", "name": "isSpecial", "version": 1}],
                "steps": [{"type": "close-connection"}],
                "completionChecker": {"type": "always"}
            },
            {
                "id": "reset3", "type": "http", "activated": true,
                "matchers": [{"type": "simple-path", "path": "https://flaky.example.com/upload"}],
                "steps": [{"type": "reset-connection"}],
                "completionChecker": {"type": "times", "count": 3}
            },
            {
                "id": "binary", "type": "http", "activated": true,
                "matchers": [{"method": 0, "uiType": "GET", "type": "method"}, {"type": "simple-path", "path": "/bin"}],
                "steps": [{"type": "simple", "status": 200, "data": {"type": "Buffer", "data": [0, 159, 146, 150]}}],
                "completionChecker": {"type": "once"}
            },
            {
                "id": "ws-reject", "type": "websocket", "activated": true,
                "matchers": [{"uiType": "ws-wildcard", "type": "wildcard"}, {"type": "simple-path", "path": "/socket"}],
                "steps": [{"type": "ws-reject", "statusCode": 403, "statusMessage": "Forbidden", "headers": {}, "body": "nope"}],
                "completionChecker": {"type": "always"}
            },
            {
                "id": "ws-listen", "type": "websocket", "activated": true,
                "matchers": [{"uiType": "ws-wildcard", "type": "wildcard"}],
                "steps": [{"type": "ws-listen"}],
                "completionChecker": {"type": "always"}
            },
            {
                "id": "eth", "type": "ethereum", "activated": true,
                "matchers": [{"uiType": "eth-method", "type": "json-body-matching",
                              "body": {"jsonrpc": "2.0", "method": "eth_blockNumber"}, "methodName": "eth_blockNumber"}],
                "steps": [{"uiType": "eth-number-result", "type": "json-rpc-response", "result": {"result": "0x10"}, "value": 16}],
                "completionChecker": {"type": "always"}
            },
            {
                "id": "ipfs", "type": "ipfs", "activated": true,
                "matchers": [
                    {"uiType": "ipfs-interaction", "type": "simple-path", "path": "/api/v0/cat", "interactionName": "cat"},
                    {"uiType": "ipfs-arg", "type": "query", "queryObject": {"arg": "QmHash"}, "interactionName": "cat", "argValue": "QmHash"}
                ],
                "steps": [{"uiType": "ipfs-cat-text", "type": "simple", "status": 200, "data": "hello ipfs",
                           "headers": {"cache-control": "no-cache"}}],
                "completionChecker": {"type": "always"}
            },
            {
                "id": "rtc", "type": "webrtc", "activated": true,
                "matchers": [{"type": "rtc-wildcard"}],
                "steps": [{"type": "echo-rtc"}],
                "completionChecker": {"type": "always"}
            },
            {
                "id": "never", "type": "http", "activated": true,
                "matchers": [],
                "steps": [{"type": "passthrough"}],
                "completionChecker": {"type": "always"}
            },
            {
                "id": "default-group",
                "title": "Default rules",
                "items": [
                    {
                        "id": "default-amiusing", "type": "http", "activated": true, "priority": 2,
                        "matchers": [
                            {"method": 0, "uiType": "GET", "type": "method"},
                            {"uiType": "am-i-using", "type": "regex-path", "regexSource": "^https?:\\/\\/amiusing\\.httptoolkit\\.tech\\/?$", "regexFlags": ""}
                        ],
                        "steps": [{"type": "simple", "status": 200, "data": "<html>yes</html>",
                                   "headers": {"content-type": "text/html", "cache-control": "no-store", "httptoolkit-active": "true"}}],
                        "completionChecker": {"type": "always"}
                    },
                    {
                        "id": "default-certificate", "type": "http", "activated": true, "priority": 2,
                        "matchers": [
                            {"method": 0, "uiType": "GET", "type": "method"},
                            {"type": "simple-path", "path": "amiusing.httptoolkit.tech/certificate"}
                        ],
                        "steps": [{"type": "file", "status": 200, "filePath": "/home/u/.config/httptoolkit/ca.pem",
                                   "headers": {"content-type": "application/x-x509-ca-cert"}}],
                        "completionChecker": {"type": "always"}
                    },
                    {
                        "id": "default-wildcard", "type": "http", "activated": true,
                        "matchers": [{"uiType": "default-wildcard", "type": "wildcard"}],
                        "steps": [{"type": "passthrough"}],
                        "completionChecker": {"type": "always"}
                    },
                    {
                        "id": "default-ws-wildcard", "type": "websocket", "activated": true,
                        "matchers": [{"uiType": "default-ws-wildcard", "type": "wildcard"}],
                        "steps": [{"type": "ws-passthrough"}],
                        "completionChecker": {"type": "always"}
                    }
                ]
            }
        ]
    })
}

fn find<'a>(rs: &'a RuleSet, id: &str) -> &'a Rule {
    rs.find_rule(id).unwrap_or_else(|| {
        panic!(
            "rule {id} missing; have {:?}",
            rs.walk().iter().map(|(r, _)| &r.id).collect::<Vec<_>>()
        )
    })
}

fn http_proto() -> Matcher {
    Matcher::Protocol {
        protocols: vec!["http".into(), "https".into()],
    }
}

#[test]
fn converts_realistic_export() {
    let rs = convert(&sample()).unwrap();
    assert_eq!(rs.version, RULESET_VERSION);

    // Priority split: OVERRIDE rules first, the rest of the default group last.
    let top: Vec<String> = rs
        .items
        .iter()
        .map(|i| match i {
            RuleItem::Rule(r) => r.id.clone(),
            RuleItem::Group(g) => format!("group:{}", g.id),
        })
        .collect();
    assert_eq!(top.first().map(String::as_str), Some("group:default-group"));
    assert_eq!(
        top.last().map(String::as_str),
        Some("group:default-group~p1")
    );
    assert_eq!(top[1], "group:group-api");
    match &rs.items[0] {
        RuleItem::Group(g) => {
            assert!(g.collapsed && g.enabled);
            assert_eq!(g.title, "Default rules");
            assert_eq!(g.items.len(), 2);
        }
        RuleItem::Rule(_) => panic!("expected group"),
    }

    // Mock login.
    let login = find(&rs, "mock-login");
    assert!(login.enabled);
    assert_eq!(login.title.as_deref(), Some("Mock login"));
    assert_eq!(
        login.matchers,
        vec![
            http_proto(),
            Matcher::Method {
                methods: vec!["POST".into()]
            },
            Matcher::Path {
                pattern: cs(MatchOp::Equals, "/login")
            },
            Matcher::JsonBody {
                value: json!({"user": "admin"}),
                partial: true
            },
        ]
    );
    match &login.action {
        Action::Mock {
            status,
            headers,
            body,
            ..
        } => {
            assert_eq!(*status, 200);
            assert_eq!(
                headers.get_all("set-cookie").collect::<Vec<_>>(),
                vec!["a=1", "b=2"]
            );
            assert_eq!(
                body,
                &BodySource::Text {
                    text: "{\"ok\":true}".into()
                }
            );
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(login.max_matches, None);

    // Delay → modifier + terminal with identical matchers.
    let delay = find(&rs, "slow-api/delay");
    let slow = find(&rs, "slow-api");
    assert_eq!(
        delay.action,
        Action::Delay {
            ms: 500,
            jitter_ms: 0
        }
    );
    assert_eq!(slow.action, passthrough());
    assert_eq!(delay.matchers, slow.matchers);
    assert!(slow.matchers.contains(&Matcher::Host {
        pattern: cs(MatchOp::Equals, "api.example.com")
    }));
    assert!(slow.matchers.contains(&Matcher::Port { ports: vec![8443] }));

    // Forward to host.
    match &find(&rs, "fwd").action {
        Action::Forward {
            target,
            preserve_host,
            ..
        } => {
            assert_eq!(target, "https://staging.example.com:8443");
            assert!(*preserve_host);
        }
        other => panic!("{other:?}"),
    }

    // Transformer: URL rewrite script + declarative transforms.
    let script_rule = find(&rs, "transformer/script");
    assert_eq!(
        script_rule.title.as_deref(),
        Some("Rewrite API (transform script)")
    );
    let t = find(&rs, "transformer");
    assert!(t.enabled);
    match &t.action {
        Action::Passthrough {
            request: Some(req_t),
            response: Some(res_t),
        } => {
            assert_eq!(
                req_t.set_headers,
                Headers(vec![("x-debug".into(), "1".into())])
            );
            assert_eq!(req_t.remove_headers, vec!["cookie".to_string()]);
            assert_eq!(
                req_t.body,
                Some(BodyTransform::JsonMerge {
                    patch: json!({"debug": true, "secret": null})
                })
            );
            assert_eq!(res_t.status, Some(201));
            assert_eq!(res_t.remove_headers, vec!["x-powered-by".to_string()]);
            match &res_t.body {
                Some(BodyTransform::MatchReplace { replacements }) => {
                    assert_eq!(replacements[0].find, r"(\w+)@example\.com");
                    assert_eq!(replacements[0].replace, "${1}@test.dev");
                    assert!(!replacements[0].case_sensitive);
                }
                other => panic!("{other:?}"),
            }
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        t.matchers[2],
        Matcher::Any {
            matchers: vec![
                Matcher::Url {
                    pattern: cs(MatchOp::Regex, "^/api/")
                },
                Matcher::Path {
                    pattern: cs(MatchOp::Regex, "^/api/")
                },
            ]
        }
    );

    // Disabled in HTK → disabled here; base64 body + JSON patch.
    let rb = find(&rs, "replace-body");
    assert!(!rb.enabled);
    match &rb.action {
        Action::Passthrough {
            request: Some(r),
            response: Some(s),
        } => {
            assert_eq!(
                r.body,
                Some(BodyTransform::Replace {
                    body: BodySource::Base64 {
                        data: "aGVsbG8=".into()
                    }
                })
            );
            assert!(matches!(s.body, Some(BodyTransform::JsonPatch { .. })));
        }
        other => panic!("{other:?}"),
    }

    // Breakpoints.
    assert_eq!(
        find(&rs, "bp/breakpoint").action,
        Action::Breakpoint {
            request: true,
            response: true
        }
    );
    assert_eq!(find(&rs, "bp").action, passthrough());
    assert!(find(&rs, "bp").matchers.contains(&Matcher::Query {
        params: vec![
            kv("debug", Some(cs(MatchOp::Equals, "1"))),
            kv("tag", Some(cs(MatchOp::Equals, "a"))),
            kv("tag", Some(cs(MatchOp::Equals, "b"))),
        ]
    }));

    // Webhook: disabled placeholder, the rest stays active.
    let wh = find(&rs, "hook/webhook");
    assert!(!wh.enabled);
    assert!(
        wh.note
            .as_deref()
            .is_some_and(|n| n.contains("hooks.example.com"))
    );
    assert!(find(&rs, "hook").enabled);

    // Callback matcher → disabled with a note.
    let cb = find(&rs, "cb");
    assert!(!cb.enabled);
    assert!(cb.note.as_deref().is_some_and(|n| n.contains("callback")));

    // Absolute simple-path + times(3).
    let reset = find(&rs, "reset3");
    assert_eq!(reset.action, Action::Reset);
    assert_eq!(reset.max_matches, Some(3));
    assert!(reset.matchers.contains(&Matcher::Protocol {
        protocols: vec!["https".into()]
    }));
    assert!(reset.matchers.contains(&Matcher::Host {
        pattern: cs(MatchOp::Equals, "flaky.example.com")
    }));
    assert!(reset.matchers.contains(&Matcher::Port { ports: vec![443] }));

    // Buffer data → base64 mock body; once → maxMatches 1.
    let bin = find(&rs, "binary");
    assert_eq!(bin.max_matches, Some(1));
    match &bin.action {
        Action::Mock {
            body: BodySource::Base64 { data },
            ..
        } => assert_eq!(data, "AJ+Slg=="),
        other => panic!("{other:?}"),
    }

    // WebSockets.
    let wsr = find(&rs, "ws-reject");
    assert_eq!(
        wsr.action,
        Action::WsReject {
            status: 403,
            body: BodySource::Text {
                text: "nope".into()
            }
        }
    );
    assert_eq!(
        wsr.matchers[0],
        Matcher::Protocol {
            protocols: vec!["ws".into(), "wss".into()]
        }
    );
    assert!(!find(&rs, "ws-listen").enabled);

    // Ethereum JSON-RPC → script + passthrough.
    assert!(matches!(
        find(&rs, "eth/script").action,
        Action::Script { .. }
    ));
    assert_eq!(find(&rs, "eth").action, passthrough());

    // IPFS (simple-path + query + simple).
    let ipfs = find(&rs, "ipfs");
    assert!(ipfs.matchers.contains(&Matcher::Path {
        pattern: cs(MatchOp::Equals, "/api/v0/cat")
    }));
    assert!(ipfs.enabled);

    // Unsupported rule types / never-matching rules → disabled.
    assert!(!find(&rs, "rtc").enabled);
    assert!(
        find(&rs, "rtc")
            .note
            .as_deref()
            .is_some_and(|n| n.contains("WebRTC"))
    );
    assert!(!find(&rs, "never").enabled);

    // Built-in defaults are imported disabled.
    for id in [
        "default-amiusing",
        "default-certificate",
        "default-wildcard",
        "default-ws-wildcard",
    ] {
        let r = find(&rs, id);
        assert!(!r.enabled, "{id}");
        assert!(r.note.is_some(), "{id}");
    }
    let cert = find(&rs, "default-certificate");
    assert!(cert.matchers.contains(&Matcher::Host {
        pattern: cs(MatchOp::Equals, "amiusing.httptoolkit.tech")
    }));
    assert!(cert.matchers.contains(&Matcher::Path {
        pattern: cs(MatchOp::Equals, "/certificate")
    }));

    // Everything converted compiles cleanly.
    let engine = RulesEngine::new(rs.clone());
    assert_valid(&engine);
}

#[test]
fn converted_rules_behave_like_http_toolkit() {
    let rs = convert(&sample()).unwrap();
    let e = RulesEngine::new(rs.clone());

    // POST /login with a matching JSON body → mock.
    let login = with_headers(
        req("POST", "https://shop.test/login"),
        &[("Content-Type", "application/json")],
    );
    let d = e.evaluate(&MatchContext {
        head: &login,
        body: Some(br#"{"user":"admin","pw":"x"}"#),
        source: None,
    });
    assert_eq!(d.terminal_rule.as_deref(), Some("mock-login"));
    let d = e.evaluate(&MatchContext {
        head: &login,
        body: Some(br#"{"user":"bob"}"#),
        source: None,
    });
    assert_eq!(
        d.terminal_rule, None,
        "default wildcard is disabled → implicit passthrough"
    );

    // WebSocket upgrades don't hit HTTP rules.
    let ws = with_headers(
        req("GET", "http://shop.test/socket"),
        &[("Upgrade", "websocket")],
    );
    let d = e.evaluate(&MatchContext {
        head: &ws,
        body: None,
        source: None,
    });
    assert_eq!(d.terminal_rule.as_deref(), Some("ws-reject"));

    // Delay stacks with its own passthrough only.
    let slow = req("GET", "https://api.example.com:8443/x");
    let d = e.evaluate(&MatchContext {
        head: &slow,
        body: None,
        source: None,
    });
    assert_eq!(d.matched, vec!["slow-api/delay", "slow-api"]);
    assert_eq!(d.delay_ms, 500);

    // exact-query-string.
    let exact = req("GET", "http://other.test/p?a=1");
    assert_eq!(
        e.dry_run(&MatchContext {
            head: &exact,
            body: None,
            source: None
        })
        .terminal_rule
        .as_deref(),
        Some("hook")
    );
    let not_exact = req("GET", "http://other.test/p?a=1&b=2");
    assert_eq!(
        e.dry_run(&MatchContext {
            head: &not_exact,
            body: None,
            source: None
        })
        .terminal_rule,
        None
    );

    // Generated scripts run with Mockttp's semantics.
    #[cfg(feature = "scripting")]
    {
        use crate::script::{ScriptRequestOutcome, run_on_request};
        let Action::Script { source } = &find(&rs, "transformer/script").action else {
            panic!()
        };
        let (out, _) = run_on_request(
            source,
            req("GET", "https://example.com/api/v1/users?id=1"),
            Bytes::new(),
        )
        .unwrap();
        match out {
            ScriptRequestOutcome::Continue { head, .. } => {
                assert_eq!(head.url, "https://example.com/api/v2/users?id=1");
                assert_eq!(head.headers.get("host"), Some("example.com"));
            }
            ScriptRequestOutcome::Respond { .. } => panic!("unexpected response"),
        }

        let Action::Script { source } = &find(&rs, "eth/script").action else {
            panic!()
        };
        let rpc = with_headers(
            req("POST", "http://node.test/"),
            &[("Content-Type", "application/json")],
        );
        let (out, _) = run_on_request(
            source,
            rpc.clone(),
            Bytes::from_static(br#"{"jsonrpc":"2.0","id":7,"method":"eth_blockNumber"}"#),
        )
        .unwrap();
        match out {
            ScriptRequestOutcome::Respond { head, body } => {
                assert_eq!(head.status, 200);
                let v: Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(v, json!({"jsonrpc": "2.0", "id": 7, "result": "0x10"}));
            }
            ScriptRequestOutcome::Continue { .. } => panic!("expected a response"),
        }
        let (out, _) = run_on_request(source, rpc, Bytes::from_static(b"{}")).unwrap();
        assert!(matches!(out, ScriptRequestOutcome::Respond { head, .. } if head.status == 500));
    }
}

#[test]
fn url_transform_scripts_cover_host_and_query() {
    let export = json!({"id": "root", "title": "x", "items": [{
        "id": "t", "type": "http", "activated": true,
        "matchers": [{"type": "wildcard"}],
        "steps": [{"uiType": "req-res-transformer", "type": "passthrough", "transformRequest": {
            "setProtocol": "http",
            "matchReplaceHost": {"replacements": [[{"source": "prod", "flags": "g"}, "dev"], ["example.com", "example.test"]], "updateHostHeader": "custom.host"},
            "matchReplaceQuery": [[{"source": "(?<key>token)=[^&]*", "flags": "g"}, "$<key>=REDACTED"]],
            "replaceHeaders": "{\"accept\":\"*/*\"}"
        }}],
        "completionChecker": {"type": "always"}
    }]});
    let rs = convert(&export).unwrap();
    assert_valid(&RulesEngine::new(rs.clone()));
    #[cfg(feature = "scripting")]
    {
        use crate::script::{ScriptRequestOutcome, run_on_request};
        let Action::Script { source } = &find(&rs, "t/script").action else {
            panic!()
        };
        let head = with_headers(
            req("GET", "https://api.prod.example.com/x?token=abc&y=1"),
            &[("Accept", "text/html"), ("X-Drop", "1")],
        );
        let (out, _) = run_on_request(source, head, Bytes::new()).unwrap();
        let ScriptRequestOutcome::Continue { head, .. } = out else {
            panic!()
        };
        assert_eq!(head.url, "http://api.dev.example.test/x?token=REDACTED&y=1");
        assert_eq!(head.port, 80, "default port follows the new protocol");
        assert_eq!(head.headers.get("host"), Some("custom.host"));
        assert_eq!(head.headers.get("accept"), Some("*/*"));
        assert!(
            !head.headers.contains("x-drop"),
            "replaceHeaders replaces everything"
        );
    }
}

#[test]
fn legacy_formats_and_migrations() {
    let legacy = json!({"rules": [
        {"id": "r1", "matchers": [{"type": "simple-path", "path": "/x"}],
         "handler": {"type": "passthrough", "forwardToLocation": "https://other.host"}},
        {"id": "r2", "matchers": [{"type": "wildcard"}],
         "handler": {"type": "passthrough", "forwarding": {"targetHost": "plain.host:8080", "updateHostHeader": false}}},
        {"id": "default-wildcard", "matchers": [{"type": "wildcard"}], "handler": {"type": "passthrough"}}
    ]});
    assert!(looks_like_htk(&legacy));
    let rs = convert(&legacy).unwrap();
    assert_eq!(rs.items.len(), 3);
    match &find(&rs, "r1").action {
        Action::Forward {
            target,
            preserve_host,
            ..
        } => {
            assert_eq!(target, "https://other.host");
            assert!(!preserve_host);
        }
        other => panic!("{other:?}"),
    }
    match &find(&rs, "r2").action {
        Action::Forward {
            target,
            preserve_host,
            ..
        } => assert_eq!((target.as_str(), *preserve_host), ("plain.host:8080", true)),
        other => panic!("{other:?}"),
    }
    match &rs.items[2] {
        RuleItem::Group(g) => {
            assert_eq!(
                (g.id.as_str(), g.title.as_str()),
                ("default-group", "Default rules")
            );
            assert!(
                matches!(&g.items[0], RuleItem::Rule(r) if r.id == "default-wildcard" && !r.enabled)
            );
        }
        RuleItem::Rule(_) => panic!("expected the default group last"),
    }
    // Missing `type` defaults to http.
    assert_eq!(find(&rs, "r1").matchers[0], http_proto());

    // Bare arrays and single rules are accepted too.
    let arr = json!([{"id": "a", "type": "http", "activated": true, "matchers": [{"type": "wildcard"}], "steps": [{"type": "timeout"}]}]);
    assert_eq!(find(&convert(&arr).unwrap(), "a").action, Action::Timeout);
}

#[test]
fn rejects_unknown_formats() {
    assert!(matches!(
        convert(&json!({"version": 2, "items": []})),
        Err(RulesError::InvalidRule { .. })
    ));
    assert!(convert(&json!("nope")).is_err());
    assert!(convert(&json!({"foo": 1})).is_err());
    // A broken rule never fails the whole import.
    let rs = convert(&json!({"items": [42, {"id": "x", "type": "http", "matchers": [{"type": "wildcard"}], "steps": [{"type": "mystery"}]}]})).unwrap();
    let rules: Vec<&Rule> = rs.walk().into_iter().map(|(r, _)| r).collect();
    assert_eq!(rules.len(), 2);
    assert!(rules.iter().all(|r| !r.enabled && r.note.is_some()));
}

#[test]
fn matcher_and_step_edge_cases() {
    let rule_with = |matchers: Value, steps: Value| json!({"items": [{"id": "r", "type": "http", "activated": true, "matchers": matchers, "steps": steps}]});
    let pass = json!([{"type": "passthrough"}]);
    let convert_one = |v: Value| convert(&v).unwrap().find_rule("r").cloned().unwrap();

    // Lookaround regex → disabled.
    let r = convert_one(rule_with(
        json!([{"type": "regex-url", "regexSource": "(?<=a)b", "regexFlags": "i"}]),
        pass.clone(),
    ));
    assert!(!r.enabled && r.note.unwrap().contains("regular expression"));
    // Case-insensitive body regex.
    let r = convert_one(rule_with(
        json!([{"type": "raw-body-regexp", "regexString": "^hello", "regexFlags": "i"}]),
        pass.clone(),
    ));
    assert_eq!(
        r.matchers[1],
        Matcher::Body {
            pattern: sm(MatchOp::Regex, "^hello")
        }
    );
    // Multipart: name/content supported, filename not.
    let r = convert_one(rule_with(
        json!([{"type": "multipart-form-data", "matchConditions": [{"name": "a", "content": "b"}, {"name": "file"}]}]),
        pass.clone(),
    ));
    assert_eq!(
        r.matchers[1],
        Matcher::Form {
            fields: vec![kv("a", Some(cs(MatchOp::Equals, "b"))), kv("file", None)]
        }
    );
    let r = convert_one(rule_with(
        json!([{"type": "multipart-form-data", "matchConditions": [{"filename": "x.png"}]}]),
        pass.clone(),
    ));
    assert!(!r.enabled);
    // Cookie with several keys → enabled with a note.
    let r = convert_one(rule_with(
        json!([{"type": "cookie", "cookie": {"a": "1", "b": "2"}}]),
        pass.clone(),
    ));
    assert!(r.enabled && r.note.unwrap().contains("cookie"));
    // Port, protocol, hostname, form-data, raw-body, raw-body-includes, json-body.
    let r = convert_one(rule_with(
        json!([
            {"type": "port", "port": "8080"}, {"type": "protocol", "protocol": "https"}, {"type": "hostname", "hostname": "h.test"},
            {"type": "form-data", "formData": {"k": "v"}}, {"type": "raw-body", "content": "exact"},
            {"type": "raw-body-includes", "content": "part"}, {"type": "json-body", "body": {"a": 1}}
        ]),
        pass.clone(),
    ));
    assert!(r.enabled);
    assert_eq!(
        r.matchers[0],
        Matcher::Port { ports: vec![8080] },
        "explicit protocol matcher suppresses the implicit one"
    );
    assert!(r.matchers.contains(&Matcher::JsonBody {
        value: json!({"a": 1}),
        partial: false
    }));
    assert!(r.matchers.contains(&Matcher::Body {
        pattern: StringMatch {
            op: MatchOp::Contains,
            value: "part".into(),
            case_sensitive: true,
            negate: false
        }
    }));
    // Unknown method index.
    assert!(
        !convert_one(rule_with(
            json!([{"type": "method", "method": 42}]),
            pass.clone()
        ))
        .enabled
    );
    // File response, trailers note, ws-forward, timeout, close.
    let r = convert_one(rule_with(
        json!([{"type": "wildcard"}]),
        json!([{"type": "file", "status": 201, "filePath": "/tmp/x.json", "headers": {}}]),
    ));
    assert!(matches!(
        r.action,
        Action::Mock {
            status: 201,
            body: BodySource::File { .. },
            ..
        }
    ));
    let r = convert_one(rule_with(
        json!([{"type": "wildcard"}]),
        json!([{"type": "simple", "status": 200, "trailers": {"x": "y"}, "headers": {"transfer-encoding": "chunked"}}]),
    ));
    assert!(r.enabled && r.note.unwrap().contains("trailers"));
    let r = convert_one(
        json!({"items": [{"id": "r", "type": "websocket", "activated": true, "matchers": [{"type": "wildcard"}],
        "steps": [{"uiType": "ws-forward-to-host", "type": "ws-passthrough", "transformRequest": {"replaceHost": {"targetHost": "ws.test", "updateHostHeader": true}, "setProtocol": "wss"}}]}]}),
    );
    assert!(
        matches!(&r.action, Action::Forward { target, preserve_host: false, .. } if target == "wss://ws.test")
    );
    // Non-final only → passthrough appended with a note; steps after the final one ignored.
    let r = convert_one(rule_with(
        json!([{"type": "wildcard"}]),
        json!([{"type": "delay", "delayMs": 5}]),
    ));
    assert_eq!(r.action, passthrough());
    let r = convert_one(rule_with(
        json!([{"type": "wildcard"}]),
        json!([{"type": "close-connection"}, {"type": "delay", "delayMs": 5}]),
    ));
    assert_eq!(r.action, Action::Close);
    assert!(r.note.unwrap().contains("ignored"));
    // setProtocol alone → URL rewrite transform; non-global regex note; literal pattern note.
    let r = convert_one(rule_with(
        json!([{"type": "wildcard"}]),
        json!([{"uiType": "req-res-transformer", "type": "passthrough",
        "transformRequest": {"setProtocol": "https", "matchReplaceBody": [[{"source": "a", "flags": ""}, "b"], ["lit", "$&!"]]}}]),
    ));
    let Action::Passthrough {
        request: Some(t), ..
    } = &r.action
    else {
        panic!("{:?}", r.action)
    };
    assert_eq!(t.url_replace[0].replace, "https:");
    let Some(BodyTransform::MatchReplace { replacements }) = &t.body else {
        panic!()
    };
    assert_eq!(replacements[1].find, "lit");
    assert_eq!(replacements[1].replace, "${0}!");
    let note = r.note.unwrap();
    assert!(
        note.contains("first match") && note.contains("first occurrence"),
        "{note}"
    );
    // A legacy mockttp-wire passthrough with breakpoint callbacks.
    let r = convert_one(rule_with(
        json!([{"type": "wildcard"}]),
        json!([{"type": "passthrough", "hasBeforeRequestCallback": true}]),
    ));
    assert_eq!(r.action, passthrough());
    let rs = convert(&rule_with(
        json!([{"type": "wildcard"}]),
        json!([{"type": "passthrough", "hasBeforeRequestCallback": true}]),
    ))
    .unwrap();
    assert_eq!(
        find(&rs, "r/breakpoint").action,
        Action::Breakpoint {
            request: true,
            response: false
        }
    );
}

#[test]
fn json_merge_semantic_notes() {
    let export = json!({"items": [{"id": "r", "type": "http", "matchers": [{"type": "wildcard"}],
        "steps": [{"uiType": "req-res-transformer", "type": "passthrough",
                   "transformResponse": {"updateJsonBody": "{\"a\":null,\"list\":[1]}"}}]}]});
    let rs = convert(&export).unwrap();
    let r = find(&rs, "r");
    let note = r.note.clone().unwrap();
    assert!(
        note.contains("RFC 7386") && note.contains("arrays"),
        "{note}"
    );
    assert!(r.enabled);
}

#[test]
fn detection() {
    assert!(looks_like_htk(&sample()));
    assert!(!looks_like_htk(
        &serde_json::to_value(set(vec![rule("r", vec![], mock(200))])).unwrap()
    ));
    assert!(!looks_like_htk(&json!({"version": 1, "items": []})));
    assert!(looks_like_htk(
        &json!([{"id": "a", "activated": true, "matchers": [], "steps": []}])
    ));
    assert!(!looks_like_htk(&json!(5)));
}

#[test]
fn invalid_headers_are_dropped_not_fatal() {
    let export = json!({"items": [{"id": "r", "type": "http", "matchers": [{"type": "wildcard"}],
        "steps": [{"type": "simple", "status": 200, "statusMessage": "OK\r\nX: y",
                   "headers": {":status": "200", "bad header": "x", "x-ok": "1", "x-crlf": "a\r\nb"}}]}]});
    let rs = convert(&export).unwrap();
    let r = find(&rs, "r");
    assert!(r.enabled);
    match &r.action {
        Action::Mock {
            headers,
            status_message,
            ..
        } => {
            assert_eq!(headers.0, vec![("x-ok".to_string(), "1".to_string())]);
            assert_eq!(status_message, &None);
        }
        other => panic!("{other:?}"),
    }
    assert!(r.note.as_deref().is_some_and(|n| n.contains(":status")));
    assert_valid(&RulesEngine::new(rs));
}
