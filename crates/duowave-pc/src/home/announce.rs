//! «Поиск дома» (docs/plans/home-setup-flow.md, stage 6): this DuoWave gateway — a computer's or a server's — answers
//! the home network's mDNS questions about `_duowave._tcp` with what it is: its role (`centre`, `turbo` of a station's
//! home, `ear`), its port, version, the home's name and id, and for a Turbo or an ear the centre it belongs to. Another
//! computer then finds a home whose centre is not on a station, on any port, and tells a centre from an ear
//! (stations/connect/discovery.rs, `discover_homes`).
//!
//! It shares UDP 5353 with the system's own responder (the same socket the station search uses), says itself once at
//! the start, answers each question — to the group, and straight to the asker's port for a question from another port
//! (RFC 6762's legacy unicast: the search asks from a random port) — and says goodbye (TTL 0) when the gateway stops.
//! Stations do not announce: their RAM and space are tight, and they are found as before.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::task::JoinHandle;

use super::computer::Computer;
use super::Parts;
use crate::stations::connect::discovery::{self, DnsRecord, DUOWAVE_SERVICE, MDNS_GROUP, MDNS_PORT};

/// The records stand this long in the askers' caches.
pub const TTL: u32 = 120;
/// While the system keeps the announcement off the home network, it is said again this often: macOS asks the person
/// once, and «Разрешить» then takes effect without a restart.
const AGAIN: Duration = Duration::from_secs(30);

static RUNNING: Mutex<Option<(JoinHandle<()>, Vec<u8>)>> = Mutex::new(None);

/// What this gateway is, as its TXT says it.
pub fn role(parts: &Parts, pc: &Computer) -> &'static str {
    if crate::home::ear::centre(&parts.settings, &parts.secrets).is_some() {
        return "ear";
    }
    if crate::home::devices::centre_elsewhere(&pc.station_gateway).is_some() {
        return "turbo";
    }
    "centre"
}

/// A DNS label from a name: letters, digits and dashes, at most 40.
pub fn label(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    cleaned.chars().take(40).collect()
}

/// This gateway's records for an asker on `near`: the address on its network goes into A.
pub fn records(parts: &Parts, pc: &Computer, near: Option<Ipv4Addr>) -> Vec<DnsRecord> {
    let me = parts.identity.as_helper();
    let id: String = me.id.chars().filter(char::is_ascii_alphanumeric).take(8).collect();
    let id = if id.is_empty() { "duowave".to_string() } else { id };
    let instance = format!("duowave-{}-{id}.{DUOWAVE_SERVICE}", label(&me.name).trim_matches('-'));
    let host = format!("duowave-{id}.local");
    // The port it really listens on (the desktop app may bind another than PORT), else the configured one.
    let port = parts.config.app.public_port.unwrap_or_else(|| match parts.helpers.local_port() {
        0 => parts.config.app.port,
        bound => bound,
    });
    let mut txt = std::collections::BTreeMap::new();
    txt.insert("role".to_string(), role(parts, pc).to_string());
    txt.insert("port".to_string(), port.to_string());
    txt.insert("version".to_string(), crate::VERSION.to_string());
    txt.insert("name".to_string(), me.name.chars().take(60).collect());
    txt.insert("home".to_string(), parts.identity.home_name().chars().take(60).collect());
    if let Some(home) = parts.identity.home() {
        txt.insert("homeId".to_string(), home.id);
    }
    // Who this gateway is, and where its link listens when it helps: a centre whose Turbo moved finds it by these
    // (helpers::search_moved).
    if !me.id.is_empty() {
        txt.insert("id".to_string(), me.id.clone());
    }
    if let Some(link) = pc.link.port().filter(|_| pc.link.helps()) {
        txt.insert("link".to_string(), link.to_string());
    }
    if let Some((ear, _)) = crate::home::ear::centre(&parts.settings, &parts.secrets) {
        txt.insert("centre".to_string(), format!("{}:{}", ear.address, ear.port));
    }
    if let Some(record) = crate::home::devices::centre_elsewhere(&pc.station_gateway).and_then(|centre| centre.record())
    {
        txt.insert("centre".to_string(), format!("{}:{}", record.address, record.port));
    }
    let mut records = vec![
        DnsRecord::Ptr { name: DUOWAVE_SERVICE.to_string(), target: instance.clone() },
        DnsRecord::Srv { name: instance.clone(), target: host.clone(), port },
        DnsRecord::Txt { name: instance, entries: txt },
    ];
    for address in addresses(near) {
        records.push(DnsRecord::A { name: host.clone(), address: address.to_string() });
    }
    records
}

/// This computer's home addresses: the one on the asker's /24 first, when there is one; adapters of virtual machines
/// and containers left out.
fn addresses(near: Option<Ipv4Addr>) -> Vec<Ipv4Addr> {
    let mine: Vec<Ipv4Addr> = crate::stations::connect::station_connect::host_interfaces()
        .into_iter()
        .filter(|entry| entry.ipv4 && !entry.internal)
        .filter_map(|entry| entry.address.parse().ok())
        .collect();
    let same = |ip: &Ipv4Addr| near.is_some_and(|near| near.octets()[..3] == ip.octets()[..3]);
    let nearest: Vec<Ipv4Addr> = mine.iter().copied().filter(same).collect();
    if nearest.is_empty() {
        mine
    } else {
        nearest
    }
}

/// Starts the announcer (again: an earlier one of this process stops first — the desktop app restarts its engine in
/// the same process).
pub fn start(parts: Parts, pc: Arc<Computer>) {
    stop_quietly();
    let goodbye = discovery::mdns_answer(&records(&parts, &pc, None), 0);
    let task = tokio::spawn(async move {
        // A fresh Mac app says nothing on the home network until its person was asked (step «Разрешения»).
        crate::permissions::hold_until_asked(&parts.settings, &parts.db, crate::permissions::routes::desktop()).await;
        let Some(socket) = discovery::shared_socket() else {
            tracing::info!(target: "home", "announce: UDP {MDNS_PORT} is the system's own — this gateway is not announced");
            return;
        };
        if let Err(error) = socket.join_multicast_v4(MDNS_GROUP, Ipv4Addr::UNSPECIFIED) {
            tracing::info!(target: "home", "announce: no multicast here ({error}) — not announced");
            return;
        }
        let group = SocketAddr::V4(SocketAddrV4::new(MDNS_GROUP, MDNS_PORT));
        let mut failing = false;
        // Said twice at the start, a second apart (RFC 6762, 8.3), then answered when asked.
        for _ in 0..2 {
            told(socket.send_to(&discovery::mdns_answer(&records(&parts, &pc, None), TTL), group).await, &mut failing);
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let mut buffer = vec![0u8; 9000];
        loop {
            let received = if failing {
                match tokio::time::timeout(AGAIN, socket.recv_from(&mut buffer)).await {
                    Ok(received) => received,
                    Err(_) => {
                        let again = discovery::mdns_answer(&records(&parts, &pc, None), TTL);
                        told(socket.send_to(&again, group).await, &mut failing);
                        continue;
                    }
                }
            } else {
                socket.recv_from(&mut buffer).await
            };
            let Ok((size, from)) = received else { continue };
            if !discovery::asks_for(&buffer[..size], DUOWAVE_SERVICE) {
                continue;
            }
            let near = match from.ip() {
                std::net::IpAddr::V4(ip) => Some(ip),
                std::net::IpAddr::V6(_) => None,
            };
            let answer = discovery::mdns_answer(&records(&parts, &pc, near), TTL);
            if from.port() == MDNS_PORT {
                told(socket.send_to(&answer, group).await, &mut failing);
            } else {
                // A question from another port (the search's own socket): straight back to it.
                told(socket.send_to(&answer, from).await, &mut failing);
            }
        }
    });
    *RUNNING.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((task, goodbye));
}

/// One send of the announcement: a failure goes to the log once — not at every question — with what it means, and the
/// way back once more; the panel learns of «no route» from `OFF_NETWORK` (step 11 check, finding 3: a fresh build on a
/// second Mac was never found, and its log said nothing).
fn told(sent: std::io::Result<usize>, failing: &mut bool) {
    discovery::OFF_NETWORK.note(&sent);
    match sent {
        Ok(_) => {
            if std::mem::take(failing) {
                tracing::info!(target: "home", "announce: said on the home network again");
            }
        }
        Err(error) => {
            if !std::mem::replace(failing, true) {
                let hint = discovery::kept_off_hint(&error);
                tracing::warn!(target: "home", "announce: not said on the home network ({error}){hint}");
            }
        }
    }
}

fn stop_quietly() {
    if let Some((task, _)) = RUNNING.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take() {
        task.abort();
    }
}

/// The gateway stops: goodbye to the caches (TTL 0), then the announcer ends.
pub async fn stop() {
    let running = RUNNING.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take();
    let Some((task, goodbye)) = running else { return };
    task.abort();
    if let Ok(socket) = tokio::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await {
        let _ = socket.send_to(&goodbye, SocketAddr::V4(SocketAddrV4::new(MDNS_GROUP, MDNS_PORT))).await;
    }
}
