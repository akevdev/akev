//! `Alt-Svc` (RFC 7838) parsing and the per-origin HTTP/3 knowledge used by the upstream
//! client: which origins advertised `h3` (and where), for how long, and which origins recently
//! failed over QUIC ("broken", with exponential backoff — like browsers, so a network that
//! blocks UDP costs one slow attempt per origin rather than one per request).

use parking_lot::Mutex;
use std::collections::HashMap;
use std::time::Duration;
use std::time::Instant;

/// `ma` when the header doesn't say (RFC 7838 §3.1: 24 hours).
const DEFAULT_MAX_AGE: Duration = Duration::from_secs(24 * 3600);
/// Longest `ma` honoured (a typo'd huge value must not pin an alternative forever).
const MAX_MAX_AGE: Duration = Duration::from_secs(30 * 24 * 3600);
/// First "broken" period after an HTTP/3 failure; doubles with each consecutive failure.
const BROKEN_BASE: Duration = Duration::from_secs(5 * 60);
/// Longest "broken" period.
const BROKEN_MAX: Duration = Duration::from_secs(60 * 60);
/// Failures reported within this window count once (concurrent requests that all saw the
/// same failed connect).
const SAME_FAILURE_WINDOW: Duration = Duration::from_secs(5);
/// Most origins remembered (each map).
const MAX_ORIGINS: usize = 4096;

/// One `h3` alternative from an `Alt-Svc` header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AltService {
    /// Alternative host; `None` = same host as the origin.
    pub host: Option<String>,
    pub port: u16,
    pub max_age: Duration,
}

/// A parsed `Alt-Svc` field value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AltSvcHeader {
    /// `clear`: forget every alternative of the origin.
    Clear,
    /// The `h3` alternatives listed (other protocols — `h2`, drafts such as `h3-29` — are
    /// skipped). Empty when none was usable.
    Alternatives(Vec<AltService>),
}

/// Split on `sep` outside double-quoted strings.
fn split_unquoted(s: &str, sep: char) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    let mut escaped = false;
    for (i, c) in s.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            c if c == sep && !quoted => {
                out.push(&s[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

/// Strip surrounding quotes (and backslash escapes) from a token or quoted-string.
fn unquote(s: &str) -> String {
    let s = s.trim();
    match s.strip_prefix('"').and_then(|r| r.strip_suffix('"')) {
        Some(inner) => {
            let mut out = String::with_capacity(inner.len());
            let mut escaped = false;
            for c in inner.chars() {
                if escaped {
                    out.push(c);
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else {
                    out.push(c);
                }
            }
            out
        }
        None => s.to_owned(),
    }
}

/// Parse `[host]:port` (the alt-authority). IPv6 hosts are bracketed.
fn parse_alt_authority(s: &str) -> Option<(Option<String>, u16)> {
    let (host, port) = if let Some(rest) = s.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        host.parse::<std::net::Ipv6Addr>().ok()?;
        (host, after.strip_prefix(':')?)
    } else {
        let (host, port) = s.rsplit_once(':')?;
        let valid = host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_'));
        if !valid {
            return None;
        }
        (host, port)
    };
    let port: u16 = port.parse().ok().filter(|p| *p != 0)?;
    let host = (!host.is_empty()).then(|| host.to_ascii_lowercase());
    Some((host, port))
}

/// Parse every `Alt-Svc` field value of a response (in order; a later `clear` wins over
/// earlier alternatives, matching how a combined header would read).
pub(crate) fn parse<'a>(values: impl IntoIterator<Item = &'a str>) -> Option<AltSvcHeader> {
    let mut result: Option<AltSvcHeader> = None;
    for value in values {
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        if value.eq_ignore_ascii_case("clear") {
            result = Some(AltSvcHeader::Clear);
            continue;
        }
        let mut alts = match result.take() {
            Some(AltSvcHeader::Alternatives(a)) => a,
            _ => Vec::new(),
        };
        for alt in split_unquoted(value, ',') {
            let mut parts = split_unquoted(alt, ';').into_iter();
            let Some((protocol, authority)) = parts.next().and_then(|p| p.split_once('=')) else {
                continue;
            };
            // protocol-id is percent-encoded; `h3` has nothing to decode.
            if !protocol.trim().eq_ignore_ascii_case("h3") {
                continue;
            }
            let Some((host, port)) = parse_alt_authority(&unquote(authority)) else {
                continue;
            };
            let mut max_age = DEFAULT_MAX_AGE;
            for param in parts {
                let Some((name, val)) = param.split_once('=') else {
                    continue;
                };
                if name.trim().eq_ignore_ascii_case("ma")
                    && let Ok(secs) = unquote(val).parse::<u64>()
                {
                    max_age = Duration::from_secs(secs).min(MAX_MAX_AGE);
                }
            }
            alts.push(AltService {
                host,
                port,
                max_age,
            });
        }
        result = Some(AltSvcHeader::Alternatives(alts));
    }
    result
}

/// Where to reach an origin over HTTP/3.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct H3Route {
    /// Host to connect to (the certificate is still verified for the origin host).
    pub host: String,
    pub port: u16,
}

type Origin = (String, u16);

struct Advertised {
    route: H3Route,
    expires: Instant,
}

struct Broken {
    until: Instant,
    failures: u32,
    /// When the last failure was counted.
    marked: Instant,
}

/// Per-origin HTTP/3 knowledge (shared by every request of an upstream client).
#[derive(Default)]
pub(crate) struct H3Knowledge {
    advertised: Mutex<HashMap<Origin, Advertised>>,
    broken: Mutex<HashMap<Origin, Broken>>,
}

fn origin(host: &str, port: u16) -> Origin {
    (host.to_ascii_lowercase(), port)
}

/// Keep a map bounded: drop expired entries, then arbitrary ones.
fn bound<V>(map: &mut HashMap<Origin, V>, expired: impl Fn(&V) -> bool) {
    if map.len() < MAX_ORIGINS {
        return;
    }
    map.retain(|_, v| !expired(v));
    while map.len() >= MAX_ORIGINS {
        let Some(key) = map.keys().next().cloned() else {
            break;
        };
        map.remove(&key);
    }
}

impl H3Knowledge {
    /// Record the `Alt-Svc` header values of a response from `host:port` (a response without
    /// the header changes nothing, as in browsers).
    pub(crate) fn observe<'a>(
        &self,
        host: &str,
        port: u16,
        values: impl IntoIterator<Item = &'a str>,
    ) {
        self.observe_at(host, port, values, Instant::now());
    }

    fn observe_at<'a>(
        &self,
        host: &str,
        port: u16,
        values: impl IntoIterator<Item = &'a str>,
        now: Instant,
    ) {
        let Some(parsed) = parse(values) else { return };
        let key = origin(host, port);
        let mut map = self.advertised.lock();
        match parsed {
            AltSvcHeader::Clear => {
                map.remove(&key);
            }
            AltSvcHeader::Alternatives(alts) => {
                // A new header replaces what the origin advertised before; a same-host
                // alternative is preferred (no extra DNS lookup, same server).
                let best = alts
                    .iter()
                    .find(|a| a.host.as_deref().is_none_or(|h| h == key.0))
                    .or_else(|| alts.first());
                match best {
                    Some(alt) if !alt.max_age.is_zero() => {
                        bound(&mut map, |v| v.expires <= now);
                        map.insert(
                            key.clone(),
                            Advertised {
                                route: H3Route {
                                    host: alt.host.clone().unwrap_or_else(|| key.0.clone()),
                                    port: alt.port,
                                },
                                expires: now + alt.max_age,
                            },
                        );
                    }
                    _ => {
                        map.remove(&key);
                    }
                }
            }
        }
    }

    /// The advertised (unexpired) HTTP/3 route of an origin.
    pub(crate) fn advertised(&self, host: &str, port: u16) -> Option<H3Route> {
        self.advertised_at(host, port, Instant::now())
    }

    fn advertised_at(&self, host: &str, port: u16, now: Instant) -> Option<H3Route> {
        let key = origin(host, port);
        let mut map = self.advertised.lock();
        match map.get(&key) {
            Some(a) if a.expires > now => Some(a.route.clone()),
            Some(_) => {
                map.remove(&key);
                None
            }
            None => None,
        }
    }

    /// Whether HTTP/3 to this origin failed recently.
    pub(crate) fn is_broken(&self, host: &str, port: u16) -> bool {
        self.is_broken_at(host, port, Instant::now())
    }

    fn is_broken_at(&self, host: &str, port: u16, now: Instant) -> bool {
        self.broken
            .lock()
            .get(&origin(host, port))
            .is_some_and(|b| b.until > now)
    }

    /// HTTP/3 to this origin failed: avoid it for a while (longer after each failure).
    pub(crate) fn mark_broken(&self, host: &str, port: u16) -> Duration {
        self.mark_broken_at(host, port, Instant::now())
    }

    fn mark_broken_at(&self, host: &str, port: u16, now: Instant) -> Duration {
        let mut map = self.broken.lock();
        bound(&mut map, |b| b.until <= now);
        let entry = map.entry(origin(host, port)).or_insert(Broken {
            until: now,
            failures: 0,
            marked: now,
        });
        if entry.failures > 0
            && entry.until > now
            && now.saturating_duration_since(entry.marked) < SAME_FAILURE_WINDOW
        {
            return entry.until - now;
        }
        entry.failures = entry.failures.saturating_add(1);
        entry.marked = now;
        let period = BROKEN_BASE
            .saturating_mul(1u32 << (entry.failures - 1).min(10))
            .min(BROKEN_MAX);
        entry.until = now + period;
        period
    }

    /// HTTP/3 to this origin works: forget earlier failures.
    pub(crate) fn mark_working(&self, host: &str, port: u16) {
        let key = origin(host, port);
        let mut map = self.broken.lock();
        if map.contains_key(&key) {
            map.remove(&key);
        }
    }

    /// Forget every failure (the HTTP/3 mode changed: the user wants a fresh try).
    pub(crate) fn clear_broken(&self) {
        self.broken.lock().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alts(v: &str) -> Vec<AltService> {
        match parse([v]) {
            Some(AltSvcHeader::Alternatives(a)) => a,
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_common_headers() {
        assert_eq!(
            alts(r#"h3=":443"; ma=86400, h3-29=":443"; ma=86400"#),
            vec![AltService {
                host: None,
                port: 443,
                max_age: Duration::from_secs(86400)
            }]
        );
        assert_eq!(
            alts(r#"h2="alt.example.com:8443", h3="Alt.Example.com:4433";ma=60;persist=1"#),
            vec![AltService {
                host: Some("alt.example.com".into()),
                port: 4433,
                max_age: Duration::from_secs(60)
            }]
        );
        // Default max-age, IPv6 authority, quoted ma, odd spacing.
        assert_eq!(
            alts(r#" h3 = "[2001:db8::1]:443" ; ma="120" "#),
            vec![AltService {
                host: Some("2001:db8::1".into()),
                port: 443,
                max_age: Duration::from_secs(120)
            }]
        );
        assert_eq!(alts(r#"h3=":443""#)[0].max_age, DEFAULT_MAX_AGE);
        assert_eq!(
            alts(r#"h3=":443"; ma=999999999999"#)[0].max_age,
            MAX_MAX_AGE
        );
        assert_eq!(parse(["clear"]), Some(AltSvcHeader::Clear));
        assert_eq!(parse(["  CLEAR "]), Some(AltSvcHeader::Clear));
        assert_eq!(parse(Vec::<&str>::new()), None);
    }

    #[test]
    fn rejects_malformed_alternatives() {
        for bad in [
            r#"h3=":0""#,
            r#"h3=":x""#,
            r#"h3="host""#,
            r#"h3"#,
            r#"h3="a b:443""#,
            r#"h3="[::1:443""#,
            r#"h3="[nope]:443""#,
            r#"h3="user@host:443""#,
            r#"h2=":443""#,
        ] {
            assert!(alts(bad).is_empty(), "{bad}");
        }
        // A comma inside a quoted string doesn't split alternatives.
        assert!(alts(r#"h3="a,b:443""#).is_empty());
    }

    #[test]
    fn multiple_field_values_combine() {
        let parsed = parse([r#"h2=":443""#, r#"h3=":8443"; ma=10"#]).unwrap();
        assert_eq!(
            parsed,
            AltSvcHeader::Alternatives(vec![AltService {
                host: None,
                port: 8443,
                max_age: Duration::from_secs(10)
            }])
        );
        assert_eq!(parse([r#"h3=":443""#, "clear"]), Some(AltSvcHeader::Clear));
    }

    #[test]
    fn advertised_routes_expire_and_clear() {
        let k = H3Knowledge::default();
        let t0 = Instant::now();
        assert_eq!(k.advertised("Example.com", 443), None);
        k.observe_at("Example.com", 443, [r#"h3=":443"; ma=60"#], t0);
        assert_eq!(
            k.advertised_at("example.com", 443, t0),
            Some(H3Route {
                host: "example.com".into(),
                port: 443
            })
        );
        assert!(k.advertised("example.com", 443).is_some());
        // Other ports are other origins.
        assert_eq!(k.advertised_at("example.com", 8443, t0), None);
        // A response without Alt-Svc keeps the knowledge.
        k.observe_at("example.com", 443, Vec::<&str>::new(), t0);
        assert!(k.advertised_at("example.com", 443, t0).is_some());
        let later = t0 + Duration::from_secs(61);
        assert_eq!(k.advertised_at("example.com", 443, later), None);
        assert!(
            k.advertised.lock().is_empty(),
            "expired entries are dropped"
        );

        k.observe_at("example.com", 443, [r#"h3="alt.example.net:4433""#], t0);
        assert_eq!(
            k.advertised_at("example.com", 443, t0),
            Some(H3Route {
                host: "alt.example.net".into(),
                port: 4433
            })
        );
        // Without `ma`, 24 hours.
        assert!(
            k.advertised_at("example.com", 443, t0 + Duration::from_secs(86_399))
                .is_some()
        );
        // Same-host alternatives win.
        k.observe_at(
            "example.com",
            443,
            [r#"h3="alt.example.net:4433", h3=":4434""#],
            t0,
        );
        assert_eq!(k.advertised_at("example.com", 443, t0).unwrap().port, 4434);
        // ma=0 and `clear` forget.
        k.observe_at("example.com", 443, [r#"h3=":443"; ma=0"#], t0);
        assert_eq!(k.advertised_at("example.com", 443, t0), None);
        k.observe_at("example.com", 443, [r#"h3=":443""#], t0);
        k.observe_at("example.com", 443, ["clear"], t0);
        assert_eq!(k.advertised_at("example.com", 443, t0), None);
        // A header with only other protocols withdraws h3.
        k.observe_at("example.com", 443, [r#"h3=":443""#], t0);
        k.observe_at("example.com", 443, [r#"h2=":443""#], t0);
        assert_eq!(k.advertised_at("example.com", 443, t0), None);
    }

    #[test]
    fn broken_backoff_doubles_and_resets() {
        let k = H3Knowledge::default();
        let t0 = Instant::now();
        assert!(!k.is_broken("a.test", 443));
        assert_eq!(k.mark_broken_at("a.test", 443, t0), BROKEN_BASE);
        assert!(k.is_broken_at("A.test", 443, t0));
        assert!(k.is_broken("a.test", 443));
        assert!(!k.is_broken_at("a.test", 8443, t0));
        let later = t0 + BROKEN_BASE + Duration::from_secs(1);
        assert!(!k.is_broken_at("a.test", 443, later));
        assert_eq!(k.mark_broken_at("a.test", 443, later), BROKEN_BASE * 2);
        // Concurrent requests reporting the same failure count once.
        let soon = later + Duration::from_secs(1);
        assert_eq!(
            k.mark_broken_at("a.test", 443, soon),
            BROKEN_BASE * 2 - Duration::from_secs(1)
        );
        let mut t = later;
        for _ in 0..20 {
            t += Duration::from_secs(10);
            k.mark_broken_at("a.test", 443, t);
        }
        t += Duration::from_secs(10);
        assert_eq!(k.mark_broken_at("a.test", 443, t), BROKEN_MAX);
        k.mark_working("a.test", 443);
        assert!(!k.is_broken_at("a.test", 443, later));
        assert_eq!(k.mark_broken("a.test", 443), BROKEN_BASE);
        k.clear_broken();
        assert!(!k.is_broken("a.test", 443));
    }

    #[test]
    fn maps_stay_bounded() {
        let k = H3Knowledge::default();
        for i in 0..(MAX_ORIGINS + 100) {
            k.observe(&format!("h{i}.test"), 443, [r#"h3=":443""#]);
            k.mark_broken(&format!("h{i}.test"), 443);
        }
        assert!(k.advertised.lock().len() <= MAX_ORIGINS);
        assert!(k.broken.lock().len() <= MAX_ORIGINS);
    }
}
