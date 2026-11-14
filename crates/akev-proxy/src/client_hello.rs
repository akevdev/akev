//! TLS ClientHello parsing from raw peeked bytes, plus JA3 and JA4 fingerprints.
//!
//! The hello is parsed before the TLS handshake starts (the bytes are replayed into rustls
//! afterwards), so Akev can decide on passthrough, pick the leaf certificate and ALPN list,
//! and fingerprint the client without depending on what rustls exposes.

use md5::{Digest as _, Md5};
use sha2::Sha256;

/// Fields of a ClientHello relevant for routing and fingerprinting.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClientHello {
    /// `legacy_version` field of the hello (e.g. 0x0303).
    pub legacy_version: u16,
    /// Cipher suites in client order (GREASE included).
    pub cipher_suites: Vec<u16>,
    /// Extension types in client order (GREASE included).
    pub extensions: Vec<u16>,
    /// `server_name` (first host_name entry).
    pub sni: Option<String>,
    /// ALPN protocols in client order.
    pub alpn: Vec<String>,
    /// `supported_versions` list (GREASE included).
    pub supported_versions: Vec<u16>,
    /// `supported_groups` (elliptic curves) list (GREASE included).
    pub groups: Vec<u16>,
    /// `ec_point_formats` list.
    pub ec_point_formats: Vec<u8>,
    /// `signature_algorithms` in client order.
    pub signature_algorithms: Vec<u16>,
}

/// Outcome of trying to parse a ClientHello from the start of a stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HelloParse {
    /// A complete ClientHello was parsed.
    Complete(ClientHello),
    /// The hello isn't complete yet; read more bytes and retry.
    NeedMore,
    /// Not a (valid) TLS ClientHello; the reason is for diagnostics.
    Invalid(&'static str),
}

/// Upper bound for a reassembled ClientHello (post-quantum key shares make them ~2KB; anything
/// near this limit is abuse).
const MAX_HELLO: usize = 64 * 1024;

/// GREASE values (RFC 8701): 0x0a0a, 0x1a1a, ... 0xfafa.
pub fn is_grease(v: u16) -> bool {
    (v & 0x0f0f) == 0x0a0a && (v >> 8) == (v & 0xff)
}

/// Parse a ClientHello from TLS records at the start of `buf` (handshake messages may span
/// several records).
pub fn parse(buf: &[u8]) -> HelloParse {
    let mut handshake: Vec<u8> = Vec::new();
    let mut pos = 0;
    let mut needed: Option<usize> = None;
    loop {
        if buf.len() < pos + 5 {
            return HelloParse::NeedMore;
        }
        if buf[pos] != 0x16 {
            return HelloParse::Invalid("not a TLS handshake record");
        }
        if buf[pos + 1] != 0x03 {
            return HelloParse::Invalid("unsupported TLS record version");
        }
        let rec_len = u16::from_be_bytes([buf[pos + 3], buf[pos + 4]]) as usize;
        if rec_len == 0 || rec_len > 16384 + 2048 {
            return HelloParse::Invalid("invalid TLS record length");
        }
        if buf.len() < pos + 5 + rec_len {
            return HelloParse::NeedMore;
        }
        handshake.extend_from_slice(&buf[pos + 5..pos + 5 + rec_len]);
        pos += 5 + rec_len;
        if needed.is_none() && handshake.len() >= 4 {
            if handshake[0] != 0x01 {
                return HelloParse::Invalid("first handshake message is not a ClientHello");
            }
            let len = u32::from_be_bytes([0, handshake[1], handshake[2], handshake[3]]) as usize;
            if len + 4 > MAX_HELLO {
                return HelloParse::Invalid("ClientHello too large");
            }
            needed = Some(len + 4);
        }
        if let Some(n) = needed
            && handshake.len() >= n
        {
            return match parse_body(&handshake[4..n]) {
                Some(h) => HelloParse::Complete(h),
                None => HelloParse::Invalid("malformed ClientHello"),
            };
        }
        if pos > MAX_HELLO {
            return HelloParse::Invalid("ClientHello too large");
        }
    }
}

/// Byte reader over a slice; every accessor returns `None` past the end.
struct Reader<'a> {
    buf: &'a [u8],
}

impl<'a> Reader<'a> {
    fn u8(&mut self) -> Option<u8> {
        let (&b, rest) = self.buf.split_first()?;
        self.buf = rest;
        Some(b)
    }
    fn u16(&mut self) -> Option<u16> {
        let b = self.take(2)?;
        Some(u16::from_be_bytes([b[0], b[1]]))
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.buf.len() < n {
            return None;
        }
        let (a, b) = self.buf.split_at(n);
        self.buf = b;
        Some(a)
    }
    fn vec8(&mut self) -> Option<&'a [u8]> {
        let n = self.u8()? as usize;
        self.take(n)
    }
    fn vec16(&mut self) -> Option<&'a [u8]> {
        let n = self.u16()? as usize;
        self.take(n)
    }
    fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
}

fn u16_list(bytes: &[u8]) -> Vec<u16> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_be_bytes([c[0], c[1]]))
        .collect()
}

fn parse_body(body: &[u8]) -> Option<ClientHello> {
    let mut r = Reader { buf: body };
    let mut hello = ClientHello {
        legacy_version: r.u16()?,
        ..Default::default()
    };
    r.take(32)?; // random
    r.vec8()?; // session id
    hello.cipher_suites = u16_list(r.vec16()?);
    r.vec8()?; // compression methods
    if r.is_empty() {
        return Some(hello); // no extensions (very old clients)
    }
    let mut exts = Reader { buf: r.vec16()? };
    while !exts.is_empty() {
        let ty = exts.u16()?;
        let data = exts.vec16()?;
        hello.extensions.push(ty);
        let mut d = Reader { buf: data };
        match ty {
            0x0000 => {
                let mut list = Reader { buf: d.vec16()? };
                while !list.is_empty() {
                    let name_type = list.u8()?;
                    let name = list.vec16()?;
                    if name_type == 0
                        && hello.sni.is_none()
                        && let Ok(s) = std::str::from_utf8(name)
                        && !s.is_empty()
                    {
                        hello.sni = Some(s.trim_end_matches('.').to_ascii_lowercase());
                    }
                }
            }
            0x000a => hello.groups = u16_list(d.vec16()?),
            0x000b => hello.ec_point_formats = d.vec8()?.to_vec(),
            0x000d => hello.signature_algorithms = u16_list(d.vec16()?),
            0x0010 => {
                let mut list = Reader { buf: d.vec16()? };
                while !list.is_empty() {
                    let proto = list.vec8()?;
                    hello.alpn.push(String::from_utf8_lossy(proto).into_owned());
                }
            }
            0x002b => hello.supported_versions = u16_list(d.vec8()?),
            _ => {}
        }
    }
    Some(hello)
}

impl ClientHello {
    /// Highest TLS version the client supports (from `supported_versions`, else legacy).
    pub fn max_version(&self) -> u16 {
        self.supported_versions
            .iter()
            .copied()
            .filter(|v| !is_grease(*v))
            .max()
            .unwrap_or(self.legacy_version)
    }

    /// JA3 fingerprint string: `version,ciphers,extensions,groups,point_formats`.
    pub fn ja3_string(&self) -> String {
        fn join(vals: impl Iterator<Item = u16>) -> String {
            vals.map(|v| v.to_string()).collect::<Vec<_>>().join("-")
        }
        format!(
            "{},{},{},{},{}",
            self.legacy_version,
            join(
                self.cipher_suites
                    .iter()
                    .copied()
                    .filter(|v| !is_grease(*v))
            ),
            join(self.extensions.iter().copied().filter(|v| !is_grease(*v))),
            join(self.groups.iter().copied().filter(|v| !is_grease(*v))),
            join(self.ec_point_formats.iter().map(|&v| v as u16)),
        )
    }

    /// JA3 hash (MD5 hex of [`ClientHello::ja3_string`]).
    pub fn ja3(&self) -> String {
        hex::encode(Md5::digest(self.ja3_string().as_bytes()))
    }

    /// JA4 fingerprint (TCP), e.g. `t13d1516h2_8daaf6152771_e5627efa2ab1`.
    pub fn ja4(&self) -> String {
        let version = match self.max_version() {
            0x0304 => "13",
            0x0303 => "12",
            0x0302 => "11",
            0x0301 => "10",
            0x0300 => "s3",
            0x0002 => "s2",
            0xfeff => "d1",
            0xfefd => "d2",
            0xfefc => "d3",
            _ => "00",
        };
        let sni = if self.extensions.contains(&0x0000) {
            'd'
        } else {
            'i'
        };
        let ciphers: Vec<u16> = self
            .cipher_suites
            .iter()
            .copied()
            .filter(|v| !is_grease(*v))
            .collect();
        let exts: Vec<u16> = self
            .extensions
            .iter()
            .copied()
            .filter(|v| !is_grease(*v))
            .collect();
        let alpn = match self.alpn.first().map(|s| s.as_bytes()) {
            Some(a) if !a.is_empty() => {
                let (f, l) = (a[0], a[a.len() - 1]);
                if f.is_ascii_alphanumeric() && l.is_ascii_alphanumeric() {
                    format!("{}{}", f as char, l as char)
                } else {
                    let h = hex::encode(a);
                    let hb = h.as_bytes();
                    format!("{}{}", hb[0] as char, hb[hb.len() - 1] as char)
                }
            }
            _ => "00".to_string(),
        };
        let a = format!(
            "t{version}{sni}{:02}{:02}{alpn}",
            ciphers.len().min(99),
            exts.len().min(99)
        );

        let mut sorted_ciphers = ciphers.clone();
        sorted_ciphers.sort_unstable();
        let b = truncated_sha256(&hex_list(&sorted_ciphers));

        let mut sorted_exts: Vec<u16> = exts
            .into_iter()
            .filter(|&e| e != 0x0000 && e != 0x0010)
            .collect();
        sorted_exts.sort_unstable();
        let sigs: Vec<u16> = self
            .signature_algorithms
            .iter()
            .copied()
            .filter(|v| !is_grease(*v))
            .collect();
        let c_input = if sigs.is_empty() {
            hex_list(&sorted_exts)
        } else {
            format!("{}_{}", hex_list(&sorted_exts), hex_list(&sigs))
        };
        let c = truncated_sha256(&c_input);
        format!("{a}_{b}_{c}")
    }

    /// Whether the client offers any HTTP flavour via ALPN (or offers no ALPN at all).
    pub fn offers_http(&self) -> bool {
        self.alpn.is_empty()
            || self
                .alpn
                .iter()
                .any(|p| matches!(p.as_str(), "h2" | "http/1.1" | "http/1.0" | "http 1.1"))
    }
}

fn hex_list(vals: &[u16]) -> String {
    vals.iter()
        .map(|v| format!("{v:04x}"))
        .collect::<Vec<_>>()
        .join(",")
}

fn truncated_sha256(input: &str) -> String {
    if input.is_empty() {
        return "000000000000".to_string();
    }
    let digest = hex::encode(Sha256::digest(input.as_bytes()));
    digest[..12].to_string()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Build a ClientHello record from parts (test helper, also used by other modules' tests).
    pub(crate) fn build_hello(
        version: u16,
        ciphers: &[u16],
        exts: &[(u16, Vec<u8>)],
        split_records_at: Option<usize>,
    ) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&version.to_be_bytes());
        body.extend_from_slice(&[7u8; 32]);
        body.push(0); // session id
        body.extend_from_slice(&((ciphers.len() * 2) as u16).to_be_bytes());
        for c in ciphers {
            body.extend_from_slice(&c.to_be_bytes());
        }
        body.extend_from_slice(&[1, 0]); // compression: null
        let mut ext_bytes = Vec::new();
        for (ty, data) in exts {
            ext_bytes.extend_from_slice(&ty.to_be_bytes());
            ext_bytes.extend_from_slice(&(data.len() as u16).to_be_bytes());
            ext_bytes.extend_from_slice(data);
        }
        body.extend_from_slice(&(ext_bytes.len() as u16).to_be_bytes());
        body.extend_from_slice(&ext_bytes);
        let mut hs = vec![0x01];
        hs.extend_from_slice(&(body.len() as u32).to_be_bytes()[1..]);
        hs.extend_from_slice(&body);
        let chunks: Vec<&[u8]> = match split_records_at {
            Some(at) => vec![&hs[..at], &hs[at..]],
            None => vec![&hs[..]],
        };
        let mut out = Vec::new();
        for c in chunks {
            out.extend_from_slice(&[0x16, 0x03, 0x01]);
            out.extend_from_slice(&(c.len() as u16).to_be_bytes());
            out.extend_from_slice(c);
        }
        out
    }

    pub(crate) fn sni_ext(name: &str) -> (u16, Vec<u8>) {
        let mut d = Vec::new();
        d.extend_from_slice(&((name.len() + 3) as u16).to_be_bytes());
        d.push(0);
        d.extend_from_slice(&(name.len() as u16).to_be_bytes());
        d.extend_from_slice(name.as_bytes());
        (0x0000, d)
    }

    pub(crate) fn alpn_ext(protos: &[&str]) -> (u16, Vec<u8>) {
        let mut list = Vec::new();
        for p in protos {
            list.push(p.len() as u8);
            list.extend_from_slice(p.as_bytes());
        }
        let mut d = (list.len() as u16).to_be_bytes().to_vec();
        d.extend_from_slice(&list);
        (0x0010, d)
    }

    fn u16_vec_ext(ty: u16, vals: &[u16], len_bytes: usize) -> (u16, Vec<u8>) {
        let mut d = Vec::new();
        let n = vals.len() * 2;
        if len_bytes == 1 {
            d.push(n as u8);
        } else {
            d.extend_from_slice(&(n as u16).to_be_bytes());
        }
        for v in vals {
            d.extend_from_slice(&v.to_be_bytes());
        }
        (ty, d)
    }

    /// A Chrome-like hello matching the JA4 README example (`t13d1516h2_8daaf6152771_e5627efa2ab1`).
    pub(crate) fn chrome_like_hello(sni: &str) -> Vec<u8> {
        let ciphers = [
            0x2a2a, 0x1301, 0x1302, 0x1303, 0xc02b, 0xc02f, 0xc02c, 0xc030, 0xcca9, 0xcca8, 0xc013,
            0xc014, 0x009c, 0x009d, 0x002f, 0x0035,
        ];
        let exts = vec![
            (0x3a3a, vec![]),
            sni_ext(sni),
            (0x0017, vec![]),
            (0xff01, vec![0]),
            u16_vec_ext(0x000a, &[0x4a4a, 0x001d, 0x0017, 0x0018], 2),
            (0x000b, vec![1, 0]),
            (0x0023, vec![]),
            alpn_ext(&["h2", "http/1.1"]),
            (0x0005, vec![1, 0, 0, 0, 0]),
            u16_vec_ext(
                0x000d,
                &[
                    0x0403, 0x0804, 0x0401, 0x0503, 0x0805, 0x0501, 0x0806, 0x0601,
                ],
                2,
            ),
            (0x0012, vec![]),
            (0x0033, vec![0, 0]),
            (0x002d, vec![1, 1]),
            u16_vec_ext(0x002b, &[0x6a6a, 0x0304, 0x0303], 1),
            (0x001b, vec![2, 0, 2]),
            (0x4469, vec![0, 3, 2, b'h', b'2']),
            (0x0015, vec![0; 10]),
            (0x1a1a, vec![0]),
        ];
        build_hello(0x0303, &ciphers, &exts, None)
    }

    #[test]
    fn grease_detection() {
        assert!(is_grease(0x0a0a));
        assert!(is_grease(0xfafa));
        assert!(!is_grease(0x0a1a));
        assert!(!is_grease(0x1301));
    }

    #[test]
    fn ja4_matches_reference_vector() {
        let raw = chrome_like_hello("example.com");
        let HelloParse::Complete(h) = parse(&raw) else {
            panic!("parse failed")
        };
        assert_eq!(h.sni.as_deref(), Some("example.com"));
        assert_eq!(h.alpn, vec!["h2", "http/1.1"]);
        assert_eq!(h.max_version(), 0x0304);
        assert_eq!(h.ja4(), "t13d1516h2_8daaf6152771_e5627efa2ab1");
        assert_eq!(h.ja4().len(), 36);
        assert!(h.offers_http());
    }

    #[test]
    fn ja3_matches_reference_vector() {
        // Example from the JA3 README.
        let ciphers = [47, 53, 5, 10, 49161, 49162, 49171, 49172, 50, 56, 19, 4];
        let exts = vec![
            sni_ext("a.com"),
            u16_vec_ext(0x000a, &[23, 24, 25], 2),
            (0x000b, vec![1, 0]),
        ];
        let raw = build_hello(0x0301, &ciphers, &exts, None);
        let HelloParse::Complete(h) = parse(&raw) else {
            panic!("parse failed")
        };
        assert_eq!(
            h.ja3_string(),
            "769,47-53-5-10-49161-49162-49171-49172-50-56-19-4,0-10-11,23-24-25,0"
        );
        assert_eq!(h.ja3(), "ada70206e40642a3e4461f35503241d5");
        // No supported_versions → JA4 version from legacy version; no ALPN → "00".
        assert!(h.ja4().starts_with("t10d120300_"));
    }

    #[test]
    fn ja3_ignores_grease() {
        let raw = chrome_like_hello("x.test");
        let HelloParse::Complete(h) = parse(&raw) else {
            panic!()
        };
        let s = h.ja3_string();
        assert!(s.starts_with("771,4865-4866-4867-49195-49199-49196-49200-52393-52392-49171-49172-156-157-47-53,0-23-65281-10-11-35-16-5-13-18-51-45-43-27-17513-21,29-23-24,0"), "{s}");
        assert_eq!(h.ja3().len(), 32);
    }

    #[test]
    fn handles_fragmentation_and_partial_input() {
        let raw = chrome_like_hello("frag.example");
        for i in 0..raw.len() {
            assert_eq!(parse(&raw[..i]), HelloParse::NeedMore, "prefix {i}");
        }
        // Split inside the handshake header (3 bytes in: type + partial length) and mid-body.
        for at in [3, 10, 40] {
            let split = build_hello(
                0x0303,
                &[0x1301],
                &[sni_ext("split.example"), alpn_ext(&["http/1.1"])],
                Some(at),
            );
            let HelloParse::Complete(h) = parse(&split) else {
                panic!("fragmented hello not parsed (split {at})")
            };
            assert_eq!(h.sni.as_deref(), Some("split.example"));
        }
        let split = build_hello(
            0x0303,
            &[0x1301],
            &[sni_ext("split.example"), alpn_ext(&["http/1.1"])],
            Some(10),
        );
        let HelloParse::Complete(h) = parse(&split) else {
            panic!("fragmented hello not parsed")
        };
        assert_eq!(h.sni.as_deref(), Some("split.example"));
        // Partial second record.
        assert_eq!(parse(&split[..split.len() - 1]), HelloParse::NeedMore);
    }

    #[test]
    fn rejects_garbage() {
        assert!(matches!(
            parse(b"GET / HTTP/1.1\r\n"),
            HelloParse::Invalid(_)
        ));
        assert!(matches!(
            parse(&[0x16, 0x03, 0x01, 0x00, 0x04, 0x02, 0, 0, 0]),
            HelloParse::Invalid(_)
        ));
        // Truncated hello body declared as complete.
        let mut raw = vec![
            0x16, 0x03, 0x01, 0x00, 0x08, 0x01, 0x00, 0x00, 0x04, 0x03, 0x03, 0x00, 0x00,
        ];
        assert!(matches!(parse(&raw), HelloParse::Invalid(_)));
        raw[4] = 0;
        assert!(matches!(parse(&raw), HelloParse::Invalid(_)));
    }

    #[test]
    fn alpn_non_alphanumeric_uses_hex() {
        let raw = build_hello(0x0303, &[0x1301], &[alpn_ext(&["\u{1}x\u{2}"])], None);
        let HelloParse::Complete(h) = parse(&raw) else {
            panic!()
        };
        // hex "017802" → first '0', last '2'
        assert_eq!(&h.ja4()[8..10], "02");
        assert!(!h.offers_http());
        assert!(h.ja4().starts_with("t12i0101"));
    }

    #[test]
    fn parses_real_rustls_client_hello() {
        use std::sync::Arc;
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let mut cfg = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
        cfg.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        let name = rustls_pki_types::ServerName::try_from("real.example.org").unwrap();
        let mut conn = rustls::ClientConnection::new(Arc::new(cfg), name).unwrap();
        let mut out = Vec::new();
        conn.write_tls(&mut out).unwrap();
        let HelloParse::Complete(h) = parse(&out) else {
            panic!("rustls hello not parsed")
        };
        assert_eq!(h.sni.as_deref(), Some("real.example.org"));
        assert_eq!(h.alpn, vec!["h2", "http/1.1"]);
        assert_eq!(h.max_version(), 0x0304);
        assert!(h.ja4().starts_with("t13d"));
        assert!(h.ja4().contains("h2_"));
    }
}
