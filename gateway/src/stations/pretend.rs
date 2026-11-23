//! Pretend stations (docs/plans/pretend-stations.md): this computer's build in the station's role, several side by side
//! on one Mac, each on an address of its own on the loopback (10.77.0.x) — and the test Turbo beside them. What a real
//! station has around its gateway is played here:
//!
//! - it answers the station search: a question about `_yandexio._tcp` sent straight to its address (port 5353) gets
//!   the answer a station gives — the search of a pretend Turbo asks those addresses only (`search_targets`), never
//!   the home network: the owner's real station is never asked, and the Mac's live DuoWave never sees the pretend ones;
//! - its SSH greets as the station's dropbear does (port 2222) and lets nobody in: a pretend station has DuoWave
//!   already, it is never installed over SSH;
//! - its voice is `duowave pretend` (src/tools/pretend.rs): a pretend quasar on its voice socket and a pretend Yandex;
//! - what it would say aloud — a join code — goes to its terminal (and to the Mac's `say` with DUOWAVE_PRETEND_SAY);
//! - its connections to the other pretend addresses leave from its own (src/utils/net.rs).
//!
//! Nothing of it runs without DUOWAVE_PRETEND.

use std::collections::BTreeMap;
use std::net::{SocketAddr, SocketAddrV4};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};

use super::connect::discovery::{self, DnsRecord, MDNS_PORT, YANDEX_SERVICE};
use crate::app::App;
use crate::config::PretendConfig;

/// The station's SSH port (the wiki's dropbear) and its greeting.
pub const SSH_PORT: u16 = 2222;
pub const SSH_GREETING: &str = "SSH-2.0-dropbear_2022.83\r\n";
/// What a pretend station's terminal line starts with when it «says» something: `duowave pretend` shows these.
pub const SAYS: &str = "🔊 ";
/// The model its mDNS answer gives (`platform`): the panel shows it.
pub const MODEL: &str = "pretend";

static PRETEND: OnceLock<PretendConfig> = OnceLock::new();

/// This process is a pretend station or the Turbo beside them: its search and its connections from now on.
pub fn install(config: &PretendConfig) {
    let _ = PRETEND.set(config.clone());
    crate::utils::net::leave_from(config.address);
}

pub fn get() -> Option<&'static PretendConfig> {
    PRETEND.get()
}

/// Where the station search asks, in a pretend gateway: the pretend stations' own addresses, never the home network
/// (none listed — nobody). None: an ordinary gateway, the home network as always.
pub fn search_targets() -> Option<Vec<SocketAddr>> {
    get().map(targets)
}

fn targets(pretend: &PretendConfig) -> Vec<SocketAddr> {
    pretend
        .stations
        .iter()
        .filter(|station| **station != pretend.address)
        .map(|station| SocketAddr::V4(SocketAddrV4::new(*station, MDNS_PORT)))
        .collect()
}

/// A pretend station's answer about `_yandexio._tcp`, as a real one's reads: its instance, TXT (`platform` the model,
/// `name`, `deviceId`), SRV and A — its own address.
pub fn records(pretend: &PretendConfig, device: &str) -> Vec<DnsRecord> {
    let id: String = device.chars().filter(char::is_ascii_alphanumeric).take(12).collect();
    // DNS-SD instance names are UTF-8 (RFC 6763, 4.1.1): the panel shows «Kitchen» (or «Кухня») as a real station's own name.
    let named: String = pretend.name.chars().filter(|c| *c != '.').collect();
    let instance = if named.is_empty() || named.len() > 63 { format!("pretend-{id}") } else { named };
    let target = format!("{instance}.{YANDEX_SERVICE}");
    let host = format!("pretend-{id}.local");
    let mut txt = BTreeMap::new();
    txt.insert("platform".to_string(), MODEL.to_string());
    txt.insert("name".to_string(), pretend.name.clone());
    txt.insert("deviceId".to_string(), id);
    vec![
        DnsRecord::Ptr { name: YANDEX_SERVICE.to_string(), target: target.clone() },
        DnsRecord::Txt { name: target.clone(), entries: txt },
        DnsRecord::Srv { name: target, target: host.clone(), port: 1961 },
        DnsRecord::A { name: host, address: pretend.address.to_string() },
    ]
}

/// The pretend parts beside the gateway: the entry its HTTP calls to the other pretend addresses take; on a station
/// also its mDNS answer and its SSH greeting. A part that cannot start says so in the log; the gateway runs on.
pub async fn start(app: Arc<App>, pretend: &PretendConfig) {
    let decide: crate::egress::app_entry::Decide = Arc::new(|_, _, _| crate::egress::app_entry::Hop::Direct);
    let failed: crate::egress::app_entry::Failed = Arc::new(|_, host, why| {
        tracing::info!(target: "pretend", "{host} did not answer ({why})");
    });
    match crate::egress::app_entry::start(decide, failed).await {
        Ok(port) => crate::utils::net::set_entry(port),
        Err(error) => tracing::warn!(target: "pretend", "no entry for the pretend addresses: {error}"),
    }
    if !pretend.is_station() {
        tracing::warn!(
            target: "pretend",
            "a pretend Turbo at {}: the station search asks {:?} only",
            pretend.address,
            pretend.stations
        );
        return;
    }
    // Its own name as the household would have given it at the install («Kitchen»), unless a person renamed it since.
    if app.identity.view().is_ok_and(|view| !view.named) {
        if let Err(error) = app.identity.rename(Some(&pretend.name), None) {
            tracing::info!(target: "pretend", "the station's name stays the default: {}", error.message);
        }
    }
    let device = app.identity.as_helper().id;
    let answer = discovery::mdns_answer(&records(pretend, &device), 120);
    match mdns_socket(pretend.address) {
        Ok(socket) => {
            tokio::spawn(answer_questions(socket, answer));
        }
        Err(error) => tracing::warn!(
            target: "pretend",
            "{}:{MDNS_PORT} is not free ({error}): the search will not find this station — is the address added (scripts/pretend/addresses.sh)?",
            pretend.address
        ),
    }
    match TcpListener::bind((pretend.address, SSH_PORT)).await {
        Ok(listener) => {
            tokio::spawn(greet(listener));
        }
        Err(error) => tracing::warn!(target: "pretend", "{}:{SSH_PORT}: no SSH greeting ({error})", pretend.address),
    }
    tracing::warn!(target: "pretend", "a pretend station «{}» at {}", pretend.name, pretend.address);
}

/// UDP 5353 on this address, shared as the system's responder and the browsers share `*:5353` (SO_REUSEADDR and
/// SO_REUSEPORT): without the sharing the port is «taken» on a Mac. A question to this address comes here, the most
/// specific of them.
fn mdns_socket(address: std::net::Ipv4Addr) -> std::io::Result<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    #[cfg(all(unix, not(any(target_os = "solaris", target_os = "illumos", target_os = "cygwin"))))]
    socket.set_reuse_port(true)?;
    socket.bind(&SocketAddr::V4(SocketAddrV4::new(address, MDNS_PORT)).into())?;
    socket.set_nonblocking(true)?;
    UdpSocket::from_std(socket.into())
}

/// Each question about `_yandexio._tcp` straight to this address gets the station's answer, back to the asker's port.
async fn answer_questions(socket: UdpSocket, answer: Vec<u8>) {
    let mut buffer = vec![0u8; 9000];
    while let Ok((size, from)) = socket.recv_from(&mut buffer).await {
        if discovery::asks_for(&buffer[..size], YANDEX_SERVICE) {
            let _ = socket.send_to(&answer, from).await;
        }
    }
}

/// The station's dropbear says who it is, then lets nobody in.
async fn greet(listener: TcpListener) {
    while let Ok((mut stream, _)) = listener.accept().await {
        tokio::spawn(async move {
            let _ = stream.write_all(SSH_GREETING.as_bytes()).await;
            let mut sink = [0u8; 256];
            let _ = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut sink)).await;
        });
    }
}

/// What a pretend station «says» aloud: a line in its terminal (`duowave pretend` shows it), and the Mac's voice too
/// with DUOWAVE_PRETEND_SAY.
pub fn say(pretend: &PretendConfig, text: &str) {
    println!("{SAYS}{}: {text}", pretend.name);
    if pretend.say {
        let text = text.to_string();
        tokio::spawn(async move {
            let _ = tokio::process::Command::new("say").arg(text).status().await;
        });
    }
}

/// «Это ненастоящая колонка»: what would change the station's own files or reboot it.
pub fn refuse() -> crate::platform::http::ApiError {
    crate::platform::http::ApiError::conflict(crate::tr!(
        "Это ненастоящая колонка: файлов прошивки и перезагрузки у неё нет."
    ))
}

/// Whether this gateway is a pretend station.
pub fn is_station() -> bool {
    get().is_some_and(PretendConfig::is_station)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    use crate::config::PretendKind;

    fn kitchen() -> PretendConfig {
        PretendConfig {
            kind: PretendKind::Station,
            address: Ipv4Addr::new(127, 0, 0, 1),
            name: "Кухня".into(),
            stations: vec![Ipv4Addr::new(127, 0, 0, 1), Ipv4Addr::new(10, 77, 0, 3)],
            say: false,
        }
    }

    #[test]
    fn the_search_asks_the_other_pretend_stations_only() {
        assert_eq!(targets(&kitchen()), vec!["10.77.0.3:5353".parse::<SocketAddr>().unwrap()]);
    }

    /// The station search of src/stations/connect/discovery.rs reads the answer as a real station's.
    #[tokio::test]
    async fn the_search_hears_a_pretend_station_at_its_address() {
        let station = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let at = station.local_addr().unwrap();
        let answer = discovery::mdns_answer(&records(&kitchen(), "f00d-1234"), 120);
        tokio::spawn(answer_questions(station, answer));
        let found = discovery::ask_at(&[at], Duration::from_millis(800)).await;
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].address, "127.0.0.1");
        assert_eq!(found[0].txt.get("platform").map(String::as_str), Some(MODEL));
        assert_eq!(found[0].txt.get("name").map(String::as_str), Some("Кухня"));
        assert_eq!(found[0].name.as_deref(), Some("Кухня"), "its own name, as the panel shows it");
    }

    #[tokio::test]
    async fn its_ssh_greets_as_the_stations_dropbear() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(greet(listener));
        let banner = discovery::probe_ssh("127.0.0.1", port, 1500, 1500).await;
        assert_eq!(banner.as_deref(), Ok("dropbear_2022.83"));
    }
}
