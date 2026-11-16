//! Who may reach the debug API, the station's HTTP and the first-run setup: addresses in the listed networks (CIDR),
//! "private" for the home and loopback ones. And whether a service's address is at home, so what is sent there
//! stays in the house (DESIGN.md: whatever leaves it is amber).

use std::net::IpAddr;

use axum::http::HeaderMap;

const PRIVATE: &[&str] = &[
    "127.0.0.0/8",
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "169.254.0.0/16",
    "::1/128",
    "fc00::/7",
    "fe80::/10",
];

pub const LOOPBACK: &[&str] = &["127.0.0.0/8", "::1/128"];

/// Names that only resolve inside a home: mDNS and the usual router zones.
pub fn local_name(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    ["localhost", "local", "lan", "home", "internal", "home.arpa"]
        .iter()
        .any(|zone| host == *zone || host.ends_with(&format!(".{zone}")))
}

/// An address as a number and its width: IPv4 in 32 bits, IPv6 in 128; IPv4-mapped IPv6 as IPv4.
fn parse(address: &str) -> Option<(u128, u32)> {
    let plain = address.strip_prefix("::ffff:").or_else(|| address.strip_prefix("::FFFF:")).unwrap_or(address);
    match plain.trim_matches(['[', ']']).parse::<IpAddr>().ok()? {
        IpAddr::V4(v4) => Some((u32::from(v4) as u128, 32)),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => Some((u32::from(v4) as u128, 32)),
            None => Some((u128::from(v6), 128)),
        },
    }
}

/// A CIDR prefix length: an integer from 0 to `max`; the whole address when none is written.
fn prefix(text: Option<&str>, max: u32) -> Option<u32> {
    match text {
        None => Some(max),
        Some(text) if !text.is_empty() && text.len() <= 3 && text.bytes().all(|b| b.is_ascii_digit()) => {
            text.parse().ok().filter(|bits| *bits <= max)
        }
        _ => None,
    }
}

fn in_network(address: (u128, u32), network: &str) -> bool {
    let (base, bits) = match network.split_once('/') {
        Some((base, bits)) => (base, Some(bits)),
        None => (network, None),
    };
    let Some((base, width)) = parse(base) else { return false };
    if width != address.1 {
        return false;
    }
    // A prefix that is not one (a typo in DEBUG_API_ALLOW) matches nothing: closed, not open.
    let Some(bits) = prefix(bits, width) else { return false };
    if bits == 0 {
        return true;
    }
    let shift = width - bits;
    (address.0 >> shift) == (base >> shift)
}

/// Whether an address (IPv4, IPv6, or IPv4-mapped) is in one of the networks.
pub fn in_networks(address: &str, networks: &[String]) -> bool {
    let Some(parsed) = parse(address) else { return false };
    networks.iter().any(|network| {
        if network == "private" {
            PRIVATE.iter().any(|private| in_network(parsed, private))
        } else {
            in_network(parsed, network)
        }
    })
}

/// The same, for a fixed list.
pub fn in_list(address: &str, networks: &[&str]) -> bool {
    let Some(parsed) = parse(address) else { return false };
    networks.iter().any(|network| in_network(parsed, network))
}

/// The entries of a list that are not a network (a typo): for the startup log.
pub fn invalid_networks(networks: &[String]) -> Vec<String> {
    networks
        .iter()
        .filter(|network| {
            if *network == "private" {
                return false;
            }
            let (base, bits) = match network.split_once('/') {
                Some((base, bits)) => (base, Some(bits)),
                None => (network.as_str(), None),
            };
            match parse(base) {
                Some((_, width)) => prefix(bits, width).is_none(),
                None => true,
            }
        })
        .cloned()
        .collect()
}

/// The entries of a list that reach beyond these networks — outside all of them, or wider than the one they start in —
/// for the startup log of the link's proxies (pc). "private" (every home network) and a typo count as beyond
/// (`invalid_networks` names a typo).
#[cfg(feature = "pc")]
pub fn beyond(networks: &[String], within: &[&str]) -> Vec<String> {
    let network_of = |network: &str| {
        let (base, bits) = match network.split_once('/') {
            Some((base, bits)) => (base, Some(bits)),
            None => (network, None),
        };
        let address = parse(base)?;
        Some((address, prefix(bits, address.1)?))
    };
    networks
        .iter()
        .filter(|network| {
            let Some((address, bits)) = network_of(network) else { return true };
            !within.iter().any(|outer| {
                network_of(outer).is_some_and(|(_, outer_bits)| outer_bits <= bits) && in_network(address, outer)
            })
        })
        .cloned()
        .collect()
}

/// The entries of a list wider than an IPv4 network of `v4` bits or an IPv6 one of `v6` — "private" too —, for the
/// startup log of the link's proxies (pc). A typo is not counted here (`invalid_networks` names it).
#[cfg(feature = "pc")]
pub fn wider_than(networks: &[String], v4: u32, v6: u32) -> Vec<String> {
    networks
        .iter()
        .filter(|network| {
            if *network == "private" {
                return true;
            }
            let (base, bits) = match network.split_once('/') {
                Some((base, bits)) => (base, Some(bits)),
                None => (network.as_str(), None),
            };
            let Some((_, width)) = parse(base) else { return false };
            prefix(bits, width).is_some_and(|bits| bits < if width == 32 { v4 } else { v6 })
        })
        .cloned()
        .collect()
}

/// Whether a service's address is this computer or the home network, so what is sent there stays in the house. No
/// address is nothing sent anywhere. A name without a dot — a Docker service (`asr`), a machine on the home network
/// (`raspberrypi`) — has no public DNS: it is at home too.
pub fn is_local_url(url: Option<&str>) -> bool {
    let Some(url) = url.filter(|url| !url.is_empty()) else { return true };
    let Ok(parsed) = url::Url::parse(url) else { return false };
    let Some(host) = parsed.host_str() else { return false };
    let host = host.trim_matches(['[', ']']);
    if !host.contains('.') && !host.contains(':') {
        return true;
    }
    local_name(host) || in_list(host, PRIVATE)
}

/// A request that came through a Cloudflare tunnel: cloudflared on this machine connects from loopback, but the one
/// asking is on the internet. Its headers say so.
pub fn via_tunnel(headers: &HeaderMap) -> bool {
    headers.contains_key("cf-connecting-ip") || headers.contains_key("cf-ray")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nets(list: &[&str]) -> Vec<String> {
        list.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn matches_networks_both_families() {
        assert!(in_networks("192.168.1.30", &nets(&["private"])));
        assert!(in_networks("::ffff:192.168.1.5", &nets(&["192.168.1.0/24"])));
        assert!(in_networks("::1", &nets(&["private"])));
        assert!(!in_networks("8.8.8.8", &nets(&["private"])));
        assert!(in_networks("10.1.2.3", &nets(&["10.0.0.0/8"])));
        assert!(!in_networks("10.1.2.3", &nets(&["10.0.0.0/33"])), "a typo matches nothing");
        assert!(in_networks("1.2.3.4", &nets(&["0.0.0.0/0"])));
        assert!(in_networks("fd00::1", &nets(&["private"])));
        assert!(!in_networks("not an ip", &nets(&["private"])));
        assert!(in_networks("192.168.1.7", &nets(&["192.168.1.7"])));
    }

    #[cfg(feature = "pc")]
    #[test]
    fn names_the_entries_beyond_some_networks() {
        let here = &["127.0.0.0/8", "::1/128", "172.16.0.0/12"];
        assert!(
            beyond(&nets(&["127.0.0.1", "::1", "172.17.0.0/16", "172.18.0.1", "::ffff:172.17.0.1"]), here).is_empty()
        );
        let out = ["192.0.2.5", "10.0.0.0/8", "172.0.0.0/8", "0.0.0.0/0", "fd00::/8", "private", "10.0.0.0/40"];
        assert_eq!(beyond(&nets(&out), here), nets(&out), "outside, wider, every home network, a typo");
    }

    #[test]
    fn lists_typos() {
        assert_eq!(
            invalid_networks(&nets(&["private", "10.0.0.0/8", "10.0.0.0/40", "home"])),
            vec!["10.0.0.0/40", "home"]
        );
    }

    #[test]
    fn local_urls_stay_in_the_house() {
        assert!(is_local_url(None));
        assert!(is_local_url(Some("http://127.0.0.1:8090/v1")));
        assert!(is_local_url(Some("http://asr:8090/v1")));
        assert!(is_local_url(Some("http://nas.local:11434")));
        assert!(is_local_url(Some("http://192.168.1.10")));
        assert!(!is_local_url(Some("https://api.anthropic.com")));
        assert!(!is_local_url(Some("not a url")));
    }
}
