//! Raw byte tunnels for connections Akev doesn't intercept (TLS passthrough, non-HTTP
//! protocols, server-speaks-first protocols): connect upstream via the upstream client (so DNS
//! overrides and upstream proxies apply) and pipe bytes both ways with half-close support,
//! recording `TunnelOpened` / `TunnelClosed`.

use crate::conn::ConnCtx;
use crate::io::ClientIo;
use crate::shared::Shared;
use crate::upstream::UpstreamError;
use akev_core::*;
use bytes::Bytes;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;

/// Why a connection is tunnelled instead of intercepted.
#[derive(Clone, Debug)]
pub(crate) enum TunnelReason {
    /// Host matched a `tls_passthrough` pattern.
    TlsPassthrough,
    /// The TLS ClientHello can't be intercepted (e.g. non-HTTP ALPN, TLS < 1.2).
    NotInterceptable(String),
    /// The protocol inside the tunnel isn't one Akev understands.
    UnknownProtocol,
    /// The client didn't speak first (SSH, SMTP, ...).
    ServerSpeaksFirst,
}

impl TunnelReason {
    fn describe(&self) -> String {
        match self {
            TunnelReason::TlsPassthrough => "tls-passthrough".to_string(),
            TunnelReason::NotInterceptable(why) => format!("not-interceptable: {why}"),
            TunnelReason::UnknownProtocol => "unknown-protocol".to_string(),
            TunnelReason::ServerSpeaksFirst => "server-speaks-first".to_string(),
        }
    }
}

/// Tunnel `client` (with `prefix` = bytes already read from it) to `host:port`.
///
/// `rewrap` = `(sni, alpn)`: the client stream is decrypted TLS, so the upstream side must be
/// re-encrypted (non-HTTP protocol found inside an intercepted TLS connection).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run(
    shared: Arc<Shared>,
    client: ClientIo,
    prefix: Bytes,
    host: String,
    port: u16,
    ctx: &ConnCtx,
    reason: TunnelReason,
    rewrap: Option<(String, Option<String>)>,
) {
    let settings = shared.settings();
    let record = settings.capture.record_tunnels && !settings.ignore.matches(&host, Some(port));
    let id = ExchangeId::next();
    let sw = Stopwatch::start();
    let mut reason_text = reason.describe();
    if rewrap.is_some() {
        reason_text.push_str(" (TLS re-encrypted)");
    }
    if record {
        let client_info = ctx.client_info(&shared, ctx.mode()).await.0;
        shared.sink.emit(ProxyEvent::TunnelOpened {
            id,
            client: client_info,
            host: host.clone(),
            port,
            reason: reason_text.clone(),
            at: sw.started_at(),
        });
    }
    let connect = async {
        let (io, info, timings) = shared
            .upstream
            .connect_raw_restricted(&host, port, ctx.restricts_loopback(&shared))
            .await?;
        match &rewrap {
            Some((sni, alpn)) => {
                let tls =
                    crate::tls::connect_upstream(&shared, io, &host, port, sni, alpn.as_deref())
                        .await?;
                Ok::<_, UpstreamError>((tls, info, timings))
            }
            None => Ok((io, info, timings)),
        }
    };
    let connect = tokio::select! {
        r = connect => Some(r),
        _ = shared.hard_stop.cancelled() => None,
    };
    let mut stats = TunnelStats {
        bytes_sent: 0,
        bytes_received: 0,
        reason: reason_text.clone(),
    };
    match connect {
        Some(Ok((mut upstream, _info, _timings))) => {
            let counted = crate::io::Counted::new(client);
            let (sent_counter, received_counter) = counted.counters();
            let mut client = counted;
            let mut ok = true;
            if !prefix.is_empty() {
                ok = upstream.write_all(&prefix).await.is_ok();
                stats.bytes_sent += prefix.len() as u64;
            }
            if ok {
                tokio::select! {
                    _ = tokio::io::copy_bidirectional(&mut client, &mut upstream) => {}
                    _ = shared.hard_stop.cancelled() => {}
                }
            }
            stats.bytes_sent += sent_counter.load(std::sync::atomic::Ordering::Relaxed);
            stats.bytes_received += received_counter.load(std::sync::atomic::Ordering::Relaxed);
        }
        Some(Err(e)) => {
            tracing::debug!(%host, port, error = %e, "tunnel upstream connection failed");
            stats.reason = format!("{reason_text} (upstream connection failed: {})", e.message);
        }
        None => {}
    }
    if record {
        shared.sink.emit(ProxyEvent::TunnelClosed {
            id,
            stats,
            at: sw.elapsed_ms(),
        });
    }
}
