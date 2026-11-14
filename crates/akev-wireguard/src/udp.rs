//! UDP from the device that isn't DNS for the relay: QUIC is dropped (so clients fall back to
//! TCP, which Akev intercepts) and everything else is relayed through ordinary UDP sockets
//! of this machine (one per flow, NAT-style), so games, VoIP, NTP or DNS to other resolvers
//! keep working.

use parking_lot::Mutex;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// A relay flow without traffic in either direction for this long is closed.
const FLOW_IDLE: Duration = Duration::from_secs(60);

/// A datagram to send back into the tunnel: `from` (remote) → `to` (device).
#[derive(Debug)]
pub(crate) struct Reply {
    pub from: SocketAddr,
    pub to: SocketAddr,
    pub payload: Vec<u8>,
}

struct Flow {
    /// Sending side (a clone of the relay task's socket): a non-blocking `send` works at once,
    /// while Tokio's `try_send` reports `WouldBlock` until the reactor saw the socket writable.
    sender: std::net::UdpSocket,
    last_active: Arc<Mutex<Instant>>,
    stop: CancellationToken,
}

/// UDP relay state (owned by the server task).
pub(crate) struct UdpRelay {
    flows: HashMap<(SocketAddr, SocketAddr), Flow>,
    replies: mpsc::Sender<Reply>,
    max_flows: usize,
    shutdown: CancellationToken,
    pub relayed: Arc<AtomicU64>,
}

impl UdpRelay {
    pub(crate) fn new(
        replies: mpsc::Sender<Reply>,
        max_flows: usize,
        shutdown: CancellationToken,
    ) -> Self {
        UdpRelay {
            flows: HashMap::new(),
            replies,
            max_flows,
            shutdown,
            relayed: Arc::default(),
        }
    }

    pub(crate) fn flow_count(&self) -> usize {
        self.flows.len()
    }

    /// Relay one datagram from the device (`src`) to `dst`.
    pub(crate) fn send(&mut self, src: SocketAddr, dst: SocketAddr, payload: &[u8]) {
        let key = (src, dst);
        if !self.flows.contains_key(&key) {
            if self.flows.len() >= self.max_flows {
                self.sweep();
                if self.flows.len() >= self.max_flows {
                    tracing::debug!(%dst, "WireGuard: UDP flow limit reached; dropping datagram");
                    return;
                }
            }
            match self.open(src, dst) {
                Some(flow) => {
                    self.flows.insert(key, flow);
                }
                None => return,
            }
        }
        let Some(flow) = self.flows.get(&key) else {
            return;
        };
        *flow.last_active.lock() = Instant::now();
        // Best effort, like the network itself: a full socket buffer drops the datagram.
        if flow.sender.send(payload).is_ok() {
            self.relayed.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn open(&self, device: SocketAddr, dst: SocketAddr) -> Option<Flow> {
        let bind: SocketAddr = match dst.ip() {
            IpAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
            IpAddr::V6(_) => (Ipv6Addr::UNSPECIFIED, 0).into(),
        };
        let std_socket = std::net::UdpSocket::bind(bind).ok()?;
        std_socket.set_nonblocking(true).ok()?;
        std_socket.connect(dst).ok()?;
        let sender = std_socket.try_clone().ok()?;
        let socket = UdpSocket::from_std(std_socket).ok()?;
        let last_active = Arc::new(Mutex::new(Instant::now()));
        let stop = self.shutdown.child_token();
        let (task_socket, task_active, task_stop, replies) = (
            socket,
            last_active.clone(),
            stop.clone(),
            self.replies.clone(),
        );
        tokio::spawn(async move {
            let mut buf = vec![0u8; 65536];
            loop {
                let idle_for = task_active.lock().elapsed();
                if idle_for >= FLOW_IDLE {
                    break;
                }
                tokio::select! {
                    r = task_socket.recv(&mut buf) => match r {
                        Ok(n) => {
                            *task_active.lock() = Instant::now();
                            let reply = Reply { from: dst, to: device, payload: buf[..n].to_vec() };
                            // A full queue drops the datagram rather than stalling the relay.
                            let _ = replies.try_send(reply);
                        }
                        // ICMP errors (port unreachable...) surface as recv errors: ignore.
                        Err(_) => tokio::time::sleep(Duration::from_millis(5)).await,
                    },
                    _ = tokio::time::sleep(FLOW_IDLE - idle_for) => {}
                    _ = task_stop.cancelled() => break,
                }
            }
            task_stop.cancel();
        });
        Some(Flow {
            sender,
            last_active,
            stop,
        })
    }

    /// Forget flows whose relay task ended (idle).
    pub(crate) fn sweep(&mut self) {
        self.flows.retain(|_, f| !f.stop.is_cancelled());
    }

    /// Close every flow.
    pub(crate) fn close_all(&mut self) {
        for (_, f) in self.flows.drain() {
            f.stop.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn relays_both_ways_and_expires() {
        let echo = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let echo_addr = echo.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buf = [0u8; 1500];
            while let Ok((n, from)) = echo.recv_from(&mut buf).await {
                let _ = echo.send_to(&buf[..n], from).await;
            }
        });
        let (tx, mut rx) = mpsc::channel(16);
        let shutdown = CancellationToken::new();
        let mut relay = UdpRelay::new(tx, 2, shutdown.clone());
        let device: SocketAddr = "10.0.0.2:4000".parse().unwrap();
        relay.send(device, echo_addr, b"ping");
        let reply = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!((reply.from, reply.to), (echo_addr, device));
        assert_eq!(reply.payload, b"ping");
        assert_eq!(relay.flow_count(), 1);
        relay.send(device, echo_addr, b"again");
        assert_eq!(relay.flow_count(), 1, "same 4-tuple reuses its flow");
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .unwrap()
                .unwrap()
                .payload,
            b"again"
        );
        // Limit: a third flow is refused while two are open.
        relay.send("10.0.0.2:4001".parse().unwrap(), echo_addr, b"b");
        relay.send("10.0.0.2:4002".parse().unwrap(), echo_addr, b"c");
        assert_eq!(relay.flow_count(), 2);
        relay.close_all();
        assert_eq!(relay.flow_count(), 0);
        assert_eq!(relay.relayed.load(Ordering::Relaxed), 3);
    }
}
