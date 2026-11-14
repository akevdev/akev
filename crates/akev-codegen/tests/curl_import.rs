//! `parse_curl`: real-world "Copy as cURL" output from Chrome and Firefox (bash, cmd and
//! PowerShell flavours), curl option semantics, error cases, and generate → parse round trips.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use akev_codegen::{ParseError, SnippetRequest, Target, generate, parse_curl};

fn parse(cmd: &str) -> SnippetRequest {
    match parse_curl(cmd) {
        Ok(r) => r,
        Err(e) => panic!("failed to parse {cmd:?}: {e}"),
    }
}

fn headers(r: &SnippetRequest) -> Vec<(&str, &str)> {
    r.headers.iter().collect()
}

fn body(r: &SnippetRequest) -> &[u8] {
    r.body.as_deref().unwrap_or_default()
}

// ---------------------------------------------------------------------------------------------
// Browser formats

#[test]
fn chrome_bash_get() {
    let r = parse(
        r#"curl 'https://www.example.com/api/search?q=rust&page=1' \
  -H 'accept: application/json, text/plain, */*' \
  -H 'accept-language: en-US,en;q=0.9,sl;q=0.8' \
  -b '_ga=GA1.2.123; session=eyJhbGci%3D%3D' \
  -H 'priority: u=1, i' \
  -H 'referer: https://www.example.com/search' \
  -H 'sec-ch-ua: "Chromium";v="124", "Google Chrome";v="124", "Not-A.Brand";v="99"' \
  -H 'sec-ch-ua-mobile: ?0' \
  -H 'sec-ch-ua-platform: "Linux"' \
  -H 'user-agent: Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36'"#,
    );
    assert_eq!(r.method, "GET");
    assert_eq!(r.url, "https://www.example.com/api/search?q=rust&page=1");
    assert_eq!(r.body, None);
    let h = headers(&r);
    assert_eq!(h.len(), 9);
    assert_eq!(h[0], ("accept", "application/json, text/plain, */*"));
    assert_eq!(
        h[4],
        (
            "sec-ch-ua",
            r#""Chromium";v="124", "Google Chrome";v="124", "Not-A.Brand";v="99""#
        )
    );
    assert_eq!(
        r.headers.get("cookie"),
        Some("_ga=GA1.2.123; session=eyJhbGci%3D%3D")
    );
    assert!(
        r.headers
            .get("user-agent")
            .unwrap()
            .ends_with("Safari/537.36")
    );
}

#[test]
fn chrome_bash_post_with_ansi_c_quoting() {
    let r = parse(
        "curl 'https://api.example.com/v1/notes' \\\n  -H 'content-type: application/json' \\\n  -H 'origin: https://app.example.com' \\\n  --data-raw $'{\"text\":\"it\\'s a \\u00e9 test\\\\n\",\"tab\":\"a\\tb\"}'",
    );
    assert_eq!(r.method, "POST");
    assert_eq!(
        body(&r),
        "{\"text\":\"it's a é test\\n\",\"tab\":\"a\tb\"}".as_bytes()
    );
    // An explicit content type suppresses curl's form default.
    assert_eq!(r.headers.get_all("content-type").count(), 1);
}

#[test]
fn chrome_bash_put_with_explicit_method() {
    let r = parse(
        "curl 'https://x.test/items/1' \\\n  -X 'PUT' \\\n  -H 'content-type: text/plain' \\\n  --data-raw 'hello'",
    );
    assert_eq!(r.method, "PUT");
    assert_eq!(body(&r), b"hello");
}

#[test]
fn firefox_posix_post() {
    let r = parse(
        "curl 'https://example.com/api' --compressed -X POST -H 'User-Agent: Mozilla/5.0 (X11; Linux x86_64; rv:125.0) Gecko/20100101 Firefox/125.0' -H 'Accept: */*' -H 'Accept-Language: en-US,en;q=0.5' -H 'Accept-Encoding: gzip, deflate, br' -H 'Content-Type: application/json' -H 'Origin: https://example.com' -H 'Connection: keep-alive' -H 'Cookie: a=1; b=2' -H 'Sec-Fetch-Dest: empty' --data-raw '{\"a\":1}'",
    );
    assert_eq!(r.method, "POST");
    assert_eq!(r.url, "https://example.com/api");
    assert_eq!(body(&r), br#"{"a":1}"#);
    // The explicit Accept-Encoding wins over --compressed's default.
    assert_eq!(
        r.headers.get_all("accept-encoding").collect::<Vec<_>>(),
        ["gzip, deflate, br"]
    );
    assert_eq!(r.headers.get("cookie"), Some("a=1; b=2"));
    assert_eq!(r.headers.len(), 9);
}

#[test]
fn chrome_cmd_post() {
    let r = parse(
        "curl ^\"https://example.com/api?a=1^&b=2^\" ^\n  -H ^\"accept: */*^\" ^\n  -H ^\"content-type: application/json^\" ^\n  -b ^\"session=abc; theme=dark^\" ^\n  --data-raw ^\"^{^\\^\"query^\\^\":^\\^\"hello world 100^%^\\^\",^\\^\"n^\\^\":1^}^\" ^\n  --compressed",
    );
    assert_eq!(r.method, "POST");
    assert_eq!(r.url, "https://example.com/api?a=1&b=2");
    assert_eq!(body(&r), br#"{"query":"hello world 100%","n":1}"#);
    assert_eq!(r.headers.get("cookie"), Some("session=abc; theme=dark"));
    assert_eq!(
        r.headers.get("accept-encoding"),
        Some("gzip, deflate, br, zstd")
    );
}

#[test]
fn chrome_cmd_simple_quotes_and_crlf() {
    let r = parse(
        "curl \"https://example.com/\" ^\r\n  -H \"Accept: text/html\" ^\r\n  -H \"User-Agent: Mozilla/5.0\"\r\n",
    );
    assert_eq!(r.url, "https://example.com/");
    assert_eq!(
        headers(&r),
        [("Accept", "text/html"), ("User-Agent", "Mozilla/5.0")]
    );
}

#[test]
fn chrome_cmd_multiline_body() {
    let r = parse("curl ^\"https://x.test/^\" ^\n  --data-raw ^\"line1^\n\nline2^\"");
    assert_eq!(body(&r), b"line1\nline2");
}

#[test]
fn firefox_windows_powershell_style() {
    let r = parse(
        "curl.exe \"https://example.com/api\" -X POST -H \"User-Agent: Mozilla/5.0\" -H \"Content-Type: application/json\" --data-raw \"{\"\"a\"\":\"\"b c\"\",\"\"price\"\":\"\"`$5\"\"}\"",
    );
    assert_eq!(r.method, "POST");
    assert_eq!(r.url, "https://example.com/api");
    assert_eq!(body(&r), br#"{"a":"b c","price":"$5"}"#);
}

#[test]
fn powershell_backtick_continuations() {
    let r = parse("curl.exe 'https://x.test/' `\n  -H 'It''s: fine' `\n  -d 'a=1'");
    assert_eq!(headers(&r)[0], ("It's", "fine"));
    assert_eq!(body(&r), b"a=1");
}

// ---------------------------------------------------------------------------------------------
// Options

#[test]
fn method_inference() {
    assert_eq!(parse("curl https://x.test/").method, "GET");
    assert_eq!(parse("curl -d a=1 https://x.test/").method, "POST");
    assert_eq!(parse("curl -I https://x.test/").method, "HEAD");
    assert_eq!(parse("curl --head https://x.test/").method, "HEAD");
    assert_eq!(parse("curl -X PUT -d x https://x.test/").method, "PUT");
    assert_eq!(parse("curl -XDELETE https://x.test/").method, "DELETE");
    assert_eq!(parse("curl -X POST https://x.test/").method, "POST");
    assert_eq!(parse("curl -T file.txt https://x.test/").method, "PUT");
    assert_eq!(parse("curl -F a=b https://x.test/").method, "POST");
    assert_eq!(parse("curl --json '{}' https://x.test/").method, "POST");
    assert_eq!(parse("curl -X purge https://x.test/").method, "purge");
}

#[test]
fn data_options() {
    let r = parse("curl -d 'a=1' --data 'b=2' --data-ascii c=3 https://x.test/");
    assert_eq!(body(&r), b"a=1&b=2&c=3");
    assert_eq!(
        r.headers.get("content-type"),
        Some("application/x-www-form-urlencoded")
    );

    let r = parse(
        "curl --data-urlencode 'q=hello world&more' --data-urlencode '=a&b' --data-urlencode 'plain text' https://x.test/",
    );
    assert_eq!(body(&r), b"q=hello%20world%26more&a%26b&plain%20text");

    let r =
        parse("curl --data-raw '@not-a-file' --data-binary @data.bin -d @form.txt https://x.test/");
    assert_eq!(body(&r), b"@not-a-file&@data.bin&@form.txt");

    let r = parse("curl --json '{\"a\":' --json '1}' https://x.test/");
    assert_eq!(body(&r), br#"{"a":1}"#);
    assert_eq!(r.headers.get("content-type"), Some("application/json"));
    assert_eq!(r.headers.get("accept"), Some("application/json"));

    let r = parse("curl --json '{}' -H 'Accept: text/plain' https://x.test/");
    assert_eq!(
        r.headers.get_all("accept").collect::<Vec<_>>(),
        ["text/plain"]
    );

    let r = parse("curl --data-binary $'\\x89PNG\\x00\\xff' https://x.test/");
    assert_eq!(body(&r), [0x89, b'P', b'N', b'G', 0, 0xff]);

    let r = parse("curl -d '' https://x.test/");
    assert_eq!(r.method, "POST");
    assert_eq!(r.body, None);
}

#[test]
fn get_moves_data_to_query() {
    let r = parse("curl -G -d 'q=1' --data-urlencode 'r=a b' 'https://x.test/s?z=0#frag'");
    assert_eq!(r.method, "GET");
    assert_eq!(r.url, "https://x.test/s?z=0&q=1&r=a%20b#frag");
    assert_eq!(r.body, None);
    assert_eq!(r.headers.get("content-type"), None);
    let r = parse("curl -G -I -d 'a=1' https://x.test/");
    assert_eq!(r.method, "HEAD");
    assert_eq!(r.url, "https://x.test/?a=1");
}

#[test]
fn auth_options() {
    let r = parse("curl -u user:pa:ss https://x.test/");
    assert_eq!(
        r.headers.get("authorization"),
        Some("Basic dXNlcjpwYTpzcw==")
    );
    let r = parse("curl -u user https://x.test/");
    assert_eq!(r.headers.get("authorization"), Some("Basic dXNlcjo="));
    let r = parse("curl -u user:pass -H 'Authorization: Bearer x' https://x.test/");
    assert_eq!(
        r.headers.get_all("authorization").collect::<Vec<_>>(),
        ["Bearer x"]
    );
    let r = parse("curl --digest -u user:pass https://x.test/");
    assert_eq!(r.headers.get("authorization"), None);
    let r = parse("curl --oauth2-bearer tok123 https://x.test/");
    assert_eq!(r.headers.get("authorization"), Some("Bearer tok123"));
}

#[test]
fn cookie_agent_referer() {
    let r = parse(
        "curl -H 'Cookie: c=3' -b 'a=1' -b 'b=2;' -b cookies.txt -A 'MyAgent/1.0' -e 'https://ref.test/;auto' https://x.test/",
    );
    assert_eq!(
        r.headers.get_all("cookie").collect::<Vec<_>>(),
        ["c=3; a=1; b=2"]
    );
    assert_eq!(r.headers.get("user-agent"), Some("MyAgent/1.0"));
    assert_eq!(r.headers.get("referer"), Some("https://ref.test/"));
    let r = parse("curl -A '' -H 'User-Agent: explicit' --user-agent other https://x.test/");
    assert_eq!(
        r.headers.get_all("user-agent").collect::<Vec<_>>(),
        ["explicit"]
    );
}

#[test]
fn header_syntax() {
    let r = parse(
        "curl -H 'X-Empty;' -H 'Accept:' -H 'X-Spaced:   v  ' -H 'NoColon' -H @headers.txt -d a https://x.test/",
    );
    assert_eq!(
        headers(&r),
        [
            ("X-Empty", ""),
            ("X-Spaced", "v"),
            ("Content-Type", "application/x-www-form-urlencoded")
        ]
    );
    // `Content-Type:` removes curl's default.
    let r = parse("curl -H 'Content-Type:' -d a https://x.test/");
    assert!(r.headers.is_empty());
    assert_eq!(body(&r), b"a");
    // Duplicates are kept in order.
    let r = parse("curl -H 'X-A: 1' -H 'x-a: 2' https://x.test/");
    assert_eq!(headers(&r), [("X-A", "1"), ("x-a", "2")]);
}

#[test]
fn multipart_form() {
    let r = parse(
        "curl -F 'name=Ann O' -F 'photo=@/tmp/me.jpg' -F 'doc=@notes.bin;type=text/plain;filename=n.txt' --form-string 'raw=@literal;type=x' https://x.test/up",
    );
    assert_eq!(r.method, "POST");
    let b = "----AkevFormBoundary7MA4YWxkTrZu0gW";
    assert_eq!(
        r.headers.get("content-type"),
        Some(format!("multipart/form-data; boundary={b}").as_str())
    );
    let expected = format!(
        "--{b}\r\nContent-Disposition: form-data; name=\"name\"\r\n\r\nAnn O\r\n\
         --{b}\r\nContent-Disposition: form-data; name=\"photo\"; filename=\"me.jpg\"\r\nContent-Type: image/jpeg\r\n\r\n\r\n\
         --{b}\r\nContent-Disposition: form-data; name=\"doc\"; filename=\"n.txt\"\r\nContent-Type: text/plain\r\n\r\n\r\n\
         --{b}\r\nContent-Disposition: form-data; name=\"raw\"\r\n\r\n@literal;type=x\r\n\
         --{b}--\r\n"
    );
    assert_eq!(String::from_utf8_lossy(body(&r)), expected);

    let r = parse("curl -F 'a=b;type=text/x' -H 'Content-Type: multipart/mixed' https://x.test/");
    assert_eq!(
        r.headers.get("content-type"),
        Some(format!("multipart/mixed; boundary={b}").as_str())
    );
    assert!(String::from_utf8_lossy(body(&r)).contains("Content-Type: text/x\r\n\r\nb\r\n"));

    let r = parse("curl -F 'a=keeps;semicolons' https://x.test/");
    assert!(String::from_utf8_lossy(body(&r)).contains("\r\n\r\nkeeps;semicolons\r\n"));
}

#[test]
fn ignored_options_consume_values() {
    let r = parse(
        "curl -sSLk --compressed -x http://proxy:8080 --max-time 10 -o out.json --connect-timeout 5 --proxy-header 'X: y' --retry 3 -w '%{http_code}' --unknown-flag -v --http2 https://x.test/real",
    );
    assert_eq!(r.url, "https://x.test/real");
    assert_eq!(
        headers(&r),
        [("Accept-Encoding", "gzip, deflate, br, zstd")]
    );
    let r = parse("curl --no-compressed --compressed --no-compressed https://x.test/");
    assert!(r.headers.is_empty());
}

#[test]
fn short_option_clusters() {
    let r = parse("curl -sXPOST -H'X: y' -d@data.json -uuser:pw https://x.test/");
    assert_eq!(r.method, "POST");
    assert_eq!(r.headers.get("x"), Some("y"));
    assert_eq!(body(&r), b"@data.json");
    assert_eq!(r.headers.get("authorization"), Some("Basic dXNlcjpwdw=="));
}

#[test]
fn urls() {
    assert_eq!(
        parse("curl example.com/path").url,
        "http://example.com/path"
    );
    assert_eq!(
        parse("curl --url https://a.test/ -H 'A: b'").url,
        "https://a.test/"
    );
    assert_eq!(
        parse("curl https://first.test/ https://second.test/").url,
        "https://first.test/"
    );
    assert_eq!(
        parse("curl https://a.test/ --next https://b.test/ -d x").method,
        "GET"
    );
    let r = parse("curl --url-query 'a=b c' --url-query '+raw=%20' 'https://x.test/p'");
    assert_eq!(r.url, "https://x.test/p?a=b%20c&raw=%20");
    assert_eq!(
        parse("curl --header=X:\\ y https://x.test/")
            .headers
            .get("x"),
        Some("y")
    );
}

#[test]
fn shell_context() {
    for cmd in [
        "sudo curl https://x.test/",
        "$ curl https://x.test/",
        "/usr/bin/curl https://x.test/",
        "FOO=bar curl https://x.test/",
        "time curl -s https://x.test/ | jq .",
        "curl https://x.test/ > out.json 2>&1",
        "# fetch it\ncurl https://x.test/ # trailing comment",
        "cd /tmp && curl https://x.test/",
    ] {
        assert_eq!(parse(cmd).url, "https://x.test/", "{cmd}");
    }
    let r =
        parse("echo '{\"a\":1}' | curl -H 'Content-Type: application/json' -d @- https://x.test/");
    assert_eq!(body(&r), br#"{"a":1}"#);
    let r = parse("printf '%s' 'line1\nline2' | curl --data-binary @- https://x.test/");
    assert_eq!(body(&r), b"line1\nline2");
    let r = parse("echo 'iVBORw0K' | base64 --decode | curl --data-binary @- https://x.test/");
    assert_eq!(body(&r), [0x89, b'P', b'N', b'G', b'\r', b'\n']);
    let r = parse("cat data.json | curl --data-binary @- https://x.test/");
    assert_eq!(body(&r), b"@-");
}

#[test]
fn quoting_edge_cases() {
    let r = parse(
        r#"curl -H "Content-Type: application/json" -d "{\"a\": \"b \$HOME \`x\`\"}" https://x.test/"#,
    );
    assert_eq!(body(&r), br#"{"a": "b $HOME `x`"}"#);
    let r = parse("curl -d 'it'\\''s' -d $'tab\\there' -d \"\" https://x.test/");
    assert_eq!(body(&r), b"it's&tab\there&");
    let r = parse("curl \\\r\n  https://x.test/ \\\r\n  -H 'A: b'");
    assert_eq!(r.headers.get("a"), Some("b"));
    let r = parse("  \n curl\thttps://x.test/  \n");
    assert_eq!(r.url, "https://x.test/");
}

#[test]
fn errors() {
    assert_eq!(parse_curl("wget https://x.test/"), Err(ParseError::NotCurl));
    assert_eq!(parse_curl(""), Err(ParseError::NotCurl));
    assert_eq!(parse_curl("echo curl is great"), Err(ParseError::NotCurl));
    assert_eq!(parse_curl("curl"), Err(ParseError::MissingUrl));
    assert_eq!(parse_curl("curl -H 'A: b'"), Err(ParseError::MissingUrl));
    assert_eq!(
        parse_curl("curl 'https://x.test/"),
        Err(ParseError::UnterminatedQuote)
    );
    assert_eq!(parse_curl("curl $'abc"), Err(ParseError::UnterminatedQuote));
    assert!(matches!(
        parse_curl("curl https://x.test/ -H"),
        Err(ParseError::InvalidArgument(_))
    ));
    assert!(matches!(
        parse_curl("curl https://x.test/ --data"),
        Err(ParseError::InvalidArgument(_))
    ));
    assert!(matches!(
        parse_curl("curl -F a=b -d c https://x.test/"),
        Err(ParseError::InvalidArgument(_))
    ));
}

#[test]
fn never_panics_on_garbage() {
    let samples = [
        "curl ^",
        "curl `",
        "curl \\",
        "curl $'\\",
        "curl $'\\x",
        "curl $'\\u",
        "curl $'\\c",
        "curl -",
        "curl --",
        "curl ---",
        "curl -F",
        "curl -F = https://x",
        "curl -F '=@' x",
        "curl.exe \"`",
        "curl.exe \"`u{zz}\" x",
        "curl ^\"",
        "curl '' ''",
        "curl -b = x",
        "curl -u : x",
        "curl --url",
        "curl é ü",
        "curl \u{0} x",
        "curl -d @- x",
        "| curl x",
        "curl x |",
        "curl -H ': v' x",
        "curl --json @- x",
        "curl -T - x",
    ];
    for s in samples {
        let _ = parse_curl(s);
    }
}

/// Deterministic mutation fuzzing: every prefix of every generated cURL command, and seeded
/// random insertions/deletions of quoting and shell metacharacters, must parse or fail cleanly.
#[test]
fn never_panics_on_mutated_commands() {
    const SPECIAL: [char; 20] = [
        '^', '`', '\'', '"', '\\', '$', '\n', '\r', '|', '&', ';', '>', '<', '%', '@', '=', ' ',
        '-', 'é', '\u{0}',
    ];
    let mut seeds: Vec<String> = Vec::new();
    for (_, req) in round_trip_requests() {
        seeds.push(generate(Target::Curl, &req));
        seeds.push(generate(Target::CurlWindows, &req));
    }
    seeds.push("curl.exe 'https://x.test/' -H 'A: b' `\n  --data-raw '{\"a\":1}'".into());
    seeds.push("curl $'a\\x41\\u00e9\\c[' -F 'f=@\"x;y\";type=a/b' -b c=d -u u:p".into());
    let mut state: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for seed in &seeds {
        let boundaries: Vec<usize> = seed.char_indices().map(|(i, _)| i).collect();
        for &i in boundaries.iter().step_by(1 + seed.len() / 400) {
            let _ = parse_curl(&seed[..i]);
        }
        for _ in 0..40 {
            let mut chars: Vec<char> = seed.chars().collect();
            for _ in 0..1 + next() % 4 {
                let at = (next() as usize) % (chars.len() + 1);
                if next() % 3 == 0 && at < chars.len() {
                    chars.remove(at);
                } else {
                    chars.insert(at, SPECIAL[(next() as usize) % SPECIAL.len()]);
                }
            }
            let mutated: String = chars.into_iter().collect();
            let _ = parse_curl(&mutated);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Round trips

/// What the generator promises to preserve: normalised method, URL, sanitised headers (except
/// `Accept-Encoding`, which becomes `--compressed`), and the body bytes.
fn expected_after_round_trip(
    req: &SnippetRequest,
) -> (String, String, Vec<(String, String)>, bool, Vec<u8>) {
    const DROPPED: [&str; 12] = [
        "content-length",
        "content-encoding",
        "connection",
        "keep-alive",
        "proxy-connection",
        "proxy-authorization",
        "transfer-encoding",
        "te",
        "trailer",
        "upgrade",
        "expect",
        "accept-encoding",
    ];
    let host_matches = |v: &str| {
        url::Url::parse(&req.url).ok().is_some_and(|u| {
            let authority = match u.port() {
                Some(p) => format!("{}:{p}", u.host_str().unwrap_or_default()),
                None => u.host_str().unwrap_or_default().to_string(),
            };
            authority.eq_ignore_ascii_case(v)
        })
    };
    let headers = req
        .headers
        .iter()
        .filter(|(n, v)| {
            !n.starts_with(':')
                && !DROPPED.contains(&n.to_ascii_lowercase().as_str())
                && !(n.eq_ignore_ascii_case("host") && host_matches(v))
        })
        .map(|(n, v)| (n.to_string(), v.to_string()))
        .collect();
    let method = match req.method.to_ascii_uppercase().as_str() {
        m @ ("GET" | "POST" | "PUT" | "DELETE" | "HEAD" | "OPTIONS" | "PATCH" | "TRACE"
        | "CONNECT") => m.to_string(),
        _ => req.method.clone(),
    };
    let compressed = req.headers.contains("accept-encoding");
    (
        method,
        req.url.clone(),
        headers,
        compressed,
        req.body.as_deref().unwrap_or_default().to_vec(),
    )
}

fn actual(parsed: &SnippetRequest) -> (String, String, Vec<(String, String)>, bool, Vec<u8>) {
    let compressed = parsed.headers.get("accept-encoding") == Some("gzip, deflate, br, zstd");
    let headers = parsed
        .headers
        .iter()
        .filter(|(n, _)| !n.eq_ignore_ascii_case("accept-encoding"))
        .map(|(n, v)| (n.to_string(), v.to_string()))
        .collect();
    (
        parsed.method.clone(),
        parsed.url.clone(),
        headers,
        compressed,
        parsed.body.as_deref().unwrap_or_default().to_vec(),
    )
}

fn round_trip_requests() -> Vec<(String, SnippetRequest)> {
    let r = common::request;
    let mut all: Vec<(String, SnippetRequest)> = common::cases()
        .into_iter()
        .map(|c| (c.name.to_string(), c.request))
        .collect();
    let extra = [
        ("head", r("HEAD", "https://x.test/h", &[("X", "1")], None)),
        (
            "post_no_body",
            r(
                "POST",
                "https://x.test/",
                &[("Content-Type", "application/json")],
                None,
            ),
        ),
        (
            "get_with_body",
            r(
                "GET",
                "https://x.test/",
                &[("Content-Type", "text/plain")],
                Some(b"body"),
            ),
        ),
        (
            "at_sign",
            r(
                "POST",
                "https://x.test/",
                &[("Content-Type", "text/plain")],
                Some(b"@/etc/passwd"),
            ),
        ),
        (
            "all_bytes",
            r(
                "POST",
                "https://x.test/",
                &[("Content-Type", "application/octet-stream")],
                Some(&(0u8..=255).collect::<Vec<_>>()),
            ),
        ),
        (
            "controls_text",
            r(
                "POST",
                "https://x.test/",
                &[("Content-Type", "text/plain")],
                Some("\u{1}\u{7f}\u{85}\u{202e}!\t\r\n'\"\\".as_bytes()),
            ),
        ),
        (
            "percent_cmd",
            r(
                "POST",
                "https://x.test/?a=%PATH%&b=^&c",
                &[
                    ("X-P", "%USERNAME% ^ & | < >"),
                    ("Content-Type", "text/plain"),
                ],
                Some(b"50% off %PATH% ^caret& \"quoted\" \\\"x\\\\\""),
            ),
        ),
        (
            "long_json",
            r(
                "POST",
                "https://x.test/",
                &[("Content-Type", "application/json")],
                Some(format!("{{\"k\":\"{}\"}}", "v".repeat(5000)).as_bytes()),
            ),
        ),
    ];
    all.extend(extra.into_iter().map(|(n, r)| (n.to_string(), r)));
    all
}

#[test]
fn curl_bash_round_trip() {
    for (name, req) in round_trip_requests() {
        let code = generate(Target::Curl, &req);
        let parsed = parse_curl(&code).unwrap_or_else(|e| panic!("{name}: {e}\n{code}"));
        assert_eq!(
            actual(&parsed),
            expected_after_round_trip(&req),
            "{name}:\n{code}"
        );
    }
}

#[test]
fn curl_windows_round_trip() {
    for (name, req) in round_trip_requests() {
        let code = generate(Target::CurlWindows, &req);
        let parsed = parse_curl(&code).unwrap_or_else(|e| panic!("{name}: {e}\n{code}"));
        let mut expected = expected_after_round_trip(&req);
        if code.contains("akev-body.bin") {
            // Bodies cmd can't carry are written to a file first.
            expected.4 = b"@akev-body.bin".to_vec();
        }
        assert_eq!(actual(&parsed), expected, "{name}:\n{code}");
    }
}
