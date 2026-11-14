//! Precompiled text matchers.
//!
//! Plain values match case-insensitively. ASCII needles (the overwhelmingly common case) use
//! allocation-free byte comparisons; non-ASCII needles fall back to a Unicode-aware
//! case-insensitive regex. Long haystacks (bodies) use the regex engine's SIMD literal search.

use regex::bytes::{Regex, RegexBuilder};
use std::fmt;

/// Upper bound on the compiled size of a user-supplied regex.
const REGEX_SIZE_LIMIT: usize = 4 << 20;

/// The kind of literal comparison.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LitOp {
    Contains,
    Equals,
    Prefix,
    Suffix,
}

/// Regex flags from a `/pattern/flags` literal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RegexFlags {
    pub case_insensitive: bool,
    pub multi_line: bool,
    pub dot_all: bool,
    pub verbose: bool,
}

impl RegexFlags {
    /// Parse a flags string; returns the first unsupported flag on failure.
    pub(crate) fn parse(flags: &str) -> Result<RegexFlags, char> {
        let mut out = RegexFlags::default();
        for c in flags.chars() {
            match c {
                'i' => out.case_insensitive = true,
                'm' => out.multi_line = true,
                's' => out.dot_all = true,
                'x' => out.verbose = true,
                // JavaScript habits: unicode is always on, global is meaningless here.
                'u' | 'g' => {}
                other => return Err(other),
            }
        }
        Ok(out)
    }
}

/// An anchored or unanchored `*` wildcard pattern over lowercase ASCII.
#[derive(Clone, Debug)]
pub(crate) struct Glob {
    /// Literal pieces between the stars (empty pieces removed).
    parts: Box<[Box<[u8]>]>,
    /// The pattern does not start with `*` (and the glob is anchored).
    anchored_start: bool,
    /// The pattern does not end with `*` (and the glob is anchored).
    anchored_end: bool,
    source: Box<str>,
}

impl Glob {
    fn is_match(&self, h: &[u8]) -> bool {
        let n = self.parts.len();
        if n == 0 {
            return true;
        }
        let mut pos = 0usize;
        let mut lo = 0usize;
        let mut hi = n;
        if self.anchored_start {
            let first = &self.parts[0];
            if !starts_with_ci(h, first) {
                return false;
            }
            pos = first.len();
            lo = 1;
        }
        let mut end = h.len();
        if self.anchored_end && hi > lo {
            let last = &self.parts[n - 1];
            if end < pos + last.len() || !ends_with_ci(h, last) {
                return false;
            }
            end -= last.len();
            hi = n - 1;
        } else if self.anchored_end && self.anchored_start && n == 1 {
            // Single literal both anchored: covered by the prefix check plus a length check.
            return h.len() == pos;
        }
        for part in &self.parts[lo..hi] {
            match find_ci(&h[pos..end], part) {
                Some(i) => pos += i + part.len(),
                None => return false,
            }
        }
        true
    }
}

/// A compiled matcher for one value.
#[derive(Clone)]
pub(crate) enum TextMatcher {
    /// Matches any present value (empty `contains`).
    Any,
    Contains(Box<[u8]>),
    Equals(Box<[u8]>),
    Prefix(Box<[u8]>),
    Suffix(Box<[u8]>),
    Glob(Glob),
    Regex(Regex),
}

impl fmt::Debug for TextMatcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.explain())
    }
}

impl TextMatcher {
    /// A case-insensitive literal comparison. `long_haystack` selects an implementation tuned for
    /// large inputs such as bodies.
    pub(crate) fn literal(op: LitOp, text: &str, long_haystack: bool) -> TextMatcher {
        if text.is_empty() {
            return match op {
                LitOp::Equals => TextMatcher::Equals(Box::default()),
                _ => TextMatcher::Any,
            };
        }
        let needs_regex = !text.is_ascii() || (long_haystack && op == LitOp::Contains);
        if needs_regex {
            let escaped = regex::escape(text);
            let pattern = match op {
                LitOp::Contains => escaped,
                LitOp::Equals => format!(r"\A(?:{escaped})\z"),
                LitOp::Prefix => format!(r"\A(?:{escaped})"),
                LitOp::Suffix => format!(r"(?:{escaped})\z"),
            };
            if let Ok(re) = build_regex(
                &pattern,
                RegexFlags {
                    case_insensitive: true,
                    ..Default::default()
                },
            ) {
                return TextMatcher::Regex(re);
            }
            // An escaped literal always compiles unless it exceeds the size limit; fall back to
            // the ASCII path (which then only folds ASCII case).
        }
        let lower: Box<[u8]> = text.to_ascii_lowercase().into_bytes().into_boxed_slice();
        match op {
            LitOp::Contains => TextMatcher::Contains(lower),
            LitOp::Equals => TextMatcher::Equals(lower),
            LitOp::Prefix => TextMatcher::Prefix(lower),
            LitOp::Suffix => TextMatcher::Suffix(lower),
        }
    }

    /// A `*` wildcard pattern. Anchored globs must match the whole value; unanchored globs may
    /// match anywhere (used for free text).
    pub(crate) fn glob(pattern: &str, anchored: bool) -> TextMatcher {
        if !pattern.is_ascii() {
            let body: Vec<String> = pattern.split('*').map(regex::escape).collect();
            let body = body.join(".*");
            let pattern = if anchored {
                format!(r"\A(?:{body})\z")
            } else {
                body
            };
            let flags = RegexFlags {
                case_insensitive: true,
                dot_all: true,
                ..Default::default()
            };
            if let Ok(re) = build_regex(&pattern, flags) {
                return TextMatcher::Regex(re);
            }
        }
        let lower = pattern.to_ascii_lowercase();
        let parts: Box<[Box<[u8]>]> = lower
            .split('*')
            .filter(|p| !p.is_empty())
            .map(|p| p.as_bytes().to_vec().into_boxed_slice())
            .collect();
        TextMatcher::Glob(Glob {
            parts,
            anchored_start: anchored && !lower.starts_with('*'),
            anchored_end: anchored && !lower.ends_with('*'),
            source: pattern.into(),
        })
    }

    /// A regex. Errors carry a short, user-facing message.
    pub(crate) fn regex(pattern: &str, flags: RegexFlags) -> Result<TextMatcher, String> {
        build_regex(pattern, flags).map(TextMatcher::Regex)
    }

    #[inline]
    pub(crate) fn is_match(&self, h: &[u8]) -> bool {
        match self {
            TextMatcher::Any => true,
            TextMatcher::Contains(n) => find_ci(h, n).is_some(),
            TextMatcher::Equals(n) => h.eq_ignore_ascii_case(n),
            TextMatcher::Prefix(n) => starts_with_ci(h, n),
            TextMatcher::Suffix(n) => ends_with_ci(h, n),
            TextMatcher::Glob(g) => g.is_match(h),
            TextMatcher::Regex(re) => re.is_match(h),
        }
    }

    /// A compact description, used by `explain` output and tests.
    pub(crate) fn explain(&self) -> String {
        let s = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
        match self {
            TextMatcher::Any => "any".to_string(),
            TextMatcher::Contains(n) => format!("contains {:?}", s(n)),
            TextMatcher::Equals(n) => format!("eq {:?}", s(n)),
            TextMatcher::Prefix(n) => format!("prefix {:?}", s(n)),
            TextMatcher::Suffix(n) => format!("suffix {:?}", s(n)),
            TextMatcher::Glob(g) => {
                format!(
                    "glob{} {:?}",
                    if g.anchored_start || g.anchored_end {
                        ""
                    } else {
                        "~"
                    },
                    g.source
                )
            }
            TextMatcher::Regex(re) => format!("regex /{}/", re.as_str()),
        }
    }
}

fn build_regex(pattern: &str, flags: RegexFlags) -> Result<Regex, String> {
    RegexBuilder::new(pattern)
        .case_insensitive(flags.case_insensitive)
        .multi_line(flags.multi_line)
        .dot_matches_new_line(flags.dot_all)
        .ignore_whitespace(flags.verbose)
        .size_limit(REGEX_SIZE_LIMIT)
        .dfa_size_limit(REGEX_SIZE_LIMIT)
        .nest_limit(64)
        .build()
        .map_err(|e| regex_error_message(&e))
}

/// Turn a regex error into a one-line message.
fn regex_error_message(e: &regex::Error) -> String {
    match e {
        regex::Error::Syntax(s) => s
            .lines()
            .rev()
            .find_map(|l| l.trim().strip_prefix("error: "))
            .unwrap_or("invalid syntax")
            .to_string(),
        regex::Error::CompiledTooBig(_) => "pattern is too large".to_string(),
        _ => e.to_string(),
    }
}

/// ASCII case-insensitive substring search; `needle` must already be lowercase.
pub(crate) fn find_ci(h: &[u8], needle: &[u8]) -> Option<usize> {
    let n = needle.len();
    if n == 0 {
        return Some(0);
    }
    if h.len() < n {
        return None;
    }
    let last = h.len() - n;
    let first = needle[0];
    let rest = &needle[1..];
    let window = &h[..=last];
    let check = |i: usize| h[i + 1..i + n].eq_ignore_ascii_case(rest);
    if first.is_ascii_lowercase() {
        memchr::memchr2_iter(first, first.to_ascii_uppercase(), window).find(|&i| check(i))
    } else {
        memchr::memchr_iter(first, window).find(|&i| check(i))
    }
}

/// ASCII case-insensitive `starts_with`; `prefix` must already be lowercase.
#[inline]
pub(crate) fn starts_with_ci(h: &[u8], prefix: &[u8]) -> bool {
    h.len() >= prefix.len() && h[..prefix.len()].eq_ignore_ascii_case(prefix)
}

/// ASCII case-insensitive `ends_with`; `suffix` must already be lowercase.
#[inline]
pub(crate) fn ends_with_ci(h: &[u8], suffix: &[u8]) -> bool {
    h.len() >= suffix.len() && h[h.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit(op: LitOp, t: &str) -> TextMatcher {
        TextMatcher::literal(op, t, false)
    }

    #[test]
    fn find_ci_works() {
        assert_eq!(find_ci(b"Hello World", b"world"), Some(6));
        assert_eq!(find_ci(b"Hello World", b"o w"), Some(4));
        assert_eq!(find_ci(b"abc", b"abcd"), None);
        assert_eq!(find_ci(b"", b""), Some(0));
        assert_eq!(find_ci(b"aaab", b"aab"), Some(1));
        assert_eq!(find_ci(b"x-Trace-ID", b"-trace-"), Some(1));
        assert_eq!(find_ci(b"/API/v1", b"/api/"), Some(0));
        assert_eq!(find_ci("ünï".as_bytes(), b"n"), Some(2));
    }

    #[test]
    fn literals() {
        assert!(lit(LitOp::Contains, "EXAMPLE").is_match(b"api.example.com"));
        assert!(!lit(LitOp::Contains, "examples").is_match(b"api.example.com"));
        assert!(lit(LitOp::Equals, "get").is_match(b"GET"));
        assert!(!lit(LitOp::Equals, "get").is_match(b"GETX"));
        assert!(lit(LitOp::Prefix, "/API").is_match(b"/api/users"));
        assert!(lit(LitOp::Suffix, ".JSON").is_match(b"/a/b.json"));
        assert!(!lit(LitOp::Suffix, ".json").is_match(b"json"));
        assert!(lit(LitOp::Contains, "").is_match(b""));
        assert!(lit(LitOp::Equals, "").is_match(b""));
        assert!(!lit(LitOp::Equals, "").is_match(b"x"));
    }

    #[test]
    fn unicode_literals_fold_case() {
        assert!(lit(LitOp::Contains, "ÜBER").is_match("www.über.de".as_bytes()));
        assert!(lit(LitOp::Equals, "Straße").is_match("STRAßE".as_bytes()));
        assert!(!lit(LitOp::Equals, "über").is_match("überall".as_bytes()));
        assert!(lit(LitOp::Prefix, "ü").is_match("Über".as_bytes()));
        assert!(lit(LitOp::Suffix, "ö").is_match("xÖ".as_bytes()));
        // Regex metacharacters in literals are escaped.
        assert!(lit(LitOp::Contains, "ä.*").is_match("xä.*y".as_bytes()));
        assert!(!lit(LitOp::Contains, "ä.*").is_match("xäzzy".as_bytes()));
    }

    #[test]
    fn long_haystack_literal_is_equivalent() {
        let short = TextMatcher::literal(LitOp::Contains, "Error (x)", false);
        let long = TextMatcher::literal(LitOp::Contains, "Error (x)", true);
        assert!(matches!(long, TextMatcher::Regex(_)));
        for h in ["an ERROR (X) here", "error(x)", "", "error (y)"] {
            assert_eq!(
                short.is_match(h.as_bytes()),
                long.is_match(h.as_bytes()),
                "{h}"
            );
        }
    }

    #[test]
    fn globs() {
        let g = TextMatcher::glob("*.example.com", true);
        assert!(g.is_match(b"api.EXAMPLE.com"));
        assert!(!g.is_match(b"example.com"));
        assert!(!g.is_match(b"api.example.com.evil.net"));
        let g = TextMatcher::glob("/api/*", true);
        assert!(g.is_match(b"/api/"));
        assert!(g.is_match(b"/api/users/1"));
        assert!(!g.is_match(b"/v1/api/users"));
        let g = TextMatcher::glob("a*b*c", true);
        assert!(g.is_match(b"abc"));
        assert!(g.is_match(b"aXXbYYc"));
        assert!(!g.is_match(b"aXXbYY"));
        assert!(!g.is_match(b"ac"));
        // Overlap between the prefix and the suffix must not double count.
        let g = TextMatcher::glob("ab*ba", true);
        assert!(!g.is_match(b"aba"));
        assert!(g.is_match(b"abba"));
        assert!(TextMatcher::glob("*", true).is_match(b""));
        assert!(TextMatcher::glob("**", true).is_match(b"anything"));
        let g = TextMatcher::glob("api*users", false);
        assert!(g.is_match(b"https://x.com/api/v1/users?x"));
        assert!(!g.is_match(b"https://x.com/users/api"));
        let g = TextMatcher::glob("*.ü*", true);
        assert!(g.is_match("x.Über".as_bytes()));
        assert!(!g.is_match("xÜber".as_bytes()));
    }

    #[test]
    fn regexes() {
        let re = TextMatcher::regex("^/v[12]/", RegexFlags::default()).unwrap();
        assert!(re.is_match(b"/v1/users"));
        assert!(!re.is_match(b"/V1/users"));
        let re = TextMatcher::regex("^/v[12]/", RegexFlags::parse("i").unwrap()).unwrap();
        assert!(re.is_match(b"/V1/users"));
        let err = TextMatcher::regex("a(", RegexFlags::default()).unwrap_err();
        assert_eq!(err, "unclosed group");
        assert_eq!(RegexFlags::parse("imsxug").map(|f| f.dot_all), Ok(true));
        assert_eq!(RegexFlags::parse("ie"), Err('e'));
        let err = TextMatcher::regex("(a{1000}){1000}", RegexFlags::default()).unwrap_err();
        assert!(!err.is_empty());
    }
}
