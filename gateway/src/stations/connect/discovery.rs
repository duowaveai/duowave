//! Finding stations on the home network: they announce themselves over mDNS as _yandexio._tcp (glagol, the local
//! control the Yandex app uses). A query from a random port gets unicast answers (RFC 6762, legacy unicast); one from
//! 5353, where the OS lets us share it, gets the multicast ones too. Then each answer's SSH port is asked for its
//! banner — reading only. No package: a PTR query and the answer's records are a page of code.
//!
//! The same for homes (docs/plans/home-setup-flow.md, stage 6): a DuoWave gateway on a computer or a server announces
//! itself as _duowave._tcp (src/platform/announce.rs) — role, port, version, home — so a computer finds a centre that
//! is not on a station, and on a port other than 8790. Stations do not announce: their RAM is tight; they are found as
//! above, and their /health says whether they are a centre or an ear.

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tokio::io::AsyncReadExt;
use tokio::net::UdpSocket;
use tokio::time::{timeout, timeout_at, Instant};

pub const YANDEX_SERVICE: &str = "_yandexio._tcp.local";
/// DuoWave's own: a gateway on a computer or a server, with its role and its home in TXT.
pub const DUOWAVE_SERVICE: &str = "_duowave._tcp.local";
pub(crate) const MDNS_GROUP: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
pub(crate) const MDNS_PORT: u16 = 5353;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FoundStation {
    /// The IPv4 address the answer came from.
    pub address: String,
    /// The instance name it announces.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// TXT records: the model (`platform`) is what the panel shows.
    pub txt: BTreeMap<String, String>,
    /// Its SSH (port 2222 by the wiki): `dropbear_2022.83` — the banner's software — or null when closed.
    pub ssh: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type")]
pub enum DnsRecord {
    #[serde(rename = "PTR")]
    Ptr { name: String, target: String },
    #[serde(rename = "SRV")]
    Srv { name: String, target: String, port: u16 },
    #[serde(rename = "TXT")]
    Txt { name: String, entries: BTreeMap<String, String> },
    #[serde(rename = "A")]
    A { name: String, address: String },
}

/// A one-question mDNS query: PTR for the service, class IN.
pub fn mdns_query(service: &str) -> Vec<u8> {
    let mut packet = vec![0u8; 12];
    packet[4..6].copy_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    for label in service.split('.').filter(|label| !label.is_empty()) {
        packet.push(label.len() as u8);
        packet.extend_from_slice(label.as_bytes());
    }
    packet.push(0);
    packet.extend_from_slice(&12u16.to_be_bytes()); // PTR
    packet.extend_from_slice(&1u16.to_be_bytes()); // IN
    packet
}

fn u16_at(packet: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*packet.get(at)?, *packet.get(at.checked_add(1)?)?]))
}

/// The bytes from `from` to `to`, cut at the packet's end.
fn clamped(packet: &[u8], from: usize, to: usize) -> &[u8] {
    let end = to.min(packet.len());
    &packet[from.min(end)..end]
}

/// A name and where the record goes on after it; None past the end or for a name that loops.
fn read_name(packet: &[u8], start: usize) -> Option<(String, usize)> {
    let mut labels: Vec<String> = Vec::new();
    let mut offset = start;
    let mut end = None;
    for _ in 0..32 {
        let length = *packet.get(offset)?;
        if length == 0 {
            return Some((labels.join("."), end.unwrap_or(offset + 1)));
        }
        if length & 0xc0 == 0xc0 {
            end.get_or_insert(offset + 2);
            offset = (usize::from(length & 0x3f) << 8) | usize::from(packet.get(offset + 1).copied().unwrap_or(0));
            continue;
        }
        let label = clamped(packet, offset + 1, offset + 1 + usize::from(length));
        labels.push(String::from_utf8_lossy(label).into_owned());
        offset += 1 + usize::from(length);
    }
    None
}

fn read_txt(data: &[u8]) -> BTreeMap<String, String> {
    let mut entries = BTreeMap::new();
    let mut offset = 0;
    while offset < data.len() {
        let length = usize::from(data[offset]);
        let text = String::from_utf8_lossy(clamped(data, offset + 1, offset + 1 + length)).into_owned();
        offset += 1 + length;
        match text.find('=') {
            Some(equals) if equals > 0 => {
                entries.insert(text[..equals].to_string(), text[equals + 1..].to_string());
            }
            _ if !text.is_empty() => {
                entries.insert(text, String::new());
            }
            _ => {}
        }
    }
    entries
}

/// The records of an mDNS answer: answers and additional ones alike. Malformed packets give what was read so far.
pub fn parse_records(packet: &[u8]) -> Vec<DnsRecord> {
    let mut records = Vec::new();
    if packet.len() >= 12 {
        // A truncated or hostile packet stops the reading: what was read stays.
        let _ = read_records(packet, &mut records);
    }
    records
}

fn read_records(packet: &[u8], records: &mut Vec<DnsRecord>) -> Option<()> {
    let questions = u16_at(packet, 4)?;
    let total = usize::from(u16_at(packet, 6)?) + usize::from(u16_at(packet, 8)?) + usize::from(u16_at(packet, 10)?);
    let mut offset = 12;
    for _ in 0..questions {
        offset = read_name(packet, offset)?.1 + 4;
    }
    for _ in 0..total {
        let (name, end) = read_name(packet, offset)?;
        offset = end;
        let kind = u16_at(packet, offset)?;
        let length = usize::from(u16_at(packet, offset + 8)?);
        let data = offset + 10;
        offset = data + length;
        if offset > packet.len() {
            break;
        }
        match kind {
            12 => records.push(DnsRecord::Ptr { name, target: read_name(packet, data)?.0 }),
            33 => {
                let port = u16_at(packet, data + 4)?;
                records.push(DnsRecord::Srv { name, port, target: read_name(packet, data + 6)?.0 });
            }
            16 => records.push(DnsRecord::Txt { name, entries: read_txt(&packet[data..offset]) }),
            1 if length == 4 => {
                let address = Ipv4Addr::new(packet[data], packet[data + 1], packet[data + 2], packet[data + 3]);
                records.push(DnsRecord::A { name, address: address.to_string() });
            }
            _ => {}
        }
    }
    Some(())
}

/// The station an answer names, by the address it came from.
pub fn stations_in(records: &[DnsRecord], from: &str, service: &str) -> Option<FoundStation> {
    let target = records.iter().find_map(|record| match record {
        DnsRecord::Ptr { name, target } if name == service => Some(target),
        _ => None,
    })?;
    let txt = records.iter().find_map(|record| match record {
        DnsRecord::Txt { name, entries } if name == target => Some(entries.clone()),
        _ => None,
    });
    let label = target.strip_suffix(&format!(".{service}")).unwrap_or(target);
    Some(FoundStation {
        address: from.to_string(),
        name: (!label.is_empty() && !firmware_label(label)).then(|| label.to_string()),
        txt: txt.unwrap_or_default(),
        ssh: None,
    })
}

/// The instance a station's firmware announces by itself, `YandexIOReceiver-<serial>`: no name a person gave — the panel
/// names such a station by its model («Колонка Лайт 2»), never by this (the first steps, 24.11).
fn firmware_label(label: &str) -> bool {
    label.get(..17).is_some_and(|head| head.eq_ignore_ascii_case("YandexIOReceiver-"))
}

/// Asks the home network for stations for `ms` and returns what answered, one entry per address.
pub async fn discover_stations(ms: u64) -> Vec<FoundStation> {
    ask(&[SocketAddr::V4(SocketAddrV4::new(MDNS_GROUP, MDNS_PORT))], true, Duration::from_millis(ms)).await
}

/// Whether a DuoWave gateway the search heard is this gateway itself: by its id when the answer gives one, at any
/// address — the name is not to be trusted, it is said in the language of whoever reads it (24.11: the announcement
/// said «Mac computer» and the panel «Компьютер Mac», and the first steps offered a fresh Mac its own home as one
/// found). An answer without an id (a gateway that announces none) is this one at an address of this computer under
/// its name, as before.
pub fn is_this_gateway(home: &FoundStation, own_addresses: &[String], id: &str, name: &str) -> bool {
    match home.txt.get("id").filter(|found| !found.is_empty()) {
        Some(found) => !id.is_empty() && found == id,
        None => own_addresses.contains(&home.address) && home.txt.get("name").map(String::as_str) == Some(name),
    }
}

/// DuoWave gateways that announce themselves (computers, servers) for `ms`: their TXT has `role`, `port`, `version`,
/// `home`, `homeId`, and `centre` for a Turbo or an ear.
pub async fn discover_homes(ms: u64) -> Vec<FoundStation> {
    ask_for(
        DUOWAVE_SERVICE,
        &[SocketAddr::V4(SocketAddrV4::new(MDNS_GROUP, MDNS_PORT))],
        true,
        Duration::from_millis(ms),
    )
    .await
}

async fn ask(to: &[SocketAddr], shared: bool, window: Duration) -> Vec<FoundStation> {
    ask_for(YANDEX_SERVICE, to, shared, window).await
}

/// Stations at these addresses only (a pretend station in tests), unicast.
pub async fn ask_at(to: &[SocketAddr], window: Duration) -> Vec<FoundStation> {
    ask(to, false, window).await
}

/// Whether an mDNS packet is a question about `service` (PTR or ANY): the announcer answers those.
pub fn asks_for(packet: &[u8], service: &str) -> bool {
    let Some(flags) = u16_at(packet, 2) else { return false };
    if flags & 0x8000 != 0 {
        return false;
    }
    let questions = u16_at(packet, 4).unwrap_or(0);
    let mut offset = 12;
    for _ in 0..questions.min(16) {
        let Some((name, after)) = read_name(packet, offset) else { return false };
        let kind = u16_at(packet, after).unwrap_or(0);
        if name.eq_ignore_ascii_case(service) && (kind == 12 || kind == 255) {
            return true;
        }
        offset = after + 4;
    }
    false
}

fn put_name(packet: &mut Vec<u8>, name: &str) {
    for label in name.split('.').filter(|label| !label.is_empty()) {
        let bytes = &label.as_bytes()[..label.len().min(63)];
        packet.push(bytes.len() as u8);
        packet.extend_from_slice(bytes);
    }
    packet.push(0);
}

/// An mDNS answer with these records, as a responder sends it (QR and AA set, no questions; names uncompressed).
/// `ttl` — how long the records stand: 120 s, and 0 to say goodbye.
pub fn mdns_answer(records: &[DnsRecord], ttl: u32) -> Vec<u8> {
    let mut packet = vec![0u8; 12];
    packet[2..4].copy_from_slice(&0x8400u16.to_be_bytes());
    packet[6..8].copy_from_slice(&(records.len().min(usize::from(u16::MAX)) as u16).to_be_bytes());
    for record in records {
        let (name, kind, data) = match record {
            DnsRecord::Ptr { name, target } => {
                let mut data = Vec::new();
                put_name(&mut data, target);
                (name, 12u16, data)
            }
            DnsRecord::Srv { name, target, port } => {
                let mut data = vec![0, 0, 0, 0];
                data.extend_from_slice(&port.to_be_bytes());
                put_name(&mut data, target);
                (name, 33u16, data)
            }
            DnsRecord::Txt { name, entries } => {
                let mut data = Vec::new();
                for (key, value) in entries {
                    let text = format!("{key}={value}");
                    let bytes = &text.as_bytes()[..text.len().min(255)];
                    data.push(bytes.len() as u8);
                    data.extend_from_slice(bytes);
                }
                if data.is_empty() {
                    data.push(0);
                }
                (name, 16u16, data)
            }
            DnsRecord::A { name, address } => {
                let Ok(ip) = address.parse::<Ipv4Addr>() else { continue };
                (name, 1u16, ip.octets().to_vec())
            }
        };
        put_name(&mut packet, name);
        packet.extend_from_slice(&kind.to_be_bytes());
        // Class IN with the cache-flush bit for the unique records (SRV, TXT, A); a PTR is shared.
        let class: u16 = if kind == 12 { 1 } else { 0x8001 };
        packet.extend_from_slice(&class.to_be_bytes());
        packet.extend_from_slice(&ttl.to_be_bytes());
        packet.extend_from_slice(&(data.len() as u16).to_be_bytes());
        packet.extend_from_slice(&data);
    }
    packet
}

/// The query sent to each of `to` (the mDNS group; a pretend station in tests), and the answers heard for `window`.
async fn ask_for(service: &'static str, to: &[SocketAddr], shared: bool, window: Duration) -> Vec<FoundStation> {
    // A pretend gateway asks the pretend stations' addresses only, never the home network (src/stations/pretend.rs).
    let pretend =
        super::super::pretend::search_targets().filter(|_| to.iter().any(|target| target.ip().is_multicast()));
    let (to, shared) = match &pretend {
        Some(targets) => (targets.as_slice(), false),
        None => (to, shared),
    };
    if to.is_empty() {
        return Vec::new();
    }
    let query = mdns_query(service);
    let mut sockets = Vec::new();
    // Unicast answers to a random port.
    match UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await {
        Ok(unicast) => {
            for target in to {
                if let Err(error) = unicast.send_to(&query, target).await {
                    tracing::warn!("station search: the mDNS query to {target} was not sent: {error}");
                }
            }
            sockets.push(unicast);
        }
        Err(error) => tracing::warn!("station search: no UDP socket for the mDNS query: {error}"),
    }
    // Multicast ones on 5353, beside the OS's own responder where it allows; nothing lost if it does not.
    if shared {
        match shared_socket() {
            Some(socket) => {
                // No multicast here (a container's bridge): the unicast answers are all there is.
                match socket.join_multicast_v4(MDNS_GROUP, Ipv4Addr::UNSPECIFIED) {
                    Ok(()) => {
                        for target in to {
                            let _ = socket.send_to(&query, target).await;
                        }
                    }
                    Err(error) => {
                        tracing::info!("station search: no multicast on this computer ({error}); unicast answers only")
                    }
                }
                sockets.push(socket);
            }
            None => tracing::info!("station search: UDP {MDNS_PORT} is the system's own; unicast answers only"),
        }
    }
    let found: Arc<Mutex<Vec<FoundStation>>> = Arc::default();
    let deadline = Instant::now() + window;
    let listeners: Vec<_> = sockets
        .into_iter()
        .map(|socket| {
            let found = found.clone();
            tokio::spawn(async move {
                let mut buffer = vec![0u8; 9000];
                while let Ok(Ok((size, from))) = timeout_at(deadline, socket.recv_from(&mut buffer)).await {
                    let address = from.ip().to_string();
                    let Some(station) = stations_in(&parse_records(&buffer[..size]), &address, service) else {
                        continue;
                    };
                    let mut found = found.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    if !found.iter().any(|known| known.address == station.address) {
                        found.push(station);
                    }
                }
            })
        })
        .collect();
    for listener in listeners {
        let _ = listener.await;
    }
    let found = found.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    found.clone()
}

/// UDP 5353 shared with the system's responder (SO_REUSEADDR, and SO_REUSEPORT where it exists, as Node's
/// `reuseAddr` does); None when the system keeps it to itself.
pub(crate) fn shared_socket() -> Option<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).ok()?;
    socket.set_reuse_address(true).ok()?;
    #[cfg(all(unix, not(any(target_os = "solaris", target_os = "illumos", target_os = "cygwin"))))]
    socket.set_reuse_port(true).ok()?;
    socket.bind(&SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, MDNS_PORT)).into()).ok()?;
    socket.set_nonblocking(true).ok()?;
    UdpSocket::from_std(socket.into()).ok()
}

/// JavaScript's `\s` within Latin-1: what ends the banner's software.
fn js_space(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{A0}')
}

/// `SSH-<version>-<software>`: the software, when the line says it. A software not yet followed by a space or the
/// line's end may still be arriving: it counts only once the line is `complete`.
fn banner_software(text: &str, complete: bool) -> Option<String> {
    let rest = text.strip_prefix("SSH-")?;
    let version = rest.bytes().take_while(|byte| byte.is_ascii_digit() || *byte == b'.').count();
    if version == 0 {
        return None;
    }
    let rest = rest[version..].strip_prefix('-')?;
    let software: String = rest.chars().take_while(|c| !js_space(*c)).collect();
    if software.is_empty() {
        return None;
    }
    let terminated = rest.chars().nth(software.chars().count()).is_some();
    (terminated || complete).then(|| software.chars().take(60).collect())
}

/// Why nothing answered at address:port, as the connect itself said it (test 19.11, remark 18: the one-command install
/// said «No SSH answers» while the speaker's SSH was up — macOS kept the freshly downloaded program off the home
/// network, and every reason looked the same).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unreached {
    /// No answer in time: the speaker is off, elsewhere, or a firewall drops it.
    Timeout,
    /// It answered «closed»: the speaker is there, nothing listens on the port.
    Refused,
    /// «No route to host», «network unreachable», «permission denied»: the address is not reachable from here — on
    /// macOS also what Local Network privacy gives a program it has not let onto the home network.
    NoRoute,
    /// Anything else.
    Other,
}

/// The kind of a connect's error. The raw codes are macOS's and Linux's for the same: EPERM 1, EACCES 13, and
/// EHOSTUNREACH / ENETUNREACH (65 / 51 on macOS, 113 / 101 on Linux); Windows' WSAE* codes come as the kinds.
fn unreached_of(error: &std::io::Error) -> Unreached {
    use std::io::ErrorKind;
    match error.kind() {
        ErrorKind::ConnectionRefused => Unreached::Refused,
        ErrorKind::TimedOut => Unreached::Timeout,
        ErrorKind::HostUnreachable | ErrorKind::NetworkUnreachable | ErrorKind::PermissionDenied => Unreached::NoRoute,
        _ if matches!(error.raw_os_error(), Some(1 | 13 | 51 | 65 | 101 | 113)) => Unreached::NoRoute,
        _ => Unreached::Other,
    }
}

/// Why no SSH banner came (test 21.11 on Windows: the wizard said «SSH нет» while `Test-NetConnection` reached port
/// 2222 — one answer for every failure, and nothing in the log said which).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoBanner {
    /// The connect itself failed — with the system's own words.
    Unreached(Unreached, String),
    /// Connected, but no SSH line came in time.
    Silent,
    /// Connected, but the other side closed, or said something that is not SSH — what it said, if anything.
    NotSsh(String),
}

impl std::fmt::Display for NoBanner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NoBanner::Unreached(kind, said) => write!(f, "the connect failed ({kind:?}): {said}"),
            NoBanner::Silent => write!(f, "the port is open, but no SSH line came in time"),
            NoBanner::NotSsh(said) if said.is_empty() => write!(f, "the port is open, but it closed without a word"),
            NoBanner::NotSsh(said) => write!(f, "the port is open, but it answered not SSH: {said:?}"),
        }
    }
}

/// The SSH banner's software (`dropbear_2022.83`) at address:port, or why there is none: the connect may take
/// `connect_ms`, the banner `banner_ms` more — a busy station's dropbear answers after its fork.
pub async fn probe_ssh(address: &str, port: u16, connect_ms: u64, banner_ms: u64) -> Result<String, NoBanner> {
    let mut stream = match timeout(Duration::from_millis(connect_ms), crate::utils::net::dial(address, port)).await {
        Err(_) => return Err(NoBanner::Unreached(Unreached::Timeout, format!("no answer in {connect_ms} ms"))),
        Ok(Err(error)) => return Err(NoBanner::Unreached(unreached_of(&error), error.to_string())),
        Ok(Ok(stream)) => stream,
    };
    // Latin-1: every byte is one character, as Node's 'latin1' read it.
    let mut text = String::new();
    let mut buffer = [0u8; 256];
    let deadline = Instant::now() + Duration::from_millis(banner_ms);
    let said = |text: &str| text.chars().take(80).collect::<String>();
    loop {
        let size = match timeout_at(deadline, stream.read(&mut buffer)).await {
            Err(_) if text.is_empty() => return Err(NoBanner::Silent),
            Err(_) => return Err(NoBanner::NotSsh(said(&text))),
            Ok(Err(error)) => return Err(NoBanner::NotSsh(format!("{}{error}", said(&text)))),
            Ok(Ok(size)) => size,
        };
        if size == 0 {
            return banner_software(&text, true).ok_or_else(|| NoBanner::NotSsh(said(&text)));
        }
        text.extend(buffer[..size].iter().map(|byte| char::from(*byte)));
        let complete = text.contains('\n') || text.chars().count() > 255;
        if let Some(software) = banner_software(&text, complete) {
            return Ok(software);
        }
        if complete {
            return Err(NoBanner::NotSsh(said(&text)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    /// The search drops this gateway's own announcement by its id, whatever name it was said in and at whichever
    /// address it came; another gateway on this very computer (a pretend station, a second DuoWave) is a home to find;
    /// an answer without an id is this gateway only at an address of this computer under its name.
    #[test]
    fn the_search_knows_its_own_announcement_by_id_whatever_its_name() {
        let found = |address: &str, txt: &[(&str, &str)]| FoundStation {
            address: address.to_string(),
            name: None,
            txt: txt.iter().map(|(key, value)| (key.to_string(), value.to_string())).collect(),
            ssh: None,
        };
        let own = vec!["192.0.2.10".to_string()];
        let (id, name) = ("gateway-self-id", "Компьютер Mac");
        let mine = |home: &FoundStation| is_this_gateway(home, &own, id, name);
        assert!(mine(&found("192.0.2.10", &[("id", id), ("name", "Mac computer")])));
        assert!(mine(&found("100.64.0.7", &[("id", id), ("name", "Mac computer")])));
        assert!(!mine(&found("192.0.2.10", &[("id", "another-gateway"), ("name", name)])));
        assert!(mine(&found("192.0.2.10", &[("name", name)])));
        assert!(!mine(&found("192.0.2.10", &[("name", "Mac computer")])));
        assert!(!mine(&found("192.0.2.20", &[("name", name)])));
        // Without a record of its own yet this gateway has no id, and an answer with one is never it.
        assert!(!is_this_gateway(&found("192.0.2.10", &[("id", id), ("name", name)]), &own, "", name));
    }

    #[test]
    fn a_connect_error_says_why_nothing_answered() {
        use std::io::{Error, ErrorKind};
        assert_eq!(unreached_of(&Error::from(ErrorKind::ConnectionRefused)), Unreached::Refused);
        assert_eq!(unreached_of(&Error::from(ErrorKind::HostUnreachable)), Unreached::NoRoute);
        // macOS's «No route to host» as the OS gives it, and a program kept off the local network.
        assert_eq!(unreached_of(&Error::from_raw_os_error(65)), Unreached::NoRoute);
        assert_eq!(unreached_of(&Error::from(ErrorKind::PermissionDenied)), Unreached::NoRoute);
        assert_eq!(unreached_of(&Error::other("odd")), Unreached::Other);
    }

    /// A DNS name in wire format, without compression.
    fn name(text: &str) -> Vec<u8> {
        let mut bytes = Vec::new();
        for label in text.split('.') {
            bytes.push(label.len() as u8);
            bytes.extend_from_slice(label.as_bytes());
        }
        bytes.push(0);
        bytes
    }

    fn record(owner: &[u8], kind: u16, data: &[u8]) -> Vec<u8> {
        let mut bytes = owner.to_vec();
        bytes.extend_from_slice(&kind.to_be_bytes());
        bytes.extend_from_slice(&1u16.to_be_bytes());
        bytes.extend_from_slice(&120u32.to_be_bytes());
        bytes.extend_from_slice(&(data.len() as u16).to_be_bytes());
        bytes.extend_from_slice(data);
        bytes
    }

    /// A made-up station's answer: PTR to its instance, then TXT and SRV for the instance (compressed to the PTR's
    /// target).
    fn answer() -> Vec<u8> {
        let mut header = vec![0u8; 12];
        header[2..4].copy_from_slice(&0x8400u16.to_be_bytes());
        header[6..8].copy_from_slice(&1u16.to_be_bytes()); // one answer
        header[10..12].copy_from_slice(&2u16.to_be_bytes()); // two additional
        let service = name(YANDEX_SERVICE);
        let instance = name(&format!("Test-Station.{YANDEX_SERVICE}"));
        let ptr = record(&service, 12, &instance);
        // The instance name sits right after the PTR's fixed part: 12 + service + 10.
        let pointer = [0xc0, (12 + service.len() + 10) as u8];
        let mut txt = vec![13];
        txt.extend_from_slice(b"platform=test");
        txt.push(9);
        txt.extend_from_slice(b"cluster=x");
        let mut srv = vec![0, 0, 0, 0, 0x07, 0xa9];
        srv.extend_from_slice(&name("test-station.local"));
        [header, ptr, record(&pointer, 16, &txt), record(&pointer, 33, &srv)].concat()
    }

    fn entries(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(key, value)| (key.to_string(), value.to_string())).collect()
    }

    #[test]
    fn the_firmwares_own_instance_is_no_name() {
        let found = |instance: &str| {
            let target = format!("{instance}.{YANDEX_SERVICE}");
            let records = [DnsRecord::Ptr { name: YANDEX_SERVICE.into(), target }];
            stations_in(&records, "192.168.1.101", YANDEX_SERVICE).unwrap().name
        };
        assert_eq!(found("YandexIOReceiver-FF98F0293ACA2B21"), None);
        assert_eq!(found("yandexioreceiver-ff98"), None);
        assert_eq!(found("Кухня").as_deref(), Some("Кухня"));
        assert_eq!(found("YandexIO"), Some("YandexIO".into()), "a short one is someone's");
    }

    #[test]
    fn asks_one_question_ptr_yandexio_tcp_local_class_in() {
        let query = mdns_query(YANDEX_SERVICE);
        assert_eq!(u16_at(&query, 4), Some(1));
        let service = name(YANDEX_SERVICE);
        assert_eq!(&query[12..12 + service.len()], &service[..]);
        assert_eq!(u16_at(&query, query.len() - 4), Some(12));
        assert_eq!(u16_at(&query, query.len() - 2), Some(1));
    }

    #[test]
    fn reads_a_station_from_its_answer_compressed_names_and_all() {
        let records = parse_records(&answer());
        let instance = format!("Test-Station.{YANDEX_SERVICE}");
        assert_eq!(
            records,
            vec![
                DnsRecord::Ptr { name: YANDEX_SERVICE.into(), target: instance.clone() },
                DnsRecord::Txt { name: instance.clone(), entries: entries(&[("platform", "test"), ("cluster", "x")]) },
                DnsRecord::Srv { name: instance, port: 1961, target: "test-station.local".into() },
            ]
        );
        assert_eq!(
            stations_in(&records, "192.0.2.23", YANDEX_SERVICE),
            Some(FoundStation {
                address: "192.0.2.23".into(),
                name: Some("Test-Station".into()),
                txt: entries(&[("platform", "test"), ("cluster", "x")]),
                ssh: None,
            })
        );
    }

    #[test]
    fn ignores_answers_about_other_services_and_survives_broken_packets() {
        assert_eq!(stations_in(&parse_records(&answer()), "192.0.2.23", "_other._tcp.local"), None);
        assert_eq!(parse_records(&[1, 2, 3]), vec![]);
        let looped = [&[0u8, 0, 0x84, 0, 0, 0, 0, 1, 0, 0, 0, 0][..], &[0xc0, 12]].concat();
        assert_eq!(parse_records(&looped), vec![]);
        assert!(parse_records(&answer()[..60]).len() < 3);
    }

    /// Not a TS test: the query and the answers over real sockets, the station a pretend one on loopback — never
    /// the network's mDNS.
    #[tokio::test]
    async fn hears_a_station_that_answers_the_query() {
        let station = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let at = station.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buffer = [0u8; 512];
            if let Ok((size, from)) = station.recv_from(&mut buffer).await {
                if buffer[..size] == mdns_query(YANDEX_SERVICE)[..] {
                    let _ = station.send_to(&answer(), from).await;
                    let _ = station.send_to(&answer(), from).await;
                }
            }
        });
        let found = ask(&[at], false, Duration::from_millis(300)).await;
        assert_eq!(found.len(), 1, "one entry per address: {found:?}");
        assert_eq!(found[0].address, "127.0.0.1");
        assert_eq!(found[0].name.as_deref(), Some("Test-Station"));
    }

    async fn serve_once(reply: &'static str) -> (u16, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let _ = socket.write_all(reply.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
        });
        (port, task)
    }

    #[tokio::test]
    async fn names_the_ssh_software_that_answers() {
        let (port, server) = serve_once("SSH-2.0-dropbear_2022.83\r\n").await;
        assert_eq!(probe_ssh("127.0.0.1", port, 1500, 1500).await.as_deref(), Ok("dropbear_2022.83"));
        server.abort();
    }

    #[tokio::test]
    async fn says_why_nothing_ssh_answers() {
        let (port, server) = serve_once("HTTP/1.1 400 Bad Request\r\n\r\n").await;
        let answer = probe_ssh("127.0.0.1", port, 1500, 1500).await;
        assert!(matches!(&answer, Err(NoBanner::NotSsh(said)) if said.starts_with("HTTP/1.1 400")), "{answer:?}");
        server.abort();
        let _ = server.await;
        // The wizard's own wait (station_shell.rs, CONNECT_MS): Windows says «refused» only after about two seconds of
        // its own retries; elsewhere it comes at once.
        let Err(NoBanner::Unreached(kind, _)) = probe_ssh("127.0.0.1", port, 3_000, 500).await else {
            panic!("a closed port is unreached");
        };
        assert_eq!(kind, Unreached::Refused);
    }

    #[tokio::test]
    async fn a_port_that_closes_at_once_is_not_ssh() {
        let (port, server) = serve_once("").await;
        assert_eq!(probe_ssh("127.0.0.1", port, 1500, 1500).await, Err(NoBanner::NotSsh(String::new())));
        server.abort();
    }

    #[tokio::test]
    async fn a_port_that_says_nothing_is_silent() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let held = tokio::spawn(async move {
            let (_socket, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
        assert_eq!(probe_ssh("127.0.0.1", port, 1500, 300).await, Err(NoBanner::Silent));
        held.abort();
    }

    #[test]
    fn waits_for_the_banner_line_to_end() {
        assert_eq!(banner_software("SSH-2.0-drop", false), None, "more may be coming");
        assert_eq!(banner_software("SSH-2.0-dropbear_2022.83\r", false).as_deref(), Some("dropbear_2022.83"));
        assert_eq!(banner_software("SSH-2.0-OpenSSH_9.6 Ubuntu", false).as_deref(), Some("OpenSSH_9.6"));
        assert_eq!(banner_software("SSH--x\r\n", true), None);
        assert_eq!(banner_software("HTTP/1.1 400", true), None);
    }
}
