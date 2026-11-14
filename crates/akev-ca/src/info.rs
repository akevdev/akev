//! Derived, display-oriented facts about the CA certificate.

use base64::Engine as _;
use md5::{Digest as _, Md5};
use ring::digest::{SHA256, digest};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use x509_parser::certificate::X509Certificate;

use crate::{CaExpiry, CaInfo, EXPIRY_WARNING_DAYS};

/// Everything in [`CaInfo`] except the time-dependent `days_remaining`, computed once.
#[derive(Clone, Debug)]
pub(crate) struct CaDetails {
    pub(crate) subject: String,
    pub(crate) sha256_fingerprint: String,
    pub(crate) spki_fingerprint: String,
    pub(crate) subject_hash_old: String,
    pub(crate) not_before: OffsetDateTime,
    pub(crate) not_after: OffsetDateTime,
}

impl CaDetails {
    pub(crate) fn from_cert(cert: &X509Certificate<'_>, der: &[u8]) -> Self {
        let tbs = &cert.tbs_certificate;
        CaDetails {
            subject: tbs.subject.to_string(),
            sha256_fingerprint: colon_hex(digest(&SHA256, der).as_ref()),
            spki_fingerprint: spki_fingerprint(tbs.subject_pki.raw),
            subject_hash_old: subject_hash_old(tbs.subject.as_raw()),
            not_before: tbs.validity.not_before.to_datetime(),
            not_after: tbs.validity.not_after.to_datetime(),
        }
    }

    pub(crate) fn to_info(&self, now: OffsetDateTime) -> CaInfo {
        let remaining = self.not_after - now;
        let expiry = if remaining <= time::Duration::ZERO {
            CaExpiry::Expired
        } else if remaining < time::Duration::days(EXPIRY_WARNING_DAYS) {
            CaExpiry::Expiring
        } else {
            CaExpiry::Valid
        };
        CaInfo {
            subject: self.subject.clone(),
            sha256_fingerprint: self.sha256_fingerprint.clone(),
            spki_fingerprint: self.spki_fingerprint.clone(),
            subject_hash_old: self.subject_hash_old.clone(),
            not_before: rfc3339(self.not_before),
            not_after: rfc3339(self.not_after),
            days_remaining: remaining.whole_days(),
            expiry,
        }
    }
}

/// Format a timestamp as RFC 3339 (`2026-11-08T12:00:00Z`).
pub(crate) fn rfc3339(t: OffsetDateTime) -> String {
    // Certificate times are always UTC with whole seconds; years outside 0..=9999 (which
    // RFC 3339 cannot express) are impossible in X.509 time encodings.
    t.format(&Rfc3339).unwrap_or_else(|_| t.to_string())
}

/// Uppercase hex with `:` separators, as shown by browsers and `openssl x509 -fingerprint`.
pub(crate) fn colon_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(bytes.len() * 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 {
            out.push(':');
        }
        out.push(char::from(HEX[usize::from(b >> 4)]));
        out.push(char::from(HEX[usize::from(b & 0x0f)]));
    }
    out
}

/// Base64 SHA-256 over the DER SubjectPublicKeyInfo (HPKP / Chrome SPKI pin format).
pub(crate) fn spki_fingerprint(spki_der: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(digest(&SHA256, spki_der))
}

/// OpenSSL `X509_NAME_hash_old`: MD5 over the DER-encoded subject Name, first four bytes read
/// little-endian, printed as 8 lowercase hex digits (Android's `<hash>.0` store file name).
pub(crate) fn subject_hash_old(subject_der: &[u8]) -> String {
    let md5 = Md5::digest(subject_der);
    let hash = u32::from_le_bytes([md5[0], md5[1], md5[2], md5[3]]);
    format!("{hash:08x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colon_hex_formats() {
        assert_eq!(colon_hex(&[]), "");
        assert_eq!(colon_hex(&[0x0a]), "0A");
        assert_eq!(colon_hex(&[0xde, 0xad, 0xbe, 0xef]), "DE:AD:BE:EF");
    }

    #[test]
    fn subject_hash_old_is_little_endian_md5_prefix() {
        // MD5("") = d41d8cd98f00b204e9800998ecf8427e → bytes d4 1d 8c d9 read LE.
        assert_eq!(subject_hash_old(b""), "d98c1dd4");
    }

    #[test]
    fn spki_fingerprint_is_standard_base64_of_sha256() {
        // SHA-256("") = e3b0c442...
        assert_eq!(
            spki_fingerprint(b""),
            "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU="
        );
    }

    #[test]
    fn rfc3339_formats_utc() {
        let t = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        assert_eq!(rfc3339(t), "2023-11-14T22:13:20Z");
    }
}
