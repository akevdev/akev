//! # akev-wireguard
//!
//! Akev's **WireGuard mode**: an embedded, userspace WireGuard server. A phone (or any
//! machine) imports the client configuration — by scanning a QR code with the official
//! WireGuard app — and from then on *all* of its TCP traffic arrives in Akev with the
//! original destination `IP:port` known, exactly like the inside of a CONNECT tunnel:
//! HTTP is parsed, TLS is MITM'd using the client's SNI (or passed through), and anything
//! else is tunnelled raw to the real destination. No proxy settings, no root, no OS
//! networking changes on either side.
//!
//! The design follows mitmproxy's WireGuard mode (`mitmproxy_rs`):
//!
//! * [`boringtun`]'s `noise::Tunn` implements the protocol (handshake, ChaCha20-Poly1305
//!   transport, timers, cookies under load) for the single client peer.
//! * Decrypted IP packets go to a [`smoltcp`] interface that accepts *any* destination
//!   address; a listening socket is created on the destination of every new SYN, so each
//!   TCP connection terminates locally and becomes a [`TcpStream`] (`local_addr()` = the
//!   original destination). A [`TcpHandler`] — normally [`ProxyBridge`] — serves it.
//! * UDP is handled here: DNS queries to [`DNS_ADDRESS`] are answered by resolving them on
//!   this machine ([`dns`]), QUIC (UDP 443) is dropped so clients fall back to TCP, and other
//!   datagrams are relayed through ordinary UDP sockets. ICMP is dropped.
//!
//! Keys are generated once and persisted ([`keys`]); [`config::client_config`] renders the
//! configuration (the QR code payload).

#![warn(missing_docs)]

mod bridge;
pub mod config;
mod device;
pub mod dns;
pub mod keys;
mod packet;
mod server;
mod stack;
mod stream;
#[doc(hidden)]
pub mod testing;
mod udp;

use std::net::Ipv4Addr;

pub use bridge::ProxyBridge;
pub use config::client_config;
pub use keys::{KeyError, PrivateKey, PublicKeyBytes, WireguardKeys};
pub use server::{PeerStatus, ServerOptions, ServerStatus, WireguardServer};
pub use stream::TcpStream;

/// The device's address inside the tunnel.
pub const CLIENT_ADDRESS: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 2);
/// Akev's DNS relay inside the tunnel.
pub const DNS_ADDRESS: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 53);
/// Akev's own address inside the tunnel (`akev.cert` resolves to it).
pub const SERVER_ADDRESS: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 1);
/// The magic host serving Akev's setup page (CA certificate downloads).
pub const MAGIC_HOST: &str = "akev.cert";
/// Interceptor id recorded on tunnelled exchanges (`ClientInfo::interceptor`).
pub const INTERCEPTOR_ID: &str = "wireguard";
/// Default MTU of packets sent into the tunnel (the WireGuard apps' default interface MTU).
pub const TUNNEL_MTU: usize = 1420;

/// Serves the TCP connections of the tunnelled device.
pub trait TcpHandler: Send + Sync + 'static {
    /// Called on the server task for every new, established connection. Must not block:
    /// spawn a task to serve the stream.
    fn handle(&self, stream: TcpStream);
}
