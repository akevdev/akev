//! Smoke test / benchmark against a running Akev with WireGuard mode on, acting as a phone
//! (boringtun + smoltcp WireGuard client, no root, no OS networking changes):
//!
//! ```text
//! cargo run --release -p akev-wireguard --example wg_smoke -- \
//!     127.0.0.1:51820 ~/.local/share/akev [--https example.com] [--bench 64]
//! ```
//!
//! * resolves `--https` host through the tunnel's DNS relay and fetches `https://host/`
//!   trusting only the Akev CA from the data directory (so the response must have been
//!   intercepted);
//! * `--bench N` downloads and uploads N MiB through the tunnel from a local HTTP server and
//!   prints the throughput.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use akev_wireguard::WireguardKeys;
use akev_wireguard::keys::KEYS_FILE;
use akev_wireguard::testing::TestClient;
use hickory_resolver::proto::op::{Message, MessageType, OpCode, Query};
use hickory_resolver::proto::rr::{Name, RData, RecordType};
use rustls_pki_types::pem::PemObject;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const T: Duration = Duration::from_secs(30);

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let usage = "usage: wg_smoke <server-udp-addr> <akev-data-dir> [--https HOST] [--bench MIB]";
    let server: SocketAddr = args.next().ok_or(usage)?.parse()?;
    let data_dir = PathBuf::from(args.next().ok_or(usage)?);
    let (mut https, mut bench) = (None, None);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--https" => https = args.next(),
            "--bench" => bench = args.next().map(|n| n.parse::<usize>()).transpose()?,
            _ => return Err(usage.into()),
        }
    }
    if !data_dir.join(KEYS_FILE).exists() {
        return Err(format!(
            "{} has no {KEYS_FILE}: enable WireGuard mode first",
            data_dir.display()
        )
        .into());
    }
    let keys = WireguardKeys::load_or_generate(&data_dir)?;
    let mut client = TestClient::start(server, &keys).await?;
    let t = Instant::now();
    client.handshake(Duration::from_secs(5)).await?;
    println!("handshake with {server}: {:?}", t.elapsed());

    if let Some(host) = https {
        let ip = resolve(&mut client, &host).await?;
        println!("DNS relay: {host} → {ip}");
        let ca = std::fs::read(data_dir.join("ca.pem"))?;
        let mut roots = rustls::RootCertStore::empty();
        roots.add(rustls_pki_types::CertificateDer::from_pem_slice(&ca)?)?;
        let cfg = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots)
        .with_no_client_auth();
        let t = Instant::now();
        let tcp = client.connect(SocketAddr::new(ip, 443)).await?;
        let name = rustls_pki_types::ServerName::try_from(host.clone())?;
        let mut tls = tokio_rustls::TlsConnector::from(Arc::new(cfg))
            .connect(name, tcp)
            .await?;
        tls.write_all(
            format!("GET / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nUser-Agent: wg_smoke\r\n\r\n")
                .as_bytes(),
        )
        .await?;
        let mut resp = Vec::new();
        tokio::time::timeout(T, tls.read_to_end(&mut resp)).await??;
        let head = String::from_utf8_lossy(&resp);
        println!(
            "HTTPS via tunnel (Akev CA only): {} ({} bytes, {:?})",
            head.lines().next().unwrap_or(""),
            resp.len(),
            t.elapsed()
        );
    }

    if let Some(mib) = bench {
        let size = mib * 1024 * 1024;
        let origin = bench_origin(size).await?;
        let mut s = client.connect(origin).await?;
        let t = Instant::now();
        s.write_all(
            format!("GET /download HTTP/1.1\r\nHost: {origin}\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await?;
        let mut got = 0usize;
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            let n = tokio::time::timeout(T, s.read(&mut buf)).await??;
            if n == 0 {
                break;
            }
            got += n;
        }
        let secs = t.elapsed().as_secs_f64();
        println!(
            "download: {:.1} MiB in {secs:.2}s = {:.1} MiB/s",
            got as f64 / 1048576.0,
            got as f64 / 1048576.0 / secs
        );
        let mut s = client.connect(origin).await?;
        let t = Instant::now();
        s.write_all(
            format!("POST /upload HTTP/1.1\r\nHost: {origin}\r\nContent-Length: {size}\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await?;
        let chunk = vec![b'u'; 256 * 1024];
        let mut sent = 0;
        while sent < size {
            let n = chunk.len().min(size - sent);
            s.write_all(&chunk[..n]).await?;
            sent += n;
        }
        let mut resp = Vec::new();
        tokio::time::timeout(T, s.read_to_end(&mut resp)).await??;
        let secs = t.elapsed().as_secs_f64();
        println!(
            "upload: {mib} MiB in {secs:.2}s = {:.1} MiB/s ({})",
            mib as f64 / secs,
            String::from_utf8_lossy(&resp).lines().next().unwrap_or("")
        );
    }
    Ok(())
}

async fn resolve(
    client: &mut TestClient,
    host: &str,
) -> Result<IpAddr, Box<dyn std::error::Error>> {
    let mut m = Message::new(0x5a5a, MessageType::Query, OpCode::Query);
    m.metadata.recursion_desired = true;
    m.add_query(Query::query(
        Name::from_ascii(format!("{host}."))?,
        RecordType::A,
    ));
    client.send_udp(5353, "10.0.0.53:53".parse()?, &m.to_vec()?);
    let d = client.recv_udp(T).await.ok_or("no DNS answer")?;
    let answer = Message::from_vec(&d.payload)?;
    answer
        .answers
        .iter()
        .find_map(|r| match &r.data {
            RData::A(a) => Some(IpAddr::V4(a.0)),
            _ => None,
        })
        .ok_or_else(|| {
            format!(
                "no A record for {host}: {:?}",
                answer.metadata.response_code
            )
            .into()
        })
}

/// Local HTTP server: `GET` returns `size` bytes, `POST` reads the body and answers 200.
async fn bench_origin(size: usize) -> std::io::Result<SocketAddr> {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = l.local_addr()?;
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            tokio::spawn(async move {
                let mut buf = vec![0u8; 256 * 1024];
                let mut head = Vec::new();
                let end = loop {
                    let Ok(n) = s.read(&mut buf).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    head.extend_from_slice(&buf[..n]);
                    if let Some(p) = head.windows(4).position(|w| w == b"\r\n\r\n") {
                        break p + 4;
                    }
                };
                if head.starts_with(b"GET") {
                    let _ = s
                        .write_all(
                            format!("HTTP/1.1 200 OK\r\nContent-Length: {size}\r\n\r\n").as_bytes(),
                        )
                        .await;
                    let chunk = vec![b'd'; 256 * 1024];
                    let mut sent = 0;
                    while sent < size {
                        let n = chunk.len().min(size - sent);
                        if s.write_all(&chunk[..n]).await.is_err() {
                            return;
                        }
                        sent += n;
                    }
                } else {
                    let mut got = head.len() - end;
                    while got < size {
                        match s.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => got += n,
                        }
                    }
                    let _ = s
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                        .await;
                }
            });
        }
    });
    Ok(addr)
}
