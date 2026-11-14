//! HTTP/3 on the inbound side: Akev's own QUIC listener (UDP, by default on the proxy's port
//! number, off unless `ProxySettings::http3.listen`) and the public building blocks to serve
//! HTTP/3 on any QUIC endpoint — e.g. one fed by a userspace network stack that hands over
//! UDP/443 flows ([`ProxyHandle::serve_quic`]).
//!
//! Requests arriving over HTTP/3 are recorded with `HttpVersion::H3`, go through the same
//! pipeline as every other exchange and are answered over HTTP/3. Like direct (transparent /
//! reverse proxy) TCP requests, their destination is the `:authority` of the request (or, for
//! [`ProxyHandle::serve_quic`] with an original destination, that address — the authority then
//! only names the host).

mod server;

use crate::server::parse_bind_ip;
use crate::shared::Shared;
use crate::{ProxyError, ProxyHandle};
use akev_ca::CertificateAuthority;
use akev_core::settings::ProxySettings;
use parking_lot::Mutex;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

pub(crate) use server::H3Responder;

/// IPv4-mapped IPv6 addresses as IPv4.
pub(crate) fn canonical_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        v4 => v4,
    }
}

/// State of Akev's HTTP/3 (QUIC) listener.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Http3ListenerStatus {
    /// `ProxySettings::http3.listen`.
    pub enabled: bool,
    /// UDP address it listens on (when running).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    /// UDP port it listens on (when running).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Why it isn't running although enabled (e.g. the UDP port is in use).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

struct Running {
    addr: SocketAddr,
    endpoint: quinn::Endpoint,
}

#[derive(Default)]
struct ListenerState {
    /// Where the settings want the listener (`None` = disabled).
    desired: Option<SocketAddr>,
    running: Option<Running>,
    error: Option<String>,
}

/// Akev's own QUIC listener, following the settings.
pub(crate) struct QuicListener {
    configs: Arc<server::ServerConfigs>,
    state: Arc<Mutex<ListenerState>>,
}

/// Binding again after "address in use": a socket that was just closed (rebind on the same
/// port, proxy restart) is released asynchronously, once its endpoint has wound down.
const BIND_RETRIES: u32 = 20;
const BIND_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(250);

/// Bind a UDP socket (`::` is dual-stack).
fn bind_udp(addr: SocketAddr) -> std::io::Result<std::net::UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let socket = Socket::new(Domain::for_address(addr), Type::DGRAM, Some(Protocol::UDP))?;
    if addr.is_ipv6() && addr.ip().is_unspecified() {
        let _ = socket.set_only_v6(false);
    }
    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;
    Ok(socket.into())
}

/// A quinn endpoint serving QUIC on `socket`.
fn endpoint_on(
    configs: &server::ServerConfigs,
    socket: std::net::UdpSocket,
) -> Result<quinn::Endpoint, String> {
    let local = socket.local_addr().ok().map(|a| a.ip());
    let config = configs.get(local)?;
    let runtime = quinn::default_runtime().ok_or("no async runtime to run the QUIC endpoint")?;
    quinn::Endpoint::new(
        quinn::EndpointConfig::default(),
        Some(config),
        socket,
        runtime,
    )
    .map_err(|e| e.to_string())
}

/// Start listening on `addr`. Returns whether a later attempt may succeed (address in use).
fn start_locked(
    shared: &Arc<Shared>,
    configs: &Arc<server::ServerConfigs>,
    st: &mut ListenerState,
    addr: SocketAddr,
) -> bool {
    let socket = match bind_udp(addr) {
        Ok(s) => s,
        Err(e) => {
            st.error = Some(format!("cannot listen on UDP {addr}: {e}"));
            return e.kind() == std::io::ErrorKind::AddrInUse;
        }
    };
    match endpoint_on(configs, socket) {
        Ok(endpoint) => {
            let actual = endpoint.local_addr().unwrap_or(addr);
            tracing::info!(addr = %actual, "HTTP/3 (QUIC) listening");
            let task =
                server::serve_endpoint(shared.clone(), configs.clone(), endpoint.clone(), None);
            shared.tracker.spawn(task);
            st.running = Some(Running {
                addr: actual,
                endpoint,
            });
            st.error = None;
            false
        }
        Err(e) => {
            st.error = Some(e);
            false
        }
    }
}

/// Keep trying to bind `addr` for a while (until the settings want something else).
fn spawn_retries(
    shared: Arc<Shared>,
    configs: Arc<server::ServerConfigs>,
    state: Arc<Mutex<ListenerState>>,
    addr: SocketAddr,
) {
    let Ok(rt) = tokio::runtime::Handle::try_current() else {
        return;
    };
    rt.spawn(async move {
        for _ in 0..BIND_RETRIES {
            tokio::select! {
                _ = tokio::time::sleep(BIND_RETRY_DELAY) => {}
                _ = shared.shutdown.cancelled() => return,
            }
            let mut st = state.lock();
            if st.desired != Some(addr) || st.running.is_some() {
                return;
            }
            if !start_locked(&shared, &configs, &mut st, addr) {
                return;
            }
        }
    });
}

impl QuicListener {
    pub(crate) fn new(ca: Arc<CertificateAuthority>) -> Self {
        QuicListener {
            configs: Arc::new(server::ServerConfigs::new(ca)),
            state: Arc::new(Mutex::new(ListenerState::default())),
        }
    }

    /// Start, move or stop the listener to match `settings` (`proxy_port`: the TCP port the
    /// proxy listens on, used when `listen_port` is 0). A port in use is retried for a few
    /// seconds, and on every later call.
    pub(crate) fn reconcile(
        &self,
        shared: &Arc<Shared>,
        settings: &ProxySettings,
        proxy_port: u16,
    ) {
        let mut st = self.state.lock();
        let desired = if settings.http3.listen {
            match parse_bind_ip(&settings.bind_address) {
                Ok(ip) => {
                    let port = match settings.http3.listen_port {
                        0 => proxy_port,
                        p => p,
                    };
                    Some(SocketAddr::new(ip, port))
                }
                Err(e) => {
                    Self::stop_locked(&mut st);
                    st.desired = None;
                    st.error = Some(format!("invalid bind address: {e}"));
                    return;
                }
            }
        } else {
            None
        };
        if st.desired == desired && (desired.is_none() || st.running.is_some()) {
            return;
        }
        Self::stop_locked(&mut st);
        st.desired = desired;
        st.error = None;
        let Some(addr) = desired else { return };
        if start_locked(shared, &self.configs, &mut st, addr) {
            tracing::warn!(error = ?st.error, "HTTP/3 listener not started yet; retrying");
            spawn_retries(
                shared.clone(),
                self.configs.clone(),
                self.state.clone(),
                addr,
            );
        } else if let Some(e) = &st.error {
            tracing::warn!(error = %e, "HTTP/3 listener not started");
        }
    }

    fn stop_locked(st: &mut ListenerState) {
        if let Some(running) = st.running.take() {
            running
                .endpoint
                .close(quinn::VarInt::from_u32(0x100), b"listener stopped");
        }
    }

    /// Close the listener (proxy shutdown).
    pub(crate) fn stop(&self) {
        let mut st = self.state.lock();
        Self::stop_locked(&mut st);
        st.desired = None;
    }

    fn status(&self, enabled: bool) -> Http3ListenerStatus {
        let st = self.state.lock();
        Http3ListenerStatus {
            enabled,
            address: st.running.as_ref().map(|r| r.addr.to_string()),
            port: st.running.as_ref().map(|r| r.addr.port()),
            error: if enabled { st.error.clone() } else { None },
        }
    }
}

impl ProxyHandle {
    /// State of the HTTP/3 (QUIC) listener (`ProxySettings::http3.listen`).
    pub fn http3_status(&self) -> Http3ListenerStatus {
        let enabled = self.inner.shared.settings().proxy.http3.listen;
        self.inner.quic.status(enabled)
    }

    /// A QUIC server configuration presenting Akev's certificates: a leaf certificate for the
    /// SNI of each connection (for `local_ip`, or `localhost`, without SNI), TLS 1.3, ALPN `h3`.
    pub fn quic_server_config(
        &self,
        local_ip: Option<IpAddr>,
    ) -> std::result::Result<quinn::ServerConfig, ProxyError> {
        self.inner
            .quic
            .configs
            .get(local_ip)
            .map_err(ProxyError::Other)
    }

    /// Serve HTTP/3 on a QUIC endpoint created by the caller — for instance with
    /// [`quinn::Endpoint::new_with_abstract_socket`] over a userspace network stack that hands
    /// over the UDP/443 flows of a VPN / transparent capture. Every connection is accepted with
    /// Akev's certificates (see [`quic_server_config`](Self::quic_server_config); the
    /// endpoint's own server configuration is not used) and every request becomes an exchange
    /// recorded as HTTP/3. `original_destination`: where the client's packets were going, used
    /// as the request target (the `:authority` then only names the host, as with a CONNECT
    /// tunnel to an IP). Returns once the endpoint is closed or the proxy shuts down; accepted
    /// connections keep being served until they end.
    pub async fn serve_quic(
        &self,
        endpoint: quinn::Endpoint,
        original_destination: Option<SocketAddr>,
    ) {
        let shared = self.inner.shared.clone();
        let configs = self.inner.quic.configs.clone();
        server::serve_endpoint(shared, configs, endpoint, original_destination).await;
    }

    /// Like [`serve_quic`](Self::serve_quic) on a UDP socket bound by the caller.
    pub async fn serve_quic_socket(
        &self,
        socket: std::net::UdpSocket,
        original_destination: Option<SocketAddr>,
    ) -> std::result::Result<(), ProxyError> {
        socket.set_nonblocking(true)?;
        let endpoint = endpoint_on(&self.inner.quic.configs, socket).map_err(ProxyError::Other)?;
        self.serve_quic(endpoint.clone(), original_destination)
            .await;
        endpoint.close(quinn::VarInt::from_u32(0x100), b"");
        Ok(())
    }
}
