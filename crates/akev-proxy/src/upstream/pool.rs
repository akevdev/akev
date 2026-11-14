//! Connection pool: idle HTTP/1.1 keep-alive connections and shared HTTP/2 connections,
//! keyed by (scheme, host, port, upstream proxy, TLS identity).
//!
//! * HTTP/1.1: at most [`MAX_IDLE_PER_HOST`] idle connections per key, evicted after
//!   [`IDLE_TIMEOUT`]; connections are checked for liveness (peer FIN / unexpected bytes)
//!   before reuse.
//! * HTTP/1.1 connections that carried connection-oriented authentication (NTLM /
//!   Negotiate) are *pinned* to the client connection ([`ConnectionOwner`]) whose request
//!   authenticated them: they're kept apart and only ever reused for that client's requests
//!   (so a multi-leg handshake stays on one connection and another client can never ride on
//!   the authenticated connection), and they're dropped once that client connection is gone.
//! * HTTP/2: multiplexed connections shared per key. A new stream goes to a connection with
//!   spare capacity ([`H2Handle::has_capacity`]); when every connection is busy (streams
//!   stalled by slow consumers, the server's concurrency limit), another one is opened.
//!   Closed connections (GOAWAY / error) are dropped.
//! * The first connection to a key that may negotiate h2 holds a per-key connect lock so
//!   concurrent requests wait for it and multiplex instead of opening N connections. Once a
//!   key is known to speak only HTTP/1.1, connects proceed in parallel without the lock.
//! * Everything is tagged with the config generation; [`Pool::reset`] (on settings changes
//!   affecting DNS/TLS/proxy) drops all connections and stale connections are never pooled.

use super::h2::H2Handle;
use super::{BoxIo, ConnectionOwner, UpstreamError, WeakOwner};
use akev_core::UpstreamInfo;
use bytes::BytesMut;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::task::{Context, Waker};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, ReadBuf};

/// Idle connections older than this are closed.
pub(crate) const IDLE_TIMEOUT: Duration = Duration::from_secs(90);
/// Maximum idle HTTP/1.1 connections kept per key (and per owner, for pinned connections).
pub(crate) const MAX_IDLE_PER_HOST: usize = 8;
/// Maximum idle pinned HTTP/1.1 connections kept per key, all owners together.
const MAX_PINNED_PER_HOST: usize = 64;
/// How long an "HTTP/1.1 only" ALPN result is remembered.
const H1_HINT_TTL: Duration = Duration::from_secs(600);
/// Waiters for a failed connect reuse its error for this long instead of retrying serially.
const CONNECT_ERROR_TTL: Duration = Duration::from_secs(5);
const REAP_INTERVAL: Duration = Duration::from_secs(15);

/// Pool key.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PoolKey {
    pub tls: bool,
    /// Empty for absolute-form connections to an HTTP proxy (shared by all plain-HTTP targets).
    pub host: Arc<str>,
    pub port: u16,
    /// Upstream proxy identity (URL with username, without password).
    pub proxy: Option<Arc<str>>,
    /// Certificate verification disabled.
    pub insecure: bool,
    /// Index of the client certificate presented.
    pub client_cert: Option<usize>,
    /// TLS connection opened offering only `http/1.1` via ALPN (`http1_only`, upgrades).
    /// Kept apart from connections that offered `h2` so such requests never hide a
    /// server's HTTP/2 support from the requests that could use it.
    pub h1_alpn: bool,
    /// Browser whose TLS / HTTP/2 fingerprint the connection presents (`None` = native):
    /// connections with different fingerprints are never shared.
    pub browser: Option<super::fingerprint::Browser>,
    /// Opened for a client that may not reach loopback addresses
    /// ([`super::RequestOptions::restrict_loopback`]): never shared with connections opened
    /// without that check.
    pub restricted: bool,
}

/// Immutable facts about an established connection.
#[derive(Debug)]
pub(crate) struct ConnMeta {
    pub key: PoolKey,
    pub generation: u64,
    /// Upstream info as of connection establishment (`reused` = false).
    pub info: UpstreamInfo,
    /// `host:port` of the target for messages.
    pub peer: Arc<str>,
    /// Plain-HTTP request through an HTTP proxy: absolute-form request targets.
    pub absolute_form: bool,
    /// `Proxy-Authorization` value for absolute-form requests.
    pub proxy_auth: Option<String>,
}

/// An HTTP/1.1 connection (idle or in use).
pub(crate) struct H1Conn {
    pub io: BoxIo,
    /// Bytes read but not yet consumed.
    pub buf: BytesMut,
    pub meta: Arc<ConnMeta>,
    /// The client connection this connection is pinned to (it carried connection-oriented
    /// authentication for it); `None` for shared connections.
    pub pinned: Option<WeakOwner>,
}

struct Idle {
    conn: H1Conn,
    since: Instant,
}

#[derive(Default)]
struct HostEntry {
    idle: Vec<Idle>,
    /// Idle connections pinned to a client connection (see the module docs).
    pinned: Vec<Idle>,
    h2: Vec<H2Handle>,
    h1_hint: Option<Instant>,
    connect_lock: Option<Arc<tokio::sync::Mutex<()>>>,
    connect_error: Option<(Instant, UpstreamError)>,
}

impl HostEntry {
    fn is_empty(&self) -> bool {
        self.idle.is_empty()
            && self.pinned.is_empty()
            && self.h2.is_empty()
            && self.h1_hint.is_none()
            && self.connect_error.is_none()
            && self
                .connect_lock
                .as_ref()
                .is_none_or(|l| Arc::strong_count(l) == 1)
    }
}

struct PoolInner {
    generation: u64,
    hosts: HashMap<PoolKey, HostEntry>,
}

/// The connection pool (shared by all config generations of a client).
pub(crate) struct Pool {
    inner: Mutex<PoolInner>,
    reaper_started: AtomicBool,
}

impl std::fmt::Debug for Pool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self.inner.lock();
        f.debug_struct("Pool")
            .field("generation", &inner.generation)
            .field("keys", &inner.hosts.len())
            .finish()
    }
}

impl Pool {
    pub(crate) fn new(generation: u64) -> Arc<Self> {
        Arc::new(Pool {
            inner: Mutex::new(PoolInner {
                generation,
                hosts: HashMap::new(),
            }),
            reaper_started: AtomicBool::new(false),
        })
    }

    /// Drop every pooled connection and start a new generation.
    pub(crate) fn reset(&self, generation: u64) {
        let old = {
            let mut inner = self.inner.lock();
            inner.generation = generation;
            std::mem::take(&mut inner.hosts)
        };
        drop(old);
    }

    /// Return a reusable HTTP/1.1 connection. A pinned connection whose client connection is
    /// gone is closed instead.
    pub(crate) fn put_h1(self: &Arc<Self>, conn: H1Conn) {
        let evicted = {
            let mut inner = self.inner.lock();
            if conn.meta.generation != inner.generation {
                return;
            }
            if conn.pinned.as_ref().is_some_and(|o| !o.is_alive()) {
                drop(inner);
                drop(conn);
                return;
            }
            let entry = inner.hosts.entry(conn.meta.key.clone()).or_default();
            let evicted = match &conn.pinned {
                None => (entry.idle.len() >= MAX_IDLE_PER_HOST).then(|| entry.idle.remove(0)),
                Some(owner) => {
                    let same_owner = entry
                        .pinned
                        .iter()
                        .filter(|i| i.conn.pinned.as_ref().is_some_and(|o| o.same(owner)))
                        .count();
                    let oldest = if same_owner >= MAX_IDLE_PER_HOST {
                        entry
                            .pinned
                            .iter()
                            .position(|i| i.conn.pinned.as_ref().is_some_and(|o| o.same(owner)))
                    } else if entry.pinned.len() >= MAX_PINNED_PER_HOST {
                        Some(0)
                    } else {
                        None
                    };
                    oldest.map(|i| entry.pinned.remove(i))
                }
            };
            let idle = Idle {
                conn,
                since: Instant::now(),
            };
            if idle.conn.pinned.is_some() {
                entry.pinned.push(idle);
            } else {
                entry.idle.push(idle);
            }
            evicted
        };
        drop(evicted);
        self.ensure_reaper();
    }

    /// Take a live idle HTTP/1.1 connection (most recently used first): one pinned to
    /// `owner` if there is one, else a shared one. Connections pinned to another client
    /// connection are never handed out.
    pub(crate) fn take_h1(&self, key: &PoolKey, owner: Option<&ConnectionOwner>) -> Option<H1Conn> {
        loop {
            let idle = {
                let mut inner = self.inner.lock();
                let entry = inner.hosts.get_mut(key)?;
                let pinned = owner.and_then(|owner| {
                    let pos = entry
                        .pinned
                        .iter()
                        .rposition(|i| i.conn.pinned.as_ref().is_some_and(|o| o.is(owner)))?;
                    Some(entry.pinned.remove(pos))
                });
                match pinned {
                    Some(idle) => idle,
                    None => entry.idle.pop()?,
                }
            };
            if idle.since.elapsed() >= IDLE_TIMEOUT {
                continue;
            }
            let mut conn = idle.conn;
            if is_alive(&mut conn) {
                return Some(conn);
            }
        }
    }

    /// An open HTTP/2 connection for `key` that can take another stream right away (the
    /// least busy one). `None` when there is none, or when every connection is saturated:
    /// the caller then opens an additional connection.
    pub(crate) fn get_h2(&self, key: &PoolKey) -> Option<H2Handle> {
        let mut inner = self.inner.lock();
        let entry = inner.hosts.get_mut(key)?;
        entry.h2.retain(|h| !h.is_closed());
        entry
            .h2
            .iter()
            .filter(|h| h.has_capacity())
            .min_by_key(|h| h.active_streams())
            .cloned()
    }

    /// The least busy open HTTP/2 connection for `key`, even if saturated (fallback when an
    /// additional connection can't be opened: the request then waits for a free stream).
    pub(crate) fn any_h2(&self, key: &PoolKey) -> Option<H2Handle> {
        let inner = self.inner.lock();
        inner
            .hosts
            .get(key)?
            .h2
            .iter()
            .filter(|h| !h.is_closed())
            .min_by_key(|h| h.active_streams())
            .cloned()
    }

    /// Register a new HTTP/2 connection.
    pub(crate) fn put_h2(self: &Arc<Self>, handle: H2Handle) {
        {
            let mut inner = self.inner.lock();
            if handle.meta().generation != inner.generation {
                return;
            }
            let entry = inner.hosts.entry(handle.meta().key.clone()).or_default();
            entry.h2.retain(|h| !h.is_closed());
            entry.h2.push(handle);
            entry.h1_hint = None;
        }
        self.ensure_reaper();
    }

    /// Forget an HTTP/2 connection that failed.
    pub(crate) fn remove_h2(&self, handle: &H2Handle) {
        let mut inner = self.inner.lock();
        if let Some(entry) = inner.hosts.get_mut(&handle.meta().key) {
            entry.h2.retain(|h| !h.same(handle));
        }
    }

    /// Whether `key` recently negotiated HTTP/1.1 (no need to serialise connects).
    pub(crate) fn is_h1_only(&self, key: &PoolKey) -> bool {
        let inner = self.inner.lock();
        inner
            .hosts
            .get(key)
            .and_then(|e| e.h1_hint)
            .is_some_and(|at| at.elapsed() < H1_HINT_TTL)
    }

    /// Remember that `key` negotiated HTTP/1.1.
    pub(crate) fn set_h1_only(&self, key: &PoolKey, generation: u64) {
        let mut inner = self.inner.lock();
        if inner.generation == generation {
            inner.hosts.entry(key.clone()).or_default().h1_hint = Some(Instant::now());
        }
    }

    /// The per-key connect lock.
    pub(crate) fn connect_lock(&self, key: &PoolKey) -> Arc<tokio::sync::Mutex<()>> {
        let mut inner = self.inner.lock();
        inner
            .hosts
            .entry(key.clone())
            .or_default()
            .connect_lock
            .get_or_insert_with(Default::default)
            .clone()
    }

    /// Record a connect failure for lock waiters.
    pub(crate) fn record_connect_error(&self, key: &PoolKey, error: &UpstreamError) {
        let mut inner = self.inner.lock();
        inner.hosts.entry(key.clone()).or_default().connect_error =
            Some((Instant::now(), error.clone()));
    }

    /// A connect failure recorded after `since` (while we were waiting for the lock).
    pub(crate) fn connect_error_since(
        &self,
        key: &PoolKey,
        since: Instant,
    ) -> Option<UpstreamError> {
        let inner = self.inner.lock();
        let (at, e) = inner.hosts.get(key)?.connect_error.as_ref()?;
        (*at >= since && at.elapsed() < CONNECT_ERROR_TTL).then(|| e.clone())
    }

    fn ensure_reaper(self: &Arc<Self>) {
        if self.reaper_started.swap(true, Ordering::AcqRel) {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            self.reaper_started.store(false, Ordering::Release);
            return;
        };
        let weak: Weak<Pool> = Arc::downgrade(self);
        runtime.spawn(async move {
            let mut tick = tokio::time::interval(REAP_INTERVAL);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            tick.tick().await;
            loop {
                tick.tick().await;
                let Some(pool) = weak.upgrade() else { break };
                pool.reap();
            }
        });
    }

    /// Close expired idle connections and forget dead entries.
    pub(crate) fn reap(&self) {
        let mut dropped: Vec<Idle> = Vec::new();
        let mut dropped_h2: Vec<H2Handle> = Vec::new();
        {
            let mut inner = self.inner.lock();
            inner.hosts.retain(|_, entry| {
                let (keep, expired): (Vec<Idle>, Vec<Idle>) = std::mem::take(&mut entry.idle)
                    .into_iter()
                    .partition(|i| i.since.elapsed() < IDLE_TIMEOUT);
                entry.idle = keep;
                dropped.extend(expired);
                // Pinned connections also go when their client connection is gone.
                let (keep, expired): (Vec<Idle>, Vec<Idle>) = std::mem::take(&mut entry.pinned)
                    .into_iter()
                    .partition(|i| {
                        i.since.elapsed() < IDLE_TIMEOUT
                            && i.conn.pinned.as_ref().is_some_and(WeakOwner::is_alive)
                    });
                entry.pinned = keep;
                dropped.extend(expired);
                let (keep, expired): (Vec<H2Handle>, Vec<H2Handle>) = std::mem::take(&mut entry.h2)
                    .into_iter()
                    .partition(|h| !h.is_closed() && h.idle_for() < IDLE_TIMEOUT);
                entry.h2 = keep;
                dropped_h2.extend(expired);
                if entry.h1_hint.is_some_and(|at| at.elapsed() >= H1_HINT_TTL) {
                    entry.h1_hint = None;
                }
                if entry
                    .connect_error
                    .as_ref()
                    .is_some_and(|(at, _)| at.elapsed() >= CONNECT_ERROR_TTL)
                {
                    entry.connect_error = None;
                }
                !entry.is_empty()
            });
        }
        drop(dropped);
        drop(dropped_h2);
    }

    /// Number of idle HTTP/1.1 connections for `key` (diagnostics/tests).
    #[cfg(test)]
    pub(crate) fn idle_count(&self, key: &PoolKey) -> usize {
        self.inner.lock().hosts.get(key).map_or(0, |e| e.idle.len())
    }

    /// Number of idle pinned HTTP/1.1 connections for `key` (tests).
    #[cfg(test)]
    pub(crate) fn pinned_count(&self, key: &PoolKey) -> usize {
        self.inner
            .lock()
            .hosts
            .get(key)
            .map_or(0, |e| e.pinned.len())
    }
}

/// Non-blocking check that an idle connection is still usable: no EOF, no error and no
/// unsolicited bytes waiting.
fn is_alive(conn: &mut H1Conn) -> bool {
    if !conn.buf.is_empty() {
        return false;
    }
    let mut cx = Context::from_waker(Waker::noop());
    let mut scratch = [0u8; 1];
    let mut buf = ReadBuf::new(&mut scratch);
    // Ready means EOF (0 bytes), stray data, or an error: not reusable.
    Pin::new(&mut conn.io)
        .poll_read(&mut cx, &mut buf)
        .is_pending()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(host: &str) -> PoolKey {
        PoolKey {
            tls: false,
            host: host.into(),
            port: 80,
            proxy: None,
            insecure: false,
            client_cert: None,
            h1_alpn: false,
            browser: None,
            restricted: false,
        }
    }

    fn conn(io: BoxIo, k: &PoolKey, generation: u64) -> H1Conn {
        H1Conn {
            io,
            buf: BytesMut::new(),
            meta: Arc::new(ConnMeta {
                key: k.clone(),
                generation,
                info: UpstreamInfo::default(),
                peer: "test:80".into(),
                absolute_form: false,
                proxy_auth: None,
            }),
            pinned: None,
        }
    }

    fn pinned_conn(io: BoxIo, k: &PoolKey, owner: &ConnectionOwner) -> H1Conn {
        H1Conn {
            pinned: Some(owner.downgrade()),
            ..conn(io, k, 0)
        }
    }

    #[tokio::test]
    async fn liveness_and_generations() {
        let pool = Pool::new(1);
        let k = key("a");
        let (client, server) = tokio::io::duplex(64);
        pool.put_h1(conn(Box::new(client), &k, 1));
        assert_eq!(pool.idle_count(&k), 1);
        assert!(pool.take_h1(&k, None).is_some());

        // Closed peer: not reused.
        let (client, server2) = tokio::io::duplex(64);
        drop(server2);
        pool.put_h1(conn(Box::new(client), &k, 1));
        assert!(pool.take_h1(&k, None).is_none());

        // Stale generation: never pooled.
        let (client, _server3) = tokio::io::duplex(64);
        pool.put_h1(conn(Box::new(client), &k, 0));
        assert_eq!(pool.idle_count(&k), 0);

        // Reset drops everything.
        let (client, _server4) = tokio::io::duplex(64);
        pool.put_h1(conn(Box::new(client), &k, 1));
        pool.reset(2);
        assert_eq!(pool.idle_count(&k), 0);
        drop(server);
    }

    #[tokio::test]
    async fn pinned_connections_only_go_back_to_their_owner() {
        let pool = Pool::new(0);
        let k = key("ntlm");
        let alice = ConnectionOwner::new();
        let bob = ConnectionOwner::new();
        let (client, _server) = tokio::io::duplex(64);
        pool.put_h1(pinned_conn(Box::new(client), &k, &alice));
        assert_eq!((pool.idle_count(&k), pool.pinned_count(&k)), (0, 1));
        // Another client connection, or a request without an owner, never gets it.
        assert!(pool.take_h1(&k, Some(&bob)).is_none());
        assert!(pool.take_h1(&k, None).is_none());
        assert_eq!(pool.pinned_count(&k), 1);
        // Its owner does, ahead of shared connections.
        let (client, _shared_peer) = tokio::io::duplex(64);
        pool.put_h1(conn(Box::new(client), &k, 0));
        let taken = pool.take_h1(&k, Some(&alice)).expect("pinned connection");
        assert!(taken.pinned.as_ref().is_some_and(|o| o.is(&alice)));
        // Shared connections remain available to everyone.
        let shared = pool.take_h1(&k, Some(&bob)).expect("shared connection");
        assert!(shared.pinned.is_none());
    }

    #[tokio::test]
    async fn pinned_connections_close_with_their_owner() {
        let pool = Pool::new(0);
        let k = key("gone");
        let owner = ConnectionOwner::new();
        let (client, _server) = tokio::io::duplex(64);
        pool.put_h1(pinned_conn(Box::new(client), &k, &owner));
        assert_eq!(pool.pinned_count(&k), 1);
        drop(owner);
        pool.reap();
        assert_eq!(pool.pinned_count(&k), 0);
        // Returned after the owner went away: closed right away.
        let owner = ConnectionOwner::new();
        let (client, _server) = tokio::io::duplex(64);
        let c = pinned_conn(Box::new(client), &k, &owner);
        drop(owner);
        pool.put_h1(c);
        assert_eq!(pool.pinned_count(&k), 0);
    }

    #[tokio::test]
    async fn bounded_idle_per_host() {
        let pool = Pool::new(0);
        let k = key("b");
        let mut peers = Vec::new();
        for _ in 0..(MAX_IDLE_PER_HOST + 3) {
            let (client, server) = tokio::io::duplex(64);
            peers.push(server);
            pool.put_h1(conn(Box::new(client), &k, 0));
        }
        assert_eq!(pool.idle_count(&k), MAX_IDLE_PER_HOST);
    }

    #[tokio::test]
    async fn h1_hint_and_connect_errors() {
        let pool = Pool::new(3);
        let k = key("c");
        assert!(!pool.is_h1_only(&k));
        pool.set_h1_only(&k, 3);
        assert!(pool.is_h1_only(&k));
        pool.set_h1_only(&key("d"), 2);
        assert!(!pool.is_h1_only(&key("d")));

        let before = Instant::now();
        let err = super::super::error::internal("boom");
        pool.record_connect_error(&k, &err);
        assert!(pool.connect_error_since(&k, before).is_some());
        assert!(
            pool.connect_error_since(&k, Instant::now() + Duration::from_secs(1))
                .is_none()
        );
        let lock = pool.connect_lock(&k);
        assert!(Arc::ptr_eq(&lock, &pool.connect_lock(&k)));
        pool.reap();
    }
}
