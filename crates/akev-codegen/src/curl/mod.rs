//! cURL command import: tokenize (POSIX / cmd / PowerShell), find the curl invocation, and
//! interpret its options the way curl does.

mod options;
mod tokenize;

use crate::{ParseError, SnippetRequest};
use akev_core::{Headers, header_value_to_string};
use base64::Engine as _;
use bytes::Bytes;
use options::{DataKind, Opt};
use tokenize::Token;

/// Fixed multipart boundary for `-F`, so imports are deterministic.
const FORM_BOUNDARY: &str = "----AkevFormBoundary7MA4YWxkTrZu0gW";

/// What `--compressed` makes curl send (with every encoding a modern curl supports).
const COMPRESSED_ACCEPT_ENCODING: &str = "gzip, deflate, br, zstd";

pub(crate) fn parse(command: &str) -> Result<SnippetRequest, ParseError> {
    let dialect = tokenize::detect(command);
    let tokens = tokenize::tokenize(command, dialect)?;
    let segments = split_commands(tokens);
    let (index, start) = segments
        .iter()
        .enumerate()
        .find_map(|(i, s)| curl_position(&s.words).map(|pos| (i, pos + 1)))
        .ok_or(ParseError::NotCurl)?;
    let mut builder = Builder {
        stdin: stdin_for(&segments, index),
        ..Builder::default()
    };
    builder.parse_args(&segments[index].words[start..])?;
    builder.finish()
}

/// One simple command of a command line.
struct Command {
    words: Vec<Vec<u8>>,
    /// Its stdin is the previous command's stdout.
    piped: bool,
}

fn split_commands(tokens: Vec<Token>) -> Vec<Command> {
    let mut commands = Vec::new();
    let mut current = Command {
        words: Vec::new(),
        piped: false,
    };
    let mut skip_next_word = false;
    for token in tokens {
        match token {
            Token::Word(word) => {
                if !std::mem::take(&mut skip_next_word) {
                    current.words.push(word);
                }
            }
            Token::Redirect => skip_next_word = true,
            Token::Pipe | Token::Separator => {
                let piped = token == Token::Pipe;
                commands.push(std::mem::replace(
                    &mut current,
                    Command {
                        words: Vec::new(),
                        piped,
                    },
                ));
            }
        }
    }
    commands.push(current);
    commands
}

fn program_name(word: &[u8]) -> String {
    let word = String::from_utf8_lossy(word);
    let base = word.rsplit(['/', '\\']).next().unwrap_or_default();
    base.to_ascii_lowercase()
}

/// Index of the `curl` word, allowing harmless prefixes (`sudo`, `time`, a `$` prompt,
/// `VAR=value` assignments, and their options).
fn curl_position(words: &[Vec<u8>]) -> Option<usize> {
    const PREFIXES: [&str; 11] = [
        "$", "%", "sudo", "time", "command", "exec", "nohup", "env", "builtin", "noglob", "ps>",
    ];
    for (i, word) in words.iter().enumerate() {
        let name = program_name(word);
        if name == "curl" || name == "curl.exe" {
            return Some(i);
        }
        let text = String::from_utf8_lossy(word);
        let assignment = text.split_once('=').is_some_and(|(var, _)| {
            !var.is_empty() && var.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        });
        let option = i > 0 && text.starts_with('-');
        if !(PREFIXES.contains(&name.as_str()) || assignment || option) {
            return None;
        }
    }
    None
}

/// Work out what the curl command reads on stdin when it's at the end of a pipeline of simple
/// producers (`echo`, `printf`, `cat`, `base64 --decode`).
fn stdin_for(commands: &[Command], index: usize) -> Option<Vec<u8>> {
    let mut first = index;
    while first > 0 && commands[first].piped {
        first -= 1;
    }
    if first == index {
        return None;
    }
    let mut data: Option<Vec<u8>> = None;
    for command in &commands[first..index] {
        data = Some(run_producer(&command.words, data)?);
    }
    data
}

fn run_producer(words: &[Vec<u8>], input: Option<Vec<u8>>) -> Option<Vec<u8>> {
    let (program, args) = words.split_first()?;
    match program_name(program).as_str() {
        "echo" => {
            let mut args = args;
            let mut newline = true;
            while let Some((flag, rest)) = args.split_first() {
                match flag.as_slice() {
                    b"-n" => newline = false,
                    b"-E" => {}
                    _ => break,
                }
                args = rest;
            }
            let mut out = args.join(&b' ');
            if newline {
                out.push(b'\n');
            }
            Some(out)
        }
        "printf" => {
            let (format, rest) = args.split_first()?;
            if format == b"%s" {
                Some(rest.concat())
            } else if rest.is_empty() && !format.windows(2).any(|w| w[0] == b'%' && w[1] != b'%') {
                Some(printf_unescape(format))
            } else {
                None
            }
        }
        "cat" if args.is_empty() => input,
        "base64" => {
            let decode = args.iter().any(|a| {
                a == b"--decode"
                    || (a.starts_with(b"-")
                        && !a.starts_with(b"--")
                        && a[1..].iter().any(|&b| b == b'd' || b == b'D'))
            });
            if !decode {
                return None;
            }
            let text: Vec<u8> = input?
                .into_iter()
                .filter(|b| !b.is_ascii_whitespace())
                .collect();
            base64::engine::general_purpose::STANDARD.decode(text).ok()
        }
        _ => None,
    }
}

/// Backslash escapes and `%%` in a `printf` format without conversions.
fn printf_unescape(format: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(format.len());
    let mut i = 0;
    while i < format.len() {
        match format[i] {
            b'\\' if i + 1 < format.len() => {
                i += 1;
                match format[i] {
                    b'n' => out.push(b'\n'),
                    b't' => out.push(b'\t'),
                    b'r' => out.push(b'\r'),
                    b'\\' => out.push(b'\\'),
                    b'"' => out.push(b'"'),
                    b'a' => out.push(0x07),
                    b'b' => out.push(0x08),
                    b'f' => out.push(0x0c),
                    b'v' => out.push(0x0b),
                    d @ b'0'..=b'7' => {
                        let mut value = u32::from(d - b'0');
                        let mut n = 1;
                        while n < 3
                            && i + 1 < format.len()
                            && (b'0'..=b'7').contains(&format[i + 1])
                        {
                            i += 1;
                            value = value * 8 + u32::from(format[i] - b'0');
                            n += 1;
                        }
                        out.push((value & 0xff) as u8);
                    }
                    other => {
                        out.push(b'\\');
                        out.push(other);
                    }
                }
            }
            b'%' if format.get(i + 1) == Some(&b'%') => {
                out.push(b'%');
                i += 1;
            }
            b => out.push(b),
        }
        i += 1;
    }
    out
}

/// curl's URL encoding (`curl_easy_escape`): everything but unreserved characters.
fn curl_escape(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for &b in bytes {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// One `-F` part.
struct FormPart {
    name: String,
    filename: Option<String>,
    content_type: Option<String>,
    headers: Vec<String>,
    content: Vec<u8>,
}

#[derive(Default)]
struct Builder {
    stdin: Option<Vec<u8>>,
    method: Option<String>,
    head: bool,
    get: bool,
    url: Option<String>,
    headers: Vec<(String, String)>,
    /// Names removed with `-H 'Name:'` (suppresses headers curl would add).
    removed: Vec<String>,
    data: Vec<u8>,
    has_data: bool,
    json: bool,
    form: Vec<FormPart>,
    user: Option<String>,
    bearer: Option<String>,
    non_basic_auth: bool,
    cookies: Vec<String>,
    user_agent: Option<String>,
    referer: Option<String>,
    compressed: bool,
    upload: Option<Option<Vec<u8>>>,
    url_queries: Vec<String>,
}

fn missing_value(option: &str) -> ParseError {
    ParseError::InvalidArgument(format!("option {option} requires a value"))
}

impl Builder {
    fn parse_args(&mut self, args: &[Vec<u8>]) -> Result<(), ParseError> {
        let mut i = 0;
        while i < args.len() {
            let arg = &args[i];
            i += 1;
            if let Some(long) = arg.strip_prefix(b"--") {
                if long.is_empty() {
                    continue;
                }
                let long = String::from_utf8_lossy(long);
                let (name, inline) = match long.split_once('=') {
                    Some((n, v))
                        if options::long(&long).is_none()
                            && options::long(n).is_some_and(Opt::takes_value) =>
                    {
                        (n.to_string(), Some(v.as_bytes().to_vec()))
                    }
                    _ => (long.to_string(), None),
                };
                let opt = match options::long(&name) {
                    Some(opt) => opt,
                    None => {
                        if let Some(opt) = name.strip_prefix("no-").and_then(options::long)
                            && opt.is_boolean()
                        {
                            self.negate(opt);
                        }
                        continue;
                    }
                };
                if opt == Opt::Next {
                    break;
                }
                let value = if opt.takes_value() {
                    match inline {
                        Some(v) => Some(v),
                        None => {
                            let v = args
                                .get(i)
                                .ok_or_else(|| missing_value(&format!("--{name}")))?;
                            i += 1;
                            Some(v.clone())
                        }
                    }
                } else {
                    None
                };
                self.apply(opt, value.unwrap_or_default());
            } else if arg.len() > 1 && arg[0] == b'-' {
                // A cluster of short options; a value-taking one consumes the rest of the
                // word, or the next word.
                let mut k = 1;
                while k < arg.len() {
                    let c = char::from(arg[k]);
                    let opt = if arg[k].is_ascii() {
                        options::short(c)
                    } else {
                        Opt::Ignored { takes_value: false }
                    };
                    k += 1;
                    if opt == Opt::Next {
                        return Ok(());
                    }
                    if opt.takes_value() {
                        let value = if k < arg.len() {
                            arg[k..].to_vec()
                        } else {
                            let v = args.get(i).ok_or_else(|| missing_value(&format!("-{c}")))?;
                            i += 1;
                            v.clone()
                        };
                        self.apply(opt, value);
                        break;
                    }
                    self.apply(opt, Vec::new());
                }
            } else if self.url.is_none() {
                self.url = Some(String::from_utf8_lossy(arg).into_owned());
            }
        }
        Ok(())
    }

    fn negate(&mut self, opt: Opt) {
        match opt {
            Opt::Head => self.head = false,
            Opt::Get => self.get = false,
            Opt::Compressed => self.compressed = false,
            _ => {}
        }
    }

    fn apply(&mut self, opt: Opt, value: Vec<u8>) {
        let text = || header_value_to_string(&value);
        match opt {
            Opt::Request => self.method = Some(text()),
            Opt::Header => self.header(&text()),
            Opt::Data(kind) => self.data(kind, &value),
            Opt::Form { literal } => {
                if let Some(part) = self.form_part(&value, literal) {
                    self.form.push(part);
                }
            }
            Opt::User => self.user = Some(text()),
            Opt::Oauth2Bearer => self.bearer = Some(text()),
            Opt::NonBasicAuth { .. } => self.non_basic_auth = true,
            Opt::Basic => self.non_basic_auth = false,
            Opt::Cookie => {
                let cookie = text();
                // Without `=` it's a cookie *file*, which we can't read.
                if cookie.contains('=') {
                    self.cookies
                        .push(cookie.trim().trim_end_matches(';').to_string());
                }
            }
            Opt::UserAgent => self.user_agent = Some(text()),
            Opt::Referer => {
                let referer = text();
                self.referer = Some(
                    referer
                        .strip_suffix(";auto")
                        .unwrap_or(&referer)
                        .to_string(),
                );
            }
            Opt::Head => self.head = true,
            Opt::Get => self.get = true,
            Opt::Compressed => self.compressed = true,
            Opt::Url => {
                if self.url.is_none() {
                    self.url = Some(text());
                }
            }
            Opt::UrlQuery => {
                let query = match value.strip_prefix(b"+") {
                    Some(raw) => String::from_utf8_lossy(raw).into_owned(),
                    None => self.urlencode_arg(&value),
                };
                self.url_queries.push(query);
            }
            Opt::UploadFile => {
                let content = (value == b"-").then(|| self.stdin.clone()).flatten();
                self.upload = Some(content);
            }
            Opt::Next | Opt::Ignored { .. } => {}
        }
    }

    fn header(&mut self, line: &str) {
        if line.starts_with('@') {
            return; // header file
        }
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim();
            let value = value.trim();
            if name.is_empty() {
                return;
            }
            if value.is_empty() {
                self.removed.push(name.to_string());
            } else {
                self.headers.push((name.to_string(), value.to_string()));
            }
        } else if let Some(name) = line.trim_end().strip_suffix(';') {
            let name = name.trim();
            if !name.is_empty() {
                self.headers.push((name.to_string(), String::new()));
            }
        }
    }

    /// Contents of `@name`: stdin for `@-` when known, otherwise the literal reference.
    fn file_data(&self, value: &[u8], strip_newlines: bool) -> Vec<u8> {
        match &self.stdin {
            Some(stdin) if value == b"@-" => {
                if strip_newlines {
                    stdin
                        .iter()
                        .copied()
                        .filter(|b| !matches!(b, b'\r' | b'\n'))
                        .collect()
                } else {
                    stdin.clone()
                }
            }
            _ => value.to_vec(),
        }
    }

    fn data(&mut self, kind: DataKind, value: &[u8]) {
        let piece = match kind {
            DataKind::Raw => value.to_vec(),
            DataKind::Ascii if value.starts_with(b"@") => self.file_data(value, true),
            DataKind::Binary | DataKind::Json if value.starts_with(b"@") => {
                self.file_data(value, false)
            }
            DataKind::UrlEncode => self.urlencode_arg(value).into_bytes(),
            DataKind::Ascii | DataKind::Binary | DataKind::Json => value.to_vec(),
        };
        if kind == DataKind::Json {
            self.json = true;
        } else if self.has_data {
            self.data.push(b'&');
        }
        self.data.extend_from_slice(&piece);
        self.has_data = true;
    }

    /// `--data-urlencode` / `--url-query` argument: `content`, `=content`, `name=content`,
    /// `@file`, `name@file`.
    fn urlencode_arg(&self, value: &[u8]) -> String {
        let split = value
            .iter()
            .position(|&b| b == b'=')
            .or_else(|| value.iter().position(|&b| b == b'@'));
        let (name, content) = match split {
            Some(i) if value[i] == b'@' => {
                let file = &value[i + 1..];
                match &self.stdin {
                    Some(stdin) if file == b"-" => (&value[..i], stdin.clone()),
                    _ => return String::from_utf8_lossy(value).into_owned(),
                }
            }
            Some(i) => (&value[..i], value[i + 1..].to_vec()),
            None => (&value[..0], value.to_vec()),
        };
        let encoded = curl_escape(&content);
        if name.is_empty() {
            encoded
        } else {
            format!("{}={encoded}", String::from_utf8_lossy(name))
        }
    }

    fn form_part(&self, value: &[u8], literal: bool) -> Option<FormPart> {
        let eq = value.iter().position(|&b| b == b'=')?;
        let name = String::from_utf8_lossy(&value[..eq]).into_owned();
        let rest = &value[eq + 1..];
        if literal {
            return Some(FormPart {
                name,
                filename: None,
                content_type: None,
                headers: Vec::new(),
                content: rest.to_vec(),
            });
        }
        let rest_text = String::from_utf8_lossy(rest);
        let is_file = rest_text.starts_with('@');
        let spec = if is_file {
            &rest_text[1..]
        } else {
            &rest_text[..]
        };
        let (main, params) = split_form_params(spec);
        let mut part = FormPart {
            name,
            filename: None,
            content_type: None,
            headers: Vec::new(),
            content: Vec::new(),
        };
        for (key, val) in params {
            match key.as_str() {
                "type" => part.content_type = Some(val),
                "filename" => part.filename = Some(val),
                "headers" => part.headers.push(val),
                _ => {}
            }
        }
        if is_file {
            if part.filename.is_none() {
                part.filename = Some(
                    main.rsplit(['/', '\\'])
                        .next()
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            if main == "-" {
                part.content = self.stdin.clone().unwrap_or_default();
            }
            if part.content_type.is_none() {
                part.content_type = Some(content_type_for(&main).to_string());
            }
        } else if params_were_split(spec, &main) {
            part.content = main.into_bytes();
        } else {
            // Keep the original bytes (not a lossy round trip) when nothing was parsed out.
            part.content = rest.to_vec();
        }
        Some(part)
    }

    fn has_header(&self, name: &str) -> bool {
        self.headers
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case(name))
    }

    fn is_removed(&self, name: &str) -> bool {
        self.removed.iter().any(|n| n.eq_ignore_ascii_case(name))
    }

    /// Add a header curl would generate, unless given or removed explicitly.
    fn add_default(&mut self, name: &str, value: String) {
        if !self.has_header(name) && !self.is_removed(name) {
            self.headers.push((name.to_string(), value));
        }
    }

    fn finish(mut self) -> Result<SnippetRequest, ParseError> {
        let mut url = self.url.take().ok_or(ParseError::MissingUrl)?;
        if url.trim().is_empty() {
            return Err(ParseError::MissingUrl);
        }
        if !url.contains("://") {
            url.insert_str(0, "http://");
        }
        if !self.form.is_empty() && self.has_data {
            return Err(ParseError::InvalidArgument(
                "--form cannot be combined with --data".into(),
            ));
        }

        let mut body = None;
        if self.has_data {
            let data = std::mem::take(&mut self.data);
            if self.get {
                append_query(&mut url, &String::from_utf8_lossy(&data));
            } else {
                body = Some(data);
            }
        }
        for query in std::mem::take(&mut self.url_queries) {
            append_query(&mut url, &query);
        }

        let has_body = body.is_some() || !self.form.is_empty();
        let method = match (&self.method, self.head, self.get) {
            (Some(m), _, _) => m.clone(),
            (None, true, _) => "HEAD".into(),
            (None, false, true) => "GET".into(),
            _ if has_body => "POST".into(),
            _ if self.upload.is_some() => "PUT".into(),
            _ => "GET".into(),
        };

        if let Some(bearer) = self.bearer.take() {
            self.add_default("Authorization", format!("Bearer {bearer}"));
        } else if let Some(user) = self.user.take().filter(|_| !self.non_basic_auth) {
            let credentials = if user.contains(':') {
                user
            } else {
                format!("{user}:")
            };
            let encoded = base64::engine::general_purpose::STANDARD.encode(credentials.as_bytes());
            self.add_default("Authorization", format!("Basic {encoded}"));
        }
        if let Some(ua) = self.user_agent.take().filter(|ua| !ua.is_empty()) {
            self.add_default("User-Agent", ua);
        }
        if let Some(referer) = self.referer.take().filter(|r| !r.is_empty()) {
            self.add_default("Referer", referer);
        }
        if !self.cookies.is_empty() {
            let jar = self.cookies.join("; ");
            match self
                .headers
                .iter_mut()
                .find(|(n, _)| n.eq_ignore_ascii_case("cookie"))
            {
                Some((_, existing)) => {
                    existing.push_str("; ");
                    existing.push_str(&jar);
                }
                None => self.add_default("Cookie", jar),
            }
        }
        if self.compressed {
            self.add_default("Accept-Encoding", COMPRESSED_ACCEPT_ENCODING.into());
        }

        if !self.form.is_empty() {
            let (boundary, bytes) = build_multipart(&self.form);
            let ct = self
                .headers
                .iter_mut()
                .find(|(n, _)| n.eq_ignore_ascii_case("content-type"));
            match ct {
                Some((_, value)) if !value.to_ascii_lowercase().contains("boundary=") => {
                    value.push_str(&format!("; boundary={boundary}"));
                }
                Some(_) => {}
                None => self.add_default(
                    "Content-Type",
                    format!("multipart/form-data; boundary={boundary}"),
                ),
            }
            body = Some(bytes);
        } else if self.json {
            self.add_default("Content-Type", "application/json".into());
            self.add_default("Accept", "application/json".into());
        } else if body.is_some() {
            self.add_default("Content-Type", "application/x-www-form-urlencoded".into());
        }
        if body.is_none()
            && let Some(upload) = self.upload.take()
        {
            body = upload;
        }

        Ok(SnippetRequest {
            method,
            url,
            headers: Headers(self.headers),
            body: body.filter(|b| !b.is_empty()).map(Bytes::from),
        })
    }
}

/// Append `query` to the URL's query string (before any fragment).
fn append_query(url: &mut String, query: &str) {
    if query.is_empty() {
        return;
    }
    let fragment = url.find('#').map(|i| url.split_off(i));
    let sep = match url.find('?') {
        None => "?",
        Some(_) if url.ends_with('?') || url.ends_with('&') => "",
        Some(_) => "&",
    };
    url.push_str(sep);
    url.push_str(query);
    if let Some(fragment) = fragment {
        url.push_str(&fragment);
    }
}

const FORM_PARAMS: [&str; 4] = ["type", "filename", "headers", "encoder"];

/// Split `value;type=x;filename=y` into the value and its recognised parameters. Only `;`
/// followed by a known parameter splits, so values containing `;` survive.
fn split_form_params(spec: &str) -> (String, Vec<(String, String)>) {
    let starts_param = |s: &str| {
        let s = s.trim_start().to_ascii_lowercase();
        FORM_PARAMS
            .iter()
            .any(|p| s.starts_with(p) && s[p.len()..].starts_with('='))
    };
    let (main, mut rest) = if let Some(quoted) = spec.strip_prefix('"') {
        let mut value = String::new();
        let mut chars = quoted.char_indices();
        let mut end = quoted.len();
        while let Some((i, c)) = chars.next() {
            match c {
                '\\' => {
                    if let Some((_, next)) = chars.next() {
                        value.push(next);
                    }
                }
                '"' => {
                    end = i + 1;
                    break;
                }
                c => value.push(c),
            }
        }
        (value, &quoted[end..])
    } else {
        let split = spec
            .match_indices(';')
            .map(|(i, _)| i)
            .find(|&i| starts_param(&spec[i + 1..]));
        match split {
            Some(i) => (spec[..i].to_string(), &spec[i..]),
            None => (spec.to_string(), ""),
        }
    };
    let mut params = Vec::new();
    while let Some(after) = rest.strip_prefix(';') {
        let next = after
            .match_indices(';')
            .map(|(i, _)| i)
            .find(|&i| starts_param(&after[i + 1..]));
        let (param, tail) = match next {
            Some(i) => (&after[..i], &after[i..]),
            None => (after, ""),
        };
        if let Some((key, val)) = param.split_once('=') {
            let val = val.trim();
            let val = val
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .unwrap_or(val);
            params.push((key.trim().to_ascii_lowercase(), val.to_string()));
        }
        rest = tail;
    }
    (main, params)
}

fn params_were_split(spec: &str, main: &str) -> bool {
    spec.len() != main.len()
}

/// curl's file-extension → content type guesses for `-F name=@file`.
fn content_type_for(path: &str) -> &'static str {
    let ext = path
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "gif" => "image/gif",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "txt" => "text/plain",
        "htm" | "html" => "text/html",
        "pdf" => "application/pdf",
        "xml" => "application/xml",
        "json" => "application/json",
        _ => "application/octet-stream",
    }
}

/// Quote a multipart parameter value (WHATWG: `"`, CR and LF percent-encoded).
fn quote_param(value: &str) -> String {
    value
        .replace('"', "%22")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

fn build_multipart(parts: &[FormPart]) -> (String, Vec<u8>) {
    let mut boundary = FORM_BOUNDARY.to_string();
    while parts.iter().any(|p| {
        p.content
            .windows(boundary.len())
            .any(|w| w == boundary.as_bytes())
    }) {
        boundary.push('x');
    }
    let mut body = Vec::new();
    for part in parts {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        let mut disposition = format!(
            "Content-Disposition: form-data; name=\"{}\"",
            quote_param(&part.name)
        );
        if let Some(filename) = &part.filename {
            disposition.push_str(&format!("; filename=\"{}\"", quote_param(filename)));
        }
        body.extend_from_slice(disposition.as_bytes());
        body.extend_from_slice(b"\r\n");
        if let Some(ct) = &part.content_type {
            body.extend_from_slice(format!("Content-Type: {ct}\r\n").as_bytes());
        }
        for header in &part.headers {
            body.extend_from_slice(header.as_bytes());
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(b"\r\n");
        body.extend_from_slice(&part.content);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (boundary, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_matches_curl() {
        assert_eq!(
            curl_escape(b"a b&c=d~e.f_g-h/\xff"),
            "a%20b%26c%3Dd~e.f_g-h%2F%FF"
        );
    }

    #[test]
    fn query_appending() {
        let mut url = "http://x/a".to_string();
        append_query(&mut url, "b=1");
        assert_eq!(url, "http://x/a?b=1");
        append_query(&mut url, "c=2");
        assert_eq!(url, "http://x/a?b=1&c=2");
        let mut url = "http://x/a?#frag".to_string();
        append_query(&mut url, "b=1");
        assert_eq!(url, "http://x/a?b=1#frag");
    }

    #[test]
    fn form_params() {
        assert_eq!(split_form_params("a;b"), ("a;b".to_string(), vec![]));
        assert_eq!(
            split_form_params("file.txt;type=text/plain;filename=\"x y.txt\""),
            (
                "file.txt".to_string(),
                vec![
                    ("type".into(), "text/plain".into()),
                    ("filename".into(), "x y.txt".into())
                ]
            )
        );
        assert_eq!(
            split_form_params("\"a;b\";type=x"),
            ("a;b".to_string(), vec![("type".into(), "x".into())])
        );
    }

    #[test]
    fn producers() {
        assert_eq!(
            run_producer(&[b"echo".to_vec(), b"a".to_vec(), b"b".to_vec()], None),
            Some(b"a b\n".to_vec())
        );
        assert_eq!(
            run_producer(&[b"echo".to_vec(), b"-n".to_vec(), b"a".to_vec()], None),
            Some(b"a".to_vec())
        );
        assert_eq!(
            run_producer(
                &[
                    b"printf".to_vec(),
                    b"%s".to_vec(),
                    b"x".to_vec(),
                    b"y".to_vec()
                ],
                None
            ),
            Some(b"xy".to_vec())
        );
        assert_eq!(
            run_producer(&[b"printf".to_vec(), b"a\\n\\101%%".to_vec()], None),
            Some(b"a\nA%".to_vec())
        );
        assert_eq!(
            run_producer(&[b"printf".to_vec(), b"%d".to_vec(), b"1".to_vec()], None),
            None
        );
        assert_eq!(
            run_producer(
                &[b"base64".to_vec(), b"-d".to_vec()],
                Some(b"aGk=\n".to_vec())
            ),
            Some(b"hi".to_vec())
        );
        assert_eq!(
            run_producer(&[b"base64".to_vec()], Some(b"hi".to_vec())),
            None
        );
        assert_eq!(run_producer(&[b"jq".to_vec()], Some(b"hi".to_vec())), None);
    }
}
