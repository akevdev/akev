//! The userspace TCP stack (smoltcp) terminating the device's TCP connections.
//!
//! Like mitmproxy's WireGuard mode, the interface accepts packets for *any* destination
//! (`any_ip` plus a catch-all `/0` address per family): when a SYN for a new 4-tuple arrives,
//! a socket listening on exactly that destination `IP:port` is created first, so the
//! handshake completes as if the device had reached the real server. Once established, the
//! connection becomes a [`TcpStream`] whose `local_addr` is the original destination.
//!
//! SYNs are processed one at a time (the interface is polled right after each new listener
//! is added), so concurrent connections to the same destination never race for the same
//! listening socket.

use crate::device::VirtualDevice;
use crate::stream::{STAGING_LIMIT, Shared, TcpStream, wake};
use bytes::Buf;
use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet};
use smoltcp::socket::tcp::{self, RecvError, State};
use smoltcp::time::{Duration as SmolDuration, Instant};
use smoltcp::wire::{HardwareAddress, IpAddress, IpCidr, IpEndpoint, IpListenEndpoint};
use std::collections::HashMap;
use std::net::{Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::Notify;

/// smoltcp receive buffer per connection (bounds the window advertised to the device).
const SOCKET_RX: usize = 128 * 1024;
/// smoltcp send buffer per connection (bytes in flight towards the device).
const SOCKET_TX: usize = 128 * 1024;
/// A connection that doesn't complete its handshake in time is dropped.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(20);
/// Unacknowledged data (or keep-alive probes) for this long aborts the connection.
const RETRANSMIT_TIMEOUT: SmolDuration = SmolDuration::from_secs(60);
/// Idle connections are probed this often, so a device that went away is noticed.
const KEEP_ALIVE: SmolDuration = SmolDuration::from_secs(30);
/// After the application closed a connection, the device gets this long to close its side.
const CLOSE_LINGER: Duration = Duration::from_secs(60);

/// Live TCP counters.
#[derive(Default)]
pub(crate) struct TcpStats {
    pub active: AtomicU64,
    pub total: AtomicU64,
    pub rejected: AtomicU64,
}

struct Conn {
    /// (device address, destination) this socket was created for.
    tuple: (SocketAddr, SocketAddr),
    created: std::time::Instant,
    /// Set once the handshake completed and the stream was handed out.
    stream: Option<Arc<Shared>>,
    fin_sent: bool,
    aborted: bool,
    dropped_at: Option<std::time::Instant>,
}

pub(crate) struct Stack {
    iface: Interface,
    pub device: VirtualDevice,
    sockets: SocketSet<'static>,
    conns: HashMap<SocketHandle, Conn>,
    /// (device address, destination) → socket, to recognise retransmitted SYNs.
    tuples: HashMap<(SocketAddr, SocketAddr), SocketHandle>,
    wake: Arc<Notify>,
    max_conns: usize,
    pub stats: Arc<TcpStats>,
}

fn endpoint_addr(ep: IpEndpoint) -> SocketAddr {
    SocketAddr::new(ep.addr.into(), ep.port)
}

impl Stack {
    pub(crate) fn new(mtu: usize, max_conns: usize, wake: Arc<Notify>) -> Self {
        let mut device = VirtualDevice::new(mtu);
        let mut config = Config::new(HardwareAddress::Ip);
        config.random_seed = rand::random();
        let mut iface = Interface::new(config, &mut device, Instant::now());
        iface.set_any_ip(true);
        iface.update_ip_addrs(|addrs| {
            // Catch-all prefixes: every destination is "on link", so replies need no routes.
            let _ = addrs.push(IpCidr::new(IpAddress::Ipv4(crate::SERVER_ADDRESS), 0));
            let _ = addrs.push(IpCidr::new(
                IpAddress::Ipv6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 1)),
                0,
            ));
        });
        Stack {
            iface,
            device,
            sockets: SocketSet::new(Vec::new()),
            conns: HashMap::new(),
            tuples: HashMap::new(),
            wake,
            max_conns,
            stats: Arc::default(),
        }
    }

    /// Feed one TCP/IP packet from the device. `syn` = SYN without ACK.
    pub(crate) fn ingest(&mut self, packet: Vec<u8>, src: SocketAddr, dst: SocketAddr, syn: bool) {
        if syn && self.should_listen(src, dst) {
            if self.conns.len() >= self.max_conns {
                // No listener: smoltcp answers the SYN with a RST.
                self.stats.rejected.fetch_add(1, Ordering::Relaxed);
                tracing::debug!(%src, %dst, "WireGuard: TCP connection limit reached");
            } else {
                self.listen(src, dst);
            }
            self.device.rx.push_back(packet);
            // Process this SYN now, while its listener is the only one for `dst`.
            self.iface
                .poll(Instant::now(), &mut self.device, &mut self.sockets);
            return;
        }
        self.device.rx.push_back(packet);
    }

    /// Whether a SYN for this 4-tuple needs a new listening socket.
    fn should_listen(&mut self, src: SocketAddr, dst: SocketAddr) -> bool {
        let Some(&h) = self.tuples.get(&(src, dst)) else {
            return true;
        };
        // A retransmitted SYN for a connection in progress goes to its socket; a SYN reusing
        // the 4-tuple of a finished connection starts a new one.
        let state = self.sockets.get::<tcp::Socket>(h).state();
        if matches!(state, State::TimeWait | State::Closed) {
            self.remove(h);
            true
        } else {
            false
        }
    }

    fn listen(&mut self, src: SocketAddr, dst: SocketAddr) {
        let mut socket = tcp::Socket::new(
            tcp::SocketBuffer::new(vec![0u8; SOCKET_RX]),
            tcp::SocketBuffer::new(vec![0u8; SOCKET_TX]),
        );
        socket.set_timeout(Some(RETRANSMIT_TIMEOUT));
        socket.set_keep_alive(Some(KEEP_ALIVE));
        // The proxy writes whole chunks; Nagle would only add latency.
        socket.set_nagle_enabled(false);
        if let Err(e) = socket.listen(IpListenEndpoint::from(dst)) {
            tracing::debug!(%dst, error = ?e, "WireGuard: can't listen");
            return;
        }
        let h = self.sockets.add(socket);
        self.conns.insert(
            h,
            Conn {
                tuple: (src, dst),
                created: std::time::Instant::now(),
                stream: None,
                fin_sent: false,
                aborted: false,
                dropped_at: None,
            },
        );
        self.tuples.insert((src, dst), h);
    }

    fn remove(&mut self, h: SocketHandle) {
        if let Some(conn) = self.conns.remove(&h) {
            if self.tuples.get(&conn.tuple) == Some(&h) {
                self.tuples.remove(&conn.tuple);
            }
            if let Some(shared) = conn.stream {
                shared.end();
                self.stats.active.fetch_sub(1, Ordering::Relaxed);
            }
        }
        self.sockets.remove(h);
    }

    /// Run the stack: process queued packets, move data between sockets and streams, emit
    /// packets (left in `device.tx`) and clean up. Returns the newly established connections.
    pub(crate) fn poll(&mut self) -> Vec<TcpStream> {
        self.iface
            .poll(Instant::now(), &mut self.device, &mut self.sockets);
        let new = self.service();
        self.iface
            .poll(Instant::now(), &mut self.device, &mut self.sockets);
        self.reap();
        new
    }

    /// When the stack next needs to run (retransmissions, keep-alives, delayed ACKs).
    pub(crate) fn poll_delay(&mut self) -> Option<Duration> {
        self.iface
            .poll_delay(Instant::now(), &self.sockets)
            .map(|d| Duration::from_micros(d.total_micros()))
    }

    fn service(&mut self) -> Vec<TcpStream> {
        let mut new = Vec::new();
        let mut doomed = Vec::new();
        for (&h, conn) in self.conns.iter_mut() {
            let socket = self.sockets.get_mut::<tcp::Socket>(h);
            if conn.stream.is_none() {
                match socket.state() {
                    // The SYN was refused, or the device reset the half-open connection.
                    State::Listen | State::Closed => doomed.push(h),
                    State::SynReceived => {
                        if conn.created.elapsed() > HANDSHAKE_TIMEOUT {
                            socket.abort();
                            conn.aborted = true;
                        }
                    }
                    _ => {
                        let (Some(remote), Some(local)) =
                            (socket.remote_endpoint(), socket.local_endpoint())
                        else {
                            doomed.push(h);
                            continue;
                        };
                        let shared = Shared::new(self.wake.clone());
                        conn.stream = Some(shared.clone());
                        self.stats.active.fetch_add(1, Ordering::Relaxed);
                        self.stats.total.fetch_add(1, Ordering::Relaxed);
                        new.push(TcpStream::new(
                            shared,
                            endpoint_addr(remote),
                            endpoint_addr(local),
                        ));
                    }
                }
            }
            if let Some(shared) = conn.stream.clone() {
                service_stream(socket, conn, &shared);
            }
        }
        for h in doomed {
            self.remove(h);
        }
        new
    }

    /// Remove sockets that finished (after the last poll emitted their final FIN/RST).
    fn reap(&mut self) {
        let closed: Vec<SocketHandle> = self
            .conns
            .keys()
            .copied()
            .filter(|&h| {
                let s = self.sockets.get::<tcp::Socket>(h);
                s.state() == State::Closed
            })
            .collect();
        for h in closed {
            self.remove(h);
        }
    }

    /// Abort every connection (server stopping): RSTs are left in `device.tx`.
    pub(crate) fn abort_all(&mut self) {
        for &h in self.conns.keys() {
            self.sockets.get_mut::<tcp::Socket>(h).abort();
        }
        self.iface
            .poll(Instant::now(), &mut self.device, &mut self.sockets);
        let all: Vec<SocketHandle> = self.conns.keys().copied().collect();
        for h in all {
            self.remove(h);
        }
    }
}

/// Move data between one socket and its stream; apply close/abort requests.
fn service_stream(socket: &mut tcp::Socket<'_>, conn: &mut Conn, shared: &Shared) {
    let (reader, writer) = {
        let mut st = shared.state.lock();
        let mut wake_reader = false;
        let mut wake_writer = false;

        if st.abort && !conn.aborted {
            socket.abort();
            conn.aborted = true;
        }
        if conn.aborted {
            return;
        }

        // Device → application.
        if st.dropped {
            while socket.can_recv() {
                if socket.recv(|b| (b.len(), ())).is_err() {
                    break;
                }
            }
            let since = *conn.dropped_at.get_or_insert_with(std::time::Instant::now);
            if since.elapsed() > CLOSE_LINGER && socket.state() != State::TimeWait {
                socket.abort();
                conn.aborted = true;
                return;
            }
        } else {
            while st.rx.len() < STAGING_LIMIT && socket.can_recv() {
                let room = STAGING_LIMIT - st.rx.len();
                let rx = &mut st.rx;
                match socket.recv(|b| {
                    let n = b.len().min(room);
                    rx.extend_from_slice(&b[..n]);
                    (n, n)
                }) {
                    Ok(n) if n > 0 => wake_reader = true,
                    _ => break,
                }
            }
            st.rx_pending = socket.can_recv();
        }
        if !st.rx_eof
            && !socket.may_recv()
            && matches!(socket.recv(|_| (0, ())), Err(RecvError::Finished))
        {
            st.rx_eof = true;
            wake_reader = true;
        }

        // Application → device.
        if !st.tx.is_empty()
            && socket.may_send()
            && let Ok(n) = socket.send_slice(&st.tx)
            && n > 0
        {
            st.tx.advance(n);
            wake_writer = true;
        }
        if (st.tx_shutdown || st.dropped) && st.tx.is_empty() && !conn.fin_sent {
            socket.close();
            conn.fin_sent = true;
        }
        (
            wake_reader.then(|| st.wake_reader()).flatten(),
            wake_writer.then(|| st.wake_writer()).flatten(),
        )
    };
    wake(reader);
    wake(writer);
}
