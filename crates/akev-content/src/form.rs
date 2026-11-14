//! `application/x-www-form-urlencoded` parsing and serialization (WHATWG URL Standard).

/// Parse application/x-www-form-urlencoded (also used for query strings), preserving
/// order and duplicates; `+` → space; invalid percent-escapes kept literally.
///
/// A single leading `?` is ignored (so a raw query string can be passed). Empty `&&` segments
/// are skipped; a segment without `=` has an empty value. Percent-decoded bytes are interpreted
/// as UTF-8 with replacement.
pub fn parse_urlencoded(s: &str) -> Vec<(String, String)> {
    urlencoded_pairs(s).collect()
}

/// The pairs of [`parse_urlencoded`], decoded lazily one at a time (so callers can stop early
/// on huge inputs).
pub fn urlencoded_pairs(s: &str) -> impl Iterator<Item = (String, String)> + '_ {
    let s = s.strip_prefix('?').unwrap_or(s);
    s.split('&').filter(|seg| !seg.is_empty()).map(|seg| {
        let (k, v) = seg.split_once('=').unwrap_or((seg, ""));
        (decode_component(k), decode_component(v))
    })
}

fn decode_component(s: &str) -> String {
    if !s.bytes().any(|b| b == b'+' || b == b'%') {
        return s.to_owned();
    }
    let replaced: Vec<u8> = s
        .bytes()
        .map(|b| if b == b'+' { b' ' } else { b })
        .collect();
    percent_encoding::percent_decode(&replaced)
        .decode_utf8_lossy()
        .into_owned()
}

/// Serialize pairs as urlencoded.
///
/// Per the WHATWG application/x-www-form-urlencoded serializer: ASCII alphanumerics and
/// `*-._` are kept, space becomes `+`, every other byte of the UTF-8 encoding is
/// percent-encoded (uppercase hex).
pub fn to_urlencoded(pairs: &[(String, String)]) -> String {
    let mut out = String::with_capacity(pairs.iter().map(|(k, v)| k.len() + v.len() + 2).sum());
    for (i, (k, v)) in pairs.iter().enumerate() {
        if i > 0 {
            out.push('&');
        }
        encode_into(k, &mut out);
        out.push('=');
        encode_into(v, &mut out);
    }
    out
}

fn encode_into(s: &str, out: &mut String) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for &b in s.as_bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => {
                out.push('%');
                out.push(HEX[(b >> 4) as usize] as char);
                out.push(HEX[(b & 0xf) as usize] as char);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    #[test]
    fn parse_rules() {
        assert_eq!(
            parse_urlencoded("a=1&b=two+words&a=3"),
            p(&[("a", "1"), ("b", "two words"), ("a", "3")])
        );
        assert_eq!(
            parse_urlencoded("?q=%E2%82%AC%2B&&flag&=v&k="),
            p(&[("q", "€+"), ("flag", ""), ("", "v"), ("k", "")])
        );
        assert_eq!(
            parse_urlencoded("bad=%zz%4&pct=100%"),
            p(&[("bad", "%zz%4"), ("pct", "100%")])
        );
        assert_eq!(parse_urlencoded("x=a=b"), p(&[("x", "a=b")]));
        assert_eq!(parse_urlencoded("inv=%ff"), p(&[("inv", "\u{FFFD}")]));
        assert_eq!(parse_urlencoded(""), vec![]);
    }

    #[test]
    fn serialize_rules() {
        assert_eq!(
            to_urlencoded(&p(&[("a b", "c&d=e"), ("€", "*-._~!")])),
            "a+b=c%26d%3De&%E2%82%AC=*-._%7E%21"
        );
        assert_eq!(to_urlencoded(&[]), "");
    }

    #[test]
    fn round_trip() {
        let pairs = p(&[
            ("name", "Jöhn Doe"),
            ("q", "a+b=c&d"),
            ("", ""),
            ("emoji", "😀 %"),
        ]);
        assert_eq!(parse_urlencoded(&to_urlencoded(&pairs)), pairs);
    }
}
