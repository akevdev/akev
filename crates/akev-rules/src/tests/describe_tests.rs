//! Rule summary tests.

use super::helpers::*;
use crate::describe::{action, matcher, summarize};
use akev_core::Headers;
use akev_core::rules::*;
use serde_json::json;

#[test]
fn summaries() {
    let r = rule(
        "r",
        vec![
            Matcher::Method {
                methods: vec!["GET".into()],
            },
            Matcher::Host {
                pattern: sm(MatchOp::Glob, "*.example.com"),
            },
            Matcher::Path {
                pattern: sm(MatchOp::Glob, "/api/*"),
            },
        ],
        mock(200),
    );
    assert_eq!(
        summarize(&r),
        "GET requests to *.example.com/api/* → mock 200"
    );

    let mut r = rule(
        "r",
        vec![
            Matcher::Method {
                methods: vec!["POST".into(), "PUT".into(), "PATCH".into()],
            },
            Matcher::Path {
                pattern: sm(MatchOp::Equals, "/login"),
            },
            Matcher::JsonBody {
                value: json!({"user": "admin"}),
                partial: true,
            },
        ],
        Action::Delay {
            ms: 500,
            jitter_ms: 0,
        },
    );
    r.max_matches = Some(1);
    assert_eq!(
        summarize(&r),
        r#"POST, PUT or PATCH requests to /login with JSON body including {"user":"admin"} → delay 500 ms (once)"#
    );
    r.max_matches = Some(3);
    assert!(summarize(&r).ends_with("(3 times)"));

    assert_eq!(
        summarize(&rule("r", vec![], passthrough())),
        "All requests → pass through"
    );
    assert_eq!(
        summarize(&rule(
            "r",
            vec![Matcher::Protocol {
                protocols: vec!["ws".into(), "wss".into()]
            }],
            Action::WsEcho
        )),
        "WebSockets → echo WebSocket messages"
    );
    assert_eq!(
        summarize(&rule(
            "r",
            vec![
                Matcher::Header {
                    headers: vec![kv("authorization", Some(neg(sm(MatchOp::Exists, ""))))]
                },
                Matcher::Port {
                    ports: vec![8080, 8443]
                },
            ],
            Action::Breakpoint {
                request: true,
                response: false
            },
        )),
        "Requests with header no authorization on ports 8080 or 8443 → pause request"
    );
}

#[test]
fn matcher_phrases() {
    assert_eq!(
        matcher(&Matcher::Query {
            params: vec![kv("q", Some(sm(MatchOp::Equals, "1"))), kv("flag", None)]
        }),
        "with query q=1 and flag"
    );
    assert_eq!(
        matcher(&Matcher::Cookie {
            cookies: vec![kv("session", Some(sm(MatchOp::Regex, "^a")))]
        }),
        "with cookie session matching /^a/"
    );
    assert_eq!(
        matcher(&Matcher::Body {
            pattern: sm(MatchOp::Contains, "x")
        }),
        r#"with body containing "x""#
    );
    assert_eq!(
        matcher(&Matcher::GraphqlOperation {
            pattern: sm(MatchOp::Equals, "GetUser")
        }),
        "for GraphQL operation GetUser"
    );
    assert_eq!(
        matcher(&Matcher::Source {
            pattern: sm(MatchOp::Equals, "chrome")
        }),
        "from chrome"
    );
    assert_eq!(
        matcher(&Matcher::HttpVersion {
            versions: vec!["HTTP/2".into()]
        }),
        "over HTTP/2"
    );
    assert_eq!(
        matcher(&Matcher::Not {
            matcher: Box::new(Matcher::Path {
                pattern: sm(MatchOp::StartsWith, "/admin")
            })
        }),
        "not (with path starting with /admin)"
    );
    assert_eq!(
        matcher(&Matcher::Any {
            matchers: vec![
                Matcher::Url {
                    pattern: sm(MatchOp::Contains, "a")
                },
                Matcher::Host {
                    pattern: neg(sm(MatchOp::Equals, "b"))
                }
            ]
        }),
        r#"(with URL containing "a" or with host not = b)"#
    );
}

#[test]
fn action_phrases() {
    assert_eq!(
        action(&Action::Forward {
            target: "http://localhost:3000".into(),
            strip_prefix: Some("/api".into()),
            preserve_host: true,
            request: None,
            response: None
        }),
        "forward to http://localhost:3000 (stripping /api) keeping the Host header"
    );
    assert_eq!(
        action(&Action::Transform {
            request: Some(RequestTransform {
                method: Some("POST".into()),
                set_headers: Headers(vec![("X-A".into(), "1".into()), ("x-a".into(), "2".into())]),
                body: Some(BodyTransform::JsonPatch {
                    operations: vec![json!({"op": "remove", "path": "/a"})]
                }),
                ..Default::default()
            }),
            response: Some(ResponseTransform {
                status: Some(404),
                ..Default::default()
            }),
        }),
        "transform (request: method → POST, set header X-A, patch JSON body (1 op); response: status → 404)"
    );
    assert_eq!(
        action(&Action::Throttle {
            download_bps: 125_000,
            upload_bps: 32_000,
            latency_ms: 100
        }),
        "throttle to 1.0 Mbit/s down, 256 kbit/s up, +100 ms latency"
    );
    assert_eq!(
        action(&Action::Delay {
            ms: 10,
            jitter_ms: 5
        }),
        "delay 10 ms (+0–5 ms)"
    );
    assert_eq!(
        action(&Action::Highlight {
            color: "red".into(),
            note: Some("look".into())
        }),
        "highlight red (look)"
    );
    assert_eq!(
        action(&Action::WsReject {
            status: 403,
            body: BodySource::Empty
        }),
        "reject the WebSocket (403)"
    );
    assert_eq!(
        action(&Action::Script {
            source: String::new()
        }),
        "run script"
    );
    assert_eq!(
        action(&Action::MapLocal {
            path: "/srv".into(),
            strip_prefix: None,
            index: None,
            fallback_to_upstream: false,
            headers: Headers::new()
        }),
        "serve local files from /srv"
    );
    for a in [Action::Close, Action::Reset, Action::Timeout] {
        assert!(!action(&a).is_empty());
    }
}

#[test]
fn imported_protocol_matchers_are_implicit() {
    let r = rule(
        "r",
        vec![
            Matcher::Protocol {
                protocols: vec!["http".into(), "https".into()],
            },
            Matcher::Method {
                methods: vec!["POST".into()],
            },
            Matcher::Path {
                pattern: sm(MatchOp::Equals, "/login"),
            },
        ],
        mock(200),
    );
    assert_eq!(summarize(&r), "POST requests to /login → mock 200");
    let r = rule(
        "r",
        vec![Matcher::Protocol {
            protocols: vec!["https".into()],
        }],
        mock(200),
    );
    assert_eq!(summarize(&r), "Requests over https → mock 200");
}
