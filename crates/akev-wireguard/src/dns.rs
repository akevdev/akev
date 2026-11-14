//! The DNS relay at `10.0.0.53` (the `DNS` server of the client configuration).
//!
//! Queries are parsed and answered by resolving them on this machine: through the system
//! resolver configuration (hickory: `/etc/resolv.conf` / registry, hosts file, cache), with
//! the OS resolver (`getaddrinfo`) as a fallback for names only it knows (mDNS `.local`,
//! single-label names, NSS modules). Because Akev connects upstream by hostname, the
//! addresses the device gets don't decide where the proxy connects — they only need to be
//! routable into the tunnel, which every address is.
//!
//! `akev.cert` resolves to [`crate::SERVER_ADDRESS`], so the setup page (CA certificate
//! downloads) opens on the device through the tunnel.
//!
//! UDP responses that don't fit the client's EDNS payload size (or the tunnel MTU) are
//! truncated (`TC` bit) and the client retries over TCP, which [`serve_tcp`] answers.

use crate::{MAGIC_HOST, SERVER_ADDRESS};
use futures::future::BoxFuture;
use hickory_resolver::TokioResolver;
use hickory_resolver::config::LookupIpStrategy;
use hickory_resolver::proto::op::{Edns, Message, MessageType, OpCode, ResponseCode};
use hickory_resolver::proto::rr::rdata::{A, AAAA};
use hickory_resolver::proto::rr::{DNSClass, Name, RData, Record, RecordType};
use std::net::IpAddr;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// How long one lookup may take before the client gets SERVFAIL.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(8);
/// TTL of synthesized answers (`akev.cert`, `getaddrinfo` fallback).
const SYNTH_TTL: u32 = 60;
/// Idle time after which a DNS-over-TCP connection is closed.
const TCP_IDLE: Duration = Duration::from_secs(30);
/// EDNS payload size advertised in responses.
const EDNS_PAYLOAD: u16 = 1232;

/// The result of resolving one question.
#[derive(Clone, Debug, PartialEq)]
pub enum Answer {
    /// The answer records (including any CNAME chain).
    Records(Vec<Record>),
    /// The name exists but has no records of this type.
    NoData,
    /// The name doesn't exist.
    NxDomain,
    /// Resolution failed.
    ServFail,
}

/// Resolves questions for the DNS relay. [`SystemDns`] in production; tests plug in fakes.
pub trait DnsBackend: Send + Sync + 'static {
    /// Resolve `name` (fully qualified) for `rtype`.
    fn lookup(&self, name: Name, rtype: RecordType) -> BoxFuture<'static, Answer>;
}

/// The system's resolver configuration, through hickory (built lazily, shared).
#[derive(Default)]
pub struct SystemDns {
    resolver: OnceLock<Option<TokioResolver>>,
}

impl SystemDns {
    /// A resolver using the system configuration.
    pub fn new() -> Arc<Self> {
        Arc::new(SystemDns::default())
    }

    fn resolver(&self) -> Option<TokioResolver> {
        self.resolver
            .get_or_init(|| {
                let built = TokioResolver::builder_tokio().and_then(|mut builder| {
                    builder.options_mut().ip_strategy = LookupIpStrategy::Ipv4AndIpv6;
                    builder.build()
                });
                match built {
                    Ok(resolver) => Some(resolver),
                    Err(e) => {
                        tracing::warn!(
                            "system DNS configuration unavailable ({e}); using getaddrinfo"
                        );
                        None
                    }
                }
            })
            .clone()
    }
}

impl DnsBackend for SystemDns {
    fn lookup(&self, name: Name, rtype: RecordType) -> BoxFuture<'static, Answer> {
        let resolver = self.resolver();
        Box::pin(async move {
            if let Some(resolver) = resolver {
                match resolver.lookup(name.clone(), rtype).await {
                    Ok(lookup) if !lookup.answers().is_empty() => {
                        return Answer::Records(lookup.answers().to_vec());
                    }
                    Ok(_) => return Answer::NoData,
                    Err(e) if e.is_nx_domain() || e.is_no_records_found() => {
                        if !is_local_style_name(&name) {
                            return if e.is_nx_domain() {
                                Answer::NxDomain
                            } else {
                                Answer::NoData
                            };
                        }
                    }
                    Err(e) => {
                        tracing::debug!(%name, ?rtype, error = %e, "DNS relay lookup failed");
                    }
                }
            }
            if matches!(rtype, RecordType::A | RecordType::AAAA) {
                return getaddrinfo(name, rtype).await;
            }
            Answer::ServFail
        })
    }
}

/// Names that may live outside DNS (mDNS, NetBIOS/NSS, search domains).
fn is_local_style_name(name: &Name) -> bool {
    let ascii = name.to_ascii();
    let trimmed = ascii.trim_end_matches('.');
    !trimmed.contains('.') || trimmed.to_ascii_lowercase().ends_with(".local")
}

async fn getaddrinfo(name: Name, rtype: RecordType) -> Answer {
    let host = name.to_ascii().trim_end_matches('.').to_string();
    let Ok(addrs) = tokio::net::lookup_host((host.as_str(), 0)).await else {
        return Answer::NxDomain;
    };
    let mut records = Vec::new();
    for addr in addrs {
        let rdata = match (addr.ip(), rtype) {
            (IpAddr::V4(v4), RecordType::A) => RData::A(A(v4)),
            (IpAddr::V6(v6), RecordType::AAAA) => RData::AAAA(AAAA(v6)),
            _ => continue,
        };
        let record = Record::from_rdata(name.clone(), SYNTH_TTL, rdata);
        if !records.contains(&record) {
            records.push(record);
        }
    }
    if records.is_empty() {
        Answer::NoData
    } else {
        Answer::Records(records)
    }
}

/// Answer one DNS query message. `max_len` = the largest response the transport carries (the
/// response is truncated with `TC` above it). `None` = not a query; nothing to send.
pub(crate) async fn respond(
    backend: &dyn DnsBackend,
    query: &[u8],
    max_len: usize,
) -> Option<Vec<u8>> {
    respond_within(backend, query, max_len, LOOKUP_TIMEOUT).await
}

async fn respond_within(
    backend: &dyn DnsBackend,
    query: &[u8],
    max_len: usize,
    timeout: Duration,
) -> Option<Vec<u8>> {
    let request = match Message::from_vec(query) {
        Ok(m) => m,
        Err(_) => return format_error(query),
    };
    if request.metadata.message_type != MessageType::Query {
        return None;
    }
    let mut response = Message::new(
        request.metadata.id,
        MessageType::Response,
        request.metadata.op_code,
    );
    response.metadata.recursion_desired = request.metadata.recursion_desired;
    response.metadata.recursion_available = true;
    response.metadata.checking_disabled = request.metadata.checking_disabled;
    if request.edns.is_some() {
        let mut edns = Edns::new();
        edns.set_max_payload(EDNS_PAYLOAD);
        response.edns = Some(edns);
    }
    let code = if request.metadata.op_code != OpCode::Query {
        ResponseCode::NotImp
    } else if request.queries.len() != 1 {
        ResponseCode::FormErr
    } else {
        let question = request.queries[0].clone();
        response.add_query(question.clone());
        if question.query_class() != DNSClass::IN {
            ResponseCode::NotImp
        } else {
            let answer = match local_answer(question.name(), question.query_type()) {
                Some(a) => a,
                None => tokio::time::timeout(
                    timeout,
                    backend.lookup(question.name().clone(), question.query_type()),
                )
                .await
                .unwrap_or(Answer::ServFail),
            };
            match answer {
                Answer::Records(records) => {
                    response.add_answers(records);
                    ResponseCode::NoError
                }
                Answer::NoData => ResponseCode::NoError,
                Answer::NxDomain => ResponseCode::NXDomain,
                Answer::ServFail => ResponseCode::ServFail,
            }
        }
    };
    response.metadata.response_code = code;
    let bytes = response.to_vec().ok()?;
    if bytes.len() <= max_len {
        return Some(bytes);
    }
    // Too big for this transport: an empty, truncated answer makes the client retry over TCP.
    response.answers.clear();
    response.authorities.clear();
    response.additionals.clear();
    response.metadata.truncation = true;
    response.to_vec().ok()
}

/// The answer for names Akev serves itself.
fn local_answer(name: &Name, rtype: RecordType) -> Option<Answer> {
    let magic = Name::from_ascii(format!("{MAGIC_HOST}.")).ok()?;
    if *name != magic {
        return None;
    }
    Some(match rtype {
        RecordType::A => Answer::Records(vec![Record::from_rdata(
            name.clone(),
            SYNTH_TTL,
            RData::A(A(SERVER_ADDRESS)),
        )]),
        _ => Answer::NoData,
    })
}

/// FORMERR for a message whose header is readable but whose body isn't.
fn format_error(query: &[u8]) -> Option<Vec<u8>> {
    if query.len() < 12 || query[2] & 0x80 != 0 {
        return None;
    }
    let mut out = query[..12].to_vec();
    out[2] = 0x80 | (query[2] & 0x79); // QR + opcode + RD
    out[3] = 0x80 | ResponseCode::FormErr.low(); // RA + FORMERR
    out[4..12].fill(0);
    Some(out)
}

/// Largest UDP response for a query: the client's EDNS payload size (512 without EDNS),
/// capped so the IP packet fits the tunnel MTU.
pub(crate) fn udp_limit(query: &[u8], mtu: usize) -> usize {
    let client = Message::from_vec(query)
        .ok()
        .and_then(|m| m.edns.as_ref().map(|e| e.max_payload()))
        .map(|p| usize::from(p).max(512))
        .unwrap_or(512);
    // IPv6 + UDP headers: the larger of the two families.
    client.min(mtu.saturating_sub(48)).max(512)
}

/// Serve DNS over TCP (RFC 1035 §4.2.2: 2-byte length prefix) on a tunnelled connection to
/// the relay's port 53.
pub(crate) async fn serve_tcp<S>(mut stream: S, backend: Arc<dyn DnsBackend>)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    loop {
        let mut len = [0u8; 2];
        match tokio::time::timeout(TCP_IDLE, stream.read_exact(&mut len)).await {
            Ok(Ok(_)) => {}
            _ => break,
        }
        let mut query = vec![0u8; usize::from(u16::from_be_bytes(len))];
        if tokio::time::timeout(TCP_IDLE, stream.read_exact(&mut query))
            .await
            .map_or(true, |r| r.is_err())
        {
            break;
        }
        let Some(answer) = respond(backend.as_ref(), &query, usize::from(u16::MAX)).await else {
            continue;
        };
        let mut framed = Vec::with_capacity(answer.len() + 2);
        framed.extend_from_slice(&(answer.len() as u16).to_be_bytes());
        framed.extend_from_slice(&answer);
        if stream.write_all(&framed).await.is_err() {
            break;
        }
    }
    let _ = stream.shutdown().await;
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use hickory_resolver::proto::op::Query;
    use std::net::Ipv4Addr;

    /// Answers `*.test` names from a fixed table.
    pub(crate) struct FakeDns;

    impl DnsBackend for FakeDns {
        fn lookup(&self, name: Name, rtype: RecordType) -> BoxFuture<'static, Answer> {
            Box::pin(async move {
                let host = name.to_ascii().to_ascii_lowercase();
                match (host.as_str(), rtype) {
                    ("origin.test.", RecordType::A) => Answer::Records(vec![Record::from_rdata(
                        name,
                        300,
                        RData::A(A(Ipv4Addr::new(192, 0, 2, 10))),
                    )]),
                    ("origin.test.", _) => Answer::NoData,
                    ("big.test.", RecordType::A) => Answer::Records(
                        (0..200)
                            .map(|i| {
                                Record::from_rdata(
                                    name.clone(),
                                    300,
                                    RData::A(A(Ipv4Addr::new(10, 1, (i / 250) as u8, i as u8))),
                                )
                            })
                            .collect(),
                    ),
                    ("slow.test.", _) => {
                        tokio::time::sleep(Duration::from_secs(3600)).await;
                        Answer::NoData
                    }
                    ("broken.test.", _) => Answer::ServFail,
                    _ => Answer::NxDomain,
                }
            })
        }
    }

    pub(crate) fn query(name: &str, rtype: RecordType, edns: bool) -> Vec<u8> {
        let mut m = Message::new(0x1234, MessageType::Query, OpCode::Query);
        m.metadata.recursion_desired = true;
        m.add_query(Query::query(Name::from_ascii(name).unwrap(), rtype));
        if edns {
            let mut e = Edns::new();
            e.set_max_payload(4096);
            m.edns = Some(e);
        }
        m.to_vec().unwrap()
    }

    pub(crate) fn a_records(response: &[u8]) -> Vec<Ipv4Addr> {
        Message::from_vec(response)
            .unwrap()
            .answers
            .iter()
            .filter_map(|r| match &r.data {
                RData::A(a) => Some(a.0),
                _ => None,
            })
            .collect()
    }

    async fn ask(name: &str, rtype: RecordType) -> Message {
        let raw = respond(&FakeDns, &query(name, rtype, false), 512)
            .await
            .unwrap();
        Message::from_vec(&raw).unwrap()
    }

    #[tokio::test]
    async fn answers_from_backend() {
        let m = ask("origin.test.", RecordType::A).await;
        assert_eq!(m.metadata.id, 0x1234);
        assert_eq!(m.metadata.message_type, MessageType::Response);
        assert_eq!(m.metadata.response_code, ResponseCode::NoError);
        assert!(m.metadata.recursion_available && m.metadata.recursion_desired);
        assert_eq!(m.queries.len(), 1);
        assert_eq!(
            a_records(&m.to_vec().unwrap()),
            vec![Ipv4Addr::new(192, 0, 2, 10)]
        );
        let m = ask("origin.test.", RecordType::AAAA).await;
        assert_eq!(m.metadata.response_code, ResponseCode::NoError);
        assert!(m.answers.is_empty());
        assert_eq!(
            ask("missing.test.", RecordType::A)
                .await
                .metadata
                .response_code,
            ResponseCode::NXDomain
        );
        assert_eq!(
            ask("broken.test.", RecordType::A)
                .await
                .metadata
                .response_code,
            ResponseCode::ServFail
        );
    }

    #[tokio::test]
    async fn magic_host_points_into_the_tunnel() {
        let m = ask("AKEV.cert.", RecordType::A).await;
        assert_eq!(a_records(&m.to_vec().unwrap()), vec![SERVER_ADDRESS]);
        let m = ask("akev.cert.", RecordType::AAAA).await;
        assert_eq!(m.metadata.response_code, ResponseCode::NoError);
        assert!(m.answers.is_empty());
    }

    #[tokio::test]
    async fn oversized_udp_answers_are_truncated() {
        let q = query("big.test.", RecordType::A, false);
        let raw = respond(&FakeDns, &q, udp_limit(&q, 1420)).await.unwrap();
        let m = Message::from_vec(&raw).unwrap();
        assert!(m.metadata.truncation);
        assert!(m.answers.is_empty());
        assert!(raw.len() <= 512);
        // The same query over TCP gets everything.
        let raw = respond(&FakeDns, &q, 65535).await.unwrap();
        assert_eq!(a_records(&raw).len(), 200);
    }

    #[test]
    fn udp_limits() {
        assert_eq!(
            udp_limit(&query("a.test.", RecordType::A, false), 1420),
            512
        );
        assert_eq!(
            udp_limit(&query("a.test.", RecordType::A, true), 1420),
            1372
        );
        assert_eq!(
            udp_limit(&query("a.test.", RecordType::A, true), 9000),
            4096
        );
        assert_eq!(udp_limit(b"garbage", 1420), 512);
    }

    #[tokio::test]
    async fn garbage_and_non_queries() {
        assert!(respond(&FakeDns, b"short", 512).await.is_none());
        let mut bad = query("origin.test.", RecordType::A, false);
        bad.truncate(14);
        let raw = respond(&FakeDns, &bad, 512).await.unwrap();
        assert_eq!(raw[0..2], [0x12, 0x34]);
        assert_eq!(raw[3] & 0x0f, ResponseCode::FormErr.low());
        // Responses are never answered (no reflection loops).
        let mut m = Message::from_vec(&query("origin.test.", RecordType::A, false)).unwrap();
        m.metadata.message_type = MessageType::Response;
        assert!(respond(&FakeDns, &m.to_vec().unwrap(), 512).await.is_none());
    }

    #[tokio::test]
    async fn slow_lookups_fail_with_servfail() {
        let q = query("slow.test.", RecordType::A, false);
        let raw = respond_within(&FakeDns, &q, 512, Duration::from_millis(50))
            .await
            .unwrap();
        assert_eq!(
            Message::from_vec(&raw).unwrap().metadata.response_code,
            ResponseCode::ServFail
        );
    }

    #[tokio::test]
    async fn dns_over_tcp() {
        let (mut client, server) = tokio::io::duplex(1 << 16);
        let task = tokio::spawn(serve_tcp(server, Arc::new(FakeDns) as Arc<dyn DnsBackend>));
        for _ in 0..2 {
            let q = query("big.test.", RecordType::A, true);
            client
                .write_all(&(q.len() as u16).to_be_bytes())
                .await
                .unwrap();
            client.write_all(&q).await.unwrap();
            let mut len = [0u8; 2];
            client.read_exact(&mut len).await.unwrap();
            let mut resp = vec![0u8; u16::from_be_bytes(len) as usize];
            client.read_exact(&mut resp).await.unwrap();
            assert_eq!(a_records(&resp).len(), 200);
        }
        drop(client);
        task.await.unwrap();
    }

    #[tokio::test]
    async fn system_resolver_answers_localhost() {
        // RFC 6761: hickory answers `localhost` itself, no network needed.
        let backend = SystemDns::new();
        let raw = respond(
            backend.as_ref(),
            &query("localhost.", RecordType::A, true),
            1232,
        )
        .await
        .unwrap();
        assert!(
            a_records(&raw).contains(&Ipv4Addr::LOCALHOST),
            "{:?}",
            Message::from_vec(&raw)
        );
    }

    #[test]
    fn local_style_names() {
        assert!(is_local_style_name(&Name::from_ascii("printer.").unwrap()));
        assert!(is_local_style_name(
            &Name::from_ascii("mac.local.").unwrap()
        ));
        assert!(!is_local_style_name(
            &Name::from_ascii("example.com.").unwrap()
        ));
    }
}
