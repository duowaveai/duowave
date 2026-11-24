//! Where the gateway's own connections leave from. Always the system's choice — except on a Mac with pretend stations
//! (docs/plans/pretend-stations.md): each pretend gateway has an address of its own on the loopback (10.77.0.x), and
//! a connection between two of them would leave from the address it goes to — the Mac takes the destination for the
//! source of a local one — so every gateway would see the others at its own address. Those connections leave from the
//! gateway's own pretend address instead: TCP dialled here (`dial`), and HTTP through a small entry on 127.0.0.1 that
//! dials the same way (`entry`, src/egress/mod.rs). Nothing changes without DUOWAVE_PRETEND.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::OnceLock;

use tokio::net::{TcpSocket, TcpStream};

/// This gateway's pretend address: set once, at the start of a pretend one.
static OWN: OnceLock<Ipv4Addr> = OnceLock::new();
/// The port of the entry that dials from it (0 — none).
static ENTRY: AtomicU16 = AtomicU16::new(0);

/// From now on, connections to the other pretend addresses (this one's /24) leave from `address`.
pub fn leave_from(address: Ipv4Addr) {
    let _ = OWN.set(address);
}

/// The entry on 127.0.0.1 that dials from the pretend address (src/egress/app_entry.rs, every host direct).
pub fn set_entry(port: u16) {
    ENTRY.store(port, Ordering::Relaxed);
}

/// The source for a connection to `to`: this gateway's pretend address when `to` is another one, else the system's.
pub fn source_for(to: IpAddr) -> Option<Ipv4Addr> {
    near(OWN.get().copied(), to)
}

fn near(own: Option<Ipv4Addr>, to: IpAddr) -> Option<Ipv4Addr> {
    let own = own?;
    match to {
        IpAddr::V4(to) if to != own && to.octets()[..3] == own.octets()[..3] => Some(own),
        _ => None,
    }
}

/// The proxy an HTTP call to `host` takes: the entry, for another pretend address.
pub fn proxy_for(host: &str) -> Option<url::Url> {
    let ip: IpAddr = host.trim_start_matches('[').trim_end_matches(']').parse().ok()?;
    source_for(ip)?;
    match ENTRY.load(Ordering::Relaxed) {
        0 => None,
        port => url::Url::parse(&format!("http://127.0.0.1:{port}")).ok(),
    }
}

/// A TCP connection to `host:port`, from the pretend address when `host` is another pretend one.
pub async fn dial(host: &str, port: u16) -> std::io::Result<TcpStream> {
    let to = host.trim_start_matches('[').trim_end_matches(']').parse::<IpAddr>().ok();
    match to.and_then(|to| source_for(to).map(|from| (to, from))) {
        Some((to, from)) => dial_from(from.into(), SocketAddr::new(to, port)).await,
        None => TcpStream::connect((host, port)).await,
    }
}

/// A TCP connection to `to` that leaves from `from`.
pub async fn dial_from(from: IpAddr, to: SocketAddr) -> std::io::Result<TcpStream> {
    let socket = if to.is_ipv4() { TcpSocket::new_v4()? } else { TcpSocket::new_v6()? };
    socket.bind(SocketAddr::new(from, 0))?;
    socket.connect(to).await
}

/// This machine, the home network, a station: never through a server.
pub fn is_local(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = bare.parse::<IpAddr>() {
        return match ip {
            IpAddr::V4(v4) => {
                v4.is_loopback()
                    || v4.is_private()
                    || v4.is_link_local()
                    || v4.is_unspecified()
                    || v4.octets()[0] == 100 && (64..128).contains(&v4.octets()[1])
            }
            IpAddr::V6(v6) => {
                v6.is_loopback()
                    || v6.is_unspecified()
                    || (v6.segments()[0] & 0xfe00) == 0xfc00
                    || (v6.segments()[0] & 0xffc0) == 0xfe80
            }
        };
    }
    host == "localhost"
        || !host.contains('.')
        || [".local", ".lan", ".home", ".internal", ".localhost", ".home.arpa"].iter().any(|end| host.ends_with(end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_another_pretend_address_leaves_from_this_one() {
        let own = Some(Ipv4Addr::new(10, 77, 0, 2));
        assert_eq!(near(own, "10.77.0.3".parse().unwrap()), own, "another pretend station");
        assert_eq!(near(own, "10.77.0.2".parse().unwrap()), None, "itself");
        assert_eq!(near(own, "192.168.0.20".parse().unwrap()), None, "the home network: the system's way");
        assert_eq!(near(own, "127.0.0.1".parse().unwrap()), None);
        assert_eq!(near(None, "10.77.0.3".parse().unwrap()), None, "no pretend gateway: never");
    }

    #[tokio::test]
    async fn a_connection_leaves_from_the_address_asked() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let to = listener.local_addr().unwrap();
        let dialled = tokio::spawn(async move { dial_from("127.0.0.1".parse().unwrap(), to).await.unwrap() });
        let (_, from) = listener.accept().await.unwrap();
        assert_eq!(from.ip(), "127.0.0.1".parse::<IpAddr>().unwrap());
        drop(dialled.await.unwrap());
    }
}
