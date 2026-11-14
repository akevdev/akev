//! Engine-level tests: matchers, evaluation semantics, transforms and local responses.

use crate::*;
use akev_core::http::HttpVersion;
use akev_core::rules::*;
use serde_json::json;

mod helpers {
    use super::*;

    pub fn req(method: &str, url: &str) -> RequestHead {
        let p = crate::urlutil::split_url(url).expect("test URL");
        let port = p.effective_port().expect("port");
        let mut head = RequestHead {
            method: method.into(),
            url: url.into(),
            scheme: p.scheme.clone(),
            host: p.host.clone(),
            port,
            path: p.path.clone(),
            http_version: HttpVersion::Http11,
            headers: Headers::new(),
        };
        let authority = head.authority();
        head.headers.append("Host", authority);
        head
    }

    pub fn with_headers(mut head: RequestHead, headers: &[(&str, &str)]) -> RequestHead {
        for (k, v) in headers {
            head.headers.append(*k, *v);
        }
        head
    }

    pub fn rule(id: &str, matchers: Vec<Matcher>, action: Action) -> Rule {
        Rule {
            id: id.into(),
            title: None,
            enabled: true,
            matchers,
            action,
            max_matches: None,
            note: None,
        }
    }

    pub fn set(rules: Vec<Rule>) -> RuleSet {
        RuleSet {
            version: RULESET_VERSION,
            items: rules.into_iter().map(RuleItem::Rule).collect(),
        }
    }

    pub fn mock(status: u16) -> Action {
        Action::Mock {
            status,
            status_message: None,
            headers: Headers::new(),
            body: BodySource::Empty,
        }
    }

    pub fn passthrough() -> Action {
        Action::Passthrough {
            request: None,
            response: None,
        }
    }

    pub fn sm(op: MatchOp, value: &str) -> StringMatch {
        StringMatch {
            op,
            value: value.into(),
            case_sensitive: false,
            negate: false,
        }
    }

    pub fn cs(op: MatchOp, value: &str) -> StringMatch {
        StringMatch {
            op,
            value: value.into(),
            case_sensitive: true,
            negate: false,
        }
    }

    pub fn neg(mut s: StringMatch) -> StringMatch {
        s.negate = true;
        s
    }

    pub fn kv(key: &str, value: Option<StringMatch>) -> KeyValueMatch {
        KeyValueMatch {
            key: key.into(),
            value,
        }
    }

    pub fn engine(rules: Vec<Rule>) -> RulesEngine {
        let e = RulesEngine::new(set(rules));
        assert_valid(&e);
        e
    }

    /// No compile errors — except, in builds without the `scripting` feature, the expected
    /// "scripting not compiled in" errors of script rules.
    pub fn assert_valid(e: &RulesEngine) {
        let errors: Vec<_> = e
            .errors()
            .into_iter()
            .filter(|(_, msg)| cfg!(feature = "scripting") || !msg.contains("scripting support"))
            .collect();
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");
    }

    /// Does a single matcher match this request (dry run)?
    pub fn m(matcher: Matcher, head: &RequestHead, body: Option<&[u8]>) -> bool {
        m_src(matcher, head, body, None)
    }

    pub fn m_src(
        matcher: Matcher,
        head: &RequestHead,
        body: Option<&[u8]>,
        source: Option<&str>,
    ) -> bool {
        let e = engine(vec![rule("r", vec![matcher], mock(200))]);
        e.dry_run(&MatchContext { head, body, source })
            .terminal_rule
            .is_some()
    }

    pub fn decide(e: &RulesEngine, head: &RequestHead, body: Option<&[u8]>) -> Decision {
        e.evaluate(&MatchContext {
            head,
            body,
            source: None,
        })
    }

    pub fn set_header_transform(name: &str, value: &str) -> RequestTransform {
        RequestTransform {
            set_headers: Headers(vec![(name.into(), value.into())]),
            ..Default::default()
        }
    }
}

use helpers::*;

// ==========================================================================================
// Matchers
// ==========================================================================================

mod matcher_tests {
    use super::*;

    #[test]
    fn method() {
        let get = req("GET", "http://example.com/");
        assert!(m(
            Matcher::Method {
                methods: vec!["GET".into()]
            },
            &get,
            None
        ));
        assert!(m(
            Matcher::Method {
                methods: vec!["post".into(), "get".into()]
            },
            &get,
            None
        ));
        assert!(!m(
            Matcher::Method {
                methods: vec!["POST".into()]
            },
            &get,
            None
        ));
        assert!(
            !m(Matcher::Method { methods: vec![] }, &get, None),
            "empty list matches nothing"
        );
        let custom = req("PROPFIND", "http://example.com/");
        assert!(m(
            Matcher::Method {
                methods: vec!["PROPFIND".into()]
            },
            &custom,
            None
        ));
    }

    #[test]
    fn host() {
        let h = req("GET", "https://api.Example.com/x");
        let host = |s: StringMatch| Matcher::Host { pattern: s };
        assert!(m(host(sm(MatchOp::Equals, "api.example.com")), &h, None));
        // Hosts are case-insensitive even when the pattern asks for case sensitivity (HTK bug #5).
        assert!(m(host(cs(MatchOp::Equals, "API.EXAMPLE.COM")), &h, None));
        assert!(m(host(sm(MatchOp::Glob, "*.example.com")), &h, None));
        assert!(
            m(host(sm(MatchOp::Glob, "*.com")), &h, None),
            "* crosses dots in hosts"
        );
        assert!(!m(host(sm(MatchOp::Glob, "*.example.org")), &h, None));
        assert!(!m(
            host(sm(MatchOp::Glob, "*.example.com")),
            &req("GET", "https://example.com/"),
            None
        ));
        assert!(m(host(sm(MatchOp::EndsWith, "example.com")), &h, None));
        assert!(m(host(sm(MatchOp::Regex, r"^api\.")), &h, None));
        assert!(m(host(neg(sm(MatchOp::Equals, "other.com"))), &h, None));
        assert!(!m(
            host(neg(sm(MatchOp::Equals, "api.example.com"))),
            &h,
            None
        ));
        // Lenient host:port.
        assert!(m(
            host(sm(MatchOp::Equals, "api.example.com:443")),
            &h,
            None
        ));
        assert!(!m(
            host(sm(MatchOp::Equals, "api.example.com:8443")),
            &h,
            None
        ));
        // Trailing dot (FQDN) on either side.
        let mut fqdn = req("GET", "http://example.com/");
        fqdn.host = "example.com.".into();
        assert!(m(host(sm(MatchOp::Equals, "example.com")), &fqdn, None));
        assert!(m(
            host(sm(MatchOp::Equals, "example.com.")),
            &req("GET", "http://example.com/"),
            None
        ));
    }

    #[test]
    fn host_ipv6() {
        let h = req("GET", "http://[::1]:8080/x");
        assert_eq!(h.host, "::1");
        let host = |s: StringMatch| Matcher::Host { pattern: s };
        assert!(m(host(sm(MatchOp::Equals, "::1")), &h, None));
        assert!(m(host(sm(MatchOp::Equals, "[::1]")), &h, None));
        assert!(m(host(sm(MatchOp::Equals, "[::1]:8080")), &h, None));
        assert!(!m(host(sm(MatchOp::Equals, "[::1]:9090")), &h, None));
        assert!(
            m(host(sm(MatchOp::Glob, "[::1]")), &h, None),
            "bracketed IPv6 isn't a glob class"
        );
        let h6 = req("GET", "http://[2001:db8::1]/");
        assert!(m(host(sm(MatchOp::StartsWith, "2001:db8:")), &h6, None));
        assert!(!m(host(sm(MatchOp::Equals, "::1")), &h6, None));
    }

    /// `negate` used to invert only the host comparison of a `host:port` pattern while still
    /// requiring the port, instead of inverting the whole match as documented.
    #[test]
    fn negated_host_with_port_inverts_the_whole_match() {
        let host = |s: StringMatch| Matcher::Host { pattern: s };
        let not_dev = host(neg(sm(MatchOp::Equals, "example.com:8080")));
        for (url, expected) in [
            ("http://example.com:8080/", false),
            ("http://example.com/", true),
            ("http://other.com/", true),
            ("http://other.com:8080/", true),
        ] {
            assert_eq!(
                m(not_dev.clone(), &req("GET", url), None),
                expected,
                "{url}"
            );
            // Same as wrapping the positive matcher in `not`.
            let wrapped = Matcher::Not {
                matcher: Box::new(host(sm(MatchOp::Equals, "example.com:8080"))),
            };
            assert_eq!(m(wrapped, &req("GET", url), None), expected, "{url}");
        }
        let not_v6 = host(neg(sm(MatchOp::Equals, "[::1]:8080")));
        assert!(!m(not_v6.clone(), &req("GET", "http://[::1]:8080/"), None));
        assert!(m(not_v6.clone(), &req("GET", "http://[::1]:9090/"), None));
        assert!(m(not_v6, &req("GET", "http://[::2]:8080/"), None));
        // Without a port, negation is unchanged.
        let not_host = host(neg(sm(MatchOp::Equals, "example.com")));
        assert!(!m(
            not_host.clone(),
            &req("GET", "http://example.com:8080/"),
            None
        ));
        assert!(m(not_host, &req("GET", "http://other.com:8080/"), None));
    }

    #[test]
    fn port() {
        let h = req("GET", "http://example.com:8080/");
        assert!(m(
            Matcher::Port {
                ports: vec![80, 8080]
            },
            &h,
            None
        ));
        assert!(!m(Matcher::Port { ports: vec![80] }, &h, None));
        assert!(m(
            Matcher::Port { ports: vec![443] },
            &req("GET", "https://example.com/"),
            None
        ));
        assert!(!m(Matcher::Port { ports: vec![] }, &h, None));
    }

    #[test]
    fn path() {
        let h = req("GET", "https://example.com/api/Users/42?expand=1");
        let path = |s: StringMatch| Matcher::Path { pattern: s };
        assert!(m(path(sm(MatchOp::Equals, "/api/users/42")), &h, None));
        assert!(!m(path(cs(MatchOp::Equals, "/api/users/42")), &h, None));
        assert!(
            !m(
                path(sm(MatchOp::Equals, "/api/users/42?expand=1")),
                &h,
                None
            ),
            "query excluded"
        );
        assert!(m(path(sm(MatchOp::Glob, "/api/*/42")), &h, None));
        assert!(
            !m(path(sm(MatchOp::Glob, "/api/*")), &h, None),
            "* stays within a segment"
        );
        assert!(m(path(sm(MatchOp::Glob, "/api/**")), &h, None));
        assert!(m(path(sm(MatchOp::Glob, "/**/42")), &h, None));
        assert!(m(path(sm(MatchOp::StartsWith, "/api/")), &h, None));
        assert!(m(path(sm(MatchOp::Regex, r"^/api/users/\d+$")), &h, None));
        assert!(m(path(neg(sm(MatchOp::StartsWith, "/admin"))), &h, None));
        assert!(m(path(sm(MatchOp::Exists, "")), &h, None));
    }

    #[test]
    fn path_encoding() {
        let h = req("GET", "http://example.com/files/my%20report%2Epdf");
        let path = |s: StringMatch| Matcher::Path { pattern: s };
        assert!(
            m(
                path(sm(MatchOp::Equals, "/files/my%20report%2Epdf")),
                &h,
                None
            ),
            "raw"
        );
        assert!(
            m(path(sm(MatchOp::Equals, "/files/my report.pdf")), &h, None),
            "decoded"
        );
        assert!(m(path(sm(MatchOp::Glob, "/files/*.pdf")), &h, None));
        assert!(!m(
            path(neg(sm(MatchOp::Equals, "/files/my report.pdf"))),
            &h,
            None
        ));
        // Lower-case escapes and UTF-8.
        let u = req("GET", "http://example.com/caf%c3%a9");
        assert!(m(path(sm(MatchOp::Equals, "/caf\u{e9}")), &u, None));
        assert!(
            m(path(sm(MatchOp::Equals, "/CAF\u{c9}")), &u, None),
            "unicode case folding"
        );
        // Invalid escapes never panic.
        let bad = req("GET", "http://example.com/%zz/%");
        assert!(m(path(sm(MatchOp::Equals, "/%zz/%")), &bad, None));
    }

    #[test]
    fn url() {
        let h = req("GET", "https://example.com:8443/a/b?x=1&y=2");
        let url = |s: StringMatch| Matcher::Url { pattern: s };
        assert!(m(
            url(sm(MatchOp::Equals, "https://example.com:8443/a/b?x=1&y=2")),
            &h,
            None
        ));
        assert!(
            m(url(sm(MatchOp::Glob, "https://*.com:*/a/*")), &h, None),
            "* crosses / in URLs"
        );
        assert!(m(url(sm(MatchOp::Contains, "y=2")), &h, None));
        assert!(m(
            url(sm(MatchOp::Regex, r"^https://example\.com:8443/")),
            &h,
            None
        ));
        assert!(!m(url(sm(MatchOp::StartsWith, "http://")), &h, None));
    }

    #[test]
    fn protocol() {
        let p = |list: &[&str]| Matcher::Protocol {
            protocols: list.iter().map(|s| s.to_string()).collect(),
        };
        let http = req("GET", "http://example.com/");
        let https = req("GET", "https://example.com/");
        let ws = with_headers(
            req("GET", "http://example.com/socket"),
            &[("Connection", "Upgrade"), ("Upgrade", "WebSocket")],
        );
        let wss = with_headers(
            req("GET", "https://example.com/socket"),
            &[("Upgrade", "websocket")],
        );
        assert!(m(p(&["http"]), &http, None));
        assert!(!m(p(&["https"]), &http, None));
        assert!(m(p(&["HTTPS"]), &https, None));
        assert!(m(p(&["ws"]), &ws, None));
        assert!(!m(p(&["http"]), &ws, None), "upgrades are ws, not http");
        assert!(m(p(&["wss"]), &wss, None));
        assert!(m(p(&["ws", "wss"]), &wss, None));
        let mut h2ws = with_headers(
            req("CONNECT", "https://example.com/chat"),
            &[(":protocol", "websocket")],
        );
        h2ws.http_version = HttpVersion::H2;
        assert!(m(p(&["wss"]), &h2ws, None));
        let mut scheme_ws = req("GET", "http://example.com/");
        scheme_ws.scheme = "wss".into();
        assert!(m(p(&["wss"]), &scheme_ws, None));
        assert_eq!(
            RulesEngine::new(set(vec![rule("r", vec![p(&["ftp"])], mock(200))]))
                .errors()
                .len(),
            1
        );
    }

    #[test]
    fn query() {
        let h = req(
            "GET",
            "http://example.com/search?q=hello+world&tag=a&tag=b&empty=&flag&enc=%C3%A9t%C3%A9",
        );
        let q = |params: Vec<KeyValueMatch>| Matcher::Query { params };
        assert!(
            m(
                q(vec![kv("q", Some(sm(MatchOp::Equals, "hello world")))]),
                &h,
                None
            ),
            "+ decodes to space"
        );
        assert!(
            m(q(vec![kv("tag", Some(sm(MatchOp::Equals, "b")))]), &h, None),
            "any repeated value"
        );
        assert!(m(
            q(vec![
                kv("tag", Some(sm(MatchOp::Equals, "a"))),
                kv("tag", Some(sm(MatchOp::Equals, "b")))
            ]),
            &h,
            None
        ));
        assert!(!m(
            q(vec![kv("tag", Some(sm(MatchOp::Equals, "c")))]),
            &h,
            None
        ));
        assert!(
            m(q(vec![kv("flag", None)]), &h, None),
            "key without value is present"
        );
        assert!(m(
            q(vec![kv("empty", Some(sm(MatchOp::Equals, "")))]),
            &h,
            None
        ));
        assert!(m(
            q(vec![kv("enc", Some(sm(MatchOp::Equals, "\u{e9}t\u{e9}")))]),
            &h,
            None
        ));
        assert!(
            m(
                q(vec![kv("q", Some(sm(MatchOp::Regex, "^hel+o")))]),
                &h,
                None
            ),
            "regex query matching (HTK #231)"
        );
        assert!(
            !m(q(vec![kv("Q", None)]), &h, None),
            "keys are case-sensitive"
        );
        assert!(!m(
            q(vec![kv("missing", Some(sm(MatchOp::Equals, "x")))]),
            &h,
            None
        ));
        assert!(
            m(
                q(vec![kv("missing", Some(neg(sm(MatchOp::Exists, ""))))]),
                &h,
                None
            ),
            "negated exists = absent"
        );
        assert!(!m(
            q(vec![kv("q", Some(neg(sm(MatchOp::Exists, ""))))]),
            &h,
            None
        ));
        assert!(m(
            q(vec![kv("tag", Some(neg(sm(MatchOp::Equals, "z"))))]),
            &h,
            None
        ));
        assert!(
            !m(
                q(vec![kv("tag", Some(neg(sm(MatchOp::Equals, "a"))))]),
                &h,
                None
            ),
            "negation of 'any value'"
        );
        assert!(m(q(vec![kv("q", None), kv("flag", None)]), &h, None));
        assert!(!m(
            q(vec![kv("q", None)]),
            &req("GET", "http://example.com/search"),
            None
        ));
    }

    #[test]
    fn header() {
        let h = with_headers(
            req("GET", "http://example.com/"),
            &[
                ("Accept", "text/html"),
                ("X-Multi", "one"),
                ("x-multi", "Two"),
                ("Authorization", "Bearer abc"),
            ],
        );
        let hd = |headers: Vec<KeyValueMatch>| Matcher::Header { headers };
        assert!(m(
            hd(vec![kv("accept", Some(sm(MatchOp::Equals, "TEXT/HTML")))]),
            &h,
            None
        ));
        assert!(!m(
            hd(vec![kv("accept", Some(cs(MatchOp::Equals, "TEXT/HTML")))]),
            &h,
            None
        ));
        assert!(
            m(
                hd(vec![kv("X-MULTI", Some(sm(MatchOp::Equals, "two")))]),
                &h,
                None
            ),
            "any header line"
        );
        assert!(m(
            hd(vec![kv(
                "authorization",
                Some(sm(MatchOp::StartsWith, "bearer "))
            )]),
            &h,
            None
        ));
        assert!(m(hd(vec![kv("authorization", None)]), &h, None));
        assert!(
            m(
                hd(vec![kv("cookie", Some(neg(sm(MatchOp::Exists, ""))))]),
                &h,
                None
            ),
            "header absent"
        );
        assert!(!m(hd(vec![kv("cookie", None)]), &h, None));
        assert!(m(
            hd(vec![kv("x-missing", Some(neg(sm(MatchOp::Equals, "1"))))]),
            &h,
            None
        ));
        assert!(m(
            hd(vec![kv("accept", Some(sm(MatchOp::Glob, "text/*")))]),
            &h,
            None
        ));
        assert!(m(
            hd(vec![
                kv("accept", Some(sm(MatchOp::Contains, "html"))),
                kv("x-multi", None)
            ]),
            &h,
            None
        ));
    }

    #[test]
    fn cookie() {
        let h = with_headers(
            req("GET", "http://example.com/"),
            &[
                ("Cookie", "session=abc=def; theme=dark"),
                ("cookie", "enc=a%20b; flag"),
            ],
        );
        let ck = |cookies: Vec<KeyValueMatch>| Matcher::Cookie { cookies };
        assert!(
            m(
                ck(vec![kv("session", Some(cs(MatchOp::Equals, "abc=def")))]),
                &h,
                None
            ),
            "value with = (HTK bug)"
        );
        assert!(
            m(
                ck(vec![
                    kv("session", None),
                    kv("theme", Some(sm(MatchOp::Equals, "dark")))
                ]),
                &h,
                None
            ),
            "several cookies at once (HTK bug)"
        );
        assert!(
            m(
                ck(vec![kv("enc", Some(sm(MatchOp::Equals, "a b")))]),
                &h,
                None
            ),
            "percent-decoded"
        );
        assert!(
            m(
                ck(vec![kv("enc", Some(sm(MatchOp::Equals, "a%20b")))]),
                &h,
                None
            ),
            "raw"
        );
        assert!(m(ck(vec![kv("flag", None)]), &h, None));
        assert!(!m(ck(vec![kv("Session", None)]), &h, None));
        assert!(m(
            ck(vec![kv("tracking", Some(neg(sm(MatchOp::Exists, ""))))]),
            &h,
            None
        ));
        assert_eq!(
            parse_cookies(&h.headers),
            vec![
                ("session".to_string(), "abc=def".to_string()),
                ("theme".to_string(), "dark".to_string()),
                ("enc".to_string(), "a%20b".to_string()),
                ("flag".to_string(), String::new()),
            ]
        );
        assert!(parse_cookies(&Headers(vec![("Cookie".into(), " ; ;".into())])).is_empty());
    }

    #[test]
    fn body() {
        let h = req("POST", "http://example.com/");
        let body = |s: StringMatch| Matcher::Body { pattern: s };
        let payload: &[u8] = b"Hello World payload";
        assert!(
            m(body(sm(MatchOp::Contains, "WORLD")), &h, Some(payload)),
            "case-insensitive body (HTK #228)"
        );
        assert!(!m(body(cs(MatchOp::Contains, "WORLD")), &h, Some(payload)));
        assert!(m(
            body(cs(MatchOp::Equals, "Hello World payload")),
            &h,
            Some(payload)
        ));
        assert!(m(
            body(sm(MatchOp::Regex, r"^hello\s+world")),
            &h,
            Some(payload)
        ));
        assert!(m(
            body(neg(sm(MatchOp::Contains, "secret"))),
            &h,
            Some(payload)
        ));
        assert!(!m(body(sm(MatchOp::Exists, "")), &h, Some(b"")));
        assert!(m(body(sm(MatchOp::Exists, "")), &h, Some(payload)));
        // Unknown body never matches, even negated.
        assert!(!m(body(sm(MatchOp::Contains, "x")), &h, None));
        assert!(!m(body(neg(sm(MatchOp::Contains, "x"))), &h, None));
        // Invalid UTF-8 is matched lossily.
        assert!(m(
            body(sm(MatchOp::Contains, "abc")),
            &h,
            Some(b"\xff\xfeabc")
        ));
    }

    #[test]
    fn json_body() {
        let h = req("POST", "http://example.com/");
        let jb = |value: serde_json::Value, partial| Matcher::JsonBody { value, partial };
        let body: &[u8] = br#"{"user": {"id": 7, "roles": ["a", "b"]}, "count": 2.0}"#;
        assert!(m(
            jb(
                json!({"count": 2, "user": {"roles": ["a", "b"], "id": 7}}),
                false
            ),
            &h,
            Some(body)
        ));
        assert!(!m(jb(json!({"count": 2}), false), &h, Some(body)));
        assert!(m(jb(json!({"count": 2}), true), &h, Some(body)));
        assert!(m(
            jb(json!({"user": {"roles": ["a"]}}), true),
            &h,
            Some(body)
        ));
        assert!(!m(
            jb(json!({"user": {"roles": ["b"]}}), true),
            &h,
            Some(body)
        ));
        assert!(!m(jb(json!({"user": {"id": "7"}}), true), &h, Some(body)));
        assert!(m(jb(json!({}), true), &h, Some(body)));
        assert!(
            !m(jb(json!({}), true), &h, Some(b"not json")),
            "invalid JSON never matches"
        );
        assert!(!m(jb(json!({}), true), &h, None));
        assert!(m(jb(json!([1, 2]), false), &h, Some(b" [1, 2] ")));
        assert!(m(jb(json!(null), false), &h, Some(b"null")));
    }

    #[test]
    fn form() {
        let f = |fields: Vec<KeyValueMatch>| Matcher::Form { fields };
        let urlenc = with_headers(
            req("POST", "http://example.com/"),
            &[(
                "Content-Type",
                "application/x-www-form-urlencoded; charset=UTF-8",
            )],
        );
        let body: &[u8] = b"user=alice+smith&tag=a&tag=b&pw=p%40ss";
        assert!(m(
            f(vec![kv("user", Some(sm(MatchOp::Equals, "alice smith")))]),
            &urlenc,
            Some(body)
        ));
        assert!(m(
            f(vec![
                kv("tag", Some(sm(MatchOp::Equals, "b"))),
                kv("pw", Some(cs(MatchOp::Equals, "p@ss")))
            ]),
            &urlenc,
            Some(body)
        ));
        assert!(!m(f(vec![kv("missing", None)]), &urlenc, Some(body)));
        assert!(m(
            f(vec![kv("missing", Some(neg(sm(MatchOp::Exists, ""))))]),
            &urlenc,
            Some(body)
        ));

        let multipart = with_headers(
            req("POST", "http://example.com/upload"),
            &[("Content-Type", "multipart/form-data; boundary=XyZ")],
        );
        let mp: &[u8] = b"--XyZ\r\nContent-Disposition: form-data; name=\"title\"\r\n\r\nMy file\r\n--XyZ\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.txt\"\r\nContent-Type: text/plain\r\n\r\nfile contents\r\n--XyZ--\r\n";
        assert!(m(
            f(vec![kv("title", Some(sm(MatchOp::Equals, "my file")))]),
            &multipart,
            Some(mp)
        ));
        assert!(m(
            f(vec![kv("file", Some(sm(MatchOp::Contains, "contents")))]),
            &multipart,
            Some(mp)
        ));
        assert!(!m(f(vec![kv("other", None)]), &multipart, Some(mp)));

        let json_ct = with_headers(
            req("POST", "http://example.com/"),
            &[("Content-Type", "application/json")],
        );
        assert!(
            !m(f(vec![kv("user", None)]), &json_ct, Some(body)),
            "other content types never match"
        );
        assert!(!m(
            f(vec![kv("user", None)]),
            &req("POST", "http://example.com/"),
            Some(body)
        ));
    }

    #[test]
    fn graphql_operation() {
        let g = |s: StringMatch| Matcher::GraphqlOperation { pattern: s };
        let post = with_headers(
            req("POST", "http://example.com/graphql"),
            &[("Content-Type", "application/json")],
        );
        assert!(m(
            g(cs(MatchOp::Equals, "GetUser")),
            &post,
            Some(br#"{"operationName":"GetUser","query":"query GetUser { me }"}"#)
        ));
        assert!(m(
            g(cs(MatchOp::Equals, "Parsed")),
            &post,
            Some(br#"{"query":"query Parsed($id: ID) { user(id: $id) { id } }"}"#)
        ));
        assert!(
            m(
                g(cs(MatchOp::Equals, "Second")),
                &post,
                Some(br#"[{"operationName":"First"},{"query":"mutation Second { x }"}]"#)
            ),
            "batched"
        );
        assert!(!m(
            g(cs(MatchOp::Equals, "GetUser")),
            &post,
            Some(br#"{"query":"{ me }"}"#)
        ));
        assert!(m(
            g(neg(cs(MatchOp::Equals, "GetUser"))),
            &post,
            Some(br#"{"query":"{ me }"}"#)
        ));
        assert!(m(
            g(sm(MatchOp::StartsWith, "get")),
            &post,
            Some(br#"{"operationName":"GetUser"}"#)
        ));
        assert!(!m(
            g(sm(MatchOp::Exists, "")),
            &post,
            Some(br#"{"query":"{ me }"}"#)
        ));
        let get = req(
            "GET",
            "http://example.com/graphql?operationName=ListThings&query=query%20ListThings%20%7B%20a%20%7D",
        );
        assert!(
            m(g(cs(MatchOp::Equals, "ListThings")), &get, None),
            "GET needs no body"
        );
        let get_q = req(
            "GET",
            "http://example.com/graphql?query=query%20FromDoc%20%7B%20a%20%7D",
        );
        assert!(m(g(cs(MatchOp::Equals, "FromDoc")), &get_q, None));
        let raw = with_headers(
            req("POST", "http://example.com/graphql"),
            &[("Content-Type", "application/graphql")],
        );
        assert!(m(
            g(cs(MatchOp::Equals, "Raw")),
            &raw,
            Some(b"query Raw { a }")
        ));
        assert!(
            !m(g(cs(MatchOp::Equals, "Raw")), &post, None),
            "unknown body"
        );
    }

    #[test]
    fn source_and_version() {
        let h = req("GET", "http://example.com/");
        let s = |p: StringMatch| Matcher::Source { pattern: p };
        assert!(m_src(
            s(sm(MatchOp::Equals, "chrome")),
            &h,
            None,
            Some("Chrome")
        ));
        assert!(!m_src(
            s(sm(MatchOp::Equals, "chrome")),
            &h,
            None,
            Some("firefox")
        ));
        assert!(!m_src(s(sm(MatchOp::Equals, "chrome")), &h, None, None));
        assert!(
            m_src(s(neg(sm(MatchOp::Equals, "chrome"))), &h, None, None),
            "unknown source counts as absent"
        );
        assert!(m_src(
            s(sm(MatchOp::Glob, "android-*")),
            &h,
            None,
            Some("android-adb")
        ));
        let v = |list: &[&str]| Matcher::HttpVersion {
            versions: list.iter().map(|s| s.to_string()).collect(),
        };
        assert!(m(v(&["HTTP/1.1"]), &h, None));
        assert!(!m(v(&["HTTP/2"]), &h, None));
        let mut h2 = h.clone();
        h2.http_version = HttpVersion::H2;
        assert!(m(v(&["h2", "HTTP/3"]), &h2, None));
        assert_eq!(
            RulesEngine::new(set(vec![rule("r", vec![v(&["HTTP/9"])], mock(200))]))
                .errors()
                .len(),
            1
        );
    }

    #[test]
    fn logic() {
        let h = req("GET", "http://example.com/a");
        let get = || Matcher::Method {
            methods: vec!["GET".into()],
        };
        let post = || Matcher::Method {
            methods: vec!["POST".into()],
        };
        assert!(m(
            Matcher::Not {
                matcher: Box::new(post())
            },
            &h,
            None
        ));
        assert!(!m(
            Matcher::Not {
                matcher: Box::new(get())
            },
            &h,
            None
        ));
        assert!(m(
            Matcher::Any {
                matchers: vec![post(), get()]
            },
            &h,
            None
        ));
        assert!(!m(Matcher::Any { matchers: vec![] }, &h, None));
        assert!(m(Matcher::All { matchers: vec![] }, &h, None));
        assert!(!m(
            Matcher::All {
                matchers: vec![get(), post()]
            },
            &h,
            None
        ));
        let nested = Matcher::Any {
            matchers: vec![
                post(),
                Matcher::All {
                    matchers: vec![
                        get(),
                        Matcher::Path {
                            pattern: sm(MatchOp::Equals, "/a"),
                        },
                    ],
                },
            ],
        };
        assert!(m(nested, &h, None));
        // OR with a body matcher: decided without the body when another branch matches.
        let any_body = Matcher::Any {
            matchers: vec![
                get(),
                Matcher::Body {
                    pattern: sm(MatchOp::Contains, "x"),
                },
            ],
        };
        assert!(m(any_body, &h, None));
        let not_body = Matcher::Not {
            matcher: Box::new(Matcher::Body {
                pattern: sm(MatchOp::Contains, "x"),
            }),
        };
        assert!(
            !m(not_body, &h, None),
            "unknown body stays unknown through NOT"
        );
    }

    #[test]
    fn empty_matchers_match_everything() {
        let e = engine(vec![rule("r", vec![], mock(200))]);
        let head = req("DELETE", "https://x.y/z");
        assert!(
            e.dry_run(&MatchContext {
                head: &head,
                body: None,
                source: None
            })
            .terminal_rule
            .is_some()
        );
    }
}

// ==========================================================================================
// Evaluation semantics
// ==========================================================================================

mod engine_tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn modifier_stacking_order() {
        let e = engine(vec![
            rule(
                "t1",
                vec![],
                Action::Transform {
                    request: Some(set_header_transform("x-a", "1")),
                    response: None,
                },
            ),
            rule(
                "t2",
                vec![Matcher::Host {
                    pattern: sm(MatchOp::Glob, "*.api.com"),
                }],
                Action::Transform {
                    request: Some(set_header_transform("x-b", "2")),
                    response: Some(ResponseTransform {
                        status: Some(299),
                        ..Default::default()
                    }),
                },
            ),
            rule(
                "skip",
                vec![Matcher::Method {
                    methods: vec!["POST".into()],
                }],
                mock(500),
            ),
            rule(
                "term",
                vec![],
                Action::Passthrough {
                    request: Some(set_header_transform("x-c", "3")),
                    response: Some(ResponseTransform {
                        status_message: Some("Yo".into()),
                        ..Default::default()
                    }),
                },
            ),
            rule("after", vec![], mock(404)),
        ]);
        let d = decide(&e, &req("GET", "https://v1.api.com/users"), None);
        assert_eq!(d.matched, vec!["t1", "t2", "term"]);
        assert_eq!(d.terminal_rule.as_deref(), Some("term"));
        let names: Vec<&str> = d
            .request_transforms
            .iter()
            .map(|t| t.set_headers.0[0].0.as_str())
            .collect();
        assert_eq!(names, vec!["x-a", "x-b", "x-c"]);
        assert_eq!(d.response_transforms.len(), 2);
        assert_eq!(d.response_transforms[0].status, Some(299));
        assert_eq!(
            d.response_transforms[1].status_message.as_deref(),
            Some("Yo")
        );
        assert!(matches!(d.action, Action::Passthrough { .. }));
        assert!(!d.is_plain_passthrough());
    }

    #[test]
    fn first_terminal_wins_and_implicit_passthrough() {
        let e = engine(vec![
            rule(
                "m1",
                vec![Matcher::Path {
                    pattern: sm(MatchOp::Equals, "/a"),
                }],
                mock(201),
            ),
            rule("m2", vec![], mock(202)),
            rule("m3", vec![], mock(203)),
        ]);
        let d = decide(&e, &req("GET", "http://x/a"), None);
        assert_eq!(
            (d.terminal_rule.as_deref(), d.matched.len()),
            (Some("m1"), 1)
        );
        assert!(matches!(d.action, Action::Mock { status: 201, .. }));
        let d = decide(&e, &req("GET", "http://x/b"), None);
        assert_eq!(d.terminal_rule.as_deref(), Some("m2"));

        let only_modifiers = engine(vec![rule(
            "h",
            vec![],
            Action::Highlight {
                color: "red".into(),
                note: None,
            },
        )]);
        let d = decide(&only_modifiers, &req("GET", "http://x/"), None);
        assert_eq!(d.terminal_rule, None);
        assert_eq!(d.action, passthrough());
        assert_eq!(d.matched, vec!["h"]);
        assert!(
            d.is_plain_passthrough(),
            "highlight alone doesn't modify traffic"
        );

        let empty = RulesEngine::default();
        let d = decide(&empty, &req("GET", "http://x/"), None);
        assert!(d.is_plain_passthrough() && d.matched.is_empty());
    }

    #[test]
    fn decision_assembly() {
        let e = engine(vec![
            rule(
                "b1",
                vec![],
                Action::Breakpoint {
                    request: true,
                    response: false,
                },
            ),
            rule(
                "b2",
                vec![],
                Action::Breakpoint {
                    request: false,
                    response: true,
                },
            ),
            rule(
                "d1",
                vec![],
                Action::Delay {
                    ms: 100,
                    jitter_ms: 0,
                },
            ),
            rule(
                "d2",
                vec![],
                Action::Delay {
                    ms: 50,
                    jitter_ms: 20,
                },
            ),
            rule(
                "th1",
                vec![],
                Action::Throttle {
                    download_bps: 1000,
                    upload_bps: 0,
                    latency_ms: 10,
                },
            ),
            rule(
                "th2",
                vec![],
                Action::Throttle {
                    download_bps: 5000,
                    upload_bps: 300,
                    latency_ms: 40,
                },
            ),
            rule(
                "th3",
                vec![],
                Action::Throttle {
                    download_bps: 0,
                    upload_bps: 900,
                    latency_ms: 0,
                },
            ),
            rule(
                "h1",
                vec![],
                Action::Highlight {
                    color: "red".into(),
                    note: Some("first".into()),
                },
            ),
            rule(
                "h2",
                vec![],
                Action::Highlight {
                    color: "blue".into(),
                    note: None,
                },
            ),
            rule(
                "s1",
                vec![],
                Action::Script {
                    source: "function onRequest(r) { return r; }".into(),
                },
            ),
            rule(
                "s2",
                vec![],
                Action::Script {
                    source: "function onResponse(q, r) { return r; }".into(),
                },
            ),
        ]);
        let mut delays = std::collections::HashSet::new();
        for _ in 0..50 {
            let d = decide(&e, &req("GET", "http://x/"), None);
            assert!(d.break_on_request && d.break_on_response);
            assert!((150..=170).contains(&d.delay_ms), "delay {}", d.delay_ms);
            delays.insert(d.delay_ms);
            assert_eq!(
                d.throttle,
                Some(Throttle {
                    download_bps: 1000,
                    upload_bps: 300,
                    latency_ms: 40
                })
            );
            assert_eq!(d.highlight, Some(("blue".to_string(), None)));
            if cfg!(feature = "scripting") {
                assert_eq!(d.scripts.len(), 2);
                assert!(d.scripts[0].contains("onRequest"));
            }
            assert_eq!(d.terminal_rule, None);
        }
        assert!(delays.len() > 1, "jitter is random");
    }

    #[test]
    fn disabled_rules_and_groups_are_skipped() {
        let mut disabled = rule("off", vec![], mock(500));
        disabled.enabled = false;
        let rs = RuleSet {
            version: RULESET_VERSION,
            items: vec![
                RuleItem::Rule(disabled),
                RuleItem::Group(RuleGroup {
                    id: "g-off".into(),
                    title: "off".into(),
                    enabled: false,
                    collapsed: false,
                    items: vec![
                        RuleItem::Rule(rule("in-off-group", vec![], mock(501))),
                        RuleItem::Group(RuleGroup {
                            id: "g-nested".into(),
                            title: "nested".into(),
                            enabled: true,
                            collapsed: false,
                            items: vec![RuleItem::Rule(rule("deep", vec![], mock(502)))],
                        }),
                    ],
                }),
                RuleItem::Group(RuleGroup {
                    id: "g-on".into(),
                    title: "on".into(),
                    enabled: true,
                    collapsed: true,
                    items: vec![RuleItem::Rule(rule("on", vec![], mock(200)))],
                }),
            ],
        };
        let e = RulesEngine::new(rs.clone());
        assert!(e.errors().is_empty());
        let d = decide(&e, &req("GET", "http://x/"), None);
        assert_eq!(d.terminal_rule.as_deref(), Some("on"));
        assert_eq!(e.rules(), rs);
        let ids: Vec<String> = e.stats().into_iter().map(|s| s.rule_id).collect();
        assert_eq!(
            ids,
            vec!["off", "in-off-group", "deep", "on"],
            "stats cover disabled rules too"
        );
    }

    #[test]
    fn max_matches_budget() {
        let mut once = rule("once", vec![], mock(201));
        once.max_matches = Some(1);
        let mut twice = rule(
            "twice",
            vec![],
            Action::Highlight {
                color: "red".into(),
                note: None,
            },
        );
        twice.max_matches = Some(2);
        let mut zero = rule("zero", vec![], mock(500));
        zero.max_matches = Some(0);
        let e = engine(vec![zero, twice, once, rule("fallback", vec![], mock(200))]);
        let h = req("GET", "http://x/");
        assert_eq!(decide(&e, &h, None).matched, vec!["twice", "once"]);
        assert_eq!(decide(&e, &h, None).matched, vec!["twice", "fallback"]);
        assert_eq!(decide(&e, &h, None).matched, vec!["fallback"]);

        // Dry runs see exhausted budgets but don't consume them.
        let mut fresh = rule("fresh", vec![], mock(204));
        fresh.max_matches = Some(1);
        let e2 = engine(vec![fresh, rule("fb", vec![], mock(200))]);
        let ctx = MatchContext {
            head: &h,
            body: None,
            source: None,
        };
        for _ in 0..3 {
            assert_eq!(e2.dry_run(&ctx).terminal_rule.as_deref(), Some("fresh"));
        }
        assert_eq!(e2.stats()[0].matches, 0);
        assert_eq!(e2.stats()[0].last_matched_at, None);
        assert_eq!(
            decide(&e2, &h, None).terminal_rule.as_deref(),
            Some("fresh")
        );
        assert_eq!(e2.dry_run(&ctx).terminal_rule.as_deref(), Some("fb"));
        // reset_stats re-arms the budget.
        e2.reset_stats();
        assert_eq!(e2.stats()[0].matches, 0);
        assert_eq!(
            decide(&e2, &h, None).terminal_rule.as_deref(),
            Some("fresh")
        );
    }

    #[test]
    fn max_matches_is_atomic_under_concurrency() {
        let mut limited = rule("limited", vec![], mock(201));
        limited.max_matches = Some(100);
        let e = Arc::new(engine(vec![limited, rule("rest", vec![], mock(200))]));
        let threads: Vec<_> = (0..16)
            .map(|_| {
                let e = Arc::clone(&e);
                std::thread::spawn(move || {
                    let h = req("GET", "http://x/");
                    (0..500)
                        .filter(|_| {
                            decide(&e, &h, None).terminal_rule.as_deref() == Some("limited")
                        })
                        .count()
                })
            })
            .collect();
        let total: usize = threads.into_iter().map(|t| t.join().unwrap()).sum();
        assert_eq!(total, 100);
        let stats = e.stats();
        assert_eq!(stats[0].matches, 100);
        assert_eq!(stats[1].matches, 16 * 500 - 100);
    }

    #[test]
    fn concurrent_set_rules_and_evaluate() {
        let e = Arc::new(engine(vec![rule("a", vec![], mock(200))]));
        let writer = {
            let e = Arc::clone(&e);
            std::thread::spawn(move || {
                for i in 0..200 {
                    let id = if i % 2 == 0 { "a" } else { "b" };
                    e.set_rules(set(vec![
                        rule("a", vec![], mock(200)),
                        rule(id, vec![], mock(201)),
                    ]));
                }
            })
        };
        let readers: Vec<_> = (0..4)
            .map(|_| {
                let e = Arc::clone(&e);
                std::thread::spawn(move || {
                    let h = req("GET", "http://x/");
                    for _ in 0..2000 {
                        assert_eq!(decide(&e, &h, None).terminal_rule.as_deref(), Some("a"));
                    }
                })
            })
            .collect();
        writer.join().unwrap();
        readers.into_iter().for_each(|r| r.join().unwrap());
        assert_eq!(
            e.stats()[0].matches,
            8000,
            "counter of the persisting rule survived every swap"
        );
    }

    #[test]
    fn set_rules_keeps_counters_for_persisting_ids() {
        let mut once = rule("once", vec![], mock(201));
        once.max_matches = Some(1);
        let e = engine(vec![
            once.clone(),
            rule("keep", vec![], mock(200)),
            rule("drop", vec![], mock(202)),
        ]);
        let h = req("GET", "http://x/");
        for _ in 0..3 {
            decide(&e, &h, None);
        }
        let before = e.stats();
        assert_eq!(
            before.iter().map(|s| s.matches).collect::<Vec<_>>(),
            vec![1, 2, 0]
        );
        let last = before[1].last_matched_at.expect("timestamp");
        assert!(last > 1.6e12, "unix ms");

        // Reorder, edit the persisting rule, remove one, add one.
        let mut keep = rule(
            "keep",
            vec![Matcher::Method {
                methods: vec!["GET".into()],
            }],
            mock(299),
        );
        keep.title = Some("edited".into());
        e.set_rules(set(vec![
            rule(
                "new",
                vec![Matcher::Path {
                    pattern: sm(MatchOp::Equals, "/new"),
                }],
                mock(203),
            ),
            keep,
            once,
        ]));
        let after = e.stats();
        assert_eq!(
            after
                .iter()
                .map(|s| (s.rule_id.as_str(), s.matches))
                .collect::<Vec<_>>(),
            vec![("new", 0), ("keep", 2), ("once", 1)]
        );
        assert_eq!(after[1].last_matched_at, Some(last));
        assert_eq!(
            e.rules()
                .find_rule("keep")
                .and_then(|r| r.title.clone())
                .as_deref(),
            Some("edited")
        );
        // The exhausted budget persisted too.
        assert_eq!(decide(&e, &h, None).terminal_rule.as_deref(), Some("keep"));
        assert_eq!(e.stats()[1].matches, 3);
        // A removed-then-re-added id starts from zero.
        e.set_rules(set(vec![rule("drop", vec![], mock(202))]));
        assert_eq!(e.stats()[0].matches, 0);
    }

    /// Unclosed `{` in a glob used to double the compile time per brace (a rule save with 30
    /// of them hung for minutes, holding the rules lock) and to recurse once per brace (about
    /// 20,000 of them aborted the process with a stack overflow).
    #[test]
    fn pathological_globs_compile_quickly() {
        let start = std::time::Instant::now();
        let unclosed = format!("/api/{}", "{".repeat(30));
        let e = engine(vec![
            rule(
                "path",
                vec![Matcher::Path {
                    pattern: sm(MatchOp::Glob, &unclosed),
                }],
                mock(200),
            ),
            rule(
                "host",
                vec![Matcher::Host {
                    pattern: sm(MatchOp::Glob, &"{".repeat(20_000)),
                }],
                mock(201),
            ),
        ]);
        assert!(
            start.elapsed().as_secs_f64() < 2.0,
            "took {:?}",
            start.elapsed()
        );
        // Unclosed braces are literal.
        let d = decide(&e, &req("GET", &format!("http://x.test{unclosed}")), None);
        assert_eq!(d.terminal_rule.as_deref(), Some("path"));
        // Nesting beyond the limit is reported as an invalid rule instead.
        let nested = format!("{}a{}", "{".repeat(40), "}".repeat(40));
        let e = RulesEngine::new(set(vec![rule(
            "nested",
            vec![Matcher::Path {
                pattern: sm(MatchOp::Glob, &nested),
            }],
            mock(200),
        )]));
        let errors = e.errors();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].1.contains("nested"), "{errors:?}");
    }

    #[test]
    fn invalid_rules_are_reported_and_skipped() {
        let bad_regex = rule(
            "bad-regex",
            vec![Matcher::Path {
                pattern: sm(MatchOp::Regex, "("),
            }],
            mock(500),
        );
        let bad_header = rule(
            "bad-header",
            vec![],
            Action::Mock {
                status: 200,
                status_message: None,
                headers: Headers(vec![("bad name".into(), "x".into())]),
                body: BodySource::Empty,
            },
        );
        let bad_status = rule("bad-status", vec![], mock(42));
        let bad_patch = rule(
            "bad-patch",
            vec![],
            Action::Transform {
                request: Some(RequestTransform {
                    body: Some(BodyTransform::JsonPatch {
                        operations: vec![json!({"op": "frobnicate", "path": "/a"})],
                    }),
                    ..Default::default()
                }),
                response: None,
            },
        );
        let bad_target = rule(
            "bad-target",
            vec![],
            Action::Forward {
                target: "ftp://x".into(),
                strip_prefix: None,
                preserve_host: false,
                request: None,
                response: None,
            },
        );
        let bad_replace = rule(
            "bad-replace",
            vec![],
            Action::Transform {
                request: Some(RequestTransform {
                    url_replace: vec![MatchReplace {
                        find: "[".into(),
                        replace: "".into(),
                        regex: true,
                        case_sensitive: false,
                    }],
                    ..Default::default()
                }),
                response: None,
            },
        );
        let bad_b64 = rule(
            "bad-b64",
            vec![],
            Action::Mock {
                status: 200,
                status_message: None,
                headers: Headers::new(),
                body: BodySource::Base64 { data: "%%%".into() },
            },
        );
        let mut disabled_bad = rule(
            "disabled-bad",
            vec![Matcher::Url {
                pattern: sm(MatchOp::Regex, "["),
            }],
            mock(500),
        );
        disabled_bad.enabled = false;
        let e = RulesEngine::new(set(vec![
            bad_regex,
            bad_header,
            bad_status,
            bad_patch,
            bad_target,
            bad_replace,
            bad_b64,
            disabled_bad,
            rule(
                "dup",
                vec![],
                Action::Highlight {
                    color: "red".into(),
                    note: None,
                },
            ),
            rule("dup", vec![], mock(500)),
            rule("ok", vec![], mock(200)),
        ]));
        let errors = e.errors();
        let ids: Vec<&str> = errors.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "bad-regex",
                "bad-header",
                "bad-status",
                "bad-patch",
                "bad-target",
                "bad-replace",
                "bad-b64",
                "disabled-bad",
                "dup"
            ]
        );
        assert!(errors[0].1.contains("invalid regex"), "{}", errors[0].1);
        assert!(errors[8].1.contains("duplicate"));
        let d = decide(&e, &req("GET", "http://x/"), None);
        assert_eq!(d.matched, vec!["dup", "ok"]);
    }

    #[test]
    fn needs_body() {
        let body_rule = |id: &str, path: &str| {
            rule(
                id,
                vec![
                    Matcher::Path {
                        pattern: sm(MatchOp::Equals, path),
                    },
                    Matcher::Body {
                        pattern: sm(MatchOp::Contains, "x"),
                    },
                ],
                mock(200),
            )
        };
        let e = engine(vec![body_rule("b", "/upload")]);
        assert!(e.needs_body(&req("POST", "http://x/upload"), None));
        assert!(
            !e.needs_body(&req("POST", "http://x/other"), None),
            "non-body matchers exclude it"
        );

        // A terminal rule that certainly matches first ends evaluation.
        let e = engine(vec![
            rule("mock-all", vec![], mock(200)),
            body_rule("b", "/upload"),
        ]);
        assert!(!e.needs_body(&req("POST", "http://x/upload"), None));
        // ... but a modifier doesn't.
        let e = engine(vec![
            rule(
                "hl",
                vec![],
                Action::Highlight {
                    color: "red".into(),
                    note: None,
                },
            ),
            body_rule("b", "/upload"),
        ]);
        assert!(e.needs_body(&req("POST", "http://x/upload"), None));
        // An earlier terminal that depends on the body itself needs it.
        let e = engine(vec![
            body_rule("b1", "/upload"),
            rule("rest", vec![], mock(200)),
        ]);
        assert!(e.needs_body(&req("POST", "http://x/upload"), None));
        // Exhausted rules don't count.
        let mut limited = body_rule("limited", "/upload");
        limited.max_matches = Some(1);
        let e = engine(vec![limited]);
        let h = req("POST", "http://x/upload");
        assert!(e.needs_body(&h, None));
        decide(&e, &h, Some(b"x"));
        assert!(!e.needs_body(&h, None));
        // Disabled body rules don't count; no body rules → false.
        let mut off = body_rule("off", "/upload");
        off.enabled = false;
        assert!(!engine(vec![off]).needs_body(&h, None));
        assert!(!RulesEngine::default().needs_body(&h, None));
        // Source participates in pre-filtering.
        let e = engine(vec![rule(
            "src",
            vec![
                Matcher::Source {
                    pattern: sm(MatchOp::Equals, "curl"),
                },
                Matcher::JsonBody {
                    value: json!({}),
                    partial: true,
                },
            ],
            mock(200),
        )]);
        assert!(e.needs_body(&h, Some("curl")));
        assert!(!e.needs_body(&h, Some("chrome")));
        // GraphQL GET requests identified by the URL don't need the body.
        let e = engine(vec![rule(
            "gql",
            vec![Matcher::GraphqlOperation {
                pattern: sm(MatchOp::Equals, "Op"),
            }],
            mock(200),
        )]);
        assert!(!e.needs_body(&req("GET", "http://x/graphql?operationName=Op"), None));
        assert!(e.needs_body(&req("POST", "http://x/graphql"), None));
        // Body supplied → body rules evaluate.
        let e = engine(vec![body_rule("b", "/upload")]);
        assert_eq!(
            decide(&e, &h, Some(b"xyz")).terminal_rule.as_deref(),
            Some("b")
        );
        assert_eq!(decide(&e, &h, Some(b"abc")).terminal_rule, None);
        assert_eq!(
            decide(&e, &h, None).terminal_rule,
            None,
            "body unavailable → no match"
        );
    }

    #[test]
    fn engine_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<RulesEngine>();
        assert_send_sync::<RulesSnapshot>();
    }

    /// `needs_body` used to stop at a terminal "mock once" rule that matched. When a concurrent
    /// request used up its budget before `evaluate`, evaluation went on without the body and a
    /// later body rule that should have matched didn't.
    #[test]
    fn budgeted_terminal_rule_does_not_hide_later_body_rules() {
        let api = || Matcher::Path {
            pattern: sm(MatchOp::Equals, "/api"),
        };
        let mut once = rule("once", vec![api()], mock(200));
        once.max_matches = Some(1);
        let body = rule(
            "body",
            vec![
                api(),
                Matcher::Body {
                    pattern: sm(MatchOp::Contains, "x"),
                },
            ],
            mock(201),
        );
        let e = engine(vec![once, body]);
        let h = req("POST", "http://x.test/api");
        // Request B asks whether to buffer, then request A takes the last unit of budget, then
        // B is evaluated with whatever B buffered.
        let b_needs_body = e.needs_body(&h, None);
        let a = decide(&e, &h, Some(b"x"));
        assert_eq!(a.terminal_rule.as_deref(), Some("once"));
        let b = decide(&e, &h, b_needs_body.then_some(&b"x"[..]));
        assert_eq!(b.terminal_rule.as_deref(), Some("body"));
        // Without a budget the terminal rule still ends the scan (no needless buffering).
        let e = engine(vec![
            rule("always", vec![api()], mock(200)),
            rule(
                "body",
                vec![Matcher::Body {
                    pattern: sm(MatchOp::Contains, "x"),
                }],
                mock(201),
            ),
        ]);
        assert!(!e.needs_body(&h, None));
    }

    /// A snapshot keeps `needs_body` and `evaluate` on the same rule set even when the rules are
    /// replaced in between.
    #[test]
    fn snapshot_is_consistent_across_set_rules() {
        let body_rule = rule(
            "body",
            vec![Matcher::Body {
                pattern: sm(MatchOp::Contains, "x"),
            }],
            mock(201),
        );
        let e = engine(vec![rule("plain", vec![], mock(200))]);
        let h = req("POST", "http://x.test/api");
        let snap = e.snapshot();
        e.set_rules(set(vec![body_rule]));
        assert!(e.needs_body(&h, None));
        assert!(!snap.needs_body(&h, None));
        let d = snap.evaluate(&MatchContext {
            head: &h,
            body: None,
            source: None,
        });
        assert_eq!(d.terminal_rule.as_deref(), Some("plain"));
        let dry = snap.dry_run(&MatchContext {
            head: &h,
            body: None,
            source: None,
        });
        assert_eq!(dry.terminal_rule, d.terminal_rule);
        // Counters are the engine's.
        let snap = e.snapshot();
        snap.evaluate(&MatchContext {
            head: &h,
            body: Some(b"x"),
            source: None,
        });
        assert_eq!(e.stats()[0].matches, 1);
    }
}

// ==========================================================================================
// Transforms
// ==========================================================================================

mod transform_tests {
    use super::*;

    fn apply(head: &mut RequestHead, body: &mut Bytes, t: RequestTransform) -> Result<()> {
        apply_request_transform(head, body, &t)
    }

    fn url_replace(find: &str, replace: &str, regex: bool) -> RequestTransform {
        RequestTransform {
            url_replace: vec![MatchReplace {
                find: find.into(),
                replace: replace.into(),
                regex,
                case_sensitive: true,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn method_and_headers() {
        let mut h = with_headers(
            req("get", "http://x/"),
            &[("x-remove", "1"), ("X-Keep", "a"), ("x-keep", "b")],
        );
        let mut b = Bytes::new();
        apply(
            &mut h,
            &mut b,
            RequestTransform {
                method: Some("post".into()),
                set_headers: Headers(vec![
                    ("X-Keep".into(), "new".into()),
                    ("Authorization".into(), "Bearer T".into()),
                ]),
                add_headers: Headers(vec![
                    ("X-Added".into(), "1".into()),
                    ("X-Added".into(), "2".into()),
                ]),
                remove_headers: vec!["X-REMOVE".into()],
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(h.method, "POST");
        // Upper-case names are accepted and preserved (HTK #899 / #766).
        assert_eq!(
            h.headers.0,
            vec![
                ("Host".to_string(), "x".to_string()),
                ("X-Keep".to_string(), "new".to_string()),
                ("Authorization".to_string(), "Bearer T".to_string()),
                ("X-Added".to_string(), "1".to_string()),
                ("X-Added".to_string(), "2".to_string()),
            ]
        );
    }

    #[test]
    fn url_replace_rederives_everything() {
        let mut h = req("GET", "http://old.example.com/api/v1/users?id=1");
        let mut b = Bytes::new();
        apply(
            &mut h,
            &mut b,
            RequestTransform {
                url_replace: vec![
                    MatchReplace {
                        find: r"^http://old\.example\.com".into(),
                        replace: "https://new.example.com:8443".into(),
                        regex: true,
                        case_sensitive: false,
                    },
                    MatchReplace {
                        find: "/V1/".into(),
                        replace: "/v2/".into(),
                        regex: false,
                        case_sensitive: false,
                    },
                ],
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(h.url, "https://new.example.com:8443/api/v2/users?id=1");
        assert_eq!(
            (h.scheme.as_str(), h.host.as_str(), h.port, h.path.as_str()),
            ("https", "new.example.com", 8443, "/api/v2/users?id=1")
        );
        assert_eq!(h.headers.get("host"), Some("new.example.com:8443"));
        assert_eq!(h.headers.len(), 1);
    }

    #[test]
    fn url_replace_groups_ipv6_and_errors() {
        let mut h = req("GET", "http://example.com/users/42/posts");
        let mut b = Bytes::new();
        apply(
            &mut h,
            &mut b,
            url_replace(r"/users/(?<id>\d+)/", "/u/${id}/", true),
        )
        .unwrap();
        assert_eq!(h.path, "/u/42/posts");
        assert_eq!(
            h.headers.get("host"),
            Some("example.com"),
            "authority unchanged"
        );

        let mut h = req("GET", "http://example.com/x");
        apply(
            &mut h,
            &mut b,
            url_replace("example.com", "[::1]:3000", false),
        )
        .unwrap();
        assert_eq!((h.host.as_str(), h.port), ("::1", 3000));
        assert_eq!(h.headers.get("host"), Some("[::1]:3000"));
        assert_eq!(h.url, "http://[::1]:3000/x");

        // Literal replacements don't expand `$`.
        let mut h = req("GET", "http://example.com/price");
        apply(&mut h, &mut b, url_replace("price", "$1cost", false)).unwrap();
        assert_eq!(h.path, "/$1cost");

        // Unchanged URL: nothing touched.
        let mut h = req("GET", "http://example.com/x");
        let before = h.clone();
        apply(&mut h, &mut b, url_replace("nomatch", "y", false)).unwrap();
        assert_eq!(h, before);

        let mut h = req("GET", "http://example.com/x");
        assert!(matches!(
            apply(&mut h, &mut b, url_replace("http://", "", false)),
            Err(RulesError::Transform(_))
        ));
        let mut h = req("GET", "http://example.com/x");
        assert!(
            apply(&mut h, &mut b, url_replace("http", "gopher", false)).is_err(),
            "unsupported scheme"
        );
    }

    #[test]
    fn url_paths_are_never_normalized() {
        // HTK #939 (`--path-as-is` traversal tests) and #119 (double slashes).
        let mut h = req("GET", "http://a.test/echo/../echo//x/./y");
        let mut b = Bytes::new();
        apply(&mut h, &mut b, url_replace("a.test", "b.test", false)).unwrap();
        assert_eq!(h.path, "/echo/../echo//x/./y");
        assert_eq!(h.url, "http://b.test/echo/../echo//x/./y");
    }

    #[test]
    fn set_query() {
        let mut h = req("GET", "http://x.test/s?a=1&b=2&a=3&keep=%7E");
        let mut b = Bytes::new();
        apply(
            &mut h,
            &mut b,
            RequestTransform {
                set_query: vec![
                    ("a".into(), Some("new value".into())),
                    ("b".into(), None),
                    ("c".into(), Some("&".into())),
                ],
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(h.path, "/s?a=new+value&keep=%7E&c=%26");
        assert_eq!(h.url, "http://x.test/s?a=new+value&keep=%7E&c=%26");
        apply(
            &mut h,
            &mut b,
            RequestTransform {
                set_query: vec![
                    ("a".into(), None),
                    ("keep".into(), None),
                    ("c".into(), None),
                ],
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(h.path, "/s");
    }

    #[test]
    fn body_transforms() {
        let mut h = req("POST", "http://x/");
        let mut b = Bytes::from_static(b"user=alice; mail=alice@example.com; Mail=BOB@EXAMPLE.COM");
        apply(
            &mut h,
            &mut b,
            RequestTransform {
                body: Some(BodyTransform::MatchReplace {
                    replacements: vec![
                        MatchReplace {
                            find: r"(\w+)@example\.com".into(),
                            replace: "${1}@test.dev".into(),
                            regex: true,
                            case_sensitive: false,
                        },
                        MatchReplace {
                            find: "alice".into(),
                            replace: "carol".into(),
                            regex: false,
                            case_sensitive: true,
                        },
                    ],
                }),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            &b[..],
            b"user=carol; mail=carol@test.dev; Mail=BOB@test.dev"
        );

        let mut b = Bytes::from_static(br#"{"a":1,"b":{"c":2,"d":3},"z":true}"#);
        apply(
            &mut h,
            &mut b,
            RequestTransform {
                body: Some(BodyTransform::JsonMerge {
                    patch: json!({"b": {"c": null, "e": [1]}, "a": "x"}),
                }),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(&b[..], br#"{"a":"x","b":{"d":3,"e":[1]},"z":true}"#);

        let mut b = Bytes::from_static(br#"{"list":[1,2],"obj":{"k":"v"}}"#);
        apply(
            &mut h,
            &mut b,
            RequestTransform {
                body: Some(BodyTransform::JsonPatch {
                    operations: vec![
                        json!({"op": "test", "path": "/obj/k", "value": "v"}),
                        json!({"op": "add", "path": "/list/-", "value": 3}),
                        json!({"op": "copy", "from": "/obj/k", "path": "/copied"}),
                        json!({"op": "move", "from": "/obj", "path": "/moved"}),
                        json!({"op": "replace", "path": "/list/0", "value": 0}),
                        json!({"op": "remove", "path": "/list/1"}),
                    ],
                }),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(&b[..], br#"{"list":[0,3],"copied":"v","moved":{"k":"v"}}"#);

        // Failures leave the body untouched.
        let original = Bytes::from_static(br#"{"a":1}"#);
        let mut b = original.clone();
        let failing = RequestTransform {
            body: Some(BodyTransform::JsonPatch {
                operations: vec![
                    json!({"op": "add", "path": "/b", "value": 1}),
                    json!({"op": "test", "path": "/a", "value": 2}),
                ],
            }),
            ..Default::default()
        };
        assert!(matches!(
            apply(&mut h, &mut b, failing),
            Err(RulesError::Transform(_))
        ));
        assert_eq!(b, original);
        let mut b = Bytes::from_static(b"not json");
        let merge = RequestTransform {
            body: Some(BodyTransform::JsonMerge {
                patch: json!({"a": 1}),
            }),
            ..Default::default()
        };
        assert!(apply(&mut h, &mut b, merge.clone()).is_err());
        assert_eq!(&b[..], b"not json");
        // Empty bodies count as null.
        let mut b = Bytes::new();
        apply(&mut h, &mut b, merge).unwrap();
        assert_eq!(&b[..], br#"{"a":1}"#);

        let mut b = Bytes::from_static(b"old");
        apply(
            &mut h,
            &mut b,
            RequestTransform {
                body: Some(BodyTransform::Replace {
                    body: BodySource::Json {
                        value: json!({"x": [1, "y"]}),
                    },
                }),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(&b[..], br#"{"x":[1,"y"]}"#);
        // Content-Length is the caller's job.
        assert!(!h.headers.contains("content-length"));
    }

    #[test]
    fn response_transforms() {
        let mut head = ResponseHead {
            status: 200,
            status_message: "OK".into(),
            http_version: HttpVersion::Http11,
            headers: Headers(vec![
                ("Server".into(), "nginx".into()),
                ("ETag".into(), "x".into()),
            ]),
        };
        let mut body = Bytes::from_static(b"hello");
        apply_response_transform(
            &mut head,
            &mut body,
            &ResponseTransform {
                status: Some(404),
                set_headers: Headers(vec![("Server".into(), "akev".into())]),
                add_headers: Headers(vec![
                    ("Set-Cookie".into(), "a=1".into()),
                    ("Set-Cookie".into(), "b=2".into()),
                ]),
                remove_headers: vec!["etag".into()],
                body: Some(BodyTransform::Replace {
                    body: BodySource::Text {
                        text: "gone".into(),
                    },
                }),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            (head.status, head.status_message.as_str()),
            (404, "Not Found")
        );
        assert_eq!(head.headers.get_all("set-cookie").count(), 2);
        assert_eq!(head.headers.get("server"), Some("akev"));
        assert!(!head.headers.contains("etag"));
        assert_eq!(&body[..], b"gone");
        let custom = ResponseTransform {
            status: Some(418),
            status_message: Some("Custom".into()),
            ..Default::default()
        };
        apply_response_transform(&mut head, &mut body, &custom).unwrap();
        assert_eq!((head.status, head.status_message.as_str()), (418, "Custom"));
        // Same status keeps a custom reason.
        apply_response_transform(
            &mut head,
            &mut body,
            &ResponseTransform {
                status: Some(418),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(head.status_message, "Custom");
    }

    /// json-merge / json-patch used to round-trip the whole body through `serde_json::Value`:
    /// integers beyond 64 bits and long decimals in fields the patch never touched were
    /// rounded, and a body containing `1e400` failed to parse, so the patch was skipped.
    #[test]
    fn json_transforms_keep_untouched_numbers_exact() {
        let mut h = req("POST", "http://x.test/api");
        let merge = |patch: serde_json::Value| RequestTransform {
            body: Some(BodyTransform::JsonMerge { patch }),
            ..Default::default()
        };
        let mut b = Bytes::from_static(
            br#"{"id":12345678901234567890123,"price":19.90,"ratio":0.1000000000000000055511151231257827,"e":1E5,"neg":-0.0,"big":1e400,"n":[-12345678901234567890,1.5e-400]}"#,
        );
        apply(&mut h, &mut b, merge(json!({"debug": true}))).unwrap();
        assert_eq!(
            std::str::from_utf8(&b).unwrap(),
            r#"{"id":12345678901234567890123,"price":19.90,"ratio":0.1000000000000000055511151231257827,"e":1E5,"neg":-0.0,"big":1e400,"n":[-12345678901234567890,1.5e-400],"debug":true}"#
        );
        // Numbers that do get patched are written like before.
        let mut b = Bytes::from_static(br#"{"a":1e400,"b":{"c":2.50}}"#);
        apply(&mut h, &mut b, merge(json!({"a": 1.5, "b": {"d": 7}}))).unwrap();
        assert_eq!(&b[..], br#"{"a":1.5,"b":{"c":2.50,"d":7}}"#);

        let mut b = Bytes::from_static(
            br#"{"ids":[98765432109876543210,1],"meta":{"v":3.14159265358979323846264338327950288}}"#,
        );
        apply(
            &mut h,
            &mut b,
            RequestTransform {
                body: Some(BodyTransform::JsonPatch {
                    operations: vec![
                        json!({"op": "test", "path": "/ids/1", "value": 1.0}),
                        json!({"op": "test", "path": "/ids/0", "value": 98765432109876543210.0}),
                        json!({"op": "copy", "from": "/meta/v", "path": "/pi"}),
                        json!({"op": "move", "from": "/ids/0", "path": "/first"}),
                        json!({"op": "add", "path": "/ids/-", "value": 2}),
                    ],
                }),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            std::str::from_utf8(&b).unwrap(),
            r#"{"ids":[1,2],"meta":{"v":3.14159265358979323846264338327950288},"pi":3.14159265358979323846264338327950288,"first":98765432109876543210}"#
        );
        // A failing `test` still compares by value and leaves the body untouched.
        let original = Bytes::from_static(br#"{"v":1e400}"#);
        let mut b = original.clone();
        let failing = RequestTransform {
            body: Some(BodyTransform::JsonPatch {
                operations: vec![json!({"op": "test", "path": "/v", "value": 1})],
            }),
            ..Default::default()
        };
        let err = apply(&mut h, &mut b, failing).unwrap_err().to_string();
        assert!(err.contains("value at `/v` is 1e400, expected 1"), "{err}");
        assert_eq!(b, original);
        // Responses too.
        let mut head = ResponseHead {
            status: 200,
            status_message: "OK".into(),
            http_version: HttpVersion::Http11,
            headers: Headers::new(),
        };
        let mut b = Bytes::from_static(br#"[18446744073709551616]"#);
        apply_response_transform(
            &mut head,
            &mut b,
            &ResponseTransform {
                body: Some(BodyTransform::JsonPatch {
                    operations: vec![json!({"op": "add", "path": "/0", "value": "x"})],
                }),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(&b[..], br#"["x",18446744073709551616]"#);
    }

    #[test]
    fn need_body() {
        let with_body = RequestTransform {
            body: Some(BodyTransform::JsonMerge { patch: json!({}) }),
            ..Default::default()
        };
        let res_body = ResponseTransform {
            body: Some(BodyTransform::Replace {
                body: BodySource::Empty,
            }),
            ..Default::default()
        };
        assert_eq!(transforms_need_body(&[], &[]), (false, false));
        assert_eq!(
            transforms_need_body(
                &[RequestTransform::default(), with_body],
                &[ResponseTransform::default()]
            ),
            (true, false)
        );
        assert_eq!(transforms_need_body(&[], &[res_body]), (false, true));
    }

    #[test]
    fn resolve_body_sources() {
        assert!(resolve_body(&BodySource::Empty).unwrap().is_empty());
        assert_eq!(
            &resolve_body(&BodySource::Text {
                text: "h\u{e9}".into()
            })
            .unwrap()[..],
            "h\u{e9}".as_bytes()
        );
        assert_eq!(
            &resolve_body(&BodySource::Base64 {
                data: "AP8=".into()
            })
            .unwrap()[..],
            &[0, 255]
        );
        assert!(matches!(
            resolve_body(&BodySource::Base64 { data: "!!".into() }),
            Err(RulesError::Transform(_))
        ));
        assert_eq!(
            &resolve_body(&BodySource::Json {
                value: json!({"a": [1, 2]})
            })
            .unwrap()[..],
            br#"{"a":[1,2]}"#
        );
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("body.txt");
        std::fs::write(&file, "v1").unwrap();
        let src = BodySource::File {
            path: file.to_string_lossy().into_owned(),
        };
        assert_eq!(&resolve_body(&src).unwrap()[..], b"v1");
        std::fs::write(&file, "v2").unwrap();
        assert_eq!(
            &resolve_body(&src).unwrap()[..],
            b"v2",
            "re-read on every call"
        );
        assert!(matches!(
            resolve_body(&BodySource::File {
                path: "/definitely/not/here".into()
            }),
            Err(RulesError::Io(_))
        ));
    }
}

// ==========================================================================================
// Mock / map-local / forward
// ==========================================================================================

mod respond_tests {
    use super::*;

    fn mock_action(status: u16, headers: &[(&str, &str)], body: BodySource) -> Action {
        Action::Mock {
            status,
            status_message: None,
            headers: Headers(
                headers
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            ),
            body,
        }
    }

    #[test]
    fn mock_basics() {
        let r = req("GET", "https://x/");
        let (head, body) = mock_response(
            &mock_action(
                200,
                &[],
                BodySource::Json {
                    value: json!({"ok": true}),
                },
            ),
            &r,
        )
        .unwrap();
        assert_eq!((head.status, head.status_message.as_str()), (200, "OK"));
        assert_eq!(head.headers.get("content-type"), Some("application/json"));
        assert_eq!(head.headers.get("content-length"), Some("11"));
        assert!(head.headers.get("date").is_some_and(|d| d.ends_with("GMT")));
        assert_eq!(&body[..], br#"{"ok":true}"#);
        assert_eq!(head.http_version, HttpVersion::Http11);

        // Explicit content-type kept; stale content-length corrected; repeated headers kept (HTK #358).
        let action = mock_action(
            201,
            &[
                ("Content-Type", "application/vnd.api+json"),
                ("Content-Length", "999"),
                ("Set-Cookie", "a=1"),
                ("Set-Cookie", "b=2"),
            ],
            BodySource::Json { value: json!([]) },
        );
        let (head, _) = mock_response(&action, &r).unwrap();
        assert_eq!(
            head.headers.get("content-type"),
            Some("application/vnd.api+json")
        );
        assert_eq!(
            head.headers.get_all("content-length").collect::<Vec<_>>(),
            vec!["2"]
        );
        assert_eq!(
            head.headers
                .0
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                .unwrap()
                .0,
            "Content-Length"
        );
        assert_eq!(
            head.headers.get_all("set-cookie").collect::<Vec<_>>(),
            vec!["a=1", "b=2"]
        );
        assert_eq!(head.status_message, "Created");

        let custom = Action::Mock {
            status: 299,
            status_message: Some("Whatever".into()),
            headers: Headers::new(),
            body: BodySource::Text { text: "hi".into() },
        };
        let (head, body) = mock_response(&custom, &r).unwrap();
        assert_eq!(
            (head.status_message.as_str(), &body[..]),
            ("Whatever", &b"hi"[..])
        );
        assert!(
            !head.headers.contains("content-type"),
            "no guessing for text"
        );
        let (head, _) = mock_response(&mock_action(799, &[], BodySource::Empty), &r).unwrap();
        assert_eq!(head.status_message, "");
    }

    #[test]
    fn mock_special_cases() {
        let mut head_req = req("HEAD", "http://x/");
        head_req.http_version = HttpVersion::H2;
        let (head, body) = mock_response(
            &mock_action(
                200,
                &[],
                BodySource::Text {
                    text: "12345".into(),
                },
            ),
            &head_req,
        )
        .unwrap();
        assert!(body.is_empty());
        assert_eq!(head.headers.get("content-length"), Some("5"));
        assert_eq!(head.http_version, HttpVersion::H2);

        let get = req("GET", "http://x/");
        let (head, body) = mock_response(
            &mock_action(
                204,
                &[("content-length", "3")],
                BodySource::Text { text: "abc".into() },
            ),
            &get,
        )
        .unwrap();
        assert!(body.is_empty() && !head.headers.contains("content-length"));
        let (head, body) = mock_response(
            &mock_action(304, &[], BodySource::Text { text: "abc".into() }),
            &get,
        )
        .unwrap();
        assert!(body.is_empty() && !head.headers.contains("content-length"));
        let chunked = mock_action(
            200,
            &[("Transfer-Encoding", "chunked"), ("Content-Length", "1")],
            BodySource::Text { text: "abc".into() },
        );
        let (head, body) = mock_response(&chunked, &get).unwrap();
        assert!(!head.headers.contains("content-length"));
        assert_eq!(&body[..], b"abc");
        let (head, _) = mock_response(
            &mock_action(
                200,
                &[("Date", "Mon, 01 Jan 2024 00:00:00 GMT")],
                BodySource::Empty,
            ),
            &get,
        )
        .unwrap();
        assert_eq!(head.headers.get_all("date").count(), 1);
        assert_eq!(head.headers.get("content-length"), Some("0"));

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("data.json");
        std::fs::write(&file, "[1]").unwrap();
        let (head, body) = mock_response(
            &mock_action(
                200,
                &[],
                BodySource::File {
                    path: file.to_string_lossy().into_owned(),
                },
            ),
            &get,
        )
        .unwrap();
        assert_eq!(
            head.headers.get("content-type"),
            Some("application/json; charset=utf-8")
        );
        assert_eq!(&body[..], b"[1]");

        let (head, body) = mock_response(
            &Action::WsReject {
                status: 403,
                body: BodySource::Text { text: "no".into() },
            },
            &get,
        )
        .unwrap();
        assert_eq!(
            (head.status, head.status_message.as_str(), &body[..]),
            (403, "Forbidden", &b"no"[..])
        );
        assert!(mock_response(&passthrough(), &get).is_err());
        assert!(matches!(
            mock_response(
                &mock_action(
                    200,
                    &[],
                    BodySource::File {
                        path: "/no/such/file".into()
                    }
                ),
                &get
            ),
            Err(RulesError::Io(_))
        ));
    }

    fn map_action(path: &std::path::Path, strip: Option<&str>, fallback: bool) -> Action {
        Action::MapLocal {
            path: path.to_string_lossy().into_owned(),
            strip_prefix: strip.map(str::to_string),
            index: None,
            fallback_to_upstream: fallback,
            headers: Headers::new(),
        }
    }

    fn site() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("site");
        std::fs::create_dir_all(root.join("docs/sub")).unwrap();
        std::fs::write(root.join("index.html"), "<h1>home</h1>").unwrap();
        std::fs::write(root.join("app.js"), "console.log(1)").unwrap();
        std::fs::write(root.join("docs/index.html"), "docs").unwrap();
        std::fs::write(root.join("docs/my file.txt"), "spaced").unwrap();
        std::fs::write(root.join("docs/sub/data.bin"), [0u8, 1, 2]).unwrap();
        std::fs::write(dir.path().join("secret.txt"), "TOP SECRET").unwrap();
        dir
    }

    fn found(outcome: MapLocalOutcome) -> (ResponseHead, Bytes) {
        match outcome {
            MapLocalOutcome::Found(h, b) => (h, b),
            MapLocalOutcome::FoundFile { head, .. } => panic!("streamed: {head:?}"),
            MapLocalOutcome::NotFound(h, b) => panic!("not found: {} {:?}", h.status, b),
            MapLocalOutcome::FallbackToUpstream => panic!("fallback"),
        }
    }

    fn not_found(outcome: MapLocalOutcome) -> (ResponseHead, Bytes) {
        match outcome {
            MapLocalOutcome::NotFound(h, b) => (h, b),
            MapLocalOutcome::Found(_, b) => panic!("unexpectedly found: {b:?}"),
            MapLocalOutcome::FoundFile { head, .. } => panic!("unexpectedly found: {head:?}"),
            MapLocalOutcome::FallbackToUpstream => panic!("fallback"),
        }
    }

    #[test]
    fn map_local_streams_large_files() {
        let dir = site();
        let root = dir.path().join("site");
        let len = crate::MAP_LOCAL_STREAM_MIN + 10;
        let data: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
        std::fs::write(root.join("big.bin"), &data).unwrap();
        let action = map_action(&root, None, false);
        match map_local(&action, &req("GET", "http://x/big.bin")).unwrap() {
            MapLocalOutcome::FoundFile {
                head,
                file: Some(mut file),
                len: n,
            } => {
                assert_eq!(head.status, 200);
                assert_eq!(n, len);
                assert_eq!(
                    head.headers.get("content-length"),
                    Some(len.to_string().as_str())
                );
                assert_eq!(
                    head.headers.get("content-type"),
                    Some("application/octet-stream")
                );
                let mut read = Vec::new();
                std::io::Read::read_to_end(&mut file, &mut read).unwrap();
                assert_eq!(read, data, "the open file is the mapped one");
            }
            other => panic!("expected a streamed file, got {other:?}"),
        }
        // HEAD: same head, nothing to stream.
        match map_local(&action, &req("HEAD", "http://x/big.bin")).unwrap() {
            MapLocalOutcome::FoundFile { head, file, .. } => {
                assert!(file.is_none());
                assert_eq!(
                    head.headers.get("content-length"),
                    Some(len.to_string().as_str())
                );
            }
            other => panic!("expected a streamed file, got {other:?}"),
        }
        // Small files are still answered from memory.
        assert_eq!(
            &found(map_local(&action, &req("GET", "http://x/app.js")).unwrap()).1[..],
            b"console.log(1)"
        );
    }

    #[test]
    fn map_local_directory() {
        let dir = site();
        let root = dir.path().join("site");
        let action = map_action(&root, None, false);
        let (head, body) = found(map_local(&action, &req("GET", "http://x/app.js?v=3")).unwrap());
        assert_eq!(head.status, 200);
        assert!(
            head.headers
                .get("content-type")
                .is_some_and(|ct| ct.ends_with("javascript; charset=utf-8"))
        );
        assert!(
            head.headers
                .get("last-modified")
                .is_some_and(|d| d.ends_with("GMT"))
        );
        assert_eq!(head.headers.get("content-length"), Some("14"));
        assert_eq!(&body[..], b"console.log(1)");
        assert_eq!(
            &found(map_local(&action, &req("GET", "http://x/")).unwrap()).1[..],
            b"<h1>home</h1>"
        );
        assert_eq!(
            &found(map_local(&action, &req("GET", "http://x/docs")).unwrap()).1[..],
            b"docs",
            "directory → index"
        );
        assert_eq!(
            &found(map_local(&action, &req("GET", "http://x/docs/")).unwrap()).1[..],
            b"docs"
        );
        assert_eq!(
            &found(map_local(&action, &req("GET", "http://x/docs/my%20file.txt")).unwrap()).1[..],
            b"spaced"
        );
        assert_eq!(
            &found(map_local(&action, &req("GET", "http://x/docs//sub/./data.bin")).unwrap()).1[..],
            &[0, 1, 2]
        );
        let (head, body) = found(map_local(&action, &req("HEAD", "http://x/app.js")).unwrap());
        assert!(body.is_empty());
        assert_eq!(head.headers.get("content-length"), Some("14"));

        // Strip prefix + custom index + header overrides.
        let action = Action::MapLocal {
            path: root.to_string_lossy().into_owned(),
            strip_prefix: Some("/static".into()),
            index: Some("app.js".into()),
            fallback_to_upstream: false,
            headers: Headers(vec![
                ("Cache-Control".into(), "no-cache".into()),
                ("content-type".into(), "text/plain".into()),
            ]),
        };
        let (head, body) = found(map_local(&action, &req("GET", "http://x/static/")).unwrap());
        assert_eq!(&body[..], b"console.log(1)");
        assert_eq!(head.headers.get("cache-control"), Some("no-cache"));
        assert_eq!(
            head.headers.get_all("content-type").collect::<Vec<_>>(),
            vec!["text/plain"]
        );
        assert_eq!(
            &found(map_local(&action, &req("GET", "http://x/static/docs/index.html")).unwrap()).1[..],
            b"docs"
        );
    }

    /// The action's headers used to be merged one by one with "replace all values", so a
    /// header listed several times (two Set-Cookie lines) kept only its last value.
    #[test]
    fn map_local_keeps_repeated_action_headers() {
        let dir = site();
        let root = dir.path().join("site");
        let action = Action::MapLocal {
            path: root.to_string_lossy().into_owned(),
            strip_prefix: None,
            index: None,
            fallback_to_upstream: false,
            headers: Headers(vec![
                ("Set-Cookie".into(), "a=1".into()),
                ("X-Custom".into(), "x".into()),
                ("set-cookie".into(), "b=2".into()),
                ("Content-Type".into(), "text/x-one".into()),
                ("Link".into(), "</a.css>; rel=preload".into()),
                ("Link".into(), "</b.js>; rel=preload".into()),
            ]),
        };
        let (head, _) = found(map_local(&action, &req("GET", "http://x/app.js")).unwrap());
        assert_eq!(
            head.headers.get_all("set-cookie").collect::<Vec<_>>(),
            vec!["a=1", "b=2"]
        );
        assert_eq!(
            head.headers.get_all("link").collect::<Vec<_>>(),
            vec!["</a.css>; rel=preload", "</b.js>; rel=preload"]
        );
        assert_eq!(head.headers.get("x-custom"), Some("x"));
        // Generated headers are still replaced (once).
        assert_eq!(
            head.headers.get_all("content-type").collect::<Vec<_>>(),
            vec!["text/x-one"]
        );
        assert_eq!(head.headers.get("content-length"), Some("14"));
        // Same behaviour as a mock with these headers.
        let mock = Action::Mock {
            status: 200,
            status_message: None,
            headers: Headers(vec![
                ("Set-Cookie".into(), "a=1".into()),
                ("set-cookie".into(), "b=2".into()),
            ]),
            body: BodySource::Empty,
        };
        let (mock_head, _) = mock_response(&mock, &req("GET", "http://x/")).unwrap();
        assert_eq!(
            mock_head.headers.get_all("set-cookie").collect::<Vec<_>>(),
            head.headers.get_all("set-cookie").collect::<Vec<_>>()
        );
    }

    #[test]
    fn map_local_single_file_and_misses() {
        let dir = site();
        let root = dir.path().join("site");
        let file_action = map_action(&root.join("app.js"), None, false);
        assert_eq!(
            &found(map_local(&file_action, &req("GET", "http://x/anything/at/all")).unwrap()).1[..],
            b"console.log(1)"
        );

        let action = map_action(&root, None, false);
        let (head, body) =
            not_found(map_local(&action, &req("GET", "http://x/missing.css")).unwrap());
        assert_eq!(head.status, 404);
        assert_eq!(
            head.headers.get("content-type"),
            Some("text/plain; charset=utf-8")
        );
        assert!(String::from_utf8_lossy(&body).contains("/missing.css"));
        assert!(
            !String::from_utf8_lossy(&body).contains(root.to_string_lossy().as_ref()),
            "no local path leak"
        );
        let fallback = map_action(&root, None, true);
        assert!(matches!(
            map_local(&fallback, &req("GET", "http://x/missing.css")).unwrap(),
            MapLocalOutcome::FallbackToUpstream
        ));
        let missing_root = map_action(&dir.path().join("nope"), None, true);
        assert!(matches!(
            map_local(&missing_root, &req("GET", "http://x/")).unwrap(),
            MapLocalOutcome::FallbackToUpstream
        ));
        assert!(map_local(&passthrough(), &req("GET", "http://x/")).is_err());
    }

    #[test]
    fn map_local_blocks_traversal() {
        let dir = site();
        let root = dir.path().join("site");
        for fallback in [false, true] {
            let action = map_action(&root, None, fallback);
            for path in [
                "/../secret.txt",
                "/docs/../../secret.txt",
                "/%2e%2e/secret.txt",
                "/%2E%2E%2Fsecret.txt",
                "/docs/%2F..%2F..%2Fsecret.txt",
                "/..%5Csecret.txt",
                "/docs/..%5C..%5Csecret.txt",
                "/%00/secret.txt",
                "/docs/%ff%fe",
            ] {
                let mut r = req("GET", "http://x/");
                r.path = path.to_string();
                let (head, body) = not_found(map_local(&action, &r).unwrap());
                assert_eq!(head.status, 404, "{path}");
                assert!(
                    !String::from_utf8_lossy(&body).contains("TOP SECRET"),
                    "{path}"
                );
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn map_local_blocks_symlink_escape() {
        let dir = site();
        let root = dir.path().join("site");
        std::os::unix::fs::symlink(dir.path().join("secret.txt"), root.join("link.txt")).unwrap();
        std::os::unix::fs::symlink(dir.path(), root.join("up")).unwrap();
        std::fs::write(root.join("docs/inner.txt"), "inner").unwrap();
        std::os::unix::fs::symlink(root.join("docs/inner.txt"), root.join("ok-link.txt")).unwrap();
        let action = map_action(&root, None, true);
        for path in ["/link.txt", "/up/secret.txt"] {
            let (head, body) =
                not_found(map_local(&action, &req("GET", &format!("http://x{path}"))).unwrap());
            assert_eq!(head.status, 404);
            assert!(!String::from_utf8_lossy(&body).contains("TOP SECRET"));
        }
        assert_eq!(
            &found(map_local(&action, &req("GET", "http://x/ok-link.txt")).unwrap()).1[..],
            b"inner",
            "links inside the root work"
        );
    }

    fn fwd(target: &str, strip: Option<&str>, preserve_host: bool) -> Action {
        Action::Forward {
            target: target.into(),
            strip_prefix: strip.map(str::to_string),
            preserve_host,
            request: None,
            response: None,
        }
    }

    #[test]
    fn forward_rewrites() {
        let r = with_headers(
            req("POST", "http://app.example.com/api/users?page=2"),
            &[("Content-Type", "application/json")],
        );
        let out =
            forward_request(&fwd("https://staging.example.com:8443", None, false), &r).unwrap();
        assert_eq!(out.url, "https://staging.example.com:8443/api/users?page=2");
        assert_eq!(
            (
                out.scheme.as_str(),
                out.host.as_str(),
                out.port,
                out.path.as_str()
            ),
            ("https", "staging.example.com", 8443, "/api/users?page=2")
        );
        assert_eq!(out.headers.get("host"), Some("staging.example.com:8443"));
        assert_eq!(out.headers.get("content-type"), Some("application/json"));
        assert_eq!(out.method, "POST");

        let out =
            forward_request(&fwd("http://localhost:3000/v2/", Some("/api"), false), &r).unwrap();
        assert_eq!(out.url, "http://localhost:3000/v2/users?page=2");
        let out =
            forward_request(&fwd("http://localhost:3000/v2", Some("/apix"), false), &r).unwrap();
        assert_eq!(
            out.path, "/v2/api/users?page=2",
            "non-matching prefix: target path is prepended"
        );
        let out =
            forward_request(&fwd("http://localhost:3000", Some("/api/users"), false), &r).unwrap();
        assert_eq!(out.path, "/?page=2");

        let out = forward_request(&fwd("https://other.test", None, true), &r).unwrap();
        assert_eq!(
            out.headers.get("host"),
            Some("app.example.com"),
            "preserve_host"
        );
        assert_eq!(
            (out.port, out.url.as_str()),
            (443, "https://other.test/api/users?page=2")
        );

        let out = forward_request(&fwd("localhost:9000", None, false), &r).unwrap();
        assert_eq!(
            (out.scheme.as_str(), out.port),
            ("http", 9000),
            "scheme-less target keeps the scheme"
        );
        let out = forward_request(&fwd("[::1]:8080", None, false), &r).unwrap();
        assert_eq!(
            (out.host.as_str(), out.headers.get("host")),
            ("::1", Some("[::1]:8080"))
        );
        let out = forward_request(&fwd("http://t.test/base?x=1", None, false), &r).unwrap();
        assert_eq!(out.path, "/base/api/users?x=1&page=2");
        assert!(forward_request(&fwd("ftp://bad", None, false), &r).is_err());
        assert!(forward_request(&mock(200), &r).is_err());
    }

    #[test]
    fn forward_preserves_raw_paths_and_websockets() {
        let r = req("GET", "http://a.test/echo/../echo//x%2Fy");
        let out = forward_request(&fwd("http://b.test:81", None, false), &r).unwrap();
        assert_eq!(out.path, "/echo/../echo//x%2Fy");
        let ws = with_headers(
            req("GET", "https://a.test/socket"),
            &[("Upgrade", "websocket")],
        );
        let out = forward_request(&fwd("https://b.test", None, false), &ws).unwrap();
        assert_eq!(
            (out.scheme.as_str(), out.url.as_str()),
            ("wss", "wss://b.test/socket")
        );
        let mut h2 = req("GET", "https://a.test/x");
        h2.http_version = HttpVersion::H2;
        h2.headers = Headers(vec![(":authority".into(), "a.test".into())]);
        let out = forward_request(&fwd("https://b.test", None, false), &h2).unwrap();
        assert_eq!(
            out.headers.0,
            vec![(":authority".to_string(), "b.test".to_string())],
            "HTTP/2 gets no Host header"
        );
    }
}

#[test]
fn locked_action_types_are_skipped_reported_and_kept() {
    use helpers::*;
    let rules = set(vec![
        rule("mock", vec![], mock(201)),
        rule(
            "highlight",
            vec![],
            Action::Highlight {
                color: "red".into(),
                note: None,
            },
        ),
    ]);
    let mut locked = LockedActions::new();
    locked.insert("mock", "needs Pro".to_string());
    let e = RulesEngine::with_locked_actions(rules.clone(), locked.clone());
    assert_eq!(e.errors(), vec![("mock".into(), "needs Pro".to_string())]);
    assert_eq!(e.rules(), rules, "the rule set is kept as given");
    let r = req("GET", "http://x.test/");
    assert!(
        decide(&e, &r, None)
            .matched
            .iter()
            .all(|id| id.as_str() != "mock")
    );

    // Unlocking recompiles the current rules.
    e.set_locked_actions(LockedActions::new());
    assert!(e.errors().is_empty());
    assert!(
        decide(&e, &r, None)
            .matched
            .iter()
            .any(|id| id.as_str() == "mock")
    );

    // Disabled rules aren't reported.
    let mut disabled = rules.clone();
    if let RuleItem::Rule(r) = &mut disabled.items[0] {
        r.enabled = false;
    }
    e.set_rules(disabled);
    e.set_locked_actions(locked);
    assert!(e.errors().is_empty());
}

mod describe_tests;
mod htk_tests;
mod script_tests;
