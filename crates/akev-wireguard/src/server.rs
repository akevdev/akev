//! The WireGuard endpoint: one UDP socket, one boringtun tunnel (the single client peer) and
//! the userspace network stack, all driven by a single task.
//!
//! Each loop iteration waits for: a datagram from the peer, a UDP/DNS reply to send back, a
//! wake-up from a connection stream (data written / buffer drained / closed), the 250 ms
//! WireGuard timer tick, or the TCP stack's next timer. It then decrypts what arrived, feeds
//! the packets to the stack, moves data between sockets and streams, and encrypts and sends
//! whatever the stack emitted.

use crate::dns::{self, DnsBackend};
use crate::keys::{WireguardKeys, now_ms};
use crate::packet::{self, Classified};
use crate::stack::{Stack, TcpStats};
use crate::stream::TcpStream;
use crate::udp::{Reply, UdpRelay};
use crate::{DNS_ADDRESS, SERVER_ADDRESS, TcpHandler};
use boringtun::noise::{Tunn, TunnResult};
use parking_lot::Mutex;
use serde::Serialize;
use socket2::{Domain, Protocol, Socket, Type};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::{Notify, Semaphore, mpsc};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// WireGuard's timer granularity (boringtun expects `update_timers` about this often).
const TIMER_TICK: Duration = Duration::from_millis(250);
/// A session older than this is dead (WireGuard's REJECT_AFTER_TIME).
const SESSION_LIFETIME: Duration = Duration::from_secs(180);
/// DNS queries resolved concurrently; more are dropped (the client retries).
const MAX_DNS_IN_FLIGHT: usize = 256;
/// Datagrams handled per wake-up before the stack runs.
const RECV_BATCH: usize = 64;
/// Queue of replies (DNS answers, relayed UDP) waiting to be sent into the tunnel.
const REPLY_QUEUE: usize = 1024;

/// Options for [`WireguardServer::start`].
#[derive(Clone, Debug)]
pub struct ServerOptions {
    /// UDP address to listen on.
    pub bind: SocketAddr,
    /// Server and client keys.
    pub keys: WireguardKeys,
    /// Largest IP packet sent into the tunnel (the client's interface MTU; WireGuard apps
    /// default to 1420 or less).
    pub mtu: usize,
    /// Most simultaneous TCP connections; further SYNs are refused with a RST.
    pub max_tcp_connections: usize,
    /// Most simultaneous relayed UDP flows.
    pub max_udp_flows: usize,
}

impl ServerOptions {
    /// Defaults for everything but the address and keys.
    pub fn new(bind: SocketAddr, keys: WireguardKeys) -> Self {
        ServerOptions {
            bind,
            keys,
            mtu: crate::TUNNEL_MTU,
            max_tcp_connections: 1024,
            max_udp_flows: 512,
        }
    }
}

/// The client peer as seen by the server.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerStatus {
    /// The client's public key.
    pub public_key: String,
    /// Where the device's packets come from (`ip:port`), once it connected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// When the current session was established (unix ms).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_handshake_at: Option<u64>,
    /// When the last authenticated packet arrived (unix ms).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<u64>,
    /// Bytes of IP packets received from the device.
    pub rx_bytes: u64,
    /// Bytes of IP packets sent to the device.
    pub tx_bytes: u64,
    /// A session is established and younger than WireGuard's 3-minute session lifetime.
    pub connected: bool,
}

/// Live state of a running server.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerStatus {
    /// The UDP address the server listens on.
    pub listen_addr: String,
    /// The client.
    pub peer: PeerStatus,
    /// TCP connections currently open (handed to the proxy).
    pub tcp_active: u64,
    /// TCP connections accepted since start.
    pub tcp_total: u64,
    /// TCP connections refused because of the connection limit.
    pub tcp_rejected: u64,
    /// DNS queries received by the relay.
    pub dns_queries: u64,
    /// UDP flows currently relayed.
    pub udp_flows: u64,
    /// UDP datagrams relayed to the network.
    pub udp_relayed: u64,
    /// QUIC datagrams (UDP port 443) dropped so clients fall back to TCP.
    pub quic_dropped: u64,
    /// Other packets dropped (ICMP, fragments, malformed...).
    pub dropped_packets: u64,
}

#[derive(Default)]
struct Shared {
    peer: Mutex<PeerStatus>,
    last_seen_at: AtomicU64,
    dns_queries: AtomicU64,
    quic_dropped: AtomicU64,
    dropped_packets: AtomicU64,
    udp_flows: AtomicU64,
    udp_relayed: Arc<AtomicU64>,
    tcp: Arc<TcpStats>,
}

/// A running WireGuard server. Stops when [`WireguardServer::stop`] is called or it's dropped.
pub struct WireguardServer {
    local_addr: SocketAddr,
    shared: Arc<Shared>,
    shutdown: CancellationToken,
    task: Option<JoinHandle<()>>,
}

impl WireguardServer {
    /// Bind the UDP socket and start serving. TCP connections are handed to `handler`; DNS
    /// queries to the relay are answered with `dns`.
    pub async fn start(
        options: ServerOptions,
        handler: Arc<dyn TcpHandler>,
        dns: Arc<dyn DnsBackend>,
    ) -> std::io::Result<Self> {
        let udp = Arc::new(bind_udp(options.bind)?);
        let local_addr = udp.local_addr()?;
        let shutdown = CancellationToken::new();
        let wake = Arc::new(Notify::new());
        let stack = Stack::new(options.mtu, options.max_tcp_connections, wake.clone());
        let (replies_tx, replies_rx) = mpsc::channel(REPLY_QUEUE);
        let relay = UdpRelay::new(replies_tx.clone(), options.max_udp_flows, shutdown.clone());
        let shared = Arc::new(Shared {
            peer: Mutex::new(PeerStatus {
                public_key: options.keys.client.public_key().to_base64(),
                ..Default::default()
            }),
            udp_relayed: relay.relayed.clone(),
            tcp: stack.stats.clone(),
            ..Default::default()
        });
        let tunn = Tunn::new(
            options.keys.server.secret(),
            options.keys.client.public_key().key(),
            None,
            None,
            rand::random::<u32>() >> 8,
            None,
        );
        let worker = Worker {
            udp,
            tunn: Box::new(tunn),
            endpoint: None,
            stack,
            relay,
            replies_rx,
            replies_tx,
            handler,
            dns,
            dns_permits: Arc::new(Semaphore::new(MAX_DNS_IN_FLIGHT)),
            wake,
            shared: shared.clone(),
            shutdown: shutdown.clone(),
            mtu: options.mtu,
            buf: vec![0u8; 65536 + 256],
        };
        let task = tokio::spawn(worker.run());
        tracing::info!(addr = %local_addr, "WireGuard server listening");
        Ok(WireguardServer {
            local_addr,
            shared,
            shutdown,
            task: Some(task),
        })
    }

    /// The UDP address the server listens on.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Current status (peer handshake state, counters).
    pub fn status(&self) -> ServerStatus {
        let s = &self.shared;
        let mut peer = s.peer.lock().clone();
        let last_seen = s.last_seen_at.load(Ordering::Relaxed);
        peer.last_seen_at = (last_seen > 0).then_some(last_seen);
        ServerStatus {
            listen_addr: self.local_addr.to_string(),
            peer,
            tcp_active: s.tcp.active.load(Ordering::Relaxed),
            tcp_total: s.tcp.total.load(Ordering::Relaxed),
            tcp_rejected: s.tcp.rejected.load(Ordering::Relaxed),
            dns_queries: s.dns_queries.load(Ordering::Relaxed),
            udp_flows: s.udp_flows.load(Ordering::Relaxed),
            udp_relayed: s.udp_relayed.load(Ordering::Relaxed),
            quic_dropped: s.quic_dropped.load(Ordering::Relaxed),
            dropped_packets: s.dropped_packets.load(Ordering::Relaxed),
        }
    }

    /// Stop: reset all tunnelled connections and close the socket.
    pub async fn stop(mut self) {
        self.shutdown.cancel();
        if let Some(task) = self.task.take() {
            let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
        }
        tracing::info!(addr = %self.local_addr, "WireGuard server stopped");
    }
}

impl Drop for WireguardServer {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

/// Run a boringtun call with its own logging muted: it reports routine protocol events (e.g.
/// handshake retries towards a device that went away) at WARN level, every few seconds.
/// The server logs what matters itself (device connected / session expired).
fn quiet<R>(f: impl FnOnce() -> R) -> R {
    static MUTED: std::sync::LazyLock<tracing::Dispatch> =
        std::sync::LazyLock::new(tracing::Dispatch::none);
    tracing::dispatcher::with_default(&MUTED, f)
}

fn bind_udp(addr: SocketAddr) -> std::io::Result<UdpSocket> {
    let socket = Socket::new(Domain::for_address(addr), Type::DGRAM, Some(Protocol::UDP))?;
    if addr.is_ipv6() && addr.ip().is_unspecified() {
        // Dual-stack: `::` also serves IPv4 devices.
        let _ = socket.set_only_v6(false);
    }
    // Bursts of full-size segments towards a phone shouldn't be dropped by small defaults.
    let _ = socket.set_recv_buffer_size(4 << 20);
    let _ = socket.set_send_buffer_size(4 << 20);
    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;
    UdpSocket::from_std(socket.into())
}

struct Worker {
    udp: Arc<UdpSocket>,
    tunn: Box<Tunn>,
    /// Where the peer's authenticated packets last came from (WireGuard roaming).
    endpoint: Option<SocketAddr>,
    stack: Stack,
    relay: UdpRelay,
    replies_rx: mpsc::Receiver<Reply>,
    replies_tx: mpsc::Sender<Reply>,
    handler: Arc<dyn TcpHandler>,
    dns: Arc<dyn DnsBackend>,
    dns_permits: Arc<Semaphore>,
    wake: Arc<Notify>,
    shared: Arc<Shared>,
    shutdown: CancellationToken,
    mtu: usize,
    /// Scratch buffer for boringtun output.
    buf: Vec<u8>,
}

impl Worker {
    async fn run(mut self) {
        let mut datagram = vec![0u8; 65536];
        let mut timers = tokio::time::interval(TIMER_TICK);
        timers.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            let delay = self
                .stack
                .poll_delay()
                .unwrap_or(Duration::from_secs(1))
                .min(Duration::from_secs(1));
            tokio::select! {
                // Timers and replies first: a saturated tunnel must not starve WireGuard's
                // timers (keepalives, rekeying, rate limiting) or DNS answers.
                biased;
                _ = self.shutdown.cancelled() => break,
                _ = timers.tick() => self.on_timer().await,
                Some(reply) = self.replies_rx.recv() => {
                    self.send_reply(reply).await;
                    for _ in 0..RECV_BATCH {
                        match self.replies_rx.try_recv() {
                            Ok(reply) => self.send_reply(reply).await,
                            Err(_) => break,
                        }
                    }
                }
                r = self.udp.recv_from(&mut datagram) => {
                    match r {
                        Ok((n, from)) => self.on_datagram(&datagram[..n], from).await,
                        Err(e) => {
                            // Windows reports ICMP port-unreachable as a receive error.
                            tracing::debug!(error = %e, "WireGuard socket receive error");
                            if !matches!(e.kind(), std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::Interrupted) {
                                tokio::time::sleep(Duration::from_millis(20)).await;
                            }
                        }
                    }
                    for _ in 0..RECV_BATCH {
                        match self.udp.try_recv_from(&mut datagram) {
                            Ok((n, from)) => self.on_datagram(&datagram[..n], from).await,
                            Err(_) => break,
                        }
                    }
                }
                _ = self.wake.notified() => {}
                _ = tokio::time::sleep(delay) => {}
            }
            self.pump().await;
        }
        // Stopping: reset every connection so the device and the proxy notice at once.
        self.stack.abort_all();
        self.flush_stack().await;
        self.relay.close_all();
        self.shared.udp_flows.store(0, Ordering::Relaxed);
    }

    /// Decrypt one datagram and act on its content.
    async fn on_datagram(&mut self, data: &[u8], from: SocketAddr) {
        let mut buf = std::mem::take(&mut self.buf);
        let tunn = &mut self.tunn;
        match quiet(|| tunn.decapsulate(Some(from.ip()), data, &mut buf)) {
            TunnResult::WriteToNetwork(reply) => {
                // Handshake response (type 2): the initiation was authenticated.
                if reply.first() == Some(&2) {
                    self.endpoint = Some(from);
                    self.seen();
                }
                let _ = self.udp.send_to(reply, from).await;
                // Flush packets queued while there was no session.
                while let TunnResult::WriteToNetwork(p) =
                    quiet(|| self.tunn.decapsulate(None, &[], &mut buf))
                {
                    let _ = self.udp.send_to(p, from).await;
                }
            }
            TunnResult::WriteToTunnelV4(packet, _) | TunnResult::WriteToTunnelV6(packet, _) => {
                self.endpoint = Some(from);
                self.seen();
                let packet = packet.to_vec();
                self.on_ip_packet(packet);
            }
            TunnResult::Done => {
                // Authenticated keepalive.
                self.endpoint = Some(from);
                self.seen();
            }
            TunnResult::Err(e) => {
                tracing::trace!(%from, error = ?e, "WireGuard: dropped datagram");
            }
        }
        self.buf = buf;
    }

    fn seen(&self) {
        self.shared.last_seen_at.store(now_ms(), Ordering::Relaxed);
    }

    /// Route one decrypted IP packet from the device.
    fn on_ip_packet(&mut self, packet: Vec<u8>) {
        let classified = packet::classify(&packet);
        let (src, dst, payload) = match classified {
            Classified::Tcp { src, dst, syn } => {
                self.stack.ingest(packet, src, dst, syn);
                return;
            }
            Classified::Udp { src, dst, payload } => (src, dst, payload),
            Classified::Other(why) => {
                tracing::trace!(why, "WireGuard: dropped packet");
                self.shared.dropped_packets.fetch_add(1, Ordering::Relaxed);
                return;
            }
        };
        if dst.ip() == IpAddr::V4(DNS_ADDRESS) && dst.port() == 53 {
            self.dns_query(src, dst, payload.to_vec());
        } else if dst.ip() == IpAddr::V4(DNS_ADDRESS) || dst.ip() == IpAddr::V4(SERVER_ADDRESS) {
            self.shared.dropped_packets.fetch_add(1, Ordering::Relaxed);
        } else if dst.port() == 443 {
            // QUIC / HTTP/3: dropping it makes clients fall back to TCP, which is intercepted.
            self.shared.quic_dropped.fetch_add(1, Ordering::Relaxed);
        } else {
            self.relay.send(src, dst, payload);
            self.shared
                .udp_flows
                .store(self.relay.flow_count() as u64, Ordering::Relaxed);
        }
    }

    fn dns_query(&self, src: SocketAddr, dst: SocketAddr, query: Vec<u8>) {
        self.shared.dns_queries.fetch_add(1, Ordering::Relaxed);
        let Ok(permit) = self.dns_permits.clone().try_acquire_owned() else {
            tracing::debug!("WireGuard: too many DNS queries in flight; dropping one");
            return;
        };
        let backend = self.dns.clone();
        let replies = self.replies_tx.clone();
        let limit = dns::udp_limit(&query, self.mtu);
        tokio::spawn(async move {
            let _permit = permit;
            if let Some(answer) = dns::respond(backend.as_ref(), &query, limit).await {
                let _ = replies
                    .send(Reply {
                        from: dst,
                        to: src,
                        payload: answer,
                    })
                    .await;
            }
        });
    }

    async fn send_reply(&mut self, reply: Reply) {
        if let Some(packet) = packet::udp_packet(reply.from, reply.to, &reply.payload) {
            self.send_to_peer(&packet).await;
        }
    }

    /// Run the TCP stack, dispatch new connections and send what it emitted.
    async fn pump(&mut self) {
        for stream in self.stack.poll() {
            self.dispatch(stream);
        }
        self.flush_stack().await;
    }

    fn dispatch(&self, stream: TcpStream) {
        let dst = stream.local_addr();
        if dst.ip() == IpAddr::V4(DNS_ADDRESS) {
            if dst.port() == 53 {
                tokio::spawn(dns::serve_tcp(stream, self.dns.clone()));
            } else {
                stream
                    .reset_flag()
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
            return;
        }
        self.handler.handle(stream);
    }

    async fn flush_stack(&mut self) {
        while let Some(packet) = self.stack.device.tx.pop_front() {
            self.send_to_peer(&packet).await;
        }
    }

    /// Encrypt an IP packet and send it to the peer's current endpoint.
    async fn send_to_peer(&mut self, packet: &[u8]) {
        let Some(endpoint) = self.endpoint else {
            return;
        };
        let mut buf = std::mem::take(&mut self.buf);
        if buf.len() < packet.len() + 148 {
            buf.resize(packet.len() + 148, 0);
        }
        let tunn = &mut self.tunn;
        match quiet(|| tunn.encapsulate(packet, &mut buf)) {
            TunnResult::WriteToNetwork(datagram) => {
                let _ = self.udp.send_to(datagram, endpoint).await;
            }
            TunnResult::Err(e) => tracing::trace!(error = ?e, "WireGuard: encapsulation failed"),
            _ => {}
        }
        self.buf = buf;
    }

    async fn on_timer(&mut self) {
        let mut buf = std::mem::take(&mut self.buf);
        let tunn = &mut self.tunn;
        if let TunnResult::WriteToNetwork(datagram) = quiet(|| tunn.update_timers(&mut buf))
            && let Some(endpoint) = self.endpoint
        {
            let _ = self.udp.send_to(datagram, endpoint).await;
        }
        self.buf = buf;
        self.relay.sweep();
        self.shared
            .udp_flows
            .store(self.relay.flow_count() as u64, Ordering::Relaxed);
        let (since_handshake, tx_bytes, rx_bytes, _, _) = self.tunn.stats();
        let mut peer = self.shared.peer.lock();
        peer.endpoint = self.endpoint.map(|e| e.to_string());
        peer.last_handshake_at =
            since_handshake.map(|d| now_ms().saturating_sub(d.as_millis() as u64));
        let connected = since_handshake.is_some_and(|d| d < SESSION_LIFETIME);
        if connected != peer.connected {
            match (connected, self.endpoint) {
                (true, Some(from)) => tracing::info!(%from, "WireGuard: device connected"),
                (true, None) => tracing::info!("WireGuard: device connected"),
                (false, _) => tracing::info!("WireGuard: device session expired"),
            }
        }
        peer.connected = connected;
        peer.rx_bytes = rx_bytes as u64;
        peer.tx_bytes = tx_bytes as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Metadata, Subscriber};

    struct Count(Arc<AtomicUsize>);

    impl Subscriber for Count {
        fn enabled(&self, _: &Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _: &Attributes<'_>) -> Id {
            Id::from_u64(1)
        }
        fn record(&self, _: &Id, _: &Record<'_>) {}
        fn record_follows_from(&self, _: &Id, _: &Id) {}
        fn event(&self, _: &Event<'_>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
        fn enter(&self, _: &Id) {}
        fn exit(&self, _: &Id) {}
    }

    #[test]
    fn boringtun_logging_is_muted() {
        let n = Arc::new(AtomicUsize::new(0));
        tracing::subscriber::with_default(Count(n.clone()), || {
            tracing::warn!("visible");
            quiet(|| tracing::warn!("HANDSHAKE(REKEY_TIMEOUT)"));
            tracing::warn!("visible again");
        });
        assert_eq!(n.load(Ordering::Relaxed), 2);
    }
}
