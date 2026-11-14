//! Human-readable rule summaries (used by the CLI, MCP tools and as the UI fallback title).
//!
//! ```text
//! GET requests to *.example.com/api/* → mock 200
//! POST requests to /login with JSON body including {"user":"admin"} → delay 500 ms (once)
//! All requests → pass through
//! ```

use akev_core::rules::{
    Action, BodySource, BodyTransform, KeyValueMatch, MatchOp, Matcher, RequestTransform,
    ResponseTransform, Rule, StringMatch, WsBreakpointDirection,
};

/// One-line summary: `<what matches> → <what happens>[ (once|N times)]`.
pub fn summarize(rule: &Rule) -> String {
    let mut out = format!("{} → {}", matchers(&rule.matchers), action(&rule.action));
    match rule.max_matches {
        Some(1) => out.push_str(" (once)"),
        Some(n) => out.push_str(&format!(" ({n} times)")),
        None => {}
    }
    out
}

/// Describe a set of AND-ed matchers, e.g. `GET requests to example.com/api/*`.
pub fn matchers(ms: &[Matcher]) -> String {
    let mut methods: Option<&[String]> = None;
    let mut ws = false;
    let mut host: Option<&StringMatch> = None;
    let mut path: Option<&StringMatch> = None;
    let mut rest = Vec::new();
    for m in ms {
        match m {
            Matcher::Method { methods: list } if methods.is_none() => methods = Some(list),
            Matcher::Protocol { protocols }
                if !protocols.is_empty()
                    && protocols
                        .iter()
                        .all(|p| p.eq_ignore_ascii_case("ws") || p.eq_ignore_ascii_case("wss")) =>
            {
                ws = true;
                if protocols.len() == 1 {
                    rest.push(format!("over {}", protocols[0].to_ascii_lowercase()));
                }
            }
            // "HTTP, not WebSocket" (e.g. imported HTTP Toolkit rules) is implied by "requests".
            Matcher::Protocol { protocols }
                if protocols.len() == 2
                    && ["http", "https"]
                        .iter()
                        .all(|p| protocols.iter().any(|q| q.eq_ignore_ascii_case(p))) => {}
            Matcher::Host { pattern } if host.is_none() && is_plain(pattern) => {
                host = Some(pattern)
            }
            Matcher::Path { pattern } if path.is_none() && is_plain(pattern) => {
                path = Some(pattern)
            }
            other => rest.push(matcher(other)),
        }
    }
    let noun = if ws { "WebSockets" } else { "requests" };
    let mut out = match methods {
        Some(list) if !list.is_empty() => format!("{} {noun}", join_or(list)),
        Some(_) => format!("No {noun}"),
        None if ms.is_empty() => format!("All {noun}"),
        None => capitalize(noun),
    };
    match (host, path) {
        (Some(h), Some(p)) => out.push_str(&format!(" to {}{}", h.value, p.value)),
        (Some(h), None) => out.push_str(&format!(" to {}", h.value)),
        (None, Some(p)) => out.push_str(&format!(" to {}", p.value)),
        (None, None) => {}
    }
    for r in rest {
        out.push(' ');
        out.push_str(&r);
    }
    out
}

fn is_plain(p: &StringMatch) -> bool {
    matches!(p.op, MatchOp::Equals | MatchOp::Glob) && !p.negate
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn join_or(items: &[String]) -> String {
    join(items, "or")
}

fn join(items: &[String], word: &str) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} {word} {last}", init.join(", ")),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// Describe a string comparison against a field, e.g. `starting with /api`.
fn string_match(sm: &StringMatch) -> String {
    let v = truncate(&sm.value, 60);
    let base = match sm.op {
        MatchOp::Equals => format!("= {v}"),
        MatchOp::Contains => format!("containing {v:?}"),
        MatchOp::StartsWith => format!("starting with {v}"),
        MatchOp::EndsWith => format!("ending with {v}"),
        MatchOp::Glob => format!("matching {v}"),
        MatchOp::Regex => format!("matching /{v}/"),
        MatchOp::Exists => "present".to_string(),
    };
    if sm.negate {
        format!("not {base}")
    } else {
        base
    }
}

fn key_values(kvs: &[KeyValueMatch], sep: &str) -> String {
    let parts: Vec<String> = kvs
        .iter()
        .map(|kv| match &kv.value {
            None => kv.key.clone(),
            Some(sm) if sm.op == MatchOp::Equals && !sm.negate => {
                format!("{}{sep}{}", kv.key, truncate(&sm.value, 40))
            }
            Some(sm) if sm.op == MatchOp::Exists && sm.negate => format!("no {}", kv.key),
            Some(sm) => format!("{} {}", kv.key, string_match(sm)),
        })
        .collect();
    join(&parts, "and")
}

/// Describe a single matcher as a phrase (`with header x-a: 1`, `on port 8080`...).
pub fn matcher(m: &Matcher) -> String {
    match m {
        Matcher::Method { methods } => format!("using {}", join_or(methods)),
        Matcher::Url { pattern } => format!("with URL {}", string_match(pattern)),
        Matcher::Host { pattern } => format!("with host {}", string_match(pattern)),
        Matcher::Port { ports } => {
            let list: Vec<String> = ports.iter().map(u16::to_string).collect();
            if ports.len() == 1 {
                format!("on port {}", list[0])
            } else {
                format!("on ports {}", join_or(&list))
            }
        }
        Matcher::Path { pattern } => format!("with path {}", string_match(pattern)),
        Matcher::Protocol { protocols } => format!("over {}", join_or(protocols)),
        Matcher::Query { params } => format!("with query {}", key_values(params, "=")),
        Matcher::Header { headers } => format!("with header {}", key_values(headers, ": ")),
        Matcher::Cookie { cookies } => format!("with cookie {}", key_values(cookies, "=")),
        Matcher::Body { pattern } => format!("with body {}", string_match(pattern)),
        Matcher::JsonBody { value, partial } => {
            let json = truncate(&value.to_string(), 60);
            if *partial {
                format!("with JSON body including {json}")
            } else {
                format!("with JSON body {json}")
            }
        }
        Matcher::Form { fields } => format!("with form field {}", key_values(fields, "=")),
        Matcher::GraphqlOperation { pattern } => match pattern.op {
            MatchOp::Equals if !pattern.negate => {
                format!("for GraphQL operation {}", pattern.value)
            }
            _ => format!("for GraphQL operation {}", string_match(pattern)),
        },
        Matcher::Source { pattern } => match pattern.op {
            MatchOp::Equals if !pattern.negate => format!("from {}", pattern.value),
            _ => format!("from source {}", string_match(pattern)),
        },
        Matcher::HttpVersion { versions } => format!("over {}", join_or(versions)),
        Matcher::Not { matcher: inner } => format!("not ({})", matcher(inner)),
        Matcher::Any { matchers } => {
            let parts: Vec<String> = matchers.iter().map(matcher).collect();
            format!("({})", join(&parts, "or"))
        }
        Matcher::All { matchers } => {
            let parts: Vec<String> = matchers.iter().map(matcher).collect();
            format!("({})", join(&parts, "and"))
        }
    }
}

fn body_source(b: &BodySource) -> String {
    match b {
        BodySource::Empty => "an empty body".to_string(),
        BodySource::Text { text } => format!("text {:?}", truncate(text, 40)),
        BodySource::Base64 { .. } => "binary data".to_string(),
        BodySource::Json { value } => format!("JSON {}", truncate(&value.to_string(), 40)),
        BodySource::File { path } => format!("file {path}"),
    }
}

fn body_transform(b: &BodyTransform) -> String {
    match b {
        BodyTransform::Replace { body } => format!("replace body with {}", body_source(body)),
        BodyTransform::MatchReplace { replacements } => {
            format!(
                "find/replace in body ({} rule{})",
                replacements.len(),
                if replacements.len() == 1 { "" } else { "s" }
            )
        }
        BodyTransform::JsonMerge { .. } => "merge into JSON body".to_string(),
        BodyTransform::JsonPatch { operations } => format!(
            "patch JSON body ({} op{})",
            operations.len(),
            if operations.len() == 1 { "" } else { "s" }
        ),
    }
}

fn header_names(h: &akev_core::Headers) -> String {
    let mut names: Vec<String> = Vec::new();
    for (k, _) in h.iter() {
        if !names.iter().any(|n| n.eq_ignore_ascii_case(k)) {
            names.push(k.to_string());
        }
    }
    join(&names, "and")
}

/// Describe a request transform, e.g. `set header X-Debug, replace body with text "x"`.
pub fn request_transform(t: &RequestTransform) -> String {
    let mut parts = Vec::new();
    if let Some(m) = &t.method {
        parts.push(format!("method → {m}"));
    }
    if !t.url_replace.is_empty() {
        parts.push("rewrite URL".to_string());
    }
    if !t.set_query.is_empty() {
        let keys: Vec<String> = t.set_query.iter().map(|(k, _)| k.clone()).collect();
        parts.push(format!("set query {}", join(&keys, "and")));
    }
    if !t.remove_headers.is_empty() {
        parts.push(format!("remove header {}", join(&t.remove_headers, "and")));
    }
    if !t.set_headers.is_empty() {
        parts.push(format!("set header {}", header_names(&t.set_headers)));
    }
    if !t.add_headers.is_empty() {
        parts.push(format!("add header {}", header_names(&t.add_headers)));
    }
    if let Some(b) = &t.body {
        parts.push(body_transform(b));
    }
    parts.join(", ")
}

/// Describe a response transform, e.g. `status → 404, remove header ETag`.
pub fn response_transform(t: &ResponseTransform) -> String {
    let mut parts = Vec::new();
    if let Some(s) = t.status {
        parts.push(format!("status → {s}"));
    }
    if let Some(m) = &t.status_message {
        parts.push(format!("status message → {m:?}"));
    }
    if !t.remove_headers.is_empty() {
        parts.push(format!("remove header {}", join(&t.remove_headers, "and")));
    }
    if !t.set_headers.is_empty() {
        parts.push(format!("set header {}", header_names(&t.set_headers)));
    }
    if !t.add_headers.is_empty() {
        parts.push(format!("add header {}", header_names(&t.add_headers)));
    }
    if let Some(b) = &t.body {
        parts.push(body_transform(b));
    }
    parts.join(", ")
}

fn transforms(req: Option<&RequestTransform>, res: Option<&ResponseTransform>) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(t) = req.filter(|t| !t.is_empty()) {
        parts.push(format!("request: {}", request_transform(t)));
    }
    if let Some(t) = res.filter(|t| !t.is_empty()) {
        parts.push(format!("response: {}", response_transform(t)));
    }
    (!parts.is_empty()).then(|| parts.join("; "))
}

fn rate(bps: u64) -> String {
    let bits = bps as f64 * 8.0;
    if bits >= 1_000_000.0 {
        format!("{:.1} Mbit/s", bits / 1_000_000.0)
    } else if bits >= 1_000.0 {
        format!("{:.0} kbit/s", bits / 1_000.0)
    } else {
        format!("{bits:.0} bit/s")
    }
}

fn profile_label(p: akev_core::settings::TlsProfile) -> &'static str {
    use akev_core::settings::TlsProfile as P;
    match p {
        P::Native => "native",
        P::Chrome => "Chrome",
        P::Firefox => "Firefox",
        P::Safari => "Safari",
        P::Edge => "Edge",
        P::MirrorClient => "the client's browser",
    }
}

/// Describe an action, e.g. `mock 200`, `forward to http://localhost:3000`.
pub fn action(a: &Action) -> String {
    match a {
        Action::Passthrough { request, response } => {
            match transforms(request.as_ref(), response.as_ref()) {
                Some(t) => format!("pass through ({t})"),
                None => "pass through".to_string(),
            }
        }
        Action::Forward {
            target,
            strip_prefix,
            preserve_host,
            request,
            response,
        } => {
            let mut out = format!("forward to {target}");
            if let Some(p) = strip_prefix {
                out.push_str(&format!(" (stripping {p})"));
            }
            if *preserve_host {
                out.push_str(" keeping the Host header");
            }
            if let Some(t) = transforms(request.as_ref(), response.as_ref()) {
                out.push_str(&format!(" ({t})"));
            }
            out
        }
        Action::Mock { status, .. } => format!("mock {status}"),
        Action::MapLocal { path, .. } => format!("serve local files from {path}"),
        Action::Close => "close the connection".to_string(),
        Action::Reset => "reset the connection".to_string(),
        Action::Timeout => "time out (never respond)".to_string(),
        Action::WsEcho => "echo WebSocket messages".to_string(),
        Action::WsReject { status, .. } => format!("reject the WebSocket ({status})"),
        Action::Transform { request, response } => {
            match transforms(request.as_ref(), response.as_ref()) {
                Some(t) => format!("transform ({t})"),
                None => "transform (nothing)".to_string(),
            }
        }
        Action::Breakpoint { request, response } => match (request, response) {
            (true, true) => "pause request and response".to_string(),
            (true, false) => "pause request".to_string(),
            (false, true) => "pause response".to_string(),
            (false, false) => "breakpoint (disabled)".to_string(),
        },
        Action::Delay { ms, jitter_ms } => {
            if *jitter_ms > 0 {
                format!("delay {ms} ms (+0–{jitter_ms} ms)")
            } else {
                format!("delay {ms} ms")
            }
        }
        Action::Throttle {
            download_bps,
            upload_bps,
            latency_ms,
        } => {
            let mut parts = Vec::new();
            if *download_bps > 0 {
                parts.push(format!("{} down", rate(*download_bps)));
            }
            if *upload_bps > 0 {
                parts.push(format!("{} up", rate(*upload_bps)));
            }
            if *latency_ms > 0 {
                parts.push(format!("+{latency_ms} ms latency"));
            }
            if parts.is_empty() {
                "throttle (unlimited)".to_string()
            } else {
                format!("throttle to {}", parts.join(", "))
            }
        }
        Action::Highlight { color, note } => match note {
            Some(n) => format!("highlight {color} ({})", truncate(n, 40)),
            None => format!("highlight {color}"),
        },
        Action::Script { .. } => "run script".to_string(),
        Action::TlsProfile { profile } => match profile {
            akev_core::settings::TlsProfile::Native => {
                "use Akev's native TLS fingerprint upstream".to_string()
            }
            akev_core::settings::TlsProfile::MirrorClient => {
                "mirror the client's browser TLS fingerprint upstream".to_string()
            }
            p => format!("impersonate {} upstream (TLS + HTTP/2)", profile_label(*p)),
        },
        Action::WsBreakpoint { direction, pattern } => {
            let what = match direction {
                WsBreakpointDirection::Sent => "sent WebSocket messages",
                WsBreakpointDirection::Received => "received WebSocket messages",
                WsBreakpointDirection::Both => "WebSocket messages",
            };
            match pattern {
                Some(p) => format!("pause {what} {}", string_match(p)),
                None => format!("pause {what}"),
            }
        }
    }
}
