//! Upstream TLS (rustls client).
//!
//! * Trust: webpki-roots + the OS store (rustls-native-certs, loaded once per process in the
//!   background) + `settings.trusted_ca_paths` (PEM files).
//! * Verification: a custom [`ServerCertVerifier`] that skips verification for hosts matching
//!   `settings.ignore_upstream_cert_errors` (or [`RequestOptions::insecure`]), otherwise
//!   delegates to [`WebPkiServerVerifier`] and turns failures into precise, human messages with
//!   stable codes (`CERT_HAS_EXPIRED`, `SELF_SIGNED`, `HOSTNAME_MISMATCH`, `UNKNOWN_ISSUER`, ...).
//!   Signatures on the handshake itself are always verified, even in insecure mode.
//! * The leaf certificate is captured for every successful handshake (`peer_certificates`)
//!   and summarised into [`UpstreamInfo::certificate`]; the whole presented chain, plus the
//!   path rustls-webpki verified and the root it ended at, into [`UpstreamInfo::chain`]
//!   ([`TlsContext::describe_chain`]).
//! * Client certificates: `settings.client_certificates` matched by host glob; PEM (chain +
//!   key) and PKCS#12 (`.p12`/`.pfx`, passphrase) files.
//! * Strict and insecure connections never share a session cache (a session resumed without
//!   verification must not satisfy a strict request).
//!
//! [`RequestOptions::insecure`]: super::RequestOptions::insecure
//! [`UpstreamInfo::certificate`]: akev_core::UpstreamInfo::certificate
//! [`UpstreamInfo::chain`]: akev_core::UpstreamInfo::chain

use super::UpstreamError;
use super::cert;
use super::error::{error, from_io};
use super::glob::{HostPattern, parse_patterns};
use akev_core::settings::ProxySettings;
use akev_core::{
    CertificateChain, ChainTrust, ChainTrustStatus, ErrorKind, TrustAnchorInfo, TrustAnchorSource,
    UpstreamInfo,
};
use parking_lot::Mutex;
use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{
    CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, TrustAnchor, UnixTime,
};
use rustls::{
    CertificateError, ClientConfig, DigitallySignedStruct, OtherError, RootCertStore,
    SignatureScheme,
};
use std::collections::HashMap;
use std::io;
use std::net::IpAddr;
use std::sync::{Arc, OnceLock};

/// Which ALPN protocols to offer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Alpn {
    /// `h2`, `http/1.1`.
    H2AndHttp1,
    /// `http/1.1` only (upgrades, `http1_only`, TLS to HTTPS proxies).
    Http1,
}

/// The process-wide rustls crypto provider (ring).
pub(crate) fn provider() -> Arc<CryptoProvider> {
    static PROVIDER: OnceLock<Arc<CryptoProvider>> = OnceLock::new();
    PROVIDER
        .get_or_init(|| Arc::new(rustls::crypto::ring::default_provider()))
        .clone()
}

/// Makes ring rustls' process-wide default provider as well. Akev hands its provider to
/// rustls explicitly, but reqwest (built with `rustls-no-provider`, so that aws-lc-rs isn't
/// compiled in next to ring) takes the default and panics when there is none. Call before
/// building a reqwest client; repeated calls, or a provider installed earlier, are fine.
pub fn install_default_crypto_provider() {
    if CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
}

/// webpki-roots + OS trust store, loaded once per process.
fn base_roots() -> &'static RootCertStore {
    static ROOTS: OnceLock<RootCertStore> = OnceLock::new();
    ROOTS.get_or_init(|| {
        let mut store = RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
        let native = rustls_native_certs::load_native_certs();
        if !native.errors.is_empty() {
            tracing::debug!(errors = ?native.errors, "some OS trust store certificates could not be loaded");
        }
        let (added, ignored) = store.add_parsable_certificates(native.certs);
        tracing::debug!(added, ignored, "loaded OS trust store");
        store
    })
}

/// Load the OS trust store on a background thread so the first TLS connection doesn't pay
/// for it.
pub(crate) fn warm_up() {
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| {
        let _ = std::thread::Builder::new()
            .name("akev-tls-roots".into())
            .spawn(|| {
                let _ = base_roots();
            });
    });
}

/// Client config for the DNS-over-HTTPS resolver (standard verification, no ALPN — hickory
/// inserts its own).
pub(crate) fn doh_client_config() -> Result<ClientConfig, rustls::Error> {
    Ok(ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()?
        .with_root_certificates(base_roots().clone())
        .with_no_client_auth())
}

/// A client identity (certificate chain + private key).
struct Identity {
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
}

struct ClientCertEntry {
    pattern: HostPattern,
    path: String,
    identity: Result<Identity, String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct ConfigKey {
    insecure: bool,
    alpn: Alpn,
    client_cert: Option<usize>,
}

/// (strict, insecure) verifiers sharing one root store, and that store (for
/// [`TlsContext::describe_chain`]).
type VerifierPair = (Arc<AkevVerifier>, Arc<AkevVerifier>, Arc<RootCertStore>);

/// Compiled TLS settings (one per TLS-settings generation).
pub(crate) struct TlsContext {
    ignore: Vec<HostPattern>,
    extra_roots: Vec<CertificateDer<'static>>,
    client_certs: Vec<ClientCertEntry>,
    verifiers: OnceLock<Result<VerifierPair, String>>,
    configs: Mutex<HashMap<ConfigKey, Arc<ClientConfig>>>,
}

impl std::fmt::Debug for TlsContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TlsContext")
            .field("ignore", &self.ignore)
            .field("extra_roots", &self.extra_roots.len())
            .field(
                "client_certs",
                &self
                    .client_certs
                    .iter()
                    .map(|c| &c.path)
                    .collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

impl TlsContext {
    /// Compile from settings. Unreadable files are logged; a client certificate that fails to
    /// load makes requests to its hosts fail with a descriptive error rather than silently
    /// connecting without it.
    pub(crate) fn new(settings: &ProxySettings) -> Self {
        let mut extra_roots = Vec::new();
        for path in &settings.trusted_ca_paths {
            match load_pem_certs(path) {
                Ok(certs) if certs.is_empty() => {
                    tracing::warn!(path, "no certificates found in trusted CA file")
                }
                Ok(certs) => extra_roots.extend(certs),
                Err(e) => tracing::warn!(path, error = %e, "cannot load trusted CA file"),
            }
        }
        let client_certs = settings
            .client_certificates
            .iter()
            .filter_map(|c| {
                let pattern = HostPattern::parse(&c.host)?;
                let identity = load_identity(&c.path, c.passphrase.as_deref());
                if let Err(e) = &identity {
                    tracing::warn!(path = %c.path, error = %e, "cannot load client certificate");
                }
                Some(ClientCertEntry {
                    pattern,
                    path: c.path.clone(),
                    identity,
                })
            })
            .collect();
        TlsContext {
            ignore: parse_patterns(&settings.ignore_upstream_cert_errors),
            extra_roots,
            client_certs,
            verifiers: OnceLock::new(),
            configs: Mutex::new(HashMap::new()),
        }
    }

    /// Whether certificate errors are ignored for this host (settings globs).
    pub(crate) fn ignores_errors_for(&self, host: &str, port: u16) -> bool {
        self.ignore.iter().any(|p| p.matches(host, port))
    }

    /// Index of the client certificate to present to this host, if any.
    pub(crate) fn client_cert_for(&self, host: &str, port: u16) -> Option<usize> {
        self.client_certs
            .iter()
            .position(|c| c.pattern.matches(host, port))
    }

    fn verifiers(&self) -> Result<&VerifierPair, UpstreamError> {
        self.verifiers
            .get_or_init(|| {
                let mut roots = base_roots().clone();
                let (_, ignored) =
                    roots.add_parsable_certificates(self.extra_roots.iter().cloned());
                if ignored > 0 {
                    tracing::warn!(ignored, "some trusted CA certificates could not be parsed");
                }
                let provider = provider();
                let algorithms = provider.signature_verification_algorithms;
                let roots = Arc::new(roots);
                let inner = WebPkiServerVerifier::builder_with_provider(roots.clone(), provider)
                    .build()
                    .map_err(|e| format!("cannot build certificate verifier: {e}"))?;
                Ok((
                    Arc::new(AkevVerifier {
                        inner: inner.clone(),
                        insecure: false,
                        algorithms,
                    }),
                    Arc::new(AkevVerifier {
                        inner,
                        insecure: true,
                        algorithms,
                    }),
                    roots,
                ))
            })
            .as_ref()
            .map_err(|e| error(ErrorKind::Internal, e.clone(), "TLS_CONFIG"))
    }

    /// A (cached) client config for the given verification mode, ALPN and client identity.
    pub(crate) fn client_config(
        &self,
        insecure: bool,
        alpn: Alpn,
        client_cert: Option<usize>,
    ) -> Result<Arc<ClientConfig>, UpstreamError> {
        let key = ConfigKey {
            insecure,
            alpn,
            client_cert,
        };
        if let Some(cfg) = self.configs.lock().get(&key) {
            return Ok(cfg.clone());
        }
        let (strict, lax, _) = self.verifiers()?;
        let verifier: Arc<dyn ServerCertVerifier> = if insecure {
            lax.clone()
        } else {
            strict.clone()
        };
        let builder = ClientConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .map_err(|e| {
                error(
                    ErrorKind::Internal,
                    format!("TLS configuration error: {e}"),
                    "TLS_CONFIG",
                )
            })?
            .dangerous()
            .with_custom_certificate_verifier(verifier);
        let mut cfg = match client_cert.and_then(|i| self.client_certs.get(i)) {
            Some(entry) => {
                let identity = entry.identity.as_ref().map_err(|e| {
                    error(
                        ErrorKind::UpstreamTlsFailed,
                        format!("client certificate {}: {e}", entry.path),
                        "CLIENT_CERT_INVALID",
                    )
                })?;
                builder
                    .with_client_auth_cert(identity.chain.clone(), identity.key.clone_key())
                    .map_err(|e| {
                        error(
                            ErrorKind::UpstreamTlsFailed,
                            format!("client certificate {} is unusable: {e}", entry.path),
                            "CLIENT_CERT_INVALID",
                        )
                    })?
            }
            None => builder.with_no_client_auth(),
        };
        cfg.alpn_protocols = match alpn {
            Alpn::H2AndHttp1 => vec![b"h2".to_vec(), b"http/1.1".to_vec()],
            Alpn::Http1 => vec![b"http/1.1".to_vec()],
        };
        // Honour SSLKEYLOGFILE (no-op when unset) so upstream traffic can be decrypted in
        // Wireshark when debugging.
        cfg.key_log = Arc::new(rustls::KeyLogFile::new());
        let cfg = Arc::new(cfg);
        self.configs.lock().insert(key, cfg.clone());
        Ok(cfg)
    }

    /// Verify a server certificate chain (leaf first) exactly like the rustls connections do
    /// (same roots, same per-host bypass, same error messages and codes). Used by the
    /// impersonation backend, whose TLS stack hands the chain over for verification.
    #[cfg_attr(not(feature = "impersonate"), allow(dead_code))]
    pub(crate) fn verify_chain(
        &self,
        insecure: bool,
        chain: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
    ) -> Result<(), CertFailure> {
        if insecure {
            return Ok(());
        }
        let Some((leaf, intermediates)) = chain.split_first() else {
            return Err(CertFailure {
                message: "the server presented no certificate".into(),
                code: "NO_CERTIFICATE",
            });
        };
        let (strict, _, _) = self.verifiers().map_err(|e| CertFailure {
            message: e.message,
            code: "TLS_CONFIG",
        })?;
        match strict.verify_server_cert(
            leaf,
            intermediates,
            server_name,
            ocsp_response,
            UnixTime::now(),
        ) {
            Ok(_) => Ok(()),
            Err(rustls::Error::InvalidCertificate(ce)) => {
                Err(describe_cert_error(&ce, leaf, intermediates))
            }
            Err(other) => Err(CertFailure {
                message: format!("certificate verification failed: {other}"),
                code: "CERT_INVALID",
            }),
        }
    }

    /// Describe the chain a server presented (leaf first) for [`UpstreamInfo::chain`]: a
    /// summary of every certificate and what the trust decision rested on. Unless verification
    /// was skipped (`insecure`), rustls-webpki rebuilds the path the verifier accepted during the
    /// handshake, against the same roots, to report its certificates and trusted root.
    ///
    /// Runs once per new TLS connection (a few signature checks); pooled exchanges share the
    /// result.
    pub(crate) fn describe_chain(
        &self,
        presented: &[CertificateDer<'_>],
        insecure: bool,
    ) -> Option<Arc<CertificateChain>> {
        if presented.is_empty() {
            return None;
        }
        let certificates = presented.iter().map(|c| cert::summarize_any(c)).collect();
        let trust = if insecure {
            ChainTrust {
                status: ChainTrustStatus::NotVerified,
                ..ChainTrust::default()
            }
        } else {
            self.verified_path(presented).unwrap_or_default()
        };
        Some(Arc::new(CertificateChain {
            certificates,
            trust,
        }))
    }

    /// The verified path of a presented chain, `None` when it can't be rebuilt.
    fn verified_path(&self, presented: &[CertificateDer<'_>]) -> Option<ChainTrust> {
        let (leaf, intermediates) = presented.split_first()?;
        let (_, _, roots) = self.verifiers().ok()?;
        let end_entity = webpki::EndEntityCert::try_from(leaf).ok()?;
        let path = end_entity
            .verify_for_usage(
                provider().signature_verification_algorithms.all,
                &roots.roots,
                intermediates,
                UnixTime::now(),
                webpki::KeyUsage::server_auth(),
                None,
                None,
            )
            .ok()?;
        let mut indices = vec![0];
        for c in path.intermediate_certificates() {
            let der = c.der();
            if let Some(i) = presented.iter().position(|p| p.as_ref() == der.as_ref()) {
                indices.push(i);
            }
        }
        let anchor = path.anchor();
        // A server may send its root too; it's then the certificate the path ended at.
        let index = presented.iter().position(|p| {
            webpki::anchor_from_trusted_cert(p).is_ok_and(|a| same_anchor(&a, anchor))
        });
        Some(ChainTrust {
            status: ChainTrustStatus::Verified,
            path: indices,
            anchor: Some(TrustAnchorInfo {
                subject: cert::name_from_contents(anchor.subject.as_ref())
                    .unwrap_or_else(|| "Unknown root".to_owned()),
                index,
                source: self.anchor_source(anchor),
            }),
        })
    }

    /// Where a trusted root comes from: Mozilla's built-in store, a trusted CA file from the
    /// settings, or (otherwise) the OS trust store.
    fn anchor_source(&self, anchor: &TrustAnchor<'_>) -> TrustAnchorSource {
        if webpki_roots::TLS_SERVER_ROOTS
            .iter()
            .any(|r| same_anchor(r, anchor))
        {
            return TrustAnchorSource::BuiltIn;
        }
        if self
            .extra_roots
            .iter()
            .any(|c| webpki::anchor_from_trusted_cert(c).is_ok_and(|a| same_anchor(&a, anchor)))
        {
            return TrustAnchorSource::Custom;
        }
        TrustAnchorSource::System
    }

    /// The client identity (chain, key) at `index` (see [`TlsContext::client_cert_for`]), with
    /// the same errors as [`TlsContext::client_config`] when it couldn't be loaded.
    #[cfg_attr(not(feature = "impersonate"), allow(dead_code))]
    pub(crate) fn client_identity(
        &self,
        index: usize,
    ) -> Result<Option<(&[CertificateDer<'static>], &PrivateKeyDer<'static>)>, UpstreamError> {
        let Some(entry) = self.client_certs.get(index) else {
            return Ok(None);
        };
        let identity = entry.identity.as_ref().map_err(|e| {
            error(
                ErrorKind::UpstreamTlsFailed,
                format!("client certificate {}: {e}", entry.path),
                "CLIENT_CERT_INVALID",
            )
        })?;
        Ok(Some((&identity.chain, &identity.key)))
    }

    /// The configured path of the client certificate at `index` (for error messages).
    #[cfg_attr(not(feature = "impersonate"), allow(dead_code))]
    pub(crate) fn client_cert_path(&self, index: usize) -> Option<&str> {
        self.client_certs.get(index).map(|c| c.path.as_str())
    }
}

/// Whether two trust anchors are the same root (same name and key).
fn same_anchor(a: &TrustAnchor<'_>, b: &TrustAnchor<'_>) -> bool {
    a.subject.as_ref() == b.subject.as_ref()
        && a.subject_public_key_info.as_ref() == b.subject_public_key_info.as_ref()
}

/// SNI / verification name for a host: DNS name, or IP address (no SNI is sent for IPs).
pub(crate) fn server_name(host: &str) -> Result<ServerName<'static>, UpstreamError> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(ServerName::IpAddress(ip.into()));
    }
    ServerName::try_from(host.to_owned()).map_err(|_| {
        error(
            ErrorKind::UpstreamTlsFailed,
            format!("{host:?} is not a valid TLS server name"),
            "ERR_INVALID_SERVERNAME",
        )
    })
}

/// Fill the negotiated TLS parameters, the leaf certificate summary and the presented chain
/// (`insecure`: verification was skipped for this connection).
pub(crate) fn fill_info(
    info: &mut UpstreamInfo,
    conn: &rustls::ClientConnection,
    tls: &TlsContext,
    insecure: bool,
) {
    info.tls_version = conn.protocol_version().map(|v| {
        match v {
            rustls::ProtocolVersion::TLSv1_3 => "TLSv1.3",
            rustls::ProtocolVersion::TLSv1_2 => "TLSv1.2",
            rustls::ProtocolVersion::TLSv1_1 => "TLSv1.1",
            rustls::ProtocolVersion::TLSv1_0 => "TLSv1",
            _ => "unknown",
        }
        .to_owned()
    });
    info.cipher = conn.negotiated_cipher_suite().map(|s| {
        s.suite()
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{:?}", s.suite()))
    });
    info.alpn = conn
        .alpn_protocol()
        .map(|p| String::from_utf8_lossy(p).into_owned());
    let presented = conn.peer_certificates().unwrap_or_default();
    info.certificate = presented.first().and_then(|der| cert::summarize(der));
    info.chain = tls.describe_chain(presented, insecure);
}

/// A certificate verification failure with a precise message and code (carried through rustls
/// inside [`CertificateError::Other`]).
#[derive(Debug, Clone)]
pub(crate) struct CertFailure {
    pub message: String,
    pub code: &'static str,
}

impl std::fmt::Display for CertFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CertFailure {}

#[derive(Debug)]
struct AkevVerifier {
    inner: Arc<WebPkiServerVerifier>,
    insecure: bool,
    algorithms: WebPkiSupportedAlgorithms,
}

impl AkevVerifier {
    /// In insecure mode a failed handshake-signature check is accepted too: with certificate
    /// verification off it proves nothing an attacker couldn't fake, and it would only make
    /// servers with keys ring refuses (e.g. RSA-1024 on legacy/embedded devices) unreachable
    /// although the user asked to ignore their certificate problems.
    fn lenient(
        &self,
        result: Result<HandshakeSignatureValid, rustls::Error>,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        match result {
            Err(e) if self.insecure => {
                tracing::debug!(error = %e, "ignoring handshake signature failure (insecure mode)");
                Ok(HandshakeSignatureValid::assertion())
            }
            other => other,
        }
    }
}

impl ServerCertVerifier for AkevVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if self.insecure {
            return Ok(ServerCertVerified::assertion());
        }
        match self.inner.verify_server_cert(
            end_entity,
            intermediates,
            server_name,
            ocsp_response,
            now,
        ) {
            Ok(v) => Ok(v),
            Err(rustls::Error::InvalidCertificate(ce)) => {
                let failure = describe_cert_error(&ce, end_entity, intermediates);
                Err(rustls::Error::InvalidCertificate(CertificateError::Other(
                    OtherError(Arc::new(failure)),
                )))
            }
            Err(e) => Err(e),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.lenient(rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.algorithms,
        ))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.lenient(rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.algorithms,
        ))
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

/// Turn a webpki verification error into a human message + code, using the certificate
/// itself for details (validity dates, SANs, issuer).
pub(crate) fn describe_cert_error(
    ce: &CertificateError,
    end_entity: &CertificateDer<'_>,
    intermediates: &[CertificateDer<'_>],
) -> CertFailure {
    let facts = cert::facts(end_entity).unwrap_or_default();
    let (message, code) = match ce {
        CertificateError::Expired | CertificateError::ExpiredContext { .. } => (
            with_detail("certificate has expired", "valid until", &facts.not_after),
            "CERT_HAS_EXPIRED",
        ),
        CertificateError::NotValidYet | CertificateError::NotValidYetContext { .. } => (
            with_detail(
                "certificate is not yet valid",
                "valid from",
                &facts.not_before,
            ),
            "CERT_NOT_YET_VALID",
        ),
        CertificateError::UnknownIssuer => {
            let chain_top_self_signed = intermediates
                .last()
                .and_then(|c| cert::facts(c))
                .is_some_and(|f| f.self_issued);
            if facts.self_issued && intermediates.is_empty() {
                ("self-signed certificate".to_owned(), "SELF_SIGNED")
            } else if chain_top_self_signed {
                (
                    "self-signed certificate in certificate chain".to_owned(),
                    "SELF_SIGNED",
                )
            } else if facts.issuer.is_empty() {
                ("unknown issuer".to_owned(), "UNKNOWN_ISSUER")
            } else {
                (
                    format!("unknown issuer: {}", facts.issuer),
                    "UNKNOWN_ISSUER",
                )
            }
        }
        CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. } => {
            let presented = match ce {
                _ if !facts.san.is_empty() => facts.san.clone(),
                CertificateError::NotValidForNameContext { presented, .. }
                    if !presented.is_empty() =>
                {
                    presented.iter().map(|p| clean_presented_name(p)).collect()
                }
                _ if !facts.subject.is_empty() => vec![facts.subject.clone()],
                _ => Vec::new(),
            };
            if presented.is_empty() {
                (
                    "hostname mismatch: cert has no subject names".to_owned(),
                    "HOSTNAME_MISMATCH",
                )
            } else {
                (
                    format!("hostname mismatch: cert valid for {}", presented.join(", ")),
                    "HOSTNAME_MISMATCH",
                )
            }
        }
        CertificateError::BadSignature => (
            "certificate has an invalid signature".to_owned(),
            "CERT_SIGNATURE_FAILURE",
        ),
        CertificateError::Revoked => ("certificate has been revoked".to_owned(), "CERT_REVOKED"),
        CertificateError::InvalidPurpose | CertificateError::InvalidPurposeContext { .. } => (
            "certificate is not valid for TLS server authentication".to_owned(),
            "CERT_INVALID_PURPOSE",
        ),
        CertificateError::UnhandledCriticalExtension => (
            "certificate has an unsupported critical extension".to_owned(),
            "CERT_UNSUPPORTED_EXTENSION",
        ),
        CertificateError::BadEncoding => {
            ("certificate is malformed".to_owned(), "CERT_BAD_ENCODING")
        }
        CertificateError::UnsupportedSignatureAlgorithmContext { .. }
        | CertificateError::UnsupportedSignatureAlgorithmForPublicKeyContext { .. } => (
            "certificate uses an unsupported signature algorithm".to_owned(),
            "CERT_UNSUPPORTED_ALGORITHM",
        ),
        CertificateError::Other(other) => {
            if let Some(f) = other.0.downcast_ref::<CertFailure>() {
                return f.clone();
            }
            (
                format!("certificate verification failed: {other}"),
                "CERT_INVALID",
            )
        }
        other => (
            format!("certificate verification failed: {other:?}"),
            "CERT_INVALID",
        ),
    };
    CertFailure { message, code }
}

/// webpki reports presented names in Debug form (`DnsName("a.test")`, `IpAddress(..)`).
fn clean_presented_name(name: &str) -> String {
    for prefix in ["DnsName(", "IpAddress(", "UriName(", "Ip("] {
        if let Some(inner) = name.strip_prefix(prefix).and_then(|r| r.strip_suffix(')')) {
            return inner.trim_matches('"').to_owned();
        }
    }
    name.trim_matches('"').to_owned()
}

fn with_detail(base: &str, label: &str, value: &str) -> String {
    if value.is_empty() {
        base.to_owned()
    } else {
        format!("{base} ({label} {value})")
    }
}

/// Map a failed TLS handshake to an [`UpstreamError`].
pub(crate) fn map_handshake_error(e: &io::Error, peer: &str) -> UpstreamError {
    if let Some(tls) = e.get_ref().and_then(|i| i.downcast_ref::<rustls::Error>()) {
        return map_rustls_error(tls, peer);
    }
    match e.kind() {
        io::ErrorKind::UnexpectedEof => error(
            ErrorKind::UpstreamTlsFailed,
            format!("TLS handshake with {peer} failed: connection closed by the server"),
            "TLS_HANDSHAKE_EOF",
        ),
        io::ErrorKind::ConnectionReset
        | io::ErrorKind::ConnectionAborted
        | io::ErrorKind::BrokenPipe => error(
            ErrorKind::UpstreamTlsFailed,
            format!("TLS handshake with {peer} failed: connection reset by the server"),
            "ECONNRESET",
        ),
        _ => from_io(e, format_args!("TLS handshake with {peer}")),
    }
}

/// Map a rustls error to an [`UpstreamError`].
pub(crate) fn map_rustls_error(e: &rustls::Error, peer: &str) -> UpstreamError {
    use rustls::Error as E;
    match e {
        E::InvalidCertificate(ce) => {
            let failure = match ce {
                CertificateError::Other(other) => match other.0.downcast_ref::<CertFailure>() {
                    Some(f) => f.clone(),
                    None => describe_cert_error(ce, &CertificateDer::from(Vec::new()), &[]),
                },
                _ => describe_cert_error(ce, &CertificateDer::from(Vec::new()), &[]),
            };
            error(
                ErrorKind::UpstreamCertInvalid,
                failure.message,
                failure.code,
            )
        }
        E::NoCertificatesPresented => error(
            ErrorKind::UpstreamCertInvalid,
            format!("{peer} presented no certificate"),
            "NO_CERTIFICATE",
        ),
        E::AlertReceived(alert) => {
            let name = alert
                .as_str()
                .map(snake_case)
                .unwrap_or_else(|| format!("{alert:?}"));
            error(
                ErrorKind::UpstreamTlsFailed,
                format!("TLS handshake with {peer} failed: server sent alert {name}"),
                &format!("TLS_ALERT_{}", name.to_ascii_uppercase()),
            )
        }
        E::InvalidMessage(m) => error(
            ErrorKind::UpstreamTlsFailed,
            format!(
                "TLS handshake with {peer} failed: invalid TLS message ({m:?}); the server may not speak TLS on this port"
            ),
            "TLS_INVALID_MESSAGE",
        ),
        E::PeerIncompatible(p) => error(
            ErrorKind::UpstreamTlsFailed,
            format!("TLS handshake with {peer} failed: incompatible server ({p:?})"),
            "TLS_PEER_INCOMPATIBLE",
        ),
        E::PeerMisbehaved(p) => error(
            ErrorKind::UpstreamTlsFailed,
            format!("TLS handshake with {peer} failed: protocol violation ({p:?})"),
            "TLS_PEER_MISBEHAVED",
        ),
        E::NoApplicationProtocol => error(
            ErrorKind::UpstreamTlsFailed,
            format!("TLS handshake with {peer} failed: no common application protocol (ALPN)"),
            "TLS_NO_APPLICATION_PROTOCOL",
        ),
        other => error(
            ErrorKind::UpstreamTlsFailed,
            format!("TLS handshake with {peer} failed: {other}"),
            "TLS_ERROR",
        ),
    }
}

/// Map a TLS error that surfaced on an established connection (e.g. a TLS 1.3 server
/// rejecting the client certificate after the client finished its handshake).
pub(crate) fn map_tls_io_error(e: &rustls::Error, context: &str) -> UpstreamError {
    match e {
        rustls::Error::AlertReceived(alert) => alert_error(
            &alert
                .as_str()
                .map(snake_case)
                .unwrap_or_else(|| format!("{alert:?}")),
            context,
        ),
        other => error(
            ErrorKind::UpstreamTlsFailed,
            format!("{context}: TLS error: {other}"),
            "TLS_ERROR",
        ),
    }
}

/// Recognise rustls' alert message in an IO error whose source was stripped (h2 re-creates
/// IO errors from kind + message).
pub(crate) fn alert_from_message(message: &str, context: &str) -> Option<UpstreamError> {
    let name = message.strip_prefix("received fatal alert: ")?.trim();
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    Some(alert_error(&snake_case(name), context))
}

fn alert_error(name: &str, context: &str) -> UpstreamError {
    let message = match name {
        "certificate_required" => {
            format!("{context}: the server requires a client certificate (TLS alert {name})")
        }
        "bad_certificate" | "unknown_c_a" | "certificate_unknown" | "access_denied" => {
            format!("{context}: the server rejected the client certificate (TLS alert {name})")
        }
        _ => format!("{context}: the server sent TLS alert {name}"),
    };
    error(
        ErrorKind::UpstreamTlsFailed,
        message,
        &format!("TLS_ALERT_{}", name.to_ascii_uppercase()),
    )
}

/// `HandshakeFailure` → `handshake_failure`.
fn snake_case(camel: &str) -> String {
    let mut out = String::with_capacity(camel.len() + 4);
    for (i, ch) in camel.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

fn load_pem_certs(path: &str) -> Result<Vec<CertificateDer<'static>>, String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    CertificateDer::pem_slice_iter(&data)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("invalid PEM: {e}"))
}

fn looks_like_pem(data: &[u8]) -> bool {
    let start = data
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(data.len());
    data[start..].starts_with(b"-----BEGIN")
}

fn load_identity(path: &str, passphrase: Option<&str>) -> Result<Identity, String> {
    let data = std::fs::read(path).map_err(|e| format!("cannot read file: {e}"))?;
    if looks_like_pem(&data) {
        load_pem_identity(&data)
    } else {
        load_pkcs12_identity(&data, passphrase.unwrap_or(""))
    }
}

fn load_pem_identity(data: &[u8]) -> Result<Identity, String> {
    let chain = CertificateDer::pem_slice_iter(data)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("invalid PEM: {e}"))?;
    // The first PKCS#1, PKCS#8 or SEC1 key in the file.
    let key = PrivateKeyDer::from_pem_slice(data).ok();
    let key = key.ok_or_else(|| {
        let contains = |needle: &[u8]| memchr::memmem::find(data, needle).is_some();
        if contains(b"ENCRYPTED PRIVATE KEY") || contains(b"Proc-Type: 4,ENCRYPTED") {
            "encrypted PEM private keys are not supported; use a PKCS#12 (.p12) file or decrypt the key".to_owned()
        } else {
            "no private key found in PEM file".to_owned()
        }
    })?;
    if chain.is_empty() {
        return Err("no certificate found in PEM file".into());
    }
    Ok(Identity { chain, key })
}

fn load_pkcs12_identity(data: &[u8], passphrase: &str) -> Result<Identity, String> {
    use p12_keystore::{KeyStore, Pkcs12ImportPolicy};
    let keystore = KeyStore::from_pkcs12(data, passphrase, Pkcs12ImportPolicy::Strict)
        .or_else(|_| KeyStore::from_pkcs12(data, passphrase, Pkcs12ImportPolicy::Relaxed))
        .map_err(|e| format!("cannot read PKCS#12 file (wrong passphrase?): {e}"))?;
    let (_, chain) = keystore
        .private_key_chain()
        .ok_or("no private key found in PKCS#12 file")?;
    let certs: Vec<CertificateDer<'static>> = chain
        .certs()
        .iter()
        .map(|c| CertificateDer::from(c.as_der().to_vec()))
        .collect();
    if certs.is_empty() {
        return Err("no certificate found for the private key in PKCS#12 file".into());
    }
    Ok(Identity {
        chain: certs,
        key: PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(chain.key().as_der().to_vec())),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use akev_core::settings::ClientCertificate;

    #[test]
    fn alert_names() {
        assert_eq!(snake_case("HandshakeFailure"), "handshake_failure");
        assert_eq!(snake_case("UnknownCA"), "unknown_c_a");
        let e = map_rustls_error(
            &rustls::Error::AlertReceived(rustls::AlertDescription::HandshakeFailure),
            "h:443",
        );
        assert_eq!(e.kind, ErrorKind::UpstreamTlsFailed);
        assert_eq!(e.code.as_deref(), Some("TLS_ALERT_HANDSHAKE_FAILURE"));
        assert!(e.message.contains("handshake_failure"));
    }

    #[test]
    fn cert_failures_round_trip_through_rustls_errors() {
        let failure = CertFailure {
            message: "self-signed certificate".into(),
            code: "SELF_SIGNED",
        };
        let e = rustls::Error::InvalidCertificate(CertificateError::Other(OtherError(Arc::new(
            failure,
        ))));
        let io = io::Error::new(io::ErrorKind::InvalidData, e);
        let mapped = map_handshake_error(&io, "self.test:443");
        assert_eq!(mapped.kind, ErrorKind::UpstreamCertInvalid);
        assert_eq!(mapped.code.as_deref(), Some("SELF_SIGNED"));
        assert_eq!(mapped.message, "self-signed certificate");

        let plain = map_rustls_error(
            &rustls::Error::InvalidCertificate(CertificateError::Expired),
            "x",
        );
        assert_eq!(plain.code.as_deref(), Some("CERT_HAS_EXPIRED"));
        let plain = map_rustls_error(
            &rustls::Error::InvalidCertificate(CertificateError::UnknownIssuer),
            "x",
        );
        assert_eq!(plain.code.as_deref(), Some("UNKNOWN_ISSUER"));
    }

    #[test]
    fn hostname_mismatch_lists_presented_names() {
        let ce = CertificateError::NotValidForNameContext {
            expected: ServerName::try_from("wrong.test").unwrap(),
            presented: vec!["right.test".into(), "*.right.test".into()],
        };
        let f = describe_cert_error(&ce, &CertificateDer::from(Vec::new()), &[]);
        assert_eq!(f.code, "HOSTNAME_MISMATCH");
        assert_eq!(
            f.message,
            "hostname mismatch: cert valid for right.test, *.right.test"
        );
    }

    #[test]
    fn handshake_io_errors() {
        let e = map_handshake_error(&io::Error::from(io::ErrorKind::UnexpectedEof), "h:1");
        assert_eq!(e.kind, ErrorKind::UpstreamTlsFailed);
        let e = map_handshake_error(&io::Error::from(io::ErrorKind::TimedOut), "h:1");
        assert_eq!(e.kind, ErrorKind::Timeout);
    }

    #[test]
    fn server_names() {
        assert!(matches!(
            server_name("127.0.0.1").unwrap(),
            ServerName::IpAddress(_)
        ));
        assert!(matches!(
            server_name("::1").unwrap(),
            ServerName::IpAddress(_)
        ));
        assert!(matches!(
            server_name("example.com").unwrap(),
            ServerName::DnsName(_)
        ));
        assert!(server_name("bad host").is_err());
    }

    fn write_identity(dir: &std::path::Path) -> (String, String, Vec<u8>) {
        let key = rcgen::KeyPair::generate().unwrap();
        let params = rcgen::CertificateParams::new(vec!["client.test".into()]).unwrap();
        let cert = params.self_signed(&key).unwrap();
        let pem_path = dir.join("client.pem");
        std::fs::write(&pem_path, format!("{}{}", cert.pem(), key.serialize_pem())).unwrap();
        let mut ks = p12_keystore::KeyStore::new();
        let chain = p12_keystore::PrivateKeyChain::new(
            vec![1u8],
            p12_keystore::PrivateKey::from_der(&key.serialize_der()).unwrap(),
            [p12_keystore::Certificate::from_der(cert.der()).unwrap()],
        );
        ks.add_entry(
            "client",
            p12_keystore::KeyStoreEntry::PrivateKeyChain(chain),
        );
        let p12 = ks.writer("s3cret").write().unwrap();
        let p12_path = dir.join("client.p12");
        std::fs::write(&p12_path, p12).unwrap();
        (
            pem_path.to_string_lossy().into_owned(),
            p12_path.to_string_lossy().into_owned(),
            cert.der().to_vec(),
        )
    }

    #[test]
    fn loads_pem_and_pkcs12_identities() {
        let dir = tempfile::tempdir().unwrap();
        let (pem, p12, der) = write_identity(dir.path());
        let id = load_identity(&pem, None).unwrap();
        assert_eq!(id.chain[0].as_ref(), der.as_slice());
        let id = load_identity(&p12, Some("s3cret")).unwrap();
        assert_eq!(id.chain[0].as_ref(), der.as_slice());
        assert!(load_identity(&p12, Some("wrong")).is_err());
        assert!(load_identity("/nonexistent/file.pem", None).is_err());

        let settings = ProxySettings {
            client_certificates: vec![
                ClientCertificate {
                    host: "*.secure.test".into(),
                    path: p12.clone(),
                    passphrase: Some("s3cret".into()),
                },
                ClientCertificate {
                    host: "broken.test".into(),
                    path: "/nonexistent.p12".into(),
                    passphrase: None,
                },
            ],
            ..Default::default()
        };
        let ctx = TlsContext::new(&settings);
        assert_eq!(ctx.client_cert_for("api.secure.test", 443), Some(0));
        assert_eq!(ctx.client_cert_for("other.test", 443), None);
        assert!(ctx.client_config(false, Alpn::Http1, Some(0)).is_ok());
        let err = ctx
            .client_config(false, Alpn::Http1, ctx.client_cert_for("broken.test", 443))
            .unwrap_err();
        assert_eq!(err.code.as_deref(), Some("CLIENT_CERT_INVALID"));
    }

    #[test]
    fn only_insecure_mode_tolerates_bad_handshake_signatures() {
        let ctx = TlsContext::new(&ProxySettings::default());
        let (strict, lax, _) = ctx.verifiers().unwrap();
        let failure = || Err(rustls::Error::General("bad signature".into()));
        assert!(strict.lenient(failure()).is_err());
        assert!(lax.lenient(failure()).is_ok());
        assert!(
            strict
                .lenient(Ok(HandshakeSignatureValid::assertion()))
                .is_ok()
        );
    }

    #[test]
    fn encrypted_pem_keys_get_a_helpful_error() {
        let pem = b"-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n-----BEGIN ENCRYPTED PRIVATE KEY-----\nAAAA\n-----END ENCRYPTED PRIVATE KEY-----\n";
        let err = load_pem_identity(pem).err().unwrap();
        assert!(
            err.contains("encrypted PEM private keys are not supported"),
            "{err}"
        );
    }

    #[test]
    fn configs_are_cached_per_mode() {
        let ctx = TlsContext::new(&ProxySettings {
            ignore_upstream_cert_errors: vec!["*.dev.test".into(), "pinned.test:8443".into()],
            ..Default::default()
        });
        assert!(ctx.ignores_errors_for("a.dev.test", 443));
        assert!(ctx.ignores_errors_for("pinned.test", 8443));
        assert!(!ctx.ignores_errors_for("pinned.test", 443));
        let a = ctx.client_config(false, Alpn::H2AndHttp1, None).unwrap();
        let b = ctx.client_config(false, Alpn::H2AndHttp1, None).unwrap();
        let c = ctx.client_config(true, Alpn::H2AndHttp1, None).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        assert!(!Arc::ptr_eq(&a, &c));
        assert_eq!(a.alpn_protocols, vec![b"h2".to_vec(), b"http/1.1".to_vec()]);
        let h1 = ctx.client_config(false, Alpn::Http1, None).unwrap();
        assert_eq!(h1.alpn_protocols, vec![b"http/1.1".to_vec()]);
    }
}
