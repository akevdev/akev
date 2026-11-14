//! String-literal encoders for every target language.
//!
//! Conventions: functions return a complete literal including its delimiters. "Text" variants
//! pick the most readable safe form for multi-line bodies (raw strings, text blocks,
//! here-strings); the plain variants always produce a single-line escaped literal.

use std::fmt::Write as _;

/// Characters that never appear raw inside generated code: control characters other than
/// `\n`/`\t` (including `\r`, which most compilers normalise away), invisible formatting
/// characters, and the bidi controls behind "Trojan Source" attacks (which rustc rejects).
pub(crate) fn is_invisible(c: char) -> bool {
    match c {
        '\n' | '\t' => false,
        c if c.is_control() => true,
        '\u{00AD}'
        | '\u{061C}'
        | '\u{180E}'
        | '\u{200B}'..='\u{200F}'
        | '\u{2028}'..='\u{202E}'
        | '\u{2060}'..='\u{2064}'
        | '\u{2066}'..='\u{2069}'
        | '\u{FEFF}'
        | '\u{FFF9}'..='\u{FFFB}' => true,
        _ => false,
    }
}

pub(crate) fn has_invisible(s: &str) -> bool {
    s.chars().any(is_invisible)
}

fn push_hex(out: &mut String, prefix: &str, value: u32, width: usize, suffix: &str) {
    let _ = write!(out, "{prefix}{value:0width$X}{suffix}");
}

/// Prefer double quotes unless that needs more escapes than single quotes (Prettier/Black).
fn preferred_quote(s: &str, default: char) -> char {
    let doubles = s.matches('"').count();
    let singles = s.matches('\'').count();
    match default {
        '"' if doubles > singles => '\'',
        '\'' if singles > doubles => '"',
        q => q,
    }
}

/// Length of the longest run of `#` directly following any of `after` characters.
fn longest_hash_run_after(s: &str, after: &[char]) -> Option<usize> {
    let mut best = None;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if after.contains(&c) {
            let mut run = 0;
            while chars.peek() == Some(&'#') {
                chars.next();
                run += 1;
            }
            best = Some(best.map_or(run, |b: usize| b.max(run)));
        }
    }
    best
}

/// Escape a line for a delimited multi-line literal (Java text block, Swift `"""`):
/// backslashes doubled, any quote that follows a quote escaped (so `"""` can't occur), `\r`
/// and invisible characters escaped, and a trailing space/tab escaped so it survives
/// trailing-whitespace stripping.
fn escape_block_line(
    out: &mut String,
    line: &str,
    invisible: fn(&mut String, char),
    trailing_space: &str,
) {
    let start = out.len();
    let mut prev_quote = false;
    for c in line.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' if prev_quote => out.push_str("\\\""),
            '"' => out.push('"'),
            '\r' => out.push_str("\\r"),
            c if is_invisible(c) => invisible(out, c),
            c => out.push(c),
        }
        prev_quote = c == '"';
    }
    if out.len() > start {
        if out.ends_with(' ') {
            out.pop();
            out.push_str(trailing_space);
        } else if out.ends_with('\t') {
            out.pop();
            out.push_str("\\t");
        }
    }
}

// ---------------------------------------------------------------------------------------------
// POSIX shell

/// A POSIX shell word: single-quoted, or ANSI-C `$'…'` when the string holds tabs, `\r` or
/// other invisible characters (which a terminal paste could mangle).
pub(crate) fn sh(s: &str) -> String {
    if s.chars().any(|c| c == '\t' || is_invisible(c)) {
        return sh_ansi_c(s);
    }
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

fn sh_ansi_c(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 3);
    out.push_str("$'");
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Interactive bash's history expansion doesn't understand `\'` inside `$'…'`.
            '!' => out.push_str("\\x21"),
            c if is_invisible(c) => {
                let mut buf = [0u8; 4];
                for b in c.encode_utf8(&mut buf).bytes() {
                    push_hex(&mut out, "\\x", u32::from(b), 2, "");
                }
            }
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// A shell word, left bare when it only contains characters that are never special.
pub(crate) fn sh_word(s: &str) -> String {
    let safe = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c));
    if safe { s.to_string() } else { sh(s) }
}

// ---------------------------------------------------------------------------------------------
// Windows cmd.exe

/// Characters a `cmd.exe` argument can't carry at all: control characters other than tab and
/// line feed. cmd deletes every CR before it processes carets (so `^` + CR leaves a lone caret
/// that cancels the next escape, and an `&` after it runs a command), reads Ctrl-Z as a line
/// break in batch files, and ends the line at NUL.
pub(crate) fn cmd_unrepresentable(c: char) -> bool {
    c.is_control() && c != '\n' && c != '\t'
}

/// Quote `s` as one argument for a program started from `cmd.exe` (MSVCRT argument rules).
///
/// Strings are simply double-quoted (with MSVCRT `\"` escapes) when cmd can't misread them:
/// no `%` or newlines, and — if they contain `"`, which toggles cmd's own quote state — no cmd
/// metacharacters either. Others use Chrome's approach: the quotes themselves are
/// caret-escaped (`^"`) so cmd never enters quote mode, and every cmd metacharacter is
/// caret-escaped; `%` gets a caret after it so no environment variable can match, and a
/// newline becomes `^` + two newlines. Non-ASCII characters are never special to cmd and are
/// left as they are. Characters cmd can't carry ([`cmd_unrepresentable`]) become spaces: no
/// raw control character is ever written.
pub(crate) fn cmd(s: &str) -> String {
    /// Characters written without a caret.
    fn bare(c: char) -> bool {
        !c.is_ascii() || c.is_ascii_alphanumeric() || " \t_-:=+~'/.,?;()*`".contains(c)
    }

    let cleaned: String;
    let s = if s.chars().any(cmd_unrepresentable) {
        cleaned = s
            .chars()
            .map(|c| if cmd_unrepresentable(c) { ' ' } else { c })
            .collect();
        cleaned.as_str()
    } else {
        s
    };
    let inner = crt_inner(s);
    let simple = !s.contains(['%', '\n', '\r'])
        && (!s.contains('"') || !s.contains(['&', '|', '<', '>', '^']));
    if simple {
        return format!("\"{inner}\"");
    }
    let mut out = String::with_capacity(inner.len() * 2 + 4);
    out.push_str("^\"");
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\n' => out.push_str("^\n\n"),
            '%' => {
                out.push_str("^%");
                // `%^name` can't name a variable: carets are removed only after expansion.
                // (A character that gets its own caret is already protected.)
                if chars.peek().is_some_and(|&n| bare(n)) {
                    out.push('^');
                }
            }
            c if bare(c) => out.push(c),
            c => {
                out.push('^');
                out.push(c);
            }
        }
    }
    out.push_str("^\"");
    out
}

/// The inside of an MSVCRT double-quoted argument: `"` → `\"`, and backslashes doubled where
/// they precede a quote (including the closing one).
fn crt_inner(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    let mut backslashes = 0usize;
    for c in s.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            c => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', backslashes * 2));
    out
}

// ---------------------------------------------------------------------------------------------
// PowerShell

const PS_SINGLE_QUOTES: [char; 5] = ['\'', '\u{2018}', '\u{2019}', '\u{201A}', '\u{201B}'];
const PS_DOUBLE_QUOTES: [char; 4] = ['"', '\u{201C}', '\u{201D}', '\u{201E}'];

/// A PowerShell string: verbatim single-quoted (all quote look-alikes doubled, since
/// PowerShell treats typographic quotes as delimiters), or double-quoted with backtick escapes
/// when it contains characters single quotes can't express.
pub(crate) fn ps(s: &str) -> String {
    if has_invisible(s) {
        return ps_double(s);
    }
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if PS_SINGLE_QUOTES.contains(&c) {
            out.push(c);
        }
        out.push(c);
    }
    out.push('\'');
    out
}

fn ps_double(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '`' => out.push_str("``"),
            '$' => out.push_str("`$"),
            c if PS_DOUBLE_QUOTES.contains(&c) => {
                out.push('`');
                out.push(c);
            }
            '\n' => out.push_str("`n"),
            '\r' => out.push_str("`r"),
            '\t' => out.push_str("`t"),
            '\0' => out.push_str("`0"),
            c if is_invisible(c) => push_hex(&mut out, "`u{", c as u32, 1, "}"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A verbatim here-string (`@'…'@`) for multi-line text, when one can represent `s`.
pub(crate) fn ps_here(s: &str) -> Option<String> {
    if !s.contains('\n') || has_invisible(s) {
        return None;
    }
    let terminates = s.split('\n').any(|line| {
        let mut chars = line.trim_start().chars();
        matches!((chars.next(), chars.next()), (Some(q), Some('@')) if PS_SINGLE_QUOTES.contains(&q))
    });
    if terminates {
        return None;
    }
    Some(format!("@'\n{s}\n'@"))
}

// ---------------------------------------------------------------------------------------------
// JavaScript

/// A JavaScript string literal (double quotes unless single quotes need fewer escapes).
pub(crate) fn js(s: &str) -> String {
    let q = preferred_quote(s, '"');
    let mut out = String::with_capacity(s.len() + 2);
    out.push(q);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            c if c == q => {
                out.push('\\');
                out.push(c);
            }
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\u{b}' => out.push_str("\\v"),
            c if is_invisible(c) => push_js_escape(&mut out, c),
            c => out.push(c),
        }
    }
    out.push(q);
    out
}

fn push_js_escape(out: &mut String, c: char) {
    let v = c as u32;
    if v <= 0xFF {
        push_hex(out, "\\x", v, 2, "");
    } else {
        push_hex(out, "\\u", v, 4, "");
    }
}

/// A template literal, used for multi-line text.
pub(crate) fn js_template(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('`');
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.push_str("\\\\"),
            '`' => out.push_str("\\`"),
            '$' if chars.peek() == Some(&'{') => out.push_str("\\$"),
            c if is_invisible(c) => match c {
                '\r' => out.push_str("\\r"),
                c => push_js_escape(&mut out, c),
            },
            c => out.push(c),
        }
    }
    out.push('`');
    out
}

/// A body string: template literal when multi-line, plain literal otherwise.
pub(crate) fn js_text(s: &str) -> String {
    if s.contains('\n') {
        js_template(s)
    } else {
        js(s)
    }
}

/// An object-literal key: bare when it's an identifier. `__proto__` is computed so it defines
/// an own property instead of setting the prototype.
pub(crate) fn js_key(s: &str) -> String {
    if s == "__proto__" {
        return format!("[{}]", js(s));
    }
    let mut chars = s.chars();
    let ident = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
    if ident { s.to_string() } else { js(s) }
}

/// An object-literal key that is always quoted (header names).
pub(crate) fn js_quoted_key(s: &str) -> String {
    if s == "__proto__" {
        format!("[{}]", js(s))
    } else {
        js(s)
    }
}

// ---------------------------------------------------------------------------------------------
// Python

/// A Python string literal (double quotes unless single quotes need fewer escapes).
pub(crate) fn py(s: &str) -> String {
    let q = preferred_quote(s, '"');
    let mut out = String::with_capacity(s.len() + 2);
    out.push(q);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            c if c == q => {
                out.push('\\');
                out.push(c);
            }
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if is_invisible(c) => push_py_escape(&mut out, c),
            c => out.push(c),
        }
    }
    out.push(q);
    out
}

fn push_py_escape(out: &mut String, c: char) {
    match c {
        '\r' => out.push_str("\\r"),
        c if (c as u32) <= 0xFF => push_hex(out, "\\x", c as u32, 2, ""),
        c => push_hex(out, "\\u", c as u32, 4, ""),
    }
}

/// A body string: triple-quoted when multi-line, plain literal otherwise.
pub(crate) fn py_text(s: &str) -> String {
    if !s.contains('\n') {
        return py(s);
    }
    let mut out = String::with_capacity(s.len() + 6);
    out.push_str("\"\"\"");
    let mut chars = s.chars().peekable();
    let mut prev_quote = false;
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' if prev_quote || chars.peek().is_none() => out.push_str("\\\""),
            '\t' => out.push_str("\\t"),
            c if is_invisible(c) => push_py_escape(&mut out, c),
            c => out.push(c),
        }
        prev_quote = c == '"';
    }
    out.push_str("\"\"\"");
    out
}

// ---------------------------------------------------------------------------------------------
// Go

/// A Go interpreted string literal.
pub(crate) fn go(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // `\x` would emit a raw byte; non-ASCII must use `\u` to stay UTF-8.
            c if is_invisible(c) && (c as u32) < 0x80 => push_hex(&mut out, "\\x", c as u32, 2, ""),
            c if is_invisible(c) => push_hex(&mut out, "\\u", c as u32, 4, ""),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A body string: a raw string when it has quotes/backslashes/newlines and can be one (Go raw
/// strings drop `\r` and can't contain backticks or a BOM).
pub(crate) fn go_text(s: &str) -> String {
    if s.contains(['"', '\\', '\n']) && !s.contains('`') && !has_invisible(s) {
        format!("`{s}`")
    } else {
        go(s)
    }
}

// ---------------------------------------------------------------------------------------------
// Rust

/// A Rust string literal.
pub(crate) fn rust(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            c if is_invisible(c) => push_hex(&mut out, "\\u{", c as u32, 1, "}"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A body string: a raw string (`r#"…"#` with enough hashes) when it has quotes/backslashes
/// and can be one (raw strings can't hold `\r` or escapes).
pub(crate) fn rust_text(s: &str) -> String {
    if !s.contains(['"', '\\']) || has_invisible(s) {
        return rust(s);
    }
    let hashes = longest_hash_run_after(s, &['"']).map_or(0, |n| n + 1);
    let h = "#".repeat(hashes);
    format!("r{h}\"{s}\"{h}")
}

/// A byte-string literal.
pub(crate) fn rust_bytes(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2 + 3);
    out.push_str("b\"");
    for &b in bytes {
        match b {
            b'\\' => out.push_str("\\\\"),
            b'"' => out.push_str("\\\""),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            0 => out.push_str("\\0"),
            0x20..=0x7E => out.push(char::from(b)),
            b => push_hex(&mut out, "\\x", u32::from(b), 2, ""),
        }
    }
    out.push('"');
    out
}

// ---------------------------------------------------------------------------------------------
// Java

fn push_java_escape(out: &mut String, c: char) {
    let v = c as u32;
    if v <= 0o377 {
        // Always three octal digits so a following digit can't extend the escape.
        let _ = write!(out, "\\{v:03o}");
    } else {
        // Unicode escapes are translated before lexing; safe for these (non-terminator) chars.
        push_hex(out, "\\u", v, 4, "");
    }
}

/// A Java string literal.
pub(crate) fn java(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if is_invisible(c) => push_java_escape(&mut out, c),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A Java 15+ text block whose lines (and closing delimiter) are indented by `indent`.
///
/// The closing delimiter sits on its own line so it, not the content, determines the
/// incidental indentation; a final `\<newline>` suppresses the trailing newline when the text
/// has none. Trailing whitespace is escaped (`\s`) since text blocks strip it.
pub(crate) fn java_text_block(s: &str, indent: &str) -> String {
    let mut out = String::with_capacity(s.len() + 16);
    out.push_str("\"\"\"\n");
    let lines: Vec<&str> = s.split('\n').collect();
    let count = lines.len();
    for (i, line) in lines.into_iter().enumerate() {
        let last = i + 1 == count;
        if last && line.is_empty() {
            break;
        }
        if !line.is_empty() {
            out.push_str(indent);
            escape_block_line(&mut out, line, push_java_escape, "\\s");
        }
        if last {
            out.push('\\');
        }
        out.push('\n');
    }
    out.push_str(indent);
    out.push_str("\"\"\"");
    out
}

/// A body string: text block when it's multi-line or has quotes, plain literal otherwise.
pub(crate) fn java_text(s: &str, indent: &str) -> String {
    if s.contains(['\n', '"']) {
        java_text_block(s, indent)
    } else {
        java(s)
    }
}

// ---------------------------------------------------------------------------------------------
// Kotlin

/// A Kotlin string literal.
pub(crate) fn kotlin(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '$' => out.push_str("\\$"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            c if is_invisible(c) => push_hex(&mut out, "\\u", c as u32, 4, ""),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A body string: a raw string (`trimMargin()`-ed when multi-line) when it can be one, a plain
/// literal otherwise. `$` is written as `${'$'}` inside raw strings.
pub(crate) fn kotlin_text(s: &str, indent: &str) -> String {
    let raw_ok = !s.contains("\"\"\"") && !has_invisible(s);
    if raw_ok && s.contains('\n') {
        let mut out = String::from("\"\"\"\n");
        for line in s.split('\n') {
            out.push_str(indent);
            out.push('|');
            out.push_str(&line.replace('$', "${'$'}"));
            out.push('\n');
        }
        out.push_str(indent);
        out.push_str("\"\"\".trimMargin()");
        return out;
    }
    if raw_ok && s.contains(['"', '\\']) && !s.starts_with('"') && !s.ends_with('"') {
        return format!("\"\"\"{}\"\"\"", s.replace('$', "${'$'}"));
    }
    kotlin(s)
}

// ---------------------------------------------------------------------------------------------
// C#

/// A C# regular string literal.
pub(crate) fn csharp(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            // `\x` is variable-length in C#; `\u` is always exactly four digits.
            c if is_invisible(c) => push_hex(&mut out, "\\u", c as u32, 4, ""),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A body string: a multi-line raw string literal (C# 11) indented by `indent` when it has
/// quotes, backslashes or newlines and can be one, a regular literal otherwise.
pub(crate) fn csharp_text(s: &str, indent: &str) -> String {
    let wants_raw = s.contains(['"', '\\', '\n']);
    let raw_ok = !has_invisible(s)
        && !s
            .split('\n')
            .any(|l| !l.is_empty() && l.chars().all(char::is_whitespace));
    if !(wants_raw && raw_ok) {
        return csharp(s);
    }
    let mut longest = 0;
    let mut run = 0;
    for c in s.chars() {
        run = if c == '"' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    let delim = "\"".repeat((longest + 1).max(3));
    let mut out = String::with_capacity(s.len() + 16);
    out.push_str(&delim);
    out.push('\n');
    for line in s.split('\n') {
        if !line.is_empty() {
            out.push_str(indent);
            out.push_str(line);
        }
        out.push('\n');
    }
    out.push_str(indent);
    out.push_str(&delim);
    out
}

// ---------------------------------------------------------------------------------------------
// PHP

/// A PHP string: single-quoted, or double-quoted with escapes for invisible characters.
pub(crate) fn php(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    if has_invisible(s) {
        out.push('"');
        for c in s.chars() {
            match c {
                '\\' => out.push_str("\\\\"),
                '"' => out.push_str("\\\""),
                '$' => out.push_str("\\$"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                '\u{b}' => out.push_str("\\v"),
                '\u{c}' => out.push_str("\\f"),
                '\u{1b}' => out.push_str("\\e"),
                c if is_invisible(c) => push_hex(&mut out, "\\u{", c as u32, 1, "}"),
                c => out.push(c),
            }
        }
        out.push('"');
    } else {
        out.push('\'');
        for c in s.chars() {
            match c {
                '\\' => out.push_str("\\\\"),
                '\'' => out.push_str("\\'"),
                c => out.push(c),
            }
        }
        out.push('\'');
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Ruby

fn ruby_double(s: &str, literal_newlines: bool) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '#' if matches!(chars.peek(), Some('{' | '@' | '$')) => out.push_str("\\#"),
            '\n' if literal_newlines => out.push('\n'),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{1b}' => out.push_str("\\e"),
            c if is_invisible(c) => push_hex(&mut out, "\\u{", c as u32, 1, "}"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn ruby_single(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// A single-line Ruby string literal (single quotes unless escapes are needed).
pub(crate) fn ruby(s: &str) -> String {
    if has_invisible(s) || s.contains('\n') || (s.contains('\'') && !s.contains('"')) {
        ruby_double(s, false)
    } else {
        ruby_single(s)
    }
}

/// A body string; newlines stay literal.
pub(crate) fn ruby_text(s: &str) -> String {
    if has_invisible(s) || (s.contains('\'') && !s.contains('"')) {
        ruby_double(s, true)
    } else {
        ruby_single(s)
    }
}

// ---------------------------------------------------------------------------------------------
// Swift

fn push_swift_escape(out: &mut String, c: char) {
    push_hex(out, "\\u{", c as u32, 1, "}");
}

/// A Swift string literal.
pub(crate) fn swift(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            c if is_invisible(c) => push_swift_escape(&mut out, c),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A body string: a multi-line `"""` literal indented by `indent` when multi-line, a raw
/// `#"…"#` literal when it has quotes/backslashes, a plain literal otherwise.
pub(crate) fn swift_text(s: &str, indent: &str) -> String {
    if s.contains('\n') {
        let mut out = String::with_capacity(s.len() + 16);
        out.push_str("\"\"\"\n");
        for line in s.split('\n') {
            if !line.is_empty() {
                out.push_str(indent);
                escape_block_line(&mut out, line, push_swift_escape, "\\u{20}");
            }
            out.push('\n');
        }
        out.push_str(indent);
        out.push_str("\"\"\"");
        return out;
    }
    if s.contains(['"', '\\']) && !has_invisible(s) {
        let hashes = longest_hash_run_after(s, &['"', '\\']).map_or(1, |n| n + 1);
        let h = "#".repeat(hashes);
        return format!("{h}\"{s}\"{h}");
    }
    swift(s)
}

// ---------------------------------------------------------------------------------------------
// Dart

/// A Dart string literal (single quotes unless double quotes need fewer escapes).
pub(crate) fn dart(s: &str) -> String {
    let q = preferred_quote(s, '\'');
    let mut out = String::with_capacity(s.len() + 2);
    out.push(q);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            c if c == q => {
                out.push('\\');
                out.push(c);
            }
            '$' => out.push_str("\\$"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\u{b}' => out.push_str("\\v"),
            c if is_invisible(c) => push_hex(&mut out, "\\u{", c as u32, 1, "}"),
            c => out.push(c),
        }
    }
    out.push(q);
    out
}

/// A body string: raw (`r'''…'''` when multi-line, `r'…'` when it has `\`/`$`) where possible.
pub(crate) fn dart_text(s: &str) -> String {
    if has_invisible(s) {
        return dart(s);
    }
    if s.contains('\n') && !s.contains("'''") && !s.ends_with('\'') {
        // The (empty) first line after `'''` is dropped by Dart.
        return format!("r'''\n{s}'''");
    }
    if !s.contains('\n') && s.contains(['\\', '$']) && !s.contains('\'') {
        return format!("r'{s}'");
    }
    dart(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invisible_classification() {
        assert!(!is_invisible('\n'));
        assert!(!is_invisible('\t'));
        assert!(!is_invisible('é'));
        assert!(!is_invisible('😀'));
        for c in [
            '\r', '\0', '\u{1b}', '\u{7f}', '\u{85}', '\u{202e}', '\u{2066}', '\u{feff}',
            '\u{200b}',
        ] {
            assert!(is_invisible(c), "{c:?}");
        }
    }

    #[test]
    fn shell_quoting() {
        assert_eq!(sh("abc"), "'abc'");
        assert_eq!(sh("it's"), "'it'\\''s'");
        assert_eq!(sh("$HOME `x` \"q\" !h"), "'$HOME `x` \"q\" !h'");
        assert_eq!(sh("a\nb"), "'a\nb'");
        assert_eq!(sh("a\tb\r\n'!\\"), "$'a\\tb\\r\\n\\'\\x21\\\\'");
        assert_eq!(sh("\u{1b}[0m\u{85}"), "$'\\x1B[0m\\xC2\\x85'");
        assert_eq!(sh_word("POST"), "POST");
        assert_eq!(sh_word("M&M"), "'M&M'");
        assert_eq!(sh_word(""), "''");
    }

    #[test]
    fn cmd_quoting() {
        assert_eq!(
            cmd("https://example.com/?a=1&b=2"),
            "\"https://example.com/?a=1&b=2\""
        );
        assert_eq!(cmd(r"C:\dir\"), r#""C:\dir\\""#);
        assert_eq!(cmd(r#"{"a":"b"}"#), r#""{\"a\":\"b\"}""#);
        assert_eq!(cmd(r#"{"a":"b&c"}"#), r#"^"^{^\^"a^\^":^\^"b^&c^\^"^}^""#);
        assert_eq!(cmd("50%off %PATH%"), "^\"50^%^off ^%^PATH^%^\"");
        assert_eq!(cmd("a\nb\"c"), "^\"a^\n\nb^\\^\"c^\"");
        assert_eq!(cmd(r#"\"x"#), r#""\\\"x""#);
        assert_eq!(cmd(r#"\"x%"#), r#"^"^\^\^\^"x^%^""#);
        assert_eq!(cmd("é%日本&"), "^\"é^%^日本^&^\"");
        assert_eq!(cmd("100% off%%"), "^\"100^%^ off^%^%^\"");
        // Control characters can't be carried: no raw CR (whose removal by cmd would leave a
        // lone caret escaping the next caret, un-escaping the `&`), Ctrl-Z or NUL.
        assert_eq!(cmd("id\r& calc.exe"), "\"id & calc.exe\"");
        assert_eq!(cmd("id\r& \"calc\""), "^\"id ^& ^\\^\"calc^\\^\"^\"");
        assert_eq!(cmd("a\u{1a}b\0c\u{1b}"), "\"a b c \"");
        for s in ["a\r&b", "x\r\n%PATH%\r", "\"\r|\u{85}"] {
            assert!(!cmd(s).chars().any(cmd_unrepresentable), "{s:?}");
        }
    }

    #[test]
    fn powershell_quoting() {
        assert_eq!(ps("it's ‘x’"), "'it''s ‘‘x’’'");
        assert_eq!(ps("$x `y`"), "'$x `y`'");
        assert_eq!(ps("a\r\n$b\u{1b}\""), "\"a`r`n`$b`u{1B}`\"\"");
        assert_eq!(ps_here("a\nb"), Some("@'\na\nb\n'@".to_string()));
        assert_eq!(ps_here("a\n'@b"), None);
        assert_eq!(ps_here("a\n  ’@"), None);
        assert_eq!(ps_here("single"), None);
    }

    #[test]
    fn js_quoting() {
        assert_eq!(js("a\"b"), "'a\"b'");
        assert_eq!(js("it's"), "\"it's\"");
        assert_eq!(js("\u{2028}\0\n\\"), "\"\\u2028\\x00\\n\\\\\"");
        assert_eq!(
            js_template("a\n`${x}` $y \\ \r"),
            "`a\n\\`\\${x}\\` $y \\\\ \\r`"
        );
        assert_eq!(js_key("name"), "name");
        assert_eq!(js_key("$ok_1"), "$ok_1");
        assert_eq!(js_key("content-type"), "\"content-type\"");
        assert_eq!(js_key("1a"), "\"1a\"");
        assert_eq!(js_key("__proto__"), "[\"__proto__\"]");
    }

    #[test]
    fn python_quoting() {
        assert_eq!(py("a\"b"), "'a\"b'");
        assert_eq!(py("\r\u{85}\u{2028}"), "\"\\r\\x85\\u2028\"");
        assert_eq!(py_text("a\n\"\"\"b\""), "\"\"\"a\n\"\\\"\\\"b\\\"\"\"\"");
        assert_eq!(py_text("x\\\n\r"), "\"\"\"x\\\\\n\\r\"\"\"");
    }

    #[test]
    fn go_quoting() {
        assert_eq!(go("é\u{85}\u{1}"), "\"é\\u0085\\x01\"");
        assert_eq!(go_text("{\"a\":1}"), "`{\"a\":1}`");
        assert_eq!(go_text("a`b\"c"), "\"a`b\\\"c\"");
        assert_eq!(go_text("a\r\nb"), "\"a\\r\\nb\"");
        assert_eq!(go_text("\u{feff}\"x"), "\"\\uFEFF\\\"x\"");
    }

    #[test]
    fn rust_quoting() {
        assert_eq!(rust_text("{\"a\":1}"), "r#\"{\"a\":1}\"#");
        assert_eq!(rust_text("\"#\"##"), "r###\"\"#\"##\"###");
        assert_eq!(rust_text("C:\\x"), "r\"C:\\x\"");
        assert_eq!(rust_text("\u{202e}\"x"), "\"\\u{202E}\\\"x\"");
        assert_eq!(
            rust_bytes(&[0x89, b'P', b'"', 0, b'\\']),
            "b\"\\x89P\\\"\\0\\\\\""
        );
    }

    #[test]
    fn java_quoting() {
        assert_eq!(java("\u{1}1\u{2028}"), "\"\\0011\\u2028\"");
        assert_eq!(
            java_text_block("{\"a\":1}", "    "),
            "\"\"\"\n    {\"a\":1}\\\n    \"\"\""
        );
        assert_eq!(
            java_text_block("a \n\n b\n", "  "),
            "\"\"\"\n  a\\s\n\n   b\n  \"\"\""
        );
        assert_eq!(
            java_text_block("\"\"\"\\", ""),
            "\"\"\"\n\"\\\"\\\"\\\\\\\n\"\"\""
        );
    }

    #[test]
    fn kotlin_quoting() {
        assert_eq!(kotlin("$x\"\u{7}"), "\"\\$x\\\"\\u0007\"");
        assert_eq!(
            kotlin_text("{\"a\":\"$b\"}", ""),
            "\"\"\"{\"a\":\"${'$'}b\"}\"\"\""
        );
        assert_eq!(
            kotlin_text("a\nb", "  "),
            "\"\"\"\n  |a\n  |b\n  \"\"\".trimMargin()"
        );
        assert_eq!(kotlin_text("\"x\"", ""), "\"\\\"x\\\"\"");
    }

    #[test]
    fn csharp_quoting() {
        assert_eq!(csharp("a\"\u{1}"), "\"a\\\"\\u0001\"");
        assert_eq!(
            csharp_text("{\"a\":\"\"\"\"}", "  "),
            "\"\"\"\"\"\n  {\"a\":\"\"\"\"}\n  \"\"\"\"\""
        );
        assert_eq!(csharp_text("a\n\nb\n", ""), "\"\"\"\na\n\nb\n\n\"\"\"");
        assert_eq!(csharp_text("a\n  \nb", ""), "\"a\\n  \\nb\"");
        assert_eq!(csharp_text("plain", ""), "\"plain\"");
    }

    #[test]
    fn php_quoting() {
        assert_eq!(php("it's \\ $x"), "'it\\'s \\\\ $x'");
        assert_eq!(php("a\r$x\u{1b}"), "\"a\\r\\$x\\e\"");
    }

    #[test]
    fn ruby_quoting() {
        assert_eq!(ruby("a#{b}"), "'a#{b}'");
        assert_eq!(ruby("it's #{x} #@y"), "\"it's \\#{x} \\#@y\"");
        assert_eq!(ruby("a\nb"), "\"a\\nb\"");
        assert_eq!(ruby_text("a\nb"), "'a\nb'");
        assert_eq!(ruby_text("a\r\n"), "\"a\\r\n\"");
    }

    #[test]
    fn swift_quoting() {
        assert_eq!(swift("\\(x)\"\u{0}"), "\"\\\\(x)\\\"\\0\"");
        assert_eq!(
            swift_text("{\"a\":\"\\(x)\"}", ""),
            "#\"{\"a\":\"\\(x)\"}\"#"
        );
        assert_eq!(swift_text("\"#x", ""), "##\"\"#x\"##");
        assert_eq!(
            swift_text("a \n\"\"\"\n", "  "),
            "\"\"\"\n  a\\u{20}\n  \"\\\"\\\"\n\n  \"\"\""
        );
    }

    #[test]
    fn dart_quoting() {
        assert_eq!(dart("it's $x"), "\"it's \\$x\"");
        assert_eq!(dart("a\u{1}"), "'a\\u{1}'");
        assert_eq!(dart_text("a\nb"), "r'''\na\nb'''");
        assert_eq!(dart_text("a\nb'"), "\"a\\nb'\"");
        assert_eq!(dart_text("$x\\y"), "r'$x\\y'");
    }
}
