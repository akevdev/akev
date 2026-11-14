//! Small shared helpers: media-type parameter parsing, URL path helpers, RFC 3339 formatting,
//! base64 engines and indentation output.

use base64::alphabet;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};

/// Standard padded base64 (used for every binary value we render into JSON).
pub(crate) const B64: GeneralPurpose = base64::engine::general_purpose::STANDARD;

const LENIENT: GeneralPurposeConfig = GeneralPurposeConfig::new()
    .with_decode_padding_mode(DecodePaddingMode::Indifferent)
    .with_decode_allow_trailing_bits(true);

/// Standard alphabet, padding optional.
pub(crate) const B64_LENIENT: GeneralPurpose = GeneralPurpose::new(&alphabet::STANDARD, LENIENT);
/// URL-safe alphabet, padding optional.
pub(crate) const B64_URL_LENIENT: GeneralPurpose =
    GeneralPurpose::new(&alphabet::URL_SAFE, LENIENT);

/// Encode bytes as standard padded base64.
pub(crate) fn b64(bytes: &[u8]) -> String {
    use base64::Engine as _;
    B64.encode(bytes)
}

/// Lowercased media-type essence (`type/subtype`) without parameters or surrounding whitespace.
/// When several comma-separated types are present (a broken but seen-in-the-wild header), the
/// first one is used.
pub(crate) fn essence(content_type: &str) -> String {
    let first = content_type.split(',').next().unwrap_or("");
    first
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

/// Iterate the `;`-separated parameters of a header value such as a Content-Type or
/// Content-Disposition (`form-data; name="a"; filename*=UTF-8''x`). The leading value before the
/// first `;` is skipped. Parameter names are lowercased; quoted-string values are unquoted, with
/// only `\"` and `\\` treated as escapes (so Windows paths sent unescaped by old browsers survive).
pub(crate) fn params(value: &str) -> Vec<(String, String)> {
    let bytes = value.as_bytes();
    let mut out = Vec::new();
    // Skip the leading value (it can't contain quoted strings that matter here).
    let Some(first) = memchr::memchr(b';', bytes) else {
        return out;
    };
    let mut i = first + 1;
    while i < bytes.len() {
        // skip whitespace and stray separators
        while i < bytes.len() && (bytes[i] == b';' || bytes[i].is_ascii_whitespace()) {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let name_start = i;
        while i < bytes.len() && bytes[i] != b'=' && bytes[i] != b';' {
            i += 1;
        }
        let name = value[name_start..i].trim().to_ascii_lowercase();
        if i >= bytes.len() || bytes[i] == b';' {
            if !name.is_empty() {
                out.push((name, String::new()));
            }
            continue;
        }
        i += 1; // '='
        while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
            i += 1;
        }
        let mut val = String::new();
        if i < bytes.len() && bytes[i] == b'"' {
            i += 1;
            let mut seg_start = i;
            while i < bytes.len() {
                match bytes[i] {
                    b'"' => break,
                    b'\\'
                        if i + 1 < bytes.len()
                            && (bytes[i + 1] == b'"' || bytes[i + 1] == b'\\') =>
                    {
                        val.push_str(&value[seg_start..i]);
                        val.push(bytes[i + 1] as char);
                        i += 2;
                        seg_start = i;
                        continue;
                    }
                    _ => i += 1,
                }
            }
            val.push_str(&value[seg_start..i.min(bytes.len())]);
            // skip closing quote and anything up to the next ';'
            while i < bytes.len() && bytes[i] != b';' {
                i += 1;
            }
        } else {
            let start = i;
            while i < bytes.len() && bytes[i] != b';' {
                i += 1;
            }
            val.push_str(value[start..i].trim());
        }
        if !name.is_empty() {
            out.push((name, val));
        }
    }
    out
}

/// First value of the named parameter (case-insensitive name).
pub(crate) fn param(value: &str, name: &str) -> Option<String> {
    params(value)
        .into_iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v)
}

/// The URL path without query string or fragment.
pub(crate) fn path_only(path: &str) -> &str {
    let end = path.find(['?', '#']).unwrap_or(path.len());
    &path[..end]
}

/// Lowercased file extension of the last path segment, if any.
pub(crate) fn path_extension(path: &str) -> Option<String> {
    let p = path_only(path);
    let last = p.rsplit('/').next().unwrap_or("");
    let (stem, ext) = last.rsplit_once('.')?;
    if stem.is_empty() || ext.is_empty() || ext.len() > 12 {
        return None;
    }
    Some(ext.to_ascii_lowercase())
}

/// Days since 1970-01-01 → (year, month, day) in the proleptic Gregorian calendar
/// (Howard Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Format a Unix timestamp as RFC 3339 UTC (`2024-05-01T12:00:00Z`, with a fractional part only
/// when `nanos` is non-zero). Returns `None` outside years 0..=9999.
pub(crate) fn format_rfc3339(secs: i64, nanos: u32) -> Option<String> {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    if !(0..=9999).contains(&y) || nanos >= 1_000_000_000 {
        return None;
    }
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let mut s = format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}");
    if nanos != 0 {
        if nanos.is_multiple_of(1_000_000) {
            s.push_str(&format!(".{:03}", nanos / 1_000_000));
        } else if nanos.is_multiple_of(1_000) {
            s.push_str(&format!(".{:06}", nanos / 1_000));
        } else {
            s.push_str(&format!(".{nanos:09}"));
        }
    }
    s.push('Z');
    Some(s)
}

/// Largest output a formatter may produce for an input of `input_len` bytes: 16× the input plus
/// 4 MiB. Real documents expand far less (minified JSON nested a few levels deep is ~2–4×);
/// only pathological nesting gets close, and then formatting gives up (`None`, the raw text is
/// shown) instead of allocating hundreds of megabytes.
pub(crate) fn output_budget(input_len: usize) -> usize {
    input_len.saturating_mul(16).saturating_add(4 << 20)
}

/// Maximum indentation depth rendered by the formatters. Deeper nesting is still formatted
/// (one item per line) but not indented further, which bounds output size on pathological input.
pub(crate) const MAX_INDENT_LEVELS: usize = 100;

const SPACES: &str = "                                                                                                                                ";

/// Append `levels` two-space indentation units (capped at [`MAX_INDENT_LEVELS`]).
pub(crate) fn push_indent(out: &mut String, levels: usize) {
    let mut n = levels.min(MAX_INDENT_LEVELS) * 2;
    while n > 0 {
        let k = n.min(SPACES.len());
        out.push_str(&SPACES[..k]);
        n -= k;
    }
}

/// Convert an `f64` to a JSON value; non-finite values become strings (`"NaN"`, `"Infinity"`).
/// Longest rendering of a non-string map key (CBOR / MessagePack keys can be whole
/// structures).
pub(crate) const MAX_KEY_CHARS: usize = 256;

/// A decoded map key as a JSON object key: strings as they are, anything else as its JSON
/// text, cut at [`MAX_KEY_CHARS`]. Without the cap, keys that are maps keyed by maps grow
/// exponentially (each level escapes the one inside), so a few dozen input bytes could
/// keep a body view busy for seconds (found by the `content_decoders` fuzz target).
pub(crate) fn key_text(v: serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s,
        other => {
            let text = other.to_string();
            match text.char_indices().nth(MAX_KEY_CHARS) {
                Some((cut, _)) => format!("{}…", &text[..cut]),
                None => text,
            }
        }
    }
}

pub(crate) fn json_f64(v: f64) -> serde_json::Value {
    match serde_json::Number::from_f64(v) {
        Some(n) => serde_json::Value::Number(n),
        None if v.is_nan() => serde_json::Value::String("NaN".into()),
        None if v > 0.0 => serde_json::Value::String("Infinity".into()),
        None => serde_json::Value::String("-Infinity".into()),
    }
}

/// Strip a leading UTF-8 BOM.
pub(crate) fn strip_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes)
}

/// Skip a UTF-8 BOM and ASCII whitespace.
pub(crate) fn skip_bom_ws(bytes: &[u8]) -> &[u8] {
    let b = strip_bom(bytes);
    let start = b
        .iter()
        .position(|c| !c.is_ascii_whitespace())
        .unwrap_or(b.len());
    &b[start..]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn essence_and_params() {
        assert_eq!(
            essence(" Application/JSON ; charset=UTF-8"),
            "application/json"
        );
        assert_eq!(essence("text/html, text/plain"), "text/html");
        assert_eq!(
            param("text/html; charset=\"utf-8\"", "CHARSET").as_deref(),
            Some("utf-8")
        );
        assert_eq!(
            param("multipart/form-data; boundary=abc;x=1", "boundary").as_deref(),
            Some("abc")
        );
        assert_eq!(
            param(
                r#"form-data; name="a\"b"; filename="C:\Users\x.txt""#,
                "filename"
            )
            .as_deref(),
            Some(r"C:\Users\x.txt")
        );
        assert_eq!(
            param(r#"form-data; name="a\"b""#, "name").as_deref(),
            Some("a\"b")
        );
        assert_eq!(param("text/plain", "charset"), None);
        assert_eq!(param("text/plain;;flag; a = b ", "a").as_deref(), Some("b"));
        assert_eq!(
            param("text/plain; q=\"unterminated", "q").as_deref(),
            Some("unterminated")
        );
    }

    #[test]
    fn paths() {
        assert_eq!(path_only("/a/b.js?x=1#f"), "/a/b.js");
        assert_eq!(path_extension("/a/b.min.JS?x=1").as_deref(), Some("js"));
        assert_eq!(path_extension("/a/.env"), None);
        assert_eq!(path_extension("/a/b/"), None);
        assert_eq!(path_extension("/v1.2/items"), None);
    }

    #[test]
    fn rfc3339() {
        assert_eq!(
            format_rfc3339(0, 0).as_deref(),
            Some("1970-01-01T00:00:00Z")
        );
        assert_eq!(
            format_rfc3339(1_700_000_000, 0).as_deref(),
            Some("2023-11-14T22:13:20Z")
        );
        assert_eq!(
            format_rfc3339(951_782_400, 0).as_deref(),
            Some("2000-02-29T00:00:00Z")
        );
        assert_eq!(
            format_rfc3339(-1, 500_000_000).as_deref(),
            Some("1969-12-31T23:59:59.500Z")
        );
        assert_eq!(format_rfc3339(i64::MAX, 0), None);
        assert_eq!(
            format_rfc3339(253_402_300_799, 0).as_deref(),
            Some("9999-12-31T23:59:59Z")
        );
        assert_eq!(format_rfc3339(253_402_300_800, 0), None);
    }

    #[test]
    fn indent_is_capped() {
        let mut s = String::new();
        push_indent(&mut s, 3);
        assert_eq!(s, "      ");
        let mut s = String::new();
        push_indent(&mut s, 10_000);
        assert_eq!(s.len(), MAX_INDENT_LEVELS * 2);
    }

    #[test]
    fn json_numbers() {
        assert_eq!(json_f64(1.5), serde_json::json!(1.5));
        assert_eq!(json_f64(f64::NAN), serde_json::json!("NaN"));
        assert_eq!(json_f64(f64::NEG_INFINITY), serde_json::json!("-Infinity"));
    }
}
