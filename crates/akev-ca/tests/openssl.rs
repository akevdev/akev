//! Cross-checks against the `openssl` CLI. Every test is skipped (with a note on stderr) when
//! `openssl` is not installed.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Command;

use akev_ca::CertificateAuthority;
use base64::Engine as _;

fn have_openssl() -> bool {
    let ok = Command::new("openssl")
        .arg("version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        eprintln!("openssl not found; skipping OpenSSL cross-check");
    }
    ok
}

struct Out {
    ok: bool,
    stdout: String,
    stderr: String,
}

fn openssl(args: &[&str]) -> Out {
    let out = Command::new("openssl").args(args).output().unwrap();
    Out {
        ok: out.status.success(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

fn write(dir: &Path, name: &str, contents: impl AsRef<[u8]>) -> String {
    let path = dir.join(name);
    std::fs::write(&path, contents).unwrap();
    path.to_str().unwrap().to_owned()
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn fixture(name: &str) -> String {
    std::fs::read_to_string(fixtures().join(name)).unwrap()
}

fn leaf_pem(ca: &CertificateAuthority, host: &str) -> String {
    let leaf = ca.leaf_for(host).unwrap();
    let b64 = base64::engine::general_purpose::STANDARD.encode(&leaf.cert[0]);
    let mut pem = String::from("-----BEGIN CERTIFICATE-----\n");
    for line in b64.as_bytes().chunks(64) {
        pem.push_str(std::str::from_utf8(line).unwrap());
        pem.push('\n');
    }
    pem.push_str("-----END CERTIFICATE-----\n");
    pem
}

#[test]
fn subject_hash_old_matches_openssl() {
    if !have_openssl() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let generated = CertificateAuthority::generate().unwrap();
    let rsa = CertificateAuthority::from_pem(&fixture("rsa-ca.pem"), &fixture("rsa-ca.pkcs1.key"))
        .unwrap();
    let int = CertificateAuthority::from_pem(
        &fixture("intermediate-chain.pem"),
        &fixture("intermediate-ca.key"),
    )
    .unwrap();
    for (name, ca) in [
        ("generated", &generated),
        ("rsa", &rsa),
        ("intermediate", &int),
    ] {
        let path = write(tmp.path(), &format!("{name}.pem"), ca.cert_pem());
        let out = openssl(&["x509", "-subject_hash_old", "-noout", "-in", &path]);
        assert!(out.ok, "{}", out.stderr);
        assert_eq!(out.stdout.trim(), ca.info().subject_hash_old, "{name}");
    }
}

#[test]
fn fingerprints_match_openssl() {
    if !have_openssl() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let ca = CertificateAuthority::generate().unwrap();
    let info = ca.info();
    let cert = write(tmp.path(), "ca.pem", ca.cert_pem());

    let out = openssl(&["x509", "-fingerprint", "-sha256", "-noout", "-in", &cert]);
    assert!(out.ok, "{}", out.stderr);
    let fingerprint = out.stdout.trim().rsplit('=').next().unwrap();
    assert_eq!(fingerprint, info.sha256_fingerprint);

    let out = openssl(&["x509", "-pubkey", "-noout", "-in", &cert]);
    assert!(out.ok, "{}", out.stderr);
    let pubkey = write(tmp.path(), "pub.pem", &out.stdout);
    let der_path = tmp.path().join("pub.der");
    let out = openssl(&[
        "pkey",
        "-pubin",
        "-in",
        &pubkey,
        "-outform",
        "DER",
        "-out",
        der_path.to_str().unwrap(),
    ]);
    assert!(out.ok, "{}", out.stderr);
    let spki = std::fs::read(der_path).unwrap();
    let expected = base64::engine::general_purpose::STANDARD
        .encode(ring::digest::digest(&ring::digest::SHA256, &spki));
    assert_eq!(info.spki_fingerprint, expected);
}

#[test]
fn ca_certificate_text_dump() {
    if !have_openssl() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let ca = CertificateAuthority::generate().unwrap();
    let cert = write(tmp.path(), "ca.pem", ca.cert_pem());
    let out = openssl(&["x509", "-text", "-noout", "-in", &cert]);
    assert!(out.ok, "{}", out.stderr);
    let text = out.stdout;
    for needle in [
        "Signature Algorithm: ecdsa-with-SHA256",
        "Subject: O=Akev, OU=",
        "CN=Akev CA",
        "X509v3 Basic Constraints: critical",
        "CA:TRUE, pathlen:0",
        "X509v3 Key Usage: critical",
        "Digital Signature, Certificate Sign, CRL Sign",
        "X509v3 Subject Key Identifier",
        "X509v3 Authority Key Identifier",
    ] {
        assert!(text.contains(needle), "missing {needle:?} in:\n{text}");
    }
    assert!(
        text.contains("prime256v1") || text.contains("P-256"),
        "{text}"
    );
    assert!(
        !text.contains("C=") && !text.contains("C = "),
        "country must be omitted:\n{text}"
    );
}

#[test]
fn leaf_certificate_text_dump() {
    if !have_openssl() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let ca = CertificateAuthority::generate().unwrap();
    let leaf = write(tmp.path(), "leaf.pem", leaf_pem(&ca, "api.example.com"));
    let out = openssl(&["x509", "-text", "-noout", "-in", &leaf]);
    assert!(out.ok, "{}", out.stderr);
    let text = out.stdout;
    for needle in [
        "Subject: CN=api.example.com",
        "DNS:api.example.com, DNS:*.example.com",
        "TLS Web Server Authentication",
        "X509v3 Basic Constraints: critical",
        "CA:FALSE",
        "X509v3 Key Usage: critical",
        "Digital Signature",
        "X509v3 Authority Key Identifier",
    ] {
        assert!(text.contains(needle), "missing {needle:?} in:\n{text}");
    }
    assert!(!text.contains("Key Encipherment"), "{text}");

    let ip_leaf = write(tmp.path(), "ip.pem", leaf_pem(&ca, "[::1]"));
    let out = openssl(&["x509", "-text", "-noout", "-in", &ip_leaf]);
    assert!(out.ok, "{}", out.stderr);
    assert!(out.stdout.contains("IP Address:"), "{}", out.stdout);
}

/// `openssl verify` in strict mode, checking the hostname / IP against the leaf.
fn verify(ca_file: &str, untrusted: Option<&str>, leaf_file: &str, check: &[&str]) -> Out {
    let mut args = vec![
        "verify",
        "-x509_strict",
        "-purpose",
        "sslserver",
        "-CAfile",
        ca_file,
    ];
    if let Some(untrusted) = untrusted {
        args.extend(["-untrusted", untrusted]);
    }
    args.extend_from_slice(check);
    args.push(leaf_file);
    openssl(&args)
}

#[test]
fn openssl_verifies_leaves_strictly() {
    if !have_openssl() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let ca = CertificateAuthority::generate().unwrap();
    let ca_file = write(tmp.path(), "ca.pem", ca.cert_pem());

    let dns = write(tmp.path(), "dns.pem", leaf_pem(&ca, "api.example.com"));
    for host in ["api.example.com", "www.example.com"] {
        let out = verify(&ca_file, None, &dns, &["-verify_hostname", host]);
        assert!(
            out.ok && out.stdout.contains(": OK"),
            "{host}: {}{}",
            out.stdout,
            out.stderr
        );
    }
    for host in ["example.com", "evil.example.org", "a.b.example.com"] {
        let out = verify(&ca_file, None, &dns, &["-verify_hostname", host]);
        assert!(!out.ok, "{host} must not verify: {}", out.stdout);
    }

    let v4 = write(tmp.path(), "v4.pem", leaf_pem(&ca, "192.168.1.10"));
    assert!(verify(&ca_file, None, &v4, &["-verify_ip", "192.168.1.10"]).ok);
    assert!(!verify(&ca_file, None, &v4, &["-verify_ip", "192.168.1.11"]).ok);

    let v6 = write(tmp.path(), "v6.pem", leaf_pem(&ca, "[2001:db8::1]"));
    assert!(verify(&ca_file, None, &v6, &["-verify_ip", "2001:db8::1"]).ok);
    assert!(!verify(&ca_file, None, &v6, &["-verify_ip", "2001:db8::2"]).ok);

    let other = CertificateAuthority::generate().unwrap();
    let other_file = write(tmp.path(), "other.pem", other.cert_pem());
    assert!(
        !verify(&other_file, None, &dns, &[]).ok,
        "untrusted issuer must fail"
    );
}

#[test]
fn openssl_verifies_leaves_from_user_provided_cas() {
    if !have_openssl() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    for (cert, key) in [
        ("rsa-ca.pem", "rsa-ca.pkcs1.key"),
        ("ec-ca.pem", "ec-ca.sec1.key"),
        ("p384-ca.pem", "p384-ca.key"),
        ("ed25519-ca.pem", "ed25519-ca.key"),
    ] {
        let ca = CertificateAuthority::from_pem(&fixture(cert), &fixture(key)).unwrap();
        let leaf = write(tmp.path(), "leaf.pem", leaf_pem(&ca, "api.example.com"));
        let ca_file = fixtures().join(cert);
        let out = verify(
            ca_file.to_str().unwrap(),
            None,
            &leaf,
            &["-verify_hostname", "api.example.com"],
        );
        assert!(out.ok, "{cert}: {}{}", out.stdout, out.stderr);
    }

    let int = CertificateAuthority::from_pem(
        &fixture("intermediate-chain.pem"),
        &fixture("intermediate-ca.key"),
    )
    .unwrap();
    let leaf = write(
        tmp.path(),
        "int-leaf.pem",
        leaf_pem(&int, "api.example.com"),
    );
    let untrusted = write(tmp.path(), "int.pem", int.cert_pem());
    let root = fixtures().join("root-ca.pem");
    let out = verify(
        root.to_str().unwrap(),
        Some(&untrusted),
        &leaf,
        &["-verify_hostname", "api.example.com"],
    );
    assert!(out.ok, "{}{}", out.stdout, out.stderr);
}

#[test]
fn persisted_key_is_readable_by_openssl() {
    if !have_openssl() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    CertificateAuthority::load_or_generate(tmp.path()).unwrap();
    let key = tmp.path().join("ca.key");
    let cert = tmp.path().join("ca.pem");
    let out = openssl(&["pkey", "-in", key.to_str().unwrap(), "-pubout"]);
    assert!(out.ok, "{}", out.stderr);
    let from_cert = openssl(&["x509", "-in", cert.to_str().unwrap(), "-pubkey", "-noout"]);
    assert_eq!(out.stdout, from_cert.stdout, "key and certificate match");
}
