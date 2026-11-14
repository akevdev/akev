//! Hands the tunnelled device's TCP connections to the intercepting proxy.

use crate::{INTERCEPTOR_ID, TcpHandler, TcpStream};
use akev_core::ProxyMode;
use akev_proxy::{ProxyHandle, TunnelledConnection};

/// A [`TcpHandler`] that serves every connection through the proxy returned by `proxy` (a
/// function, because the proxy is replaced when e.g. the CA is regenerated). Connections
/// arriving while there's no proxy are closed.
pub struct ProxyBridge<F> {
    proxy: F,
}

impl<F> ProxyBridge<F>
where
    F: Fn() -> Option<ProxyHandle> + Send + Sync + 'static,
{
    /// A bridge to the proxy `proxy()` returns at the time of each connection.
    pub fn new(proxy: F) -> Self {
        ProxyBridge { proxy }
    }
}

impl<F> TcpHandler for ProxyBridge<F>
where
    F: Fn() -> Option<ProxyHandle> + Send + Sync + 'static,
{
    fn handle(&self, stream: TcpStream) {
        let Some(proxy) = (self.proxy)() else {
            return;
        };
        let connection = TunnelledConnection {
            client: stream.peer_addr(),
            destination: stream.local_addr(),
            via: ProxyMode::Wireguard,
            interceptor: Some(INTERCEPTOR_ID.to_string()),
            reset: stream.reset_flag(),
            stream: Box::new(stream),
        };
        proxy.serve_tunnelled(connection);
    }
}
