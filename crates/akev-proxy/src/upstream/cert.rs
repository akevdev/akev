//! Certificate summaries (for [`UpstreamInfo::certificate`]) and the certificate facts used to
//! build precise verification error messages.
//!
//! [`UpstreamInfo::certificate`]: akev_core::UpstreamInfo::certificate

use akev_core::CertificateSummary;
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::net::{Ipv4Addr, Ipv6Addr};
use x509_parser::prelude::*;

/// Parsed facts about a certificate that error messages need.
#[derive(Clone, Debug, Default)]
pub(crate) struct CertFacts {
    pub subject: String,
    pub issuer: String,
    pub san: Vec<String>,
    pub not_before: String,
    pub not_after: String,
    pub self_issued: bool,
}

/// Summarise a DER certificate. `None` if it can't be parsed.
pub(crate) fn summarize(der: &[u8]) -> Option<CertificateSummary> {
    let (_, cert) = parse_x509_certificate(der).ok()?;
    let facts = facts_of(&cert);
    Some(CertificateSummary {
        subject: facts.subject,
        issuer: facts.issuer,
        san: facts.san,
        not_before: facts.not_before,
        not_after: facts.not_after,
        serial: colon_hex(cert.raw_serial()),
        sha256_fingerprint: colon_hex(&Sha256::digest(der)),
    })
}

/// Summarise a DER certificate; one that can't be parsed still gets its fingerprint (chain
/// views keep every certificate the server sent, in order).
pub(crate) fn summarize_any(der: &[u8]) -> CertificateSummary {
    summarize(der).unwrap_or_else(|| CertificateSummary {
        subject: "Unreadable certificate".to_owned(),
        sha256_fingerprint: colon_hex(&Sha256::digest(der)),
        ..CertificateSummary::default()
    })
}

/// Format a distinguished name given as the contents of its DER `SEQUENCE` (how trust anchors
/// store subjects), e.g. `C=US, O=Let's Encrypt, CN=R3`.
pub(crate) fn name_from_contents(contents: &[u8]) -> Option<String> {
    let len = contents.len();
    let mut der = Vec::with_capacity(len + 4);
    der.push(0x30);
    match len {
        0..=0x7f => der.push(len as u8),
        0x80..=0xff => der.extend_from_slice(&[0x81, len as u8]),
        0x100..=0xffff => der.extend_from_slice(&[0x82, (len >> 8) as u8, len as u8]),
        _ => return None,
    }
    der.extend_from_slice(contents);
    let (rest, name) = X509Name::from_der(&der).ok()?;
    Some(name.to_string()).filter(|n| rest.is_empty() && !n.is_empty())
}

/// Facts about a DER certificate. `None` if it can't be parsed.
pub(crate) fn facts(der: &[u8]) -> Option<CertFacts> {
    let (_, cert) = parse_x509_certificate(der).ok()?;
    Some(facts_of(&cert))
}

fn facts_of(cert: &X509Certificate<'_>) -> CertFacts {
    CertFacts {
        subject: cert.subject().to_string(),
        issuer: cert.issuer().to_string(),
        san: subject_alt_names(cert),
        not_before: rfc3339(cert.validity().not_before),
        not_after: rfc3339(cert.validity().not_after),
        self_issued: cert.subject().as_raw() == cert.issuer().as_raw(),
    }
}

fn subject_alt_names(cert: &X509Certificate<'_>) -> Vec<String> {
    let Ok(Some(ext)) = cert.subject_alternative_name() else {
        return Vec::new();
    };
    ext.value
        .general_names
        .iter()
        .filter_map(|name| match name {
            GeneralName::DNSName(dns) => Some((*dns).to_owned()),
            GeneralName::IPAddress(bytes) => match bytes.len() {
                4 => Some(Ipv4Addr::new(bytes[0], bytes[1], bytes[2], bytes[3]).to_string()),
                16 => {
                    let mut octets = [0u8; 16];
                    octets.copy_from_slice(bytes);
                    Some(Ipv6Addr::from(octets).to_string())
                }
                _ => None,
            },
            GeneralName::URI(uri) => Some((*uri).to_owned()),
            GeneralName::RFC822Name(mail) => Some(format!("email:{mail}")),
            _ => None,
        })
        .collect()
}

fn rfc3339(t: ASN1Time) -> String {
    t.to_datetime()
        .format(&::time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

/// `AB:CD:EF` uppercase colon-separated hex (the format certificate viewers use).
pub(crate) fn colon_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 {
            out.push(':');
        }
        let _ = write!(out, "{b:02X}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarises_generated_certificate() {
        let mut params =
            rcgen::CertificateParams::new(vec!["example.test".into(), "127.0.0.1".into()]).unwrap();
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "example.test");
        params.serial_number = Some(rcgen::SerialNumber::from_slice(&[0x01, 0xAB]));
        params.not_before = rcgen::date_time_ymd(2020, 1, 2);
        params.not_after = rcgen::date_time_ymd(2030, 3, 4);
        let key = rcgen::KeyPair::generate().unwrap();
        let cert = params.self_signed(&key).unwrap();
        let summary = summarize(cert.der()).unwrap();
        assert_eq!(summary.subject, "CN=example.test");
        assert_eq!(summary.issuer, "CN=example.test");
        assert_eq!(
            summary.san,
            vec!["example.test".to_string(), "127.0.0.1".to_string()]
        );
        assert_eq!(summary.not_before, "2020-01-02T00:00:00Z");
        assert_eq!(summary.not_after, "2030-03-04T00:00:00Z");
        assert_eq!(summary.serial, "01:AB");
        assert_eq!(summary.sha256_fingerprint.len(), 32 * 3 - 1);
        let f = facts(cert.der()).unwrap();
        assert!(f.self_issued);
    }

    #[test]
    fn names_from_trust_anchor_subjects() {
        let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
        params
            .distinguished_name
            .push(rcgen::DnType::OrganizationName, "Akev Test");
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "Akev Test Root");
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let key = rcgen::KeyPair::generate().unwrap();
        let cert = params.self_signed(&key).unwrap();
        let anchor = webpki::anchor_from_trusted_cert(cert.der()).unwrap();
        // The same text as the certificate's own subject.
        let subject = facts(cert.der()).unwrap().subject;
        assert!(subject.contains("CN=Akev Test Root") && subject.contains("O=Akev Test"));
        assert_eq!(name_from_contents(anchor.subject.as_ref()), Some(subject));
        assert_eq!(name_from_contents(b"\x01\x02"), None);
        // Long names use the multi-byte length form.
        let mut long = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
        long.distinguished_name
            .push(rcgen::DnType::CommonName, "x".repeat(300));
        let cert = long.self_signed(&key).unwrap();
        let anchor = webpki::anchor_from_trusted_cert(cert.der()).unwrap();
        assert_eq!(
            name_from_contents(anchor.subject.as_ref()),
            Some(format!("CN={}", "x".repeat(300)))
        );
    }

    #[test]
    fn unreadable_certificates_keep_their_fingerprint() {
        let s = summarize_any(b"junk");
        assert_eq!(s.subject, "Unreadable certificate");
        assert_eq!(s.sha256_fingerprint.len(), 32 * 3 - 1);
    }

    #[test]
    fn garbage_is_none() {
        assert!(summarize(b"not a certificate").is_none());
        assert_eq!(colon_hex(&[0, 255, 16]), "00:FF:10");
    }
}
