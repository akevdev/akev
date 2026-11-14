//! The WireGuard server and its userspace network stack, driven by a real WireGuard client
//! (boringtun + smoltcp, like a phone running the WireGuard app) over loopback UDP. TCP
//! connections go to an echo handler here; `e2e.rs` covers the proxy integration.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use akev_wireguard::dns::{Answer, DnsBackend};
use akev_wireguard::testing::TestClient;
use akev_wireguard::{ServerOptions, TcpHandler, TcpStream, WireguardKeys, WireguardServer};
use futures::future::BoxFuture;
use hickory_resolver::proto::op::{Message, MessageType, OpCode, Query, ResponseCode};
use hickory_resolver::proto::rr::rdata::A;
use hickory_resolver::proto::rr::{Name, RData, Record, RecordType};
use parking_lot::Mutex;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Echoes every connection and remembers (device address, original destination).
#[derive(Default)]
struct Echo {
    seen: Mutex<Vec<(SocketAddr, SocketAddr)>>,
}

impl TcpHandler for Echo {
    fn handle(&self, stream: TcpStream) {
        self.seen
            .lock()
            .push((stream.peer_addr(), stream.local_addr()));
        tokio::spawn(async move {
            let (mut r, mut w) = tokio::io::split(stream);
            let _ = tokio::io::copy(&mut r, &mut w).await;
            let _ = w.shutdown().await;
        });
    }
}

/// Resets every connection right away.
struct Resetter;

impl TcpHandler for Resetter {
    fn handle(&self, stream: TcpStream) {
        stream.reset_flag().store(true, Ordering::Relaxed);
        drop(stream);
    }
}

/// Holds every connection open without reading.
#[derive(Default)]
struct Hold {
    streams: Mutex<Vec<TcpStream>>,
}

impl TcpHandler for Hold {
    fn handle(&self, stream: TcpStream) {
        self.streams.lock().push(stream);
    }
}

/// `*.test` answers; everything else NXDOMAIN.
struct FakeDns;

impl DnsBackend for FakeDns {
    fn lookup(&self, name: Name, rtype: RecordType) -> BoxFuture<'static, Answer> {
        Box::pin(async move {
            match (name.to_ascii().as_str(), rtype) {
                ("origin.test.", RecordType::A) => Answer::Records(vec![Record::from_rdata(
                    name,
                    60,
                    RData::A(A(Ipv4Addr::new(192, 0, 2, 7))),
                )]),
                ("many.test.", RecordType::A) => Answer::Records(
                    (0..150u8)
                        .map(|i| {
                            Record::from_rdata(
                                name.clone(),
                                60,
                                RData::A(A(Ipv4Addr::new(10, 9, 0, i))),
                            )
                        })
                        .collect(),
                ),
                _ => Answer::NxDomain,
            }
        })
    }
}

async fn start(
    handler: Arc<dyn TcpHandler>,
    tweak: impl FnOnce(&mut ServerOptions),
) -> (WireguardServer, WireguardKeys) {
    let keys = WireguardKeys::generate();
    let mut options = ServerOptions::new("127.0.0.1:0".parse().unwrap(), keys.clone());
    tweak(&mut options);
    let server = WireguardServer::start(options, handler, Arc::new(FakeDns))
        .await
        .expect("server starts");
    (server, keys)
}

async fn client(server: &WireguardServer, keys: &WireguardKeys) -> TestClient {
    let c = TestClient::start(server.local_addr(), keys).await.unwrap();
    c.handshake(Duration::from_secs(10))
        .await
        .expect("handshake");
    c
}

const T: Duration = Duration::from_secs(20);

#[tokio::test]
async fn tcp_echo_with_original_destination() {
    let echo = Arc::new(Echo::default());
    let (server, keys) = start(echo.clone(), |_| {}).await;
    let c = client(&server, &keys).await;
    let dst: SocketAddr = "203.0.113.9:7777".parse().unwrap();
    let mut s = c.connect(dst).await.unwrap();
    s.write_all(b"hello through the tunnel").await.unwrap();
    s.shutdown().await.unwrap();
    let mut out = Vec::new();
    tokio::time::timeout(T, s.read_to_end(&mut out))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out, b"hello through the tunnel");

    let seen = echo.seen.lock().clone();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].1, dst, "local_addr is the original destination");
    assert_eq!(seen[0].0.ip(), Ipv4Addr::new(10, 0, 0, 2));

    let status = server.status();
    assert_eq!(status.tcp_total, 1);
    assert!(status.peer.last_seen_at.is_some());
    assert_eq!(status.peer.public_key, keys.client.public_key().to_base64());
    // The peer status is refreshed on the 250 ms timer.
    tokio::time::sleep(Duration::from_millis(400)).await;
    let status = server.status();
    assert!(status.peer.connected, "{status:?}");
    assert!(status.peer.last_handshake_at.is_some());
    assert!(
        status
            .peer
            .endpoint
            .as_deref()
            .unwrap()
            .starts_with("127.0.0.1:")
    );
    assert!(status.peer.rx_bytes > 0 && status.peer.tx_bytes > 0);
    server.stop().await;
}

#[tokio::test]
async fn large_transfers_in_both_directions() {
    let (server, keys) = start(Arc::new(Echo::default()), |_| {}).await;
    let c = client(&server, &keys).await;
    let s = c
        .connect("198.51.100.1:9000".parse().unwrap())
        .await
        .unwrap();
    let data: Vec<u8> = (0..3 * 1024 * 1024u32)
        .map(|i| (i.wrapping_mul(2654435761) >> 13) as u8)
        .collect();
    let (mut r, mut w) = tokio::io::split(s);
    let expected = data.clone();
    let writer = tokio::spawn(async move {
        w.write_all(&data).await.unwrap();
        w.shutdown().await.unwrap();
    });
    let mut out = Vec::new();
    tokio::time::timeout(Duration::from_secs(60), r.read_to_end(&mut out))
        .await
        .expect("transfer finishes")
        .unwrap();
    writer.await.unwrap();
    assert_eq!(out.len(), expected.len());
    assert!(out == expected, "echoed bytes differ");
    server.stop().await;
}

#[tokio::test]
async fn concurrent_connections_to_one_destination() {
    let echo = Arc::new(Echo::default());
    let (server, keys) = start(echo.clone(), |_| {}).await;
    let c = Arc::new(client(&server, &keys).await);
    let dst: SocketAddr = "192.0.2.80:80".parse().unwrap();
    let tasks: Vec<_> = (0..40)
        .map(|i| {
            let c = c.clone();
            tokio::spawn(async move {
                let mut s = c.connect(dst).await.unwrap();
                let msg = format!("connection {i}").repeat(100);
                s.write_all(msg.as_bytes()).await.unwrap();
                s.shutdown().await.unwrap();
                let mut out = String::new();
                s.read_to_string(&mut out).await.unwrap();
                assert_eq!(out, msg);
            })
        })
        .collect();
    for t in tasks {
        tokio::time::timeout(T, t).await.unwrap().unwrap();
    }
    let seen = echo.seen.lock().clone();
    assert_eq!(seen.len(), 40);
    assert!(seen.iter().all(|(_, d)| *d == dst));
    let mut ports: Vec<u16> = seen.iter().map(|(p, _)| p.port()).collect();
    ports.sort_unstable();
    ports.dedup();
    assert_eq!(ports.len(), 40, "each connection has its own 4-tuple");
    assert_eq!(server.status().tcp_total, 40);
    server.stop().await;
}

#[tokio::test]
async fn ipv6_destinations_are_terminated_too() {
    let echo = Arc::new(Echo::default());
    let (server, keys) = start(echo.clone(), |_| {}).await;
    let c = client(&server, &keys).await;
    // The client config only assigns IPv4; an IPv6 SYN from the device still works.
    let mut syn = Vec::new();
    {
        use smoltcp::phy::ChecksumCapabilities;
        use smoltcp::wire::*;
        let src: Ipv6Address = "fd00::2".parse().unwrap();
        let dst: Ipv6Address = "2001:db8::1".parse().unwrap();
        let tcp = TcpRepr {
            src_port: 50000,
            dst_port: 443,
            control: TcpControl::Syn,
            seq_number: TcpSeqNumber(1000),
            ack_number: None,
            window_len: 65535,
            window_scale: None,
            max_seg_size: Some(1200),
            sack_permitted: false,
            sack_ranges: [None; 3],
            timestamp: None,
            payload: &[],
        };
        let ip = Ipv6Repr {
            src_addr: src,
            dst_addr: dst,
            next_header: IpProtocol::Tcp,
            payload_len: tcp.buffer_len(),
            hop_limit: 64,
        };
        syn.resize(ip.buffer_len() + tcp.buffer_len(), 0);
        let mut pkt = Ipv6Packet::new_unchecked(&mut syn[..]);
        ip.emit(&mut pkt);
        let mut seg = TcpPacket::new_unchecked(pkt.payload_mut());
        tcp.emit(
            &mut seg,
            &IpAddress::Ipv6(src),
            &IpAddress::Ipv6(dst),
            &ChecksumCapabilities::default(),
        );
    }
    c.send_ip(syn);
    // The SYN-ACK goes back to the client (which ignores it): the server created a socket.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(server.status().tcp_rejected, 0);
    assert_eq!(server.status().dropped_packets, 0);
    server.stop().await;
}

#[tokio::test]
async fn connection_limit_refuses_with_reset() {
    let hold = Arc::new(Hold::default());
    let (server, keys) = start(hold.clone(), |o| o.max_tcp_connections = 2).await;
    let c = client(&server, &keys).await;
    let dst: SocketAddr = "192.0.2.1:443".parse().unwrap();
    let _a = c.connect(dst).await.unwrap();
    let _b = c.connect(dst).await.unwrap();
    let third = c.connect(dst).await;
    assert_eq!(
        third.unwrap_err().kind(),
        std::io::ErrorKind::ConnectionRefused
    );
    assert_eq!(server.status().tcp_rejected, 1);
    assert_eq!(hold.streams.lock().len(), 2);
    server.stop().await;
}

#[tokio::test]
async fn reset_flag_and_server_stop_close_connections() {
    let (server, keys) = start(Arc::new(Resetter), |_| {}).await;
    let c = client(&server, &keys).await;
    let mut s = c.connect("192.0.2.2:80".parse().unwrap()).await.unwrap();
    let mut buf = [0u8; 16];
    let n = tokio::time::timeout(T, s.read(&mut buf)).await.unwrap();
    assert!(matches!(n, Ok(0) | Err(_)), "reset connection must close");
    server.stop().await;

    let hold = Arc::new(Hold::default());
    let (server, keys) = start(hold.clone(), |_| {}).await;
    let c = client(&server, &keys).await;
    let mut s = c.connect("192.0.2.3:80".parse().unwrap()).await.unwrap();
    // The server hands the connection out once the client's final ACK arrived.
    for _ in 0..200 {
        if !hold.streams.lock().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    server.stop().await;
    let n = tokio::time::timeout(T, s.read(&mut buf)).await.unwrap();
    assert!(
        matches!(n, Ok(0) | Err(_)),
        "stopping the server closes connections"
    );
    let mut held = hold.streams.lock().pop().unwrap();
    let r = tokio::time::timeout(T, held.read(&mut buf)).await.unwrap();
    assert!(r.is_err(), "the application side sees a reset");
}

#[tokio::test]
async fn wrong_key_gets_no_session() {
    let (server, _keys) = start(Arc::new(Echo::default()), |_| {}).await;
    let impostor = WireguardKeys::generate();
    let c = TestClient::start(server.local_addr(), &impostor)
        .await
        .unwrap();
    assert!(c.handshake(Duration::from_secs(1)).await.is_err());
    let status = server.status();
    assert!(status.peer.endpoint.is_none() && status.peer.last_seen_at.is_none());
    server.stop().await;
}

fn dns_query(name: &str, rtype: RecordType, id: u16) -> Vec<u8> {
    let mut m = Message::new(id, MessageType::Query, OpCode::Query);
    m.metadata.recursion_desired = true;
    m.add_query(Query::query(Name::from_ascii(name).unwrap(), rtype));
    m.to_vec().unwrap()
}

fn a_records(m: &Message) -> Vec<Ipv4Addr> {
    m.answers
        .iter()
        .filter_map(|r| match &r.data {
            RData::A(a) => Some(a.0),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn dns_relay_over_udp_and_tcp() {
    let (server, keys) = start(Arc::new(Echo::default()), |_| {}).await;
    let mut c = client(&server, &keys).await;
    let relay: SocketAddr = "10.0.0.53:53".parse().unwrap();

    c.send_udp(5353, relay, &dns_query("origin.test.", RecordType::A, 1));
    let d = c.recv_udp(T).await.expect("DNS answer");
    assert_eq!(d.from, relay);
    assert_eq!(d.to_port, 5353);
    let m = Message::from_vec(&d.payload).unwrap();
    assert_eq!(m.metadata.id, 1);
    assert_eq!(a_records(&m), vec![Ipv4Addr::new(192, 0, 2, 7)]);

    c.send_udp(5354, relay, &dns_query("akev.cert.", RecordType::A, 2));
    let m = Message::from_vec(&c.recv_udp(T).await.unwrap().payload).unwrap();
    assert_eq!(a_records(&m), vec![Ipv4Addr::new(10, 0, 0, 1)]);

    c.send_udp(5355, relay, &dns_query("nope.test.", RecordType::A, 3));
    let m = Message::from_vec(&c.recv_udp(T).await.unwrap().payload).unwrap();
    assert_eq!(m.metadata.response_code, ResponseCode::NXDomain);

    // Too big for UDP: truncated, then fetched over TCP from the same relay.
    c.send_udp(5356, relay, &dns_query("many.test.", RecordType::A, 4));
    let m = Message::from_vec(&c.recv_udp(T).await.unwrap().payload).unwrap();
    assert!(m.metadata.truncation);
    let mut s = c.connect(relay).await.unwrap();
    let q = dns_query("many.test.", RecordType::A, 5);
    s.write_all(&(q.len() as u16).to_be_bytes()).await.unwrap();
    s.write_all(&q).await.unwrap();
    let mut len = [0u8; 2];
    tokio::time::timeout(T, s.read_exact(&mut len))
        .await
        .unwrap()
        .unwrap();
    let mut resp = vec![0u8; u16::from_be_bytes(len) as usize];
    s.read_exact(&mut resp).await.unwrap();
    assert_eq!(a_records(&Message::from_vec(&resp).unwrap()).len(), 150);
    assert_eq!(server.status().dns_queries, 4);
    server.stop().await;
}

#[tokio::test]
async fn udp_is_relayed_and_quic_dropped() {
    let echo = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let echo_addr = echo.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buf = [0u8; 2048];
        while let Ok((n, from)) = echo.recv_from(&mut buf).await {
            let _ = echo.send_to(&buf[..n], from).await;
        }
    });
    let (server, keys) = start(Arc::new(Echo::default()), |_| {}).await;
    let mut c = client(&server, &keys).await;

    c.send_udp(6000, echo_addr, b"udp ping");
    let d = c.recv_udp(T).await.expect("relayed reply");
    assert_eq!(d.from, echo_addr);
    assert_eq!(d.to_port, 6000);
    assert_eq!(d.payload, b"udp ping");

    c.send_udp(6001, "192.0.2.50:443".parse().unwrap(), b"\xc0quic initial");
    assert!(c.recv_udp(Duration::from_millis(300)).await.is_none());
    c.send_ip(vec![
        0x45, 0, 0, 20, 0, 0, 0, 0, 64, 1, 0, 0, 10, 0, 0, 2, 1, 1, 1, 1,
    ]);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let status = server.status();
    assert_eq!(status.quic_dropped, 1);
    assert_eq!(status.udp_relayed, 1);
    assert_eq!(status.udp_flows, 1);
    assert_eq!(status.dropped_packets, 1);
    server.stop().await;
}
