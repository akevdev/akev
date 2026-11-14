//! End-to-end: a rustls server resolving certificates with `leaf_for`, a rustls client that
//! trusts only the CA, talking over an in-memory duplex pipe.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;

use akev_ca::{CertificateAuthority, CertifiedKey};
use rustls::RootCertStore;
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls_pki_types::{CertificateDer, ServerName};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_rustls::{TlsAcceptor, TlsConnector};

/// Resolves by SNI (like the proxy), with an optional forced host and a fallback for clients
/// that send no SNI (IP-address connections).
#[derive(Debug)]
struct Resolver {
    ca: Arc<CertificateAuthority>,
    forced: Option<String>,
    fallback: String,
}

impl ResolvesServerCert for Resolver {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let host = self
            .forced
            .as_deref()
            .or(hello.server_name())
            .unwrap_or(&self.fallback);
        self.ca.leaf_for(host).ok()
    }
}

struct Handshake<'a> {
    ca: Arc<CertificateAuthority>,
    trust: &'a [u8],
    connect_to: &'a str,
    forced: Option<&'a str>,
    tls12_only: bool,
}

impl<'a> Handshake<'a> {
    fn new(ca: &Arc<CertificateAuthority>, connect_to: &'a str) -> Self {
        // An empty `trust` means "trust the CA's own certificate".
        Handshake {
            ca: Arc::clone(ca),
            trust: &[],
            connect_to,
            forced: None,
            tls12_only: false,
        }
    }

    async fn run(self) -> Result<(), String> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let versions: &[&rustls::SupportedProtocolVersion] = if self.tls12_only {
            &[&rustls::version::TLS12]
        } else {
            rustls::DEFAULT_VERSIONS
        };

        let fallback = self.connect_to.to_owned();
        let server_config = rustls::ServerConfig::builder_with_provider(Arc::clone(&provider))
            .with_protocol_versions(versions)
            .unwrap()
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(Resolver {
                ca: Arc::clone(&self.ca),
                forced: self.forced.map(str::to_owned),
                fallback,
            }));

        let trust = if self.trust.is_empty() {
            self.ca.cert_der().to_vec()
        } else {
            self.trust.to_vec()
        };
        let mut roots = RootCertStore::empty();
        roots.add(CertificateDer::from(trust)).unwrap();
        let client_config = rustls::ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(versions)
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();

        let server_name = match self.connect_to.parse::<IpAddr>() {
            Ok(ip) => ServerName::IpAddress(ip.into()),
            Err(_) => ServerName::try_from(self.connect_to.to_owned()).unwrap(),
        };

        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let acceptor = TlsAcceptor::from(Arc::new(server_config));
        let server = tokio::spawn(async move {
            let mut tls = acceptor
                .accept(server_io)
                .await
                .map_err(|e| e.to_string())?;
            let mut ping = [0u8; 4];
            tls.read_exact(&mut ping).await.map_err(|e| e.to_string())?;
            tls.write_all(b"pong").await.map_err(|e| e.to_string())?;
            tls.flush().await.map_err(|e| e.to_string())?;
            Ok::<_, String>(ping)
        });

        let connector = TlsConnector::from(Arc::new(client_config));
        let client = async {
            let mut tls = connector
                .connect(server_name, client_io)
                .await
                .map_err(|e| e.to_string())?;
            tls.write_all(b"ping").await.map_err(|e| e.to_string())?;
            let mut pong = [0u8; 4];
            tls.read_exact(&mut pong).await.map_err(|e| e.to_string())?;
            assert_eq!(&pong, b"pong");
            Ok::<_, String>(())
        };
        match client.await {
            Ok(()) => {
                let ping = server.await.unwrap()?;
                assert_eq!(&ping, b"ping");
                Ok(())
            }
            Err(e) => {
                server.abort();
                Err(e)
            }
        }
    }
}

fn new_ca() -> Arc<CertificateAuthority> {
    Arc::new(CertificateAuthority::generate().unwrap())
}

#[tokio::test]
async fn handshake_succeeds_for_sni_hosts() {
    let ca = new_ca();
    for host in [
        "api.example.com",
        "example.com",
        "localhost",
        "xn--bcher-kva.example",
        "my_svc.example.com",
    ] {
        Handshake::new(&ca, host)
            .run()
            .await
            .unwrap_or_else(|e| panic!("{host}: {e}"));
    }
    assert_eq!(ca.cached_leaf_count(), 5);
}

#[tokio::test]
async fn handshake_succeeds_for_ip_addresses_without_sni() {
    let ca = new_ca();
    for ip in ["127.0.0.1", "::1", "2001:db8::7"] {
        Handshake::new(&ca, ip)
            .run()
            .await
            .unwrap_or_else(|e| panic!("{ip}: {e}"));
    }
}

#[tokio::test]
async fn handshake_succeeds_over_tls12() {
    let ca = new_ca();
    let mut hs = Handshake::new(&ca, "legacy.example.com");
    hs.tls12_only = true;
    hs.run().await.unwrap();
}

#[tokio::test]
async fn handshake_fails_for_the_wrong_name() {
    let ca = new_ca();
    let mut hs = Handshake::new(&ca, "api.example.com");
    hs.forced = Some("other.test");
    let err = hs.run().await.unwrap_err();
    assert!(
        err.contains("NotValidForName") || err.contains("not valid for name"),
        "{err}"
    );

    let mut hs = Handshake::new(&ca, "127.0.0.2");
    hs.forced = Some("127.0.0.1");
    assert!(hs.run().await.is_err());
}

#[tokio::test]
async fn wildcard_sibling_covers_other_subdomains() {
    let ca = new_ca();
    let mut hs = Handshake::new(&ca, "www.example.com");
    hs.forced = Some("api.example.com");
    hs.run().await.unwrap();

    // ...but only one level deep, and never the parent itself.
    for name in ["example.com", "deep.www.example.com"] {
        let mut hs = Handshake::new(&ca, name);
        hs.forced = Some("api.example.com");
        assert!(hs.run().await.is_err(), "{name}");
    }
}

#[tokio::test]
async fn handshake_fails_when_the_ca_is_not_trusted() {
    let ca = new_ca();
    let other = CertificateAuthority::generate().unwrap();
    let mut hs = Handshake::new(&ca, "example.com");
    hs.trust = other.cert_der();
    let err = hs.run().await.unwrap_err();
    assert!(
        err.contains("UnknownIssuer") || err.contains("unknown issuer"),
        "{err}"
    );
}

#[tokio::test]
async fn handshake_with_a_user_provided_rsa_ca() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let cert = std::fs::read_to_string(fixtures.join("rsa-ca.pem")).unwrap();
    let key = std::fs::read_to_string(fixtures.join("rsa-ca.pkcs1.key")).unwrap();
    let ca = Arc::new(CertificateAuthority::from_pem(&cert, &key).unwrap());
    Handshake::new(&ca, "api.example.com").run().await.unwrap();
}

#[tokio::test]
async fn handshake_with_an_intermediate_ca_chain() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let chain = std::fs::read_to_string(fixtures.join("intermediate-chain.pem")).unwrap();
    let key = std::fs::read_to_string(fixtures.join("intermediate-ca.key")).unwrap();
    let root_pem = std::fs::read_to_string(fixtures.join("root-ca.pem")).unwrap();
    let root = rustls_pki_types::pem::PemObject::from_pem_slice(root_pem.as_bytes())
        .map(|der: CertificateDer<'static>| der.to_vec())
        .unwrap();
    let ca = Arc::new(CertificateAuthority::from_pem(&chain, &key).unwrap());
    let mut hs = Handshake::new(&ca, "api.example.com");
    hs.trust = &root;
    hs.run().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parallel_handshakes_to_a_new_host_share_one_certificate() {
    let ca = new_ca();
    let tasks: Vec<_> = (0..24)
        .map(|_| {
            let ca = Arc::clone(&ca);
            tokio::spawn(async move { Handshake::new(&ca, "burst.example.com").run().await })
        })
        .collect();
    for task in tasks {
        task.await.unwrap().unwrap();
    }
    assert_eq!(ca.cached_leaf_count(), 1);
}
