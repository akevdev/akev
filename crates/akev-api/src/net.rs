//! Local network helpers: LAN IPv4 addresses other devices (phones, VMs) can reach the proxy on.

use parking_lot::Mutex;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

const CACHE_TTL: Duration = Duration::from_secs(15);

static CACHE: Mutex<Option<(Instant, Vec<Ipv4Addr>)>> = Mutex::new(None);

/// Non-loopback, non-link-local IPv4 addresses of this machine's real network interfaces
/// (container bridges and virtual Ethernet pairs excluded), private ranges first. Cached
/// for a few seconds.
pub fn lan_ipv4_addresses() -> Vec<Ipv4Addr> {
    let mut cache = CACHE.lock();
    if let Some((at, addrs)) = cache.as_ref()
        && at.elapsed() < CACHE_TTL
    {
        return addrs.clone();
    }
    let addrs = enumerate();
    *cache = Some((Instant::now(), addrs.clone()));
    addrs
}

fn enumerate() -> Vec<Ipv4Addr> {
    let networks = sysinfo::Networks::new_with_refreshed_list();
    let mut out: Vec<Ipv4Addr> = networks
        .iter()
        .filter(|(name, _)| !is_virtual_interface(name))
        .flat_map(|(_, data)| data.ip_networks().iter().map(|n| n.addr))
        .filter_map(|addr| match addr {
            IpAddr::V4(v4) if is_usable(v4) => Some(v4),
            _ => None,
        })
        .collect();
    if out.is_empty()
        && let Some(primary) = primary_outbound_ipv4()
    {
        out.push(primary);
    }
    sort_addresses(&mut out);
    out
}

/// The address the OS would use for outbound traffic ("UDP connect" trick: no packet is sent).
fn primary_outbound_ipv4() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect(SocketAddr::from(([192, 0, 2, 1], 9))).ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(v4) if is_usable(v4) => Some(v4),
        _ => None,
    }
}

fn is_usable(ip: Ipv4Addr) -> bool {
    !(ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_multicast()
        || ip.is_broadcast())
}

/// Interfaces that other devices can't use to reach us (container bridges etc).
pub fn is_virtual_interface(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n == "lo"
        || n.starts_with("lo0")
        || n == "docker0"
        || n.starts_with("br-")
        || n.starts_with("veth")
        || n.starts_with("virbr")
        || n.starts_with("vboxnet")
        || n.starts_with("cni")
        || n.starts_with("flannel")
        || n.contains("loopback")
}

/// Private (RFC 1918) addresses first — those are what phones on the same Wi-Fi can reach.
fn sort_addresses(addrs: &mut Vec<Ipv4Addr>) {
    addrs.sort_by_key(|ip| (!ip.is_private(), *ip));
    addrs.dedup();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_interfaces_are_excluded() {
        for n in [
            "lo",
            "docker0",
            "br-1a2b3c",
            "veth12ab",
            "virbr0",
            "Loopback Pseudo-Interface 1",
        ] {
            assert!(is_virtual_interface(n), "{n}");
        }
        for n in ["eth0", "wlan0", "en0", "wlp3s0", "tailscale0", "Wi-Fi"] {
            assert!(!is_virtual_interface(n), "{n}");
        }
    }

    #[test]
    fn usable_addresses() {
        assert!(is_usable(Ipv4Addr::new(192, 168, 1, 5)));
        assert!(is_usable(Ipv4Addr::new(100, 64, 1, 5)));
        assert!(!is_usable(Ipv4Addr::LOCALHOST));
        assert!(!is_usable(Ipv4Addr::new(169, 254, 3, 4)));
        assert!(!is_usable(Ipv4Addr::UNSPECIFIED));
    }

    #[test]
    fn private_addresses_sort_first() {
        let mut v = vec![
            Ipv4Addr::new(100, 64, 0, 1),
            Ipv4Addr::new(192, 168, 1, 2),
            Ipv4Addr::new(10, 0, 0, 3),
            Ipv4Addr::new(192, 168, 1, 2),
        ];
        sort_addresses(&mut v);
        assert_eq!(
            v,
            vec![
                Ipv4Addr::new(10, 0, 0, 3),
                Ipv4Addr::new(192, 168, 1, 2),
                Ipv4Addr::new(100, 64, 0, 1)
            ]
        );
    }

    #[test]
    fn enumeration_does_not_panic() {
        let addrs = lan_ipv4_addresses();
        assert!(addrs.iter().all(|a| is_usable(*a)));
        // cached second call
        assert_eq!(lan_ipv4_addresses(), addrs);
    }
}
