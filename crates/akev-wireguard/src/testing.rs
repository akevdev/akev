//! A minimal WireGuard *client* for tests: boringtun for the protocol and a smoltcp
//! interface (`10.0.0.2`) for TCP, exactly what a phone running the WireGuard app does.
//! TCP connections are exposed as Tokio duplex streams; UDP datagrams can be sent and
//! received. Not part of the public API.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::keys::WireguardKeys;
use crate::packet::{self, Classified};
use boringtun::noise::{Tunn, TunnResult};
use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet};
use smoltcp::phy::{self, DeviceCapabilities, Medium};
use smoltcp::socket::tcp;
use smoltcp::time::Instant;
use smoltcp::wire::{HardwareAddress, IpAddress, IpCidr, Ipv4Address};
use std::collections::{HashMap, VecDeque};
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot};

/// A UDP datagram received through the tunnel.
#[derive(Debug)]
pub struct Datagram {
    /// Sender (inside the tunnel's address space).
    pub from: SocketAddr,
    /// Local port it was addressed to.
    pub to_port: u16,
    /// Payload.
    pub payload: Vec<u8>,
}

enum Cmd {
    Connect {
        dst: SocketAddr,
        ready: oneshot::Sender<io::Result<DuplexStream>>,
    },
    Data {
        id: u64,
        data: Vec<u8>,
    },
    Shutdown {
        id: u64,
    },
    Udp {
        src_port: u16,
        dst: SocketAddr,
        payload: Vec<u8>,
    },
    RawIp(Vec<u8>),
    Handshake(oneshot::Sender<()>),
}

/// A tunnelled test device.
pub struct TestClient {
    cmds: mpsc::UnboundedSender<Cmd>,
    udp_rx: mpsc::UnboundedReceiver<Datagram>,
    _task: tokio::task::JoinHandle<()>,
}

impl TestClient {
    /// Connect to the server at `server` with the client half of `keys`.
    pub async fn start(server: SocketAddr, keys: &WireguardKeys) -> io::Result<TestClient> {
        let bind: SocketAddr = if server.is_ipv4() {
            "127.0.0.1:0".parse().unwrap()
        } else {
            "[::1]:0".parse().unwrap()
        };
        let socket = socket2::Socket::new(
            socket2::Domain::for_address(bind),
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )?;
        let _ = socket.set_recv_buffer_size(4 << 20);
        socket.set_nonblocking(true)?;
        socket.bind(&bind.into())?;
        let udp = UdpSocket::from_std(socket.into())?;
        udp.connect(server).await?;
        let tunn = Tunn::new(
            keys.client.secret(),
            keys.server.public_key().key(),
            None,
            None,
            rand::random::<u32>() >> 8,
            None,
        );
        let mut device = Queues::default();
        let mut iface = Interface::new(
            Config::new(HardwareAddress::Ip),
            &mut device,
            Instant::now(),
        );
        iface.update_ip_addrs(|a| {
            let _ = a.push(IpCidr::new(IpAddress::Ipv4(crate::CLIENT_ADDRESS), 32));
        });
        iface
            .routes_mut()
            .add_default_ipv4_route(Ipv4Address::new(10, 0, 0, 1))
            .expect("route");
        let (cmds, cmd_rx) = mpsc::unbounded_channel();
        let (udp_tx, udp_rx) = mpsc::unbounded_channel();
        let driver = Driver {
            udp,
            tunn: Box::new(tunn),
            iface,
            device,
            sockets: SocketSet::new(Vec::new()),
            conns: HashMap::new(),
            next_id: 1,
            next_port: 40000,
            cmds_tx: cmds.clone(),
            udp_tx,
            handshake_waiters: Vec::new(),
        };
        let task = tokio::spawn(driver.run(cmd_rx));
        Ok(TestClient {
            cmds,
            udp_rx,
            _task: task,
        })
    }

    /// Complete a WireGuard handshake (fails after `timeout`).
    pub async fn handshake(&self, timeout: Duration) -> io::Result<()> {
        let (tx, rx) = oneshot::channel();
        let _ = self.cmds.send(Cmd::Handshake(tx));
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(())) => Ok(()),
            _ => Err(io::Error::new(io::ErrorKind::TimedOut, "no handshake")),
        }
    }

    /// Open a TCP connection to `dst` through the tunnel.
    pub async fn connect(&self, dst: SocketAddr) -> io::Result<DuplexStream> {
        let (tx, rx) = oneshot::channel();
        let _ = self.cmds.send(Cmd::Connect { dst, ready: tx });
        match tokio::time::timeout(Duration::from_secs(15), rx).await {
            Ok(Ok(r)) => r,
            _ => Err(io::Error::new(io::ErrorKind::TimedOut, "connect timed out")),
        }
    }

    /// Send a UDP datagram from `10.0.0.2:src_port` to `dst`.
    pub fn send_udp(&self, src_port: u16, dst: SocketAddr, payload: &[u8]) {
        let _ = self.cmds.send(Cmd::Udp {
            src_port,
            dst,
            payload: payload.to_vec(),
        });
    }

    /// Send a raw IP packet into the tunnel.
    pub fn send_ip(&self, packet: Vec<u8>) {
        let _ = self.cmds.send(Cmd::RawIp(packet));
    }

    /// The next UDP datagram received (within `timeout`).
    pub async fn recv_udp(&mut self, timeout: Duration) -> Option<Datagram> {
        tokio::time::timeout(timeout, self.udp_rx.recv())
            .await
            .ok()
            .flatten()
    }
}

#[derive(Default)]
struct Queues {
    rx: VecDeque<Vec<u8>>,
    tx: VecDeque<Vec<u8>>,
}

impl phy::Device for Queues {
    type RxToken<'a> = Rx;
    type TxToken<'a> = Tx<'a>;
    fn receive(&mut self, _: Instant) -> Option<(Rx, Tx<'_>)> {
        let p = self.rx.pop_front()?;
        Some((Rx(p), Tx(&mut self.tx)))
    }
    fn transmit(&mut self, _: Instant) -> Option<Tx<'_>> {
        Some(Tx(&mut self.tx))
    }
    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ip;
        caps.max_transmission_unit = crate::TUNNEL_MTU;
        caps
    }
}

struct Rx(Vec<u8>);
impl phy::RxToken for Rx {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        f(&self.0)
    }
}

struct Tx<'a>(&'a mut VecDeque<Vec<u8>>);
impl phy::TxToken for Tx<'_> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let mut p = vec![0u8; len];
        let r = f(&mut p);
        self.0.push_back(p);
        r
    }
}

struct ClientConn {
    handle: SocketHandle,
    ready: Option<(oneshot::Sender<io::Result<DuplexStream>>, DuplexStream)>,
    to_app: Option<mpsc::UnboundedSender<Vec<u8>>>,
    outgoing: VecDeque<u8>,
    shutdown: bool,
    closed: bool,
}

struct Driver {
    udp: UdpSocket,
    tunn: Box<Tunn>,
    iface: Interface,
    device: Queues,
    sockets: SocketSet<'static>,
    conns: HashMap<u64, ClientConn>,
    next_id: u64,
    next_port: u16,
    cmds_tx: mpsc::UnboundedSender<Cmd>,
    udp_tx: mpsc::UnboundedSender<Datagram>,
    handshake_waiters: Vec<oneshot::Sender<()>>,
}

impl Driver {
    async fn run(mut self, mut cmds: mpsc::UnboundedReceiver<Cmd>) {
        let mut buf = vec![0u8; 65536];
        let mut tick = tokio::time::interval(Duration::from_millis(50));
        loop {
            tokio::select! {
                r = self.udp.recv(&mut buf) => match r {
                    Ok(n) => self.on_datagram(buf[..n].to_vec()).await,
                    Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
                },
                c = cmds.recv() => match c {
                    Some(c) => self.on_cmd(c).await,
                    None => return,
                },
                _ = tick.tick() => {
                    let mut out = vec![0u8; 2048];
                    if let TunnResult::WriteToNetwork(p) = self.tunn.update_timers(&mut out) {
                        let _ = self.udp.send(p).await;
                    }
                }
            }
            self.pump().await;
            if self.tunn.stats().0.is_some() {
                for w in self.handshake_waiters.drain(..) {
                    let _ = w.send(());
                }
            }
        }
    }

    async fn on_datagram(&mut self, data: Vec<u8>) {
        let mut out = vec![0u8; 65536];
        match self.tunn.decapsulate(None, &data, &mut out) {
            TunnResult::WriteToNetwork(p) => {
                let _ = self.udp.send(p).await;
                let mut more = vec![0u8; 65536];
                while let TunnResult::WriteToNetwork(p) =
                    self.tunn.decapsulate(None, &[], &mut more)
                {
                    let _ = self.udp.send(p).await;
                }
            }
            TunnResult::WriteToTunnelV4(p, _) | TunnResult::WriteToTunnelV6(p, _) => {
                let p = p.to_vec();
                let udp = match packet::classify(&p) {
                    Classified::Udp { src, dst, payload } => Some(Datagram {
                        from: src,
                        to_port: dst.port(),
                        payload: payload.to_vec(),
                    }),
                    _ => None,
                };
                match udp {
                    Some(d) => {
                        let _ = self.udp_tx.send(d);
                    }
                    None => self.device.rx.push_back(p),
                }
            }
            _ => {}
        }
    }

    async fn on_cmd(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Handshake(done) => {
                self.handshake_waiters.push(done);
                let mut out = vec![0u8; 2048];
                if let TunnResult::WriteToNetwork(p) =
                    self.tunn.format_handshake_initiation(&mut out, false)
                {
                    let _ = self.udp.send(p).await;
                }
            }
            Cmd::Connect { dst, ready } => {
                let mut socket = tcp::Socket::new(
                    tcp::SocketBuffer::new(vec![0u8; 256 * 1024]),
                    tcp::SocketBuffer::new(vec![0u8; 256 * 1024]),
                );
                let port = self.next_port;
                self.next_port = self.next_port.wrapping_add(1).max(40000);
                let remote = (IpAddress::from(dst.ip()), dst.port());
                if let Err(e) = socket.connect(self.iface.context(), remote, port) {
                    let _ = ready.send(Err(io::Error::other(format!("{e:?}"))));
                    return;
                }
                let handle = self.sockets.add(socket);
                let (app, ours) = tokio::io::duplex(256 * 1024);
                let id = self.next_id;
                self.next_id += 1;
                self.conns.insert(
                    id,
                    ClientConn {
                        handle,
                        ready: Some((ready, app)),
                        to_app: None,
                        outgoing: VecDeque::new(),
                        shutdown: false,
                        closed: false,
                    },
                );
                // Bridge the duplex end to the driver.
                let (mut ours_r, mut ours_w) = tokio::io::split(ours);
                let (to_app, mut from_driver) = mpsc::unbounded_channel::<Vec<u8>>();
                self.conns.get_mut(&id).unwrap().to_app = Some(to_app);
                tokio::spawn(async move {
                    while let Some(chunk) = from_driver.recv().await {
                        if chunk.is_empty() {
                            let _ = ours_w.shutdown().await;
                            break;
                        }
                        if ours_w.write_all(&chunk).await.is_err() {
                            break;
                        }
                    }
                });
                let cmds = self.cmds_tx.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 16 * 1024];
                    loop {
                        match ours_r.read(&mut buf).await {
                            Ok(0) | Err(_) => {
                                let _ = cmds.send(Cmd::Shutdown { id });
                                break;
                            }
                            Ok(n) => {
                                let _ = cmds.send(Cmd::Data {
                                    id,
                                    data: buf[..n].to_vec(),
                                });
                            }
                        }
                    }
                });
            }
            Cmd::Data { id, data } => {
                if let Some(c) = self.conns.get_mut(&id) {
                    c.outgoing.extend(data);
                }
            }
            Cmd::Shutdown { id } => {
                if let Some(c) = self.conns.get_mut(&id) {
                    c.shutdown = true;
                }
            }
            Cmd::Udp {
                src_port,
                dst,
                payload,
            } => {
                let src = SocketAddr::new(IpAddr::V4(crate::CLIENT_ADDRESS), src_port);
                if let Some(p) = packet::udp_packet(src, dst, &payload) {
                    self.send_ip(&p).await;
                }
            }
            Cmd::RawIp(p) => self.send_ip(&p).await,
        }
    }

    async fn send_ip(&mut self, packet: &[u8]) {
        let mut out = vec![0u8; packet.len() + 256];
        if let TunnResult::WriteToNetwork(p) = self.tunn.encapsulate(packet, &mut out) {
            let _ = self.udp.send(p).await;
        }
    }

    async fn pump(&mut self) {
        self.iface
            .poll(Instant::now(), &mut self.device, &mut self.sockets);
        for c in self.conns.values_mut() {
            let s = self.sockets.get_mut::<tcp::Socket>(c.handle);
            if c.ready.is_some() && s.may_send() {
                let (ready, app) = c.ready.take().unwrap();
                let _ = ready.send(Ok(app));
            } else if c.ready.is_some() && s.state() == tcp::State::Closed {
                let (ready, _) = c.ready.take().unwrap();
                let _ = ready.send(Err(io::Error::new(
                    io::ErrorKind::ConnectionRefused,
                    "connection refused",
                )));
                c.closed = true;
            }
            while s.can_recv() {
                let mut chunk = Vec::new();
                let _ = s.recv(|b| {
                    chunk.extend_from_slice(b);
                    (b.len(), ())
                });
                if chunk.is_empty() {
                    break;
                }
                if let Some(tx) = &c.to_app {
                    let _ = tx.send(chunk);
                }
            }
            if !s.may_recv() && !c.closed && c.ready.is_none() {
                c.closed = true;
                if let Some(tx) = c.to_app.take() {
                    let _ = tx.send(Vec::new());
                }
            }
            while !c.outgoing.is_empty() && s.can_send() {
                let (a, _) = c.outgoing.as_slices();
                match s.send_slice(a) {
                    Ok(n) if n > 0 => {
                        c.outgoing.drain(..n);
                    }
                    _ => break,
                }
            }
            if c.shutdown && c.outgoing.is_empty() && s.may_send() {
                s.close();
            }
        }
        self.iface
            .poll(Instant::now(), &mut self.device, &mut self.sockets);
        while let Some(p) = self.device.tx.pop_front() {
            self.send_ip(&p).await;
        }
    }
}
