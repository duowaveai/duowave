//! Where a request comes from, decided once per request: this computer, the home network, or outside (the internet,
//! and anything that came through a Cloudflare tunnel — cloudflared connects from loopback, but the one asking is
//! outside). A zone narrows what is reachable; it never stands in for signing in.
//!
//! A request a proxy handed on (it carries `X-Forwarded-For`, `Forwarded`, `X-Real-IP` or `Via`) is judged by the
//! client it came for, never by the proxy: a proxy on this very computer would make the whole internet look like this
//! computer (docs/plans/home-and-helpers.md, stage 7, D23). Every address those headers give counts — the last of
//! `X-Forwarded-For`, `X-Real-IP`, the last `for=` of `Forwarded` — and the farthest of them decides: a proxy writes one
//! of them and passes the others on as the client sent them, so a client can only take itself farther. The zone is
//! capped by the socket's, and it is never «this computer»: a home client behind a home proxy stays at home.

use std::net::{IpAddr, SocketAddr};

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

impl Zone {
    /// How near: outside 0, the home 1, this computer 2.
    fn nearness(self) -> u8 {
        match self {
            Zone::Remote => 0,
            Zone::Home => 1,
            Zone::Local => 2,
        }
    }
}

/// The headers a proxy writes when it hands a request on.
const FORWARDING: [&str; 4] = ["x-forwarded-for", "forwarded", "x-real-ip", "via"];

/// The request came through a proxy that said so.
pub fn forwarded(headers: &HeaderMap) -> bool {
    FORWARDING.iter().any(|name| headers.contains_key(*name))
}

/// An address as a proxy writes it: quotes, brackets and a port cut; None when it is no address.
fn address_of(text: &str) -> Option<IpAddr> {
    let text = text.trim().trim_matches('"');
    let bare = if let Some(rest) = text.strip_prefix('[') {
        rest.split(']').next().unwrap_or(rest)
    } else if text.matches(':').count() == 1 {
        text.split(':').next().unwrap_or(text)
    } else {
        text
    };
    bare.parse().ok()
}

/// A header's values as one list; None when the request has no such header.
fn all_of(headers: &HeaderMap, name: &str) -> Option<String> {
    let values: Vec<&str> = headers.get_all(name).iter().filter_map(|value| value.to_str().ok()).collect();
    (!values.is_empty() || headers.contains_key(name)).then(|| values.join(","))
}

/// The last address of `X-Forwarded-For` — the one the nearest proxy wrote —, when it has one.
fn forwarded_for_last(headers: &HeaderMap) -> Option<Option<IpAddr>> {
    let list = all_of(headers, "x-forwarded-for")?;
    Some(list.split(',').map(str::trim).rfind(|item| !item.is_empty()).and_then(address_of))
}

/// Who a proxied request came for, as each forwarding header that names one says it: the last of `X-Forwarded-For`,
/// `X-Real-IP`, the last `for=` of `Forwarded` — None for a header that names no address. Empty when no header gives
/// one (`Via` alone).
fn forwarded_clients(headers: &HeaderMap) -> Vec<Option<IpAddr>> {
    let mut clients = Vec::new();
    if let Some(last) = forwarded_for_last(headers) {
        clients.push(last);
    }
    if let Some(real) = all_of(headers, "x-real-ip") {
        clients.push(address_of(&real));
    }
    if let Some(forwarded) = all_of(headers, "forwarded") {
        let last_for = forwarded
            .split([',', ';'])
            .filter_map(|pair| pair.trim().split_once('='))
            .filter(|(key, _)| key.trim().eq_ignore_ascii_case("for"))
            .map(|(_, value)| value)
            .next_back();
        clients.push(last_for.and_then(address_of));
    }
    clients
}

pub fn zone_of(headers: &HeaderMap, peer: Option<SocketAddr>, home_networks: &[String]) -> Zone {
    if via_tunnel(headers) {
        return Zone::Remote;
    }
    let address = peer.map(|peer| peer.ip().to_string()).unwrap_or_default();
    let socket = if in_list(&address, LOOPBACK) {
        Zone::Local
    } else if in_networks(&address, home_networks) {
        Zone::Home
    } else {
        Zone::Remote
    };
    if !forwarded(headers) {
        return socket;
    }
    // A proxy is never «this computer»: its client is at home at the most, and never nearer than the proxy itself. A
    // header that names no address leaves the client outside; of those that do, the farthest decides.
    let clients = forwarded_clients(headers);
    if clients.is_empty() || clients.iter().any(Option::is_none) {
        return Zone::Remote;
    }
    let zone = clients
        .iter()
        .flatten()
        .map(|client| {
            let client = client.to_string();
            if in_list(&client, LOOPBACK) || in_networks(&client, home_networks) {
                Zone::Home
            } else {
                Zone::Remote
            }
        })
        .min_by_key(|zone| zone.nearness())
        .unwrap_or(Zone::Remote);
    if zone.nearness() <= socket.nearness() {
        zone
    } else {
        socket
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

/// The place a request a proxy on this computer handed on is counted by (sign-in failures, pairings): `proxied:<client>`
/// when every forwarding header that names a client names the same one — the one the nearest proxy wrote —; `proxied`,
/// one place for all of them, when none names one or they disagree: a client never picks a place of its own by adding a
/// header of its own, and never lands in this computer's own.
pub fn forwarded_source(headers: &HeaderMap) -> String {
    let clients = forwarded_clients(headers);
    match clients.first() {
        Some(Some(first)) if clients.iter().all(|client| *client == Some(*first)) => format!("proxied:{first}"),
        _ => "proxied".to_string(),
    }
}

/// The address to count sign-in failures by. Only a proxy on this computer is trusted to say whom a request came for:
/// from any other socket, the socket's own address. From loopback: a Cloudflare tunnel's visitor
/// (`tunnel:<cf-connecting-ip>`, beside cloudflared's `cf-ray`, when the last `X-Forwarded-For` hop — the one
/// Cloudflare's edge appended — is that visitor, and no other forwarding header names anyone else); a request another proxy handed on, by the client it
/// wrote (`forwarded_source`); this computer's own address only for a request nobody handed on — so the internet never
/// shares a place with a sign-in at the computer itself.
pub fn client_address(headers: &HeaderMap, peer: Option<SocketAddr>) -> String {
    let socket = peer.map(|peer| peer.ip().to_string()).unwrap_or_else(|| "unknown".into());
    if !in_list(&socket, LOOPBACK) {
        return socket;
    }
    let visitor = headers.get("cf-connecting-ip").and_then(|value| value.to_str().ok()).and_then(address_of);
    if let Some(visitor) = visitor.filter(|_| headers.contains_key("cf-ray")) {
        // Every forwarding header must name that visitor, and `X-Forwarded-For` must be there: a proxy that writes only
        // `X-Real-IP` (nginx's common setup) passes a client's own `cf-*` headers on, and a fresh `cf-connecting-ip` per
        // request would be a fresh place for each wrong password.
        let clients = forwarded_clients(headers);
        if forwarded_for_last(headers).is_some() && clients.iter().all(|client| *client == Some(visitor)) {
            return format!("tunnel:{visitor}");
        }
    }
    if forwarded(headers) || via_tunnel(headers) {
        return forwarded_source(headers);
    }
    socket
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(ip: &str) -> Option<SocketAddr> {
        Some(SocketAddr::new(ip.parse().unwrap(), 5000))
    }

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.append(axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(), value.parse().unwrap());
        }
        headers
    }

    #[test]
    fn decides_the_zone() {
        let home = vec!["private".to_string()];
        let none = HeaderMap::new();
        assert_eq!(zone_of(&none, peer("127.0.0.1"), &home), Zone::Local);
        assert_eq!(zone_of(&none, peer("192.168.1.20"), &home), Zone::Home);
        assert_eq!(zone_of(&none, peer("8.8.8.8"), &home), Zone::Remote);
        let tunnel = headers(&[("cf-ray", "x")]);
        assert_eq!(zone_of(&tunnel, peer("127.0.0.1"), &home), Zone::Remote);
        assert!(zone_allows(Zone::Home, Zone::Local));
        assert!(!zone_allows(Zone::Local, Zone::Home));
        let visitor =
            headers(&[("cf-connecting-ip", "1.2.3.4"), ("cf-ray", "8f00-WAW"), ("x-forwarded-for", "1.2.3.4")]);
        assert_eq!(client_address(&visitor, peer("127.0.0.1")), "tunnel:1.2.3.4");
        assert_eq!(client_address(&visitor, peer("10.0.0.2")), "10.0.0.2", "only loopback is trusted to forward");
    }

    #[test]
    fn a_proxied_request_is_judged_by_the_client_it_came_for() {
        let home = vec!["private".to_string()];
        let zone = |pairs: &[(&str, &str)], socket: &str| zone_of(&headers(pairs), peer(socket), &home);
        // A proxy on this computer: the internet stays outside, a home client stays at home — never «this computer».
        assert_eq!(zone(&[("x-forwarded-for", "198.51.100.7")], "127.0.0.1"), Zone::Remote);
        assert_eq!(zone(&[("x-forwarded-for", "192.168.1.20")], "127.0.0.1"), Zone::Home);
        assert_eq!(zone(&[("x-forwarded-for", "198.51.100.7, 192.168.1.20")], "127.0.0.1"), Zone::Home, "the last");
        assert_eq!(zone(&[("x-forwarded-for", "192.168.1.20, 198.51.100.7")], "127.0.0.1"), Zone::Remote);
        assert_eq!(zone(&[("x-forwarded-for", "127.0.0.1")], "192.168.1.20"), Zone::Home, "never Local");
        assert_eq!(zone(&[("x-forwarded-for", "127.0.0.1")], "127.0.0.1"), Zone::Home, "a proxy is not this computer");
        // Capped by the socket: a remote proxy cannot bring a home address in.
        assert_eq!(zone(&[("x-forwarded-for", "192.168.1.20")], "198.51.100.9"), Zone::Remote);
        // The other headers, ports and brackets cut.
        assert_eq!(zone(&[("x-real-ip", "192.168.1.30")], "127.0.0.1"), Zone::Home);
        assert_eq!(zone(&[("forwarded", "for=198.51.100.7;proto=https, for=\"[::1]:4711\"")], "::1"), Zone::Home);
        assert_eq!(zone(&[("forwarded", "for=192.168.1.20:5000")], "127.0.0.1"), Zone::Home);
        assert_eq!(zone(&[("forwarded", "for=198.51.100.7")], "127.0.0.1"), Zone::Remote);
        // A forwarding header without an address: outside.
        assert_eq!(zone(&[("via", "1.1 caddy")], "127.0.0.1"), Zone::Remote);
        assert_eq!(zone(&[("x-forwarded-for", "unknown")], "127.0.0.1"), Zone::Remote);
        assert_eq!(zone(&[("forwarded", "proto=https")], "127.0.0.1"), Zone::Remote);
        // Every address counts, the farthest decides: a proxy that writes only X-Real-IP passes the client's own
        // X-Forwarded-For on, and a home address there brings nobody home.
        let real_ip_only = [("x-forwarded-for", "192.168.1.20"), ("x-real-ip", "198.51.100.7")];
        assert_eq!(zone(&real_ip_only, "127.0.0.1"), Zone::Remote);
        assert_eq!(
            zone(&[("x-forwarded-for", "198.51.100.7"), ("x-real-ip", "192.168.1.20")], "127.0.0.1"),
            Zone::Remote
        );
        assert_eq!(
            zone(&[("x-forwarded-for", "192.168.1.20"), ("x-real-ip", "192.168.1.20")], "127.0.0.1"),
            Zone::Home
        );
        assert_eq!(zone(&[("x-real-ip", "192.168.1.20"), ("forwarded", "for=unknown")], "127.0.0.1"), Zone::Remote);
    }

    #[test]
    fn a_proxied_sign_in_is_counted_by_its_client_never_by_this_computer() {
        let at = |pairs: &[(&str, &str)], socket: &str| client_address(&headers(pairs), peer(socket));
        // This computer itself, nobody between.
        assert_eq!(at(&[], "127.0.0.1"), "127.0.0.1");
        // A Cloudflare tunnel as cloudflared hands it on: the visitor, also the last hop the edge appended.
        let tunnel =
            [("cf-connecting-ip", "198.51.100.7"), ("cf-ray", "8f00-WAW"), ("x-forwarded-for", "198.51.100.7")];
        assert_eq!(at(&tunnel, "127.0.0.1"), "tunnel:198.51.100.7");
        let earlier =
            [("cf-connecting-ip", "198.51.100.7"), ("cf-ray", "x"), ("x-forwarded-for", "10.9.9.9, 198.51.100.7")];
        assert_eq!(at(&earlier, "127.0.0.1"), "tunnel:198.51.100.7", "what the visitor sent itself comes first");
        // A proxy on this computer: the client it wrote; a cloudflared header another proxy passed on is no visitor.
        assert_eq!(at(&[("x-forwarded-for", "198.51.100.8")], "127.0.0.1"), "proxied:198.51.100.8");
        let passed = [("cf-connecting-ip", "1.2.3.4"), ("cf-ray", "x"), ("x-forwarded-for", "198.51.100.8")];
        assert_eq!(at(&passed, "127.0.0.1"), "proxied:198.51.100.8");
        // nginx writing only `X-Real-IP`: a client's own `cf-*` headers name no visitor — never a fresh place per request.
        let real_only = [("cf-connecting-ip", "1.2.3.4"), ("cf-ray", "x"), ("x-real-ip", "198.51.100.8")];
        assert_eq!(at(&real_only, "127.0.0.1"), "proxied:198.51.100.8");
        let no_hop = [("cf-connecting-ip", "1.2.3.4"), ("cf-ray", "x")];
        assert_ne!(at(&no_hop, "127.0.0.1"), "tunnel:1.2.3.4", "no X-Forwarded-For: not the edge's request");
        let disagree = [
            ("cf-connecting-ip", "1.2.3.4"),
            ("cf-ray", "x"),
            ("x-forwarded-for", "1.2.3.4"),
            ("x-real-ip", "198.51.100.8"),
        ];
        assert_eq!(at(&disagree, "127.0.0.1"), "proxied");
        // Headers that disagree or name nobody: one place for all of them, never this computer's.
        assert_eq!(at(&[("x-forwarded-for", "198.51.100.8"), ("x-real-ip", "198.51.100.9")], "127.0.0.1"), "proxied");
        assert_eq!(at(&[("via", "1.1 caddy")], "127.0.0.1"), "proxied");
        assert_eq!(
            at(&[("cf-connecting-ip", "1.2.3.4")], "127.0.0.1"),
            "proxied",
            "cloudflared's header needs its ray"
        );
        // Any other socket counts by itself, whatever it says.
        assert_eq!(at(&tunnel, "10.0.0.2"), "10.0.0.2");
        assert_eq!(at(&[("x-forwarded-for", "198.51.100.8")], "192.168.1.30"), "192.168.1.30");
    }
}
