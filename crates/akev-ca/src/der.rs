//! Minimal DER plumbing used to re-wrap legacy private-key encodings as PKCS#8.
//!
//! `ring` (and therefore `rcgen` built on it) only accepts PKCS#8 private keys, while users
//! bringing their own CA frequently have OpenSSL "traditional" keys: PKCS#1 (`BEGIN RSA
//! PRIVATE KEY`) or SEC1 (`BEGIN EC PRIVATE KEY`). Both are wrapped losslessly into a
//! `PrivateKeyInfo` structure (RFC 5958) here — no crypto involved, just framing.

/// `rsaEncryption` (1.2.840.113549.1.1.1).
const OID_RSA_ENCRYPTION: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];
/// `id-ecPublicKey` (1.2.840.10045.2.1).
const OID_EC_PUBLIC_KEY: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
/// `prime256v1` / `secp256r1` (1.2.840.10045.3.1.7).
const OID_P256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];
/// `secp384r1` (1.3.132.0.34).
const OID_P384: &[u8] = &[0x2b, 0x81, 0x04, 0x00, 0x22];
/// `secp521r1` (1.3.132.0.35).
const OID_P521: &[u8] = &[0x2b, 0x81, 0x04, 0x00, 0x23];

const TAG_INTEGER: u8 = 0x02;
const TAG_OCTET_STRING: u8 = 0x04;
const TAG_NULL: u8 = 0x05;
const TAG_OID: u8 = 0x06;
const TAG_SEQUENCE: u8 = 0x30;
const TAG_CONTEXT_0: u8 = 0xa0;
const TAG_CONTEXT_1: u8 = 0xa1;

/// Append a DER length.
fn push_len(out: &mut Vec<u8>, len: usize) {
    if len < 0x80 {
        // Short form; the guard makes the narrowing lossless.
        out.push(len as u8);
    } else {
        let bytes = len.to_be_bytes();
        let skip = bytes.iter().take_while(|b| **b == 0).count();
        let significant = &bytes[skip..];
        out.push(0x80 | significant.len() as u8);
        out.extend_from_slice(significant);
    }
}

/// Encode one tag-length-value element.
pub(crate) fn tlv(tag: u8, value: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(value.len() + 6);
    out.push(tag);
    push_len(&mut out, value.len());
    out.extend_from_slice(value);
    out
}

/// Build a version-0 `PrivateKeyInfo` around an algorithm identifier body and a key.
fn pkcs8(algorithm_identifier_body: &[u8], private_key: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(private_key.len() + algorithm_identifier_body.len() + 16);
    body.extend_from_slice(&[TAG_INTEGER, 0x01, 0x00]);
    body.extend_from_slice(&tlv(TAG_SEQUENCE, algorithm_identifier_body));
    body.extend_from_slice(&tlv(TAG_OCTET_STRING, private_key));
    tlv(TAG_SEQUENCE, &body)
}

/// Wrap a PKCS#1 `RSAPrivateKey` into PKCS#8.
pub(crate) fn pkcs1_to_pkcs8(pkcs1: &[u8]) -> Vec<u8> {
    let mut alg = tlv(TAG_OID, OID_RSA_ENCRYPTION);
    alg.extend_from_slice(&[TAG_NULL, 0x00]);
    pkcs8(&alg, pkcs1)
}

/// Wrap a SEC1 `ECPrivateKey` into PKCS#8.
///
/// The curve comes from the key's `[0] parameters`; when absent it is inferred from the
/// private scalar length. The `[1] publicKey` field is required because `ring` refuses keys
/// without it (and cannot derive it).
pub(crate) fn sec1_to_pkcs8(sec1: &[u8]) -> Result<Vec<u8>, String> {
    let curve = sec1_curve(sec1)?;
    let mut alg = tlv(TAG_OID, OID_EC_PUBLIC_KEY);
    alg.extend_from_slice(&tlv(TAG_OID, curve));
    Ok(pkcs8(&alg, sec1))
}

fn sec1_curve(sec1: &[u8]) -> Result<&'static [u8], String> {
    const MALFORMED: &str = "malformed SEC1 EC private key";
    let mut outer = Reader::new(sec1);
    let body = outer.read_tagged(TAG_SEQUENCE).ok_or(MALFORMED)?;
    if !outer.is_empty() {
        return Err(MALFORMED.into());
    }
    let mut fields = Reader::new(body);
    if fields.read_tagged(TAG_INTEGER).ok_or(MALFORMED)? != [0x01] {
        return Err("unsupported SEC1 EC private key version".into());
    }
    let scalar = fields.read_tagged(TAG_OCTET_STRING).ok_or(MALFORMED)?;
    let mut named_curve = None;
    let mut has_public_key = false;
    while !fields.is_empty() {
        let (tag, value) = fields.next().ok_or(MALFORMED)?;
        match tag {
            TAG_CONTEXT_0 => {
                let oid = Reader::new(value).read_tagged(TAG_OID).ok_or(
                    "SEC1 EC key uses explicit curve parameters (only named curves are supported)",
                )?;
                named_curve = Some(oid);
            }
            TAG_CONTEXT_1 => has_public_key = true,
            _ => {}
        }
    }
    if !has_public_key {
        return Err(
            "SEC1 EC private key does not embed its public key; convert it with \
                    `openssl pkcs8 -topk8 -nocrypt -in ca.key -out ca-pkcs8.key`"
                .into(),
        );
    }
    match named_curve {
        Some(oid) if oid == OID_P256 => Ok(OID_P256),
        Some(oid) if oid == OID_P384 => Ok(OID_P384),
        Some(oid) if oid == OID_P521 => {
            Err("EC curve P-521 is not supported (use P-256, P-384, RSA or Ed25519)".into())
        }
        Some(_) => Err("unsupported EC curve (use P-256, P-384, RSA or Ed25519)".into()),
        None => match scalar.len() {
            32 => Ok(OID_P256),
            48 => Ok(OID_P384),
            _ => Err("cannot determine the curve of the SEC1 EC private key".into()),
        },
    }
}

/// A tiny forward-only DER reader (definite lengths only, as DER requires).
struct Reader<'a> {
    data: &'a [u8],
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Reader { data }
    }

    fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Read the next element, returning its tag and contents.
    fn next(&mut self) -> Option<(u8, &'a [u8])> {
        let (&tag, rest) = self.data.split_first()?;
        if tag & 0x1f == 0x1f {
            return None; // multi-byte tags never occur in the structures we read
        }
        let (&first, mut rest) = rest.split_first()?;
        let len = if first < 0x80 {
            usize::from(first)
        } else {
            let n = usize::from(first & 0x7f);
            if n == 0 || n > 4 || rest.len() < n {
                return None; // indefinite or absurd length
            }
            let (len_bytes, tail) = rest.split_at(n);
            rest = tail;
            len_bytes
                .iter()
                .fold(0usize, |acc, b| (acc << 8) | usize::from(*b))
        };
        if rest.len() < len {
            return None;
        }
        let (value, tail) = rest.split_at(len);
        self.data = tail;
        Some((tag, value))
    }

    /// Read the next element and require it to carry `tag`.
    fn read_tagged(&mut self, tag: u8) -> Option<&'a [u8]> {
        match self.next()? {
            (t, value) if t == tag => Some(value),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_use_minimal_encoding() {
        let mut out = Vec::new();
        push_len(&mut out, 0x7f);
        assert_eq!(out, [0x7f]);
        out.clear();
        push_len(&mut out, 0x80);
        assert_eq!(out, [0x81, 0x80]);
        out.clear();
        push_len(&mut out, 0x1234);
        assert_eq!(out, [0x82, 0x12, 0x34]);
    }

    #[test]
    fn reader_round_trips_tlv() {
        let long = vec![0xab; 300];
        let encoded = [tlv(TAG_OCTET_STRING, b"hi"), tlv(TAG_OCTET_STRING, &long)].concat();
        let mut r = Reader::new(&encoded);
        assert_eq!(r.read_tagged(TAG_OCTET_STRING), Some(&b"hi"[..]));
        assert_eq!(r.read_tagged(TAG_OCTET_STRING), Some(&long[..]));
        assert!(r.is_empty());
    }

    #[test]
    fn reader_rejects_truncated_and_indefinite() {
        assert!(Reader::new(&[0x04, 0x05, 0x00]).next().is_none());
        assert!(Reader::new(&[0x30, 0x80, 0x00, 0x00]).next().is_none());
        assert!(Reader::new(&[0x04]).next().is_none());
    }

    #[test]
    fn sec1_rejects_garbage() {
        assert!(sec1_to_pkcs8(b"not der").is_err());
        assert!(sec1_to_pkcs8(&tlv(TAG_SEQUENCE, &[])).is_err());
    }

    #[test]
    fn sec1_without_curve_infers_from_scalar_length() {
        // version 1, 32-byte scalar, [1] public key (contents irrelevant for curve detection).
        let body = [
            vec![TAG_INTEGER, 0x01, 0x01],
            tlv(TAG_OCTET_STRING, &[7u8; 32]),
            tlv(TAG_CONTEXT_1, &[0x03, 0x02, 0x00, 0x04]),
        ]
        .concat();
        assert_eq!(sec1_curve(&tlv(TAG_SEQUENCE, &body)), Ok(OID_P256));

        let body48 = [
            vec![TAG_INTEGER, 0x01, 0x01],
            tlv(TAG_OCTET_STRING, &[7u8; 48]),
            tlv(TAG_CONTEXT_1, &[0x03, 0x02, 0x00, 0x04]),
        ]
        .concat();
        assert_eq!(sec1_curve(&tlv(TAG_SEQUENCE, &body48)), Ok(OID_P384));
    }

    #[test]
    fn sec1_rejects_p521_and_missing_public_key() {
        let p521 = [
            vec![TAG_INTEGER, 0x01, 0x01],
            tlv(TAG_OCTET_STRING, &[7u8; 66]),
            tlv(TAG_CONTEXT_0, &tlv(TAG_OID, OID_P521)),
            tlv(TAG_CONTEXT_1, &[0x03, 0x02, 0x00, 0x04]),
        ]
        .concat();
        let err = sec1_curve(&tlv(TAG_SEQUENCE, &p521)).unwrap_err();
        assert!(err.contains("P-521"), "{err}");

        let no_pub = [
            vec![TAG_INTEGER, 0x01, 0x01],
            tlv(TAG_OCTET_STRING, &[7u8; 32]),
            tlv(TAG_CONTEXT_0, &tlv(TAG_OID, OID_P256)),
        ]
        .concat();
        let err = sec1_curve(&tlv(TAG_SEQUENCE, &no_pub)).unwrap_err();
        assert!(err.contains("public key"), "{err}");
    }

    #[test]
    fn pkcs1_wrapping_has_expected_framing() {
        let wrapped = pkcs1_to_pkcs8(&[0x30, 0x00]);
        let mut outer = Reader::new(&wrapped);
        let body = outer.read_tagged(TAG_SEQUENCE).unwrap();
        let mut fields = Reader::new(body);
        assert_eq!(fields.read_tagged(TAG_INTEGER), Some(&[0u8][..]));
        let alg = fields.read_tagged(TAG_SEQUENCE).unwrap();
        let mut alg_reader = Reader::new(alg);
        assert_eq!(alg_reader.read_tagged(TAG_OID), Some(OID_RSA_ENCRYPTION));
        assert_eq!(alg_reader.read_tagged(TAG_NULL), Some(&[][..]));
        assert_eq!(
            fields.read_tagged(TAG_OCTET_STRING),
            Some(&[0x30, 0x00][..])
        );
    }
}
