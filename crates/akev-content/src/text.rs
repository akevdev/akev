//! Charset-aware text decoding (WHATWG Encoding Standard labels via `encoding_rs`).
//!
//! Precedence: byte-order mark (UTF-8 / UTF-16LE / UTF-16BE) → `charset` parameter of the
//! Content-Type → for HTML/XML documents without a charset parameter, an in-document declaration
//! within the first 1024 bytes (`<meta charset>`, `<meta http-equiv content="...; charset=">`,
//! `<?xml encoding="..."?>`) → UTF-8. Invalid sequences are replaced with U+FFFD.

use encoding_rs::{Encoding, UTF_8, UTF_16BE, UTF_16LE};

/// Decode bytes into text using the charset from `content_type` (default UTF-8, with BOM
/// detection), replacing invalid sequences. Returns (text, had_errors).
pub fn decode_text(bytes: &[u8], content_type: Option<&str>) -> (String, bool) {
    let (encoding, bom_len) = pick_encoding(bytes, content_type);
    let body = &bytes[bom_len..];
    if encoding == UTF_8
        && let Ok(s) = std::str::from_utf8(body)
    {
        return (s.to_owned(), false);
    }
    let (text, had_errors) = encoding.decode_without_bom_handling(body);
    (text.into_owned(), had_errors)
}

/// Decode at most roughly `max_len` bytes of text (the result is at most `max_len` bytes of
/// UTF-8, cut on a character boundary). Returns `(text, had_errors, truncated)` where
/// `truncated` is true when some of the input was not decoded. Only the needed prefix of the
/// input is processed, so this is cheap on huge bodies.
pub fn decode_text_prefix(
    bytes: &[u8],
    content_type: Option<&str>,
    max_len: usize,
) -> (String, bool, bool) {
    let (encoding, bom_len) = pick_encoding(bytes, content_type);
    let body = &bytes[bom_len..];
    if body.len() <= max_len {
        let (mut text, errors) = if encoding == UTF_8 {
            match std::str::from_utf8(body) {
                Ok(s) => (s.to_owned(), false),
                Err(_) => {
                    let (t, e) = encoding.decode_without_bom_handling(body);
                    (t.into_owned(), e)
                }
            }
        } else {
            let (t, e) = encoding.decode_without_bom_handling(body);
            (t.into_owned(), e)
        };
        // Replacement characters or multi-byte expansion can push the text past the limit.
        let cut = truncate_on_char_boundary(&mut text, max_len);
        return (text, errors, cut);
    }
    let input = &body[..max_len];
    let mut decoder = encoding.new_decoder_without_bom_handling();
    let capacity = decoder
        .max_utf8_buffer_length(input.len())
        .unwrap_or(input.len().saturating_mul(3));
    let mut text = String::with_capacity(capacity);
    // `last = false`: an incomplete trailing sequence is held back instead of becoming U+FFFD.
    let (_, _, errors) = decoder.decode_to_string(input, &mut text, false);
    truncate_on_char_boundary(&mut text, max_len);
    (text, errors, true)
}

/// Truncate to at most `max` bytes on a char boundary; returns whether anything was removed.
pub(crate) fn truncate_on_char_boundary(text: &mut String, max: usize) -> bool {
    if text.len() <= max {
        return false;
    }
    let mut cut = max;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    text.truncate(cut);
    true
}

/// Resolve the encoding and the BOM length to skip.
pub(crate) fn pick_encoding(
    bytes: &[u8],
    content_type: Option<&str>,
) -> (&'static Encoding, usize) {
    if let Some((enc, len)) = Encoding::for_bom(bytes) {
        return (enc, len);
    }
    if let Some(ct) = content_type {
        if let Some(label) = crate::util::param(ct, "charset")
            && let Some(enc) = Encoding::for_label(label.trim().as_bytes())
        {
            return (enc, 0);
        }
        let essence = crate::util::essence(ct);
        let markup = essence == "text/html"
            || essence.ends_with("/xml")
            || essence.ends_with("+xml")
            || essence == "application/xhtml";
        if markup && let Some(enc) = sniff_document_charset(bytes) {
            return (enc, 0);
        }
    }
    (UTF_8, 0)
}

/// Look for an in-document charset declaration in the first 1024 bytes.
fn sniff_document_charset(bytes: &[u8]) -> Option<&'static Encoding> {
    let head = &bytes[..bytes.len().min(1024)];
    let lower: Vec<u8> = head.iter().map(u8::to_ascii_lowercase).collect();
    let found = if lower.starts_with(b"<?xml") {
        let end = memchr::memmem::find(&lower, b"?>").unwrap_or(lower.len());
        attr_value(&lower[..end], b"encoding")
    } else {
        let mut found = None;
        for start in memchr::memmem::find_iter(&lower, b"<meta") {
            let end = memchr::memchr(b'>', &lower[start..]).map_or(lower.len(), |e| start + e);
            if let Some(v) = attr_value(&lower[start..end], b"charset") {
                found = Some(v);
                break;
            }
        }
        found
    }?;
    let enc = Encoding::for_label(&found)?;
    // A document that is readable as ASCII can't really be UTF-16 (HTML spec: treat as UTF-8).
    Some(if enc == UTF_16LE || enc == UTF_16BE {
        UTF_8
    } else {
        enc
    })
}

/// Find `name=value` (value optionally quoted) inside an ASCII-lowercased tag.
fn attr_value(tag: &[u8], name: &[u8]) -> Option<Vec<u8>> {
    let pos = memchr::memmem::find(tag, name)?;
    let mut i = pos + name.len();
    while i < tag.len() && tag[i].is_ascii_whitespace() {
        i += 1;
    }
    if tag.get(i) != Some(&b'=') {
        return None;
    }
    i += 1;
    while i < tag.len() && tag[i].is_ascii_whitespace() {
        i += 1;
    }
    let quote = match tag.get(i) {
        Some(&q @ (b'"' | b'\'')) => {
            i += 1;
            Some(q)
        }
        _ => None,
    };
    let start = i;
    while i < tag.len() {
        let c = tag[i];
        let end = match quote {
            Some(q) => c == q,
            None => {
                c.is_ascii_whitespace()
                    || c == b';'
                    || c == b'"'
                    || c == b'\''
                    || c == b'/'
                    || c == b'>'
            }
        };
        if end {
            break;
        }
        i += 1;
    }
    let v = tag[start..i].to_vec();
    (!v.is_empty()).then_some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_default_and_bom() {
        assert_eq!(
            decode_text("héllo".as_bytes(), None),
            ("héllo".into(), false)
        );
        assert_eq!(
            decode_text(b"\xEF\xBB\xBFhi", Some("text/plain")),
            ("hi".into(), false)
        );
        let (t, err) = decode_text(b"a\xffb", None);
        assert_eq!(t, "a\u{FFFD}b");
        assert!(err);
    }

    #[test]
    fn utf16_boms_override_charset() {
        let le = [0xFF, 0xFE, b'h', 0, b'i', 0];
        assert_eq!(
            decode_text(&le, Some("text/plain; charset=iso-8859-1")).0,
            "hi"
        );
        let be = [0xFE, 0xFF, 0, b'h', 0, b'i'];
        assert_eq!(decode_text(&be, None).0, "hi");
    }

    #[test]
    fn legacy_charsets() {
        assert_eq!(
            decode_text(b"caf\xe9", Some("text/html; charset=ISO-8859-1")).0,
            "café"
        );
        // WHATWG maps latin1 to windows-1252: 0x80 is the euro sign.
        assert_eq!(
            decode_text(b"\x80", Some("text/plain; charset=latin1")).0,
            "€"
        );
        assert_eq!(
            decode_text(b"\x82\xa0", Some("text/plain; charset=shift_jis")).0,
            "あ"
        );
        assert_eq!(
            decode_text(b"\xc4\xe3\xba\xc3", Some("text/plain; charset=\"GBK\"")).0,
            "你好"
        );
        assert_eq!(
            decode_text(b"\xc7\xd1", Some("text/plain; charset=euc-kr")).0,
            "한"
        );
        // Unknown labels fall back to UTF-8.
        assert_eq!(
            decode_text("ü".as_bytes(), Some("text/plain; charset=bogus")).0,
            "ü"
        );
    }

    #[test]
    fn in_document_declarations() {
        let html = b"<html><head><meta charset=\"windows-1251\"></head><body>\xcf\xf0\xe8</body>";
        assert!(decode_text(html, Some("text/html")).0.contains("При"));
        let html2 = b"<meta http-equiv='Content-Type' content='text/html; charset=iso-8859-2'>\xb1";
        assert!(decode_text(html2, Some("text/html")).0.ends_with('ą'));
        let xml = b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><a>\xe9</a>";
        assert_eq!(
            decode_text(xml, Some("application/xml")).0,
            "<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><a>é</a>"
        );
        // The header charset wins over the document.
        assert!(decode_text(html, Some("text/html; charset=utf-8")).1);
        // Not consulted for non-markup types.
        assert!(decode_text(html, Some("text/plain")).1);
        // UTF-16 in a meta tag is treated as UTF-8.
        let m = b"<meta charset=utf-16>\xc3\xa9";
        assert!(decode_text(m, Some("text/html")).0.ends_with('é'));
    }

    #[test]
    fn prefix_decoding() {
        let s = "aé€😀".repeat(100);
        let (t, err, trunc) = decode_text_prefix(s.as_bytes(), None, 10);
        assert!(!err && trunc);
        assert_eq!(t, "aé€😀");
        // Cutting inside the emoji holds the partial sequence back (no U+FFFD).
        let (t, err, trunc) = decode_text_prefix(s.as_bytes(), None, 9);
        assert!(!err && trunc);
        assert_eq!(t, "aé€");
        let (t, _, trunc) = decode_text_prefix(b"short", None, 10);
        assert_eq!((t.as_str(), trunc), ("short", false));
        let (t, _, trunc) =
            decode_text_prefix(b"\xe9\xe9\xe9\xe9", Some("text/plain; charset=latin1"), 3);
        assert_eq!((t.as_str(), trunc), ("é", true));
        let (t, _, trunc) = decode_text_prefix(b"", None, 0);
        assert_eq!((t.as_str(), trunc), ("", false));
        let (t, _, trunc) = decode_text_prefix(b"abc", None, 0);
        assert_eq!((t.as_str(), trunc), ("", true));
    }
}
