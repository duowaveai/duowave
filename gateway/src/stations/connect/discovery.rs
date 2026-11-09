//! Finding stations on the home network: they announce themselves over mDNS as _yandexio._tcp (glagol, the local
//! control the Yandex app uses). A query from a random port gets unicast answers (RFC 6762, legacy unicast); one from
//! 5353, where the OS lets us share it, gets the multicast ones too. Then each answer's SSH port is asked for its
//! banner — reading only. No package: a PTR query and the answer's records are a page of code.

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tokio::io::AsyncReadExt;
use tokio::net::{TcpStream, UdpSocket};
use tokio::time::{timeout, timeout_at, Instant};

pub const YANDEX_SERVICE: &str = "_yandexio._tcp.local";
const MDNS_GROUP: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
const MDNS_PORT: u16 = 5353;

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
        name: (!label.is_empty()).then(|| label.to_string()),
        txt: txt.unwrap_or_default(),
        ssh: None,
    })
}

/// Asks the home network for stations for `ms` and returns what answered, one entry per address.
pub async fn discover_stations(ms: u64) -> Vec<FoundStation> {
    ask(&[SocketAddr::V4(SocketAddrV4::new(MDNS_GROUP, MDNS_PORT))], true, Duration::from_millis(ms)).await
}

/// The query sent to each of `to` (the mDNS group; a pretend station in tests), and the answers heard for `window`.
async fn ask(to: &[SocketAddr], shared: bool, window: Duration) -> Vec<FoundStation> {
    let query = mdns_query(YANDEX_SERVICE);
    let mut sockets = Vec::new();
    // Unicast answers to a random port.
    if let Ok(unicast) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await {
        for target in to {
            let _ = unicast.send_to(&query, target).await;
        }
        sockets.push(unicast);
    }
    // Multicast ones on 5353, beside the OS's own responder where it allows; nothing lost if it does not.
    if shared {
        if let Some(socket) = shared_socket() {
            // No multicast here (a container's bridge): the unicast answers are all there is.
            if socket.join_multicast_v4(MDNS_GROUP, Ipv4Addr::UNSPECIFIED).is_ok() {
                for target in to {
                    let _ = socket.send_to(&query, target).await;
                }
            }
            sockets.push(socket);
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
                    let Some(station) = stations_in(&parse_records(&buffer[..size]), &address, YANDEX_SERVICE) else {
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
fn shared_socket() -> Option<UdpSocket> {
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

/// The SSH banner's software (`dropbear_2022.83`) at address:port, or None when nothing SSH answers in `ms`.
pub async fn ssh_banner(address: &str, port: u16, ms: u64) -> Option<String> {
    let idle = Duration::from_millis(ms);
    let mut stream = timeout(idle, TcpStream::connect((address, port))).await.ok()?.ok()?;
    // Latin-1: every byte is one character, as Node's 'latin1' read it.
    let mut text = String::new();
    let mut buffer = [0u8; 256];
    loop {
        let size = timeout(idle, stream.read(&mut buffer)).await.ok()?.ok()?;
        if size == 0 {
            return banner_software(&text, true);
        }
        text.extend(buffer[..size].iter().map(|byte| char::from(*byte)));
        let complete = text.contains('\n') || text.chars().count() > 255;
        if let Some(software) = banner_software(&text, complete) {
            return Some(software);
        }
        if complete {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

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
        assert_eq!(ssh_banner("127.0.0.1", port, 1500).await.as_deref(), Some("dropbear_2022.83"));
        server.abort();
    }

    #[tokio::test]
    async fn is_null_where_nothing_ssh_answers() {
        let (port, server) = serve_once("HTTP/1.1 400 Bad Request\r\n\r\n").await;
        assert_eq!(ssh_banner("127.0.0.1", port, 1500).await, None);
        server.abort();
        let _ = server.await;
        assert_eq!(ssh_banner("127.0.0.1", port, 500).await, None);
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
