//! Where a request comes from, decided once per request: this computer, the home network, or outside (the internet,
//! and anything that came through a Cloudflare tunnel — cloudflared connects from loopback, but the one asking is
//! outside). A zone narrows what is reachable; it never stands in for signing in.

use std::net::SocketAddr;

use axum::http::HeaderMap;
use serde::Serialize;

use crate::operations::networks::{in_list, in_networks, via_tunnel, LOOPBACK};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Zone {
    Local,
    Home,
    Remote,
}

pub fn zone_of(headers: &HeaderMap, peer: Option<SocketAddr>, home_networks: &[String]) -> Zone {
    if via_tunnel(headers) {
        return Zone::Remote;
    }
    let address = peer.map(|peer| peer.ip().to_string()).unwrap_or_default();
    if in_list(&address, LOOPBACK) {
        return Zone::Local;
    }
    if in_networks(&address, home_networks) {
        Zone::Home
    } else {
        Zone::Remote
    }
}

/// Whether a zone meets a requirement: `home` takes this computer too.
pub fn zone_allows(required: Zone, zone: Zone) -> bool {
    match required {
        Zone::Remote => true,
        Zone::Home => matches!(zone, Zone::Home | Zone::Local),
        Zone::Local => zone == Zone::Local,
    }
}

/// The address to count sign-in failures by: the visitor's own behind a tunnel (cloudflared's header, trusted only
/// when the connection itself is loopback), the socket's otherwise.
pub fn client_address(headers: &HeaderMap, peer: Option<SocketAddr>) -> String {
    let socket = peer.map(|peer| peer.ip().to_string()).unwrap_or_else(|| "unknown".into());
    if let Some(forwarded) = headers.get("cf-connecting-ip").and_then(|value| value.to_str().ok()) {
        if !forwarded.is_empty() && in_list(&socket, LOOPBACK) {
            return format!("tunnel:{forwarded}");
        }
    }
    socket
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(ip: &str) -> Option<SocketAddr> {
        Some(SocketAddr::new(ip.parse().unwrap(), 5000))
    }

    #[test]
    fn decides_the_zone() {
        let home = vec!["private".to_string()];
        let none = HeaderMap::new();
        assert_eq!(zone_of(&none, peer("127.0.0.1"), &home), Zone::Local);
        assert_eq!(zone_of(&none, peer("192.168.1.20"), &home), Zone::Home);
        assert_eq!(zone_of(&none, peer("8.8.8.8"), &home), Zone::Remote);
        let mut tunnel = HeaderMap::new();
        tunnel.insert("cf-ray", "x".parse().unwrap());
        assert_eq!(zone_of(&tunnel, peer("127.0.0.1"), &home), Zone::Remote);
        assert!(zone_allows(Zone::Home, Zone::Local));
        assert!(!zone_allows(Zone::Local, Zone::Home));
        let mut visitor = HeaderMap::new();
        visitor.insert("cf-connecting-ip", "1.2.3.4".parse().unwrap());
        assert_eq!(client_address(&visitor, peer("127.0.0.1")), "tunnel:1.2.3.4");
        assert_eq!(client_address(&visitor, peer("10.0.0.2")), "10.0.0.2", "only loopback is trusted to forward");
    }
}
