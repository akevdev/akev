//! The original destination of a connection redirected by netfilter (nftables / iptables
//! `REDIRECT` or `DNAT`). Conntrack remembers the pre-NAT tuple; on Linux
//! `getsockopt(SOL_IP, SO_ORIGINAL_DST)` (IPv4) and
//! `getsockopt(SOL_IPV6, IP6T_SO_ORIGINAL_DST)` (IPv6) return it as a `sockaddr_in` /
//! `sockaddr_in6`, parsed by [`parse_sockaddr`].
//!
//! (`TPROXY` keeps the original destination as the socket's local address instead, which needs
//! `IP_TRANSPARENT` and therefore `CAP_NET_ADMIN`; Akev uses `REDIRECT`, which doesn't.)

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};

/// `AF_INET` on Linux (`sa_family` of a `sockaddr_in`).
pub const LINUX_AF_INET: u16 = 2;
/// `AF_INET6` on Linux (`sa_family` of a `sockaddr_in6`).
pub const LINUX_AF_INET6: u16 = 10;

/// Parse a Linux `sockaddr_in` / `sockaddr_in6` exactly as the kernel fills it in:
/// `sa_family` in native byte order, port (and IPv4 address) in network byte order.
///
/// An IPv4-mapped IPv6 address comes back as IPv4. Other families and truncated buffers give
/// `None`.
pub fn parse_sockaddr(buf: &[u8]) -> Option<SocketAddr> {
    let family = u16::from_ne_bytes([*buf.first()?, *buf.get(1)?]);
    match family {
        LINUX_AF_INET => {
            // sin_family(2) sin_port(2) sin_addr(4) [sin_zero(8)]
            let b = buf.get(..8)?;
            let port = u16::from_be_bytes([b[2], b[3]]);
            let ip = Ipv4Addr::new(b[4], b[5], b[6], b[7]);
            Some(SocketAddr::V4(SocketAddrV4::new(ip, port)))
        }
        LINUX_AF_INET6 => {
            // sin6_family(2) sin6_port(2) sin6_flowinfo(4) sin6_addr(16) sin6_scope_id(4)
            let b = buf.get(..24)?;
            let port = u16::from_be_bytes([b[2], b[3]]);
            let flowinfo = u32::from_ne_bytes([b[4], b[5], b[6], b[7]]);
            let mut octets = [0u8; 16];
            octets.copy_from_slice(&b[8..24]);
            let scope_id = buf
                .get(24..28)
                .map(|s| u32::from_ne_bytes([s[0], s[1], s[2], s[3]]))
                .unwrap_or(0);
            let ip = Ipv6Addr::from(octets);
            Some(match ip.to_ipv4_mapped() {
                Some(v4) => SocketAddr::V4(SocketAddrV4::new(v4, port)),
                None => SocketAddr::V6(SocketAddrV6::new(ip, port, flowinfo, scope_id)),
            })
        }
        _ => None,
    }
}

/// IPv4-mapped IPv6 addresses as IPv4 (dual-stack sockets report IPv4 peers that way).
pub(crate) fn canonical(addr: SocketAddr) -> SocketAddr {
    match addr.ip() {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => SocketAddr::new(IpAddr::V4(v4), addr.port()),
            None => addr,
        },
        IpAddr::V4(_) => addr,
    }
}

#[cfg(any(target_os = "linux", test))]
/// Whether the socket's original destination must be read with the IPv6 option: only for real
/// IPv6 connections (an IPv4 client on a dual-stack socket was tracked as IPv4).
pub(crate) fn needs_ipv6_query(local: SocketAddr) -> bool {
    matches!(canonical(local).ip(), IpAddr::V6(_))
}

/// Interpret a lookup result: the destination the client was really connecting to, or `None`
/// when the connection wasn't redirected (no NAT entry, or the "original" destination is the
/// listener itself because the client connected to the port directly).
pub(crate) fn redirected_destination(
    original: Option<SocketAddr>,
    local: SocketAddr,
) -> Option<SocketAddr> {
    let dst = canonical(original?);
    if dst.port() == 0 || dst.ip().is_unspecified() || dst == canonical(local) {
        return None;
    }
    Some(dst)
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::io;
    use std::os::fd::RawFd;

    /// `getsockopt(SO_ORIGINAL_DST / IP6T_SO_ORIGINAL_DST)` on `fd`.
    pub(crate) fn query(fd: RawFd, ipv6: bool) -> io::Result<SocketAddr> {
        // SAFETY: `sockaddr_storage` is plain old data; all-zero is a valid value.
        let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::sockaddr_storage>();
        let mut len = size as libc::socklen_t;
        let (level, name) = if ipv6 {
            (libc::SOL_IPV6, libc::IP6T_SO_ORIGINAL_DST)
        } else {
            (libc::SOL_IP, libc::SO_ORIGINAL_DST)
        };
        // SAFETY: `storage` is a writable buffer of `len` bytes that outlives the call; the
        // kernel writes at most `len` bytes and stores the written length in `len`.
        let rc = unsafe {
            libc::getsockopt(
                fd,
                level,
                name,
                (&mut storage as *mut libc::sockaddr_storage).cast(),
                &mut len,
            )
        };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        let len = (len as usize).min(size);
        // SAFETY: `storage` is fully initialised (zeroed, then partly overwritten by the
        // kernel) and `len <= size_of::<sockaddr_storage>()`.
        let bytes = unsafe {
            std::slice::from_raw_parts(
                (&storage as *const libc::sockaddr_storage).cast::<u8>(),
                len,
            )
        };
        parse_sockaddr(bytes).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected address family from SO_ORIGINAL_DST",
            )
        })
    }

    /// The pre-NAT destination recorded by conntrack for an accepted socket. `Ok(None)` when
    /// there is none: no conntrack entry (`ENOENT`) or conntrack isn't loaded (`ENOPROTOOPT`).
    pub(crate) fn original_destination(
        fd: RawFd,
        local: SocketAddr,
    ) -> io::Result<Option<SocketAddr>> {
        match query(fd, needs_ipv6_query(local)) {
            Ok(dst) => Ok(Some(dst)),
            Err(e)
                if matches!(
                    e.raw_os_error(),
                    Some(libc::ENOENT | libc::ENOPROTOOPT | libc::EOPNOTSUPP)
                ) =>
            {
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) use linux::original_destination;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A Linux `sockaddr_in` as the kernel would write it.
    pub(crate) fn sockaddr_in(ip: Ipv4Addr, port: u16) -> Vec<u8> {
        let mut b = Vec::with_capacity(16);
        b.extend_from_slice(&LINUX_AF_INET.to_ne_bytes());
        b.extend_from_slice(&port.to_be_bytes());
        b.extend_from_slice(&ip.octets());
        b.extend_from_slice(&[0; 8]);
        b
    }

    /// A Linux `sockaddr_in6` as the kernel would write it.
    pub(crate) fn sockaddr_in6(ip: Ipv6Addr, port: u16, scope_id: u32) -> Vec<u8> {
        let mut b = Vec::with_capacity(28);
        b.extend_from_slice(&LINUX_AF_INET6.to_ne_bytes());
        b.extend_from_slice(&port.to_be_bytes());
        b.extend_from_slice(&0u32.to_ne_bytes());
        b.extend_from_slice(&ip.octets());
        b.extend_from_slice(&scope_id.to_ne_bytes());
        b
    }

    #[test]
    fn parses_sockaddr_in() {
        let b = sockaddr_in(Ipv4Addr::new(93, 184, 216, 34), 443);
        assert_eq!(
            parse_sockaddr(&b),
            Some("93.184.216.34:443".parse().unwrap())
        );
        // sin_zero is optional; the address part is not.
        assert_eq!(
            parse_sockaddr(&b[..8]),
            Some("93.184.216.34:443".parse().unwrap())
        );
        assert_eq!(parse_sockaddr(&b[..7]), None);
        // High port numbers are big-endian on the wire.
        let b = sockaddr_in(Ipv4Addr::new(10, 0, 0, 1), 0xBEEF);
        assert_eq!(parse_sockaddr(&b).unwrap().port(), 0xBEEF);
    }

    #[test]
    fn parses_sockaddr_in6() {
        let ip: Ipv6Addr = "2606:2800:220:1:248:1893:25c8:1946".parse().unwrap();
        let b = sockaddr_in6(ip, 8443, 0);
        assert_eq!(
            parse_sockaddr(&b),
            Some("[2606:2800:220:1:248:1893:25c8:1946]:8443".parse().unwrap())
        );
        let link_local: Ipv6Addr = "fe80::1".parse().unwrap();
        match parse_sockaddr(&sockaddr_in6(link_local, 80, 3)) {
            Some(SocketAddr::V6(v6)) => {
                assert_eq!(*v6.ip(), link_local);
                assert_eq!(v6.scope_id(), 3);
            }
            other => panic!("{other:?}"),
        }
        // Without the trailing scope id it still parses (scope 0).
        assert_eq!(parse_sockaddr(&b[..24]).unwrap().port(), 8443);
        assert_eq!(parse_sockaddr(&b[..23]), None);
        // IPv4-mapped destinations are reported as IPv4.
        let mapped = Ipv4Addr::new(1, 2, 3, 4).to_ipv6_mapped();
        assert_eq!(
            parse_sockaddr(&sockaddr_in6(mapped, 80, 0)),
            Some("1.2.3.4:80".parse().unwrap())
        );
    }

    #[test]
    fn rejects_other_families_and_garbage() {
        assert_eq!(parse_sockaddr(&[]), None);
        assert_eq!(parse_sockaddr(&[2]), None);
        let mut b = sockaddr_in(Ipv4Addr::LOCALHOST, 80);
        b[..2].copy_from_slice(&1u16.to_ne_bytes()); // AF_UNIX
        assert_eq!(parse_sockaddr(&b), None);
        b[..2].copy_from_slice(&LINUX_AF_INET6.to_ne_bytes()); // too short for sockaddr_in6
        assert_eq!(parse_sockaddr(&b), None);
    }

    /// The parser agrees with the real `libc` structure layout.
    #[cfg(target_os = "linux")]
    #[test]
    fn matches_libc_layout() {
        fn bytes_of<T>(v: &T) -> Vec<u8> {
            // SAFETY: reading the bytes of a fully initialised plain-old-data value.
            unsafe {
                std::slice::from_raw_parts((v as *const T).cast::<u8>(), std::mem::size_of::<T>())
                    .to_vec()
            }
        }
        // SAFETY: plain old data, all-zero is valid.
        let mut v4: libc::sockaddr_in = unsafe { std::mem::zeroed() };
        v4.sin_family = libc::AF_INET as libc::sa_family_t;
        v4.sin_port = 8443u16.to_be();
        v4.sin_addr.s_addr = u32::from_ne_bytes([192, 0, 2, 7]);
        let b = bytes_of(&v4);
        assert_eq!(b, sockaddr_in(Ipv4Addr::new(192, 0, 2, 7), 8443));
        assert_eq!(parse_sockaddr(&b), Some("192.0.2.7:8443".parse().unwrap()));

        // SAFETY: as above.
        let mut v6: libc::sockaddr_in6 = unsafe { std::mem::zeroed() };
        v6.sin6_family = libc::AF_INET6 as libc::sa_family_t;
        v6.sin6_port = 443u16.to_be();
        v6.sin6_addr.s6_addr = "2001:db8::5".parse::<Ipv6Addr>().unwrap().octets();
        v6.sin6_scope_id = 0;
        let b = bytes_of(&v6);
        assert_eq!(b.len(), 28);
        assert_eq!(
            parse_sockaddr(&b),
            Some("[2001:db8::5]:443".parse().unwrap())
        );
        assert_eq!(LINUX_AF_INET, libc::AF_INET as u16);
        assert_eq!(LINUX_AF_INET6, libc::AF_INET6 as u16);
    }

    #[test]
    fn redirect_interpretation() {
        let local: SocketAddr = "127.0.0.1:8001".parse().unwrap();
        let dst: SocketAddr = "93.184.216.34:443".parse().unwrap();
        assert_eq!(redirected_destination(Some(dst), local), Some(dst));
        assert_eq!(redirected_destination(None, local), None);
        // Conntrack entry without NAT: the "original" destination is the listener itself.
        assert_eq!(redirected_destination(Some(local), local), None);
        let mapped_local: SocketAddr = "[::ffff:127.0.0.1]:8001".parse().unwrap();
        assert_eq!(redirected_destination(Some(local), mapped_local), None);
        assert_eq!(
            redirected_destination(Some("0.0.0.0:443".parse().unwrap()), local),
            None
        );
        assert_eq!(
            redirected_destination(Some("1.2.3.4:0".parse().unwrap()), local),
            None
        );
        let mapped_dst: SocketAddr = "[::ffff:1.2.3.4]:80".parse().unwrap();
        assert_eq!(
            redirected_destination(Some(mapped_dst), local),
            Some("1.2.3.4:80".parse().unwrap())
        );
        assert!(!needs_ipv6_query(local));
        assert!(!needs_ipv6_query(mapped_local));
        assert!(needs_ipv6_query("[::1]:8001".parse().unwrap()));
    }

    /// A real, unredirected connection: the kernel reports no NAT (or conntrack isn't loaded),
    /// which must read as "not redirected" — never as an error or a bogus destination.
    #[cfg(target_os = "linux")]
    #[test]
    fn unredirected_kernel_connection_reads_as_direct() {
        use std::os::fd::AsRawFd;
        for bind in ["127.0.0.1:0", "[::1]:0"] {
            let Ok(listener) = std::net::TcpListener::bind(bind) else {
                continue; // no IPv6 on this machine
            };
            let local = listener.local_addr().unwrap();
            let _client = std::net::TcpStream::connect(local).unwrap();
            let (server, _) = listener.accept().unwrap();
            let got = original_destination(server.as_raw_fd(), local).unwrap();
            assert_eq!(redirected_destination(got, local), None, "{bind}: {got:?}");
        }
    }
}
