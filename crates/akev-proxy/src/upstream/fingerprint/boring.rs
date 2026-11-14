//! The BoringSSL backend (cargo feature `impersonate`): TLS connections whose ClientHello
//! reproduces a browser's (see [`super::profiles`]).
//!
//! BoringSSL is Chrome's own TLS library; the `boring2` bindings carry the patches that make
//! the remaining browsers reproducible (fixed extension order, record size limit, delegated
//! credentials, key share limit, ALPS codepoint, preserved TLS 1.3 cipher order, ffdhe groups,
//! legacy 3DES suites) — the approach of curl-impersonate and the `wreq` crate.
//!
//! Certificate verification is *not* done by BoringSSL: a custom verify callback hands the
//! chain to [`TlsContext::verify_chain`], so impersonated connections trust exactly what the
//! rustls connections trust (webpki + OS roots + `trusted_ca_paths`), honour
//! `ignore_upstream_cert_errors`, and fail with the same messages and codes. Client
//! certificates come from the same [`TlsContext`] too.

use super::Browser;
use super::profiles::{self, CertCompression, TlsSpec};
use crate::upstream::UpstreamError;
use crate::upstream::cert;
use crate::upstream::error::{error, from_io};
use crate::upstream::tls::{Alpn, CertFailure, TlsContext};
use akev_core::{ErrorKind, UpstreamInfo};
use boring2::pkey::PKey;
use boring2::ssl::{
    CertificateCompressionAlgorithm, CertificateCompressor, ErrorCode, ExtensionType, SslAlert,
    SslConnector, SslMethod, SslOptions, SslVerifyError, SslVerifyMode, SslVersion,
};
use boring2::x509::X509;
use parking_lot::Mutex;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_boring2::SslStream;

/// Largest decompressed certificate message accepted (BoringSSL bounds it as well).
const MAX_DECOMPRESSED_CERT: u64 = 1 << 20;

macro_rules! decompressor {
    ($name:ident, $alg:expr, |$input:ident, $out:ident| $body:expr) => {
        struct $name;
        impl CertificateCompressor for $name {
            const ALGORITHM: CertificateCompressionAlgorithm = $alg;
            const CAN_COMPRESS: bool = false;
            const CAN_DECOMPRESS: bool = true;
            fn decompress<W: Write>(&self, $input: &[u8], output: &mut W) -> io::Result<()> {
                let mut $out = Vec::new();
                $body?;
                output.write_all(&$out)
            }
        }
    };
}

decompressor!(
    ZlibDecompressor,
    CertificateCompressionAlgorithm::ZLIB,
    |input, out| flate2::read::ZlibDecoder::new(input)
        .take(MAX_DECOMPRESSED_CERT)
        .read_to_end(&mut out)
);
decompressor!(
    BrotliDecompressor,
    CertificateCompressionAlgorithm::BROTLI,
    |input, out| brotli::Decompressor::new(input, 4096)
        .take(MAX_DECOMPRESSED_CERT)
        .read_to_end(&mut out)
);
decompressor!(
    ZstdDecompressor,
    CertificateCompressionAlgorithm::ZSTD,
    |input, out| zstd::stream::read::Decoder::new(input)
        .and_then(|d| d.take(MAX_DECOMPRESSED_CERT).read_to_end(&mut out))
);

fn config_error(what: &str, e: impl std::fmt::Display) -> UpstreamError {
    error(
        ErrorKind::Internal,
        format!("TLS impersonation configuration error ({what}): {e}"),
        "TLS_CONFIG",
    )
}

/// Build the `SSL_CTX` of a browser profile for an ALPN offer.
fn build(spec: &TlsSpec, alpn: Alpn) -> Result<SslConnector, UpstreamError> {
    let mut b = SslConnector::no_default_verify_builder(SslMethod::tls_client())
        .map_err(|e| config_error("context", e))?;
    if !spec.session_ticket {
        b.set_options(SslOptions::NO_TICKET);
    }
    b.set_grease_enabled(spec.grease);
    b.set_permute_extensions(spec.permute_extensions);
    if !spec.permute_extensions {
        let order: Vec<ExtensionType> = spec
            .extensions
            .iter()
            .map(|&id| ExtensionType::from(id))
            .collect();
        b.set_extension_permutation(&order)
            .map_err(|e| config_error("extension order", e))?;
    }
    // Keep the TLS 1.3 suites in the profile's order, independent of AES hardware support.
    b.set_preserve_tls13_cipher_list(true);
    b.set_aes_hw_override(true);
    b.set_strict_cipher_list(spec.cipher_list)
        .map_err(|e| config_error("cipher suites", e))?;
    b.set_curves_list(spec.curves)
        .map_err(|e| config_error("groups", e))?;
    b.set_sigalgs_list(spec.sigalgs_list)
        .map_err(|e| config_error("signature algorithms", e))?;
    b.set_min_proto_version(Some(SslVersion::TLS1_2))
        .and_then(|_| b.set_max_proto_version(Some(SslVersion::TLS1_3)))
        .map_err(|e| config_error("protocol versions", e))?;
    b.enable_ocsp_stapling();
    b.enable_signed_cert_timestamps();
    for alg in spec.cert_compression {
        match alg {
            CertCompression::Zlib => b.add_certificate_compression_algorithm(ZlibDecompressor),
            CertCompression::Brotli => b.add_certificate_compression_algorithm(BrotliDecompressor),
            CertCompression::Zstd => b.add_certificate_compression_algorithm(ZstdDecompressor),
        }
        .map_err(|e| config_error("certificate compression", e))?;
    }
    if let Some(limit) = spec.record_size_limit {
        b.set_record_size_limit(limit);
    }
    if let Some(algs) = spec.delegated_credentials {
        b.set_delegated_credentials(algs)
            .map_err(|e| config_error("delegated credentials", e))?;
    }
    if let Some(limit) = spec.key_shares_limit {
        b.set_key_shares_limit(limit);
    }
    let protos: &[u8] = match alpn {
        Alpn::H2AndHttp1 => b"\x02h2\x08http/1.1",
        Alpn::Http1 => b"\x08http/1.1",
    };
    b.set_alpn_protos(protos)
        .map_err(|e| config_error("ALPN", e))?;
    // Honour SSLKEYLOGFILE like the rustls connections, so impersonated traffic can be
    // decrypted in Wireshark too.
    if let Some(path) = std::env::var_os("SSLKEYLOGFILE").filter(|p| !p.is_empty()) {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path);
        match file {
            Ok(file) => {
                let file = Mutex::new(file);
                b.set_keylog_callback(move |_, line| {
                    let _ = writeln!(file.lock(), "{line}");
                });
            }
            Err(e) => tracing::warn!(path = ?path, error = %e, "cannot open SSLKEYLOGFILE"),
        }
    }
    Ok(b.build())
}

/// Process-wide cache of contexts per (browser, ALPN offer): they hold no per-settings state
/// (verification and client certificates are configured per connection).
fn connector(browser: Browser, alpn: Alpn) -> Result<SslConnector, UpstreamError> {
    static CACHE: OnceLock<Mutex<HashMap<(Browser, Alpn), SslConnector>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(c) = cache.lock().get(&(browser, alpn)) {
        return Ok(c.clone());
    }
    let built = build(profiles::profile(browser).tls, alpn)?;
    cache.lock().insert((browser, alpn), built.clone());
    Ok(built)
}

/// Everything needed to open one impersonated TLS connection.
pub(crate) struct BoringTarget {
    pub browser: Browser,
    pub alpn: Alpn,
    pub tls: Arc<TlsContext>,
    pub insecure: bool,
    pub client_cert: Option<usize>,
}

/// Set the client certificate (chain + key) on a connection.
fn set_identity(
    ssl: &mut boring2::ssl::SslRef,
    chain: &[CertificateDer<'static>],
    key: &PrivateKeyDer<'static>,
    path: &str,
) -> Result<(), UpstreamError> {
    let invalid = |what: &str, e: &dyn std::fmt::Display| {
        error(
            ErrorKind::UpstreamTlsFailed,
            format!("client certificate {path} is unusable ({what}): {e}"),
            "CLIENT_CERT_INVALID",
        )
    };
    let (leaf, rest) = chain
        .split_first()
        .ok_or_else(|| invalid("chain", &"no certificate"))?;
    let leaf = X509::from_der(leaf).map_err(|e| invalid("certificate", &e))?;
    ssl.set_certificate(&leaf)
        .map_err(|e| invalid("certificate", &e))?;
    for c in rest {
        let c = X509::from_der(c).map_err(|e| invalid("chain", &e))?;
        ssl.add_chain_cert(&c).map_err(|e| invalid("chain", &e))?;
    }
    let der = key.secret_der();
    let key = PKey::private_key_from_der(der)
        .or_else(|_| PKey::private_key_from_pkcs8(der))
        .map_err(|e| invalid("private key", &e))?;
    ssl.set_private_key(&key)
        .map_err(|e| invalid("private key", &e))
}

/// Run the TLS handshake over `io` (SNI = `host` unless it is an IP address).
pub(crate) async fn connect<S>(
    target: BoringTarget,
    host: &str,
    server_name: ServerName<'static>,
    io: S,
    peer: &str,
) -> Result<SslStream<S>, UpstreamError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let spec = profiles::profile(target.browser).tls;
    let mut cfg = connector(target.browser, target.alpn)?
        .configure()
        .map_err(|e| config_error("connection", e))?;
    // Hostname checks are part of the verification below.
    cfg.set_verify_hostname(false);
    if spec.ech_grease {
        cfg.set_enable_ech_grease(true);
    }
    if spec.alps && target.alpn == Alpn::H2AndHttp1 {
        cfg.add_application_settings(b"h2")
            .map_err(|e| config_error("ALPS", e))?;
        cfg.set_alps_use_new_codepoint(true);
    }
    if let Some(index) = target.client_cert
        && let Some((chain, key)) = target.tls.client_identity(index)?
    {
        let path = target.tls.client_cert_path(index).unwrap_or_default();
        set_identity(&mut cfg, chain, key, path)?;
    }
    let failure: Arc<Mutex<Option<CertFailure>>> = Arc::default();
    {
        let tls = target.tls.clone();
        let insecure = target.insecure;
        let failure = failure.clone();
        let name = server_name.clone();
        cfg.set_custom_verify_callback(SslVerifyMode::PEER, move |ssl| {
            let chain = presented_chain(ssl);
            let ocsp = ssl.ocsp_status().unwrap_or_default();
            match tls.verify_chain(insecure, &chain, &name, ocsp) {
                Ok(()) => Ok(()),
                Err(f) => {
                    *failure.lock() = Some(f);
                    Err(SslVerifyError::Invalid(SslAlert::BAD_CERTIFICATE))
                }
            }
        });
    }
    let ssl = cfg
        .into_ssl(host)
        .map_err(|e| config_error("server name", e))?;
    let mut stream = SslStream::new(ssl, io).map_err(|e| config_error("stream", e))?;
    match Pin::new(&mut stream).connect().await {
        Ok(()) => Ok(stream),
        Err(e) => {
            if let Some(f) = failure.lock().take() {
                return Err(error(ErrorKind::UpstreamCertInvalid, f.message, f.code));
            }
            Err(map_handshake_error(&e, peer))
        }
    }
}

/// The certificates the server presented, leaf first (BoringSSL's client-side chain may or
/// may not repeat the leaf).
fn presented_chain(ssl: &boring2::ssl::SslRef) -> Vec<CertificateDer<'static>> {
    let mut chain: Vec<CertificateDer<'static>> = Vec::new();
    if let Some(leaf) = ssl.peer_certificate().and_then(|c| c.to_der().ok()) {
        chain.push(CertificateDer::from(leaf));
    }
    if let Some(stack) = ssl.peer_cert_chain() {
        for c in stack {
            if let Ok(der) = c.to_der()
                && chain
                    .first()
                    .is_none_or(|leaf| leaf.as_ref() != der.as_slice())
            {
                chain.push(CertificateDer::from(der));
            }
        }
    }
    chain
}

/// Fill the negotiated parameters, the leaf certificate summary and the presented chain
/// (`insecure`: verification was skipped for this connection).
pub(crate) fn fill_info<S>(
    info: &mut UpstreamInfo,
    stream: &SslStream<S>,
    tls: &TlsContext,
    insecure: bool,
) {
    let ssl = stream.ssl();
    info.tls_version = Some(ssl.version_str().to_owned()).filter(|v| v != "unknown");
    info.cipher = ssl
        .current_cipher()
        .map(|c| c.standard_name().unwrap_or(c.name()).to_owned());
    info.alpn = ssl
        .selected_alpn_protocol()
        .map(|p| String::from_utf8_lossy(p).into_owned());
    let chain = presented_chain(ssl);
    info.certificate = chain.first().and_then(|der| cert::summarize(der));
    info.chain = tls.describe_chain(&chain, insecure);
}

/// Whether ALPN negotiated `h2`.
pub(crate) fn negotiated_h2<S>(stream: &SslStream<S>) -> bool {
    stream.ssl().selected_alpn_protocol() == Some(b"h2")
}

/// Map a failed BoringSSL handshake to an [`UpstreamError`] with the same codes as the
/// rustls path (`TLS_ALERT_*`, `TLS_HANDSHAKE_EOF`, `ECONNRESET`...).
fn map_handshake_error(e: &boring2::ssl::Error, peer: &str) -> UpstreamError {
    if let Some(io) = e.io_error() {
        return crate::upstream::tls::map_handshake_error(io, peer);
    }
    let reasons: Vec<String> = e
        .ssl_error()
        .map(|stack| {
            stack
                .errors()
                .iter()
                .filter_map(|err| err.reason().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let tls_failed =
        |message: String, code: &str| error(ErrorKind::UpstreamTlsFailed, message, code);
    for reason in &reasons {
        let upper = reason.to_ascii_uppercase().replace(' ', "_");
        let alert = ["SSLV3_ALERT_", "TLSV1_ALERT_", "TLSV13_ALERT_", "TLSV1_"]
            .iter()
            .find_map(|p| upper.strip_prefix(p).filter(|_| upper.contains("ALERT")));
        if let Some(name) = alert {
            let name = name.to_ascii_lowercase();
            return tls_failed(
                format!("TLS handshake with {peer} failed: server sent alert {name}"),
                &format!("TLS_ALERT_{}", name.to_ascii_uppercase()),
            );
        }
        match upper.as_str() {
            "WRONG_VERSION_NUMBER" | "UNKNOWN_PROTOCOL" | "HTTP_REQUEST" | "BAD_PACKET_LENGTH" => {
                return tls_failed(
                    format!(
                        "TLS handshake with {peer} failed: invalid TLS message ({reason}); the server may not speak TLS on this port"
                    ),
                    "TLS_INVALID_MESSAGE",
                );
            }
            "NO_SHARED_CIPHER" | "UNSUPPORTED_PROTOCOL" | "NO_SHARED_GROUP" => {
                return tls_failed(
                    format!("TLS handshake with {peer} failed: incompatible server ({reason})"),
                    "TLS_PEER_INCOMPATIBLE",
                );
            }
            _ => {}
        }
    }
    match e.code() {
        ErrorCode::SYSCALL | ErrorCode::ZERO_RETURN if reasons.is_empty() => tls_failed(
            format!("TLS handshake with {peer} failed: connection closed by the server"),
            "TLS_HANDSHAKE_EOF",
        ),
        _ if reasons.is_empty() => from_io(
            &io::Error::other(e.to_string()),
            format_args!("TLS handshake with {peer}"),
        ),
        _ => tls_failed(
            format!("TLS handshake with {peer} failed: {}", reasons.join(", ")),
            "TLS_ERROR",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_profile_builds_for_both_alpn_offers() {
        for browser in Browser::ALL {
            for alpn in [Alpn::H2AndHttp1, Alpn::Http1] {
                build(profiles::profile(browser).tls, alpn)
                    .unwrap_or_else(|e| panic!("{browser:?} {alpn:?}: {e:?}"));
                // Cached on second use.
                connector(browser, alpn).unwrap();
            }
        }
    }

    fn decompress<C: CertificateCompressor>(c: C, input: &[u8]) -> io::Result<Vec<u8>> {
        let mut out = Vec::new();
        c.decompress(input, &mut out)?;
        Ok(out)
    }

    #[test]
    fn certificate_decompressors() {
        let cert = b"0\x82\x01\x0a certificate bytes ".repeat(50);
        let mut zlib = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        zlib.write_all(&cert).unwrap();
        assert_eq!(
            decompress(ZlibDecompressor, &zlib.finish().unwrap()).unwrap(),
            cert
        );

        let mut br = Vec::new();
        {
            let mut w = brotli::CompressorWriter::new(&mut br, 4096, 5, 22);
            w.write_all(&cert).unwrap();
        }
        assert_eq!(decompress(BrotliDecompressor, &br).unwrap(), cert);

        let zst = zstd::stream::encode_all(&cert[..], 3).unwrap();
        assert_eq!(decompress(ZstdDecompressor, &zst).unwrap(), cert);

        assert!(decompress(ZstdDecompressor, b"not zstd").is_err());
        assert!(decompress(ZlibDecompressor, b"not zlib").is_err());
    }

    #[test]
    fn decompression_is_bounded() {
        // 8 MiB of zeros compresses to a few KB; only MAX_DECOMPRESSED_CERT is produced (and
        // BoringSSL then rejects the length mismatch).
        let bomb = zstd::stream::encode_all(&vec![0u8; 8 << 20][..], 19).unwrap();
        assert!(bomb.len() < 64 * 1024);
        let out = decompress(ZstdDecompressor, &bomb).unwrap();
        assert_eq!(out.len() as u64, MAX_DECOMPRESSED_CERT);
    }
}
