//! Browser fingerprint data: the TLS ClientHello and HTTP/2 connection preface of each
//! impersonated browser.
//!
//! Plain data (no BoringSSL types), so it is compiled into every build: the catalogue the API
//! serves, the expected fingerprints the tests check against and the backend configuration
//! all come from here.
//!
//! Sources: captures of the real browsers as published by tls.peet.ws / the JA4+ database and
//! the curl-impersonate / wreq projects. Every expected JA4 below is recomputed from the data
//! in the unit tests (`ja4_constants_match_the_data`), so the data and the documented
//! fingerprints can't drift apart.

// Without the backend only the catalogue (expected JA4 / HTTP/2 fingerprints) reads the data.
#![cfg_attr(not(feature = "impersonate"), allow(dead_code))]

use super::Browser;

/// IANA cipher suite / extension / group / signature algorithm ids used below.
pub(crate) mod ids {
    pub const SERVER_NAME: u16 = 0;
    pub const STATUS_REQUEST: u16 = 5;
    pub const SUPPORTED_GROUPS: u16 = 10;
    pub const EC_POINT_FORMATS: u16 = 11;
    pub const SIGNATURE_ALGORITHMS: u16 = 13;
    pub const ALPN: u16 = 16;
    pub const SCT: u16 = 18;
    pub const PADDING: u16 = 21;
    pub const EXTENDED_MASTER_SECRET: u16 = 23;
    pub const COMPRESS_CERTIFICATE: u16 = 27;
    pub const RECORD_SIZE_LIMIT: u16 = 28;
    pub const DELEGATED_CREDENTIALS: u16 = 34;
    pub const SESSION_TICKET: u16 = 35;
    pub const SUPPORTED_VERSIONS: u16 = 43;
    pub const PSK_KEY_EXCHANGE_MODES: u16 = 45;
    pub const KEY_SHARE: u16 = 51;
    pub const ALPS_NEW: u16 = 17613;
    pub const ECH: u16 = 65037;
    pub const RENEGOTIATION_INFO: u16 = 65281;
}

use ids::*;

/// Certificate compression algorithms (RFC 8879).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CertCompression {
    Zlib,
    Brotli,
    Zstd,
}

/// One browser's ClientHello.
#[derive(Debug)]
pub(crate) struct TlsSpec {
    /// Cipher suites in wire order (GREASE excluded).
    pub ciphers: &'static [u16],
    /// The same list as a BoringSSL cipher string (TLS 1.3 suites included, order kept).
    pub cipher_list: &'static str,
    /// `supported_groups` in wire order (GREASE excluded).
    pub groups: &'static [u16],
    /// The same list as a BoringSSL curves string.
    pub curves: &'static str,
    /// `signature_algorithms` in wire order (duplicates included: Safari sends one).
    pub sigalgs: &'static [u16],
    /// The same list as a BoringSSL sigalgs string.
    pub sigalgs_list: &'static str,
    /// Extensions sent (GREASE excluded). In wire order unless `permute_extensions`, in which
    /// case the order is randomised per connection (Chrome since v110) and this is the set.
    pub extensions: &'static [u16],
    /// Chrome-style random extension order.
    pub permute_extensions: bool,
    /// GREASE values in ciphers / extensions / groups / versions (RFC 8701).
    pub grease: bool,
    /// ALPS (`application_settings`, new codepoint) for `h2`.
    pub alps: bool,
    pub cert_compression: &'static [CertCompression],
    /// Send a GREASE `encrypted_client_hello` extension.
    pub ech_grease: bool,
    pub session_ticket: bool,
    pub record_size_limit: Option<u16>,
    /// `delegated_credentials` signature algorithms (BoringSSL sigalgs string).
    pub delegated_credentials: Option<&'static str>,
    /// Most key shares sent (BoringSSL sends two by default when the first group is a
    /// post-quantum hybrid).
    pub key_shares_limit: Option<u8>,
    /// JA4 when `h2` + `http/1.1` are offered (the `h1` variant differs only in the ALPN part).
    pub ja4: &'static str,
}

/// HTTP/2 SETTINGS identifiers.
pub(crate) mod setting {
    pub const HEADER_TABLE_SIZE: u16 = 1;
    pub const ENABLE_PUSH: u16 = 2;
    pub const MAX_CONCURRENT_STREAMS: u16 = 3;
    pub const INITIAL_WINDOW_SIZE: u16 = 4;
    pub const MAX_FRAME_SIZE: u16 = 5;
    pub const MAX_HEADER_LIST_SIZE: u16 = 6;
    pub const NO_RFC7540_PRIORITIES: u16 = 9;
}

/// HTTP/2 pseudo-header fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pseudo {
    Method,
    Authority,
    Scheme,
    Path,
}

impl Pseudo {
    fn letter(self) -> char {
        match self {
            Pseudo::Method => 'm',
            Pseudo::Authority => 'a',
            Pseudo::Scheme => 's',
            Pseudo::Path => 'p',
        }
    }
}

/// One browser's HTTP/2 connection preface and request framing.
#[derive(Debug)]
pub(crate) struct H2Spec {
    /// SETTINGS parameters `(id, value)` in wire order.
    pub settings: &'static [(u16, u32)],
    /// Connection receive window after the preface's WINDOW_UPDATE (65 535 + increment).
    pub connection_window: u32,
    pub pseudo_order: [Pseudo; 4],
    /// HEADERS frame priority `(weight - 1, exclusive)` on stream 0, if sent.
    pub headers_priority: Option<(u8, bool)>,
}

impl H2Spec {
    /// The `SETTINGS_INITIAL_WINDOW_SIZE` sent (per-stream receive window).
    pub(crate) fn stream_window(&self) -> u32 {
        self.value(setting::INITIAL_WINDOW_SIZE).unwrap_or(65_535)
    }

    pub(crate) fn value(&self, id: u16) -> Option<u32> {
        self.settings
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, v)| *v)
    }

    /// Akamai-style fingerprint: `settings|window_update|priority_frames|pseudo_order`.
    pub(crate) fn akamai(&self) -> String {
        akamai(
            self.settings,
            self.connection_window.saturating_sub(65_535),
            &self.pseudo_order,
        )
    }
}

/// Render an Akamai HTTP/2 fingerprint (no PRIORITY frames are ever sent: `0`).
pub(crate) fn akamai(settings: &[(u16, u32)], window_update: u32, pseudo: &[Pseudo]) -> String {
    let settings = settings
        .iter()
        .map(|(id, v)| format!("{id}:{v}"))
        .collect::<Vec<_>>()
        .join(";");
    let pseudo = pseudo
        .iter()
        .map(|p| p.letter().to_string())
        .collect::<Vec<_>>()
        .join(",");
    let window = if window_update == 0 {
        "00".to_string()
    } else {
        window_update.to_string()
    };
    format!("{settings}|{window}|0|{pseudo}")
}

/// A browser profile: TLS + HTTP/2 + what it imitates.
#[derive(Debug)]
pub(crate) struct Profile {
    /// Which browser version(s) the data was captured from.
    pub version: &'static str,
    pub tls: &'static TlsSpec,
    pub h2: &'static H2Spec,
}

// ---------------------------------------------------------------------------------------
// Chrome (and Edge, which shares Chromium's network stack)
// ---------------------------------------------------------------------------------------

static CHROME_TLS: TlsSpec = TlsSpec {
    ciphers: &[
        0x1301, 0x1302, 0x1303, 0xc02b, 0xc02f, 0xc02c, 0xc030, 0xcca9, 0xcca8, 0xc013, 0xc014,
        0x009c, 0x009d, 0x002f, 0x0035,
    ],
    cipher_list: "TLS_AES_128_GCM_SHA256:TLS_AES_256_GCM_SHA384:TLS_CHACHA20_POLY1305_SHA256:\
                  TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256:TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256:\
                  TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384:TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384:\
                  TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256:\
                  TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256:\
                  TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA:TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA:\
                  TLS_RSA_WITH_AES_128_GCM_SHA256:TLS_RSA_WITH_AES_256_GCM_SHA384:\
                  TLS_RSA_WITH_AES_128_CBC_SHA:TLS_RSA_WITH_AES_256_CBC_SHA",
    groups: &[0x11ec, 0x001d, 0x0017, 0x0018],
    curves: "X25519MLKEM768:X25519:P-256:P-384",
    sigalgs: &[
        0x0403, 0x0804, 0x0401, 0x0503, 0x0805, 0x0501, 0x0806, 0x0601,
    ],
    sigalgs_list: "ecdsa_secp256r1_sha256:rsa_pss_rsae_sha256:rsa_pkcs1_sha256:\
                   ecdsa_secp384r1_sha384:rsa_pss_rsae_sha384:rsa_pkcs1_sha384:\
                   rsa_pss_rsae_sha512:rsa_pkcs1_sha512",
    extensions: &[
        SERVER_NAME,
        EXTENDED_MASTER_SECRET,
        RENEGOTIATION_INFO,
        SUPPORTED_GROUPS,
        EC_POINT_FORMATS,
        SESSION_TICKET,
        ALPN,
        STATUS_REQUEST,
        SIGNATURE_ALGORITHMS,
        SCT,
        KEY_SHARE,
        PSK_KEY_EXCHANGE_MODES,
        SUPPORTED_VERSIONS,
        COMPRESS_CERTIFICATE,
        ALPS_NEW,
        ECH,
    ],
    permute_extensions: true,
    grease: true,
    alps: true,
    cert_compression: &[CertCompression::Brotli],
    ech_grease: true,
    session_ticket: true,
    record_size_limit: None,
    delegated_credentials: None,
    key_shares_limit: None,
    ja4: "t13d1516h2_8daaf6152771_d8a2da3f94cd",
};

static CHROME_H2: H2Spec = H2Spec {
    settings: &[
        (setting::HEADER_TABLE_SIZE, 65_536),
        (setting::ENABLE_PUSH, 0),
        (setting::INITIAL_WINDOW_SIZE, 6_291_456),
        (setting::MAX_HEADER_LIST_SIZE, 262_144),
    ],
    connection_window: 15_728_640,
    pseudo_order: [
        Pseudo::Method,
        Pseudo::Authority,
        Pseudo::Scheme,
        Pseudo::Path,
    ],
    headers_priority: Some((255, true)),
};

static CHROME: Profile = Profile {
    version: "Chrome 133+ (desktop)",
    tls: &CHROME_TLS,
    h2: &CHROME_H2,
};

static EDGE: Profile = Profile {
    version: "Edge 133+ (desktop, Chromium)",
    tls: &CHROME_TLS,
    h2: &CHROME_H2,
};

// ---------------------------------------------------------------------------------------
// Firefox
// ---------------------------------------------------------------------------------------

static FIREFOX_TLS: TlsSpec = TlsSpec {
    ciphers: &[
        0x1301, 0x1303, 0x1302, 0xc02b, 0xc02f, 0xcca9, 0xcca8, 0xc02c, 0xc030, 0xc00a, 0xc009,
        0xc013, 0xc014, 0x009c, 0x009d, 0x002f, 0x0035,
    ],
    cipher_list: "TLS_AES_128_GCM_SHA256:TLS_CHACHA20_POLY1305_SHA256:TLS_AES_256_GCM_SHA384:\
                  TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256:TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256:\
                  TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256:\
                  TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256:\
                  TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384:TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384:\
                  TLS_ECDHE_ECDSA_WITH_AES_256_CBC_SHA:TLS_ECDHE_ECDSA_WITH_AES_128_CBC_SHA:\
                  TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA:TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA:\
                  TLS_RSA_WITH_AES_128_GCM_SHA256:TLS_RSA_WITH_AES_256_GCM_SHA384:\
                  TLS_RSA_WITH_AES_128_CBC_SHA:TLS_RSA_WITH_AES_256_CBC_SHA",
    groups: &[0x11ec, 0x001d, 0x0017, 0x0018, 0x0019, 0x0100, 0x0101],
    curves: "X25519MLKEM768:X25519:P-256:P-384:P-521:ffdhe2048:ffdhe3072",
    sigalgs: &[
        0x0403, 0x0503, 0x0603, 0x0804, 0x0805, 0x0806, 0x0401, 0x0501, 0x0601, 0x0203, 0x0201,
    ],
    sigalgs_list: "ecdsa_secp256r1_sha256:ecdsa_secp384r1_sha384:ecdsa_secp521r1_sha512:\
                   rsa_pss_rsae_sha256:rsa_pss_rsae_sha384:rsa_pss_rsae_sha512:\
                   rsa_pkcs1_sha256:rsa_pkcs1_sha384:rsa_pkcs1_sha512:ecdsa_sha1:rsa_pkcs1_sha1",
    extensions: &[
        SERVER_NAME,
        EXTENDED_MASTER_SECRET,
        RENEGOTIATION_INFO,
        SUPPORTED_GROUPS,
        EC_POINT_FORMATS,
        SESSION_TICKET,
        ALPN,
        STATUS_REQUEST,
        DELEGATED_CREDENTIALS,
        SCT,
        KEY_SHARE,
        SUPPORTED_VERSIONS,
        SIGNATURE_ALGORITHMS,
        PSK_KEY_EXCHANGE_MODES,
        RECORD_SIZE_LIMIT,
        COMPRESS_CERTIFICATE,
        ECH,
    ],
    permute_extensions: false,
    grease: false,
    alps: false,
    cert_compression: &[
        CertCompression::Zlib,
        CertCompression::Brotli,
        CertCompression::Zstd,
    ],
    ech_grease: true,
    session_ticket: true,
    record_size_limit: Some(0x4001),
    delegated_credentials: Some(
        "ecdsa_secp256r1_sha256:ecdsa_secp384r1_sha384:ecdsa_secp521r1_sha512:ecdsa_sha1",
    ),
    key_shares_limit: Some(3),
    ja4: "t13d1717h2_5b57614c22b0_3cbfd9057e0d",
};

static FIREFOX_H2: H2Spec = H2Spec {
    settings: &[
        (setting::HEADER_TABLE_SIZE, 65_536),
        (setting::ENABLE_PUSH, 0),
        (setting::INITIAL_WINDOW_SIZE, 131_072),
        (setting::MAX_FRAME_SIZE, 16_384),
    ],
    connection_window: 12_582_912,
    pseudo_order: [
        Pseudo::Method,
        Pseudo::Path,
        Pseudo::Authority,
        Pseudo::Scheme,
    ],
    headers_priority: Some((41, false)),
};

static FIREFOX: Profile = Profile {
    version: "Firefox 135+ (desktop)",
    tls: &FIREFOX_TLS,
    h2: &FIREFOX_H2,
};

// ---------------------------------------------------------------------------------------
// Safari
// ---------------------------------------------------------------------------------------

static SAFARI_TLS: TlsSpec = TlsSpec {
    ciphers: &[
        0x1301, 0x1302, 0x1303, 0xc02c, 0xc02b, 0xcca9, 0xc030, 0xc02f, 0xcca8, 0xc00a, 0xc009,
        0xc014, 0xc013, 0x009d, 0x009c, 0x0035, 0x002f, 0xc008, 0xc012, 0x000a,
    ],
    cipher_list: "TLS_AES_128_GCM_SHA256:TLS_AES_256_GCM_SHA384:TLS_CHACHA20_POLY1305_SHA256:\
                  TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384:TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256:\
                  TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256:\
                  TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384:TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256:\
                  TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256:\
                  TLS_ECDHE_ECDSA_WITH_AES_256_CBC_SHA:TLS_ECDHE_ECDSA_WITH_AES_128_CBC_SHA:\
                  TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA:TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA:\
                  TLS_RSA_WITH_AES_256_GCM_SHA384:TLS_RSA_WITH_AES_128_GCM_SHA256:\
                  TLS_RSA_WITH_AES_256_CBC_SHA:TLS_RSA_WITH_AES_128_CBC_SHA:\
                  TLS_ECDHE_ECDSA_WITH_3DES_EDE_CBC_SHA:TLS_ECDHE_RSA_WITH_3DES_EDE_CBC_SHA:\
                  TLS_RSA_WITH_3DES_EDE_CBC_SHA",
    groups: &[0x001d, 0x0017, 0x0018, 0x0019],
    curves: "X25519:P-256:P-384:P-521",
    sigalgs: &[
        0x0403, 0x0804, 0x0401, 0x0503, 0x0203, 0x0805, 0x0805, 0x0501, 0x0806, 0x0601, 0x0201,
    ],
    sigalgs_list: "ecdsa_secp256r1_sha256:rsa_pss_rsae_sha256:rsa_pkcs1_sha256:\
                   ecdsa_secp384r1_sha384:ecdsa_sha1:rsa_pss_rsae_sha384:rsa_pss_rsae_sha384:\
                   rsa_pkcs1_sha384:rsa_pss_rsae_sha512:rsa_pkcs1_sha512:rsa_pkcs1_sha1",
    extensions: &[
        SERVER_NAME,
        EXTENDED_MASTER_SECRET,
        RENEGOTIATION_INFO,
        SUPPORTED_GROUPS,
        EC_POINT_FORMATS,
        ALPN,
        STATUS_REQUEST,
        SIGNATURE_ALGORITHMS,
        SCT,
        KEY_SHARE,
        PSK_KEY_EXCHANGE_MODES,
        SUPPORTED_VERSIONS,
        COMPRESS_CERTIFICATE,
        PADDING,
    ],
    permute_extensions: false,
    grease: true,
    alps: false,
    cert_compression: &[CertCompression::Zlib],
    ech_grease: false,
    session_ticket: false,
    record_size_limit: None,
    delegated_credentials: None,
    key_shares_limit: None,
    ja4: "t13d2014h2_a09f3c656075_14788d8d241b",
};

static SAFARI_H2: H2Spec = H2Spec {
    settings: &[
        (setting::ENABLE_PUSH, 0),
        (setting::MAX_CONCURRENT_STREAMS, 100),
        (setting::INITIAL_WINDOW_SIZE, 2_097_152),
        (setting::NO_RFC7540_PRIORITIES, 1),
    ],
    connection_window: 10_485_760,
    pseudo_order: [
        Pseudo::Method,
        Pseudo::Scheme,
        Pseudo::Authority,
        Pseudo::Path,
    ],
    headers_priority: None,
};

static SAFARI: Profile = Profile {
    version: "Safari 18 (macOS)",
    tls: &SAFARI_TLS,
    h2: &SAFARI_H2,
};

/// The profile data of a browser.
pub(crate) fn profile(browser: Browser) -> &'static Profile {
    match browser {
        Browser::Chrome => &CHROME,
        Browser::Edge => &EDGE,
        Browser::Firefox => &FIREFOX,
        Browser::Safari => &SAFARI,
    }
}

/// Akev's own HTTP/2 preface (native profile): see `upstream::h2`.
pub(crate) fn native_h2_akamai(stream_window: u32, connection_window: u32) -> String {
    akamai(
        &[
            (setting::ENABLE_PUSH, 0),
            (setting::INITIAL_WINDOW_SIZE, stream_window),
        ],
        connection_window.saturating_sub(65_535),
        &[
            Pseudo::Method,
            Pseudo::Scheme,
            Pseudo::Authority,
            Pseudo::Path,
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client_hello::ClientHello;

    /// Build the ClientHello the data describes (GREASE-free) and fingerprint it.
    fn hello_of(spec: &TlsSpec) -> ClientHello {
        ClientHello {
            legacy_version: 0x0303,
            cipher_suites: spec.ciphers.to_vec(),
            extensions: spec.extensions.to_vec(),
            sni: Some("example.com".into()),
            alpn: vec!["h2".into(), "http/1.1".into()],
            supported_versions: vec![0x0304, 0x0303],
            groups: spec.groups.to_vec(),
            ec_point_formats: vec![0],
            signature_algorithms: spec.sigalgs.to_vec(),
        }
    }

    #[test]
    fn ja4_constants_match_the_data() {
        for browser in Browser::ALL {
            let p = profile(browser);
            assert_eq!(hello_of(p.tls).ja4(), p.tls.ja4, "{browser:?}");
        }
        // Published values (JA4+ database / tls.peet.ws) for the browsers imitated.
        assert_eq!(
            profile(Browser::Chrome).tls.ja4,
            "t13d1516h2_8daaf6152771_d8a2da3f94cd"
        );
        assert_eq!(
            profile(Browser::Firefox).tls.ja4,
            "t13d1717h2_5b57614c22b0_3cbfd9057e0d"
        );
        assert_eq!(
            profile(Browser::Safari).tls.ja4,
            "t13d2014h2_a09f3c656075_14788d8d241b"
        );
    }

    #[test]
    fn strings_and_id_lists_agree() {
        for browser in Browser::ALL {
            let t = profile(browser).tls;
            assert_eq!(
                t.cipher_list.split(':').count(),
                t.ciphers.len(),
                "{browser:?} ciphers"
            );
            assert_eq!(
                t.curves.split(':').count(),
                t.groups.len(),
                "{browser:?} groups"
            );
            assert_eq!(
                t.sigalgs_list.split(':').count(),
                t.sigalgs.len(),
                "{browser:?} sigalgs"
            );
            assert!(
                t.cipher_list
                    .split(':')
                    .all(|c| !c.trim().is_empty() && !c.contains(' '))
            );
            assert_eq!(t.alps, t.extensions.contains(&ALPS_NEW));
            assert_eq!(t.ech_grease, t.extensions.contains(&ECH));
            assert_eq!(t.session_ticket, t.extensions.contains(&SESSION_TICKET));
            assert_eq!(
                t.record_size_limit.is_some(),
                t.extensions.contains(&RECORD_SIZE_LIMIT)
            );
            assert_eq!(
                t.delegated_credentials.is_some(),
                t.extensions.contains(&DELEGATED_CREDENTIALS)
            );
        }
    }

    #[test]
    fn akamai_fingerprints() {
        assert_eq!(
            profile(Browser::Chrome).h2.akamai(),
            "1:65536;2:0;4:6291456;6:262144|15663105|0|m,a,s,p"
        );
        assert_eq!(
            profile(Browser::Edge).h2.akamai(),
            profile(Browser::Chrome).h2.akamai()
        );
        assert_eq!(
            profile(Browser::Firefox).h2.akamai(),
            "1:65536;2:0;4:131072;5:16384|12517377|0|m,p,a,s"
        );
        assert_eq!(
            profile(Browser::Safari).h2.akamai(),
            "2:0;3:100;4:2097152;9:1|10420225|0|m,s,a,p"
        );
        assert_eq!(profile(Browser::Chrome).h2.stream_window(), 6_291_456);
        assert_eq!(
            native_h2_akamai(2 << 20, 1 << 30),
            "2:0;4:2097152|1073676289|0|m,s,a,p"
        );
        assert_eq!(akamai(&[], 0, &[]), "|00|0|");
    }
}
