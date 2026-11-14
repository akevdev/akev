//! Crate-level tests: parse trees, matching semantics against fake targets, error messages,
//! lazy body evaluation, suggestions, docs, fuzzing and a benchmark.

use crate::{Filter, FilterTarget, ParseError, SuggestContext, Suggestion, field_docs, suggest};
use akev_core::{
    BreakpointStage, ContentKind, ErrorKind, ExchangeId, ExchangeKind, ExchangeState,
    ExchangeSummary, Headers, HttpVersion,
};
use std::borrow::Cow;
use std::cell::Cell;

// ---------------------------------------------------------------------------------------------
// Fake target
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Fake {
    name: &'static str,
    s: ExchangeSummary,
    req_headers: Headers,
    res_headers: Option<Headers>,
    req_body: Option<String>,
    res_body: Option<String>,
    ip: Option<String>,
    note: Option<String>,
    rule_title: Option<String>,
    req_body_calls: Cell<usize>,
    res_body_calls: Cell<usize>,
    header_calls: Cell<usize>,
}

impl FilterTarget for Fake {
    fn summary(&self) -> &ExchangeSummary {
        &self.s
    }
    fn request_headers(&self) -> &Headers {
        self.header_calls.set(self.header_calls.get() + 1);
        &self.req_headers
    }
    fn response_headers(&self) -> Option<&Headers> {
        self.header_calls.set(self.header_calls.get() + 1);
        self.res_headers.as_ref()
    }
    fn request_body_text(&self) -> Option<Cow<'_, str>> {
        self.req_body_calls.set(self.req_body_calls.get() + 1);
        self.req_body.as_deref().map(Cow::Borrowed)
    }
    fn response_body_text(&self) -> Option<Cow<'_, str>> {
        self.res_body_calls.set(self.res_body_calls.get() + 1);
        self.res_body.as_deref().map(Cow::Borrowed)
    }
    fn client_ip(&self) -> Option<&str> {
        self.ip.as_deref()
    }
    fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }
    fn rule_title(&self) -> Option<&str> {
        self.rule_title.as_deref()
    }
}

fn headers(pairs: &[(&str, &str)]) -> Headers {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

fn base(
    name: &'static str,
    id: u64,
    method: &str,
    scheme: &str,
    host: &str,
    port: u16,
    path: &str,
) -> Fake {
    Fake {
        name,
        s: ExchangeSummary {
            id: ExchangeId(id),
            kind: ExchangeKind::Http,
            state: ExchangeState::Complete,
            started_at: 0.0,
            method: method.to_string(),
            scheme: scheme.to_string(),
            host: host.to_string(),
            port,
            path: path.to_string(),
            http_version: HttpVersion::Http11,
            status: None,
            content_kind: ContentKind::Unknown,
            content_type: None,
            request_size: 0,
            response_size: 0,
            duration: None,
            error: None,
            paused: None,
            source: None,
            rule: None,
            modified: false,
            mocked: false,
            pinned: false,
            color: None,
            message_count: 0,
            has_note: false,
            llm: None,
        },
        req_headers: Headers::new(),
        res_headers: None,
        req_body: None,
        res_body: None,
        ip: None,
        note: None,
        rule_title: None,
        req_body_calls: Cell::new(0),
        res_body_calls: Cell::new(0),
        header_calls: Cell::new(0),
    }
}

/// Ten varied exchanges named a..j.
fn fixtures() -> Vec<Fake> {
    let mut a = base(
        "a",
        1,
        "GET",
        "https",
        "api.example.com",
        443,
        "/v1/users?page=2&q=hello%20world",
    );
    a.s.http_version = HttpVersion::H2;
    a.s.status = Some(200);
    a.s.content_kind = ContentKind::Json;
    a.s.content_type = Some("application/json".into());
    a.s.response_size = 2048;
    a.s.duration = Some(120.0);
    a.s.source = Some("Chrome".into());
    a.req_headers = headers(&[
        ("Host", "api.example.com"),
        ("User-Agent", "Mozilla/5.0 Chrome/120"),
        ("Accept", "application/json"),
        ("Cookie", "session=abc123; theme=dark"),
        ("Authorization", "Bearer tok"),
    ]);
    a.res_headers = Some(headers(&[
        ("Content-Type", "application/json; charset=utf-8"),
        ("X-Cache", "HIT"),
        ("Set-Cookie", "sid=zzz; Path=/; HttpOnly"),
    ]));
    a.res_body = Some(r#"{"users":[{"id":1,"name":"Ada"}]}"#.into());
    a.ip = Some("127.0.0.1".into());

    let mut b = base("b", 2, "POST", "http", "localhost", 3000, "/api/login");
    b.s.status = Some(401);
    b.s.content_kind = ContentKind::Json;
    b.s.content_type = Some("application/json".into());
    b.s.request_size = 38;
    b.s.response_size = 26;
    b.s.duration = Some(1500.0);
    b.s.source = Some("curl".into());
    b.s.mocked = true;
    b.s.rule = Some("r-1".into());
    b.s.has_note = true;
    b.s.color = Some("red".into());
    b.s.pinned = true;
    b.rule_title = Some("Mock login".into());
    b.note = Some("check this later".into());
    b.req_headers = headers(&[
        ("Content-Type", "application/json"),
        ("User-Agent", "curl/8.4.0"),
    ]);
    b.res_headers = Some(headers(&[("Content-Type", "application/json")]));
    b.req_body = Some(r#"{"user":"bob","password":"hunter2"}"#.into());
    b.res_body = Some(r#"{"error":"Unauthorized"}"#.into());
    b.ip = Some("192.168.1.20".into());

    let mut c = base(
        "c",
        3,
        "GET",
        "https",
        "cdn.example.org",
        443,
        "/static/app.js",
    );
    c.s.http_version = HttpVersion::H2;
    c.s.status = Some(200);
    c.s.content_kind = ContentKind::Javascript;
    c.s.content_type = Some("application/javascript".into());
    c.s.response_size = 1_572_864;
    c.s.duration = Some(45.0);
    c.s.source = Some("Firefox".into());
    c.req_headers = headers(&[("Accept", "*/*")]);
    c.res_headers = Some(headers(&[
        ("Content-Type", "application/javascript"),
        ("Cache-Control", "public, max-age=31536000"),
    ]));

    let mut d = base(
        "d",
        4,
        "DELETE",
        "https",
        "api.example.com",
        8443,
        "/v1/users/7",
    );
    d.s.state = ExchangeState::Waiting;
    d.s.modified = true;
    d.req_headers = headers(&[("Authorization", "Basic Zm9vOmJhcg==")]);

    let mut e = base("e", 5, "GET", "https", "bad.example.net", 443, "/");
    e.s.state = ExchangeState::Aborted;
    e.s.error = Some(ErrorKind::Timeout);
    e.s.duration = Some(30_000.0);

    let mut f = base("f", 6, "", "https", "pinned.example.com", 443, "");
    f.s.kind = ExchangeKind::TlsFailure;
    f.s.state = ExchangeState::Aborted;
    f.s.error = Some(ErrorKind::ClientTlsRejected);

    let mut g = base("g", 7, "GET", "wss", "ws.example.com", 443, "/socket");
    g.s.kind = ExchangeKind::WebSocket;
    g.s.state = ExchangeState::Streaming;
    g.s.status = Some(101);
    g.s.message_count = 42;
    g.req_headers = headers(&[("Upgrade", "websocket"), ("Connection", "Upgrade")]);
    g.res_headers = Some(headers(&[("Upgrade", "websocket")]));

    let mut h = base("h", 8, "GET", "http", "::1", 8080, "/health?verbose");
    h.s.status = Some(503);
    h.s.content_kind = ContentKind::Text;
    h.s.content_type = Some("text/plain".into());
    h.s.duration = Some(5000.0);
    h.s.source = Some("python3".into());
    h.ip = Some("::1".into());
    h.res_headers = Some(headers(&[
        ("Server", "nginx/1.25"),
        ("Content-Type", "text/plain"),
    ]));
    h.res_body = Some("Service Unavailable".into());

    let mut i = base(
        "i",
        9,
        "PUT",
        "https",
        "upload.example.com",
        443,
        "/files/report.pdf",
    );
    i.s.http_version = HttpVersion::H3;
    i.s.status = Some(201);
    i.s.request_size = 10 * 1024 * 1024;
    i.s.duration = Some(900.0);
    i.s.color = Some("grey".into());
    i.req_headers = headers(&[("Content-Type", "application/pdf")]);

    let mut j = base("j", 10, "GET", "https", "www.über.de", 443, "/straße");
    j.s.http_version = HttpVersion::H2;
    j.s.status = Some(404);
    j.s.content_kind = ContentKind::Html;
    j.s.content_type = Some("text/html".into());
    j.s.response_size = 512;
    j.s.duration = Some(80.0);
    j.req_headers = headers(&[("X-Trace-Id", "abc-123")]);
    j.res_headers = Some(headers(&[("Content-Type", "text/html; charset=utf-8")]));
    j.res_body = Some("<html>Nicht gefunden</html>".into());

    vec![a, b, c, d, e, f, g, h, i, j]
}

fn matching(query: &str, fx: &[Fake]) -> String {
    let filter =
        Filter::parse(query).unwrap_or_else(|e| panic!("{query:?} failed to parse: {e:?}"));
    fx.iter()
        .filter(|t| filter.matches(*t))
        .map(|t| t.name)
        .collect::<Vec<_>>()
        .join(" ")
}

fn tree(query: &str) -> String {
    crate::parse::parse(query)
        .unwrap_or_else(|e| panic!("{query:?} failed to parse: {e:?}"))
        .explain()
}

fn optimized(query: &str) -> String {
    Filter::parse(query)
        .unwrap_or_else(|e| panic!("{query:?} failed to parse: {e:?}"))
        .explain()
}

fn perr(query: &str) -> ParseError {
    match Filter::parse(query) {
        Ok(f) => panic!("{query:?} unexpectedly parsed: {}", f.explain()),
        Err(e) => e,
    }
}

// ---------------------------------------------------------------------------------------------
// Matching semantics
// ---------------------------------------------------------------------------------------------

#[test]
fn semantics_table() {
    let fx = fixtures();
    let all = "a b c d e f g h i j";
    let cases: &[(&str, &str)] = &[
        // Empty / match-all.
        ("", all),
        ("   ", all),
        ("\"\"", all),
        // Free text: URL (default port omitted), method, status code and reason.
        ("example", "a c d e f g i"),
        ("EXAMPLE.COM", "a d f g i"),
        ("localhost:3000", "b"),
        ("localhost:3000/api", "b"),
        ("http://localhost", "b"),
        ("https://api.example.com/v1", "a"),
        ("api.example.com:8443", "d"),
        ("api.example.com:443", ""),
        ("/v1/users", "a d"),
        ("post", "b"),
        ("404", "j"),
        ("\"not found\"", "j"),
        ("unauthorized", "b"),
        ("api*users", "a d"),
        ("/user[s]/", "a d"),
        ("/USERS/i", "a d"),
        ("/USERS/", "d"),
        ("[::1]", "h"),
        ("über", "j"),
        ("ÜBER", "j"),
        ("STRASSE", ""),
        ("'hello%20world'", "a"),
        ("foo[0]", ""),
        // Method.
        ("method:get", "a c e g h j"),
        ("method:GET,post", "a b c e g h j"),
        ("METHOD:Get", "a c e g h j"),
        ("-method:get", "b d f i"),
        ("method!=get,post", "d f i"),
        ("method:p*", "b i"),
        ("method^=p", "b i"),
        ("method$=e", "d"),
        ("method:pos", ""),
        ("method*=e", "a c d e g h j"),
        ("method~/^(get|put)$/", ""),
        ("method~/^(get|put)$/i", "a c e g h i j"),
        ("method~^p", "b i"),
        ("verb=delete", "d"),
        // Status: codes, classes, ranges, comparisons.
        ("status:200", "a c"),
        ("status=200", "a c"),
        ("status:2xx", "a c i"),
        ("status:2XX", "a c i"),
        ("status:20x", "a c i"),
        ("status:4xx", "b j"),
        ("status:400-499", "b j"),
        ("status:499-400", "b j"),
        ("status:4xx-5xx", "b h j"),
        ("status:>=500", "h"),
        ("status>=500", "h"),
        ("status:<200", "g"),
        ("status<200", "g"),
        ("status:>404", "h"),
        ("status:>4xx", "h"),
        ("status:<=4xx", "a b c g i j"),
        ("status:<4xx", "a c g i"),
        ("status!=200", "b d e f g h i j"),
        ("-status:200", "b d e f g h i j"),
        ("status:200,404", "a c j"),
        ("status:1xx,5xx", "g h"),
        ("status:<200,>=500", "g h"),
        ("status:\"404\"", "j"),
        ("code:404", "j"),
        ("has:response", "a b c g h i j"),
        ("-has:response", "d e f"),
        // Host.
        ("host:example.com", "a d f g i"),
        ("host:*.example.com", "a d f g i"),
        ("host=api.example.com", "a d"),
        ("host:example.*", ""),
        ("host:*example*", "a c d e f g i"),
        ("domain:localhost", "b"),
        ("hostname=localhost", "b"),
        ("host:::1", "h"),
        ("host:über.de", "j"),
        ("host:ÜBER.DE", "j"),
        ("-host:example", "b h j"),
        ("host^=api.", "a d"),
        ("host$=.org", "c"),
        ("host:\"*.example.com\"", ""),
        // A registrable domain and its subdomains (the traffic sidebar's domain clause).
        ("(host=example.com | host:*.example.com)", "a d f g i"),
        ("(host=example.org | host:*.example.org)", "c"),
        // Path (without query), URL.
        ("path:/api", "b"),
        ("path:/v1/*", "a d"),
        ("path:*.js", "c"),
        ("path:*.JS", "c"),
        ("path=/v1/users", "a"),
        ("path:page", ""),
        ("path~^/v[12]/", "a d"),
        ("path:/straße", "j"),
        ("path:/api/", "b"),
        ("url:example.com/v1", "a"),
        ("url^=http://", "b h"),
        ("url:/v\\d+\\/users/", "a d"),
        ("url:localhost:3000", "b"),
        ("url:\"[::1]:8080\"", "h"),
        ("url$=report.pdf", "i"),
        // Query.
        ("query:page=2", "a"),
        ("query.page:2", "a"),
        ("query.q:\"hello world\"", "a"),
        ("query.Q:hello", "a"),
        ("query.page=3", ""),
        ("query[verbose]", "h"),
        ("query.verbose=\"\"", "h"),
        ("has:query", "a h"),
        ("query=\"\"", "b c d e f g i j"),
        ("-has:query", "b c d e f g i j"),
        // Content type.
        ("type:json", "a b"),
        ("type=json", "a b"),
        ("type:JSON", "a b"),
        ("type:image,font", ""),
        ("type:js", "c"),
        ("content-type:javascript", "c"),
        ("type:application/javascript", "c"),
        ("type:text/*", "h j"),
        ("type:html", "j"),
        ("category=html", "j"),
        ("-type:json", "c d e f g h i j"),
        ("type!=json", "c d e f g h i j"),
        ("type!=js,html", "a b d e f g h i"),
        ("version!=h1", "a c i j"),
        ("type:data", "a b"),
        ("type:unknown", "d e f g i"),
        ("type:text", "h"),
        ("type^=application/", "a b c"),
        ("type~/^text\\//", "h j"),
        // Sizes with units.
        ("size:>1mb", "c"),
        ("size:>1kb", "a c"),
        ("size:0", "d e f g h i"),
        ("size:<1kb", "b d e f g h i j"),
        ("size:1kb-2kb", "a"),
        ("size:1-2kb", "a"),
        ("size:>=1.5mb", "c"),
        ("size:<1kb,>1mb", "b c d e f g h i j"),
        ("size>2047", "a c"),
        ("size:2048b", "a"),
        ("size:2KB", "a"),
        ("reqsize:>1mb", "i"),
        ("reqsize:>0", "b i"),
        ("totalsize:>2000", "a c i"),
        ("bodysize:>2000", "a c i"),
        ("bodySize>2000", "a c i"),
        // Durations with units.
        ("duration:>1s", "b e h"),
        ("duration:<100ms", "c j"),
        ("time:100-200", "a"),
        ("duration:>=5s", "e h"),
        ("duration:>=0.5m", "e"),
        ("duration:>0.5m", ""),
        ("-duration:>1s", "a c d f g i j"),
        ("duration:1.5s", "b"),
        ("latency:<1s", "a c i j"),
        // Port, scheme, version.
        ("port:443", "a c e f g i j"),
        ("port:>1024", "b d h"),
        ("-port:443", "b d h"),
        ("port:3000,8080", "b h"),
        ("scheme:http", "b h"),
        ("scheme:ws,wss", "g"),
        ("protocol:https", "a c d e f i j"),
        ("is:http", "b h"),
        ("is:https", "a c d e f g i j"),
        ("is:tls", "a c d e f g i j"),
        ("version:h2", "a c j"),
        ("version:h1", "b d e f g h"),
        ("version:1.1", "b d e f g h"),
        ("version:h3", "i"),
        ("-version:h1", "a c i j"),
        ("httpversion=2", "a c j"),
        ("httpVersion=2", "a c j"),
        ("version:http/1.1", "b d e f g h"),
        ("version:\"HTTP/2\"", "a c j"),
        ("version^=http/1", "b d e f g h"),
        // Headers.
        ("header:x-cache", "a"),
        ("header:X-CACHE", "a"),
        ("header:x-*", "a j"),
        ("header~^x-", "a j"),
        ("reqheader:authorization", "a d"),
        ("has:auth", "a d"),
        ("resheader:set-cookie", "a"),
        ("reqheader:set-cookie", ""),
        ("header.content-type:json", "a b"),
        ("reqheader.content-type:json", "b"),
        ("resheader.content-type:html", "j"),
        ("header[x-cache]=hit", "a"),
        ("header[x-cache]", "a"),
        ("header[X-Trace-Id]:abc", "j"),
        ("header.server:nginx*", "h"),
        ("-header:authorization", "b c e f g h i j"),
        ("header.user-agent~/chrome/i", "a"),
        ("header.user-agent~/chrome/", ""),
        ("header.user-agent~chrome", "a"),
        ("header.cache-control:max-age=*", ""),
        ("header.cache-control:*max-age=*", "c"),
        ("header.x-cache!=hit", "b c d e f g h i j"),
        ("headers:\"x-cache: hit\"", "a"),
        ("headers:gzip", ""),
        ("headers=hit", "a"),
        ("headers^=nginx", "h"),
        ("headers=\"server: nginx/1.25\"", "h"),
        ("headers:nginx", "h"),
        ("headers~/^upgrade:\\swebsocket$/i", "g"),
        ("resbody:<html", "j"),
        ("body:>Nicht", "j"),
        ("url:<script>", ""),
        // Cookies.
        ("cookie:session", "a"),
        ("cookie:sid", "a"),
        ("cookie:sess", ""),
        ("cookie:sess*", "a"),
        ("cookie.session:abc*", "a"),
        ("cookie.theme=dark", "a"),
        ("cookie[sid]", "a"),
        ("has:cookie", "a"),
        ("cookie:nope", ""),
        // Bodies.
        ("body:ada", "a"),
        ("body:hunter2", "b"),
        ("reqbody:hunter2", "b"),
        ("resbody:hunter2", ""),
        ("body:/\"id\":\\s*\\d+/", "a"),
        ("body:unauthorized", "b"),
        ("-body:ada", "b c d e f g h i j"),
        ("body:\"service unavailable\"", "h"),
        ("body:GEFUNDEN", "j"),
        ("resbody~/^<html>/", "j"),
        ("resbody^=<html>", "j"),
        ("resbody$=\"}\"", "a b"),
        ("body=\"Service Unavailable\"", "h"),
        ("body:\"\\\"password\\\"\"", "b"),
        ("contains:hunter2", "b"),
        ("contains:hit", "a"),
        ("contains:\"not found\"", "j"),
        ("any:bob", "b"),
        ("contains:delete", "d"),
        // Flags.
        ("is:pending", "d g"),
        ("is:complete", "a b c h i j"),
        ("is:error", "e f"),
        ("is:failed", "e f"),
        ("has:error", "e f"),
        ("is:aborted", "e f"),
        ("is:pinned", "b"),
        ("is:mocked", "b"),
        ("is:modified", "d"),
        ("is:websocket", "g"),
        ("is:ws", "g"),
        ("IS:WS", "g"),
        ("is:streaming", "g"),
        ("is:tls-failure", "f"),
        ("is:highlighted", "b i"),
        ("has:color", "b i"),
        ("is:mutative", "b d i"),
        ("has:note", "b"),
        ("is:note", "b"),
        ("has:body", "a b c i j"),
        ("has:reqbody", "b i"),
        ("has:resbody", "a b c j"),
        ("has:rule", "b"),
        ("has:messages", "g"),
        ("is:paused", ""),
        ("is:sse", ""),
        ("is:tunnel", ""),
        ("is:malformed", ""),
        ("is:error,pinned", "b e f"),
        ("is!=pending", "a b c e f h i j"),
        ("is=pinned", "b"),
        ("is:\"pinned\"", "b"),
        // Rule, note, source, colour, error, ip, id, messages.
        ("rule:mock", "b"),
        ("rule:r-1", "b"),
        ("rule:\"Mock login\"", "b"),
        ("note:later", "b"),
        ("-note:later", "a c d e f g h i j"),
        ("source:chrome", "a"),
        ("source:fire*", "c"),
        ("source:curl,python", "b h"),
        ("app:python", "h"),
        // Any source / none (the sidebar's "Unknown app").
        ("source:*", "a b c h"),
        ("-source:*", "d e f g i j"),
        ("color:red", "b"),
        ("color:gray", "i"),
        ("colour:GREY", "i"),
        ("color:red,grey", "b i"),
        ("color:re", ""),
        ("error:timeout", "e"),
        ("error:tls", "f"),
        ("error:dns", ""),
        ("error=client-tls-rejected", "f"),
        ("ip:127.0.0.1", "a"),
        ("ip:192.168.*", "b"),
        ("ip:::1", "h"),
        ("id:3", "c"),
        ("id:>8", "i j"),
        ("id:2-4", "b c d"),
        ("messages:>10", "g"),
        ("messages:0", "a b c d e f h i j"),
        // Boolean structure, precedence, parentheses.
        ("status:4xx OR status:5xx", "b h j"),
        ("status:4xx or status:5xx", "b h j"),
        ("status:4xx | is:error", "b e f j"),
        ("status:4xx || is:error", "b e f j"),
        ("method:get status:200", "a c"),
        ("method:get AND status:200", "a c"),
        ("method:get and status:200", "a c"),
        ("method:get && status:200", "a c"),
        ("(method:post OR method:put) status:2xx", "i"),
        ("method:post OR method:put status:2xx", "b i"),
        ("method:put status:2xx OR method:post", "b i"),
        ("-(status:2xx OR is:pending)", "b e f h j"),
        ("NOT is:pending", "a b c e f h i j"),
        ("not(is:pending)", "a b c e f h i j"),
        ("!is:pending", "a b c e f h i j"),
        ("- is:pending", "a b c e f h i j"),
        ("--is:pinned", "b"),
        ("NOT NOT is:pinned", "b"),
        ("is:pinned or is:mocked or color:grey", "b i"),
        ("((status:200))", "a c"),
        ("(status:200 (host:cdn | host:api)) | is:ws", "a c g"),
        ("status:200 -host:cdn", "a"),
        ("example -(host:api OR host:cdn) -is:error", "g i"),
        ("type:json body:ada", "a"),
        ("body:ada | body:hunter2", "a b"),
        ("-zzz \"\"", "a b c d e f g h i j"),
        ("-\"\"", ""),
    ];
    let mut failures = Vec::new();
    for (query, expected) in cases {
        let got = matching(query, &fx);
        if got != *expected {
            failures.push(format!("{query:?}: expected [{expected}] got [{got}]"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Filters as HTTP Toolkit serializes them (its tags copy as these strings, joined by spaces).
#[test]
fn http_toolkit_syntax() {
    let fx = fixtures();
    let cases: &[(&str, &str)] = &[
        ("method=GET", "a c e g h j"),
        ("method!=GET", "b d f i"),
        ("hostname=localhost", "b"),
        ("hostname$=example.com", "a d f g i"),
        ("hostname*=example", "a c d e f g i"),
        ("path^=/v1/", "a d"),
        ("path=/api/login", "b"),
        // HTTP Toolkit's query includes the leading '?'.
        ("query^=?page", "a"),
        ("query=?verbose", "h"),
        ("query*=?q=", "a"),
        ("query*=q=", "a"),
        ("status=404", "j"),
        ("status>=500", "h"),
        ("status!=200", "b d e f g h i j"),
        ("headers*=nginx", "h"),
        ("headers^=application/json", "a b"),
        ("header[x-cache]", "a"),
        ("header[x-cache]=hit", "a"),
        ("header[content-type]*=json", "a b"),
        ("body*=hunter2", "b"),
        ("body^={\"users\"", "a"),
        ("bodySize>2000", "a c i"),
        ("bodySize<=64", "b d e f g h"),
        ("port=8443", "d"),
        ("port>1024", "b d h"),
        ("protocol=wss", "g"),
        ("httpVersion=1", "b d e f g h"),
        ("httpVersion=3", "i"),
        ("category=html", "j"),
        ("category=js", "c"),
        ("category!=js", "a b d e f g h i j"),
        // Categories that are states map to flags.
        ("category=mutative", "b d i"),
        ("category=websocket", "g"),
        ("category=incomplete", "d g"),
        ("category=aborted", "e f"),
        ("category!=mutative", "a c e f g h j"),
        ("type:json,mutative", "a b d i"),
        ("type!=json,mutative", "c e f g h j"),
        // contains(text): everything incl. headers & bodies, up to the first ')'.
        ("contains(hunter2)", "b"),
        ("contains(not found)", "j"),
        ("CONTAINS(Bob)", "b"),
        ("-contains(hunter2)", "a c d e f g h i j"),
        // HTTP Toolkit's own bug: it searched `0: name,value` so this could never match.
        ("contains(content-type: application/json)", "a b"),
        ("contains()", "a b c d e f g h i j"),
        // not(…) and or(…), nesting and combining with other tags.
        ("not(status=200)", "b d e f g h i j"),
        ("or(method=GET, status=401)", "a b c e g h j"),
        ("or(status=404,status=500)", "j"),
        ("OR(is:pinned, color:grey)", "b i"),
        ("not(or(is:pinned, is:mocked))", "a c d e f g h i j"),
        ("or(not(method=GET), status=404)", "b d f i j"),
        ("or(or(status=404, status=401), port=8443)", "b d j"),
        ("status=200 or(hostname*=cdn, hostname*=upload)", "c"),
        ("or(hostname*=cdn, hostname*=upload) status=200", "c"),
        ("method=GET or(status=404, status=503) hostname$=.de", "j"),
        // Inside or(…) a comma ends a value, as in HTTP Toolkit.
        ("or(status:4xx,5xx)", "b j"),
        ("or((status:200 method:get), is:ws)", "a c g"),
        // `x or(y)` (a single filter) is our binary OR with a group.
        ("status:404 or(is:pinned)", "b j"),
        ("status:404 or (is:pinned)", "b j"),
    ];
    let mut failures = Vec::new();
    for (query, expected) in cases {
        let got = matching(query, &fx);
        if got != *expected {
            failures.push(format!("{query:?}: expected [{expected}] got [{got}]"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn llm_fields_and_flag() {
    use akev_core::LlmTag;
    let tagged = |name, id, provider: &str, model: Option<&str>| {
        let mut f = base(
            name,
            id,
            "POST",
            "https",
            "api.example.com",
            443,
            "/v1/chat/completions",
        );
        f.s.llm = Some(Box::new(LlmTag {
            provider: provider.into(),
            model: model.map(str::to_string),
        }));
        f
    };
    let fx = vec![
        tagged("gpt", 1, "openai", Some("gpt-4.1-mini")),
        tagged("claude", 2, "anthropic", Some("claude-sonnet-4-5")),
        tagged("azure", 3, "azure-openai", None),
        tagged("local", 4, "ollama", Some("llama3.2:3b")),
        base("plain", 5, "GET", "https", "api.example.com", 443, "/users"),
    ];
    assert_eq!(matching("is:llm", &fx), "gpt claude azure local");
    assert_eq!(matching("is:ai", &fx), "gpt claude azure local");
    assert_eq!(matching("-is:llm", &fx), "plain");
    assert_eq!(matching("model:gpt", &fx), "gpt");
    assert_eq!(matching("model:CLAUDE*", &fx), "claude");
    assert_eq!(matching("model:mini,llama", &fx), "gpt local");
    assert_eq!(matching("model=llama3.2:3b", &fx), "local");
    assert_eq!(matching("model~/^gpt-4\\.1/", &fx), "gpt");
    // Absent models never match a positive term; negations include them.
    assert_eq!(matching("-model:gpt", &fx), "claude azure local plain");
    // `provider:` matches the whole id (use a wildcard for families).
    assert_eq!(matching("provider:openai", &fx), "gpt");
    assert_eq!(matching("provider:*openai*", &fx), "gpt azure");
    assert_eq!(matching("vendor:anthropic,ollama", &fx), "claude local");
    assert_eq!(
        matching("provider!=openai is:llm", &fx),
        "claude azure local"
    );
    // Cheap summary terms: no headers or bodies are read.
    for f in &fx {
        assert_eq!(f.header_calls.get() + f.req_body_calls.get(), 0);
    }
}

#[test]
fn paused_and_pending_flags() {
    let mut fx = fixtures();
    fx[0].s.state = ExchangeState::Paused;
    fx[0].s.paused = Some(BreakpointStage::Response);
    assert_eq!(matching("is:paused", &fx), "a");
    assert_eq!(matching("is:breakpoint", &fx), "a");
    assert!(matching("is:pending", &fx).starts_with('a'));
    fx[0].s.kind = ExchangeKind::EventStream;
    assert_eq!(matching("is:sse", &fx), "a");
    fx[1].s.kind = ExchangeKind::Tunnel;
    fx[2].s.kind = ExchangeKind::ClientError;
    assert_eq!(matching("is:tunnel", &fx), "b");
    assert_eq!(matching("is:malformed", &fx), "c");
    assert_eq!(matching("is:error", &fx), "c e f");
}

#[test]
fn rule_title_and_ip_absent() {
    let mut fx = fixtures();
    fx[1].rule_title = None;
    assert_eq!(matching("rule:mock", &fx), "");
    assert_eq!(matching("rule:r-1", &fx), "b");
    assert_eq!(matching("-ip:1", &fx), "c d e f g i j");
}

// ---------------------------------------------------------------------------------------------
// Parse trees
// ---------------------------------------------------------------------------------------------

#[test]
fn tree_table() {
    let cases: &[(&str, &str)] = &[
        ("", "true"),
        ("  \t ", "true"),
        ("\"\"", "true"),
        ("a", r#"(text contains "a")"#),
        (
            "a b | c",
            r#"(or (and (text contains "a") (text contains "b")) (text contains "c"))"#,
        ),
        (
            "a | b c",
            r#"(or (text contains "a") (and (text contains "b") (text contains "c")))"#,
        ),
        (
            "a OR b AND c",
            r#"(or (text contains "a") (and (text contains "b") (text contains "c")))"#,
        ),
        (
            "-a b",
            r#"(and (not (text contains "a")) (text contains "b"))"#,
        ),
        (
            "-(a b)",
            r#"(not (and (text contains "a") (text contains "b")))"#,
        ),
        (
            "NOT a OR b",
            r#"(or (not (text contains "a")) (text contains "b"))"#,
        ),
        (
            "(a | b) c",
            r#"(and (or (text contains "a") (text contains "b")) (text contains "c"))"#,
        ),
        ("a&b", r#"(text contains "a&b")"#),
        ("a,b", r#"(text contains "a,b")"#),
        (
            "\"a b\" 'c'",
            r#"(and (text contains "a b") (text contains "c"))"#,
        ),
        ("a*b", r#"(text glob~ "a*b")"#),
        ("/a.b/i", "(text regex /a.b/)"),
        ("status:4xx", "(status [400,499])"),
        ("status:404", "(status =404)"),
        ("status:40x", "(status [400,409])"),
        ("status:>=500", "(status >=500)"),
        ("status>500", "(status >500)"),
        ("status:<300,>=500", "(status <300 | >=500)"),
        ("status!=200", "(not (status =200))"),
        ("size:>1mb", "(size >1048576)"),
        ("size:1.5kb", "(size =1536)"),
        ("size:1-2kb", "(size [1024,2048])"),
        ("duration:<1.5s", "(duration <1500)"),
        ("duration:2m", "(duration =120000)"),
        ("duration:1h", "(duration =3600000)"),
        ("port:80,443", "(port =80 | =443)"),
        ("method:get,post", r#"(method eq "get" | eq "post")"#),
        ("method!=get", r#"(not (method eq "get"))"#),
        ("host:x", r#"(host contains "x")"#),
        ("host=x", r#"(host eq "x")"#),
        ("host^=x", r#"(host prefix "x")"#),
        ("host$=x", r#"(host suffix "x")"#),
        ("host*=x*", r#"(host contains "x*")"#),
        ("host:*.x.com", r#"(host glob "*.x.com")"#),
        ("host:\"*.x.com\"", r#"(host contains "*.x.com")"#),
        ("path~^/a", "(path regex /^/a/)"),
        ("path:/^\\/a/", "(path regex /^\\/a/)"),
        ("path~\"\\d+\"", "(path regex /\\d+/)"),
        ("path=/a.*/", r#"(path eq "/a.*/")"#),
        ("is:pending,error", "(or (is Pending) (is Error))"),
        ("has:body", "(is Body)"),
        ("is!=pinned", "(not (is Pinned))"),
        (
            "type:json,text/plain",
            r#"(type kinds[json,json-lines] | contains "text/plain")"#,
        ),
        ("type:media", "(type kinds[audio,video])"),
        ("version:h2", "(version [HTTP/2])"),
        ("version:h1", "(version [HTTP/0.9,HTTP/1.0,HTTP/1.1])"),
        ("version:x", r#"(version eq "x")"#),
        ("header:x-a", r#"(any-header-name eq "x-a")"#),
        ("header.x-a:b", r#"(any-header[x-a] contains "b")"#),
        ("reqheader[x y]=z", r#"(req-header[x y] eq "z")"#),
        ("resheader[x]", "(res-header[x] any)"),
        ("headers:x", r#"(any-header-line contains "x")"#),
        ("cookie:s", r#"(cookie-name eq "s")"#),
        ("cookie.s:v", r#"(cookie[s] contains "v")"#),
        ("query.page:2", r#"(query[page] contains "2")"#),
        ("query:a", r#"(query contains "a")"#),
        ("body:x", "(any-body regex /x/)"),
        ("reqbody=x", r#"(req-body eq "x")"#),
        ("contains:x", r#"(contains contains "x")"#),
        ("color:gray", r#"(color eq "grey")"#),
        ("localhost:8080", r#"(text contains "localhost:8080")"#),
        ("https://x.io/a", r#"(text contains "https://x.io/a")"#),
        (
            "host.docker.internal:80",
            r#"(text contains "host.docker.internal:80")"#,
        ),
        ("x.y:80,81", r#"(text contains "x.y:80,81")"#),
        ("not(a)", r#"(not (text contains "a"))"#),
        (
            "or(a, b)",
            r#"(or (text contains "a") (text contains "b"))"#,
        ),
        ("or(a)", r#"(text contains "a")"#),
        (
            "x or(a, b)",
            r#"(and (text contains "x") (or (text contains "a") (text contains "b")))"#,
        ),
        ("x or(a)", r#"(or (text contains "x") (text contains "a"))"#),
        (
            "or(a b, c | d)",
            r#"(or (and (text contains "a") (text contains "b")) (or (text contains "c") (text contains "d")))"#,
        ),
        ("contains(a b)", r#"(contains contains "a b")"#),
        ("contains(\"x\")", r#"(contains contains "\"x\"")"#),
        ("category=mutative", "(is Mutative)"),
        ("type:mutative,aborted", "(or (is Mutative) (is Aborted))"),
        (
            "type:json,mutative",
            "(or (type kinds[json,json-lines]) (is Mutative))",
        ),
        (
            "type!=html,incomplete",
            "(not (or (type kinds[html]) (is Pending)))",
        ),
        ("query^=?a", r#"(query prefix "a")"#),
        ("query=?", r#"(query eq "?")"#),
        ("query.x:?y", r#"(query[x] contains "?y")"#),
        (
            "a || b && c",
            r#"(or (text contains "a") (and (text contains "b") (text contains "c")))"#,
        ),
        (
            "order nothing android",
            r#"(and (text contains "order") (text contains "nothing") (text contains "android"))"#,
        ),
    ];
    let mut failures = Vec::new();
    for (query, expected) in cases {
        let got = tree(query);
        if got != *expected {
            failures.push(format!(
                "{query:?}:\n  expected {expected}\n  got      {got}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn optimizer_orders_cheap_terms_first() {
    assert_eq!(
        optimized("body:x method:get"),
        r#"(and (method eq "get") (any-body regex /x/))"#
    );
    assert_eq!(
        optimized("body:x OR status:200"),
        "(or (status =200) (any-body regex /x/))"
    );
    assert_eq!(
        optimized("header:x status:200 example"),
        r#"(and (status =200) (text contains "example") (any-header-name eq "x"))"#
    );
    assert_eq!(
        optimized("a (b c)"),
        r#"(and (text contains "a") (text contains "b") (text contains "c"))"#
    );
    assert_eq!(
        optimized("a | (b | c)"),
        r#"(or (text contains "a") (text contains "b") (text contains "c"))"#
    );
    assert_eq!(optimized("--x"), r#"(text contains "x")"#);
    assert_eq!(optimized("\"\" x"), r#"(text contains "x")"#);
    assert_eq!(optimized("-\"\""), "false");
    assert_eq!(optimized("x | \"\""), "true");
    assert_eq!(
        optimized("contains:x is:pinned body:y"),
        r#"(and (is Pinned) (any-body regex /y/) (contains contains "x"))"#
    );
    // Equal-cost terms keep the written order.
    assert_eq!(
        optimized("is:pinned is:mocked"),
        "(and (is Pinned) (is Mocked))"
    );
    assert_eq!(
        optimized("is:mocked is:pinned"),
        "(and (is Mocked) (is Pinned))"
    );
}

// ---------------------------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------------------------

#[test]
fn error_table() {
    let cases: &[(&str, &str, usize, usize)] = &[
        (
            "stauts:404",
            "Unknown field 'stauts' — did you mean 'status'?",
            0,
            6,
        ),
        (
            "mehtod:get",
            "Unknown field 'mehtod' — did you mean 'method'?",
            0,
            6,
        ),
        (
            "stat:404",
            "Unknown field 'stat' — did you mean 'status'?",
            0,
            4,
        ),
        (
            "x stauts:404",
            "Unknown field 'stauts' — did you mean 'status'?",
            2,
            8,
        ),
        (
            "foo=bar",
            "Unknown field 'foo'. To search for text containing '=', quote it: \"foo=bar\"",
            0,
            3,
        ),
        (
            "foo:bar",
            "Unknown field 'foo'. To search for text containing ':', quote it: \"foo:bar\"",
            0,
            3,
        ),
        (
            "heder.x-foo:bar",
            "Unknown field 'heder' — did you mean 'header'?",
            0,
            5,
        ),
        ("status:>abc", "Expected a number after '>'", 8, 11),
        ("status>abc", "Expected a number after '>'", 7, 10),
        ("status:>", "Expected a number after '>'", 7, 8),
        ("status:>=", "Expected a number after '>='", 7, 9),
        ("status>", "Expected a number after '>'", 6, 7),
        ("status:", "Expected a value after 'status:'", 0, 7),
        ("host: x", "Expected a value after 'host:'", 0, 5),
        ("(host:)", "Expected a value after 'host:'", 1, 6),
        (
            "status:abc",
            "Invalid status 'abc' — use a code (404), a class (4xx), a range (400-499) or a comparison (>=500)",
            7,
            10,
        ),
        (
            "size:>1zb",
            "Unknown size unit 'zb' — use b, kb, mb or gb",
            6,
            9,
        ),
        (
            "size:abc",
            "Invalid size 'abc' — use a number with an optional unit (b, kb, mb, gb), e.g. 500, 10kb, >1mb",
            5,
            8,
        ),
        (
            "duration:5parsecs",
            "Unknown duration unit 'parsecs' — use ms, s, m or h",
            9,
            17,
        ),
        (
            "port:1kb",
            "'port' takes a plain number, not '1kb' — e.g. port:80, port>1000 or port:10-20",
            5,
            8,
        ),
        (
            "id:x",
            "Expected a number for 'id', e.g. id:80, id>1000 or id:10-20",
            3,
            4,
        ),
        ("size:1-", "Expected a number after '-'", 5, 7),
        (
            "status:-5",
            "Invalid status '-5' — use a code (404), a class (4xx), a range (400-499) or a comparison (>=500)",
            7,
            9,
        ),
        ("host:\"abc", "Unterminated string — add a closing \"", 5, 9),
        ("host:'abc", "Unterminated string — add a closing '", 5, 9),
        ("\"abc", "Unterminated string — add a closing \"", 0, 4),
        ("a 'b c", "Unterminated string — add a closing '", 2, 6),
        (
            "path~/abc",
            "Unterminated regex — add a closing '/' (use \\s to match spaces)",
            5,
            9,
        ),
        (
            "url~/a b/",
            "Unterminated regex — add a closing '/' (use \\s to match spaces)",
            4,
            6,
        ),
        ("path~/a(/", "Invalid regex: unclosed group", 5, 9),
        ("path:/a(/", "Invalid regex: unclosed group", 5, 9),
        ("/a(/", "Invalid regex: unclosed group", 0, 4),
        ("path~[", "Invalid regex: unclosed character class", 5, 6),
        (
            "path~/api/users",
            "Unknown regex flag 'e' (supported: i, m, s, x). To match a literal '/', escape it as \\/ or use ':' instead of '~'",
            5,
            15,
        ),
        ("(status:200", "Missing ')' to close this '('", 0, 1),
        ("a (b (c)", "Missing ')' to close this '('", 2, 3),
        (
            "status:200)",
            "Unmatched ')' — there is no '(' for it to close",
            10,
            11,
        ),
        (")", "Unmatched ')' — there is no '(' for it to close", 0, 1),
        ("()", "Empty parentheses — put a filter inside", 0, 2),
        ("a ( ) b", "Empty parentheses — put a filter inside", 2, 5),
        ("a OR", "Expected a filter after 'OR'", 2, 4),
        ("a or )", "Expected a filter after 'or'", 2, 4),
        ("OR a", "Expected a filter before 'OR'", 0, 2),
        ("a OR OR b", "Expected a filter after 'OR'", 2, 4),
        ("a ||", "Expected a filter after '||'", 2, 4),
        ("a |", "Expected a filter after '|'", 2, 3),
        ("a &&", "Expected a filter after '&&'", 2, 4),
        ("&& a", "Expected a filter before '&&'", 0, 2),
        ("-", "Expected a filter after '-'", 0, 1),
        ("a -", "Expected a filter after '-'", 2, 3),
        ("NOT", "Expected a filter after 'NOT'", 0, 3),
        ("(a | )", "Expected a filter after '|'", 3, 4),
        (
            "is:pinnd",
            "Unknown flag 'pinnd' — did you mean 'pinned'?",
            3,
            8,
        ),
        (
            "is:pinned,mockd",
            "Unknown flag 'mockd' — did you mean 'mocked'?",
            10,
            15,
        ),
        (
            "is:",
            "Expected a flag after 'is:' — e.g. is:error, is:pending, has:body",
            0,
            3,
        ),
        (
            "has:",
            "Expected a flag after 'has:' — e.g. is:error, is:pending, has:body",
            0,
            4,
        ),
        (
            "is>3",
            "Write flags as is:<flag>, e.g. is:error or has:body",
            2,
            3,
        ),
        (
            "is.x:y",
            "Write flags as is:<flag>, e.g. is:error or has:body",
            2,
            4,
        ),
        (
            "host>5",
            "'>' compares numbers, but 'host' is text — use host: (contains), host= (equals), host^= (starts with), host$= (ends with) or host~ (regex)",
            4,
            5,
        ),
        (
            "method<=x",
            "'<=' compares numbers, but 'method' is text — use method: (is), method= (equals), method^= (starts with), method$= (ends with) or method~ (regex)",
            6,
            8,
        ),
        (
            "host:>\"foo\"",
            "'>' compares numbers, but 'host' is text — quote the whole value to search for it",
            5,
            6,
        ),
        (
            "api:8080",
            "Unknown field 'api' — did you mean 'app'?",
            0,
            3,
        ),
        (
            "status^=4",
            "'^=' works on text, but 'status' is a number — try status:4xx, status>=400 or status:400-499",
            6,
            8,
        ),
        (
            "size~1",
            "'~' works on text, but 'size' is a number — try size:>1mb, size<10kb or size:1kb-1mb",
            4,
            5,
        ),
        (
            "status:/4\\d\\d/",
            "'status' is a number, so regexes don't apply — try status:4xx, status>=400 or status:400-499",
            7,
            14,
        ),
        ("method:get,", "Expected a value after ','", 10, 11),
        ("method:get,,post", "Expected a value after ','", 10, 11),
        ("header.:x", "Expected a header name after 'header.'", 6, 7),
        ("header[]:x", "Expected a header name after 'header.'", 6, 8),
        ("cookie.=x", "Expected a cookie name after 'cookie.'", 6, 7),
        (
            "host.foo=bar",
            "'host' doesn't take a sub-key (only header, reqheader, resheader, cookie and query do)",
            4,
            8,
        ),
        (
            "status[x]:1",
            "'status' doesn't take a sub-key (only header, reqheader, resheader, cookie and query do)",
            6,
            9,
        ),
        // HTTP Toolkit call syntax.
        (
            "or()",
            "Empty or() — list filters inside, separated by commas: or(status:404, is:error)",
            0,
            4,
        ),
        ("or(", "Missing ')' to close this 'or('", 0, 3),
        ("or(a, b", "Missing ')' to close this 'or('", 0, 3),
        ("x OR(a, b", "Missing ')' to close this 'or('", 2, 5),
        ("or(a,)", "Expected a filter after ','", 4, 5),
        ("or(a,,b)", "Expected a filter after ','", 4, 5),
        ("or(, a)", "Expected a filter before ','", 3, 4),
        (
            "or(stauts:1, a)",
            "Unknown field 'stauts' — did you mean 'status'?",
            3,
            9,
        ),
        ("contains(abc", "Missing ')' to close 'contains('", 0, 12),
        ("x CONTAINS(", "Missing ')' to close 'CONTAINS('", 2, 11),
    ];
    let mut failures = Vec::new();
    for (query, message, start, end) in cases {
        match Filter::parse(query) {
            Ok(f) => failures.push(format!("{query:?}: parsed as {}", f.explain())),
            Err(e) => {
                if e.message != *message || e.start != *start || e.end != *end {
                    failures.push(format!(
                        "{query:?}:\n  expected {message:?} {start}..{end}\n  got      {:?} {}..{}",
                        e.message, e.start, e.end
                    ));
                }
                assert!(
                    e.start <= e.end && e.end <= query.len(),
                    "{query:?}: span out of range"
                );
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn nesting_limit() {
    let deep = format!("{}a{}", "(".repeat(200), ")".repeat(200));
    assert_eq!(perr(&deep).message, "Filter is nested too deeply");
    let calls = format!("{}a, b{}", "or(".repeat(200), ")".repeat(200));
    assert_eq!(perr(&calls).message, "Filter is nested too deeply");
    let nots = format!("{}a", "-".repeat(200));
    assert_eq!(perr(&nots).message, "Filter is nested too deeply");
    let ok = format!("{}a{}", "(".repeat(50), ")".repeat(50));
    assert!(Filter::parse(&ok).is_ok());
    // Long flat queries are fine (AND/OR are iterative).
    let long = vec!["status:200"; 5000].join(" OR ");
    assert!(Filter::parse(&long).is_ok());
}

#[test]
fn parse_error_serializes_camel_case() {
    let e = perr("stauts:1");
    let json = serde_json::to_value(&e).unwrap();
    assert_eq!(json["start"], 0);
    assert_eq!(json["end"], 6);
    assert!(json["message"].as_str().unwrap().contains("status"));
    assert_eq!(e.to_string(), e.message);
}

// ---------------------------------------------------------------------------------------------
// Laziness, needs_body, API
// ---------------------------------------------------------------------------------------------

#[test]
fn body_not_accessed_when_cheap_and_term_fails() {
    let fx = fixtures();
    let a = &fx[0];
    for query in [
        "status:500 body:ada",
        "body:ada status:500",
        "body:ada -is:complete",
        "(body:x | body:y) method:post",
    ] {
        let f = Filter::parse(query).unwrap();
        assert!(!f.matches(a), "{query}");
        assert_eq!(
            a.req_body_calls.get() + a.res_body_calls.get(),
            0,
            "{query}: body accessed"
        );
    }
    // Headers are also skipped when a summary term fails first.
    let f = Filter::parse("header:x-cache status:500").unwrap();
    assert!(!f.matches(a));
    assert_eq!(a.header_calls.get(), 0);
}

#[test]
fn body_not_accessed_when_cheap_or_term_succeeds() {
    let fx = fixtures();
    let a = &fx[0];
    let f = Filter::parse("body:nothing-here OR status:200").unwrap();
    assert!(f.matches(a));
    assert_eq!(a.req_body_calls.get() + a.res_body_calls.get(), 0);
}

#[test]
fn bodies_fetched_at_most_once_per_evaluation() {
    let fx = fixtures();
    let a = &fx[0];
    let f =
        Filter::parse("body:ada body:users resbody:name contains:zzz-none | contains:Ada").unwrap();
    assert!(f.matches(a));
    assert!(
        a.req_body_calls.get() <= 1,
        "request body fetched {} times",
        a.req_body_calls.get()
    );
    assert!(
        a.res_body_calls.get() <= 1,
        "response body fetched {} times",
        a.res_body_calls.get()
    );
    // A new evaluation fetches again (no state leaks between calls).
    let before = a.res_body_calls.get();
    assert!(f.matches(a));
    assert_eq!(a.res_body_calls.get(), before + 1);
}

#[test]
fn needs_body_and_match_all() {
    for (q, needs) in [
        ("status:200", false),
        ("body:x", true),
        ("a | reqbody:x", true),
        ("-resbody:x", true),
        ("contains:x", true),
        ("headers:x has:body", false),
        ("", false),
    ] {
        assert_eq!(Filter::parse(q).unwrap().needs_body(), needs, "{q}");
    }
    assert!(Filter::parse("").unwrap().is_match_all());
    assert!(Filter::parse("  ").unwrap().is_match_all());
    assert!(Filter::parse("\"\"").unwrap().is_match_all());
    assert!(!Filter::parse("x").unwrap().is_match_all());
    assert!(Filter::all().is_match_all());
    assert!(Filter::default().is_match_all());
    assert_eq!(Filter::all().source(), "");
    assert!(!Filter::all().needs_body());
    let q = "  status:200   OR x ";
    assert_eq!(Filter::parse(q).unwrap().source(), q);
    let fx = fixtures();
    assert!(fx.iter().all(|t| Filter::all().matches(t)));
    assert!(
        fx.iter()
            .all(|t| !Filter::parse("-\"\"").unwrap().matches(t))
    );
}

#[test]
fn filter_is_send_sync_clone() {
    fn assert_traits<T: Send + Sync + Clone + 'static>() {}
    assert_traits::<Filter>();
    let f = Filter::parse("status:4xx host:example").unwrap();
    let g = f.clone();
    assert!(std::sync::Arc::ptr_eq(&f.inner, &g.inner));
    let handle = std::thread::spawn(move || g.source().len());
    assert_eq!(handle.join().unwrap(), f.source().len());
}

#[test]
fn kind_names_match_serde() {
    for kind in crate::eval::ALL_KINDS {
        let json = serde_json::to_value(kind).unwrap();
        assert_eq!(json.as_str().unwrap(), crate::fields::kind_name(kind));
    }
    for (kind, name, _) in crate::fields::ERROR_KINDS {
        assert_eq!(serde_json::to_value(kind).unwrap().as_str().unwrap(), *name);
    }
}

// ---------------------------------------------------------------------------------------------
// Docs
// ---------------------------------------------------------------------------------------------

#[test]
fn field_docs_are_complete_and_examples_parse() {
    let docs = field_docs();
    assert_eq!(
        docs.len(),
        crate::fields::FIELDS.len() + crate::fields::FLAGS.len()
    );
    let mut names = std::collections::HashSet::new();
    for doc in &docs {
        assert!(names.insert(doc.name), "duplicate doc {}", doc.name);
        assert!(
            !doc.description.is_empty(),
            "{} has no description",
            doc.name
        );
        assert!(
            (2..=4).contains(&doc.examples.len()),
            "{} has {} examples",
            doc.name,
            doc.examples.len()
        );
        for ex in doc.examples {
            if let Err(e) = Filter::parse(ex) {
                panic!("example {ex:?} for {} fails: {e:?}", doc.name);
            }
        }
    }
    for flag in [
        "is:pending",
        "is:error",
        "has:body",
        "has:auth",
        "is:websocket",
    ] {
        assert!(names.contains(flag), "missing {flag}");
    }
    for field in [
        "method", "status", "host", "header", "body", "duration", "size",
    ] {
        assert!(names.contains(field), "missing {field}");
    }
    let json = serde_json::to_value(&docs[0]).unwrap();
    assert!(json.get("examples").is_some() && json.get("aliases").is_some());
}

#[test]
fn every_field_and_alias_parses() {
    for field in crate::fields::FIELDS {
        for name in std::iter::once(field.name).chain(field.aliases.iter().copied()) {
            let value = if field.subkey.is_some()
                || matches!(field.ty, crate::fields::FieldType::Text(_))
            {
                "x"
            } else {
                "1"
            };
            let q = format!("{name}:{value}");
            assert!(
                Filter::parse(&q).is_ok(),
                "{q} failed: {:?}",
                Filter::parse(&q).err()
            );
            let q = format!("{}:{value}", name.to_ascii_uppercase());
            assert!(Filter::parse(&q).is_ok(), "{q} failed");
        }
    }
    for flag in crate::fields::FLAGS {
        for name in std::iter::once(flag.name).chain(flag.aliases.iter().copied()) {
            for prefix in ["is", "has"] {
                let q = format!("{prefix}:{name}");
                assert!(
                    Filter::parse(&q).is_ok(),
                    "{q} failed: {:?}",
                    Filter::parse(&q).err()
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Suggestions
// ---------------------------------------------------------------------------------------------

fn ctx() -> SuggestContext {
    SuggestContext {
        hosts: vec![
            "api.example.com".into(),
            "cdn.example.org".into(),
            "localhost".into(),
        ],
        methods: vec!["GET".into(), "POST".into(), "patch".into()],
        sources: vec!["Chrome".into(), "curl".into()],
        content_types: vec!["application/json".into(), "text/html".into()],
        header_names: vec![
            "Content-Type".into(),
            "X-Request-Id".into(),
            "Authorization".into(),
        ],
        models: vec!["gpt-4.1".into(), "claude-sonnet-4-5".into()],
        providers: vec!["anthropic".into()],
    }
}

fn sug(query: &str, cursor: usize) -> Vec<Suggestion> {
    let out = suggest(query, cursor, &ctx());
    assert!(out.len() <= 30);
    for s in &out {
        assert!(
            s.replace_start <= s.replace_end && s.replace_end <= query.len(),
            "{query:?}: {s:?}"
        );
        assert!(query.is_char_boundary(s.replace_start) && query.is_char_boundary(s.replace_end));
    }
    out
}

fn labels(v: &[Suggestion]) -> Vec<&str> {
    v.iter().map(|s| s.label.as_str()).collect()
}

fn apply(query: &str, s: &Suggestion) -> String {
    format!(
        "{}{}{}",
        &query[..s.replace_start],
        s.insert,
        &query[s.replace_end..]
    )
}

#[track_caller]
fn first(query: &str, cursor: usize) -> Suggestion {
    sug(query, cursor)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("no suggestions for {query:?}@{cursor}"))
}

#[track_caller]
fn find(query: &str, cursor: usize, label: &str) -> Suggestion {
    let all = sug(query, cursor);
    all.iter()
        .find(|s| s.label == label)
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "{label:?} not suggested for {query:?}@{cursor}: {:?}",
                labels(&all)
            )
        })
}

#[test]
fn suggest_empty_query_shows_examples_then_fields() {
    let out = sug("", 0);
    assert_eq!(out.len(), 30);
    assert_eq!(out[0].label, "host:api.example.com");
    assert_eq!(out[0].kind, "example");
    assert_eq!(out[1].label, "status:4xx");
    assert!(
        out.iter()
            .all(|s| s.replace_start == 0 && s.replace_end == 0)
    );
    let ls = labels(&out);
    for want in [
        "is:", "has:", "method:", "status:", "host:", "path:", "type:", "body:",
    ] {
        assert!(ls.contains(&want), "missing {want}: {ls:?}");
    }
    let is_pos = ls.iter().position(|l| *l == "is:").unwrap();
    let host_pos = ls.iter().position(|l| *l == "host:").unwrap();
    let path_pos = ls.iter().position(|l| *l == "path:").unwrap();
    assert!(host_pos < is_pos && is_pos < path_pos, "{ls:?}");
    let status = find("", 0, "status:");
    assert_eq!(status.kind, "field");
    assert_eq!(status.insert, "status:");
    assert!(status.detail.contains("4xx"));
    // Whitespace-only queries behave like empty ones, at the cursor.
    let out = sug("   ", 2);
    assert_eq!(out[1].label, "status:4xx");
    assert_eq!((out[0].replace_start, out[0].replace_end), (2, 2));
}

#[test]
fn suggest_field_prefix() {
    let s = first("sta", 3);
    assert_eq!(
        (
            s.label.as_str(),
            s.insert.as_str(),
            s.replace_start,
            s.replace_end,
            s.kind
        ),
        ("status:", "status:", 0, 3, "field")
    );
    assert_eq!(apply("sta", &s), "status:");
    // Case-insensitive, and the whole word is replaced when the cursor is in the middle.
    let s = first("STA", 3);
    assert_eq!(s.label, "status:");
    let s = first("status", 3);
    assert_eq!(
        (s.label.as_str(), s.replace_start, s.replace_end),
        ("status:", 0, 6)
    );
    // Aliases find the canonical field.
    let s = first("doma", 4);
    assert_eq!(s.label, "host:");
    let s = first("lat", 3);
    assert_eq!(s.label, "duration:");
    // Fuzzy: subsequence and typos.
    assert!(labels(&sug("drtn", 4)).contains(&"duration:"));
    assert_eq!(first("stauts", 6).label, "status:");
    // Negation and groups keep exact offsets.
    let s = first("-sta", 4);
    assert_eq!((s.replace_start, s.replace_end), (1, 4));
    assert_eq!(apply("-sta", &s), "-status:");
    let s = first("(method:get) sta", 16);
    assert_eq!(
        (s.label.as_str(), s.replace_start, s.replace_end),
        ("status:", 13, 16)
    );
    // Sub-key fields offer both forms.
    let ls = labels(&sug("head", 4))
        .into_iter()
        .map(String::from)
        .collect::<Vec<_>>();
    assert!(
        ls.contains(&"header:".to_string()) && ls.contains(&"header.<name>:".to_string()),
        "{ls:?}"
    );
    assert_eq!(find("head", 4, "header.<name>:").insert, "header.");
}

#[test]
fn suggest_flags() {
    let s = first("pin", 3);
    assert_eq!(
        (s.label.as_str(), s.kind, s.insert.as_str()),
        ("is:pinned", "flag", "is:pinned")
    );
    let out = sug("is", 2);
    assert_eq!(out[0].label, "is:");
    // Canonical names beat aliases: "s" doesn't lead with is:pinned (alias "starred").
    let out = sug("s", 1);
    assert_eq!(
        &labels(&out)[..4],
        ["status:", "size:", "scheme:", "source:"]
    );
    assert!(
        labels(&out).iter().position(|l| *l == "is:sse")
            < labels(&out).iter().position(|l| *l == "is:pinned")
    );
    // Typing an alias that prefixes the canonical name doesn't jump to operators.
    let out = sug("err", 3);
    assert_eq!(out[0].label, "error:");
    assert_eq!(first("time", 4).label, "time:");
    assert!(labels(&out).contains(&"is:error"));
    let out = sug("is:p", 4);
    assert_eq!(&labels(&out)[..3], ["pending", "paused", "pinned"]);
    assert!(
        out.iter()
            .all(|s| s.replace_start == 3 && s.replace_end == 4 && s.kind == "flag")
    );
    let out = sug("has:", 4);
    assert_eq!(out[0].label, "note");
    assert!(labels(&out).contains(&"error"));
    let s = first("is:error,ab", 11);
    assert_eq!(
        (s.label.as_str(), s.replace_start, s.replace_end),
        ("aborted", 9, 11)
    );
    let s = first("is:fail", 7);
    assert_eq!(s.label, "error");
}

#[test]
fn suggest_operators_after_bare_field() {
    let out = sug("status", 6);
    assert_eq!(out[0].label, "status:");
    assert_eq!(out[0].kind, "operator");
    let ls = labels(&out);
    for op in ["status>=", "status<", "status!="] {
        assert!(ls.contains(&op), "{op} missing: {ls:?}");
    }
    assert!(!ls.contains(&"status^="));
    assert!(
        out.iter()
            .take(6)
            .all(|s| s.replace_start == 0 && s.replace_end == 6)
    );
    let out = sug("host", 4);
    let ls = labels(&out);
    for op in ["host:", "host=", "host^=", "host$=", "host~", "host!="] {
        assert!(ls.contains(&op), "{op} missing: {ls:?}");
    }
    assert!(!ls.contains(&"host>="));
    // The alias the user typed is kept.
    assert_eq!(first("domain", 6).label, "domain:");
    // Cursor inside a two-character operator replaces it.
    let s = first("status>=4", 7);
    assert_eq!((s.replace_start, s.replace_end, s.kind), (0, 8, "operator"));
}

#[test]
fn suggest_status_values() {
    let out = sug("status:", 7);
    assert_eq!(&labels(&out)[..5], ["2xx", "3xx", "4xx", "5xx", "1xx"]);
    assert!(
        out.iter()
            .all(|s| (s.replace_start, s.replace_end) == (7, 7) && s.kind == "value")
    );
    assert!(labels(&out).contains(&">=400"));
    let out = sug("status:4", 8);
    assert_eq!(out[0].label, "4xx");
    assert_eq!(out[1].label, "400");
    assert_eq!(out[1].detail, "Bad Request");
    assert_eq!((out[0].replace_start, out[0].replace_end), (7, 8));
    let out = sug("status:5", 8);
    assert_eq!(labels(&out), ["5xx", "500", "501", "502", "503", "504"]);
    let s = find("status:40", 9, "404");
    assert_eq!(s.detail, "Not Found");
    assert_eq!(apply("status:40", &s), "status:404");
    let out = sug("status:>=", 9);
    assert!(
        out.iter()
            .all(|s| (s.replace_start, s.replace_end) == (9, 9))
    );
    assert!(labels(&out).contains(&"500"));
    assert!(!labels(&out).contains(&">=400"));
    let out = sug("status>", 7);
    assert!(labels(&out).contains(&"400"));
    // Alternatives: completion applies to the alternative under the cursor.
    let s = find("status:200,4", 12, "4xx");
    assert_eq!((s.replace_start, s.replace_end), (11, 12));
    assert_eq!(apply("status:200,4", &s), "status:200,4xx");
    let s = find("status:20,404", 9, "200");
    assert_eq!((s.replace_start, s.replace_end), (7, 9));
}

#[test]
fn suggest_values_from_context() {
    let out = sug("host:", 5);
    assert_eq!(
        labels(&out),
        ["api.example.com", "cdn.example.org", "localhost"]
    );
    let out = sug("host:exa", 8);
    assert_eq!(labels(&out), ["api.example.com", "cdn.example.org"]);
    assert_eq!((out[0].replace_start, out[0].replace_end), (5, 8));
    let s = first("host:loc foo", 8);
    assert_eq!(
        (s.label.as_str(), s.replace_start, s.replace_end),
        ("localhost", 5, 8)
    );
    assert_eq!(apply("host:loc foo", &s), "host:localhost foo");
    // Cursor in the middle of a value replaces the whole value.
    let s = first("host:locxyz", 8);
    assert_eq!((s.replace_start, s.replace_end), (5, 11));
    let out = sug("method:p", 8);
    assert_eq!(labels(&out), ["POST", "PATCH", "PUT", "OPTIONS"]);
    let out = sug("method:", 7);
    assert_eq!(&labels(&out)[..4], ["GET", "POST", "PATCH", "PUT"]);
    let s = first("method:get,po", 13);
    assert_eq!(
        (s.label.as_str(), s.replace_start, s.replace_end),
        ("POST", 11, 13)
    );
    let out = sug("method:get,", 11);
    assert!(
        out.iter()
            .all(|s| (s.replace_start, s.replace_end) == (11, 11))
    );
    let out = sug("source:", 7);
    assert_eq!(labels(&out), ["Chrome", "curl"]);
    let out = sug("type:j", 6);
    assert_eq!(&labels(&out)[..3], ["json", "js", "jsonl"]);
    let s = first("category=mut", 12);
    assert_eq!((s.label.as_str(), s.replace_start), ("mutative", 9));
    assert!(s.detail.contains("POST"));
    assert!(labels(&sug("type:", 5)).contains(&"text/html"));
    let out = sug("scheme:", 7);
    assert_eq!(labels(&out), ["https", "http", "wss", "ws"]);
    assert_eq!(&labels(&sug("version:", 8))[..3], ["h1", "h2", "h3"]);
    assert_eq!(&labels(&sug("color:", 6))[..2], ["red", "orange"]);
    assert_eq!(first("color:gr", 8).label, "green");
    assert_eq!(first("error:t", 7).label, "timeout");
    assert_eq!(first("error:dns", 9).label, "dns-failed");
    assert_eq!(first("port:", 5).label, "443");
    // LLM models seen in traffic; providers seen first, then the known ones.
    assert_eq!(labels(&sug("model:", 6)), ["gpt-4.1", "claude-sonnet-4-5"]);
    assert_eq!(first("model:cla", 9).label, "claude-sonnet-4-5");
    let out = sug("provider:", 9);
    assert_eq!(&labels(&out)[..3], ["anthropic", "openai", "google"]);
    assert!(out[0].detail.contains("seen in traffic"));
    assert_eq!(first("provider:oll", 12).label, "ollama");
    assert_eq!(first("is:ll", 5).label, "llm");
    assert_eq!(first("llm", 3).label, "is:llm");
}

#[test]
fn suggest_units() {
    let out = sug("size:>10", 8);
    assert_eq!(labels(&out), ["10kb", "10mb", "10b", "10gb"]);
    assert!(
        out.iter()
            .all(|s| (s.replace_start, s.replace_end) == (6, 8))
    );
    assert_eq!(labels(&sug("size:>10m", 9)), ["10mb"]);
    assert_eq!(labels(&sug("duration:5", 10)), ["5ms", "5s", "5m"]);
    assert_eq!(labels(&sug("duration:1.5", 12)), ["1.5ms", "1.5s", "1.5m"]);
    assert_eq!(first("size:", 5).label, ">1mb");
    assert_eq!(first("size:>", 6).label, "1kb");
    assert_eq!(first("duration:", 9).label, ">1s");
    assert_eq!(first("reqsize:<", 9).label, "1kb");
}

#[test]
fn suggest_header_names_and_values() {
    let out = sug("header.", 7);
    assert_eq!(
        &labels(&out)[..3],
        ["content-type", "x-request-id", "authorization"]
    );
    assert_eq!(out[0].insert, "content-type:");
    assert_eq!((out[0].replace_start, out[0].replace_end), (7, 7));
    let s = first("header.x-r", 10);
    assert_eq!(
        (
            s.label.as_str(),
            s.insert.as_str(),
            s.replace_start,
            s.replace_end
        ),
        ("x-request-id", "x-request-id:", 7, 10)
    );
    let s = first("header[x-r", 10);
    assert_eq!(s.insert, "x-request-id]:");
    assert_eq!(apply("header[x-r", &s), "header[x-request-id]:");
    let s = first("reqheader.auth", 14);
    assert_eq!((s.insert.as_str(), s.replace_start), ("authorization:", 10));
    let out = sug("header:", 7);
    assert_eq!(out[0].insert, "content-type");
    assert!(labels(&out).contains(&"user-agent"));
    // Cursor inside the sub-key of a complete term.
    let s = first("header.x-r:abc", 10);
    assert_eq!(
        (s.insert.as_str(), s.replace_start, s.replace_end),
        ("x-request-id", 7, 10)
    );
    let out = sug("header.content-type:", 20);
    assert_eq!(&labels(&out)[..2], ["application/json", "text/html"]);
    let out = sug("resheader.content-encoding:g", 28);
    assert_eq!(out[0].label, "gzip");
    // No header names for cookie/query sub-keys.
    assert!(sug("cookie.", 7).iter().all(|s| s.replace_start != 7));
}

#[test]
fn suggest_quoted_values() {
    let s = first("host:\"exa", 9);
    assert_eq!(
        (
            s.label.as_str(),
            s.insert.as_str(),
            s.replace_start,
            s.replace_end
        ),
        ("api.example.com", "\"api.example.com\"", 5, 9)
    );
    let s = first("host:\"exa\" x", 9);
    assert_eq!((s.replace_start, s.replace_end), (5, 10));
    assert_eq!(apply("host:\"exa\" x", &s), "host:\"api.example.com\" x");
    // Values needing quotes get them.
    let c = SuggestContext {
        sources: vec!["Google Chrome".into()],
        ..Default::default()
    };
    let out = suggest("source:goo", 10, &c);
    assert_eq!(out[0].insert, "\"Google Chrome\"");
    // Regex literals get nothing.
    assert!(sug("host:/a.*/", 9).is_empty());
    assert!(sug("\"some text", 5).is_empty());
}

#[test]
fn suggest_name_corrections() {
    let s = first("stat:404", 2);
    assert_eq!(
        (
            s.label.as_str(),
            s.insert.as_str(),
            s.replace_start,
            s.replace_end
        ),
        ("status", "status", 0, 4)
    );
    let s = first("stauts:4", 8);
    assert_eq!(
        (s.label.as_str(), s.replace_start, s.replace_end),
        ("status", 0, 6)
    );
    assert_eq!(apply("stauts:4", &s), "status:4");
    let s = first("status:404 ", 3);
    assert_eq!(
        (s.label.as_str(), s.replace_start, s.replace_end),
        ("status", 0, 6)
    );
}

#[test]
fn suggest_shortcuts_and_keywords() {
    let s = find("exa", 3, "host:api.example.com");
    assert_eq!((s.replace_start, s.replace_end, s.kind), (0, 3, "value"));
    let out = sug("po", 2);
    let ls = labels(&out);
    let port = ls.iter().position(|l| *l == "port:").unwrap();
    let post = ls.iter().position(|l| *l == "method:post").unwrap();
    assert!(port < post, "{ls:?}");
    assert!(labels(&sug("chr", 3)).contains(&"source:Chrome"));
    assert!(labels(&sug("40", 2)).contains(&"status:404"));
    assert!(labels(&sug("jso", 3)).contains(&"type:json"));
    let o = find("o", 1, "OR");
    assert_eq!(o.kind, "operator");
    let s = first("a or", 4);
    assert_eq!(
        (s.label.as_str(), s.replace_start, s.replace_end),
        ("OR", 2, 4)
    );
    assert!(labels(&sug("a or", 4)).contains(&"color:"));
    assert!(labels(&sug("a no", 4)).contains(&"NOT"));
}

#[test]
fn suggest_new_term_positions() {
    let out = sug("status:200 ", 11);
    assert!(
        out.iter()
            .all(|s| (s.replace_start, s.replace_end) == (11, 11))
    );
    assert!(out.iter().all(|s| s.kind != "example"));
    assert!(labels(&out).contains(&"method:"));
    for (q, c) in [
        ("(", 1),
        ("a | ", 4),
        ("-", 1),
        ("a OR ", 5),
        ("status:200 (", 12),
        ("x ||", 4),
        ("or(", 3),
        ("or(a, ", 6),
        ("x or(a,", 7),
    ] {
        let out = sug(q, c);
        assert!(!out.is_empty(), "{q:?}");
        assert!(
            out.iter()
                .all(|s| (s.replace_start, s.replace_end) == (c, c)),
            "{q:?}: {:?}",
            out[0]
        );
    }
    // Before an existing term: insert at the cursor.
    let s = find("foo", 0, "host:");
    assert_eq!((s.replace_start, s.replace_end), (0, 0));
    assert_eq!(apply("foo", &s), "host:foo");
    // Inside HTTP Toolkit style calls.
    let s = first("or(sta", 6);
    assert_eq!(
        (s.label.as_str(), s.replace_start, s.replace_end),
        ("status:", 3, 6)
    );
    let s = first("or(a, status:4", 14);
    assert_eq!(
        (s.label.as_str(), s.replace_start, s.replace_end),
        ("4xx", 13, 14)
    );
    assert!(sug("contains(abc", 12).is_empty());
}

#[test]
fn suggest_cursor_clamping_and_unicode() {
    assert_eq!(sug("sta", 100), sug("sta", 3));
    // A cursor inside a multi-byte character snaps back to its start.
    let q = "über";
    assert_eq!(sug(q, 1), sug(q, 0));
    let out = sug("host:über", 9);
    assert!(out.iter().all(|s| s.replace_start == 5));
    let c = SuggestContext {
        hosts: vec!["www.über.de".into()],
        ..Default::default()
    };
    let out = suggest("host:ÜB", "host:ÜB".len(), &c);
    assert_eq!(out[0].label, "www.über.de");
}

#[test]
fn suggestion_serializes_camel_case() {
    let s = first("sta", 3);
    let json = serde_json::to_value(&s).unwrap();
    assert_eq!(json["replaceStart"], 0);
    assert_eq!(json["replaceEnd"], 3);
    assert_eq!(json["kind"], "field");
    assert_eq!(json["insert"], "status:");
}

#[test]
fn accepted_suggestions_parse() {
    // Accepting a value suggestion for a field yields a valid query.
    for q in [
        "status:",
        "host:",
        "method:",
        "type:",
        "size:",
        "duration:",
        "is:",
        "has:",
        "color:",
        "error:",
        "version:",
        "scheme:",
        "header:",
        "port:",
        "header.content-type:",
    ] {
        let out = sug(q, q.len());
        assert!(!out.is_empty(), "{q}");
        for s in &out {
            let applied = apply(q, s);
            assert!(
                Filter::parse(&applied).is_ok(),
                "{q} + {s:?} = {applied:?}: {:?}",
                Filter::parse(&applied).err()
            );
        }
    }
    // And accepting a field suggestion followed by its first value suggestion, too.
    for s in sug("", 0) {
        let q = apply("", &s);
        if let Some(v) = sug(&q, q.len()).first() {
            let applied = apply(&q, v);
            // `header.` + `content-type:` still needs a value; everything else is complete.
            if !applied.ends_with(':') {
                assert!(
                    Filter::parse(&applied).is_ok(),
                    "{applied:?}: {:?}",
                    Filter::parse(&applied).err()
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Robustness
// ---------------------------------------------------------------------------------------------

/// xorshift64* — deterministic, dependency-free.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const FRAGMENTS: &[&str] = &[
    "status",
    "host",
    "header",
    "is",
    "has",
    "body",
    "size",
    "duration",
    "method",
    "type",
    "cookie",
    "query",
    "url",
    "version",
    "contains",
    ":",
    "=",
    "!=",
    "^=",
    "$=",
    "*=",
    "~",
    ">",
    ">=",
    "<",
    "<=",
    "!",
    "-",
    "(",
    ")",
    "\"",
    "'",
    "/",
    "\\",
    ",",
    " ",
    "  ",
    "|",
    "||",
    "&",
    "&&",
    "*",
    ".",
    "[",
    "]",
    "ü",
    "😀",
    "\u{a0}",
    "\u{2028}",
    "4xx",
    "OR",
    "or",
    "AND",
    "not",
    "NOT",
    "a",
    "1",
    "1.5",
    "kb",
    "ms",
    "\t",
    "\n",
    "x-trace",
    "example.com",
    "//",
    "i",
    "e",
    "0",
    "-",
    "2xx-5xx",
    "\u{0}",
    "é",
    "ß",
    "İ",
    "or(",
    "contains(",
    "category=",
    "mutative",
];

fn random_query(rng: &mut Rng) -> String {
    let n = rng.below(16);
    let mut s = String::new();
    for _ in 0..n {
        s.push_str(FRAGMENTS[rng.below(FRAGMENTS.len())]);
    }
    s
}

#[test]
fn fuzz_no_panics() {
    let fx = fixtures();
    let c = ctx();
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for round in 0..2500 {
        let q = if round % 5 == 4 {
            // Arbitrary bytes, made valid UTF-8.
            let len = rng.below(24);
            let bytes: Vec<u8> = (0..len).map(|_| (rng.next() & 0xFF) as u8).collect();
            String::from_utf8_lossy(&bytes).into_owned()
        } else {
            random_query(&mut rng)
        };
        match Filter::parse(&q) {
            Ok(f) => {
                for t in &fx {
                    let _ = f.matches(t);
                }
            }
            Err(e) => {
                assert!(
                    e.start <= e.end && e.end <= q.len(),
                    "{q:?}: bad span {e:?}"
                );
                assert!(
                    q.is_char_boundary(e.start) && q.is_char_boundary(e.end),
                    "{q:?}: span splits a char {e:?}"
                );
                assert!(!e.message.is_empty());
            }
        }
        // Every position for short queries; a sample (plus both ends) for long ones.
        let cursors: Vec<usize> = if q.len() <= 16 {
            (0..=q.len() + 1).collect()
        } else {
            let mut v: Vec<usize> = (0..10).map(|_| rng.below(q.len() + 1)).collect();
            v.extend([0, q.len(), q.len() + 3]);
            v
        };
        let mut pos = 0;
        for t in crate::highlight(&q) {
            assert!(
                t.start >= pos && t.end > t.start && t.end <= q.len(),
                "{q:?}: {t:?}"
            );
            assert!(
                q.is_char_boundary(t.start) && q.is_char_boundary(t.end),
                "{q:?}: {t:?}"
            );
            pos = t.end;
        }
        for cursor in cursors {
            for s in suggest(&q, cursor, &c) {
                assert!(
                    s.replace_start <= s.replace_end && s.replace_end <= q.len(),
                    "{q:?}@{cursor}: {s:?}"
                );
                assert!(
                    q.is_char_boundary(s.replace_start) && q.is_char_boundary(s.replace_end),
                    "{q:?}@{cursor}: {s:?}"
                );
            }
        }
    }
}

#[test]
fn scanner_tokens_cover_input_in_order() {
    // Token spans are ordered, non-overlapping and only skip whitespace.
    let mut rng = Rng(42);
    for _ in 0..3000 {
        let q = random_query(&mut rng);
        let tokens = crate::scan::Scanner::tokenize(&q);
        let mut pos = 0;
        for t in &tokens {
            assert!(
                t.span.start >= pos && t.span.end >= t.span.start,
                "{q:?}: {t:?}"
            );
            assert!(
                q[pos..t.span.start].chars().all(char::is_whitespace),
                "{q:?}: gap before {t:?}"
            );
            assert!(t.span.end > t.span.start, "{q:?}: empty token {t:?}");
            pos = t.span.end;
        }
        assert!(
            q[pos..].chars().all(char::is_whitespace),
            "{q:?}: trailing input not tokenized"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Benchmark
// ---------------------------------------------------------------------------------------------

/// Run with `cargo test -p akev-filter --release -- --ignored --nocapture bench`.
#[test]
#[ignore]
fn bench_100k_exchanges() {
    let fx = fixtures();
    let targets: Vec<Fake> = (0..100_000)
        .map(|i| {
            let mut t = fx[i % fx.len()].clone();
            t.s.id = ExchangeId(i as u64);
            t.s.host = format!("svc{}.{}", i % 97, t.s.host);
            t
        })
        .collect();
    let queries = [
        "method:get status:2xx host:example",
        "host:*.example.com -is:error size:>1kb",
        "api status:4xx,5xx duration:>100ms",
        "type:json path:/v1/* -method:options",
        "body:ada status:500",
    ];
    for q in queries {
        let filter = Filter::parse(q).unwrap();
        let mut best = std::time::Duration::MAX;
        let mut count = 0;
        for _ in 0..5 {
            let start = std::time::Instant::now();
            count = targets.iter().filter(|t| filter.matches(*t)).count();
            best = best.min(start.elapsed());
        }
        println!(
            "{q:45} {count:>6} matches  {:>8.2} ms",
            best.as_secs_f64() * 1000.0
        );
        if !cfg!(debug_assertions) {
            assert!(best.as_millis() < 30, "{q}: {best:?} (budget 30ms)");
        }
    }
    let start = std::time::Instant::now();
    for _ in 0..1000 {
        let _ = Filter::parse("method:get,post status:2xx host:*.example.com -is:error size:>1kb")
            .unwrap();
    }
    println!(
        "parse+compile: {:.1} µs/query",
        start.elapsed().as_secs_f64() * 1e6 / 1000.0
    );
    let c = ctx();
    for q in [
        "status:200 host:exa",
        "",
        "s",
        "stauts",
        "qwertyuiop",
        "status:4",
        "header.x-",
        "type:",
    ] {
        let start = std::time::Instant::now();
        for _ in 0..1000 {
            let _ = suggest(q, q.len(), &c);
        }
        println!(
            "suggest {q:?}: {:.1} µs/call",
            start.elapsed().as_secs_f64() * 1e6 / 1000.0
        );
    }
}
