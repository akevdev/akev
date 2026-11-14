//! JSON Web Token decoding (no signature verification).

use serde::Serialize;

/// A decoded (unverified) JWT.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecodedJwt {
    /// The JOSE header (`alg`, `typ`, `kid`, ...).
    pub header: serde_json::Value,
    /// The claims set.
    pub payload: serde_json::Value,
    /// `exp` as RFC3339 if present.
    pub expires_at: Option<String>,
    /// `iat` as RFC3339 if present.
    pub issued_at: Option<String>,
    /// Whether `exp` is in the past according to the system clock (None without `exp`).
    pub expired: Option<bool>,
}

/// Decode (without verifying) a JWT. Accepts a bare token or `Bearer <token>`.
///
/// Only JWS compact serialization (three dot-separated base64url parts; the signature may be
/// empty for `alg: none`) is accepted, and both header and payload must be JSON objects.
/// Padding and the standard base64 alphabet are tolerated. Numeric `exp` / `iat` (seconds,
/// possibly fractional) are converted to RFC 3339 UTC.
pub fn decode(token: &str) -> Option<DecodedJwt> {
    decode_at(token, now_secs())
}

fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

pub(crate) fn decode_at(token: &str, now: f64) -> Option<DecodedJwt> {
    let mut t = token.trim();
    if t.len() > 7 && t.as_bytes()[..7].eq_ignore_ascii_case(b"bearer ") {
        t = t[7..].trim_start();
    }
    let t = t.trim_matches('"');
    let mut parts = t.split('.');
    let (h, p, sig) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || h.is_empty() || p.is_empty() {
        return None;
    }
    let valid_chars = |s: &str| {
        s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'+' | b'/' | b'='))
    };
    if !valid_chars(sig) {
        return None;
    }
    let header = json_object(h)?;
    let payload = json_object(p)?;
    let exp = payload.get("exp").and_then(serde_json::Value::as_f64);
    let iat = payload.get("iat").and_then(serde_json::Value::as_f64);
    Some(DecodedJwt {
        expires_at: exp.and_then(rfc3339_from_secs),
        issued_at: iat.and_then(rfc3339_from_secs),
        expired: exp.map(|e| e <= now),
        header,
        payload,
    })
}

fn json_object(part: &str) -> Option<serde_json::Value> {
    use base64::Engine as _;
    let bytes = if part.contains(['+', '/']) {
        crate::util::B64_LENIENT.decode(part).ok()?
    } else {
        crate::util::B64_URL_LENIENT.decode(part).ok()?
    };
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    v.is_object().then_some(v)
}

fn rfc3339_from_secs(secs: f64) -> Option<String> {
    if !secs.is_finite() || secs.abs() > 1e15 {
        return None;
    }
    let whole = secs.floor();
    let nanos = ((secs - whole) * 1e9).round().min(999_999_999.0) as u32;
    // Round to milliseconds for display stability.
    let nanos = (nanos / 1_000_000) * 1_000_000;
    crate::util::format_rfc3339(whole as i64, nanos)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use serde_json::json;

    fn token(header: &serde_json::Value, payload: &serde_json::Value) -> String {
        let e = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        format!(
            "{}.{}.c2lnbmF0dXJl",
            e.encode(header.to_string()),
            e.encode(payload.to_string())
        )
    }

    #[test]
    fn decodes_standard_token() {
        // jwt.io sample
        let t = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyfQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";
        let d = decode(t).unwrap();
        assert_eq!(d.header, json!({"alg": "HS256", "typ": "JWT"}));
        assert_eq!(d.payload["name"], "John Doe");
        assert_eq!(d.issued_at.as_deref(), Some("2018-01-18T01:30:22Z"));
        assert_eq!(d.expires_at, None);
        assert_eq!(d.expired, None);
        assert_eq!(decode(&format!("Bearer {t}")).unwrap(), d);
        assert_eq!(decode(&format!("  bearer   {t} ")).unwrap(), d);
    }

    #[test]
    fn expiry() {
        let t = token(
            &json!({"alg": "none"}),
            &json!({"exp": 2_000_000_000u64, "iat": 1_700_000_000.25}),
        );
        let d = decode_at(&t, 1_900_000_000.0).unwrap();
        assert_eq!(d.expires_at.as_deref(), Some("2033-05-18T03:33:20Z"));
        assert_eq!(d.issued_at.as_deref(), Some("2023-11-14T22:13:20.250Z"));
        assert_eq!(d.expired, Some(false));
        assert_eq!(decode_at(&t, 2_100_000_000.0).unwrap().expired, Some(true));
        // System clock: year 2033 is in the future relative to "now" only until then — use a
        // token that's definitely expired.
        let old = token(&json!({"alg": "HS256"}), &json!({"exp": 1}));
        assert_eq!(decode(&old).unwrap().expired, Some(true));
        // Non-numeric exp is ignored.
        let s = token(&json!({"alg": "HS256"}), &json!({"exp": "soon"}));
        assert_eq!(decode(&s).unwrap().expired, None);
    }

    #[test]
    fn rejects_non_jwts() {
        assert!(decode("not a token").is_none());
        assert!(decode("bearer\u{fffd}x.y.z").is_none()); // regression: char-boundary slicing
        assert!(decode("a.b").is_none());
        assert!(decode("a.b.c.d.e").is_none()); // JWE
        assert!(decode("eyJhIjoxfQ.bm90IGpzb24.sig").is_none()); // payload not JSON
        assert!(decode("WzFd.eyJhIjoxfQ.sig").is_none()); // header is an array
        let t = token(&json!({"alg": "HS256"}), &json!({"a": 1}));
        assert!(decode(&format!("{t} extra")).is_none());
        // alg none with empty signature is fine
        let none = t.rsplit_once('.').unwrap().0.to_string() + ".";
        assert!(decode(&none).is_some());
    }
}
