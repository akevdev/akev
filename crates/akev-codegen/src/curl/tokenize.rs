//! Shell tokenizers: POSIX (bash/zsh), Windows `cmd.exe` + MSVCRT, and PowerShell.
//!
//! Words are byte strings (`$'\xff'` produces a raw byte). Control operators are kept as
//! tokens so the parser can pick the curl command out of a pipeline.

use crate::ParseError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Token {
    Word(Vec<u8>),
    /// `|`: the next command reads this one's output.
    Pipe,
    /// `;`, `&`, `&&`, `||`, or (cmd) an unescaped `&`/`|`/redirect ending the command.
    Separator,
    /// `>`, `>>`, `<`, `2>`, `&>`, …: the next word is a file name, not an argument.
    Redirect,
}

/// Which quoting rules the command follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Dialect {
    Posix,
    Cmd,
    PowerShell,
}

/// Guess the dialect: cmd when there are `^` continuations or `^"` escapes (Chrome's "Copy
/// as cURL (cmd)"), PowerShell when the command starts with `curl.exe` or uses backtick
/// continuations (Firefox's "Copy as cURL (Windows)"), POSIX otherwise.
pub(super) fn detect(input: &str) -> Dialect {
    let bytes = input.as_bytes();
    let caret_escape = bytes
        .windows(2)
        .any(|w| w[0] == b'^' && matches!(w[1], b'\n' | b'\r' | b'"'));
    if caret_escape {
        return Dialect::Cmd;
    }
    let first = input.split_whitespace().next().unwrap_or_default();
    let backtick_continuation = bytes
        .windows(2)
        .any(|w| w[0] == b'`' && matches!(w[1], b'\n' | b'\r'));
    if first.eq_ignore_ascii_case("curl.exe") || backtick_continuation {
        Dialect::PowerShell
    } else {
        Dialect::Posix
    }
}

pub(super) fn tokenize(input: &str, dialect: Dialect) -> Result<Vec<Token>, ParseError> {
    match dialect {
        Dialect::Posix => Posix::new(input).run(),
        Dialect::Cmd => Ok(tokenize_cmd(input)),
        Dialect::PowerShell => PowerShell::new(input).run(),
    }
}

fn push_char(word: &mut Vec<u8>, c: char) {
    let mut buf = [0u8; 4];
    word.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
}

/// Accumulates the current word; `None` means "no word started" (distinct from `''`).
#[derive(Default)]
struct WordBuf {
    word: Option<Vec<u8>>,
    /// Whether any part of the word was quoted (so `2>` digits aren't an fd).
    quoted: bool,
    tokens: Vec<Token>,
}

impl WordBuf {
    fn push(&mut self, c: char) {
        push_char(self.word.get_or_insert_with(Vec::new), c);
    }

    fn push_byte(&mut self, b: u8) {
        self.word.get_or_insert_with(Vec::new).push(b);
    }

    fn start_quoted(&mut self) {
        self.word.get_or_insert_with(Vec::new);
        self.quoted = true;
    }

    fn finish(&mut self) {
        if let Some(word) = self.word.take() {
            self.tokens.push(Token::Word(word));
        }
        self.quoted = false;
    }

    fn operator(&mut self, token: Token) {
        self.finish();
        self.tokens.push(token);
    }

    /// Start a redirect: an unquoted all-digit word right before it is a file descriptor.
    fn redirect(&mut self) {
        let is_fd = !self.quoted
            && self
                .word
                .as_ref()
                .is_some_and(|w| !w.is_empty() && w.iter().all(u8::is_ascii_digit));
        if is_fd {
            self.word = None;
        }
        self.operator(Token::Redirect);
    }
}

// ---------------------------------------------------------------------------------------------
// POSIX

struct Posix {
    chars: Vec<char>,
    pos: usize,
    buf: WordBuf,
}

impl Posix {
    fn new(input: &str) -> Self {
        Posix {
            chars: input.chars().collect(),
            pos: 0,
            buf: WordBuf::default(),
        }
    }

    fn peek(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).copied()
    }

    /// Length of a line break starting at `offset` (`\n` or `\r\n`), if any.
    fn newline_len(&self, offset: usize) -> usize {
        match (self.peek(offset), self.peek(offset + 1)) {
            (Some('\n'), _) => 1,
            (Some('\r'), Some('\n')) => 2,
            _ => 0,
        }
    }

    fn run(mut self) -> Result<Vec<Token>, ParseError> {
        while let Some(c) = self.peek(0) {
            match c {
                ' ' | '\t' | '\n' | '\r' => {
                    self.buf.finish();
                    self.pos += 1;
                }
                '\\' => {
                    let nl = self.newline_len(1);
                    if nl > 0 {
                        self.pos += 1 + nl;
                    } else if let Some(next) = self.peek(1) {
                        self.buf.push(next);
                        self.buf.quoted = true;
                        self.pos += 2;
                    } else {
                        self.pos += 1;
                    }
                }
                '\'' => self.single_quoted()?,
                '"' => self.double_quoted()?,
                '$' if self.peek(1) == Some('\'') => self.ansi_c_quoted()?,
                '$' if self.peek(1) == Some('"') => {
                    self.pos += 1;
                    self.double_quoted()?;
                }
                '#' if self.buf.word.is_none() => {
                    while self.peek(0).is_some_and(|c| c != '\n') {
                        self.pos += 1;
                    }
                }
                '|' => {
                    if self.peek(1) == Some('|') {
                        self.buf.operator(Token::Separator);
                        self.pos += 2;
                    } else {
                        self.buf.operator(Token::Pipe);
                        self.pos += 1;
                    }
                }
                '&' => match self.peek(1) {
                    Some('&') => {
                        self.buf.operator(Token::Separator);
                        self.pos += 2;
                    }
                    Some('>') => {
                        self.buf.redirect();
                        self.pos += 2;
                        if self.peek(0) == Some('>') {
                            self.pos += 1;
                        }
                    }
                    _ => {
                        self.buf.operator(Token::Separator);
                        self.pos += 1;
                    }
                },
                ';' => {
                    self.buf.operator(Token::Separator);
                    self.pos += 1;
                }
                '>' | '<' => {
                    self.buf.redirect();
                    self.pos += 1;
                    // `>>`, `>&`, `>|`, `<<<`, `<>`, `<&`
                    while self
                        .peek(0)
                        .is_some_and(|c| matches!(c, '>' | '<' | '&' | '|'))
                    {
                        self.pos += 1;
                    }
                }
                c => {
                    self.buf.push(c);
                    self.pos += 1;
                }
            }
        }
        self.buf.finish();
        Ok(self.buf.tokens)
    }

    fn single_quoted(&mut self) -> Result<(), ParseError> {
        self.buf.start_quoted();
        self.pos += 1;
        loop {
            match self.peek(0) {
                None => return Err(ParseError::UnterminatedQuote),
                Some('\'') => {
                    self.pos += 1;
                    return Ok(());
                }
                Some(c) => {
                    self.buf.push(c);
                    self.pos += 1;
                }
            }
        }
    }

    fn double_quoted(&mut self) -> Result<(), ParseError> {
        self.buf.start_quoted();
        self.pos += 1;
        loop {
            match self.peek(0) {
                None => return Err(ParseError::UnterminatedQuote),
                Some('"') => {
                    self.pos += 1;
                    return Ok(());
                }
                Some('\\') => {
                    let nl = self.newline_len(1);
                    if nl > 0 {
                        self.pos += 1 + nl;
                        continue;
                    }
                    match self.peek(1) {
                        Some(c @ ('$' | '`' | '"' | '\\')) => {
                            self.buf.push(c);
                            self.pos += 2;
                        }
                        _ => {
                            self.buf.push('\\');
                            self.pos += 1;
                        }
                    }
                }
                Some(c) => {
                    self.buf.push(c);
                    self.pos += 1;
                }
            }
        }
    }

    /// Read up to `max` digits in `radix`, returning the value and the number read.
    fn digits(&self, offset: usize, radix: u32, max: usize) -> (u32, usize) {
        let mut value = 0u32;
        let mut count = 0;
        while count < max {
            match self.peek(offset + count).and_then(|c| c.to_digit(radix)) {
                Some(d) => {
                    value = value.wrapping_mul(radix).wrapping_add(d);
                    count += 1;
                }
                None => break,
            }
        }
        (value, count)
    }

    /// `$'…'` (ANSI-C quoting), including `\xHH` raw bytes and `\u`/`\U` code points.
    fn ansi_c_quoted(&mut self) -> Result<(), ParseError> {
        self.buf.start_quoted();
        self.pos += 2;
        loop {
            let Some(c) = self.peek(0) else {
                return Err(ParseError::UnterminatedQuote);
            };
            self.pos += 1;
            match c {
                '\'' => return Ok(()),
                '\\' => {
                    let Some(e) = self.peek(0) else {
                        return Err(ParseError::UnterminatedQuote);
                    };
                    self.pos += 1;
                    match e {
                        'a' => self.buf.push_byte(0x07),
                        'b' => self.buf.push_byte(0x08),
                        'e' | 'E' => self.buf.push_byte(0x1b),
                        'f' => self.buf.push_byte(0x0c),
                        'n' => self.buf.push_byte(b'\n'),
                        'r' => self.buf.push_byte(b'\r'),
                        't' => self.buf.push_byte(b'\t'),
                        'v' => self.buf.push_byte(0x0b),
                        '\\' | '\'' | '"' | '?' => self.buf.push(e),
                        '0'..='7' => {
                            let (rest, n) = self.digits(0, 8, 2);
                            self.pos += n;
                            let first = e.to_digit(8).unwrap_or(0);
                            let value = first * 8u32.pow(n as u32) + rest;
                            self.buf.push_byte((value & 0xff) as u8);
                        }
                        'x' => {
                            let (value, n) = self.digits(0, 16, 2);
                            if n == 0 {
                                self.buf.push('\\');
                                self.buf.push('x');
                            } else {
                                self.pos += n;
                                self.buf.push_byte(value as u8);
                            }
                        }
                        'u' | 'U' => {
                            let (value, n) = self.digits(0, 16, if e == 'u' { 4 } else { 8 });
                            if n == 0 {
                                self.buf.push('\\');
                                self.buf.push(e);
                            } else {
                                self.pos += n;
                                if let Some(ch) = char::from_u32(value) {
                                    self.buf.push(ch);
                                }
                            }
                        }
                        'c' => {
                            if let Some(x) = self.peek(0) {
                                self.pos += 1;
                                let value = if x == '?' { 0x7f } else { (x as u32) & 0x1f };
                                self.buf.push_byte(value as u8);
                            }
                        }
                        other => {
                            self.buf.push('\\');
                            self.buf.push(other);
                        }
                    }
                }
                c => self.buf.push(c),
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// cmd.exe + MSVCRT

/// cmd.exe parsing happens in two layers: cmd itself processes `^` escapes, `"` quote state and
/// command separators, producing the command line handed to the program; curl.exe's C runtime
/// then splits that line into arguments.
fn tokenize_cmd(input: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    for (i, line) in cmd_command_lines(input).into_iter().enumerate() {
        if i > 0 {
            tokens.push(Token::Separator);
        }
        tokens.extend(crt_split(&line).into_iter().map(Token::Word));
    }
    tokens
}

/// Layer 1: cmd.exe. Returns the command lines (split at unescaped `&`, `|`, `>`, `<`).
fn cmd_command_lines(input: &str) -> Vec<String> {
    let chars: Vec<char> = input.chars().collect();
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut in_quotes = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        i += 1;
        if in_quotes {
            match c {
                '"' => {
                    in_quotes = false;
                    line.push(c);
                }
                '\r' => {}
                '\n' => line.push(' '),
                c => line.push(c),
            }
            continue;
        }
        if line.trim().is_empty() && starts_rem(&chars[i - 1..]) {
            // A `rem` comment: cmd ignores the rest of the line.
            while chars.get(i).is_some_and(|&c| c != '\n') {
                i += 1;
            }
            i += 1; // the line break
            line.clear();
            continue;
        }
        match c {
            '^' => {
                // `^` + line break: continuation; the first character of the next line is
                // taken literally (Chrome writes a newline in an argument as `^` + 2 newlines).
                if chars.get(i) == Some(&'\r') {
                    i += 1;
                }
                if chars.get(i) == Some(&'\n') {
                    i += 1;
                }
                if let Some(&next) = chars.get(i) {
                    if next != '\r' {
                        line.push(next);
                    }
                    i += 1;
                }
            }
            '"' => {
                in_quotes = true;
                line.push(c);
            }
            '\r' => {}
            '\n' => line.push(' '),
            '&' | '|' | '>' | '<' => {
                if !line.trim().is_empty() {
                    lines.push(std::mem::take(&mut line));
                }
                // A redirect target or the second command follows; neither belongs to curl,
                // but later commands might be the curl one, so keep splitting.
                if matches!(c, '>' | '<') {
                    // Skip the redirect target.
                    while chars
                        .get(i)
                        .is_some_and(|c| *c == ' ' || *c == '>' || *c == '&')
                    {
                        i += 1;
                    }
                    while chars
                        .get(i)
                        .is_some_and(|c| !c.is_whitespace() && !"&|<>".contains(*c))
                    {
                        i += 1;
                    }
                }
            }
            c => line.push(c),
        }
    }
    if !line.trim().is_empty() {
        lines.push(line);
    }
    lines
}

/// Does a command start here with the `rem` (or `@rem`) comment command?
fn starts_rem(chars: &[char]) -> bool {
    let chars = chars.strip_prefix(&['@']).unwrap_or(chars);
    match chars {
        [r, e, m, rest @ ..] => {
            r.eq_ignore_ascii_case(&'r')
                && e.eq_ignore_ascii_case(&'e')
                && m.eq_ignore_ascii_case(&'m')
                && rest.first().is_none_or(|c| c.is_whitespace())
        }
        _ => false,
    }
}

/// Layer 2: MSVCRT (post-2008) argument splitting.
fn crt_split(line: &str) -> Vec<Vec<u8>> {
    let chars: Vec<char> = line.chars().collect();
    let mut words = Vec::new();
    let mut word: Option<Vec<u8>> = None;
    let mut in_quotes = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\\' => {
                let start = i;
                while chars.get(i) == Some(&'\\') {
                    i += 1;
                }
                let count = i - start;
                let w = word.get_or_insert_with(Vec::new);
                if chars.get(i) == Some(&'"') {
                    w.extend(std::iter::repeat_n(b'\\', count / 2));
                    if count % 2 == 1 {
                        w.push(b'"');
                        i += 1;
                    }
                } else {
                    w.extend(std::iter::repeat_n(b'\\', count));
                }
                continue;
            }
            '"' => {
                let w = word.get_or_insert_with(Vec::new);
                if in_quotes && chars.get(i + 1) == Some(&'"') {
                    w.push(b'"');
                    i += 1;
                } else {
                    in_quotes = !in_quotes;
                }
            }
            ' ' | '\t' if !in_quotes => {
                if let Some(w) = word.take() {
                    words.push(w);
                }
            }
            c => push_char(word.get_or_insert_with(Vec::new), c),
        }
        i += 1;
    }
    if let Some(w) = word {
        words.push(w);
    }
    words
}

// ---------------------------------------------------------------------------------------------
// PowerShell

const PS_SINGLE: [char; 5] = ['\'', '\u{2018}', '\u{2019}', '\u{201A}', '\u{201B}'];
const PS_DOUBLE: [char; 4] = ['"', '\u{201C}', '\u{201D}', '\u{201E}'];

struct PowerShell {
    chars: Vec<char>,
    pos: usize,
    buf: WordBuf,
}

impl PowerShell {
    fn new(input: &str) -> Self {
        PowerShell {
            chars: input.chars().collect(),
            pos: 0,
            buf: WordBuf::default(),
        }
    }

    fn peek(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).copied()
    }

    fn run(mut self) -> Result<Vec<Token>, ParseError> {
        while let Some(c) = self.peek(0) {
            match c {
                ' ' | '\t' | '\n' | '\r' => {
                    self.buf.finish();
                    self.pos += 1;
                }
                '`' => {
                    self.pos += 1;
                    match self.peek(0) {
                        Some('\r') if self.peek(1) == Some('\n') => self.pos += 2,
                        Some('\n') => self.pos += 1,
                        Some(next) => {
                            self.buf.push(next);
                            self.pos += 1;
                        }
                        None => {}
                    }
                }
                c if PS_SINGLE.contains(&c) => self.single_quoted()?,
                c if PS_DOUBLE.contains(&c) => self.double_quoted()?,
                '#' if self.buf.word.is_none() => {
                    while self.peek(0).is_some_and(|c| c != '\n') {
                        self.pos += 1;
                    }
                }
                '|' | '&' if self.peek(1) == Some(c) => {
                    self.buf.operator(Token::Separator);
                    self.pos += 2;
                }
                '|' => {
                    self.buf.operator(Token::Pipe);
                    self.pos += 1;
                }
                ';' => {
                    self.buf.operator(Token::Separator);
                    self.pos += 1;
                }
                '>' | '<' => {
                    self.buf.redirect();
                    self.pos += 1;
                    while self
                        .peek(0)
                        .is_some_and(|c| matches!(c, '>' | '&' | '1' | '2'))
                    {
                        self.pos += 1;
                    }
                }
                c => {
                    self.buf.push(c);
                    self.pos += 1;
                }
            }
        }
        self.buf.finish();
        Ok(self.buf.tokens)
    }

    fn single_quoted(&mut self) -> Result<(), ParseError> {
        self.buf.start_quoted();
        self.pos += 1;
        loop {
            match self.peek(0) {
                None => return Err(ParseError::UnterminatedQuote),
                Some(q) if PS_SINGLE.contains(&q) => {
                    if self.peek(1).is_some_and(|n| PS_SINGLE.contains(&n)) {
                        self.buf.push(q);
                        self.pos += 2;
                    } else {
                        self.pos += 1;
                        return Ok(());
                    }
                }
                Some(c) => {
                    self.buf.push(c);
                    self.pos += 1;
                }
            }
        }
    }

    fn double_quoted(&mut self) -> Result<(), ParseError> {
        self.buf.start_quoted();
        self.pos += 1;
        loop {
            match self.peek(0) {
                None => return Err(ParseError::UnterminatedQuote),
                Some(q) if PS_DOUBLE.contains(&q) => {
                    if self.peek(1).is_some_and(|n| PS_DOUBLE.contains(&n)) {
                        self.buf.push(q);
                        self.pos += 2;
                    } else {
                        self.pos += 1;
                        return Ok(());
                    }
                }
                Some('`') => {
                    let Some(e) = self.peek(1) else {
                        return Err(ParseError::UnterminatedQuote);
                    };
                    self.pos += 2;
                    match e {
                        '0' => self.buf.push_byte(0),
                        'a' => self.buf.push_byte(0x07),
                        'b' => self.buf.push_byte(0x08),
                        'e' => self.buf.push_byte(0x1b),
                        'f' => self.buf.push_byte(0x0c),
                        'n' => self.buf.push_byte(b'\n'),
                        'r' => self.buf.push_byte(b'\r'),
                        't' => self.buf.push_byte(b'\t'),
                        'v' => self.buf.push_byte(0x0b),
                        'u' if self.peek(0) == Some('{') => {
                            let start = self.pos + 1;
                            let end = (start..self.chars.len()).find(|&i| self.chars[i] == '}');
                            if let Some(end) = end {
                                let hex: String = self.chars[start..end].iter().collect();
                                if let Some(ch) =
                                    u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32)
                                {
                                    self.buf.push(ch);
                                }
                                self.pos = end + 1;
                            } else {
                                self.buf.push('u');
                            }
                        }
                        '\r' if self.peek(0) == Some('\n') => self.pos += 1,
                        '\n' => {}
                        other => self.buf.push(other),
                    }
                }
                Some(c) => {
                    self.buf.push(c);
                    self.pos += 1;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(input: &str) -> Vec<String> {
        tokenize(input, detect(input))
            .unwrap()
            .into_iter()
            .map(|t| match t {
                Token::Word(w) => String::from_utf8_lossy(&w).into_owned(),
                Token::Pipe => "<|>".into(),
                Token::Separator => "<;>".into(),
                Token::Redirect => "<>>".into(),
            })
            .collect()
    }

    #[test]
    fn posix_quoting() {
        assert_eq!(
            words(r#"curl 'a b' "c \"d\" \$e \x" f\ g '' """#),
            ["curl", "a b", "c \"d\" $e \\x", "f g", "", ""]
        );
        assert_eq!(words("curl 'it'\\''s'"), ["curl", "it's"]);
        assert_eq!(words("curl \\\n  -H x \\\r\n  y"), ["curl", "-H", "x", "y"]);
        assert_eq!(words("curl \"a\\\nb\""), ["curl", "ab"]);
        assert_eq!(words("curl a#b # comment\n c"), ["curl", "a#b", "c"]);
        assert_eq!(words("curl $\"x\""), ["curl", "x"]);
    }

    #[test]
    fn ansi_c_quoting() {
        let toks = tokenize(
            r"$'a\nb\t\'\x41\101\u00e9\U0001F600\cA\e\\\xff\q'",
            Dialect::Posix,
        )
        .unwrap();
        let Token::Word(w) = &toks[0] else { panic!() };
        let mut expected = b"a\nb\t'AA".to_vec();
        expected.extend("é😀".as_bytes());
        expected.extend([1, 0x1b, b'\\', 0xff, b'\\', b'q']);
        assert_eq!(w, &expected);
        assert_eq!(
            tokenize("$'abc", Dialect::Posix),
            Err(ParseError::UnterminatedQuote)
        );
    }

    #[test]
    fn posix_operators() {
        assert_eq!(
            words("echo x | curl -d @- u"),
            ["echo", "x", "<|>", "curl", "-d", "@-", "u"]
        );
        assert_eq!(
            words("curl u > out.txt 2>/dev/null; echo"),
            [
                "curl",
                "u",
                "<>>",
                "out.txt",
                "<>>",
                "/dev/null",
                "<;>",
                "echo"
            ]
        );
        assert_eq!(
            words("curl u && ls || x & y"),
            ["curl", "u", "<;>", "ls", "<;>", "x", "<;>", "y"]
        );
        assert_eq!(words("curl '2'>f"), ["curl", "2", "<>>", "f"]);
        assert_eq!(
            tokenize("curl 'x", Dialect::Posix),
            Err(ParseError::UnterminatedQuote)
        );
        assert_eq!(
            tokenize("curl \"x", Dialect::Posix),
            Err(ParseError::UnterminatedQuote)
        );
    }

    #[test]
    fn cmd_quoting() {
        let input = "curl ^\"https://x/?a=1^&b=2^\" ^\n  -H ^\"a: ^\\^\"q^\\^\"^\" ^\n  --data-raw ^\"line1^\n\nline2 50^%^off^\"";
        assert_eq!(detect(input), Dialect::Cmd);
        assert_eq!(
            words(input),
            [
                "curl",
                "https://x/?a=1&b=2",
                "-H",
                "a: \"q\"",
                "--data-raw",
                "line1\nline2 50%off"
            ]
        );
        let simple = "curl \"https://x/?a&b\" ^\n  -H \"Accept: */*\" ^\n  --data-raw \"{\\\"a\\\":\\\"C:\\\\\\\\\\\"}\"";
        assert_eq!(
            words(simple),
            [
                "curl",
                "https://x/?a&b",
                "-H",
                "Accept: */*",
                "--data-raw",
                "{\"a\":\"C:\\\\\"}"
            ]
        );
        assert_eq!(
            crt_split(r#"a "b ""c"" d" e\\"f g" h\i"#),
            [
                b"a".to_vec(),
                b"b \"c\" d".to_vec(),
                b"e\\f g".to_vec(),
                b"h\\i".to_vec()
            ]
        );
        assert_eq!(
            words("curl ^\nx & echo y"),
            ["curl", "x", "<;>", "echo", "y"]
        );
    }

    #[test]
    fn cmd_rem_lines_are_comments() {
        // `rem` (any case, `@` allowed) comments out the rest of its line, `&` included.
        assert_eq!(
            cmd_command_lines("rem Note: a & b | c\r\ncurl ^\"x^\""),
            ["curl \"x\""]
        );
        assert_eq!(cmd_command_lines("  @REM\ncurl x"), ["curl x"]);
        assert_eq!(cmd_command_lines("rem"), Vec::<String>::new());
        // Only as a command, and only the whole word.
        assert_eq!(cmd_command_lines("remote x"), ["remote x"]);
        assert_eq!(cmd_command_lines("curl rem x"), ["curl rem x"]);
    }

    #[test]
    fn powershell_quoting() {
        let input =
            "curl.exe \"https://x/\" `\n -H 'it''s' --data-raw \"{\"\"a\"\":`$1`n}\" -H ‘q’";
        assert_eq!(detect(input), Dialect::PowerShell);
        assert_eq!(
            words(input),
            [
                "curl.exe",
                "https://x/",
                "-H",
                "it's",
                "--data-raw",
                "{\"a\":$1\n}",
                "-H",
                "q"
            ]
        );
        assert_eq!(
            tokenize("curl.exe 'x", Dialect::PowerShell),
            Err(ParseError::UnterminatedQuote)
        );
    }
}
