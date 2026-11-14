//! # akev-ca
//!
//! Akev's certificate authority: generates (or loads) a root CA and mints leaf certificates
//! for intercepted hostnames on the fly, cached in an LRU.
//!
//! * CA key: ECDSA P-256 (fast, universally supported). Subject `O=Akev, OU=<install id>,
//!   CN=Akev CA` — the OU is a short random id so users can tell installs (and CA
//!   generations) apart in their trust stores. `basicConstraints` CA:true, pathlen 0
//!   (critical); `keyUsage` keyCertSign | cRLSign | digitalSignature (critical); subject and
//!   authority key identifiers; random positive 16-byte serial.
//! * CA validity: `now − 1 day` (clock-skew tolerance) to `now + 10 years`
//!   ([`CA_VALIDITY_DAYS`]): every device that trusts the CA must re-trust a new one, so it
//!   is never replaced behind the user's back. [`CertificateAuthority::load_or_generate`]
//!   keeps using a CA that expires soon or has expired and [`CaInfo::expiry`] reports it
//!   ([`EXPIRY_WARNING_DAYS`]); only the user replaces it
//!   ([`CertificateAuthority::regenerate`]). Leaves stay short-lived.
//! * Leaf certs: one shared ECDSA P-256 key for all leaves (generated at construction);
//!   SAN = the normalised hostname (lower-case, IDNA A-labels) plus a wildcard sibling
//!   `*.parent` when the host has 3+ labels (never `*.com` / `*.co.uk`), or the IP address
//!   (IPv4 or IPv6) for IP literals; CN = host; EKU serverAuth; keyUsage digitalSignature;
//!   `basicConstraints` CA:false; AKI = CA SKI. Validity backdated 1 day and ending at
//!   `min(now + 397 days, CA not_after)` (Apple's 398-day limit); serial random 16 bytes;
//!   chain = `[leaf, CA]`.
//! * Leaf cache: LRU of [`LEAF_CACHE_CAPACITY`] entries keyed by normalised host, with
//!   single-flight generation so a burst of parallel connections to a new host mints exactly
//!   one certificate. Minting takes well under a millisecond.
//!
//! Bring-your-own CAs ([`CertificateAuthority::from_pem`]) may use ECDSA P-256/P-384, RSA
//! (PKCS#1 v1.5 SHA-256 signatures) or Ed25519 keys in PKCS#8, SEC1 or PKCS#1 PEM, and may
//! carry a chain (issuing CA first) which is then served after every leaf.

mod der;
mod host;
mod info;
mod leaf;
mod storage;
#[cfg(test)]
mod tests;

use std::fmt;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Arc;

use base64::Engine as _;
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, Issuer, KeyPair,
    KeyUsagePurpose, SerialNumber,
};
use rustls_pki_types::pem::PemObject as _;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use time::{Duration, OffsetDateTime};
use x509_parser::certificate::X509Certificate;

pub use rustls::sign::CertifiedKey;
pub use storage::{CA_CERT_FILE, CA_KEY_FILE};

use crate::host::LeafName;
use crate::info::CaDetails;
use crate::leaf::{LeafCache, LeafIssuer};

/// Errors produced by the certificate authority.
#[derive(Debug, thiserror::Error)]
pub enum CaError {
    /// Reading or writing the CA files failed.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// Key generation or certificate signing failed.
    #[error("certificate generation failed: {0}")]
    Generate(String),
    /// A certificate, key or host name was rejected.
    #[error("invalid certificate or key: {0}")]
    Invalid(String),
}

impl CaError {
    /// The message without the variant prefix (used when re-wrapping an error).
    fn detail(&self) -> String {
        match self {
            CaError::Io(e) => e.to_string(),
            CaError::Generate(m) | CaError::Invalid(m) => m.clone(),
        }
    }
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, CaError>;

/// Summary information about the CA certificate (shown in Settings / setup pages).
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaInfo {
    /// Subject distinguished name, e.g. `O=Akev, OU=1a2b3c4d, CN=Akev CA`.
    pub subject: String,
    /// SHA-256 fingerprint, uppercase hex with colons.
    pub sha256_fingerprint: String,
    /// Base64 SHA-256 of the SubjectPublicKeyInfo (Chrome `--ignore-certificate-errors-spki-list`).
    pub spki_fingerprint: String,
    /// OpenSSL `subject_hash_old` (Android system store filename, e.g. `c8750f0d`).
    pub subject_hash_old: String,
    /// RFC3339
    pub not_before: String,
    /// RFC3339
    pub not_after: String,
    /// Days until expiry (negative once expired).
    pub days_remaining: i64,
    /// Whether the certificate is fine, expires within [`EXPIRY_WARNING_DAYS`] or has expired.
    pub expiry: CaExpiry,
}

/// Expiry state of the CA certificate ([`CaInfo::expiry`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CaExpiry {
    /// More than [`EXPIRY_WARNING_DAYS`] left.
    Valid,
    /// Expires within [`EXPIRY_WARNING_DAYS`]: the user should regenerate it soon.
    Expiring,
    /// Expired: HTTPS interception fails until the user regenerates it.
    Expired,
}

impl CaInfo {
    /// A warning for the user when the CA expires soon or has expired (`None` while it is
    /// valid for more than [`EXPIRY_WARNING_DAYS`]). `how` says how to regenerate it in the
    /// caller's context, e.g. "in Settings → Certificates".
    pub fn expiry_warning(&self, how: &str) -> Option<String> {
        let date = self.not_after.split('T').next().unwrap_or(&self.not_after);
        match self.expiry {
            CaExpiry::Valid => None,
            CaExpiry::Expiring => Some(format!(
                "The Akev CA certificate expires in {} day(s), on {date}. Regenerate it {how} and trust the new one on your devices.",
                self.days_remaining.max(0)
            )),
            CaExpiry::Expired => Some(format!(
                "The Akev CA certificate expired on {date}, so HTTPS interception fails. Regenerate it {how} and trust the new one on your devices."
            )),
        }
    }
}

/// Maximum number of cached leaf certificates.
pub const LEAF_CACHE_CAPACITY: usize = 2048;
/// Validity of a freshly generated CA, in days (counted from generation time): 10 years.
pub const CA_VALIDITY_DAYS: i64 = 3650;
/// [`CaInfo::expiry`] turns to [`CaExpiry::Expiring`] when fewer days than this are left.
pub const EXPIRY_WARNING_DAYS: i64 = 60;
/// Maximum leaf validity in days (Apple rejects server certificates valid for > 398 days).
pub const LEAF_MAX_VALIDITY_DAYS: i64 = 397;

const BACKDATE: Duration = Duration::days(1);
const CA_VALIDITY: Duration = Duration::days(CA_VALIDITY_DAYS);
pub(crate) const LEAF_BACKDATE: Duration = BACKDATE;
pub(crate) const LEAF_MAX_VALIDITY: Duration = Duration::days(LEAF_MAX_VALIDITY_DAYS);
const DEFAULT_CACHE_CAPACITY: NonZeroUsize = match NonZeroUsize::new(LEAF_CACHE_CAPACITY) {
    Some(n) => n,
    None => panic!("LEAF_CACHE_CAPACITY must be non-zero"),
};

/// A loaded or generated CA that mints (and caches) leaf certificates. `Send + Sync`; share
/// it behind an [`Arc`].
pub struct CertificateAuthority {
    cert_pem: String,
    cert_der: CertificateDer<'static>,
    details: CaDetails,
    leaves: LeafIssuer,
    cache: LeafCache,
}

impl CertificateAuthority {
    /// Load `ca.pem` + `ca.key` from `dir`, or generate & persist them (key file mode 0600) if
    /// missing or unparseable.
    ///
    /// Also regenerates when the key does not match the certificate or the certificate is
    /// not yet valid. A CA that expires soon, or has expired, is kept: devices trust it, so
    /// only the user replaces it ([`CertificateAuthority::regenerate`]); [`CaInfo::expiry`]
    /// reports its state. Replaced files are kept as `ca.pem.bak` / `ca.key.bak`. Files are
    /// written atomically; `dir` is created (mode 0700 on Unix) if needed. I/O errors other
    /// than "file not found" (e.g. permission denied) are returned as [`CaError::Io`] rather
    /// than triggering regeneration, so a transient problem never replaces a trusted CA.
    pub fn load_or_generate(dir: &Path) -> Result<Self> {
        storage::load_or_generate(dir)
    }

    /// Generate a new CA in `dir` (on explicit user request: `POST /api/ca/regenerate`,
    /// `akev ca regenerate`). The new CA is created in memory first; the current files are
    /// then kept as `ca.pem.bak` / `ca.key.bak` and put back if writing the new ones fails.
    pub fn regenerate(dir: &Path) -> Result<Self> {
        storage::regenerate(dir)
    }

    /// Persist this CA as `ca.pem` (issuing certificate followed by any extra chain
    /// certificates) and `ca.key` (PKCS#8, mode 0600) in `dir`, atomically, moving existing
    /// files to `*.bak`. Used to make an imported ([`from_pem`](Self::from_pem)) CA permanent.
    pub fn save(&self, dir: &Path) -> Result<()> {
        storage::save(self, dir)
    }

    /// Generate a fresh in-memory CA (tests, ephemeral sessions).
    pub fn generate() -> Result<Self> {
        let now = OffsetDateTime::now_utc();
        Self::generate_with_validity(now - BACKDATE, now + CA_VALIDITY)
    }

    /// Load from PEM strings (cert + PKCS#8/SEC1 key) — lets users bring their own CA.
    ///
    /// The key may also be a PKCS#1 RSA key; supported algorithms are ECDSA P-256/P-384, RSA
    /// and Ed25519. `cert_pem` may hold a chain: the first certificate is the issuer, the
    /// rest are sent after it in every TLS handshake. Fails with [`CaError::Invalid`] when the
    /// certificate is not a CA (cA=false or no keyCertSign usage), has expired, does not match
    /// the key, or uses a subject name that cannot be reproduced exactly in issued leaves
    /// (repeated attribute types or multi-valued RDNs); encrypted keys are not supported.
    pub fn from_pem(cert_pem: &str, key_pem: &str) -> Result<Self> {
        let chain = CertificateDer::pem_slice_iter(cert_pem.as_bytes())
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CaError::Invalid(format!("cannot parse certificate PEM: {e}")))?;
        if chain.is_empty() {
            return Err(CaError::Invalid(
                "no CERTIFICATE block found in the certificate PEM".into(),
            ));
        }
        let key = parse_private_key(key_pem)?;
        Self::from_parts(chain, key, false)
    }

    /// [`CertificateAuthority::from_pem`] for the CA files in the data directory, which may
    /// have expired: the user decides when to replace a CA their devices trust.
    pub(crate) fn from_pem_allow_expired(cert_pem: &str, key_pem: &str) -> Result<Self> {
        let chain = CertificateDer::pem_slice_iter(cert_pem.as_bytes())
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CaError::Invalid(format!("cannot parse certificate PEM: {e}")))?;
        if chain.is_empty() {
            return Err(CaError::Invalid(
                "no CERTIFICATE block found in the certificate PEM".into(),
            ));
        }
        let key = parse_private_key(key_pem)?;
        Self::from_parts(chain, key, true)
    }

    /// CA certificate, PEM.
    pub fn cert_pem(&self) -> &str {
        &self.cert_pem
    }

    /// CA certificate, DER.
    pub fn cert_der(&self) -> &[u8] {
        &self.cert_der
    }

    /// CA private key, PKCS#8 PEM.
    pub fn key_pem(&self) -> String {
        pem_encode("PRIVATE KEY", self.leaves.issuer_key().serialized_der())
    }

    /// Fingerprints, subject hash and validity of the CA certificate.
    pub fn info(&self) -> CaInfo {
        self.details.to_info(OffsetDateTime::now_utc())
    }

    /// Get a leaf certificate+key for `host` (DNS name or IP literal). Cached (LRU, 2048
    /// entries). Thread-safe; concurrent calls for the same host may both generate but only
    /// one is cached.
    ///
    /// In practice generation is single-flight: concurrent calls for the same uncached host
    /// wait for one generation and all receive the same certificate. `host` is normalised
    /// (case, trailing dot, IDNA, `[IPv6]` brackets and zone ids) so every spelling shares one
    /// cache entry. Returns [`CaError::Invalid`] for strings that are not a DNS name or IP
    /// literal (e.g. `host:port`).
    pub fn leaf_for(&self, host: &str) -> Result<Arc<CertifiedKey>> {
        let name = LeafName::parse(host)?;
        self.cache.get_or_issue(&name, &self.leaves)
    }

    /// A server certificate valid for several names at once (DNS names and IP literals), e.g.
    /// the API's own certificate when it is reachable from the network: its bind address,
    /// `localhost`, the machine's hostname and LAN addresses. Not cached; the first valid
    /// name is the common name. Names that are neither a DNS name nor an IP literal are
    /// skipped; fails with [`CaError::Invalid`] when none is left.
    pub fn server_certificate(&self, names: &[String]) -> Result<Arc<CertifiedKey>> {
        let parsed: Vec<LeafName<'_>> = names
            .iter()
            .filter_map(|n| LeafName::parse(n).ok())
            .collect();
        let first = parsed
            .first()
            .ok_or_else(|| CaError::Invalid("no valid name for the certificate".into()))?;
        let mut sans = Vec::with_capacity(parsed.len());
        for name in &parsed {
            // The exact identity only (no `*.parent` sibling).
            if let Some(san) = name.subject_alt_names()?.into_iter().next()
                && !sans.contains(&san)
            {
                sans.push(san);
            }
        }
        let leaf = self
            .leaves
            .issue_for(first.common_name(), sans, OffsetDateTime::now_utc())?;
        Ok(leaf.certified)
    }

    /// Number of cached leaf certificates.
    pub fn cached_leaf_count(&self) -> usize {
        self.cache.len()
    }

    /// Issuing CA certificate followed by any extra chain certificates, PEM: what `ca.pem`
    /// holds, and what clients should trust.
    pub fn chain_pem(&self) -> String {
        self.leaves
            .chain_tail()
            .iter()
            .map(|der| pem_encode("CERTIFICATE", der))
            .collect()
    }

    pub(crate) fn generate_with_validity(
        not_before: OffsetDateTime,
        not_after: OffsetDateTime,
    ) -> Result<Self> {
        let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
            .map_err(|e| CaError::Generate(format!("CA key generation failed: {e}")))?;
        let mut params = CertificateParams::default();
        params.not_before = not_before;
        params.not_after = not_after;
        params.serial_number = Some(random_serial()?);
        let mut dn = DistinguishedName::new();
        dn.push(DnType::OrganizationName, akev_core::PRODUCT_NAME);
        dn.push(DnType::OrganizationalUnitName, install_id()?);
        dn.push(
            DnType::CommonName,
            format!("{} CA", akev_core::PRODUCT_NAME),
        );
        params.distinguished_name = dn;
        params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
            KeyUsagePurpose::DigitalSignature,
        ];
        params.use_authority_key_identifier_extension = true;
        let cert = params.self_signed(&key).map_err(|e| {
            CaError::Generate(format!("self-signing the CA certificate failed: {e}"))
        })?;
        Self::from_parts(vec![cert.into()], key, false)
    }

    /// Validate a CA certificate (chain head) + key and build the authority around them.
    /// `allow_expired` keeps an expired CA usable (leaves can't be minted from it, see
    /// [`CaExpiry::Expired`]).
    fn from_parts(
        chain: Vec<CertificateDer<'static>>,
        key: KeyPair,
        allow_expired: bool,
    ) -> Result<Self> {
        let ca_der = chain
            .first()
            .cloned()
            .ok_or_else(|| CaError::Invalid("no CA certificate".into()))?;
        let (rest, cert) = x509_parser::parse_x509_certificate(&ca_der)
            .map_err(|e| CaError::Invalid(format!("cannot parse the CA certificate: {e}")))?;
        if !rest.is_empty() {
            return Err(CaError::Invalid(
                "trailing data after the CA certificate".into(),
            ));
        }
        validate_ca_certificate(&cert)?;
        if cert
            .tbs_certificate
            .subject_pki
            .subject_public_key
            .data
            .as_ref()
            != key.public_key_raw()
        {
            return Err(CaError::Invalid(
                "the private key does not match the CA certificate".into(),
            ));
        }
        let details = CaDetails::from_cert(&cert, &ca_der);
        let expired = details.not_after <= OffsetDateTime::now_utc();
        if expired && !allow_expired {
            return Err(CaError::Invalid(format!(
                "the CA certificate expired at {}",
                info::rfc3339(details.not_after)
            )));
        }

        let issuer = Issuer::from_ca_cert_der(&ca_der, key).map_err(|e| {
            CaError::Invalid(format!("cannot use this certificate as an issuer: {e}"))
        })?;
        let leaves = LeafIssuer::new(issuer, chain, details.not_before, details.not_after)?;
        if !expired {
            verify_issuance(&leaves, &cert)?;
        }

        Ok(CertificateAuthority {
            cert_pem: pem_encode("CERTIFICATE", &ca_der),
            cert_der: ca_der.clone(),
            details,
            leaves,
            cache: LeafCache::new(DEFAULT_CACHE_CAPACITY),
        })
    }
}

impl fmt::Debug for CertificateAuthority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CertificateAuthority")
            .field("subject", &self.details.subject)
            .field("sha256_fingerprint", &self.details.sha256_fingerprint)
            .field("not_after", &info::rfc3339(self.details.not_after))
            .field("cached_leaves", &self.cached_leaf_count())
            .finish_non_exhaustive()
    }
}

/// Reject certificates that clients would never accept as the issuer of our leaves.
fn validate_ca_certificate(cert: &X509Certificate<'_>) -> Result<()> {
    match cert.basic_constraints() {
        Ok(Some(bc)) if !bc.value.ca => {
            return Err(CaError::Invalid(
                "not a CA certificate (basicConstraints cA=false)".into(),
            ));
        }
        Ok(Some(_)) => {}
        Ok(None) => tracing::warn!(
            subject = %cert.subject(),
            "CA certificate has no basicConstraints extension; strict clients may reject its leaves"
        ),
        Err(e) => return Err(CaError::Invalid(format!("malformed basicConstraints: {e}"))),
    }
    match cert.key_usage() {
        Ok(Some(ku)) if !ku.value.key_cert_sign() => Err(CaError::Invalid(
            "the certificate's keyUsage does not allow signing certificates (keyCertSign)".into(),
        )),
        Ok(_) => Ok(()),
        Err(e) => Err(CaError::Invalid(format!("malformed keyUsage: {e}"))),
    }
}

/// Mint a throw-away leaf and check that it chains to the CA byte-for-byte: rcgen re-encodes
/// the issuer name from a parsed form that cannot represent every legal DN (repeated
/// attribute types such as `DC=example, DC=com`, multi-valued RDNs), and a mismatch would
/// produce leaves no client can chain. Also proves the key can actually sign.
fn verify_issuance(leaves: &LeafIssuer, ca: &X509Certificate<'_>) -> Result<()> {
    let probe = LeafName::parse("probe.akev.invalid")?;
    let leaf = leaves.issue(&probe, OffsetDateTime::now_utc())?;
    let der = leaf
        .certified
        .end_entity_cert()
        .map_err(|e| CaError::Generate(format!("probe leaf has no certificate: {e}")))?;
    let (_, parsed) = x509_parser::parse_x509_certificate(der)
        .map_err(|e| CaError::Generate(format!("cannot parse probe leaf certificate: {e}")))?;
    if parsed.issuer().as_raw() != ca.subject().as_raw() {
        return Err(CaError::Invalid(format!(
            "the CA subject \"{}\" cannot be reproduced exactly in issued certificates \
             (repeated attribute types or multi-valued RDNs are not supported)",
            ca.subject()
        )));
    }
    parsed.verify_signature(Some(ca.public_key())).map_err(|e| {
        CaError::Invalid(format!(
            "certificates signed with this key do not verify: {e}"
        ))
    })
}

/// Parse a PEM private key (PKCS#8, SEC1 or PKCS#1) into an rcgen key pair.
fn parse_private_key(key_pem: &str) -> Result<KeyPair> {
    let parsed = PrivateKeyDer::from_pem_slice(key_pem.as_bytes()).map_err(|e| {
        if key_pem.contains("ENCRYPTED") {
            CaError::Invalid(
                "encrypted private keys are not supported; decrypt it first \
                 (`openssl pkey -in ca.key -out ca-decrypted.key`)"
                    .into(),
            )
        } else {
            CaError::Invalid(format!("cannot parse private key PEM: {e}"))
        }
    })?;
    let pkcs8 = match &parsed {
        PrivateKeyDer::Pkcs8(key) => key.secret_pkcs8_der().to_vec(),
        PrivateKeyDer::Pkcs1(key) => der::pkcs1_to_pkcs8(key.secret_pkcs1_der()),
        PrivateKeyDer::Sec1(key) => {
            der::sec1_to_pkcs8(key.secret_sec1_der()).map_err(CaError::Invalid)?
        }
        _ => return Err(CaError::Invalid("unsupported private key encoding".into())),
    };
    KeyPair::try_from(&PrivateKeyDer::Pkcs8(pkcs8.into())).map_err(|e| {
        CaError::Invalid(format!(
            "unsupported or invalid private key ({e}); supported: ECDSA P-256/P-384, RSA, Ed25519"
        ))
    })
}

/// PEM-encode `der` with 64-column base64 lines and LF line endings.
fn pem_encode(label: &str, der: &[u8]) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(der);
    let mut out = String::with_capacity(b64.len() + b64.len() / 64 + 2 * label.len() + 32);
    out.push_str("-----BEGIN ");
    out.push_str(label);
    out.push_str("-----\n");
    for line in b64.as_bytes().chunks(64) {
        out.extend(line.iter().map(|&b| char::from(b)));
        out.push('\n');
    }
    out.push_str("-----END ");
    out.push_str(label);
    out.push_str("-----\n");
    out
}

fn fill_random(buf: &mut [u8]) -> Result<()> {
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), buf)
        .map_err(|_| CaError::Generate("system random number generator failed".into()))
}

/// A random, positive serial of exactly 16 bytes (126 bits of entropy). Unique serials matter:
/// Firefox rejects two different certificates with the same issuer + serial
/// (`SEC_ERROR_REUSED_ISSUER_AND_SERIAL`), and every leaf shares one key, so rcgen's default
/// key-derived serial would collide.
pub(crate) fn random_serial() -> Result<SerialNumber> {
    let mut bytes = [0u8; 16];
    fill_random(&mut bytes)?;
    bytes[0] = (bytes[0] & 0x7f) | 0x40;
    Ok(SerialNumber::from_slice(&bytes))
}

/// Short random id used as the CA's OU (8 lowercase hex characters).
fn install_id() -> Result<String> {
    let mut bytes = [0u8; 4];
    fill_random(&mut bytes)?;
    Ok(format!("{:08x}", u32::from_be_bytes(bytes)))
}
