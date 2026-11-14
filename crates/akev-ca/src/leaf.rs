//! Leaf certificate minting and the single-flight LRU cache in front of it.

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use lru::LruCache;
use parking_lot::Mutex;
use rcgen::{
    CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};
use rustls::sign::{CertifiedKey, SigningKey};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use time::{Duration, OffsetDateTime};

use crate::host::LeafName;
use crate::{CaError, LEAF_BACKDATE, LEAF_MAX_VALIDITY, Result, random_serial};

/// Regenerate a cached leaf this long before it expires (only matters for leaves that are
/// not capped by the CA's own expiry, i.e. processes running for more than a year).
const LEAF_REFRESH_MARGIN: Duration = Duration::hours(1);

/// Mints leaf certificates signed by the CA, all sharing one ECDSA P-256 key.
pub(crate) struct LeafIssuer {
    issuer: Issuer<'static, KeyPair>,
    /// Public half used as the subjectPublicKeyInfo of every leaf.
    leaf_key: KeyPair,
    /// The same key as a rustls signer, shared by every [`CertifiedKey`].
    leaf_signer: Arc<dyn SigningKey>,
    /// Certificates sent after the leaf: the issuing CA, then any extra chain certificates.
    chain_tail: Vec<CertificateDer<'static>>,
    ca_not_before: OffsetDateTime,
    ca_not_after: OffsetDateTime,
}

/// A freshly minted leaf.
pub(crate) struct IssuedLeaf {
    pub(crate) certified: Arc<CertifiedKey>,
    pub(crate) not_after: OffsetDateTime,
}

impl LeafIssuer {
    pub(crate) fn new(
        issuer: Issuer<'static, KeyPair>,
        chain_tail: Vec<CertificateDer<'static>>,
        ca_not_before: OffsetDateTime,
        ca_not_after: OffsetDateTime,
    ) -> Result<Self> {
        let leaf_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
            .map_err(|e| CaError::Generate(format!("leaf key generation failed: {e}")))?;
        let der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf_key.serialize_der()));
        let leaf_signer = rustls::crypto::ring::sign::any_supported_type(&der)
            .map_err(|e| CaError::Generate(format!("leaf key rejected by rustls: {e}")))?;
        Ok(LeafIssuer {
            issuer,
            leaf_key,
            leaf_signer,
            chain_tail,
            ca_not_before,
            ca_not_after,
        })
    }

    /// The CA's private key.
    pub(crate) fn issuer_key(&self) -> &KeyPair {
        self.issuer.key()
    }

    /// The certificates sent after each leaf (issuing CA first).
    pub(crate) fn chain_tail(&self) -> &[CertificateDer<'static>] {
        &self.chain_tail
    }

    /// Mint a leaf for `name`, valid from `now − 1 day` (but not before the CA) until
    /// `min(now + 397 days, CA not_after)`.
    pub(crate) fn issue(&self, name: &LeafName<'_>, now: OffsetDateTime) -> Result<IssuedLeaf> {
        self.issue_for(name.common_name(), name.subject_alt_names()?, now)
    }

    /// Mint a leaf with an explicit common name and subjectAltName list (validity as for
    /// [`LeafIssuer::issue`]).
    pub(crate) fn issue_for(
        &self,
        common_name: Option<String>,
        subject_alt_names: Vec<rcgen::SanType>,
        now: OffsetDateTime,
    ) -> Result<IssuedLeaf> {
        let not_before = (now - LEAF_BACKDATE).max(self.ca_not_before);
        let not_after = (now + LEAF_MAX_VALIDITY).min(self.ca_not_after);
        if not_after <= not_before {
            return Err(CaError::Generate(format!(
                "the CA certificate expired at {}; regenerate it",
                crate::info::rfc3339(self.ca_not_after)
            )));
        }

        let mut params = CertificateParams::default();
        params.not_before = not_before;
        params.not_after = not_after;
        params.serial_number = Some(random_serial()?);
        let mut dn = DistinguishedName::new();
        if let Some(cn) = common_name {
            dn.push(DnType::CommonName, cn);
        }
        params.distinguished_name = dn;
        params.subject_alt_names = subject_alt_names;
        params.is_ca = IsCa::ExplicitNoCa;
        // ECDSA leaf key: digitalSignature only (keyEncipherment is RSA key transport and is
        // forbidden for ECC subscriber keys by the CA/B Baseline Requirements).
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        params.use_authority_key_identifier_extension = true;

        let cert = params
            .signed_by(&self.leaf_key, &self.issuer)
            .map_err(|e| CaError::Generate(format!("signing leaf certificate failed: {e}")))?;

        let mut chain = Vec::with_capacity(1 + self.chain_tail.len());
        chain.push(CertificateDer::from(cert));
        chain.extend(self.chain_tail.iter().cloned());
        Ok(IssuedLeaf {
            certified: Arc::new(CertifiedKey::new(chain, Arc::clone(&self.leaf_signer))),
            not_after,
        })
    }
}

/// Outcome shared by every caller waiting on one generation. Errors are stringified because
/// [`CaError`] is not `Clone`.
type Flight = OnceLock<std::result::Result<Arc<CertifiedKey>, String>>;

struct CachedLeaf {
    certified: Arc<CertifiedKey>,
    /// Unix time after which the entry is regenerated; `None` when the leaf is capped by the
    /// CA expiry (regenerating could not extend it).
    refresh_at: Option<i64>,
}

struct CacheState {
    lru: LruCache<Box<str>, CachedLeaf>,
    in_flight: HashMap<Box<str>, Arc<Flight>>,
}

/// LRU of minted leaves with single-flight generation: concurrent misses for the same host
/// wait for one generation instead of each minting a certificate.
pub(crate) struct LeafCache {
    state: Mutex<CacheState>,
    /// Number of leaves minted through the cache (diagnostics & tests).
    issued: AtomicU64,
}

impl LeafCache {
    pub(crate) fn new(capacity: NonZeroUsize) -> Self {
        LeafCache {
            state: Mutex::new(CacheState {
                lru: LruCache::new(capacity),
                in_flight: HashMap::new(),
            }),
            issued: AtomicU64::new(0),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.state.lock().lru.len()
    }

    #[cfg(test)]
    pub(crate) fn issued(&self) -> u64 {
        self.issued.load(Ordering::Relaxed)
    }

    /// Return the cached leaf for `name` or mint one with `issuer`.
    pub(crate) fn get_or_issue(
        &self,
        name: &LeafName<'_>,
        issuer: &LeafIssuer,
    ) -> Result<Arc<CertifiedKey>> {
        self.get_or_issue_at(name, issuer, OffsetDateTime::now_utc())
    }

    /// [`get_or_issue`](Self::get_or_issue) with an explicit clock.
    pub(crate) fn get_or_issue_at(
        &self,
        name: &LeafName<'_>,
        issuer: &LeafIssuer,
        now: OffsetDateTime,
    ) -> Result<Arc<CertifiedKey>> {
        let key = name.cache_key();
        let flight = {
            let mut state = self.state.lock();
            if let Some(entry) = state.lru.get(&*key) {
                let fresh = entry.refresh_at.is_none_or(|at| now.unix_timestamp() < at);
                if fresh {
                    return Ok(Arc::clone(&entry.certified));
                }
                state.lru.pop(&*key);
            }
            Arc::clone(state.in_flight.entry(Box::from(&*key)).or_default())
        };

        // Exactly one caller runs the closure; everyone else holding this flight blocks until
        // it finishes and then shares its result. The state lock is never held while waiting
        // on a flight, so the closure may take it.
        let outcome = flight.get_or_init(|| {
            let issued = issuer.issue(name, now);
            self.issued.fetch_add(1, Ordering::Relaxed);
            let mut state = self.state.lock();
            state.in_flight.remove(&*key);
            match issued {
                Ok(leaf) => {
                    let refresh_at = (leaf.not_after < issuer.ca_not_after)
                        .then(|| (leaf.not_after - LEAF_REFRESH_MARGIN).unix_timestamp());
                    state.lru.put(
                        Box::from(&*key),
                        CachedLeaf {
                            certified: Arc::clone(&leaf.certified),
                            refresh_at,
                        },
                    );
                    Ok(leaf.certified)
                }
                // Not cached: the next request retries with a fresh flight.
                Err(e) => Err(e.detail()),
            }
        });
        outcome.clone().map_err(CaError::Generate)
    }
}
