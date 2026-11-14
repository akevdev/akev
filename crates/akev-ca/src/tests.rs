//! Unit tests that need crate-private access (cache internals, custom validity windows).
//! Handshake- and OpenSSL-level checks live in `tests/`.

use std::collections::HashSet;
use std::fs;
use std::net::IpAddr;
use std::path::Path;
use std::sync::{Arc, Barrier};
use std::time::Instant;

use rustls::RootCertStore;
use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::ServerCertVerifier;
use rustls_pki_types::{ServerName, UnixTime};
use time::format_description::well_known::Rfc3339;
use x509_parser::extensions::{GeneralName, ParsedExtension};
use x509_parser::oid_registry::{
    OID_EC_P256, OID_KEY_TYPE_EC_PUBLIC_KEY, OID_SIG_ECDSA_WITH_SHA256,
};
use x509_parser::prelude::X509Certificate;

use super::*;

/// Throw-away CAs generated once with OpenSSL (valid until 2126), e.g.:
///
/// ```text
/// openssl genrsa -traditional -out rsa-ca.pkcs1.key 2048            # PKCS#1
/// openssl ecparam -genkey -name prime256v1 -noout -out ec-ca.sec1.key # SEC1
/// openssl ec -in ec-ca.sec1.key -no_public -out ec-nopub.sec1.key     # SEC1 w/o public key
/// openssl req -x509 -new -key rsa-ca.pkcs1.key -days 36500 -sha256 \
///   -subj "/C=SI/O=Fixture Org/CN=Fixture RSA CA" \
///   -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign,cRLSign" \
///   -out rsa-ca.pem
/// ```
///
/// plus P-384, Ed25519, `DC=com/DC=example` (repeated attribute), CA:FALSE, no-keyCertSign,
/// encrypted-key and root → intermediate (pathlen 0) variants built the same way.
const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

fn fixture(name: &str) -> String {
    fs::read_to_string(Path::new(FIXTURES).join(name)).unwrap()
}

fn parse(der: &[u8]) -> X509Certificate<'_> {
    let (rest, cert) = x509_parser::parse_x509_certificate(der).unwrap();
    assert!(rest.is_empty());
    cert
}

fn first_cert_der(pem: &str) -> Vec<u8> {
    CertificateDer::pem_slice_iter(pem.as_bytes())
        .next()
        .unwrap()
        .unwrap()
        .to_vec()
}

fn ski(cert: &X509Certificate<'_>) -> Option<Vec<u8>> {
    cert.iter_extensions()
        .find_map(|ext| match ext.parsed_extension() {
            ParsedExtension::SubjectKeyIdentifier(id) => Some(id.0.to_vec()),
            _ => None,
        })
}

fn aki(cert: &X509Certificate<'_>) -> Option<Vec<u8>> {
    cert.iter_extensions()
        .find_map(|ext| match ext.parsed_extension() {
            ParsedExtension::AuthorityKeyIdentifier(aki) => {
                aki.key_identifier.as_ref().map(|id| id.0.to_vec())
            }
            _ => None,
        })
}

fn sans(cert: &X509Certificate<'_>) -> Vec<String> {
    let ext = cert
        .subject_alternative_name()
        .unwrap()
        .expect("SAN extension");
    ext.value
        .general_names
        .iter()
        .map(|name| match name {
            GeneralName::DNSName(dns) => format!("DNS:{dns}"),
            GeneralName::IPAddress(bytes) => match bytes.len() {
                4 => format!("IP:{}", IpAddr::from(<[u8; 4]>::try_from(*bytes).unwrap())),
                16 => format!("IP:{}", IpAddr::from(<[u8; 16]>::try_from(*bytes).unwrap())),
                n => panic!("bad IP SAN length {n}"),
            },
            other => panic!("unexpected SAN {other:?}"),
        })
        .collect()
}

fn common_names(cert: &X509Certificate<'_>) -> Vec<String> {
    cert.subject()
        .iter_common_name()
        .map(|a| a.as_str().unwrap().to_owned())
        .collect()
}

fn verify_leaf(
    root_der: &[u8],
    leaf: &CertifiedKey,
    name: &str,
) -> std::result::Result<(), rustls::Error> {
    let mut roots = RootCertStore::empty();
    roots.add(CertificateDer::from(root_der.to_vec())).unwrap();
    let verifier = WebPkiServerVerifier::builder_with_provider(
        Arc::new(roots),
        Arc::new(rustls::crypto::ring::default_provider()),
    )
    .build()
    .unwrap();
    let server_name = ServerName::try_from(name.to_owned()).unwrap();
    verifier
        .verify_server_cert(
            &leaf.cert[0],
            &leaf.cert[1..],
            &server_name,
            &[],
            UnixTime::now(),
        )
        .map(|_| ())
}

fn assert_close(actual: OffsetDateTime, expected: OffsetDateTime, what: &str) {
    let diff = (actual - expected).abs();
    assert!(
        diff <= Duration::seconds(120),
        "{what}: {actual} vs expected {expected}"
    );
}

fn with_capacity(mut ca: CertificateAuthority, capacity: usize) -> CertificateAuthority {
    ca.cache = LeafCache::new(NonZeroUsize::new(capacity).unwrap());
    ca
}

/// A self-signed CA certificate + key with an arbitrary validity window, bypassing the
/// constructor checks (which refuse expired CAs).
fn raw_ca_pem(not_before: OffsetDateTime, not_after: OffsetDateTime) -> (String, String) {
    let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let mut params = CertificateParams::default();
    params.not_before = not_before;
    params.not_after = not_after;
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let cert = params.self_signed(&key).unwrap();
    (cert.pem(), key.serialize_pem())
}

#[test]
fn certificate_authority_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<CertificateAuthority>();
}

// ---------------------------------------------------------------- CA profile

#[test]
fn generated_ca_profile() {
    let now = OffsetDateTime::now_utc();
    let ca = CertificateAuthority::generate().unwrap();
    let cert = parse(ca.cert_der());

    assert_eq!(common_names(&cert), ["Akev CA"]);
    let orgs: Vec<_> = cert
        .subject()
        .iter_organization()
        .map(|a| a.as_str().unwrap())
        .collect();
    assert_eq!(orgs, ["Akev"]);
    let ous: Vec<_> = cert
        .subject()
        .iter_organizational_unit()
        .map(|a| a.as_str().unwrap())
        .collect();
    assert_eq!(ous.len(), 1);
    assert_eq!(ous[0].len(), 8);
    assert!(
        ous[0]
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "{}",
        ous[0]
    );
    assert_eq!(
        cert.subject().iter_country().count(),
        0,
        "C must be omitted"
    );
    assert_eq!(
        cert.subject().as_raw(),
        cert.issuer().as_raw(),
        "self-signed"
    );

    let bc = cert.basic_constraints().unwrap().unwrap();
    assert!(bc.critical);
    assert!(bc.value.ca);
    assert_eq!(bc.value.path_len_constraint, Some(0));

    let ku = cert.key_usage().unwrap().unwrap();
    assert!(ku.critical);
    assert!(ku.value.key_cert_sign() && ku.value.crl_sign() && ku.value.digital_signature());
    assert!(!ku.value.key_encipherment() && !ku.value.non_repudiation());

    let ski = ski(&cert).expect("subjectKeyIdentifier");
    assert_eq!(ski.len(), 20);
    assert_eq!(aki(&cert).as_deref(), Some(&ski[..]));

    assert_close(
        cert.validity().not_before.to_datetime(),
        now - Duration::days(1),
        "CA not_before",
    );
    assert_close(
        cert.validity().not_after.to_datetime(),
        now + Duration::days(CA_VALIDITY_DAYS),
        "CA not_after (10 years)",
    );

    let serial = cert.raw_serial();
    assert_eq!(serial.len(), 16, "serial {serial:02x?}");
    assert_eq!(serial[0] & 0x80, 0, "serial must be positive");

    assert_eq!(
        cert.public_key().algorithm.algorithm,
        OID_KEY_TYPE_EC_PUBLIC_KEY
    );
    let curve = cert
        .public_key()
        .algorithm
        .parameters
        .as_ref()
        .unwrap()
        .as_oid()
        .unwrap();
    assert_eq!(curve, OID_EC_P256);
    assert_eq!(
        cert.signature_algorithm.algorithm,
        OID_SIG_ECDSA_WITH_SHA256
    );
    cert.verify_signature(None).unwrap();
}

#[test]
fn each_generated_ca_is_distinct() {
    let a = CertificateAuthority::generate().unwrap();
    let b = CertificateAuthority::generate().unwrap();
    assert_ne!(a.info().sha256_fingerprint, b.info().sha256_fingerprint);
    assert_ne!(
        a.info().subject,
        b.info().subject,
        "install id (OU) differs"
    );
    assert_ne!(a.info().subject_hash_old, b.info().subject_hash_old);
}

#[test]
fn info_fields() {
    let ca = CertificateAuthority::generate().unwrap();
    let info = ca.info();
    let cert = parse(ca.cert_der());

    let ou = cert
        .subject()
        .iter_organizational_unit()
        .next()
        .unwrap()
        .as_str()
        .unwrap();
    assert_eq!(info.subject, format!("O=Akev, OU={ou}, CN=Akev CA"));

    assert_eq!(info.sha256_fingerprint.len(), 32 * 3 - 1);
    assert_eq!(
        info.sha256_fingerprint,
        info::colon_hex(ring::digest::digest(&ring::digest::SHA256, ca.cert_der()).as_ref())
    );
    assert!(
        info.sha256_fingerprint
            .bytes()
            .all(|b| b == b':' || b.is_ascii_digit() || b.is_ascii_uppercase())
    );

    let spki = base64::engine::general_purpose::STANDARD
        .decode(&info.spki_fingerprint)
        .unwrap();
    assert_eq!(
        spki,
        ring::digest::digest(&ring::digest::SHA256, cert.public_key().raw).as_ref()
    );

    assert_eq!(info.subject_hash_old.len(), 8);
    assert!(
        info.subject_hash_old
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    );
    assert_eq!(
        info.subject_hash_old,
        info::subject_hash_old(cert.subject().as_raw())
    );

    let nb = OffsetDateTime::parse(&info.not_before, &Rfc3339).unwrap();
    let na = OffsetDateTime::parse(&info.not_after, &Rfc3339).unwrap();
    assert_eq!(nb, cert.validity().not_before.to_datetime());
    assert_eq!(na, cert.validity().not_after.to_datetime());
    assert!(
        (CA_VALIDITY_DAYS - 1..=CA_VALIDITY_DAYS).contains(&info.days_remaining),
        "{}",
        info.days_remaining
    );
    assert_eq!(info.expiry, CaExpiry::Valid);
    assert_eq!(info.expiry_warning("in Settings"), None);

    let json = serde_json::to_value(&info).unwrap();
    for key in [
        "subject",
        "sha256Fingerprint",
        "spkiFingerprint",
        "subjectHashOld",
        "notBefore",
        "notAfter",
        "daysRemaining",
    ] {
        assert!(json.get(key).is_some(), "missing {key} in {json}");
    }
}

#[test]
fn pem_round_trip_preserves_identity() {
    let ca = CertificateAuthority::generate().unwrap();
    assert!(ca.cert_pem().starts_with("-----BEGIN CERTIFICATE-----\n"));
    assert!(ca.cert_pem().ends_with("-----END CERTIFICATE-----\n"));
    assert_eq!(first_cert_der(ca.cert_pem()), ca.cert_der());
    let key = ca.key_pem();
    assert!(
        key.starts_with("-----BEGIN PRIVATE KEY-----\n"),
        "PKCS#8 expected"
    );

    let reloaded = CertificateAuthority::from_pem(ca.cert_pem(), &key).unwrap();
    assert_eq!(
        reloaded.info().sha256_fingerprint,
        ca.info().sha256_fingerprint
    );
    assert_eq!(reloaded.key_pem(), key);
    let leaf = reloaded.leaf_for("api.example.com").unwrap();
    verify_leaf(ca.cert_der(), &leaf, "api.example.com").unwrap();
}

#[test]
fn debug_output_does_not_leak_the_key() {
    let ca = CertificateAuthority::generate().unwrap();
    let debug = format!("{ca:?}");
    assert!(debug.contains("Akev CA"), "{debug}");
    assert!(!debug.contains("PRIVATE"), "{debug}");
}

// ---------------------------------------------------------------- leaf profile

#[test]
fn leaf_profile_for_dns_name() {
    let now = OffsetDateTime::now_utc();
    let ca = CertificateAuthority::generate().unwrap();
    let ca_cert = parse(ca.cert_der());
    let leaf = ca.leaf_for("api.example.com").unwrap();

    assert_eq!(leaf.cert.len(), 2, "chain = [leaf, CA]");
    assert_eq!(leaf.cert[1].as_ref(), ca.cert_der());
    leaf.keys_match().unwrap();

    let cert = parse(&leaf.cert[0]);
    assert_eq!(sans(&cert), ["DNS:api.example.com", "DNS:*.example.com"]);
    assert_eq!(common_names(&cert), ["api.example.com"]);
    assert_eq!(cert.issuer().as_raw(), ca_cert.subject().as_raw());

    let eku = cert.extended_key_usage().unwrap().unwrap();
    assert!(eku.value.server_auth);
    assert!(!eku.value.client_auth && !eku.value.any && eku.value.other.is_empty());

    let ku = cert.key_usage().unwrap().unwrap();
    assert!(ku.critical);
    assert!(ku.value.digital_signature());
    assert!(!ku.value.key_cert_sign() && !ku.value.key_encipherment());

    let bc = cert.basic_constraints().unwrap().unwrap();
    assert!(bc.critical && !bc.value.ca);

    assert_eq!(aki(&cert), ski(&ca_cert), "AKI = CA SKI");
    assert_eq!(
        cert.public_key().algorithm.algorithm,
        OID_KEY_TYPE_EC_PUBLIC_KEY
    );
    assert_eq!(
        cert.signature_algorithm.algorithm,
        OID_SIG_ECDSA_WITH_SHA256
    );
    cert.verify_signature(Some(ca_cert.public_key())).unwrap();

    assert_close(
        cert.validity().not_before.to_datetime(),
        now - Duration::days(1),
        "leaf not_before",
    );
    // The default CA lives 10 years; leaves stay short-lived (Apple's 398-day limit).
    assert_close(
        cert.validity().not_after.to_datetime(),
        now + Duration::days(LEAF_MAX_VALIDITY_DAYS),
        "leaf not_after",
    );
    assert!(cert.validity().not_after.to_datetime() < ca_cert.validity().not_after.to_datetime());
}

#[test]
fn leaf_sans_for_ip_literals() {
    let ca = CertificateAuthority::generate().unwrap();
    for (host, san, cn) in [
        ("127.0.0.1", "IP:127.0.0.1", "127.0.0.1"),
        ("10.0.0.254.", "IP:10.0.0.254", "10.0.0.254"),
        ("::1", "IP:::1", "::1"),
        ("[::1]", "IP:::1", "::1"),
        ("[2001:DB8::42]", "IP:2001:db8::42", "2001:db8::42"),
        ("fe80::1%eth0", "IP:fe80::1", "fe80::1"),
    ] {
        let leaf = ca.leaf_for(host).unwrap();
        let cert = parse(&leaf.cert[0]);
        assert_eq!(sans(&cert), [san], "{host}");
        assert_eq!(common_names(&cert), [cn], "{host}");
    }
    // `::1` and `[::1]` share one cache entry.
    assert_eq!(ca.cached_leaf_count(), 5);
}

#[test]
fn leaf_sans_for_idn() {
    let ca = CertificateAuthority::generate().unwrap();
    let leaf = ca.leaf_for("Bücher.example").unwrap();
    let cert = parse(&leaf.cert[0]);
    assert_eq!(sans(&cert), ["DNS:xn--bcher-kva.example"]);
    assert_eq!(common_names(&cert), ["xn--bcher-kva.example"]);
    let again = ca.leaf_for("xn--bcher-kva.example").unwrap();
    assert!(
        Arc::ptr_eq(&leaf, &again),
        "unicode and punycode spellings share a cache entry"
    );

    let leaf = ca.leaf_for("shop.bücher.example").unwrap();
    assert_eq!(
        sans(&parse(&leaf.cert[0])),
        [
            "DNS:shop.xn--bcher-kva.example",
            "DNS:*.xn--bcher-kva.example"
        ]
    );
}

#[test]
fn long_hosts_omit_the_common_name() {
    let ca = CertificateAuthority::generate().unwrap();
    let host = format!("{}.{}.example.com", "a".repeat(40), "b".repeat(40));
    let leaf = ca.leaf_for(&host).unwrap();
    let cert = parse(&leaf.cert[0]);
    assert!(common_names(&cert).is_empty());
    assert_eq!(sans(&cert)[0], format!("DNS:{host}"));
    // With an empty subject the SAN extension must be critical (RFC 5280 §4.2.1.6).
    let san = cert.subject_alternative_name().unwrap().unwrap();
    assert!(san.critical);
    verify_leaf(ca.cert_der(), &leaf, &host).unwrap();
}

#[test]
fn leaves_verify_with_webpki_for_the_right_names_only() {
    let ca = CertificateAuthority::generate().unwrap();
    for (host, name, ok) in [
        ("api.example.com", "api.example.com", true),
        ("api.example.com", "www.example.com", true), // wildcard sibling
        ("api.example.com", "example.com", false),
        ("api.example.com", "a.b.example.com", false), // wildcards cover one label
        ("api.example.com", "api.example.org", false),
        ("API.Example.COM.", "api.example.com", true),
        ("example.com", "example.com", true),
        ("example.com", "www.example.com", false),
        ("localhost", "localhost", true),
        ("my_svc.example.com", "my_svc.example.com", true),
        ("bücher.example", "xn--bcher-kva.example", true),
        ("127.0.0.1", "127.0.0.1", true),
        ("127.0.0.1", "127.0.0.2", false),
        ("::1", "::1", true),
        ("2001:db8::1", "2001:db8::2", false),
    ] {
        let leaf = ca.leaf_for(host).unwrap();
        let result = verify_leaf(ca.cert_der(), &leaf, name);
        assert_eq!(
            result.is_ok(),
            ok,
            "leaf for {host:?} verified as {name:?}: {result:?}"
        );
    }
}

#[test]
fn leaves_do_not_verify_against_another_ca() {
    let ca = CertificateAuthority::generate().unwrap();
    let other = CertificateAuthority::generate().unwrap();
    let leaf = ca.leaf_for("example.com").unwrap();
    let err = verify_leaf(other.cert_der(), &leaf, "example.com").unwrap_err();
    assert!(
        matches!(err, rustls::Error::InvalidCertificate(_)),
        "{err:?}"
    );
}

#[test]
fn leaf_validity_is_capped_at_397_days() {
    let now = OffsetDateTime::now_utc();
    let ca = CertificateAuthority::generate_with_validity(
        now - Duration::days(1),
        now + Duration::days(3650),
    )
    .unwrap();
    let leaf = ca.leaf_for("example.com").unwrap();
    let cert = parse(&leaf.cert[0]);
    assert_close(
        cert.validity().not_before.to_datetime(),
        now - Duration::days(1),
        "not_before",
    );
    assert_close(
        cert.validity().not_after.to_datetime(),
        now + Duration::days(397),
        "not_after",
    );
    let lifetime =
        cert.validity().not_after.to_datetime() - cert.validity().not_before.to_datetime();
    assert!(
        lifetime <= Duration::days(398),
        "Apple's 398-day limit: {lifetime}"
    );
}

#[test]
fn leaf_validity_never_exceeds_the_ca() {
    let now = OffsetDateTime::now_utc();
    let ca = CertificateAuthority::generate_with_validity(
        now - Duration::hours(1),
        now + Duration::days(10),
    )
    .unwrap();
    let ca_cert = parse(ca.cert_der());
    let leaf = ca.leaf_for("example.com").unwrap();
    let cert = parse(&leaf.cert[0]);
    assert_eq!(
        cert.validity().not_after.to_datetime(),
        ca_cert.validity().not_after.to_datetime()
    );
    assert_eq!(
        cert.validity().not_before.to_datetime(),
        ca_cert.validity().not_before.to_datetime()
    );
}

#[test]
fn issuing_after_ca_expiry_fails_cleanly() {
    let ca = CertificateAuthority::generate().unwrap();
    let name = LeafName::parse("example.com").unwrap();
    let later = OffsetDateTime::now_utc() + Duration::days(CA_VALIDITY_DAYS + 1);
    match ca.leaves.issue(&name, later) {
        Err(CaError::Generate(msg)) => assert!(msg.contains("expired"), "{msg}"),
        Err(other) => panic!("unexpected error {other}"),
        Ok(_) => panic!("issued a leaf after CA expiry"),
    }
}

#[test]
fn leaf_serials_are_unique_positive_16_bytes() {
    // Regression: rcgen's default serial is derived from the public key, and every leaf shares
    // one key — identical issuer+serial makes Firefox fail with SEC_ERROR_REUSED_ISSUER_AND_SERIAL.
    let ca = CertificateAuthority::generate().unwrap();
    let mut serials = HashSet::new();
    for i in 0..64 {
        let leaf = ca.leaf_for(&format!("host{i}.example.com")).unwrap();
        let cert = parse(&leaf.cert[0]);
        let serial = cert.raw_serial().to_vec();
        assert_eq!(serial.len(), 16);
        assert_eq!(serial[0] & 0x80, 0);
        assert!(serials.insert(serial), "duplicate serial");
    }
}

#[test]
fn all_leaves_share_one_key() {
    let ca = CertificateAuthority::generate().unwrap();
    let a = ca.leaf_for("a.example").unwrap();
    let b = ca.leaf_for("b.example").unwrap();
    assert!(Arc::ptr_eq(&a.key, &b.key));
    assert_eq!(
        parse(&a.cert[0]).public_key().raw,
        parse(&b.cert[0]).public_key().raw
    );
}

#[test]
fn invalid_hosts_are_rejected_without_caching() {
    let ca = CertificateAuthority::generate().unwrap();
    for bad in ["", "example.com:443", "*.example.com", "a b"] {
        assert!(
            matches!(ca.leaf_for(bad), Err(CaError::Invalid(_))),
            "{bad:?}"
        );
    }
    assert_eq!(ca.cached_leaf_count(), 0);
    assert_eq!(ca.cache.issued(), 0);
}

#[test]
fn leaf_generation_is_fast() {
    let ca = CertificateAuthority::generate().unwrap();
    ca.leaf_for("warmup.example").unwrap();
    let n = 200u32;
    let start = Instant::now();
    for i in 0..n {
        ca.leaf_for(&format!("perf{i}.example.com")).unwrap();
    }
    let per_leaf = start.elapsed() / n;
    assert!(
        per_leaf < std::time::Duration::from_millis(2),
        "{per_leaf:?} per leaf"
    );
}

// ---------------------------------------------------------------- cache

#[test]
fn cache_normalises_host_spellings() {
    let ca = CertificateAuthority::generate().unwrap();
    let a = ca.leaf_for("Example.COM").unwrap();
    let b = ca.leaf_for("example.com.").unwrap();
    let c = ca.leaf_for(" example.com ").unwrap();
    assert!(Arc::ptr_eq(&a, &b) && Arc::ptr_eq(&b, &c));
    assert_eq!(ca.cached_leaf_count(), 1);
    assert_eq!(ca.cache.issued(), 1);
}

#[test]
fn lru_evicts_least_recently_used() {
    let ca = with_capacity(CertificateAuthority::generate().unwrap(), 3);
    let a = ca.leaf_for("a.test").unwrap();
    let b = ca.leaf_for("b.test").unwrap();
    ca.leaf_for("c.test").unwrap();
    assert!(Arc::ptr_eq(&a, &ca.leaf_for("a.test").unwrap())); // promote a
    ca.leaf_for("d.test").unwrap(); // evicts b
    assert_eq!(ca.cached_leaf_count(), 3);
    assert_eq!(ca.cache.issued(), 4);

    assert!(
        Arc::ptr_eq(&a, &ca.leaf_for("a.test").unwrap()),
        "a survived"
    );
    assert_eq!(ca.cache.issued(), 4);
    let b2 = ca.leaf_for("b.test").unwrap();
    assert!(!Arc::ptr_eq(&b, &b2), "b was evicted and re-minted");
    assert_eq!(ca.cache.issued(), 5);
    assert_eq!(ca.cached_leaf_count(), 3);
}

#[test]
fn default_capacity_is_2048() {
    let ca = CertificateAuthority::generate().unwrap();
    for i in 0..=LEAF_CACHE_CAPACITY {
        ca.leaf_for(&format!("h{i}.test")).unwrap();
    }
    assert_eq!(ca.cached_leaf_count(), LEAF_CACHE_CAPACITY);
}

#[test]
fn single_flight_under_concurrency() {
    let ca = Arc::new(CertificateAuthority::generate().unwrap());
    let threads = 32;
    let barrier = Arc::new(Barrier::new(threads));
    let handles: Vec<_> = (0..threads)
        .map(|i| {
            let ca = Arc::clone(&ca);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                // Different spellings of the same host.
                let host = if i % 2 == 0 {
                    "burst.example.com"
                } else {
                    "BURST.example.com."
                };
                ca.leaf_for(host).unwrap()
            })
        })
        .collect();
    let leaves: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    for leaf in &leaves[1..] {
        assert!(Arc::ptr_eq(&leaves[0], leaf));
    }
    assert_eq!(ca.cache.issued(), 1, "exactly one generation for a burst");
    assert_eq!(ca.cached_leaf_count(), 1);
}

#[test]
fn concurrent_requests_for_many_hosts() {
    let ca = Arc::new(CertificateAuthority::generate().unwrap());
    let hosts: Vec<String> = (0..64).map(|i| format!("svc{i}.example.net")).collect();
    let threads = 8;
    let barrier = Arc::new(Barrier::new(threads));
    let handles: Vec<_> = (0..threads)
        .map(|t| {
            let ca = Arc::clone(&ca);
            let barrier = Arc::clone(&barrier);
            let mut order = hosts.clone();
            order.rotate_left(t * 8);
            std::thread::spawn(move || {
                barrier.wait();
                order
                    .iter()
                    .map(|h| (h.clone(), ca.leaf_for(h).unwrap()))
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let results: Vec<_> = handles
        .into_iter()
        .flat_map(|h| h.join().unwrap())
        .collect();
    assert_eq!(ca.cached_leaf_count(), 64);
    assert_eq!(ca.cache.issued(), 64);
    for (host, leaf) in results {
        assert!(Arc::ptr_eq(&leaf, &ca.leaf_for(&host).unwrap()));
    }
}

#[test]
fn stale_leaves_are_refreshed_only_when_it_helps() {
    let now = OffsetDateTime::now_utc();
    let name = LeafName::parse("example.com").unwrap();

    // Long-lived CA: the leaf (397 days) expires before the CA, so it is re-minted near expiry.
    let long = CertificateAuthority::generate_with_validity(
        now - Duration::days(1),
        now + Duration::days(3650),
    )
    .unwrap();
    let first = long
        .cache
        .get_or_issue_at(&name, &long.leaves, now)
        .unwrap();
    let same = long
        .cache
        .get_or_issue_at(&name, &long.leaves, now + Duration::days(396))
        .unwrap();
    assert!(Arc::ptr_eq(&first, &same));
    let near_expiry = now + Duration::days(397) - Duration::minutes(30);
    let renewed = long
        .cache
        .get_or_issue_at(&name, &long.leaves, near_expiry)
        .unwrap();
    assert!(!Arc::ptr_eq(&first, &renewed));
    assert_eq!(long.cache.issued(), 2);
    assert_eq!(long.cached_leaf_count(), 1);

    // Default CA: leaves are capped by the CA expiry, re-minting cannot help.
    let short = CertificateAuthority::generate().unwrap();
    let first = short
        .cache
        .get_or_issue_at(&name, &short.leaves, now)
        .unwrap();
    let later = short
        .cache
        .get_or_issue_at(&name, &short.leaves, now + Duration::days(364))
        .unwrap();
    assert!(Arc::ptr_eq(&first, &later));
    assert_eq!(short.cache.issued(), 1);
}

#[test]
fn failed_generation_is_not_cached() {
    let ca = CertificateAuthority::generate().unwrap();
    let name = LeafName::parse("example.com").unwrap();
    let later = OffsetDateTime::now_utc() + Duration::days(CA_VALIDITY_DAYS + 1);
    match ca.cache.get_or_issue_at(&name, &ca.leaves, later) {
        Err(CaError::Generate(msg)) => {
            assert!(msg.contains("expired"), "{msg}");
            assert!(
                !msg.contains("certificate generation failed"),
                "no doubled prefix: {msg}"
            );
        }
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(ca.cached_leaf_count(), 0);
    assert!(
        ca.cache
            .get_or_issue_at(&name, &ca.leaves, OffsetDateTime::now_utc())
            .is_ok()
    );
    assert_eq!(ca.cached_leaf_count(), 1);
}

// ---------------------------------------------------------------- bring your own CA

fn assert_byo_ca(cert_file: &str, key_file: &str, root_file: &str) -> CertificateAuthority {
    let ca = CertificateAuthority::from_pem(&fixture(cert_file), &fixture(key_file))
        .unwrap_or_else(|e| panic!("{cert_file} + {key_file}: {e}"));
    let root = first_cert_der(&fixture(root_file));
    for host in ["api.example.com", "www.example.com", "127.0.0.1"] {
        let leaf = ca.leaf_for(host).unwrap();
        verify_leaf(&root, &leaf, host).unwrap_or_else(|e| panic!("{cert_file}: {host}: {e:?}"));
        assert!(verify_leaf(&root, &leaf, "evil.test").is_err());
    }
    let key_pem = ca.key_pem();
    assert!(
        key_pem.starts_with("-----BEGIN PRIVATE KEY-----"),
        "normalised to PKCS#8"
    );
    let reloaded = CertificateAuthority::from_pem(ca.cert_pem(), &key_pem).unwrap();
    assert_eq!(
        reloaded.info().sha256_fingerprint,
        ca.info().sha256_fingerprint
    );
    ca
}

#[test]
fn byo_rsa_ca_with_pkcs1_key() {
    let ca = assert_byo_ca("rsa-ca.pem", "rsa-ca.pkcs1.key", "rsa-ca.pem");
    assert_eq!(ca.info().subject, "C=SI, O=Fixture Org, CN=Fixture RSA CA");
    let alg = parse(&ca.leaf_for("example.com").unwrap().cert[0])
        .signature_algorithm
        .algorithm
        .to_id_string();
    assert_eq!(alg, "1.2.840.113549.1.1.11", "sha256WithRSAEncryption");
}

#[test]
fn byo_rsa_ca_with_pkcs8_key() {
    let a = assert_byo_ca("rsa-ca.pem", "rsa-ca.pkcs8.key", "rsa-ca.pem");
    let b = CertificateAuthority::from_pem(&fixture("rsa-ca.pem"), &fixture("rsa-ca.pkcs1.key"))
        .unwrap();
    assert_eq!(
        a.key_pem(),
        b.key_pem(),
        "PKCS#1 is re-wrapped to the same PKCS#8"
    );
}

#[test]
fn byo_ec_ca_with_sec1_key() {
    assert_byo_ca("ec-ca.pem", "ec-ca.sec1.key", "ec-ca.pem");
}

#[test]
fn byo_p384_ca() {
    let ca = assert_byo_ca("p384-ca.pem", "p384-ca.key", "p384-ca.pem");
    let alg = parse(&ca.leaf_for("example.com").unwrap().cert[0])
        .signature_algorithm
        .algorithm
        .to_id_string();
    assert_eq!(alg, "1.2.840.10045.4.3.3", "ecdsa-with-SHA384");
}

#[test]
fn byo_ed25519_ca() {
    assert_byo_ca("ed25519-ca.pem", "ed25519-ca.key", "ed25519-ca.pem");
}

#[test]
fn byo_intermediate_ca_serves_the_chain() {
    let ca = assert_byo_ca(
        "intermediate-chain.pem",
        "intermediate-ca.key",
        "root-ca.pem",
    );
    let leaf = ca.leaf_for("example.com").unwrap();
    assert_eq!(leaf.cert.len(), 3, "[leaf, intermediate, root]");
    assert_eq!(
        ca.info().subject,
        "O=Fixture Org, CN=Fixture Intermediate CA"
    );
    assert_eq!(
        CertificateDer::pem_slice_iter(ca.cert_pem().as_bytes()).count(),
        1
    );
    assert_eq!(
        CertificateDer::pem_slice_iter(ca.chain_pem().as_bytes()).count(),
        2
    );
}

fn invalid(cert: &str, key: &str) -> String {
    match CertificateAuthority::from_pem(cert, key) {
        Err(CaError::Invalid(msg)) => msg,
        Err(other) => panic!("expected Invalid, got {other}"),
        Ok(_) => panic!("expected an error"),
    }
}

#[test]
fn byo_ca_rejections() {
    let ec_key = fixture("ec-ca.sec1.key");
    let ec_cert = fixture("ec-ca.pem");

    assert!(invalid("garbage", &ec_key).contains("CERTIFICATE"));
    let msg = invalid(
        "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n",
        &ec_key,
    );
    assert!(msg.contains("parse"), "{msg}");
    assert!(invalid(&ec_cert, "").contains("private key"));
    assert!(invalid(&ec_cert, &ec_cert).contains("private key"));
    assert!(invalid(&ec_cert, &fixture("encrypted.key")).contains("encrypted"));
    assert!(invalid(&ec_cert, &fixture("rsa-ca.pkcs1.key")).contains("does not match"));
    assert!(invalid(&fixture("not-ca.pem"), &fixture("not-ca.key")).contains("cA=false"));
    assert!(invalid(&fixture("nosign-ca.pem"), &fixture("nosign-ca.key")).contains("keyCertSign"));
    assert!(invalid(&ec_cert, &fixture("ec-nopub.sec1.key")).contains("public key"));
    // Repeated DC attributes cannot be re-encoded byte-for-byte by rcgen; we refuse instead of
    // minting leaves whose issuer does not match the CA subject.
    let msg = invalid(&fixture("dc-ca.pem"), &fixture("dc-ca.key"));
    assert!(msg.contains("cannot be reproduced"), "{msg}");

    let now = OffsetDateTime::now_utc();
    let (cert, key) = raw_ca_pem(now - Duration::days(30), now - Duration::days(1));
    assert!(invalid(&cert, &key).contains("expired"));
}

#[test]
fn combined_pem_file_works_for_both_arguments() {
    let ca = CertificateAuthority::generate().unwrap();
    let combined = format!("{}{}", ca.cert_pem(), ca.key_pem());
    let reloaded = CertificateAuthority::from_pem(&combined, &combined).unwrap();
    assert_eq!(
        reloaded.info().sha256_fingerprint,
        ca.info().sha256_fingerprint
    );
}

// ---------------------------------------------------------------- storage

fn list_dir(dir: &Path) -> Vec<String> {
    let mut names: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt as _;
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn generate_persist_and_reload() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let first = CertificateAuthority::load_or_generate(dir).unwrap();
    assert_eq!(
        list_dir(dir),
        ["ca.key", "ca.pem"],
        "no temp files or backups"
    );
    assert_eq!(
        fs::read_to_string(dir.join("ca.pem")).unwrap(),
        first.cert_pem()
    );
    assert_eq!(
        fs::read_to_string(dir.join("ca.key")).unwrap(),
        first.key_pem()
    );

    let second = CertificateAuthority::load_or_generate(dir).unwrap();
    assert_eq!(
        second.info().sha256_fingerprint,
        first.info().sha256_fingerprint
    );
    assert_eq!(list_dir(dir), ["ca.key", "ca.pem"]);
    // Leaves from the reloaded CA chain to the original certificate.
    verify_leaf(
        first.cert_der(),
        &second.leaf_for("example.com").unwrap(),
        "example.com",
    )
    .unwrap();
}

#[cfg(unix)]
#[test]
fn key_file_is_private_and_dir_is_created() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("nested").join("akev");
    CertificateAuthority::load_or_generate(&dir).unwrap();
    assert_eq!(mode(&dir.join("ca.key")), 0o600);
    assert_eq!(mode(&dir), 0o700);
    let cert_mode = mode(&dir.join("ca.pem"));
    assert_eq!(cert_mode & 0o600, 0o600, "certificate readable by owner");
}

#[cfg(unix)]
#[test]
fn loose_key_permissions_are_tightened_on_load() {
    use std::os::unix::fs::PermissionsExt as _;
    let tmp = tempfile::tempdir().unwrap();
    let ca = CertificateAuthority::load_or_generate(tmp.path()).unwrap();
    let key = tmp.path().join("ca.key");
    fs::set_permissions(&key, fs::Permissions::from_mode(0o644)).unwrap();
    let reloaded = CertificateAuthority::load_or_generate(tmp.path()).unwrap();
    assert_eq!(
        reloaded.info().sha256_fingerprint,
        ca.info().sha256_fingerprint
    );
    assert_eq!(mode(&key), 0o600);
}

fn assert_regenerated(dir: &Path, old_fingerprint: Option<&str>, old_cert: &[u8], old_key: &[u8]) {
    let ca = CertificateAuthority::load_or_generate(dir).unwrap();
    if let Some(old) = old_fingerprint {
        assert_ne!(ca.info().sha256_fingerprint, old, "a new CA was generated");
    }
    assert!(ca.info().days_remaining >= CA_VALIDITY_DAYS - 1);
    assert_eq!(fs::read(dir.join("ca.pem.bak")).unwrap(), old_cert);
    assert_eq!(fs::read(dir.join("ca.key.bak")).unwrap(), old_key);
    #[cfg(unix)]
    assert_eq!(mode(&dir.join("ca.key.bak")), 0o600);
    // And the new files are the ones on disk.
    let reloaded = CertificateAuthority::load_or_generate(dir).unwrap();
    assert_eq!(
        reloaded.info().sha256_fingerprint,
        ca.info().sha256_fingerprint
    );
}

#[test]
fn corrupt_certificate_is_regenerated_with_backup() {
    let tmp = tempfile::tempdir().unwrap();
    let ca = CertificateAuthority::load_or_generate(tmp.path()).unwrap();
    fs::write(tmp.path().join("ca.pem"), b"garbage").unwrap();
    assert_regenerated(
        tmp.path(),
        Some(&ca.info().sha256_fingerprint),
        b"garbage",
        ca.key_pem().as_bytes(),
    );
}

#[test]
fn corrupt_key_is_regenerated_with_backup() {
    let tmp = tempfile::tempdir().unwrap();
    let ca = CertificateAuthority::load_or_generate(tmp.path()).unwrap();
    let junk = b"\xff\xfe not utf-8";
    fs::write(tmp.path().join("ca.key"), junk).unwrap();
    assert_regenerated(
        tmp.path(),
        Some(&ca.info().sha256_fingerprint),
        ca.cert_pem().as_bytes(),
        junk,
    );
}

#[test]
fn missing_key_is_regenerated() {
    let tmp = tempfile::tempdir().unwrap();
    let ca = CertificateAuthority::load_or_generate(tmp.path()).unwrap();
    fs::remove_file(tmp.path().join("ca.key")).unwrap();
    let fresh = CertificateAuthority::load_or_generate(tmp.path()).unwrap();
    assert_ne!(
        fresh.info().sha256_fingerprint,
        ca.info().sha256_fingerprint
    );
    assert_eq!(
        fs::read_to_string(tmp.path().join("ca.pem.bak")).unwrap(),
        ca.cert_pem()
    );
    assert!(!tmp.path().join("ca.key.bak").exists());
}

#[test]
fn mismatched_key_is_regenerated() {
    let tmp = tempfile::tempdir().unwrap();
    let ca = CertificateAuthority::load_or_generate(tmp.path()).unwrap();
    let other = CertificateAuthority::generate().unwrap();
    fs::write(tmp.path().join("ca.key"), other.key_pem()).unwrap();
    assert_regenerated(
        tmp.path(),
        Some(&ca.info().sha256_fingerprint),
        ca.cert_pem().as_bytes(),
        other.key_pem().as_bytes(),
    );
}

#[test]
fn expiring_ca_is_kept_and_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let now = OffsetDateTime::now_utc();
    let expiring = CertificateAuthority::generate_with_validity(
        now - Duration::days(300),
        now + Duration::days(10),
    )
    .unwrap();
    expiring.save(tmp.path()).unwrap();
    assert!(storage::renewal_reason(&expiring, now).is_none());
    let loaded = CertificateAuthority::load_or_generate(tmp.path()).unwrap();
    assert_eq!(
        loaded.info().sha256_fingerprint,
        expiring.info().sha256_fingerprint,
        "devices trust it: never replaced automatically"
    );
    assert!(!tmp.path().join("ca.pem.bak").exists());
    let info = loaded.info();
    assert_eq!(info.expiry, CaExpiry::Expiring);
    let warning = info.expiry_warning("in Settings").unwrap();
    assert!(warning.contains("expires in 9 day(s)"), "{warning}");
    assert!(warning.contains("Regenerate it in Settings"), "{warning}");
    // It still mints leaves until it expires.
    loaded.leaf_for("example.com").unwrap();
}

#[test]
fn ca_with_more_than_60_days_left_is_valid() {
    let tmp = tempfile::tempdir().unwrap();
    let now = OffsetDateTime::now_utc();
    let ca = CertificateAuthority::generate_with_validity(
        now - Duration::days(300),
        now + Duration::days(EXPIRY_WARNING_DAYS + 1),
    )
    .unwrap();
    ca.save(tmp.path()).unwrap();
    let loaded = CertificateAuthority::load_or_generate(tmp.path()).unwrap();
    assert_eq!(
        loaded.info().sha256_fingerprint,
        ca.info().sha256_fingerprint
    );
    assert_eq!(loaded.info().expiry, CaExpiry::Valid);
    assert!(!tmp.path().join("ca.pem.bak").exists());
    let soon = CertificateAuthority::generate_with_validity(
        now - Duration::days(300),
        now + Duration::days(EXPIRY_WARNING_DAYS - 1),
    )
    .unwrap();
    assert_eq!(soon.info().expiry, CaExpiry::Expiring);
}

#[test]
fn expired_ca_is_kept_and_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let now = OffsetDateTime::now_utc();
    let (cert, key) = raw_ca_pem(now - Duration::days(400), now - Duration::days(2));
    fs::write(tmp.path().join("ca.pem"), &cert).unwrap();
    fs::write(tmp.path().join("ca.key"), &key).unwrap();
    let loaded = CertificateAuthority::load_or_generate(tmp.path()).unwrap();
    assert_eq!(
        list_dir(tmp.path()),
        ["ca.key", "ca.pem"],
        "nothing replaced"
    );
    assert_eq!(fs::read_to_string(tmp.path().join("ca.pem")).unwrap(), cert);
    let info = loaded.info();
    assert_eq!(info.expiry, CaExpiry::Expired);
    assert!(info.days_remaining < 0);
    let warning = info.expiry_warning("with `akev ca regenerate`").unwrap();
    assert!(warning.contains("expired on"), "{warning}");
    assert!(warning.contains("akev ca regenerate"), "{warning}");
    // Leaves can't be minted from it: the error says what to do.
    let err = match loaded.leaf_for("example.com") {
        Err(e) => e.to_string(),
        Ok(_) => panic!("an expired CA must not mint leaves"),
    };
    assert!(
        err.contains("expired") && err.contains("regenerate"),
        "{err}"
    );
    // Bring-your-own CAs must still be valid.
    assert!(CertificateAuthority::from_pem(&cert, &key).is_err());
    // The user regenerates it explicitly.
    let fresh = CertificateAuthority::regenerate(tmp.path()).unwrap();
    assert_eq!(fresh.info().expiry, CaExpiry::Valid);
    assert_eq!(
        fs::read_to_string(tmp.path().join("ca.pem.bak")).unwrap(),
        cert
    );
}

#[test]
fn not_yet_valid_ca_is_regenerated() {
    let tmp = tempfile::tempdir().unwrap();
    let now = OffsetDateTime::now_utc();
    let future = CertificateAuthority::generate_with_validity(
        now + Duration::days(2),
        now + Duration::days(400),
    )
    .unwrap();
    future.save(tmp.path()).unwrap();
    assert_regenerated(
        tmp.path(),
        Some(&future.info().sha256_fingerprint),
        future.cert_pem().as_bytes(),
        future.key_pem().as_bytes(),
    );
}

#[cfg(unix)]
#[test]
fn unreadable_key_is_an_error_not_a_regeneration() {
    use std::os::unix::fs::PermissionsExt as _;
    let tmp = tempfile::tempdir().unwrap();
    let ca = CertificateAuthority::load_or_generate(tmp.path()).unwrap();
    let key = tmp.path().join("ca.key");
    fs::set_permissions(&key, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read(&key).is_ok() {
        // Running as root: permissions are not enforced, nothing to test.
        fs::set_permissions(&key, fs::Permissions::from_mode(0o600)).unwrap();
        return;
    }
    let err = CertificateAuthority::load_or_generate(tmp.path()).unwrap_err();
    assert!(
        matches!(err, CaError::Io(ref e) if e.kind() == std::io::ErrorKind::PermissionDenied),
        "{err}"
    );
    fs::set_permissions(&key, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        list_dir(tmp.path()),
        ["ca.key", "ca.pem"],
        "nothing was replaced"
    );
    let reloaded = CertificateAuthority::load_or_generate(tmp.path()).unwrap();
    assert_eq!(
        reloaded.info().sha256_fingerprint,
        ca.info().sha256_fingerprint
    );
}

#[test]
fn regenerate_replaces_and_backs_up() {
    let tmp = tempfile::tempdir().unwrap();
    let ca = CertificateAuthority::load_or_generate(tmp.path()).unwrap();
    let fresh = CertificateAuthority::regenerate(tmp.path()).unwrap();
    assert_ne!(
        fresh.info().sha256_fingerprint,
        ca.info().sha256_fingerprint
    );
    assert_eq!(
        fs::read_to_string(tmp.path().join("ca.pem.bak")).unwrap(),
        ca.cert_pem()
    );
    assert_eq!(
        fs::read_to_string(tmp.path().join("ca.key.bak")).unwrap(),
        ca.key_pem()
    );
    let loaded = CertificateAuthority::load_or_generate(tmp.path()).unwrap();
    assert_eq!(
        loaded.info().sha256_fingerprint,
        fresh.info().sha256_fingerprint
    );

    // Regenerating into an empty directory works too.
    let empty = tempfile::tempdir().unwrap();
    CertificateAuthority::regenerate(empty.path()).unwrap();
    assert_eq!(list_dir(empty.path()), ["ca.key", "ca.pem"]);
}

#[test]
fn imported_ca_can_be_saved_and_reloaded() {
    let tmp = tempfile::tempdir().unwrap();
    let rsa = CertificateAuthority::from_pem(&fixture("rsa-ca.pem"), &fixture("rsa-ca.pkcs1.key"))
        .unwrap();
    rsa.save(tmp.path()).unwrap();
    assert!(
        fs::read_to_string(tmp.path().join("ca.key"))
            .unwrap()
            .starts_with("-----BEGIN PRIVATE KEY-----")
    );
    #[cfg(unix)]
    assert_eq!(mode(&tmp.path().join("ca.key")), 0o600);
    let loaded = CertificateAuthority::load_or_generate(tmp.path()).unwrap();
    assert_eq!(
        loaded.info().sha256_fingerprint,
        rsa.info().sha256_fingerprint
    );

    // Chains survive a save/load round trip.
    let chained = tempfile::tempdir().unwrap();
    let int = CertificateAuthority::from_pem(
        &fixture("intermediate-chain.pem"),
        &fixture("intermediate-ca.key"),
    )
    .unwrap();
    int.save(chained.path()).unwrap();
    let loaded = CertificateAuthority::load_or_generate(chained.path()).unwrap();
    assert_eq!(loaded.leaf_for("example.com").unwrap().cert.len(), 3);
}

#[test]
fn atomic_write_replaces_content_and_cleans_up() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("file.txt");
    storage::write_atomic(&path, b"one", false).unwrap();
    storage::write_atomic(&path, b"two", true).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"two");
    assert_eq!(list_dir(tmp.path()), ["file.txt"]);
    #[cfg(unix)]
    assert_eq!(mode(&path), 0o600);

    // Failure (target is a non-empty directory) leaves no temp file behind.
    let blocker = tmp.path().join("blocker");
    fs::create_dir(&blocker).unwrap();
    fs::write(blocker.join("x"), b"x").unwrap();
    assert!(storage::write_atomic(&blocker, b"data", false).is_err());
    assert_eq!(list_dir(tmp.path()), ["blocker", "file.txt"]);
}

#[test]
fn server_certificate_covers_every_name() {
    let ca = CertificateAuthority::generate().unwrap();
    let names = [
        "192.168.1.23".to_string(),
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "::1".to_string(),
        "my-laptop".to_string(),
        "localhost".to_string(),
        "not a name".to_string(),
    ];
    let key = ca.server_certificate(&names).unwrap();
    let cert = parse(&key.cert[0]);
    assert_eq!(common_names(&cert), ["192.168.1.23"]);
    let mut sans = Vec::new();
    for ext in cert.extensions() {
        if let ParsedExtension::SubjectAlternativeName(san) = ext.parsed_extension() {
            for name in &san.general_names {
                sans.push(match name {
                    GeneralName::DNSName(d) => format!("DNS:{d}"),
                    GeneralName::IPAddress(ip) => format!("IP:{}", ip.len()),
                    other => format!("{other:?}"),
                });
            }
        }
    }
    assert_eq!(
        sans,
        ["IP:4", "DNS:localhost", "IP:4", "IP:16", "DNS:my-laptop"],
        "every valid name once, no wildcard siblings"
    );
    // Chains to the CA and verifies for each name.
    let mut roots = RootCertStore::empty();
    roots
        .add(CertificateDer::from(ca.cert_der().to_vec()))
        .unwrap();
    let verifier = WebPkiServerVerifier::builder(Arc::new(roots))
        .build()
        .unwrap();
    for name in ["localhost", "127.0.0.1", "192.168.1.23", "my-laptop"] {
        verifier
            .verify_server_cert(
                &key.cert[0],
                &key.cert[1..],
                &ServerName::try_from(name.to_string()).unwrap(),
                &[],
                UnixTime::now(),
            )
            .unwrap_or_else(|e| panic!("{name}: {e}"));
    }
    assert!(ca.server_certificate(&["not a name".to_string()]).is_err());
}
