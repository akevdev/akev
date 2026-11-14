//! Classification of the IP packets coming out of the tunnel, and construction of the UDP
//! packets sent back into it.

use smoltcp::phy::ChecksumCapabilities;
use smoltcp::wire::{
    IpAddress, IpProtocol, Ipv4Packet, Ipv4Repr, Ipv6Packet, Ipv6Repr, TcpPacket, UdpPacket,
    UdpRepr,
};
use std::net::{IpAddr, SocketAddr};

/// What an IP packet from the device carries.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Classified<'a> {
    /// A TCP segment (handed to the TCP stack as a whole IP packet).
    Tcp {
        src: SocketAddr,
        dst: SocketAddr,
        /// A connection request: SYN without ACK.
        syn: bool,
    },
    /// A UDP datagram with a valid checksum.
    Udp {
        src: SocketAddr,
        dst: SocketAddr,
        payload: &'a [u8],
    },
    /// Anything else (ICMP, fragments, extension headers, corrupt packets): dropped.
    Other(&'static str),
}

/// Classify a decrypted IP packet.
pub(crate) fn classify(packet: &[u8]) -> Classified<'_> {
    match packet.first().map(|b| b >> 4) {
        Some(4) => classify_v4(packet),
        Some(6) => classify_v6(packet),
        _ => Classified::Other("not an IP packet"),
    }
}

fn classify_v4(packet: &[u8]) -> Classified<'_> {
    let Ok(ip) = Ipv4Packet::new_checked(packet) else {
        return Classified::Other("malformed IPv4 packet");
    };
    if !ip.verify_checksum() {
        return Classified::Other("bad IPv4 header checksum");
    }
    if ip.more_frags() || ip.frag_offset() != 0 {
        return Classified::Other("IPv4 fragment");
    }
    let (src, dst) = (IpAddr::V4(ip.src_addr()), IpAddr::V4(ip.dst_addr()));
    transport(ip.next_header(), src, dst, ip.payload())
}

fn classify_v6(packet: &[u8]) -> Classified<'_> {
    let Ok(ip) = Ipv6Packet::new_checked(packet) else {
        return Classified::Other("malformed IPv6 packet");
    };
    let (src, dst) = (IpAddr::V6(ip.src_addr()), IpAddr::V6(ip.dst_addr()));
    transport(ip.next_header(), src, dst, ip.payload())
}

fn transport(proto: IpProtocol, src: IpAddr, dst: IpAddr, payload: &[u8]) -> Classified<'_> {
    match proto {
        IpProtocol::Tcp => {
            let Ok(tcp) = TcpPacket::new_checked(payload) else {
                return Classified::Other("malformed TCP segment");
            };
            Classified::Tcp {
                src: SocketAddr::new(src, tcp.src_port()),
                dst: SocketAddr::new(dst, tcp.dst_port()),
                syn: tcp.syn() && !tcp.ack(),
            }
        }
        IpProtocol::Udp => {
            let Ok(udp) = UdpPacket::new_checked(payload) else {
                return Classified::Other("malformed UDP datagram");
            };
            // An all-zero checksum means "none" for IPv4 (never valid for IPv6).
            if !(udp.checksum() == 0 && src.is_ipv4())
                && !udp.verify_checksum(&IpAddress::from(src), &IpAddress::from(dst))
            {
                return Classified::Other("bad UDP checksum");
            }
            Classified::Udp {
                src: SocketAddr::new(src, udp.src_port()),
                dst: SocketAddr::new(dst, udp.dst_port()),
                payload: udp.payload(),
            }
        }
        IpProtocol::Icmp | IpProtocol::Icmpv6 => Classified::Other("ICMP"),
        _ => Classified::Other("unsupported IP protocol"),
    }
}

/// Build an IP/UDP packet from `src` to `dst`. `None` when the address families differ.
pub(crate) fn udp_packet(src: SocketAddr, dst: SocketAddr, payload: &[u8]) -> Option<Vec<u8>> {
    let udp_repr = UdpRepr {
        src_port: src.port(),
        dst_port: dst.port(),
    };
    let udp_len = 8 + payload.len();
    let caps = ChecksumCapabilities::default();
    let (src_ip, dst_ip) = (IpAddress::from(src.ip()), IpAddress::from(dst.ip()));
    match (src.ip(), dst.ip()) {
        (IpAddr::V4(s), IpAddr::V4(d)) => {
            let repr = Ipv4Repr {
                src_addr: s,
                dst_addr: d,
                next_header: IpProtocol::Udp,
                payload_len: udp_len,
                hop_limit: 64,
            };
            let total = repr.buffer_len() + udp_len;
            if total > u16::MAX as usize {
                return None;
            }
            let mut buf = vec![0u8; total];
            let mut ip = Ipv4Packet::new_unchecked(&mut buf[..]);
            repr.emit(&mut ip, &caps);
            let mut udp = UdpPacket::new_unchecked(ip.payload_mut());
            udp_repr.emit(
                &mut udp,
                &src_ip,
                &dst_ip,
                payload.len(),
                |b| b.copy_from_slice(payload),
                &caps,
            );
            Some(buf)
        }
        (IpAddr::V6(s), IpAddr::V6(d)) => {
            let repr = Ipv6Repr {
                src_addr: s,
                dst_addr: d,
                next_header: IpProtocol::Udp,
                payload_len: udp_len,
                hop_limit: 64,
            };
            if udp_len > u16::MAX as usize {
                return None;
            }
            let mut buf = vec![0u8; repr.buffer_len() + udp_len];
            let mut ip = Ipv6Packet::new_unchecked(&mut buf[..]);
            repr.emit(&mut ip);
            let mut udp = UdpPacket::new_unchecked(ip.payload_mut());
            udp_repr.emit(
                &mut udp,
                &src_ip,
                &dst_ip,
                payload.len(),
                |b| b.copy_from_slice(payload),
                &caps,
            );
            Some(buf)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn udp_round_trip_v4_and_v6() {
        for (src, dst) in [
            ("10.0.0.53:53", "10.0.0.2:40000"),
            ("[fd00::1]:53", "[fd00::2]:40000"),
        ] {
            let (src, dst): (SocketAddr, SocketAddr) = (src.parse().unwrap(), dst.parse().unwrap());
            let pkt = udp_packet(src, dst, b"payload").unwrap();
            assert_eq!(
                classify(&pkt),
                Classified::Udp {
                    src,
                    dst,
                    payload: b"payload"
                }
            );
        }
        assert!(
            udp_packet(
                "1.2.3.4:1".parse().unwrap(),
                "[::1]:2".parse().unwrap(),
                b""
            )
            .is_none()
        );
    }

    #[test]
    fn corrupt_udp_is_rejected() {
        let mut pkt = udp_packet(
            "10.0.0.2:1000".parse().unwrap(),
            "10.0.0.53:53".parse().unwrap(),
            b"query",
        )
        .unwrap();
        let last = pkt.len() - 1;
        pkt[last] ^= 0xff;
        assert_eq!(classify(&pkt), Classified::Other("bad UDP checksum"));
        assert_eq!(classify(&[]), Classified::Other("not an IP packet"));
        assert_eq!(
            classify(&[0x45, 0]),
            Classified::Other("malformed IPv4 packet")
        );
    }

    #[test]
    fn tcp_syn_detection() {
        use smoltcp::wire::{TcpControl, TcpRepr, TcpSeqNumber};
        let src: SocketAddr = "10.0.0.2:5555".parse().unwrap();
        let dst: SocketAddr = "93.184.216.34:443".parse().unwrap();
        let tcp = TcpRepr {
            src_port: src.port(),
            dst_port: dst.port(),
            control: TcpControl::Syn,
            seq_number: TcpSeqNumber(1),
            ack_number: None,
            window_len: 1024,
            window_scale: None,
            max_seg_size: Some(1380),
            sack_permitted: false,
            sack_ranges: [None, None, None],
            timestamp: None,
            payload: &[],
        };
        let caps = ChecksumCapabilities::default();
        let repr = Ipv4Repr {
            src_addr: match src.ip() {
                IpAddr::V4(v) => v,
                _ => unreachable!(),
            },
            dst_addr: match dst.ip() {
                IpAddr::V4(v) => v,
                _ => unreachable!(),
            },
            next_header: IpProtocol::Tcp,
            payload_len: tcp.buffer_len(),
            hop_limit: 64,
        };
        let mut buf = vec![0u8; repr.buffer_len() + tcp.buffer_len()];
        let mut ip = Ipv4Packet::new_unchecked(&mut buf[..]);
        repr.emit(&mut ip, &caps);
        let mut seg = TcpPacket::new_unchecked(ip.payload_mut());
        tcp.emit(
            &mut seg,
            &IpAddress::from(src.ip()),
            &IpAddress::from(dst.ip()),
            &caps,
        );
        assert_eq!(
            classify(&buf),
            Classified::Tcp {
                src,
                dst,
                syn: true
            }
        );
    }
}
