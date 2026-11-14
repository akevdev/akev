//! Proxy auto-config (PAC) support.
//!
//! * The script is fetched once (directly, never through a proxy) and cached for
//!   [`SCRIPT_TTL`]; on refresh failures the stale script keeps being used. If no script can be
//!   obtained, requests go DIRECT (browser behaviour) and the fetch is retried after
//!   [`FAILURE_BACKOFF`].
//! * `FindProxyForURL(url, host)` is evaluated by an embedded QuickJS engine (rquickjs) on a
//!   dedicated thread, with a memory limit and a per-call time limit (runaway scripts are
//!   interrupted). Results are cached per URL argument for [`RESULT_TTL`].
//! * Standard helpers: `isPlainHostName`, `dnsDomainIs`, `localHostOrDomainIs`, `isResolvable`,
//!   `isInNet`, `dnsResolve`, `myIpAddress`, `dnsDomainLevels`, `shExpMatch`, `weekdayRange`,
//!   `dateRange`, `timeRange`, `alert`, plus the Microsoft IPv6 extensions `dnsResolveEx`,
//!   `myIpAddressEx`, `isResolvableEx`, `isInNetEx`, `sortIpAddressList`. DNS helpers honour
//!   `settings.dns_overrides`.
//! * Result strings (`PROXY a:1; SOCKS5 b:2; DIRECT`) become ordered fail-over routes.

use super::bypass::{in_cidr, parse_cidr};
use super::{PacFetcher, ProxyKind, ProxyServer, ProxyTarget, Route, SelectEnv};
use crate::upstream::dns::{DnsResolver, parse_ip};
use crate::upstream::glob::split_host_port;
use parking_lot::Mutex;
use rquickjs::{Context, Function, Runtime};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How long a fetched PAC script is used before re-fetching.
const SCRIPT_TTL: Duration = Duration::from_secs(300);
/// After a failed fetch (and no cached script), wait this long before trying again.
const FAILURE_BACKOFF: Duration = Duration::from_secs(30);
/// How long a `FindProxyForURL` result is reused.
const RESULT_TTL: Duration = Duration::from_secs(30);
const MAX_CACHED_RESULTS: usize = 2048;
/// Per-call evaluation time limit.
const EVAL_TIMEOUT: Duration = Duration::from_secs(2);
const JS_MEMORY_LIMIT: usize = 32 * 1024 * 1024;

/// The standard PAC helper library (after Mozilla's `pac_utils.js` / Chromium's
/// `pac_js_library`), with regex-safe `shExpMatch`.
const PAC_UTILS: &str = r#"
function dnsDomainIs(host, domain) {
    host = String(host); domain = String(domain);
    return host.length >= domain.length && host.substring(host.length - domain.length) == domain;
}
function dnsDomainLevels(host) { return String(host).split('.').length - 1; }
function isPlainHostName(host) { host = String(host); return host.indexOf('.') == -1 && host.indexOf(':') == -1; }
function localHostOrDomainIs(host, hostdom) {
    return host == hostdom || String(hostdom).lastIndexOf(host + '.', 0) == 0;
}
function isResolvable(host) { return dnsResolve(host) !== null; }
function isResolvableEx(host) { return dnsResolveEx(host) !== ""; }
function convert_addr(ipchars) {
    var bytes = ipchars.split('.');
    return ((bytes[0] & 0xff) << 24) | ((bytes[1] & 0xff) << 16) | ((bytes[2] & 0xff) << 8) | (bytes[3] & 0xff);
}
function isInNet(ipaddr, pattern, maskstr) {
    var test = /^(\d{1,4})\.(\d{1,4})\.(\d{1,4})\.(\d{1,4})$/.exec(ipaddr);
    if (test == null) {
        ipaddr = dnsResolve(ipaddr);
        if (ipaddr == null) return false;
    } else if (test[1] > 255 || test[2] > 255 || test[3] > 255 || test[4] > 255) {
        return false;
    }
    var host = convert_addr(ipaddr);
    var pat = convert_addr(pattern);
    var mask = convert_addr(maskstr);
    return (host & mask) == (pat & mask);
}
function shExpMatch(url, pattern) {
    pattern = String(pattern).replace(/[.+^${}()|[\]\\]/g, '\\$&');
    pattern = pattern.replace(/\*/g, '.*');
    pattern = pattern.replace(/\?/g, '.');
    return new RegExp('^' + pattern + '$').test(String(url));
}
function sortIpAddressList(list) {
    if (!list) return false;
    var parts = String(list).split(';');
    var v6 = parts.filter(function (p) { return p.indexOf(':') != -1; });
    var v4 = parts.filter(function (p) { return p.indexOf(':') == -1; });
    return v6.concat(v4).join(';');
}
function alert(msg) {}
var wdays = {SUN: 0, MON: 1, TUE: 2, WED: 3, THU: 4, FRI: 5, SAT: 6};
var months = {JAN: 0, FEB: 1, MAR: 2, APR: 3, MAY: 4, JUN: 5, JUL: 6, AUG: 7, SEP: 8, OCT: 9, NOV: 10, DEC: 11};
function weekdayRange() {
    function getDay(weekday) { return (weekday in wdays) ? wdays[weekday] : -1; }
    var date = new Date();
    var argc = arguments.length;
    var wday;
    if (argc < 1) return false;
    if (arguments[argc - 1] == 'GMT') { argc--; wday = date.getUTCDay(); } else { wday = date.getDay(); }
    var wd1 = getDay(arguments[0]);
    var wd2 = (argc == 2) ? getDay(arguments[1]) : wd1;
    if (wd1 == -1 || wd2 == -1) return false;
    if (wd1 <= wd2) return wd1 <= wday && wday <= wd2;
    return wd2 >= wday || wday >= wd1;
}
function dateRange() {
    function getMonth(name) { return (name in months) ? months[name] : -1; }
    var date = new Date();
    var argc = arguments.length;
    if (argc < 1) return false;
    var isGMT = (arguments[argc - 1] == 'GMT');
    if (isGMT) argc--;
    if (argc == 1) {
        var tmp = parseInt(arguments[0]);
        if (isNaN(tmp)) return ((isGMT ? date.getUTCMonth() : date.getMonth()) == getMonth(arguments[0]));
        if (tmp < 32) return ((isGMT ? date.getUTCDate() : date.getDate()) == tmp);
        return ((isGMT ? date.getUTCFullYear() : date.getFullYear()) == tmp);
    }
    var year = date.getFullYear();
    var date1 = new Date(year, 0, 1, 0, 0, 0);
    var date2 = new Date(year, 11, 31, 23, 59, 59);
    var adjustMonth = false;
    for (var i = 0; i < (argc >> 1); i++) {
        var tmp = parseInt(arguments[i]);
        if (isNaN(tmp)) { date1.setMonth(getMonth(arguments[i])); }
        else if (tmp < 32) { adjustMonth = (argc <= 2); date1.setDate(tmp); }
        else { date1.setFullYear(tmp); }
    }
    for (var i = (argc >> 1); i < argc; i++) {
        var tmp = parseInt(arguments[i]);
        if (isNaN(tmp)) { date2.setMonth(getMonth(arguments[i])); }
        else if (tmp < 32) { date2.setDate(tmp); }
        else { date2.setFullYear(tmp); }
    }
    if (adjustMonth) { date1.setMonth(date.getMonth()); date2.setMonth(date.getMonth()); }
    if (isGMT) {
        var tmp = date;
        tmp.setFullYear(date.getUTCFullYear()); tmp.setMonth(date.getUTCMonth()); tmp.setDate(date.getUTCDate());
        tmp.setHours(date.getUTCHours()); tmp.setMinutes(date.getUTCMinutes()); tmp.setSeconds(date.getUTCSeconds());
        date = tmp;
    }
    return (date1 <= date2) ? (date1 <= date) && (date <= date2) : (date2 >= date) || (date >= date1);
}
function timeRange() {
    var argc = arguments.length;
    var date = new Date();
    var isGMT = false;
    if (argc < 1) return false;
    if (arguments[argc - 1] == 'GMT') { isGMT = true; argc--; }
    var hour = isGMT ? date.getUTCHours() : date.getHours();
    var date1 = new Date();
    var date2 = new Date();
    if (argc == 1) return hour == arguments[0];
    if (argc == 2) return (arguments[0] <= hour) && (hour < arguments[1]);
    switch (argc) {
    case 6:
        date1.setSeconds(arguments[2]);
        date2.setSeconds(arguments[5]);
    case 4:
        var middle = argc >> 1;
        date1.setHours(arguments[0]);
        date1.setMinutes(arguments[1]);
        date2.setHours(arguments[middle]);
        date2.setMinutes(arguments[middle + 1]);
        if (middle == 2) date2.setSeconds(59);
        break;
    default:
        throw 'timeRange: bad number of arguments';
    }
    if (isGMT) {
        date.setFullYear(date.getUTCFullYear()); date.setMonth(date.getUTCMonth()); date.setDate(date.getUTCDate());
        date.setHours(date.getUTCHours()); date.setMinutes(date.getUTCMinutes()); date.setSeconds(date.getUTCSeconds());
    }
    return (date1 <= date2) ? (date1 <= date) && (date <= date2) : (date2 >= date) || (date >= date1);
}
"#;

struct Job {
    script: Arc<str>,
    url: String,
    host: String,
    dns: Arc<DnsResolver>,
    reply: tokio::sync::oneshot::Sender<Result<String, String>>,
}

/// The PAC evaluation engine: a dedicated thread owning a QuickJS runtime (spawned lazily).
pub(crate) struct PacEngine {
    tx: Mutex<Option<mpsc::Sender<Job>>>,
}

impl Default for PacEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl PacEngine {
    pub(crate) fn new() -> Self {
        PacEngine {
            tx: Mutex::new(None),
        }
    }

    fn sender(&self) -> Result<mpsc::Sender<Job>, String> {
        let mut guard = self.tx.lock();
        if let Some(tx) = guard.as_ref() {
            return Ok(tx.clone());
        }
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("akev-pac".into())
            .spawn(move || run_engine(rx))
            .map_err(|e| format!("cannot start the PAC engine: {e}"))?;
        *guard = Some(tx.clone());
        Ok(tx)
    }

    /// Evaluate `FindProxyForURL(url, host)` with `script`.
    pub(crate) async fn evaluate(
        &self,
        script: Arc<str>,
        url: String,
        host: String,
        dns: Arc<DnsResolver>,
    ) -> Result<String, String> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let job = Job {
            script,
            url,
            host,
            dns,
            reply,
        };
        if let Err(mpsc::SendError(job)) = self.sender()?.send(job) {
            // The engine thread died (it never should); start a new one and retry once.
            *self.tx.lock() = None;
            self.sender()?
                .send(job)
                .map_err(|_| "the PAC engine is unavailable".to_owned())?;
        }
        rx.await.map_err(|_| "the PAC engine stopped".to_owned())?
    }
}

fn run_engine(rx: mpsc::Receiver<Job>) {
    let mut ctx: Option<JsPac> = None;
    while let Ok(job) = rx.recv() {
        let reuse = ctx
            .as_ref()
            .is_some_and(|c| Arc::ptr_eq(&c.script, &job.script) || *c.script == *job.script);
        if !reuse {
            ctx = None;
            match JsPac::new(job.script.clone(), &job.dns) {
                Ok(c) => ctx = Some(c),
                Err(e) => {
                    let _ = job.reply.send(Err(e));
                    continue;
                }
            }
        }
        let result = match ctx.as_mut() {
            Some(c) => c.find_proxy(&job.url, &job.host, &job.dns),
            None => Err("PAC script unavailable".to_owned()),
        };
        if result.is_err() {
            // A failed/interrupted evaluation may leave the context in a bad state.
            ctx = None;
        }
        let _ = job.reply.send(result);
    }
}

type DnsSlot = Arc<Mutex<Option<Arc<DnsResolver>>>>;

struct JsPac {
    script: Arc<str>,
    // Field order matters: the context must drop before the runtime.
    context: Context,
    _runtime: Runtime,
    deadline: Arc<AtomicU64>,
    epoch: Instant,
    dns: DnsSlot,
}

fn resolve_all(slot: &DnsSlot, host: &str) -> Vec<IpAddr> {
    if let Some(ip) = parse_ip(host) {
        return vec![ip];
    }
    let resolver = slot.lock().clone();
    match resolver {
        Some(r) => r
            .resolve_blocking(host)
            .map(|a| a.to_vec())
            .unwrap_or_default(),
        None => Vec::new(),
    }
}

/// The local address used for outbound traffic (no packets are sent).
fn my_ip(v6: bool) -> Option<IpAddr> {
    let (bind, target) = if v6 {
        ("[::]:0", "[2001:4860:4860::8888]:80")
    } else {
        ("0.0.0.0:0", "8.8.8.8:80")
    };
    let socket = std::net::UdpSocket::bind(bind).ok()?;
    socket.connect(target).ok()?;
    socket.local_addr().ok().map(|a| a.ip())
}

fn js_err(ctx: &rquickjs::Ctx<'_>, e: rquickjs::Error) -> String {
    if matches!(e, rquickjs::Error::Exception) {
        let value = ctx.catch();
        if let Some(exc) = value.as_exception() {
            return exc.message().unwrap_or_else(|| "exception".into());
        }
        if let Some(s) = value.as_string().and_then(|s| s.to_string().ok()) {
            return s;
        }
        return format!("{value:?}");
    }
    e.to_string()
}

impl JsPac {
    fn new(script: Arc<str>, dns: &Arc<DnsResolver>) -> Result<Self, String> {
        let runtime = Runtime::new().map_err(|e| format!("cannot create JS runtime: {e}"))?;
        runtime.set_memory_limit(JS_MEMORY_LIMIT);
        runtime.set_max_stack_size(512 * 1024);
        let epoch = Instant::now();
        let deadline = Arc::new(AtomicU64::new(u64::MAX));
        {
            let deadline = deadline.clone();
            runtime.set_interrupt_handler(Some(Box::new(move || {
                epoch.elapsed().as_millis() as u64 > deadline.load(Ordering::Relaxed)
            })));
        }
        let context =
            Context::full(&runtime).map_err(|e| format!("cannot create JS context: {e}"))?;
        let slot: DnsSlot = Arc::new(Mutex::new(Some(dns.clone())));
        deadline.store(
            epoch.elapsed().as_millis() as u64 + EVAL_TIMEOUT.as_millis() as u64,
            Ordering::Relaxed,
        );
        let setup = context.with(|ctx| -> Result<(), String> {
            let install = || -> rquickjs::Result<()> {
                let globals = ctx.globals();
                let s = slot.clone();
                globals.set(
                    "dnsResolve",
                    Function::new(ctx.clone(), move |host: String| -> Option<String> {
                        let ips = resolve_all(&s, &host);
                        ips.iter()
                            .find(|ip| ip.is_ipv4())
                            .or(ips.first())
                            .map(IpAddr::to_string)
                    })?,
                )?;
                let s = slot.clone();
                globals.set(
                    "dnsResolveEx",
                    Function::new(ctx.clone(), move |host: String| -> String {
                        resolve_all(&s, &host)
                            .iter()
                            .map(IpAddr::to_string)
                            .collect::<Vec<_>>()
                            .join(";")
                    })?,
                )?;
                globals.set(
                    "myIpAddress",
                    Function::new(ctx.clone(), || -> String {
                        my_ip(false).map_or_else(|| "127.0.0.1".to_owned(), |ip| ip.to_string())
                    })?,
                )?;
                globals.set(
                    "myIpAddressEx",
                    Function::new(ctx.clone(), || -> String {
                        [my_ip(true), my_ip(false)]
                            .into_iter()
                            .flatten()
                            .map(|ip| ip.to_string())
                            .collect::<Vec<_>>()
                            .join(";")
                    })?,
                )?;
                let s = slot.clone();
                globals.set(
                    "isInNetEx",
                    Function::new(ctx.clone(), move |host: String, prefix: String| -> bool {
                        let Some((net, bits)) = parse_cidr(&prefix) else {
                            return false;
                        };
                        resolve_all(&s, &host)
                            .into_iter()
                            .any(|ip| in_cidr(ip, net, bits))
                    })?,
                )?;
                ctx.eval::<(), _>(PAC_UTILS)?;
                ctx.eval::<(), _>(script.as_bytes())?;
                Ok(())
            };
            install().map_err(|e| format!("PAC script error: {}", js_err(&ctx, e)))
        });
        deadline.store(u64::MAX, Ordering::Relaxed);
        setup?;
        Ok(JsPac {
            script,
            context,
            _runtime: runtime,
            deadline,
            epoch,
            dns: slot,
        })
    }

    fn find_proxy(
        &mut self,
        url: &str,
        host: &str,
        dns: &Arc<DnsResolver>,
    ) -> Result<String, String> {
        *self.dns.lock() = Some(dns.clone());
        self.deadline.store(
            self.epoch.elapsed().as_millis() as u64 + EVAL_TIMEOUT.as_millis() as u64,
            Ordering::Relaxed,
        );
        let result = self.context.with(|ctx| -> Result<String, String> {
            let call = || -> rquickjs::Result<Option<String>> {
                let globals = ctx.globals();
                let f: Function = match globals.get::<_, Function>("FindProxyForURL") {
                    Ok(f) => f,
                    Err(_) => globals.get("FindProxyForURLEx")?,
                };
                let value: rquickjs::Value = f.call((url, host))?;
                if value.is_null() || value.is_undefined() {
                    return Ok(None);
                }
                match value.as_string() {
                    Some(s) => s.to_string().map(Some),
                    None => Ok(Some(format!("{value:?}"))),
                }
            };
            call()
                .map(|r| r.unwrap_or_else(|| "DIRECT".to_owned()))
                .map_err(|e| js_err(&ctx, e))
        });
        self.deadline.store(u64::MAX, Ordering::Relaxed);
        result.map_err(|e| format!("FindProxyForURL failed: {e}"))
    }
}

/// Parse a PAC result string into ordered routes (unknown entries skipped; empty → DIRECT).
pub(crate) fn parse_pac_result(result: &str) -> Vec<Route> {
    let mut routes = Vec::new();
    for entry in result.split(';') {
        let mut parts = entry.split_whitespace();
        let Some(kind) = parts.next() else { continue };
        let kind = kind.to_ascii_uppercase();
        if kind == "DIRECT" {
            routes.push(Route::Direct);
            continue;
        }
        let proxy_kind = match kind.as_str() {
            "PROXY" | "HTTP" => ProxyKind::Http,
            "HTTPS" => ProxyKind::Https,
            "SOCKS" | "SOCKS4" => ProxyKind::Socks4a,
            "SOCKS5" => ProxyKind::Socks5h,
            _ => continue,
        };
        let Some(addr) = parts.next() else { continue };
        let (host, port) = split_host_port(addr);
        if host.is_empty() {
            continue;
        }
        let port = port.unwrap_or(match proxy_kind {
            ProxyKind::Http => 80,
            ProxyKind::Https => 443,
            _ => 1080,
        });
        routes.push(Route::Proxy(Arc::new(ProxyServer::new(
            proxy_kind, host, port, None, None,
        ))));
    }
    if routes.is_empty() {
        routes.push(Route::Direct);
    }
    routes
}

/// Routes computed for a URL argument, with the time they were computed.
type CachedRoutes = (Arc<[Route]>, Instant);

/// A PAC script location with its cached script and results.
pub(crate) struct PacSource {
    url: String,
    script: Mutex<Option<(Arc<str>, Instant)>>,
    last_failure: Mutex<Option<Instant>>,
    fetch_lock: tokio::sync::Mutex<()>,
    results: Mutex<HashMap<String, CachedRoutes>>,
}

impl PacSource {
    pub(crate) fn new(url: &str) -> Self {
        PacSource {
            url: url.to_owned(),
            script: Mutex::new(None),
            last_failure: Mutex::new(None),
            fetch_lock: tokio::sync::Mutex::new(()),
            results: Mutex::new(HashMap::new()),
        }
    }

    /// The script URL.
    pub(crate) fn url(&self) -> &str {
        &self.url
    }

    fn fresh_script(&self) -> Option<Arc<str>> {
        self.script
            .lock()
            .as_ref()
            .filter(|(_, at)| at.elapsed() < SCRIPT_TTL)
            .map(|(s, _)| s.clone())
    }

    async fn script(&self, fetcher: &dyn PacFetcher) -> Option<Arc<str>> {
        if let Some(s) = self.fresh_script() {
            return Some(s);
        }
        let stale = self.script.lock().as_ref().map(|(s, _)| s.clone());
        if stale.is_none()
            && self
                .last_failure
                .lock()
                .is_some_and(|at| at.elapsed() < FAILURE_BACKOFF)
        {
            return None;
        }
        let _guard = self.fetch_lock.lock().await;
        if let Some(s) = self.fresh_script() {
            return Some(s);
        }
        match fetcher.fetch_pac(&self.url).await {
            Ok(text) => {
                let script: Arc<str> = text.into();
                *self.script.lock() = Some((script.clone(), Instant::now()));
                *self.last_failure.lock() = None;
                self.results.lock().clear();
                Some(script)
            }
            Err(e) => {
                tracing::warn!(url = %self.url, error = %e.message, "cannot fetch PAC script");
                *self.last_failure.lock() = Some(Instant::now());
                // Keep using the stale script; retry after the backoff.
                if let Some(s) = &stale {
                    let retry_at = Instant::now()
                        .checked_sub(SCRIPT_TTL - FAILURE_BACKOFF)
                        .unwrap_or_else(Instant::now);
                    *self.script.lock() = Some((s.clone(), retry_at));
                }
                stale
            }
        }
    }

    /// Candidate routes for `target` (DIRECT when the script is unavailable or fails).
    pub(crate) async fn select(&self, target: &ProxyTarget<'_>, env: &SelectEnv<'_>) -> Vec<Route> {
        // Like browsers, strip path and query for TLS targets (they're not visible to a proxy).
        let url_arg = if target.is_tls() {
            format!(
                "{}://{}/",
                target.scheme,
                super::host_port(target.host, target.port)
            )
        } else {
            target.url.to_owned()
        };
        if let Some((routes, at)) = self.results.lock().get(&url_arg)
            && at.elapsed() < RESULT_TTL
        {
            return routes.to_vec();
        }
        let Some(script) = self.script(env.fetcher).await else {
            return vec![Route::Direct];
        };
        match env
            .pac
            .evaluate(
                script,
                url_arg.clone(),
                target.host.to_owned(),
                env.dns.clone(),
            )
            .await
        {
            Ok(result) => {
                let routes = parse_pac_result(&result);
                let mut cache = self.results.lock();
                if cache.len() >= MAX_CACHED_RESULTS {
                    cache.clear();
                }
                cache.insert(url_arg, (routes.clone().into(), Instant::now()));
                routes
            }
            Err(e) => {
                tracing::warn!(url = %self.url, error = %e, "PAC evaluation failed; connecting directly");
                vec![Route::Direct]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use akev_core::settings::DnsOverride;
    use futures::future::BoxFuture;

    fn dns() -> Arc<DnsResolver> {
        Arc::new(DnsResolver::new(
            &[DnsOverride {
                host: "intranet.corp".into(),
                address: "10.1.2.3".into(),
            }],
            None,
        ))
    }

    async fn eval(script: &str, url: &str, host: &str) -> Result<String, String> {
        let engine = PacEngine::new();
        engine
            .evaluate(script.into(), url.into(), host.into(), dns())
            .await
    }

    #[test]
    fn result_parsing() {
        let routes = parse_pac_result(
            "PROXY proxy.a:8080; SOCKS5 socks.b:1081 ;HTTPS secure.c; SOCKS old.d:1080; DIRECT; BOGUS x:1",
        );
        assert_eq!(routes.len(), 5);
        let kinds: Vec<_> = routes
            .iter()
            .map(|r| match r {
                Route::Direct => None,
                Route::Proxy(p) => Some((p.kind, p.host.clone(), p.port)),
            })
            .collect();
        assert_eq!(kinds[0], Some((ProxyKind::Http, "proxy.a".into(), 8080)));
        assert_eq!(kinds[1], Some((ProxyKind::Socks5h, "socks.b".into(), 1081)));
        assert_eq!(kinds[2], Some((ProxyKind::Https, "secure.c".into(), 443)));
        assert_eq!(kinds[3], Some((ProxyKind::Socks4a, "old.d".into(), 1080)));
        assert_eq!(kinds[4], None);
        assert!(matches!(parse_pac_result("")[..], [Route::Direct]));
        assert!(matches!(parse_pac_result("garbage")[..], [Route::Direct]));
    }

    #[tokio::test]
    async fn standard_helpers() {
        let script = r#"
            function FindProxyForURL(url, host) {
                var checks = [
                    isPlainHostName("intranet"), !isPlainHostName("a.b"),
                    dnsDomainIs("www.example.com", ".example.com"), !dnsDomainIs("www.other.com", ".example.com"),
                    localHostOrDomainIs("www", "www.example.com"), localHostOrDomainIs("www.example.com", "www.example.com"),
                    !localHostOrDomainIs("www.other.com", "www.example.com"),
                    dnsDomainLevels("a.b.c") == 2, dnsDomainLevels("plain") == 0,
                    shExpMatch("http://x.example.com/path", "*.example.com/*"), !shExpMatch("abc", "a?d"),
                    shExpMatch("a+b.c", "a+b.c"), !shExpMatch("aab", "a+b"),
                    isInNet("10.1.2.3", "10.0.0.0", "255.0.0.0"), !isInNet("11.1.2.3", "10.0.0.0", "255.0.0.0"),
                    isInNet("intranet.corp", "10.1.0.0", "255.255.0.0"),
                    dnsResolve("intranet.corp") == "10.1.2.3", isResolvable("intranet.corp"),
                    dnsResolve("127.0.0.1") == "127.0.0.1",
                    isInNetEx("intranet.corp", "10.0.0.0/8"),
                    typeof myIpAddress() == "string", myIpAddress().length > 0,
                    weekdayRange("SUN", "SAT"), timeRange(0, 24), dateRange("JAN", "DEC"),
                    sortIpAddressList("10.0.0.1;::1") == "::1;10.0.0.1"
                ];
                for (var i = 0; i < checks.length; i++) { if (!checks[i]) return "PROXY failed-check-" + i + ":1"; }
                return "PROXY ok:8080; DIRECT";
            }"#;
        assert_eq!(
            eval(script, "http://example.com/", "example.com")
                .await
                .unwrap(),
            "PROXY ok:8080; DIRECT"
        );
    }

    #[tokio::test]
    async fn routing_decisions() {
        let script = r#"
            function FindProxyForURL(url, host) {
                if (isPlainHostName(host) || dnsDomainIs(host, ".internal.test")) return "DIRECT";
                if (url.substring(0, 6) == "https:") return "PROXY secure-proxy:3128";
                return "SOCKS5 socks:1080; DIRECT";
            }"#;
        assert_eq!(
            eval(script, "http://printer/", "printer").await.unwrap(),
            "DIRECT"
        );
        assert_eq!(
            eval(script, "https://a.internal.test/", "a.internal.test")
                .await
                .unwrap(),
            "DIRECT"
        );
        assert_eq!(
            eval(script, "https://example.com/", "example.com")
                .await
                .unwrap(),
            "PROXY secure-proxy:3128"
        );
        assert_eq!(
            eval(script, "http://example.com/x", "example.com")
                .await
                .unwrap(),
            "SOCKS5 socks:1080; DIRECT"
        );
    }

    #[tokio::test]
    async fn errors_and_runaway_scripts() {
        assert!(
            eval("function FindProxyForURL( {", "http://a/", "a")
                .await
                .unwrap_err()
                .contains("PAC script error")
        );
        assert!(
            eval(
                "function FindProxyForURL(u, h) { throw new Error('nope'); }",
                "http://a/",
                "a"
            )
            .await
            .unwrap_err()
            .contains("nope")
        );
        let started = Instant::now();
        let err = eval(
            "function FindProxyForURL(u, h) { while (true) {} }",
            "http://a/",
            "a",
        )
        .await
        .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(10), "{err}");
        assert_eq!(
            eval(
                "function FindProxyForURL(u, h) { return null; }",
                "http://a/",
                "a"
            )
            .await
            .unwrap(),
            "DIRECT"
        );
    }

    struct StaticFetch(&'static str, std::sync::atomic::AtomicUsize);
    impl PacFetcher for StaticFetch {
        fn fetch_pac<'a>(
            &'a self,
            _url: &'a str,
        ) -> BoxFuture<'a, Result<String, crate::upstream::UpstreamError>> {
            self.1.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move { Ok(self.0.to_owned()) })
        }
    }

    #[tokio::test]
    async fn source_caches_script_and_results() {
        let fetch = StaticFetch(
            "function FindProxyForURL(url, host) { return host == 'direct.test' ? 'DIRECT' : 'PROXY p.test:3128'; }",
            Default::default(),
        );
        let engine = PacEngine::new();
        let resolver = dns();
        let env = SelectEnv {
            pac: &engine,
            fetcher: &fetch,
            dns: &resolver,
        };
        let source = PacSource::new("http://wpad.test/proxy.pac");
        let t = |host| ProxyTarget {
            scheme: "https",
            host,
            port: 443,
            url: "https://ignored/path?q",
        };
        for _ in 0..3 {
            let routes = source.select(&t("example.com"), &env).await;
            assert!(
                matches!(&routes[..], [Route::Proxy(p)] if p.host == "p.test" && p.port == 3128)
            );
        }
        assert!(matches!(
            source.select(&t("direct.test"), &env).await[..],
            [Route::Direct]
        ));
        assert_eq!(fetch.1.load(Ordering::SeqCst), 1);
    }
}
