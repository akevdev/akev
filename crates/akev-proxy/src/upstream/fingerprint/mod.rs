//! Upstream TLS fingerprint impersonation.
//!
//! Anti-bot services (Cloudflare, Datadome, Akamai...) compare the TLS ClientHello (JA3/JA4)
//! and the HTTP/2 connection preface of a connection with the browser its `User-Agent`
//! claims. Through a debugging proxy the server sees the *proxy's* TLS stack instead, so
//! intercepted browser traffic gets challenged or blocked. With a browser profile Akev's
//! upstream connections reproduce that browser's ClientHello (cipher suites and their order,
//! extensions and their order or Chrome's per-connection permutation, GREASE, supported
//! groups incl. X25519MLKEM768, signature algorithms, ALPS, certificate compression, ECH
//! GREASE, record size limit, delegated credentials, key share count) and HTTP/2 preface
//! (SETTINGS values and order, connection WINDOW_UPDATE, HEADERS priority, pseudo-header
//! order).
//!
//! * Profile choice per request: a `tls-profile` rule ([`FingerprintOptions::rule`]) wins over
//!   `proxy.tlsProfile.overrides` (first matching host glob), which win over
//!   `proxy.tlsProfile.profile`. `mirror-client` resolves to the intercepted client's own
//!   browser family ([`detect_client`]: its JA4, refined/backed by its `User-Agent`), or to
//!   native for clients that aren't a recognised browser.
//! * Browser profiles need the BoringSSL backend (cargo feature `impersonate`, off by
//!   default); without it they fall back to native rustls and the exchange's
//!   [`UpstreamFingerprint`] says so.
//! * Every upstream TLS connection (native too) records the ClientHello it actually sent
//!   ([`HelloRecorder`]) and reports its JA3/JA4 in [`UpstreamInfo::fingerprint`].
//! * TLS to HTTPS *proxies* always uses rustls: only the origin sees the fingerprint.
//!
//! [`UpstreamInfo::fingerprint`]: akev_core::UpstreamInfo::fingerprint

use super::glob::HostPattern;
use crate::client_hello::{self, ClientHello, HelloParse};
use akev_core::UpstreamFingerprint;
pub use akev_core::settings::TlsProfile;
use akev_core::settings::TlsProfileSettings;
use serde::Serialize;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

#[cfg(feature = "impersonate")]
pub(crate) mod boring;
pub(crate) mod profiles;

/// Whether this build contains the impersonation backend (cargo feature `impersonate`).
pub const AVAILABLE: bool = cfg!(feature = "impersonate");

/// Whether browser profiles work in this build.
pub fn available() -> bool {
    AVAILABLE
}

/// The impersonation backend compiled in, if any.
pub fn backend() -> Option<&'static str> {
    AVAILABLE.then_some("BoringSSL")
}

/// A browser whose fingerprint can be presented.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Browser {
    Chrome,
    Firefox,
    Safari,
    Edge,
}

impl Browser {
    /// Every browser profile, in display order.
    pub const ALL: [Browser; 4] = [
        Browser::Chrome,
        Browser::Firefox,
        Browser::Safari,
        Browser::Edge,
    ];

    /// Wire name (same as the [`TlsProfile`] name).
    pub fn as_str(self) -> &'static str {
        self.profile().as_str()
    }

    /// Display name.
    pub fn label(self) -> &'static str {
        match self {
            Browser::Chrome => "Chrome",
            Browser::Firefox => "Firefox",
            Browser::Safari => "Safari",
            Browser::Edge => "Edge",
        }
    }

    /// The settings profile that selects this browser.
    pub fn profile(self) -> TlsProfile {
        match self {
            Browser::Chrome => TlsProfile::Chrome,
            Browser::Firefox => TlsProfile::Firefox,
            Browser::Safari => TlsProfile::Safari,
            Browser::Edge => TlsProfile::Edge,
        }
    }

    /// The browser a fixed profile selects (`None` for `native` and `mirror-client`).
    pub fn from_profile(profile: TlsProfile) -> Option<Browser> {
        match profile {
            TlsProfile::Chrome => Some(Browser::Chrome),
            TlsProfile::Firefox => Some(Browser::Firefox),
            TlsProfile::Safari => Some(Browser::Safari),
            TlsProfile::Edge => Some(Browser::Edge),
            TlsProfile::Native | TlsProfile::MirrorClient => None,
        }
    }

    /// Expected JA4 of this browser's ClientHello when it offers `h2`.
    pub fn ja4(self) -> &'static str {
        profiles::profile(self).tls.ja4
    }

    /// Akamai-style fingerprint of this browser's HTTP/2 connection preface.
    pub fn http2_fingerprint(self) -> String {
        profiles::profile(self).h2.akamai()
    }

    /// Which browser version(s) the profile reproduces.
    pub fn imitates(self) -> &'static str {
        profiles::profile(self).version
    }

    /// What this browser's ClientHello contains (GREASE values excluded).
    pub fn expected_hello(self) -> ExpectedHello {
        let t = profiles::profile(self).tls;
        ExpectedHello {
            ciphers: t.ciphers,
            extensions: t.extensions,
            extension_order_randomized: t.permute_extensions,
            groups: t.groups,
            signature_algorithms: t.sigalgs,
            grease: t.grease,
            ja4: t.ja4,
        }
    }
}

/// The ClientHello a browser profile sends (see [`Browser::expected_hello`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExpectedHello {
    /// Cipher suites in wire order.
    pub ciphers: &'static [u16],
    /// Extensions: in wire order, or the set when `extension_order_randomized`.
    pub extensions: &'static [u16],
    /// The browser shuffles its extensions per connection (Chrome since v110).
    pub extension_order_randomized: bool,
    /// `supported_groups` in wire order.
    pub groups: &'static [u16],
    /// `signature_algorithms` in wire order.
    pub signature_algorithms: &'static [u16],
    /// GREASE values are inserted (RFC 8701).
    pub grease: bool,
    /// JA4 when `h2` is offered.
    pub ja4: &'static str,
}

/// One profile of the catalogue (`GET /api/tls-profiles`).
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileInfo {
    /// [`TlsProfile`] wire name.
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    /// Browser version(s) reproduced (browser profiles only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imitates: Option<&'static str>,
    /// Expected JA4 (with `h2` offered; browser profiles only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ja4: Option<&'static str>,
    /// Akamai HTTP/2 fingerprint (browser profiles only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http2: Option<String>,
    /// Whether the profile works in this build (browser profiles need the impersonation
    /// backend; `mirror-client` too, to mirror anything).
    pub available: bool,
}

/// Every profile with its expected fingerprints and availability in this build.
pub fn catalog() -> Vec<ProfileInfo> {
    TlsProfile::ALL
        .into_iter()
        .map(|profile| {
            let browser = Browser::from_profile(profile);
            let (label, description) = match profile {
                TlsProfile::Native => (
                    "Native",
                    "Akev's own TLS stack (rustls). Always available.",
                ),
                TlsProfile::Chrome => (
                    "Chrome",
                    "Google Chrome's ClientHello (randomised extension order, GREASE, X25519MLKEM768, ALPS, Brotli certificate compression, ECH GREASE) and HTTP/2 preface.",
                ),
                TlsProfile::Firefox => (
                    "Firefox",
                    "Mozilla Firefox's ClientHello (NSS extension order, X25519MLKEM768 + 3 key shares, delegated credentials, record size limit, ECH GREASE) and HTTP/2 preface.",
                ),
                TlsProfile::Safari => (
                    "Safari",
                    "Safari's ClientHello (GREASE, legacy 3DES suites, zlib certificate compression, padding) and HTTP/2 preface.",
                ),
                TlsProfile::Edge => (
                    "Edge",
                    "Microsoft Edge: the same Chromium network stack, so the same fingerprint as Chrome.",
                ),
                TlsProfile::MirrorClient => (
                    "Mirror client",
                    "The intercepted client's own browser family (detected from its JA4 and User-Agent); native for clients that aren't a recognised browser.",
                ),
            };
            ProfileInfo {
                id: profile.as_str(),
                label,
                description,
                imitates: browser.map(Browser::imitates),
                ja4: browser.map(Browser::ja4),
                http2: browser.map(Browser::http2_fingerprint),
                available: profile == TlsProfile::Native || AVAILABLE,
            }
        })
        .collect()
}

/// The browser family of an intercepted client, for `mirror-client`: from its ClientHello's
/// JA4 (cipher-suite hash), refined by its `User-Agent` (Edge shares Chrome's TLS stack), or
/// from the `User-Agent` alone when the JA4 is unknown or there is none (plain HTTP clients).
/// `None` for clients that aren't a recognised browser (curl, SDKs...).
pub fn detect_client(ja4: Option<&str>, user_agent: Option<&str>) -> Option<Browser> {
    let ua = user_agent.and_then(browser_from_user_agent);
    match (ja4.and_then(browser_from_ja4), ua) {
        (Some(Browser::Chrome), Some(Browser::Edge)) => Some(Browser::Edge),
        (Some(tls), _) => Some(tls),
        (None, ua) => ua,
    }
}

/// Browser family from the JA4 cipher-suite hash (`JA4_b`), which is stable across versions
/// of a browser engine.
fn browser_from_ja4(ja4: &str) -> Option<Browser> {
    match ja4.split('_').nth(1)? {
        // Chromium (Chrome, Edge, Opera, Brave, Vivaldi...).
        "8daaf6152771" => Some(Browser::Chrome),
        // Firefox (NSS).
        "5b57614c22b0" => Some(Browser::Firefox),
        // Safari / every iOS browser (Apple's TLS stack).
        "a09f3c656075" => Some(Browser::Safari),
        _ => None,
    }
}

/// Browser family from a `User-Agent`.
fn browser_from_user_agent(ua: &str) -> Option<Browser> {
    let has = |needle: &str| ua.contains(needle);
    // Every iOS browser uses Apple's network stack.
    if has("iPhone") || has("iPad") || has("iPod") {
        return (has("AppleWebKit/") || has("Safari/")).then_some(Browser::Safari);
    }
    if has("Edg/") || has("EdgA/") {
        Some(Browser::Edge)
    } else if has("Firefox/") {
        Some(Browser::Firefox)
    } else if has("Chrome/") || has("Chromium/") {
        Some(Browser::Chrome)
    } else if has("Safari/") && has("Version/") {
        Some(Browser::Safari)
    } else {
        None
    }
}

/// Per-request inputs for the profile choice.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FingerprintOptions {
    /// Profile from a matching `tls-profile` rule (wins over settings).
    pub rule: Option<TlsProfile>,
    /// The intercepted client's browser family (see [`detect_client`]), for `mirror-client`.
    pub client: Option<Browser>,
}

/// Why a requested profile wasn't presented as is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Fallback {
    /// A browser profile in a build without the impersonation backend.
    NotCompiled,
    /// `mirror-client` for a client that isn't a recognised browser.
    NotABrowser,
}

/// The fingerprint chosen for one request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Selection {
    /// Browser to impersonate; `None` = native rustls.
    pub browser: Option<Browser>,
    /// The profile asked for (rule, override or default).
    pub requested: TlsProfile,
    pub fallback: Option<Fallback>,
}

impl Selection {
    /// Native rustls, nothing else asked for.
    pub(crate) const NATIVE: Selection = Selection {
        browser: None,
        requested: TlsProfile::Native,
        fallback: None,
    };

    /// The exchange's [`UpstreamFingerprint`], given the ClientHello that was sent.
    pub(crate) fn describe(&self, hello: Option<&ClientHello>) -> UpstreamFingerprint {
        let profile = self.browser.map(Browser::as_str).unwrap_or("native");
        UpstreamFingerprint {
            profile: profile.to_owned(),
            requested: (self.requested.as_str() != profile)
                .then(|| self.requested.as_str().to_owned()),
            note: self.fallback.map(|f| {
                match f {
                    Fallback::NotCompiled => {
                        "browser profiles need an impersonation build of Akev (cargo feature `impersonate`); the native TLS stack was used"
                    }
                    Fallback::NotABrowser => {
                        "the client isn't a recognised browser; the native TLS stack was used"
                    }
                }
                .to_owned()
            }),
            ja3: hello.map(ClientHello::ja3),
            ja3_text: hello.map(ClientHello::ja3_string),
            ja4: hello.map(ClientHello::ja4),
            http2: None,
        }
    }
}

/// Compiled `proxy.tlsProfile` settings.
#[derive(Debug, Default)]
pub(crate) struct ProfileSelector {
    default: TlsProfile,
    overrides: Vec<(HostPattern, TlsProfile)>,
}

impl ProfileSelector {
    pub(crate) fn new(settings: &TlsProfileSettings) -> Self {
        ProfileSelector {
            default: settings.profile,
            overrides: settings
                .overrides
                .iter()
                .filter_map(|o| Some((HostPattern::parse(&o.host)?, o.profile)))
                .collect(),
        }
    }

    /// The fingerprint for a request to `host:port`.
    pub(crate) fn select(&self, host: &str, port: u16, opts: &FingerprintOptions) -> Selection {
        let requested = opts
            .rule
            .or_else(|| {
                self.overrides
                    .iter()
                    .find(|(p, _)| p.matches(host, port))
                    .map(|(_, profile)| *profile)
            })
            .unwrap_or(self.default);
        let wanted = match requested {
            TlsProfile::MirrorClient => match opts.client {
                Some(b) => Some(b),
                None => {
                    return Selection {
                        browser: None,
                        requested,
                        fallback: Some(Fallback::NotABrowser),
                    };
                }
            },
            other => Browser::from_profile(other),
        };
        match wanted {
            Some(browser) if AVAILABLE => Selection {
                browser: Some(browser),
                requested,
                fallback: None,
            },
            Some(_) => Selection {
                browser: None,
                requested,
                fallback: Some(Fallback::NotCompiled),
            },
            None => Selection {
                browser: None,
                requested,
                fallback: None,
            },
        }
    }
}

/// Largest ClientHello recorded (post-quantum key shares make them ~1.8 KB).
const MAX_RECORDED: usize = 16 * 1024;

/// Wraps the transport under a TLS client and records the bytes of the ClientHello written
/// through it, so the fingerprint actually sent can be reported. Once the hello is complete
/// (or clearly isn't one) it is a plain pass-through.
pub(crate) struct HelloRecorder<T> {
    inner: T,
    recording: Option<Vec<u8>>,
    hello: Option<ClientHello>,
}

impl<T> HelloRecorder<T> {
    pub(crate) fn new(inner: T) -> Self {
        HelloRecorder {
            inner,
            recording: Some(Vec::with_capacity(2048)),
            hello: None,
        }
    }

    /// The ClientHello written so far (complete once the TLS client sent it).
    pub(crate) fn hello(&self) -> Option<&ClientHello> {
        self.hello.as_ref()
    }

    fn record(&mut self, written: &[u8]) {
        let Some(buf) = self.recording.as_mut() else {
            return;
        };
        buf.extend_from_slice(written);
        match client_hello::parse(buf) {
            HelloParse::Complete(hello) => {
                self.hello = Some(hello);
                self.recording = None;
            }
            HelloParse::Invalid(_) => self.recording = None,
            HelloParse::NeedMore if buf.len() > MAX_RECORDED => self.recording = None,
            HelloParse::NeedMore => {}
        }
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for HelloRecorder<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for HelloRecorder<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = &mut *self;
        let poll = Pin::new(&mut this.inner).poll_write(cx, buf);
        if let (Poll::Ready(Ok(n)), true) = (&poll, this.recording.is_some()) {
            this.record(&buf[..*n]);
        }
        poll
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let this = &mut *self;
        let poll = Pin::new(&mut this.inner).poll_write_vectored(cx, bufs);
        if let (Poll::Ready(Ok(n)), true) = (&poll, this.recording.is_some()) {
            let mut left = *n;
            let mut written = Vec::with_capacity(left);
            for b in bufs {
                if left == 0 {
                    break;
                }
                let take = left.min(b.len());
                written.extend_from_slice(&b[..take]);
                left -= take;
            }
            this.record(&written);
        }
        poll
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use akev_core::settings::TlsProfileOverride;
    use tokio::io::AsyncWriteExt;

    const CHROME_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";
    const EDGE_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36 Edg/141.0.0.0";
    const FIREFOX_UA: &str =
        "Mozilla/5.0 (X11; Linux x86_64; rv:144.0) Gecko/20100101 Firefox/144.0";
    const SAFARI_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.6 Safari/605.1.15";
    const CRIOS_UA: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_6 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) CriOS/141.0.0.0 Mobile/15E148 Safari/604.1";

    #[test]
    fn detects_browser_families() {
        assert_eq!(detect_client(None, Some(CHROME_UA)), Some(Browser::Chrome));
        assert_eq!(detect_client(None, Some(EDGE_UA)), Some(Browser::Edge));
        assert_eq!(
            detect_client(None, Some(FIREFOX_UA)),
            Some(Browser::Firefox)
        );
        assert_eq!(detect_client(None, Some(SAFARI_UA)), Some(Browser::Safari));
        assert_eq!(detect_client(None, Some(CRIOS_UA)), Some(Browser::Safari));
        assert_eq!(detect_client(None, Some("curl/8.9.1")), None);
        assert_eq!(detect_client(None, Some("python-requests/2.32.3")), None);
        assert_eq!(detect_client(None, None), None);

        // The TLS fingerprint describes the actual client; the UA only refines Chromium.
        let chrome_ja4 = "t13d1516h2_8daaf6152771_d8a2da3f94cd";
        let firefox_ja4 = "t13d1717h2_5b57614c22b0_3cbfd9057e0d";
        let safari_ja4 = "t13d2014h2_a09f3c656075_14788d8d241b";
        assert_eq!(detect_client(Some(chrome_ja4), None), Some(Browser::Chrome));
        assert_eq!(
            detect_client(Some(chrome_ja4), Some(EDGE_UA)),
            Some(Browser::Edge)
        );
        assert_eq!(
            detect_client(Some(firefox_ja4), Some(CHROME_UA)),
            Some(Browser::Firefox)
        );
        assert_eq!(detect_client(Some(safari_ja4), None), Some(Browser::Safari));
        // Unknown TLS stack (e.g. a script spoofing a browser UA): the UA decides.
        let python_ja4 = "t13d1812h1_85036bcba153_b26ce05bbdd6";
        assert_eq!(
            detect_client(Some(python_ja4), Some(CHROME_UA)),
            Some(Browser::Chrome)
        );
        assert_eq!(detect_client(Some(python_ja4), Some("curl/8")), None);
        assert_eq!(detect_client(Some("garbage"), None), None);
    }

    fn selector(profile: TlsProfile, overrides: &[(&str, TlsProfile)]) -> ProfileSelector {
        ProfileSelector::new(&TlsProfileSettings {
            profile,
            overrides: overrides
                .iter()
                .map(|(host, profile)| TlsProfileOverride {
                    host: host.to_string(),
                    profile: *profile,
                })
                .collect(),
        })
    }

    #[test]
    fn selection_precedence() {
        let s = selector(
            TlsProfile::Native,
            &[
                ("*.shop.test", TlsProfile::Firefox),
                ("api.test:8443", TlsProfile::Safari),
                ("", TlsProfile::Chrome),
            ],
        );
        let none = FingerprintOptions::default();
        assert_eq!(s.select("other.test", 443, &none), Selection::NATIVE);
        let sel = s.select("www.shop.test", 443, &none);
        assert_eq!(sel.requested, TlsProfile::Firefox);
        let sel = s.select("api.test", 443, &none);
        assert_eq!(sel.requested, TlsProfile::Native, "port must match");
        let sel = s.select("api.test", 8443, &none);
        assert_eq!(sel.requested, TlsProfile::Safari);
        // Rules win over overrides.
        let rule = FingerprintOptions {
            rule: Some(TlsProfile::Edge),
            client: None,
        };
        assert_eq!(
            s.select("www.shop.test", 443, &rule).requested,
            TlsProfile::Edge
        );
        let rule_native = FingerprintOptions {
            rule: Some(TlsProfile::Native),
            client: None,
        };
        assert_eq!(
            s.select("www.shop.test", 443, &rule_native),
            Selection::NATIVE
        );

        let browser = |sel: Selection| sel.browser;
        let expect = |b: Browser| if AVAILABLE { Some(b) } else { None };
        assert_eq!(
            browser(s.select("www.shop.test", 443, &none)),
            expect(Browser::Firefox)
        );
        let sel = s.select("www.shop.test", 443, &none);
        assert_eq!(sel.fallback, (!AVAILABLE).then_some(Fallback::NotCompiled));
    }

    #[test]
    fn mirror_client() {
        let s = selector(TlsProfile::MirrorClient, &[]);
        let sel = s.select("a.test", 443, &FingerprintOptions::default());
        assert_eq!(sel.browser, None);
        assert_eq!(sel.fallback, Some(Fallback::NotABrowser));
        let fp = sel.describe(None);
        assert_eq!(fp.profile, "native");
        assert_eq!(fp.requested.as_deref(), Some("mirror-client"));
        assert!(fp.note.unwrap().contains("isn't a recognised browser"));

        let sel = s.select(
            "a.test",
            443,
            &FingerprintOptions {
                rule: None,
                client: Some(Browser::Safari),
            },
        );
        assert_eq!(sel.requested, TlsProfile::MirrorClient);
        let fp = sel.describe(None);
        if AVAILABLE {
            assert_eq!(sel.browser, Some(Browser::Safari));
            assert_eq!(fp.profile, "safari");
            assert_eq!(fp.note, None);
        } else {
            assert_eq!(sel.browser, None);
            assert_eq!(fp.profile, "native");
            assert!(fp.note.unwrap().contains("impersonation build"));
        }
        assert_eq!(fp.requested.as_deref(), Some("mirror-client"));
    }

    #[test]
    fn describes_native_without_noise() {
        let fp = Selection::NATIVE.describe(None);
        assert_eq!(fp.profile, "native");
        assert_eq!(fp.requested, None);
        assert_eq!(fp.note, None);
        assert_eq!(fp.ja4, None);
    }

    #[test]
    fn catalog_lists_every_profile() {
        let c = catalog();
        let ids: Vec<&str> = c.iter().map(|p| p.id).collect();
        assert_eq!(
            ids,
            [
                "native",
                "chrome",
                "firefox",
                "safari",
                "edge",
                "mirror-client"
            ]
        );
        assert!(c[0].available);
        assert!(c.iter().skip(1).all(|p| p.available == AVAILABLE));
        let chrome = &c[1];
        assert_eq!(chrome.ja4, Some("t13d1516h2_8daaf6152771_d8a2da3f94cd"));
        assert!(chrome.http2.as_deref().unwrap().ends_with("|m,a,s,p"));
        assert_eq!(c[0].ja4, None);
        let json = serde_json::to_value(chrome).unwrap();
        assert_eq!(json["id"], "chrome");
        assert!(json["imitates"].as_str().unwrap().starts_with("Chrome"));
        assert_eq!(available(), AVAILABLE);
        assert_eq!(backend().is_some(), AVAILABLE);
    }

    #[tokio::test]
    async fn recorder_captures_the_hello_written_in_pieces() {
        let raw = client_hello::tests::chrome_like_hello("rec.test");
        let (a, mut b) = tokio::io::duplex(64 * 1024);
        let mut rec = HelloRecorder::new(a);
        for chunk in raw.chunks(7) {
            rec.write_all(chunk).await.unwrap();
        }
        rec.write_all(b"application data").await.unwrap();
        let hello = rec.hello().expect("hello recorded");
        assert_eq!(hello.sni.as_deref(), Some("rec.test"));
        assert_eq!(hello.ja4(), "t13d1516h2_8daaf6152771_e5627efa2ab1");
        assert!(rec.recording.is_none(), "stops recording after the hello");
        let mut seen = vec![0u8; raw.len() + 16];
        tokio::io::AsyncReadExt::read_exact(&mut b, &mut seen)
            .await
            .unwrap();
        assert_eq!(&seen[..raw.len()], &raw[..], "bytes pass through untouched");
    }

    #[tokio::test]
    async fn recorder_gives_up_on_non_tls() {
        let (a, _b) = tokio::io::duplex(1024);
        let mut rec = HelloRecorder::new(a);
        rec.write_all(b"GET / HTTP/1.1\r\n\r\n").await.unwrap();
        assert!(rec.hello().is_none());
        assert!(rec.recording.is_none());
    }
}
