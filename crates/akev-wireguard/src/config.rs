//! The client configuration handed to the device (WireGuard `.conf` format, also the QR code
//! payload understood by the official WireGuard apps).

use crate::keys::WireguardKeys;
use crate::{CLIENT_ADDRESS, DNS_ADDRESS};
use std::net::IpAddr;

/// File name suggested for the configuration download.
pub const CONFIG_FILE_NAME: &str = "akev.conf";

/// Render the client configuration: the device's key and tunnel address, Akev's DNS relay,
/// and a single peer (Akev) at `endpoint_host:port` that routes all IPv4 and IPv6 traffic.
pub fn client_config(keys: &WireguardKeys, endpoint_host: &str, port: u16) -> String {
    format!(
        "[Interface]\n\
         PrivateKey = {private}\n\
         Address = {address}/32\n\
         DNS = {dns}\n\
         \n\
         [Peer]\n\
         PublicKey = {public}\n\
         AllowedIPs = 0.0.0.0/0, ::/0\n\
         Endpoint = {endpoint}\n",
        private = keys.client.to_base64(),
        address = CLIENT_ADDRESS,
        dns = DNS_ADDRESS,
        public = keys.server.public_key().to_base64(),
        endpoint = endpoint(endpoint_host, port),
    )
}

/// `host:port`, with IPv6 literals bracketed.
pub fn endpoint(host: &str, port: u16) -> String {
    let host = host.trim().trim_start_matches('[').trim_end_matches(']');
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V6(v6)) => format!("[{v6}]:{port}"),
        _ => format!("{host}:{port}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::PrivateKey;

    fn keys() -> WireguardKeys {
        WireguardKeys {
            server: PrivateKey::from_bytes([1; 32]),
            client: PrivateKey::from_bytes([2; 32]),
            created_at: 0,
        }
    }

    #[test]
    fn config_text() {
        let k = keys();
        let text = client_config(&k, "192.168.1.20", 51820);
        let expected = format!(
            "[Interface]\nPrivateKey = {}\nAddress = 10.0.0.2/32\nDNS = 10.0.0.53\n\n[Peer]\nPublicKey = {}\nAllowedIPs = 0.0.0.0/0, ::/0\nEndpoint = 192.168.1.20:51820\n",
            k.client.to_base64(),
            k.server.public_key().to_base64()
        );
        assert_eq!(text, expected);
        // The server's private key never leaves the machine.
        assert!(!text.contains(&k.server.to_base64()));
        assert!(!text.contains(&k.client.public_key().to_base64()));
    }

    #[test]
    fn endpoints() {
        assert_eq!(endpoint("10.1.2.3", 1), "10.1.2.3:1");
        assert_eq!(endpoint("fd00::1", 51820), "[fd00::1]:51820");
        assert_eq!(endpoint("[fd00::1]", 51820), "[fd00::1]:51820");
        assert_eq!(endpoint(" akev.lan ", 7), "akev.lan:7");
    }
}
