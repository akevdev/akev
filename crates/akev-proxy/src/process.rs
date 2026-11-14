//! Platform process resolution: which local process owns a client TCP connection.
//!
//! * Linux: find the socket inode in `/proc/net/tcp{,6}`, then the process holding
//!   `socket:[inode]` in `/proc/*/fd` (inode → pid map cached and refreshed on misses).
//! * macOS: libproc (`proc_listpids` → `proc_pidinfo(PROC_PIDLISTFDS)` →
//!   `proc_pidfdinfo(PROC_PIDFDSOCKETINFO)`), recently matched processes first; `lsof` if the
//!   kernel struct layout isn't the expected one.
//! * Windows: `GetExtendedTcpTable` + `QueryFullProcessImageNameW`.
//!
//! Only loopback clients are resolved; results are cached per client port for 5 seconds.

use super::*;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

const CACHE_TTL: Duration = Duration::from_secs(5);
const CACHE_MAX: usize = 4096;

/// Resolves which local process owns a client TCP connection (best effort), on the current
/// platform.
pub struct SystemProcessResolver {
    cache: Mutex<HashMap<u16, (Instant, Option<ProcessInfo>)>>,
    #[cfg(target_os = "linux")]
    inodes: Mutex<linux::InodeMap>,
}

impl SystemProcessResolver {
    /// A resolver with empty caches.
    pub fn new() -> Self {
        SystemProcessResolver {
            cache: Mutex::new(HashMap::new()),
            #[cfg(target_os = "linux")]
            inodes: Mutex::new(linux::InodeMap::default()),
        }
    }

    fn lookup(&self, peer: SocketAddr, local: SocketAddr) -> Option<ProcessInfo> {
        #[cfg(target_os = "linux")]
        {
            linux::find(peer, local, &self.inodes)
        }
        #[cfg(target_os = "macos")]
        {
            macos::find(peer, local)
        }
        #[cfg(windows)]
        {
            windows::find(peer, local)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        {
            let _ = (peer, local);
            None
        }
    }
}

impl Default for SystemProcessResolver {
    fn default() -> Self {
        Self::new()
    }
}

use akev_core::access::is_loopback;

impl SystemProcessResolver {
    /// `peer` = client address as seen by the proxy, `local` = proxy's local address.
    pub fn resolve(&self, peer: SocketAddr, local: SocketAddr) -> Option<ProcessInfo> {
        // Local clients use loopback, or — when redirected by the firewall, or connecting to
        // a LAN-bound proxy — one of this host's own addresses.
        if !is_loopback(peer.ip()) && !crate::transparent::is_host_address(peer.ip()) {
            return None;
        }
        if let Some((at, info)) = self.cache.lock().get(&peer.port())
            && at.elapsed() < CACHE_TTL
        {
            return info.clone();
        }
        let info = self.lookup(peer, local);
        let mut cache = self.cache.lock();
        if cache.len() >= CACHE_MAX {
            cache.retain(|_, (at, _)| at.elapsed() < CACHE_TTL);
        }
        cache.insert(peer.port(), (Instant::now(), info.clone()));
        info
    }
}

/// `/foo/bar/name` → `name`.
#[allow(dead_code)]
fn file_name(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::fs;
    use std::net::{Ipv4Addr, Ipv6Addr};

    /// Socket inode → pid knowledge gathered from `/proc/*/fd` scans.
    ///
    /// New client connections almost always come from processes that opened connections
    /// before (browsers, CLIs in a loop), so a miss first re-scans only the recently seen owner
    /// processes, and falls back to a full `/proc` walk (which stops as soon as the inode is
    /// found) only when that fails.
    #[derive(Default)]
    pub(super) struct InodeMap {
        map: HashMap<u64, u32>,
        /// Most recently matched owner pids, newest first.
        recent: Vec<u32>,
    }

    const RECENT_PIDS: usize = 16;
    const MAX_MAPPED_INODES: usize = 65_536;

    impl InodeMap {
        fn remember(&mut self, inode: u64, pid: u32) {
            if self.map.len() >= MAX_MAPPED_INODES {
                self.map.clear();
            }
            self.map.insert(inode, pid);
            self.recent.retain(|&p| p != pid);
            self.recent.insert(0, pid);
            self.recent.truncate(RECENT_PIDS);
        }
    }

    fn normalize(addr: SocketAddr) -> SocketAddr {
        match addr.ip() {
            IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
                Some(v4) => SocketAddr::new(IpAddr::V4(v4), addr.port()),
                None => addr,
            },
            _ => addr,
        }
    }

    /// Parse `0100007F:1F90` (IPv4) or 32 hex digits (IPv6) + port, as printed by the kernel
    /// (address words in host byte order).
    pub(super) fn parse_addr(s: &str) -> Option<SocketAddr> {
        let (ip_hex, port_hex) = s.split_once(':')?;
        let port = u16::from_str_radix(port_hex, 16).ok()?;
        let ip = match ip_hex.len() {
            8 => IpAddr::V4(Ipv4Addr::from(
                u32::from_str_radix(ip_hex, 16).ok()?.to_ne_bytes(),
            )),
            32 => {
                let mut bytes = [0u8; 16];
                for (i, chunk) in bytes.chunks_mut(4).enumerate() {
                    let word = u32::from_str_radix(ip_hex.get(i * 8..i * 8 + 8)?, 16).ok()?;
                    chunk.copy_from_slice(&word.to_ne_bytes());
                }
                IpAddr::V6(Ipv6Addr::from(bytes))
            }
            _ => return None,
        };
        Some(normalize(SocketAddr::new(ip, port)))
    }

    /// Find the inode of the client's socket (local = `peer`, remote = `proxy`).
    pub(super) fn find_inode(table: &str, peer: SocketAddr, proxy: SocketAddr) -> Option<u64> {
        let (peer, proxy) = (normalize(peer), normalize(proxy));
        table.lines().skip(1).find_map(|line| {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() < 10 {
                return None;
            }
            let local = parse_addr(f[1])?;
            let remote = parse_addr(f[2])?;
            if local.port() != peer.port() || remote.port() != proxy.port() {
                return None;
            }
            if local.ip() != peer.ip() && !(local.ip().is_loopback() && peer.ip().is_loopback()) {
                return None;
            }
            f[9].parse::<u64>().ok().filter(|&i| i != 0)
        })
    }

    /// `socket:[123]` → 123.
    fn socket_inode(target: &std::path::Path) -> Option<u64> {
        target
            .to_str()?
            .strip_prefix("socket:[")?
            .strip_suffix(']')?
            .parse()
            .ok()
    }

    /// Scan one process's fds; records every socket seen and reports whether `inode` is one.
    fn scan_pid(pid: u32, inode: u64, found: &mut HashMap<u64, u32>) -> bool {
        let Ok(fds) = fs::read_dir(format!("/proc/{pid}/fd")) else {
            return false;
        };
        let mut hit = false;
        for fd in fds.flatten() {
            if let Some(i) = fs::read_link(fd.path())
                .ok()
                .as_deref()
                .and_then(socket_inode)
            {
                found.insert(i, pid);
                hit |= i == inode;
            }
        }
        hit
    }

    /// Which pid holds the socket `inode`.
    fn pid_for(inode: u64, cache: &Mutex<InodeMap>) -> Option<u32> {
        let recent = {
            let c = cache.lock();
            if let Some(pid) = c.map.get(&inode) {
                return Some(*pid);
            }
            c.recent.clone()
        };
        let mut found = HashMap::new();
        let mut owner = recent
            .into_iter()
            .find(|&pid| scan_pid(pid, inode, &mut found));
        if owner.is_none()
            && let Ok(dir) = fs::read_dir("/proc")
        {
            owner = dir
                .flatten()
                .filter_map(|e| e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()))
                .find(|&pid| scan_pid(pid, inode, &mut found));
        }
        let mut c = cache.lock();
        for (i, pid) in found {
            if i != inode {
                c.map.insert(i, pid);
            }
        }
        if let Some(pid) = owner {
            c.remember(inode, pid);
        }
        owner
    }

    pub(super) fn find(
        peer: SocketAddr,
        proxy: SocketAddr,
        cache: &Mutex<InodeMap>,
    ) -> Option<ProcessInfo> {
        let inode = ["/proc/net/tcp", "/proc/net/tcp6"]
            .iter()
            .filter_map(|p| fs::read_to_string(p).ok())
            .find_map(|table| find_inode(&table, peer, proxy))?;
        let pid = pid_for(inode, cache)?;
        let name = fs::read_to_string(format!("/proc/{pid}/comm"))
            .ok()
            .map(|s| s.trim().to_string())?;
        let path = fs::read_link(format!("/proc/{pid}/exe"))
            .ok()
            .and_then(|p| {
                p.to_str()
                    .map(|s| s.trim_end_matches(" (deleted)").to_string())
            });
        Some(ProcessInfo { pid, name, path })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        const TABLE: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:1F40 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 11111 1 0000000000000000 100 0 0 10 0
   1: 0100007F:D431 0100007F:1F40 01 00000000:00000000 00:00000000 00000000  1000        0 22222 1 0000000000000000 20 4 30 10 -1
   2: 0100007F:1F40 0100007F:D431 01 00000000:00000000 00:00000000 00000000  1000        0 33333 1 0000000000000000 20 4 30 10 -1";

        #[test]
        fn parses_kernel_addresses() {
            if cfg!(target_endian = "little") {
                assert_eq!(
                    parse_addr("0100007F:1F40"),
                    Some("127.0.0.1:8000".parse().unwrap())
                );
                assert_eq!(
                    parse_addr("00000000000000000000000001000000:0050"),
                    Some("[::1]:80".parse().unwrap())
                );
                assert_eq!(
                    parse_addr("0000000000000000FFFF00000100007F:0050"),
                    Some("127.0.0.1:80".parse().unwrap())
                );
            }
            assert_eq!(parse_addr("zz:1"), None);
        }

        #[test]
        fn finds_client_side_socket() {
            if cfg!(target_endian = "little") {
                let peer = "127.0.0.1:54321".parse().unwrap();
                let proxy = "127.0.0.1:8000".parse().unwrap();
                assert_eq!(find_inode(TABLE, peer, proxy), Some(22222));
                assert_eq!(
                    find_inode(TABLE, "127.0.0.1:1".parse().unwrap(), proxy),
                    None
                );
            }
        }

        #[test]
        fn socket_link_parsing() {
            assert_eq!(
                socket_inode(std::path::Path::new("socket:[4242]")),
                Some(4242)
            );
            assert_eq!(socket_inode(std::path::Path::new("pipe:[4242]")), None);
            assert_eq!(socket_inode(std::path::Path::new("/dev/null")), None);
        }

        #[test]
        fn back_to_back_connections_all_resolve() {
            // Regression: a second connection right after a full scan must still resolve
            // (previously misses within 250ms of a scan returned None).
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let resolver = SystemProcessResolver::new();
            for _ in 0..3 {
                let client = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
                let (_server, peer) = listener.accept().unwrap();
                let info = resolver.resolve(peer, listener.local_addr().unwrap());
                drop(client);
                assert_eq!(info.map(|i| i.pid), Some(std::process::id()));
            }
        }

        #[test]
        fn resolves_own_connection() {
            // A real loopback connection made by this test process resolves to us.
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let client = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
            let (_server, peer) = listener.accept().unwrap();
            let resolver = SystemProcessResolver::new();
            let info = resolver.resolve(peer, listener.local_addr().unwrap());
            drop(client);
            let info = info.expect("own process should be found");
            assert_eq!(info.pid, std::process::id());
            assert!(!info.name.is_empty());
            // Cached afterwards.
            assert_eq!(
                resolver
                    .resolve(peer, listener.local_addr().unwrap())
                    .map(|i| i.pid),
                Some(std::process::id())
            );
        }
    }
}

/// The part of `struct socket_fdinfo` (`<sys/proc_info.h>`) that macOS process attribution
/// reads, as byte offsets into the buffer `proc_pidfdinfo(PROC_PIDFDSOCKETINFO)` fills, and
/// the matching of a client socket. Platform independent so it is unit tested everywhere;
/// the macOS resolver also checks the layout against one of its own sockets at run time and
/// falls back to `lsof` if it doesn't hold.
///
/// `socket_fdinfo` = `proc_fileinfo` (24 bytes) + `socket_info`; in `socket_info`,
/// `soi_stat` (`vinfo_stat`, 136 bytes) is followed by `soi_so`, `soi_pcb`, `soi_type`,
/// `soi_protocol`, `soi_family`, eight shorts, `soi_oobmark`, two `sockbuf_info` (24 bytes
/// each), `soi_kind` (232) and `rfu_1`, then the protocol union at 240. Its TCP member starts
/// with `in_sockinfo`: `insi_fport`, `insi_lport` (ints holding the port in network order),
/// `insi_gencnt`, `insi_flags`, `insi_flow`, `insi_vflag` (24), `insi_ip_ttl`, `rfu_1`, then
/// `insi_faddr` (32) and `insi_laddr` (48), 16 bytes each (`in4in6_addr` or `in6_addr`).
#[cfg(any(target_os = "macos", test))]
mod socket_fdinfo {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

    /// `socket_info` starts after `proc_fileinfo`.
    const SOI: usize = 24;
    /// `soi_kind`.
    pub(super) const KIND: usize = SOI + 232;
    /// `soi_proto.pri_tcp.tcpsi_ini` (`struct in_sockinfo`).
    const INSI: usize = SOI + 240;
    pub(super) const FPORT: usize = INSI;
    pub(super) const LPORT: usize = INSI + 4;
    pub(super) const VFLAG: usize = INSI + 24;
    pub(super) const FADDR: usize = INSI + 32;
    pub(super) const LADDR: usize = INSI + 48;
    /// Bytes this parser needs.
    pub(super) const MIN_LEN: usize = LADDR + 16;
    /// The buffer given to the kernel: it only checks that it holds a `socket_fdinfo` (792
    /// bytes on current macOS) and fills that much.
    pub(super) const BUF_LEN: usize = 1024;
    /// `SOCKINFO_TCP`.
    pub(super) const SOCKINFO_TCP: i32 = 2;
    /// `INI_IPV4` / `INI_IPV6` in `insi_vflag`.
    pub(super) const INI_IPV4: u8 = 0x1;
    pub(super) const INI_IPV6: u8 = 0x2;

    /// The two ends of a TCP socket, as the owning process sees them.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(super) struct TcpEnds {
        pub local: SocketAddr,
        pub foreign: SocketAddr,
    }

    fn i32_at(buf: &[u8], off: usize) -> Option<i32> {
        Some(i32::from_ne_bytes(buf.get(off..off + 4)?.try_into().ok()?))
    }

    /// An `insi_*port` int: the port in network byte order in its low 16 bits.
    fn port_at(buf: &[u8], off: usize) -> Option<u16> {
        let raw = (i32_at(buf, off)? as u32 & 0xffff) as u16;
        Some(u16::from_be(raw))
    }

    fn addr_at(buf: &[u8], off: usize, vflag: u8) -> Option<IpAddr> {
        let bytes: [u8; 16] = buf.get(off..off + 16)?.try_into().ok()?;
        if vflag & INI_IPV4 != 0 {
            // `in4in6_addr`: three padding words, then the IPv4 address.
            Some(IpAddr::V4(Ipv4Addr::new(
                bytes[12], bytes[13], bytes[14], bytes[15],
            )))
        } else if vflag & INI_IPV6 != 0 {
            Some(IpAddr::V6(Ipv6Addr::from(bytes)))
        } else {
            None
        }
    }

    /// The ends of a TCP socket from a `socket_fdinfo` buffer (`None` for other sockets).
    pub(super) fn tcp_ends(buf: &[u8]) -> Option<TcpEnds> {
        if buf.len() < MIN_LEN || i32_at(buf, KIND)? != SOCKINFO_TCP {
            return None;
        }
        let vflag = *buf.get(VFLAG)?;
        Some(TcpEnds {
            local: SocketAddr::new(addr_at(buf, LADDR, vflag)?, port_at(buf, LPORT)?),
            foreign: SocketAddr::new(addr_at(buf, FADDR, vflag)?, port_at(buf, FPORT)?),
        })
    }

    /// Whether `ends` is the client side of the proxy connection from `peer` (the client's
    /// address as the proxy sees it) to `proxy` (the proxy's listening address).
    pub(super) fn is_client_socket(ends: &TcpEnds, peer: SocketAddr, proxy: SocketAddr) -> bool {
        ends.local.port() == peer.port()
            && ends.foreign.port() == proxy.port()
            && ends.local.ip().to_canonical() == peer.ip().to_canonical()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// A `socket_fdinfo` buffer as the kernel fills it for a TCP socket.
        pub(crate) fn buffer(local: SocketAddr, foreign: SocketAddr) -> Vec<u8> {
            let mut buf = vec![0u8; 792];
            buf[KIND..KIND + 4].copy_from_slice(&SOCKINFO_TCP.to_ne_bytes());
            let port = |p: u16| i32::from(p.to_be()).to_ne_bytes();
            buf[LPORT..LPORT + 4].copy_from_slice(&port(local.port()));
            buf[FPORT..FPORT + 4].copy_from_slice(&port(foreign.port()));
            let mut put = |off: usize, ip: IpAddr| match ip {
                IpAddr::V4(v4) => buf[off + 12..off + 16].copy_from_slice(&v4.octets()),
                IpAddr::V6(v6) => buf[off..off + 16].copy_from_slice(&v6.octets()),
            };
            put(LADDR, local.ip());
            put(FADDR, foreign.ip());
            buf[VFLAG] = if local.is_ipv4() { INI_IPV4 } else { INI_IPV6 };
            buf
        }

        #[test]
        fn offsets_follow_the_kernel_structs() {
            // proc_fileinfo (24) + vinfo_stat (136) + soi_so, soi_pcb (16) + soi_type,
            // soi_protocol, soi_family (12) + 8 shorts (16) + soi_oobmark (4) + 2
            // sockbuf_info (48) = soi_kind; then soi_kind + rfu_1 (8) = the protocol union.
            assert_eq!(KIND, 24 + 136 + 16 + 12 + 16 + 4 + 48);
            assert_eq!(LPORT, KIND + 8 + 4);
            assert_eq!(VFLAG, KIND + 8 + 24);
            assert_eq!(LADDR + 16, MIN_LEN);
            const { assert!(MIN_LEN <= 792 && 792 <= BUF_LEN) };
        }

        #[test]
        fn parses_ipv4_and_ipv6_tcp_sockets() {
            let local: SocketAddr = "127.0.0.1:53124".parse().unwrap();
            let foreign: SocketAddr = "127.0.0.1:8000".parse().unwrap();
            let ends = tcp_ends(&buffer(local, foreign)).unwrap();
            assert_eq!(ends, TcpEnds { local, foreign });

            let local: SocketAddr = "[::1]:60000".parse().unwrap();
            let foreign: SocketAddr = "[::1]:8443".parse().unwrap();
            assert_eq!(
                tcp_ends(&buffer(local, foreign)).unwrap(),
                TcpEnds { local, foreign }
            );
        }

        #[test]
        fn other_sockets_and_short_buffers_are_ignored() {
            let a: SocketAddr = "127.0.0.1:1".parse().unwrap();
            let mut udp = buffer(a, a);
            udp[KIND..KIND + 4].copy_from_slice(&1i32.to_ne_bytes());
            assert_eq!(tcp_ends(&udp), None);
            assert_eq!(tcp_ends(&buffer(a, a)[..MIN_LEN - 1]), None);
            let mut no_family = buffer(a, a);
            no_family[VFLAG] = 0;
            assert_eq!(tcp_ends(&no_family), None);
        }

        #[test]
        fn matches_only_the_client_end_of_the_proxy_connection() {
            let proxy: SocketAddr = "127.0.0.1:8000".parse().unwrap();
            let peer: SocketAddr = "127.0.0.1:53124".parse().unwrap();
            let client = tcp_ends(&buffer(peer, proxy)).unwrap();
            assert!(is_client_socket(&client, peer, proxy));
            // The proxy's own (accepted) end has the ports the other way round.
            let accepted = tcp_ends(&buffer(proxy, peer)).unwrap();
            assert!(!is_client_socket(&accepted, peer, proxy));
            // Another connection from the same port number to another server.
            let elsewhere = tcp_ends(&buffer(peer, "127.0.0.1:9999".parse().unwrap())).unwrap();
            assert!(!is_client_socket(&elsewhere, peer, proxy));
            // A dual-stack listener sees the IPv4 client as v4-mapped.
            let mapped: SocketAddr = "[::ffff:127.0.0.1]:53124".parse().unwrap();
            let mapped_proxy: SocketAddr = "[::]:8000".parse().unwrap();
            assert!(is_client_socket(&client, mapped, mapped_proxy));
        }
    }
}

/// macOS: libproc. List the processes (most recently matched first), then their socket
/// file descriptors, and find the one whose TCP ends are the client's side of the
/// connection: a handful of system calls, where `lsof` + `ps` cost two process launches per
/// new client port. The layout of the kernel structs is checked once against one of our
/// own sockets; if it doesn't match (a future macOS), `lsof` is used instead.
#[cfg(target_os = "macos")]
mod macos {
    use super::socket_fdinfo::{self, BUF_LEN, TcpEnds};
    use super::*;
    use std::ffi::{c_int, c_void};
    use std::sync::OnceLock;

    /// `PROC_ALL_PIDS` (`<sys/proc_info.h>`).
    const PROC_ALL_PIDS: u32 = 1;
    /// `PROC_PIDFDSOCKETINFO`.
    const PROC_PIDFDSOCKETINFO: c_int = 3;
    /// Processes that owned client connections recently (checked first).
    const HOT_PIDS: usize = 16;

    static HOT: Mutex<Vec<c_int>> = Mutex::new(Vec::new());

    fn pids() -> Vec<c_int> {
        // SAFETY: a null buffer asks for the size needed (in bytes).
        let needed = unsafe { libc::proc_listpids(PROC_ALL_PIDS, 0, std::ptr::null_mut(), 0) };
        if needed <= 0 {
            return Vec::new();
        }
        // Room for processes started in between.
        let cap = usize::try_from(needed).unwrap_or(0) / size_of::<c_int>() + 64;
        let mut buf: Vec<c_int> = vec![0; cap];
        let bytes = c_int::try_from(cap * size_of::<c_int>()).unwrap_or(c_int::MAX);
        // SAFETY: `buf` is valid for `bytes` bytes for the duration of the call.
        let filled =
            unsafe { libc::proc_listpids(PROC_ALL_PIDS, 0, buf.as_mut_ptr().cast(), bytes) };
        let Ok(filled) = usize::try_from(filled) else {
            return Vec::new();
        };
        buf.truncate(filled / size_of::<c_int>());
        buf.retain(|pid| *pid > 0);
        buf
    }

    fn socket_fds(pid: c_int) -> Vec<c_int> {
        let item = size_of::<libc::proc_fdinfo>();
        // SAFETY: a null buffer asks for the size needed (in bytes).
        let needed =
            unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0) };
        if needed <= 0 {
            return Vec::new();
        }
        let cap = usize::try_from(needed).unwrap_or(0) / item + 16;
        let mut buf = vec![
            libc::proc_fdinfo {
                proc_fd: 0,
                proc_fdtype: 0,
            };
            cap
        ];
        let bytes = c_int::try_from(cap * item).unwrap_or(c_int::MAX);
        // SAFETY: `buf` holds `cap` `proc_fdinfo`s, `bytes` bytes, for the call.
        let filled = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDLISTFDS,
                0,
                buf.as_mut_ptr().cast::<c_void>(),
                bytes,
            )
        };
        let Ok(filled) = usize::try_from(filled) else {
            return Vec::new();
        };
        buf.truncate(filled / item);
        buf.iter()
            .filter(|f| f.proc_fdtype == libc::PROX_FDTYPE_SOCKET as u32)
            .map(|f| f.proc_fd)
            .collect()
    }

    fn tcp_ends(pid: c_int, fd: c_int) -> Option<TcpEnds> {
        let mut buf = [0u8; BUF_LEN];
        let len = c_int::try_from(BUF_LEN).ok()?;
        // SAFETY: `buf` is valid for `len` bytes for the duration of the call.
        let filled = unsafe {
            libc::proc_pidfdinfo(
                pid,
                fd,
                PROC_PIDFDSOCKETINFO,
                buf.as_mut_ptr().cast::<c_void>(),
                len,
            )
        };
        let filled = usize::try_from(filled).ok()?.min(BUF_LEN);
        socket_fdinfo::tcp_ends(&buf[..filled])
    }

    fn owns(pid: c_int, peer: SocketAddr, proxy: SocketAddr) -> bool {
        socket_fds(pid).into_iter().any(|fd| {
            tcp_ends(pid, fd)
                .is_some_and(|ends| socket_fdinfo::is_client_socket(&ends, peer, proxy))
        })
    }

    fn c_string(buf: &[u8], len: c_int) -> Option<String> {
        let len = usize::try_from(len).ok()?.min(buf.len());
        let bytes = &buf[..len];
        let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
        let s = String::from_utf8_lossy(&bytes[..end]).trim().to_string();
        (!s.is_empty()).then_some(s)
    }

    fn process(pid: c_int) -> Option<ProcessInfo> {
        let mut path_buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        let cap = u32::try_from(path_buf.len()).unwrap_or(u32::MAX);
        // SAFETY: `path_buf` is valid for `cap` bytes for the duration of the call.
        let n = unsafe { libc::proc_pidpath(pid, path_buf.as_mut_ptr().cast(), cap) };
        let path = c_string(&path_buf, n);
        let name = match path.as_deref() {
            Some(p) => Some(file_name(p)),
            None => {
                let mut name_buf = [0u8; 256];
                let cap = u32::try_from(name_buf.len()).unwrap_or(u32::MAX);
                // SAFETY: `name_buf` is valid for `cap` bytes for the duration of the call.
                let n = unsafe { libc::proc_name(pid, name_buf.as_mut_ptr().cast(), cap) };
                c_string(&name_buf, n)
            }
        }?;
        Some(ProcessInfo {
            pid: u32::try_from(pid).ok()?,
            name,
            path,
        })
    }

    /// Whether the struct offsets hold on this macOS: one of our own loopback connections
    /// must be found where they say.
    fn layout_ok() -> bool {
        static OK: OnceLock<bool> = OnceLock::new();
        *OK.get_or_init(|| {
            let check = || -> std::io::Result<bool> {
                let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
                let server = listener.local_addr()?;
                let client = std::net::TcpStream::connect(server)?;
                let peer = client.local_addr()?;
                let pid = c_int::try_from(std::process::id()).unwrap_or(-1);
                Ok(owns(pid, peer, server))
            };
            let ok = check().unwrap_or(false);
            if !ok {
                tracing::debug!(
                    "libproc socket layout not recognised; using lsof for process attribution"
                );
            }
            ok
        })
    }

    fn remember(pid: c_int) {
        let mut hot = HOT.lock();
        hot.retain(|p| *p != pid);
        hot.insert(0, pid);
        hot.truncate(HOT_PIDS);
    }

    pub(super) fn find(peer: SocketAddr, proxy: SocketAddr) -> Option<ProcessInfo> {
        if !layout_ok() {
            return lsof::find(peer);
        }
        let hot = HOT.lock().clone();
        let found = hot
            .iter()
            .copied()
            .find(|pid| owns(*pid, peer, proxy))
            .or_else(|| {
                pids()
                    .into_iter()
                    .filter(|pid| !hot.contains(pid))
                    .find(|pid| owns(*pid, peer, proxy))
            })?;
        remember(found);
        process(found)
    }

    /// The previous implementation, kept as the fallback.
    mod lsof {
        use super::super::*;
        use std::process::Command;

        pub(in super::super) fn find(peer: SocketAddr) -> Option<ProcessInfo> {
            let out = Command::new("lsof")
                .args([
                    "-nP",
                    &format!("-iTCP:{}", peer.port()),
                    "-sTCP:ESTABLISHED",
                    "-Fpcn",
                ])
                .output()
                .ok()?;
            let text = String::from_utf8_lossy(&out.stdout);
            let prefix = format!(":{}->", peer.port());
            let (mut pid, mut name) = (None::<u32>, None::<String>);
            for line in text.lines() {
                match line.split_at_checked(1) {
                    Some(("p", v)) => {
                        pid = v.parse().ok();
                        name = None;
                    }
                    Some(("c", v)) => name = Some(v.to_string()),
                    Some(("n", v)) if v.contains(&prefix) => {
                        let pid = pid?;
                        let path = Command::new("ps")
                            .args(["-o", "comm=", "-p", &pid.to_string()])
                            .output()
                            .ok()
                            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                            .filter(|s| !s.is_empty());
                        let name = name.clone().or_else(|| path.as_deref().map(file_name))?;
                        return Some(ProcessInfo { pid, name, path });
                    }
                    _ => {}
                }
            }
            None
        }
    }
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INSUFFICIENT_BUFFER, NO_ERROR};
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, TCP_TABLE_OWNER_PID_ALL,
    };
    use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };

    /// Fetch the raw TCP table for an address family.
    fn table(family: u16) -> Option<Vec<u8>> {
        let mut size: u32 = 0;
        // SAFETY: a null buffer with size 0 asks for the required size.
        let r = unsafe {
            GetExtendedTcpTable(
                std::ptr::null_mut(),
                &mut size,
                0,
                family as u32,
                TCP_TABLE_OWNER_PID_ALL,
                0,
            )
        };
        if r != ERROR_INSUFFICIENT_BUFFER && r != NO_ERROR {
            return None;
        }
        for _ in 0..4 {
            // u32 storage keeps the buffer aligned for the table structs.
            let mut buf = vec![0u32; (size as usize).div_ceil(4) + 16];
            let mut len = (buf.len() * 4) as u32;
            // SAFETY: `buf` is valid for `len` bytes for the duration of the call.
            let r = unsafe {
                GetExtendedTcpTable(
                    buf.as_mut_ptr().cast(),
                    &mut len,
                    0,
                    family as u32,
                    TCP_TABLE_OWNER_PID_ALL,
                    0,
                )
            };
            if r == NO_ERROR {
                return Some(
                    buf.iter()
                        .flat_map(|w| w.to_ne_bytes())
                        .take(len as usize)
                        .collect(),
                );
            }
            if r != ERROR_INSUFFICIENT_BUFFER {
                return None;
            }
            size = len;
        }
        None
    }

    fn u32_at(b: &[u8], off: usize) -> Option<u32> {
        Some(u32::from_ne_bytes(b.get(off..off + 4)?.try_into().ok()?))
    }

    fn port(dw: u32) -> u16 {
        u16::from_be((dw & 0xffff) as u16)
    }

    fn find_pid(peer: SocketAddr, proxy: SocketAddr) -> Option<u32> {
        let v4 = |a: SocketAddr| match a.ip() {
            IpAddr::V4(v) => Some(v),
            IpAddr::V6(v) => v.to_ipv4_mapped(),
        };
        if let (Some(_), Some(t)) = (v4(peer), table(AF_INET)) {
            let n = u32_at(&t, 0)? as usize;
            for i in 0..n {
                let off = 4 + i * 24;
                let (laddr, lport, rport, pid) = (
                    u32_at(&t, off + 4)?,
                    u32_at(&t, off + 8)?,
                    u32_at(&t, off + 16)?,
                    u32_at(&t, off + 20)?,
                );
                let local = Ipv4Addr::from(laddr.to_ne_bytes());
                if port(lport) == peer.port() && port(rport) == proxy.port() && local.is_loopback()
                {
                    return Some(pid);
                }
            }
        }
        if let Some(t) = table(AF_INET6) {
            let n = u32_at(&t, 0)? as usize;
            for i in 0..n {
                let off = 4 + i * 56;
                let laddr: [u8; 16] = t.get(off..off + 16)?.try_into().ok()?;
                let (lport, rport, pid) = (
                    u32_at(&t, off + 20)?,
                    u32_at(&t, off + 44)?,
                    u32_at(&t, off + 52)?,
                );
                let local = Ipv6Addr::from(laddr);
                if port(lport) == peer.port()
                    && port(rport) == proxy.port()
                    && (local.is_loopback() || local.to_ipv4_mapped().is_some())
                {
                    return Some(pid);
                }
            }
        }
        None
    }

    fn image_path(pid: u32) -> Option<String> {
        // SAFETY: plain Win32 calls; the handle is closed before returning.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return None;
            }
            let mut buf = vec![0u16; 1024];
            let mut len = buf.len() as u32;
            let ok =
                QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len);
            CloseHandle(handle);
            (ok != 0).then(|| String::from_utf16_lossy(&buf[..len as usize]))
        }
    }

    pub(super) fn find(peer: SocketAddr, proxy: SocketAddr) -> Option<ProcessInfo> {
        let pid = find_pid(peer, proxy)?;
        let path = image_path(pid);
        let name = path
            .as_deref()
            .map(file_name)
            .unwrap_or_else(|| format!("pid {pid}"));
        Some(ProcessInfo { pid, name, path })
    }
}
