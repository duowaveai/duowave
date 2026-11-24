//! Connecting a station from the panel: the gateway finds it, reads it (with the admin's consent, changing
//! nothing), shows what it would change, and on «Подключить» does it — the old addresses kept in the gateway, voice.sh
//! and restore.sh put on the station, its voice pointed through our copy of quasar.cfg mounted over the firmware's own
//! (station_scripts.rs: the firmware's file is never changed), the result checked, the station rebooted — then waits
//! for it to come to /uni.ws. A station that has not come in 3 minutes is offered its way back. Nothing on a station
//! changes but by the admin's press, and every change goes to «Кто что менял».
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tokio::sync::broadcast;
use tokio::time::{timeout_at, Instant};

use super::discovery::{discover_stations, FoundStation, NoBanner, Unreached};
use super::neighbours::{self, Neighbour};
use super::quasar_config::{
    gateway_addresses, parse_gateway_host, points_to, read_config, with_addresses, yandex, GatewayHost, PointsTo,
    StationFacts, VoiceAddresses,
};
use super::station_scripts::{copy_state, deliveries, switch_to, uninstall_script, CopyState, STEPS};
use super::station_shell::{HostKeyChanged, ShellError, ShellOpener, ShellOptions, StationShell};
use crate::core::events::{Actor, EventBus, GatewayEvent};
use crate::operations::networks::in_networks;
use crate::platform::http::ApiError;
use crate::settings::definitions::{LinkAddresses, StationLink, STATION_LINKS};
use crate::settings::SettingsService;
use crate::stations::service::{StationView, StationsService};
use crate::utils::time::now_iso;

/// What stations/presence lets the connect flow ask: a station's record, the device at an address, and marking an
/// address invited. The real StationsService; a fake in tests.
pub trait Stations: Send + Sync {
    fn find(&self, device: &str) -> Option<StationView>;
    /// The station (any status) whose last address is this one.
    fn device_for_address(&self, address: &str) -> Option<String>;
    fn expect(&self, address: &str, ms: u64);
}

impl Stations for StationsService {
    fn find(&self, device: &str) -> Option<StationView> {
        StationsService::find(self, device)
    }
    fn device_for_address(&self, address: &str) -> Option<String> {
        StationsService::device_for_address(self, address)
    }
    fn expect(&self, address: &str, ms: u64) {
        StationsService::expect(self, address, ms)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ConnectTiming {
    /// How long a rebooted station has to come to /uni.ws.
    pub arrival_ms: u64,
    /// How long its first connection is trusted without the admin's approval.
    pub invitation_ms: u64,
}

impl Default for ConnectTiming {
    fn default() -> ConnectTiming {
        ConnectTiming { arrival_ms: 3 * 60_000, invitation_ms: 5 * 60_000 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StationCheck {
    pub address: String,
    /// The SSH software that answered, or null: no SSH — root and SSH by the wiki first.
    pub ssh: Option<String>,
    /// The host key, learned on this first look.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub facts: Option<StationFacts>,
    /// Free space in /data, kilobytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_free_kb: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
    /// Our call client is in /data/opt/duowave.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client: Option<bool>,
    /// Yandex's updates: off as the wiki turns them off, or still on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updates: Option<String>,
    /// Our copy of the config on the station: mounted, kept but not mounted at the last boot (and why: the firmware's
    /// file changed under it — an update), or none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copy: Option<CopyState>,
    /// What else on the station stands in the way, the worst first — named, never removed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub neighbours: Option<Vec<Neighbour>>,
    /// The memory the station has free, kilobytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mem_available_kb: Option<u64>,
    /// The station this gateway knows at that address.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// What «Подключить» would write, for the gateway the panel named.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<VoiceAddresses>,
    /// It already dials this gateway: nothing to change.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connected: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StepId {
    Copy,
    /// uninstall.sh: DuoWave off the station.
    Remove,
    Write,
    Verify,
    Reboot,
    Arrive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StepState {
    Pending,
    Running,
    Done,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JobKind {
    Connect,
    Restore,
    /// «Удалить DuoWave с колонки».
    Uninstall,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Running,
    Done,
    Failed,
    Timeout,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Step {
    pub id: StepId,
    pub state: StepState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectJob {
    pub id: String,
    pub kind: JobKind,
    pub address: String,
    pub status: JobStatus,
    pub steps: Vec<Step>,
    /// The station's copy of its config, in /data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copy: Option<String>,
    /// The station that came, by its device id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub started_at: String,
}

/// Where a restore goes back to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreTo {
    Previous,
    Yandex,
}

pub struct StationConnectService {
    settings: Arc<SettingsService>,
    stations: Arc<dyn Stations>,
    events: EventBus,
    opener: Arc<dyn ShellOpener>,
    ssh: ShellConfig,
    path: String,
    port: u16,
    timing: ConnectTiming,
    jobs: Mutex<HashMap<String, ConnectJob>>,
}

#[derive(Debug, Clone)]
pub struct ShellConfig {
    pub port: u16,
    pub user: String,
    /// STATION_SSH_PASSWORD: every station's, unless the wizard was given one of its own.
    pub password: String,
    pub passwords: StationPasswords,
}

impl ShellConfig {
    /// The root password for the station at `address`: the one typed in the wizard for it, else the common one.
    pub fn password_for(&self, address: &str) -> String {
        self.passwords.get(address).unwrap_or_else(|| self.password.clone())
    }
}

/// The secrets' id of a station's root password typed in the wizard: `station-ssh:<address>`.
pub const STATION_SSH_SECRET: &str = "station-ssh:";

/// Stations' own root passwords, typed in the wizard when the empty one was refused (Windows test 21.11: a station
/// with a root password could be connected only through STATION_SSH_PASSWORD in `.env`). Sealed among the gateway's
/// secrets by the station's address; never in a log, an event or an answer. None without secrets (tests).
#[derive(Clone, Default)]
pub struct StationPasswords(Option<Arc<crate::secrets::service::SecretsService>>);

impl StationPasswords {
    pub fn new(secrets: Arc<crate::secrets::service::SecretsService>) -> StationPasswords {
        StationPasswords(Some(secrets))
    }

    pub fn get(&self, address: &str) -> Option<String> {
        self.0.as_ref()?.get(&format!("{STATION_SSH_SECRET}{address}")).filter(|password| !password.is_empty())
    }

    /// Keeps it; None or empty forgets it.
    pub fn set(&self, address: &str, password: Option<&str>) -> Result<(), ApiError> {
        let Some(secrets) = &self.0 else {
            return Err(ApiError::conflict(crate::tr!("Пароль колонки здесь негде сохранить.")));
        };
        secrets.set(&format!("{STATION_SSH_SECRET}{address}"), password)
    }
}

impl std::fmt::Debug for StationPasswords {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StationPasswords")
    }
}

/// An interface for homeAddresses, as a station would read it.
#[derive(Debug, Clone, Default)]
pub struct Iface {
    pub ipv4: bool,
    pub internal: bool,
    pub address: String,
    /// The adapter's name as the system calls it: «Wi-Fi», «Ethernet», «vEthernet (WSL)».
    pub name: String,
    /// A point-to-point adapter — a VPN's tunnel (utun, tun, wg) — whatever its name.
    pub tunnel: bool,
}

/// An adapter that is not the home network itself: WSL and Hyper-V's switches, Docker's, VirtualBox, VMware and
/// Parallels' host-only ones, a VPN's tunnel (Windows test 21.11: a PC with WSL — the station must be given the Wi-Fi's
/// address).
pub fn virtual_adapter(name: &str) -> bool {
    let name = name.to_lowercase();
    [
        "vethernet",
        "wsl",
        "hyper-v",
        "docker",
        "virtualbox",
        "vboxnet",
        "vmware",
        "vmnet",
        "vnic",
        "tap",
        "tun",
        "wintun",
        "wireguard",
        "openvpn",
        "zerotier",
        "tailscale",
        "utun",
        "bridge",
        "loopback",
        "npcap",
    ]
    .iter()
    .any(|word| name.contains(word))
}

/// This machine's addresses in the home networks as another machine there dials them, the likeliest first: a real
/// adapter before a virtual one or a tunnel, then 192.168.x before 10.x before 172.16–31.x — WSL, Hyper-V and Docker
/// take 172.x by default, VPNs often 10.x. Nothing on the loopback adapter, whatever the address: an alias there (the
/// pretend stations' 10.77.0.x on lo0) is this machine's alone — the key sheet once offered it first (step 11 check).
pub fn home_ranked(interfaces: &[Iface]) -> Vec<&Iface> {
    let home = ["10.0.0.0/8".to_string(), "192.168.0.0/16".to_string(), "172.16.0.0/12".to_string()];
    // The loopback adapter is the one that has 127.0.0.1 or ::1: lo0, lo, «Loopback Pseudo-Interface 1».
    let loopback: Vec<&str> = interfaces
        .iter()
        .filter(|entry| entry.internal && !entry.name.is_empty())
        .map(|entry| entry.name.as_str())
        .collect();
    let mut found: Vec<&Iface> = interfaces
        .iter()
        .filter(|entry| entry.ipv4 && !entry.internal && !loopback.contains(&entry.name.as_str()))
        .filter(|entry| in_networks(&entry.address, &home))
        .collect();
    let rank = |entry: &Iface| {
        let range = if entry.address.starts_with("192.168.") {
            0
        } else if entry.address.starts_with("10.") {
            1
        } else {
            2
        };
        (entry.tunnel || virtual_adapter(&entry.name), range)
    };
    found.sort_by_key(|entry| rank(entry));
    found
}

/// A station that still dials this computer's old port: its address, the station, what it dials and where this
/// gateway is now — the panel offers «Переподключить» with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MovedGateway {
    pub address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    pub was: String,
    pub now: String,
}

/// This machine's network interfaces, as the home-address wizard reads them.
pub fn host_interfaces() -> Vec<Iface> {
    if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .map(|iface| Iface {
            ipv4: matches!(iface.addr, if_addrs::IfAddr::V4(_)),
            internal: iface.is_loopback(),
            address: iface.ip().to_string(),
            name: iface.name.clone(),
            tunnel: iface.is_p2p(),
        })
        .collect()
}

/// Whether the gateway runs in a container: then its addresses are the bridge's, not the home's.
pub fn in_container() -> bool {
    std::path::Path::new("/.dockerenv").exists() || std::env::var_os("container").is_some()
}

impl StationConnectService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        settings: Arc<SettingsService>,
        stations: Arc<dyn Stations>,
        events: EventBus,
        opener: Arc<dyn ShellOpener>,
        ssh: ShellConfig,
        path: String,
        port: u16,
        timing: ConnectTiming,
    ) -> StationConnectService {
        StationConnectService {
            settings,
            stations,
            events,
            opener,
            ssh,
            path,
            port,
            timing,
            jobs: Mutex::new(HashMap::new()),
        }
    }

    fn options(&self, address: &str, trust: Option<String>) -> ShellOptions {
        ShellOptions {
            port: self.ssh.port,
            user: self.ssh.user.clone(),
            password: self.ssh.password_for(address),
            trust,
            timeout_ms: None,
        }
    }

    /// The stations that answer mDNS, with their SSH: reading only.
    pub async fn discover(&self) -> Vec<FoundStation> {
        self.with_banners(discover_stations(2000).await).await
    }

    /// Each station's SSH banner, all asked at once as the TS Promise.all did: a silent port costs its timeout once,
    /// not once per station.
    async fn with_banners(&self, found: Vec<FoundStation>) -> Vec<FoundStation> {
        let port = self.ssh.port;
        if found.is_empty() {
            tracing::info!("station search: no station answered mDNS");
        }
        let banners =
            futures::future::join_all(found.iter().map(|station| self.opener.banner(&station.address, port))).await;
        found
            .into_iter()
            .zip(banners)
            .map(|(station, banner)| {
                let ssh = banner
                    .inspect_err(|why| tracing::warn!("station search: {}:{port} has no SSH: {why}", station.address))
                    .ok();
                FoundStation { ssh, ..station }
            })
            .collect()
    }

    /// The port a station dials for this gateway: a typed address without one means this.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// This machine's addresses on the home network, as a station would dial them: what the wizard offers when the
    /// panel is opened from here (127.0.0.1 says nothing). None in a container: its addresses are the bridge's.
    pub fn home_addresses(&self, interfaces: &[Iface], in_container: bool) -> Vec<String> {
        if in_container {
            return Vec::new();
        }
        let port = if self.port == 80 { String::new() } else { format!(":{}", self.port) };
        // A pretend Turbo is at its own pretend address for the pretend stations (docs/plans/pretend-stations.md).
        if let Some(pretend) = super::super::pretend::get() {
            return vec![format!("{}{port}", pretend.address)];
        }
        // The home network first; the first is what the wizard offers.
        home_ranked(interfaces).iter().map(|entry| format!("{}{port}", entry.address)).collect()
    }

    /// Stations this gateway connected whose voice address names this computer on a port it no longer listens on: PORT
    /// changed since (another port taken on a busy computer), so the station dials nothing. Only addresses of this
    /// computer count — a station moved to another gateway, or onto itself, is none of this. None in a container:
    /// its own addresses say nothing about the host's.
    pub fn moved(&self, interfaces: &[Iface], in_container: bool) -> Vec<MovedGateway> {
        if in_container {
            return Vec::new();
        }
        let mine: Vec<String> = interfaces
            .iter()
            .filter(|entry| entry.ipv4 && !entry.internal)
            .map(|entry| entry.address.clone())
            .collect();
        let Ok(links) = self.links() else { return Vec::new() };
        links
            .into_iter()
            .filter_map(|(address, link)| {
                let url = link.gateway?;
                let authority = url.strip_prefix("ws://")?.split('/').next()?;
                let was = parse_gateway_host(authority)?;
                if was.port == self.port || !mine.contains(&was.host) {
                    return None;
                }
                let now = GatewayHost { host: was.host.clone(), port: self.port };
                Some(MovedGateway {
                    address,
                    device: link.device,
                    was: authority.to_string(),
                    now: if now.port == 80 { now.host } else { format!("{}:{}", now.host, now.port) },
                })
            })
            .collect()
    }

    /// The look with the station's own root password, typed in the wizard after «не пустила по SSH»: kept sealed
    /// for every later step only when the station takes it; a refused one is forgotten at once.
    pub async fn check_with_password(
        &self,
        address: &str,
        gateway: Option<&GatewayHost>,
        password: Option<&str>,
    ) -> Result<StationCheck, ApiError> {
        let Some(password) = password.filter(|password| !password.is_empty()) else {
            return self.check(address, gateway).await;
        };
        let before = self.ssh.passwords.get(address);
        self.ssh.passwords.set(address, Some(password))?;
        let check = self.check(address, gateway).await;
        let refused = check.as_ref().map_or(true, |check| check.problem.as_deref() == Some("auth-failed"));
        if refused {
            self.ssh.passwords.set(address, before.as_deref())?;
            tracing::warn!("station check: {address} refused the root password typed in the panel");
        } else {
            tracing::info!("station check: {address} took the root password typed in the panel; kept sealed");
        }
        check
    }

    /// The first look: SSH, the config, the free space — reading only, nothing on the station changes. A failed read
    /// of the links is the server's error, as the TS check rejected: taken for «no link» it would skip the key check.
    pub async fn check(&self, address: &str, gateway: Option<&GatewayHost>) -> Result<StationCheck, ApiError> {
        let device = self.stations.device_for_address(address);
        let port = self.ssh.port;
        // Why there is no banner, said in the log and to the panel (Windows test 21.11: «SSH нет» for every reason).
        // A port that is open but gave no banner still gets the real SSH client: the banner is a courtesy read, and
        // where it fails but SSH works the wizard should go on.
        let (ssh, missed) = match self.opener.banner(address, port).await {
            Ok(software) => (software, None),
            Err(why) => {
                tracing::warn!("station check: {address}:{port} gave no SSH banner: {why}");
                match why {
                    NoBanner::Unreached(kind, _) => {
                        return Ok(StationCheck {
                            problem: Some(unreached_problem(kind).into()),
                            ..base(address, None, device)
                        })
                    }
                    NoBanner::Silent => ("SSH".to_string(), Some("ssh-slow")),
                    NoBanner::NotSsh(_) => ("SSH".to_string(), Some("ssh-closed")),
                }
            }
        };
        let link = self.link(address)?;
        let shell = match self
            .opener
            .open(address, self.options(address, link.as_ref().map(|l| l.fingerprint.clone())))
            .await
        {
            Ok(shell) => shell,
            Err(error) => {
                tracing::warn!("station check: SSH to {address}:{port} did not open: {error:#}");
                let problem = match missed {
                    Some(missed) if !matches!(problem_of(&error).as_str(), "auth-failed" | "host-key-changed") => {
                        missed.to_string()
                    }
                    _ => problem_of(&error),
                };
                let ssh = if missed.is_some() { None } else { Some(ssh) };
                return Ok(StationCheck { problem: Some(problem), ..base(address, ssh, device) });
            }
        };
        let result = self.look(address, &ssh, device.clone(), gateway, link.as_ref(), shell.as_ref()).await;
        shell.close().await;
        Ok(match result {
            Ok(check) => check,
            Err(error) => StationCheck { problem: Some(problem_of(&error)), ..base(address, Some(ssh), device) },
        })
    }

    async fn look(
        &self,
        address: &str,
        ssh: &str,
        device: Option<String>,
        gateway: Option<&GatewayHost>,
        link: Option<&StationLink>,
        shell: &dyn StationShell,
    ) -> anyhow::Result<StationCheck> {
        let fingerprint = shell.fingerprint().to_string();
        let config = shell.run(&STEPS.read, None).await?;
        if config.code != 0 {
            return Ok(StationCheck {
                fingerprint: Some(fingerprint),
                problem: Some("no-config".into()),
                ..base(address, Some(ssh.to_string()), device)
            });
        }
        let facts = read_config(&config.stdout)?;
        let extra = shell.run(&STEPS.facts, None).await?;
        let lines: Vec<&str> = extra.stdout.trim().split('\n').collect();
        let data_free_kb = lines.first().and_then(|line| free_kb(line));
        let arch = lines.get(1).filter(|value| arch_ok(value)).map(|value| value.to_string());
        // A station that cannot say does not stop the look: the panel just has nothing to warn of.
        let (neighbours, mem_available_kb) = match shell.run(&STEPS.neighbours, None).await {
            Ok(around) if around.code == 0 => {
                let (mut found, memory) = neighbours::read(&around.stdout);
                // Signed in with AliceBridge's «1»: anyone at home can do the same.
                neighbours::note_password(&mut found, &self.ssh.password_for(address));
                (Some(found), memory)
            }
            _ => (None, None),
        };
        let target = gateway.map(|gateway| gateway_addresses(gateway, &self.path));
        if link.is_none() {
            self.remember(address, LinkChange { fingerprint: Some(fingerprint.clone()), ..Default::default() })?;
        }
        let connected = target.as_ref().map(|target| facts.addresses.url == target.url).unwrap_or(false);
        Ok(StationCheck {
            address: address.to_string(),
            ssh: Some(ssh.to_string()),
            fingerprint: Some(fingerprint),
            facts: Some(facts),
            data_free_kb,
            arch,
            client: Some(lines.get(2) == Some(&"client")),
            updates: match lines.get(3) {
                Some(&"updates-blocked") => Some("blocked".into()),
                Some(&"updates-on") => Some("on".into()),
                _ => None,
            },
            copy: copy_state(lines.get(4).copied(), lines.get(5).copied()),
            neighbours,
            mem_available_kb,
            device,
            target,
            connected: Some(connected),
            problem: None,
        })
    }

    /// «Выключить» another mod by its own switch (neighbours.rs): its folder, settings and records stay, only its
    /// switch is turned. Over the key the first look learned — never a station not looked at. The panel looks again
    /// after it; the change goes to «Кто что менял».
    pub async fn turn_off(&self, address: &str, addon: &str, actor: Option<Actor>) -> Result<(), ApiError> {
        let Some(command) = neighbours::turn_off_command(addon) else {
            return Err(ApiError::bad_request(crate::tr!("Этот мод DuoWave выключать не умеет.")));
        };
        let Some(link) = self.link(address)? else {
            return Err(ApiError::conflict(crate::tr!("Сначала проверьте колонку: DuoWave её ещё не видел.")));
        };
        let shell = self
            .opener
            .open(address, self.options(address, Some(link.fingerprint.clone())))
            .await
            .map_err(|error| ApiError::bad_gateway(problem_of(&error)))?;
        let result = shell.run(&command, None).await;
        shell.close().await;
        let failure = match &result {
            Ok(run) if run.code == 0 => None,
            Ok(run) => Some(first_nonempty(&run.stderr, &run.stdout)),
            Err(error) => Some(problem_of(error)),
        };
        self.events.emit(GatewayEvent::StationModOff {
            address: address.to_string(),
            addon: addon.to_string(),
            result: if failure.is_none() { "done" } else { "failed" }.to_string(),
            error: failure.clone(),
            actor,
        });
        match failure {
            None => Ok(()),
            Some(why) => {
                let why: String = why.chars().take(300).collect();
                Err(ApiError::bad_gateway(crate::tr!("Мод не выключился: {why}", why = why)))
            }
        }
    }

    /// «Подключить»: the job runs on; the panel follows it by its id.
    pub fn connect(
        self: &Arc<Self>,
        address: &str,
        gateway: &GatewayHost,
        actor: Option<Actor>,
    ) -> Result<ConnectJob, ApiError> {
        let target = gateway_addresses(gateway, &self.path);
        let job = self.begin(JobKind::Connect, address)?;
        let me = self.clone();
        let id = job.id.clone();
        tokio::spawn(async move { me.run_connect(id, target, actor).await });
        Ok(job)
    }

    /// The way back: `yandex` («Вернуть к Яндексу») — the station without a gateway, as it was before any;
    /// `previous` («Вернуть как было», when it did not come) — what it had right before the last «Подключить».
    pub fn restore(
        self: &Arc<Self>,
        address: &str,
        to: RestoreTo,
        actor: Option<Actor>,
    ) -> Result<ConnectJob, ApiError> {
        let job = self.begin(JobKind::Restore, address)?;
        let me = self.clone();
        let id = job.id.clone();
        tokio::spawn(async move { me.run_restore(id, to, actor).await });
        Ok(job)
    }

    /// «Удалить DuoWave с колонки»: uninstall.sh over SSH — the voice back to Yandex, the station's gateway and call
    /// client stopped and gone, our files off it — then a check that nothing of ours is left, then a reboot.
    pub fn uninstall(self: &Arc<Self>, address: &str, actor: Option<Actor>) -> Result<ConnectJob, ApiError> {
        let job = self.begin(JobKind::Uninstall, address)?;
        let me = self.clone();
        let id = job.id.clone();
        tokio::spawn(async move { me.run_uninstall(id, actor).await });
        Ok(job)
    }

    pub fn job(&self, id: &str) -> Result<ConnectJob, ApiError> {
        self.jobs().get(id).cloned().ok_or_else(|| ApiError::not_found(crate::tr!("Нет такого подключения.")))
    }

    fn jobs(&self) -> std::sync::MutexGuard<'_, HashMap<String, ConnectJob>> {
        self.jobs.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn begin(&self, kind: JobKind, address: &str) -> Result<ConnectJob, ApiError> {
        let mut jobs = self.jobs();
        if jobs.values().any(|job| job.address == address && job.status == JobStatus::Running) {
            return Err(ApiError::conflict(crate::tr!("С этой колонкой уже идёт подключение.")));
        }
        let ids: &[StepId] = match kind {
            JobKind::Connect => &[StepId::Copy, StepId::Write, StepId::Verify, StepId::Reboot, StepId::Arrive],
            JobKind::Restore => &[StepId::Write, StepId::Verify, StepId::Reboot],
            JobKind::Uninstall => &[StepId::Remove, StepId::Verify, StepId::Reboot],
        };
        let job = ConnectJob {
            id: crate::utils::ids::uuid(),
            kind,
            address: address.to_string(),
            status: JobStatus::Running,
            steps: ids.iter().map(|id| Step { id: *id, state: StepState::Pending }).collect(),
            copy: None,
            device: None,
            error: None,
            started_at: now_iso(),
        };
        // Jobs are the panel's to watch for a while, not history: «Кто что менял» keeps that.
        let now = crate::utils::time::now_ms();
        jobs.retain(|_, old| now.saturating_sub(parse_ms(&old.started_at)) <= 3_600_000);
        jobs.insert(job.id.clone(), job.clone());
        Ok(job)
    }

    fn step(&self, id: &str, step: StepId, state: StepState) {
        if let Some(job) = self.jobs().get_mut(id) {
            if let Some(entry) = job.steps.iter_mut().find(|entry| entry.id == step) {
                entry.state = state;
            }
            // Every step's outcome in gateway.log: a stranger's log is all we see of their wizard (Windows test 21.11).
            if state != StepState::Running {
                tracing::info!("station {}: {:?} {step:?} — {state:?}", job.address, job.kind);
            }
        }
    }

    fn with_job(&self, id: &str, edit: impl FnOnce(&mut ConnectJob)) {
        if let Some(job) = self.jobs().get_mut(id) {
            edit(job);
        }
    }

    async fn run_connect(self: Arc<Self>, id: String, target: VoiceAddresses, actor: Option<Actor>) {
        let address = self.job_address(&id);
        let mut shell: Option<Box<dyn StationShell>> = None;
        let outcome = self.connect_steps(&id, &address, &target, actor.clone(), &mut shell).await;
        if let Some(shell) = shell.take() {
            shell.close().await;
        }
        if let Err(error) = outcome {
            self.fail(&id, &error);
            tracing::warn!(
                "station {address}: not connected: {}",
                self.job(&id).ok().and_then(|j| j.error).unwrap_or_default()
            );
            self.audit(&id, "gateway", "failed", &target.url, actor);
        }
    }

    async fn connect_steps(
        &self,
        id: &str,
        address: &str,
        target: &VoiceAddresses,
        actor: Option<Actor>,
        shell: &mut Option<Box<dyn StationShell>>,
    ) -> anyhow::Result<()> {
        let link = self.link(address)?;
        *shell =
            Some(self.opener.open(address, self.options(address, link.as_ref().map(|l| l.fingerprint.clone()))).await?);
        let fingerprint = shell.as_deref().unwrap().fingerprint().to_string();
        let text = shell.as_deref().unwrap().run(&STEPS.read, None).await?.stdout;
        let facts = read_config(&text)?;
        // The firmware's own file, under our copy when one is mounted: the copy is made from it.
        let firmware = shell.as_deref().unwrap().run(&STEPS.firmware, None).await?.stdout;
        let own = read_config(&firmware)?.addresses;
        let expected = with_addresses(&firmware, target)?;
        // Two ways back. What it had just now — «Вернуть как было», another gateway included. And the station
        // without a gateway — restore.sh and «Вернуть к Яндексу»: what the firmware's own file says.
        let previous = facts.addresses.clone();
        let original = own;

        self.step(id, StepId::Copy, StepState::Running);
        // The way back is in the gateway before anything on the station changes: not kept, nothing changes.
        self.remember(
            address,
            LinkChange {
                fingerprint: Some(fingerprint),
                original: Some(original.clone()),
                previous: Some(previous.clone()),
                gateway: Field::Set(target.url.clone()),
                ..Default::default()
            },
        )?;
        deliver(shell.as_deref().unwrap()).await?;
        self.step(id, StepId::Copy, StepState::Done);

        self.step(id, StepId::Write, StepState::Running);
        let switched = shell.as_deref().unwrap().run(&switch_to(target, false)?, None).await?;
        self.with_job(id, |job| job.copy = copy_of(&switched.stdout, &switched.stderr));
        if switched.code != 0 {
            anyhow::bail!("{}", first_nonempty(&switched.stderr, &format!("the switch ended with {}", switched.code)));
        }
        self.step(id, StepId::Write, StepState::Done);

        self.step(id, StepId::Verify, StepState::Running);
        let written = shell.as_deref().unwrap().run(&STEPS.read, None).await?.stdout;
        if written != expected {
            // Not the file we meant: the addresses it had back, no reboot. Said so only when the undo did it; an undo
            // that timed out or lost the station is the job's error, as the TS await made it.
            let undo = shell.as_deref().unwrap().run(&switch_to(&previous, true)?, None).await?;
            if undo.code != 0 {
                anyhow::bail!(
                    "the written file is not the one expected, and the old addresses are not back: {}",
                    first_nonempty(&undo.stderr, &format!("the undo ended with {}", undo.code))
                );
            }
            anyhow::bail!("the written file is not the one expected: the old addresses are back, no reboot");
        }
        self.step(id, StepId::Verify, StepState::Done);

        self.step(id, StepId::Reboot, StepState::Running);
        self.stations.expect(address, self.timing.invitation_ms);
        // Listen for the station's return before the reboot, so nothing is missed.
        let receiver = self.events.subscribe();
        let _ = shell.as_deref().unwrap().run(STEPS.reboot, None).await;
        if let Some(shell) = shell.take() {
            shell.close().await;
        }
        self.step(id, StepId::Reboot, StepState::Done);

        self.step(id, StepId::Arrive, StepState::Running);
        let Some(device) = self.await_arrival(receiver, address).await else {
            self.step(id, StepId::Arrive, StepState::Failed);
            self.with_job(id, |job| {
                job.status = JobStatus::Timeout;
                job.error = Some("no-arrival".into());
            });
            self.audit(id, "gateway", "timeout", &target.url, actor);
            return Ok(());
        };
        self.with_job(id, |job| job.device = Some(device.clone()));
        self.step(id, StepId::Arrive, StepState::Done);
        self.with_job(id, |job| job.status = JobStatus::Done);
        // The station is here, and its way back was kept at the copy step: a failed note of its arrival is only logged.
        let arrived = LinkChange { device: Some(device), connected_at: Field::Set(now_iso()), ..Default::default() };
        if let Err(error) = self.remember(address, arrived) {
            tracing::warn!("station {address}: {error:#}");
        }
        self.audit(id, "gateway", "done", &target.url, actor);
        Ok(())
    }

    async fn run_restore(self: Arc<Self>, id: String, to: RestoreTo, actor: Option<Actor>) {
        let address = self.job_address(&id);
        let link = match self.link(&address) {
            Ok(link) => link,
            Err(error) => {
                // Where back is unknown: nothing opens, nothing on the station changes.
                self.fail(&id, &error.into());
                tracing::warn!(
                    "station {address}: not restored: {}",
                    self.job(&id).ok().and_then(|j| j.error).unwrap_or_default()
                );
                return;
            }
        };
        // «Вернуть к Яндексу»: the station's own addresses when they are Yandex's (our copy simply goes), else Yandex's.
        let own = link.as_ref().and_then(|l| l.original.clone()).filter(|own| points_to(&own.url) == PointsTo::Yandex);
        let original = match to {
            RestoreTo::Previous => link.as_ref().and_then(|l| l.previous.clone()).or(own),
            RestoreTo::Yandex => own,
        }
        .unwrap_or_else(yandex);
        let where_to = if points_to(&original.url) == PointsTo::Gateway { "gateway" } else { "yandex" };
        let mut shell: Option<Box<dyn StationShell>> = None;
        let outcome =
            self.restore_steps(&id, &address, &original, where_to, link.as_ref(), actor.clone(), &mut shell).await;
        if let Some(shell) = shell.take() {
            shell.close().await;
        }
        if let Err(error) = outcome {
            self.fail(&id, &error);
            tracing::warn!(
                "station {address}: not restored: {}",
                self.job(&id).ok().and_then(|j| j.error).unwrap_or_default()
            );
            self.audit(&id, where_to, "failed", &original.url, actor);
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn restore_steps(
        &self,
        id: &str,
        address: &str,
        original: &VoiceAddresses,
        where_to: &str,
        link: Option<&StationLink>,
        actor: Option<Actor>,
        shell: &mut Option<Box<dyn StationShell>>,
    ) -> anyhow::Result<()> {
        *shell = Some(self.opener.open(address, self.options(address, link.map(|l| l.fingerprint.clone()))).await?);
        self.step(id, StepId::Write, StepState::Running);
        let text = shell.as_deref().unwrap().run(&STEPS.read, None).await?.stdout;
        let facts = read_config(&text)?;
        if facts.addresses.url == original.url && facts.addresses.quic == original.quic {
            for step in [StepId::Write, StepId::Verify, StepId::Reboot] {
                self.step(id, step, StepState::Skipped);
            }
            self.with_job(id, |job| job.status = JobStatus::Done);
            return Ok(());
        }
        let firmware = shell.as_deref().unwrap().run(&STEPS.firmware, None).await?.stdout;
        let expected = with_addresses(&firmware, original)?;
        // voice.sh may not be there (a station pointed by hand, or by an older gateway): the files first. The way
        // back replaces whatever is in our copy.
        deliver(shell.as_deref().unwrap()).await?;
        let switched = shell.as_deref().unwrap().run(&switch_to(original, true)?, None).await?;
        self.with_job(id, |job| job.copy = copy_of(&switched.stdout, &switched.stderr));
        if switched.code != 0 {
            anyhow::bail!("{}", first_nonempty(&switched.stderr, &format!("the switch ended with {}", switched.code)));
        }
        self.step(id, StepId::Write, StepState::Done);
        self.step(id, StepId::Verify, StepState::Running);
        if shell.as_deref().unwrap().run(&STEPS.read, None).await?.stdout != expected {
            anyhow::bail!("the written file is not the one expected");
        }
        self.step(id, StepId::Verify, StepState::Done);
        self.step(id, StepId::Reboot, StepState::Running);
        let _ = shell.as_deref().unwrap().run(STEPS.reboot, None).await;
        self.step(id, StepId::Reboot, StepState::Done);
        self.with_job(id, |job| job.status = JobStatus::Done);
        // The station is switched and rebooted: a failed note of it is only logged.
        let back = LinkChange {
            connected_at: Field::Clear,
            gateway: if where_to == "gateway" { Field::Set(original.url.clone()) } else { Field::Clear },
            ..Default::default()
        };
        if let Err(error) = self.remember(address, back) {
            tracing::warn!("station {address}: {error:#}");
        }
        self.audit(id, where_to, "done", &original.url, actor);
        Ok(())
    }

    async fn run_uninstall(self: Arc<Self>, id: String, actor: Option<Actor>) {
        let address = self.job_address(&id);
        let mut shell: Option<Box<dyn StationShell>> = None;
        let outcome = self.uninstall_steps(&id, &address, actor.clone(), &mut shell).await;
        if let Some(shell) = shell.take() {
            shell.close().await;
        }
        if let Err(error) = outcome {
            self.fail(&id, &error);
            tracing::warn!(
                "station {address}: DuoWave not removed: {}",
                self.job(&id).ok().and_then(|j| j.error).unwrap_or_default()
            );
            self.audit(&id, "removed", "failed", "", actor);
        }
    }

    async fn uninstall_steps(
        &self,
        id: &str,
        address: &str,
        actor: Option<Actor>,
        shell: &mut Option<Box<dyn StationShell>>,
    ) -> anyhow::Result<()> {
        // The station's key as it was learned: a station that answers with another one is not opened.
        let link = self.link(address)?;
        *shell = Some(self.opener.open(address, self.options(address, link.map(|l| l.fingerprint))).await?);
        let shell = shell.as_deref().unwrap();
        self.step(id, StepId::Remove, StepState::Running);
        // The script as this gateway has it, whatever the station kept (or did not: connected by an older build).
        let removed = shell.run(STEPS.uninstall, Some(&uninstall_script())).await?;
        if removed.code != 0 {
            anyhow::bail!("{}", first_nonempty(&removed.stderr, &format!("uninstall.sh ended with {}", removed.code)));
        }
        self.step(id, StepId::Remove, StepState::Done);
        self.step(id, StepId::Verify, StepState::Running);
        let left = shell.run(&STEPS.leftovers, None).await?.stdout;
        let left: Vec<&str> = left.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        if !left.is_empty() {
            anyhow::bail!("left on the station: {}", left.join(", "));
        }
        self.step(id, StepId::Verify, StepState::Done);
        self.step(id, StepId::Reboot, StepState::Running);
        let _ = shell.run(STEPS.reboot, None).await;
        self.step(id, StepId::Reboot, StepState::Done);
        self.with_job(id, |job| job.status = JobStatus::Done);
        // The key and the addresses it had stay: «Подключить» finds the station as it knew it.
        let gone = LinkChange { connected_at: Field::Clear, gateway: Field::Clear, ..Default::default() };
        if let Err(error) = self.remember(address, gone) {
            tracing::warn!("station {address}: {error:#}");
        }
        self.audit(id, "removed", "done", &yandex().url, actor);
        Ok(())
    }

    /// The device that comes to /uni.ws from this address next, or None when none does in time.
    async fn await_arrival(
        &self,
        mut receiver: broadcast::Receiver<Arc<GatewayEvent>>,
        address: &str,
    ) -> Option<String> {
        let deadline = Instant::now() + Duration::from_millis(self.timing.arrival_ms);
        loop {
            match timeout_at(deadline, receiver.recv()).await {
                Ok(Ok(event)) => {
                    if let GatewayEvent::StationConnected { device_tag, .. } = &*event {
                        if self.stations.find(device_tag).and_then(|station| station.last_address).as_deref()
                            == Some(address)
                        {
                            return Some(device_tag.clone());
                        }
                    }
                }
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
                Ok(Err(broadcast::error::RecvError::Closed)) => return None,
                Err(_) => return None,
            }
        }
    }

    fn job_address(&self, id: &str) -> String {
        self.jobs().get(id).map(|job| job.address.clone()).unwrap_or_default()
    }

    /// The links as stored. A failed read is an error, never «no links»: that would open with no key to hold to, and
    /// the next write would drop every other station's link (the TS settings read rejected).
    fn links(&self) -> rusqlite::Result<BTreeMap<String, StationLink>> {
        let mut links = self.settings.try_entry(&STATION_LINKS)?.map(|entry| entry.value).unwrap_or_default();
        // A link without its key is none, as the TS parse dropped one: an empty pin would refuse every key.
        links.retain(|_, link| !link.fingerprint.is_empty());
        Ok(links)
    }

    fn link(&self, address: &str) -> rusqlite::Result<Option<StationLink>> {
        Ok(self.links()?.remove(address))
    }

    fn remember(&self, address: &str, change: LinkChange) -> anyhow::Result<()> {
        let mut links = self.links()?;
        // No key learned and none brought: nothing to keep (the TS kept a link its parse then dropped).
        let Some(next) = change.apply(links.remove(address)) else { return Ok(()) };
        links.insert(address.to_string(), next);
        // The database's own words are in the log already (ApiError::internal).
        self.settings
            .set(&STATION_LINKS, &links, None)
            .map_err(|_| anyhow::anyhow!("the station link is not saved: the gateway's database failed"))
    }

    fn fail(&self, id: &str, error: &anyhow::Error) {
        let problem = problem_of(error);
        self.with_job(id, |job| {
            if let Some(running) = job.steps.iter_mut().find(|step| step.state == StepState::Running) {
                running.state = StepState::Failed;
            }
            job.status = JobStatus::Failed;
            job.error = Some(problem);
        });
    }

    fn audit(&self, id: &str, to: &str, result: &str, url: &str, actor: Option<Actor>) {
        let job = self.jobs().get(id).cloned();
        self.events.emit(GatewayEvent::StationSwitched {
            address: job.as_ref().map(|job| job.address.clone()).unwrap_or_default(),
            to: to.to_string(),
            result: result.to_string(),
            url: url.to_string(),
            device: job.as_ref().and_then(|job| job.device.clone()),
            error: job.and_then(|job| job.error),
            actor,
        });
    }
}

/// voice.sh, the boot script's source and restore.sh into /data/opt/duowave, each whole or not at all.
async fn deliver(shell: &dyn StationShell) -> anyhow::Result<()> {
    for (command, input) in deliveries() {
        let left = shell.run(&command, Some(&input)).await?;
        if left.code != 0 {
            anyhow::bail!(
                "the station's scripts are not written: {}",
                first_nonempty(&left.stderr, &left.code.to_string())
            );
        }
    }
    Ok(())
}

fn base(address: &str, ssh: Option<String>, device: Option<String>) -> StationCheck {
    StationCheck {
        address: address.to_string(),
        ssh,
        fingerprint: None,
        facts: None,
        data_free_kb: None,
        arch: None,
        client: None,
        updates: None,
        copy: None,
        neighbours: None,
        mem_available_kb: None,
        device,
        target: None,
        connected: None,
        problem: None,
    }
}

/// The available kilobytes from a `df -k` line (the TS `/^\S+\s+\d+\s+\d+\s+(\d+)/`): the device, then blocks and
/// used, then the available column.
fn free_kb(line: &str) -> Option<u64> {
    let mut fields = line.split_whitespace();
    fields.next()?; // the device
    let _blocks: u64 = fields.next()?.parse().ok()?;
    let _used: u64 = fields.next()?.parse().ok()?;
    fields.next()?.parse().ok()
}

fn arch_ok(value: &str) -> bool {
    (1..=20).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn copy_of(stdout: &str, stderr: &str) -> Option<String> {
    let text = format!("{stdout}{stderr}");
    let start = text.find("copy: ")? + "copy: ".len();
    let value: String = text[start..].chars().take_while(|c| !c.is_whitespace()).collect();
    (!value.is_empty()).then_some(value)
}

fn first_nonempty(preferred: &str, fallback: &str) -> String {
    let trimmed = preferred.trim();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

fn parse_ms(iso: &str) -> u64 {
    iso.parse::<chrono::DateTime<chrono::Utc>>().map(|at| at.timestamp_millis().max(0) as u64).unwrap_or(0)
}

/// What went wrong, in words the panel knows: the key, the login, the network — or the message.
/// The panel's problem for a station whose SSH port did not connect: off or elsewhere, SSH not started, no way there
/// from this computer, or the system's own odd answer (its words are in the log).
fn unreached_problem(kind: Unreached) -> &'static str {
    match kind {
        Unreached::Timeout => "station-off",
        Unreached::Refused => "no-ssh",
        Unreached::NoRoute => "no-route",
        Unreached::Other => "no-connect",
    }
}

pub fn problem_of(error: &anyhow::Error) -> String {
    if error.downcast_ref::<HostKeyChanged>().is_some() {
        return "host-key-changed".into();
    }
    if let Some(shell) = error.downcast_ref::<ShellError>() {
        match shell {
            ShellError::AuthFailed => return "auth-failed".into(),
            ShellError::Unreachable(_) => return "unreachable".into(),
            ShellError::Other(_) => {}
        }
    }
    let message = error.to_string();
    let lower = message.to_lowercase();
    if lower.contains("authentication methods failed") || lower.contains("all configured authentication") {
        return "auth-failed".into();
    }
    if ["econnrefused", "ehostunreach", "enetunreach", "etimedout", "timed out while waiting for handshake"]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        return "unreachable".into();
    }
    message.chars().take(300).collect()
}

/// A field a remembered link change may keep, set, or clear.
#[derive(Default)]
enum Field<T> {
    #[default]
    Keep,
    Set(T),
    Clear,
}

/// A change to a remembered station link: absent fields are kept, as the TS object spread did.
#[derive(Default)]
struct LinkChange {
    fingerprint: Option<String>,
    original: Option<LinkAddresses>,
    previous: Option<LinkAddresses>,
    gateway: Field<String>,
    device: Option<String>,
    connected_at: Field<String>,
}

impl LinkChange {
    /// The link after the change; None when there is none and the change brings no key. A link is its key first:
    /// one without it would be held to as an empty pin, and every later look refused as another station.
    fn apply(self, existing: Option<StationLink>) -> Option<StationLink> {
        let fingerprint = self.fingerprint.filter(|key| !key.is_empty());
        let mut link = match (existing, fingerprint) {
            (Some(mut link), fingerprint) => {
                if let Some(fingerprint) = fingerprint {
                    link.fingerprint = fingerprint;
                }
                link
            }
            (None, Some(fingerprint)) => StationLink {
                fingerprint,
                original: None,
                previous: None,
                gateway: None,
                device: None,
                connected_at: None,
            },
            (None, None) => return None,
        };
        if self.original.is_some() {
            link.original = self.original;
        }
        if self.previous.is_some() {
            link.previous = self.previous;
        }
        match self.gateway {
            Field::Keep => {}
            Field::Set(url) => link.gateway = Some(url),
            Field::Clear => link.gateway = None,
        }
        if let Some(device) = self.device {
            link.device = Some(device);
        }
        match self.connected_at {
            Field::Keep => {}
            Field::Set(at) => link.connected_at = Some(at),
            Field::Clear => link.connected_at = None,
        }
        Some(link)
    }
}

/// The emergency words' give-back on a computer's gateway (gateway/uniproxy/rescue.rs, the Station's adapter): the
/// station at its address, over SSH, the job the panel's button starts.
#[cfg(feature = "yandex-station")]
pub struct OverSsh(pub Arc<StationConnectService>);

#[cfg(feature = "yandex-station")]
impl crate::gateway::uniproxy::rescue::StationRescue for OverSsh {
    fn give_back(&self, address: &str, actor: Option<crate::core::events::Actor>) -> Result<(), String> {
        self.0.restore(address, RestoreTo::Yandex, actor).map(|_| ()).map_err(|error| error.message)
    }
}

#[cfg(test)]
mod tests {
    //! The connect flow against a pretend station behind the shell — never a real one.

    use std::sync::LazyLock;

    use futures::future::BoxFuture;
    use regex::Regex;

    use super::*;
    use crate::core::events::EventBus;
    use crate::stations::connect::quasar_config::{with_addresses, YANDEX_QUIC_URL, YANDEX_URL};
    use crate::stations::connect::station_shell::{HostKeyChanged, ShellOptions, ShellResult};
    use crate::stations::service::StationStatus;
    use crate::storage::Db;

    const ADDRESS: &str = "192.0.2.23";

    fn config_text() -> String {
        format!(
            "{{\n    \"aliced\" : {{\n        \"apiKey\" : \"not-a-real-key\",\n        \"uniProxyQuicUrl\" : \"{YANDEX_QUIC_URL}\",\n        \"uniProxyUrl\" : \"{YANDEX_URL}\"\n    }},\n    \"common\" : {{ \"deviceType\" : \"test_station\", \"softwareVersion\" : \"1.0\" }}\n}}\n"
        )
    }

    /// voice.sh's switch: `sh …/voice.sh point '<url>' '<quic>'[ --force]`.
    static POINT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^sh \S+/voice\.sh point '([^']*)' '([^']*)'( --force)?$").unwrap());

    /// A pretend station behind the shell: the firmware's file, our copy mounted over it (as voice.sh would), the files
    /// put into /data/opt/duowave, whether it rebooted. Never a real one.
    struct State {
        firmware: String,
        copy: Option<String>,
        files: HashMap<String, String>,
        reboots: u32,
        commands: Vec<String>,
        /// A write that lands wrong, to see the gateway undo it.
        corrupt: bool,
        /// How putting the old addresses back goes.
        undo: Undo,
        key: String,
        /// What the check after uninstall.sh finds still there, a line each.
        left: String,
        /// What the look for neighbours prints.
        around: String,
        /// What its SSH port says first.
        banner: Result<String, NoBanner>,
        /// Its SSH never opens.
        no_shell: bool,
        /// Its root's own password; None — the empty one, as the wiki sets it up.
        root_password: Option<String>,
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Undo {
        Works,
        /// The undo's command ends with an error.
        Fails,
        /// The station stops answering: the shell's run gives up.
        Lost,
    }

    #[derive(Clone)]
    struct FakeStation {
        state: Arc<Mutex<State>>,
    }

    impl FakeStation {
        fn new() -> FakeStation {
            FakeStation {
                state: Arc::new(Mutex::new(State {
                    firmware: config_text(),
                    copy: None,
                    files: HashMap::new(),
                    reboots: 0,
                    commands: Vec::new(),
                    corrupt: false,
                    undo: Undo::Works,
                    key: "SHA256:fake-key".into(),
                    left: String::new(),
                    around: "maind on\nmemavail 61440\n".into(),
                    banner: Ok("dropbear_test".into()),
                    no_shell: false,
                    root_password: None,
                })),
            }
        }

        fn state(&self) -> std::sync::MutexGuard<'_, State> {
            self.state.lock().unwrap_or_else(|p| p.into_inner())
        }
    }

    impl State {
        /// The config as the station reads it: our copy while it is there.
        fn config(&self) -> String {
            self.copy.clone().unwrap_or_else(|| self.firmware.clone())
        }

        fn restore(&self) -> Option<&String> {
            self.files.get(crate::stations::connect::station_scripts::RESTORE_SCRIPT)
        }
    }

    impl ShellOpener for FakeStation {
        fn banner<'a>(&'a self, _address: &'a str, _port: u16) -> BoxFuture<'a, Result<String, NoBanner>> {
            let banner = self.state().banner.clone();
            Box::pin(async move { banner })
        }

        fn open<'a>(
            &'a self,
            _address: &'a str,
            options: ShellOptions,
        ) -> BoxFuture<'a, anyhow::Result<Box<dyn StationShell>>> {
            let state = self.state.clone();
            Box::pin(async move {
                if state.lock().unwrap().no_shell {
                    return Err(ShellError::Unreachable("Timed out while waiting for handshake".into()).into());
                }
                let wanted = state.lock().unwrap().root_password.clone().unwrap_or_default();
                if options.password != wanted {
                    return Err(ShellError::AuthFailed.into());
                }
                let key = state.lock().unwrap().key.clone();
                if let Some(trust) = &options.trust {
                    if *trust != key {
                        return Err(HostKeyChanged { fingerprint: key }.into());
                    }
                }
                Ok(Box::new(FakeShell { state, fingerprint: key }) as Box<dyn StationShell>)
            })
        }
    }

    struct FakeShell {
        state: Arc<Mutex<State>>,
        fingerprint: String,
    }

    impl StationShell for FakeShell {
        fn run<'a>(&'a self, command: &'a str, input: Option<&'a str>) -> BoxFuture<'a, anyhow::Result<ShellResult>> {
            let mut state = self.state.lock().unwrap();
            state.commands.push(command.to_string());
            let done = |stdout: &str| ShellResult { code: 0, stdout: stdout.into(), stderr: String::new() };
            let delivered = deliveries().into_iter().find(|(step, _)| step == command);
            let result = if command == STEPS.read {
                done(&state.config())
            } else if command == STEPS.firmware {
                let firmware = state.firmware.clone();
                done(&firmware)
            } else if command == STEPS.facts {
                let copy = if state.copy.is_some() { "mounted" } else { "none" };
                done(&format!("/dev/ubi0_5 89088 2048 87040 2% /data\narmv7l\nnone\nupdates-blocked\n{copy}\n-\n"))
            } else if ["pulsar", "quasar-bridge", "yajavis", "jarvis-server"]
                .iter()
                .any(|id| neighbours::turn_off_command(id).as_deref() == Some(command))
            {
                done("")
            } else if command == STEPS.neighbours {
                let around = state.around.clone();
                done(&around)
            } else if let Some((step, _)) = delivered {
                let path = step.split("mv -f ").nth(1).and_then(|rest| rest.split(' ').nth(1)).unwrap().to_string();
                state.files.insert(path, input.unwrap_or_default().to_string());
                done("")
            } else if command == STEPS.reboot {
                state.reboots += 1;
                done("")
            } else if command == STEPS.uninstall && input == Some(uninstall_script().as_str()) {
                state.copy = None;
                state.files.clear();
                done("DuoWave is off this station; the reboot is next\n")
            } else if command == STEPS.leftovers {
                let left = state.left.clone();
                done(&left)
            } else if let Some(point) = POINT.captures(command) {
                let (url, quic, forced) = (point[1].to_string(), point[2].to_string(), point.get(3).is_some());
                // After a write that landed wrong, the forced one is the undo.
                if forced && state.corrupt && state.undo == Undo::Lost {
                    return Box::pin(async { Err(anyhow::anyhow!("no answer in 20 s")) });
                }
                if forced && state.corrupt && state.undo == Undo::Fails {
                    ShellResult { code: 1, stdout: String::new(), stderr: "mount: permission denied\n".into() }
                } else {
                    let next = with_addresses(&state.firmware, &VoiceAddresses { url: url.clone(), quic }).unwrap();
                    state.copy = (next != state.firmware).then_some(next);
                    if state.corrupt && !forced && url.starts_with("ws://") {
                        state.copy =
                            state.copy.take().map(|copy| format!("{} ", copy.replace("\"vadEnabled\"", "\"broken\"")));
                    }
                    done("copy: /data/opt/duowave/voice/quasar.cfg\n")
                }
            } else {
                ShellResult { code: 127, stdout: String::new(), stderr: format!("unexpected: {command}") }
            };
            Box::pin(async move { Ok(result) })
        }

        fn fingerprint(&self) -> &str {
            &self.fingerprint
        }

        fn close(&self) -> BoxFuture<'_, ()> {
            Box::pin(async {})
        }
    }

    /// A fake stations registry: the records the test puts in, and the addresses `expect` was called with.
    #[derive(Default)]
    struct FakeStations {
        records: Mutex<HashMap<String, StationView>>,
        expected: Mutex<Vec<String>>,
    }

    impl Stations for FakeStations {
        fn find(&self, device: &str) -> Option<StationView> {
            self.records.lock().unwrap().get(device).cloned()
        }
        fn device_for_address(&self, address: &str) -> Option<String> {
            self.records
                .lock()
                .unwrap()
                .values()
                .find(|station| station.last_address.as_deref() == Some(address))
                .map(|station| station.device.clone())
        }
        fn expect(&self, address: &str, _ms: u64) {
            self.expected.lock().unwrap().push(address.to_string());
        }
    }

    fn record(device: &str, address: &str) -> StationView {
        StationView {
            device: device.into(),
            name: "Колонка".into(),
            status: StationStatus::Approved,
            last_address: Some(address.into()),
            first_seen: now_iso(),
            last_seen: now_iso(),
            device_model: None,
            device_color: None,
            firmware: None,
            model: None,
            color: None,
            sticker: None,
        }
    }

    /// A recorded `station.switched`: address, to, result, url.
    type Switch = (String, String, String, String);

    struct Switched {
        _task: tokio::task::JoinHandle<()>,
        log: Arc<Mutex<Vec<Switch>>>,
    }

    impl Switched {
        fn watch(events: &EventBus) -> Switched {
            let log: Arc<Mutex<Vec<Switch>>> = Arc::default();
            let sink = log.clone();
            let mut receiver = events.subscribe();
            let task = tokio::spawn(async move {
                while let Ok(event) = receiver.recv().await {
                    if let GatewayEvent::StationSwitched { address, to, result, url, .. } = &*event {
                        sink.lock().unwrap().push((address.clone(), to.clone(), result.clone(), url.clone()));
                    }
                }
            });
            Switched { _task: task, log }
        }

        fn list(&self) -> Vec<Switch> {
            self.log.lock().unwrap().clone()
        }
    }

    struct Setup {
        service: Arc<StationConnectService>,
        station: FakeStation,
        stations: Arc<FakeStations>,
        events: EventBus,
        db: Arc<Db>,
        settings: Arc<SettingsService>,
        switched: Switched,
    }

    impl Setup {
        fn new(timing: ConnectTiming) -> Setup {
            Setup::with_opener(timing, None)
        }

        /// The same, reaching stations through `opener` instead of the pretend station.
        fn with_opener(timing: ConnectTiming, opener: Option<Arc<dyn ShellOpener>>) -> Setup {
            let events = EventBus::new();
            let station = FakeStation::new();
            let stations = Arc::new(FakeStations::default());
            let db = Arc::new(Db::memory());
            let settings = Arc::new(SettingsService::new(db.clone(), events.clone()));
            let switched = Switched::watch(&events);
            let service = Arc::new(StationConnectService::new(
                settings.clone(),
                stations.clone(),
                events.clone(),
                opener.unwrap_or_else(|| Arc::new(station.clone())),
                ShellConfig {
                    port: 2222,
                    user: "root".into(),
                    password: String::new(),
                    passwords: StationPasswords::new(Arc::new(
                        crate::secrets::service::SecretsService::new(db.clone(), None).unwrap(),
                    )),
                },
                "/uni.ws".into(),
                80,
                timing,
            ));
            Setup { service, station, stations, events, db, settings, switched }
        }

        /// Waits for a job to end and gives it as it ended.
        async fn ended(&self, job: &ConnectJob) -> ConnectJob {
            until(|| self.service.job(&job.id).unwrap().status != JobStatus::Running).await;
            self.service.job(&job.id).unwrap()
        }

        /// The rebooted station comes to /uni.ws from its address.
        fn arrive(&self, device: &str) {
            self.stations.records.lock().unwrap().insert(device.into(), record(device, ADDRESS));
            self.events.emit(GatewayEvent::StationConnected { session_id: "s1".into(), device_tag: device.into() });
        }

        fn links(&self) -> Option<std::collections::BTreeMap<String, StationLink>> {
            self.settings.get(&STATION_LINKS, None)
        }

        fn link(&self) -> StationLink {
            self.links().and_then(|map| map.get(ADDRESS).cloned()).expect("a link for the address")
        }
    }

    fn fast() -> ConnectTiming {
        ConnectTiming { arrival_ms: 300, invitation_ms: 1000 }
    }

    fn gateway() -> GatewayHost {
        GatewayHost { host: "192.0.2.10".into(), port: 80 }
    }

    async fn until(mut check: impl FnMut() -> bool) {
        for _ in 0..400 {
            if check() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("timed out");
    }

    #[tokio::test]
    async fn the_look_names_what_stands_in_the_way_and_changes_nothing() {
        let setup = Setup::new(fast());
        setup.station.state().around = "mod yajavis\nmod quasar-bridge\nmaind on\nmemavail 20480\n".into();
        let check = setup.service.check(ADDRESS, Some(&gateway())).await.unwrap();
        let found: Vec<(&str, neighbours::Severity)> =
            check.neighbours.iter().flatten().map(|n| (n.id.as_str(), n.severity)).collect();
        assert_eq!(
            found,
            [
                ("quasar-bridge", neighbours::Severity::Blocks),
                ("yajavis", neighbours::Severity::Warns),
                ("low-memory", neighbours::Severity::Warns)
            ]
        );
        // Named only: «Подключить» is still offered, the station is as it was.
        assert!(check.problem.is_none());
        assert_eq!(setup.station.state().config(), config_text());
        assert_eq!(setup.station.state().reboots, 0);
        let json = serde_json::to_value(&check).unwrap();
        assert_eq!(json["neighbours"][0]["severity"], "blocks");
        assert_eq!(json["memAvailableKb"], 20480);
    }

    #[tokio::test]
    async fn turns_a_known_mod_off_by_its_own_switch_only_after_a_look_and_says_so() {
        let setup = Setup::new(fast());
        let mut events = setup.events.subscribe();
        // Not looked at yet: no key to trust, nothing runs.
        assert!(setup.service.turn_off(ADDRESS, "pulsar", None).await.is_err());
        assert!(setup.station.state().commands.is_empty());

        setup.service.check(ADDRESS, None).await.unwrap();
        let unknown = setup.service.turn_off(ADDRESS, "alisa-voice", None).await;
        assert!(unknown.is_err(), "no switch of its own: no button");
        setup.service.turn_off(ADDRESS, "pulsar", None).await.unwrap();
        let commands = setup.station.state().commands.clone();
        assert_eq!(commands.last().map(String::as_str), neighbours::turn_off_command("pulsar").as_deref());
        assert_eq!(setup.station.state().reboots, 0);
        assert_eq!(setup.station.state().config(), config_text(), "Alice's voice untouched");

        let said: Vec<_> = std::iter::from_fn(|| events.try_recv().ok())
            .filter_map(|event| match &*event {
                GatewayEvent::StationModOff { addon, result, .. } => Some((addon.clone(), result.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(said, [("pulsar".to_string(), "done".to_string())]);
    }

    #[tokio::test]
    async fn looks_first_reading_only_and_learns_the_key() {
        let setup = Setup::new(fast());
        let check = setup.service.check(ADDRESS, Some(&gateway())).await.unwrap();
        assert_eq!(check.address, ADDRESS);
        assert_eq!(check.ssh.as_deref(), Some("dropbear_test"));
        assert_eq!(check.fingerprint.as_deref(), Some("SHA256:fake-key"));
        let facts = check.facts.clone().unwrap();
        assert_eq!(facts.addresses, VoiceAddresses { url: YANDEX_URL.into(), quic: YANDEX_QUIC_URL.into() });
        assert_eq!(facts.device_type.as_deref(), Some("test_station"));
        assert_eq!(facts.points_to, PointsTo::Yandex);
        assert_eq!(check.data_free_kb, Some(87040));
        assert_eq!(check.arch.as_deref(), Some("armv7l"));
        assert_eq!(check.client, Some(false));
        assert_eq!(check.updates.as_deref(), Some("blocked"));
        assert_eq!(check.target, Some(VoiceAddresses { url: "ws://192.0.2.10/uni.ws".into(), quic: String::new() }));
        assert_eq!(check.connected, Some(false));
        assert_eq!(check.neighbours, Some(vec![]));
        assert_eq!(check.mem_available_kb, Some(61440));
        assert_eq!(
            setup.station.state().commands,
            vec![STEPS.read.clone(), STEPS.facts.clone(), STEPS.neighbours.clone()]
        );
        assert_eq!(setup.station.state().config(), config_text());
        assert_eq!(
            setup.link(),
            StationLink {
                fingerprint: "SHA256:fake-key".into(),
                original: None,
                previous: None,
                gateway: None,
                device: None,
                connected_at: None
            }
        );
        assert!(!serde_json::to_string(&check).unwrap().contains("not-a-real-key"));
    }

    #[tokio::test]
    async fn says_why_the_station_s_ssh_did_not_answer() {
        let cases = [
            (NoBanner::Unreached(Unreached::Timeout, "no answer".into()), "station-off"),
            (NoBanner::Unreached(Unreached::Refused, "refused".into()), "no-ssh"),
            (NoBanner::Unreached(Unreached::NoRoute, "no route".into()), "no-route"),
            (NoBanner::Unreached(Unreached::Other, "odd".into()), "no-connect"),
        ];
        for (banner, problem) in cases {
            let setup = Setup::new(fast());
            setup.station.state().banner = Err(banner);
            let check = setup.service.check(ADDRESS, None).await.unwrap();
            assert_eq!(check.problem.as_deref(), Some(problem));
            assert!(check.ssh.is_none());
            assert!(setup.station.state().commands.is_empty(), "nothing tried on {problem}");
        }
    }

    #[tokio::test]
    async fn an_open_port_without_a_banner_still_gets_the_real_ssh() {
        // The banner read failed but SSH opens: the look goes on (the Windows test of 21.11).
        let setup = Setup::new(fast());
        setup.station.state().banner = Err(NoBanner::Silent);
        let check = setup.service.check(ADDRESS, None).await.unwrap();
        assert_eq!(check.problem, None);
        assert_eq!(check.ssh.as_deref(), Some("SSH"));
        // Neither opens: the banner's reason is the problem.
        for (banner, problem) in [(NoBanner::Silent, "ssh-slow"), (NoBanner::NotSsh(String::new()), "ssh-closed")] {
            let setup = Setup::new(fast());
            setup.station.state().banner = Err(banner);
            setup.station.state().no_shell = true;
            assert_eq!(setup.service.check(ADDRESS, None).await.unwrap().problem.as_deref(), Some(problem));
        }
    }

    /// Windows test 21.11: a station whose root has a password of its own. Typed in the wizard, it is kept sealed only
    /// when the station takes it, and every later step (connect) uses it; a wrong one is forgotten.
    #[tokio::test]
    async fn a_root_password_typed_in_the_wizard_is_kept_only_when_the_station_takes_it() {
        let setup = Setup::new(fast());
        setup.station.state().root_password = Some("s3cret-root".into());
        assert_eq!(setup.service.check(ADDRESS, None).await.unwrap().problem.as_deref(), Some("auth-failed"));
        let wrong = setup.service.check_with_password(ADDRESS, None, Some("nope")).await.unwrap();
        assert_eq!(wrong.problem.as_deref(), Some("auth-failed"));
        assert_eq!(setup.service.ssh.passwords.get(ADDRESS), None, "a refused one is not kept");
        let right = setup.service.check_with_password(ADDRESS, None, Some("s3cret-root")).await.unwrap();
        assert_eq!(right.problem, None);
        assert_eq!(setup.service.ssh.passwords.get(ADDRESS).as_deref(), Some("s3cret-root"));
        assert!(!serde_json::to_string(&right).unwrap().contains("s3cret-root"), "never in an answer");
        // The next look needs no password typed again.
        assert_eq!(setup.service.check(ADDRESS, None).await.unwrap().problem, None);
        // Another station's is its own.
        assert_eq!(setup.service.ssh.password_for("192.0.2.99"), "");
    }

    /// A station rooted with AliceBridge: its SSH takes root's «1». The look says to change it and never repeats it.
    #[tokio::test]
    async fn the_look_asks_to_change_alicebridge_s_default_root_password() {
        let setup = Setup::new(fast());
        setup.station.state().around = "mod alicebridge\nalicebridge wifi-stuck\nmaind on\n".into();
        setup.station.state().root_password = Some("1".into());
        let check = setup.service.check_with_password(ADDRESS, None, Some("1")).await.unwrap();
        assert_eq!(check.problem, None);
        let found: Vec<&str> = check.neighbours.iter().flatten().map(|n| n.id.as_str()).collect();
        assert_eq!(found, ["alicebridge-wifi", "alicebridge", "default-password"]);
        let json = serde_json::to_value(&check).unwrap();
        assert!(json["neighbours"][2].get("detail").is_none(), "the password itself never goes to the panel");
        // Changed to the owner's own: nothing more to say.
        setup.station.state().root_password = Some("s3cret-root".into());
        let check = setup.service.check_with_password(ADDRESS, None, Some("s3cret-root")).await.unwrap();
        assert!(check.neighbours.iter().flatten().all(|n| n.id != "default-password"));
    }

    #[tokio::test]
    async fn stops_at_a_station_whose_key_is_not_the_one_it_learned() {
        let setup = Setup::new(fast());
        setup.service.check(ADDRESS, None).await.unwrap();
        setup.station.state().key = "SHA256:another-key".into();
        assert_eq!(setup.service.check(ADDRESS, None).await.unwrap().problem.as_deref(), Some("host-key-changed"));
    }

    #[tokio::test]
    async fn gives_back_a_station_it_has_no_link_for_and_learns_its_key_at_the_next_look() {
        let setup = Setup::new(fast());
        // Pointed at a gateway by hand («Сделаю сам»): this gateway never looked at it.
        let by_hand = VoiceAddresses { url: "ws://192.0.2.10/uni.ws".into(), quic: String::new() };
        setup.station.state().copy = Some(with_addresses(&config_text(), &by_hand).unwrap());
        let back = setup.service.restore(ADDRESS, RestoreTo::Yandex, None).unwrap();
        assert_eq!(setup.ended(&back).await.status, JobStatus::Done);
        assert_eq!(read_config(&setup.station.state().config()).unwrap().addresses.url, YANDEX_URL);
        assert_eq!(setup.station.state().copy, None, "Yandex's are the firmware's own: our copy simply goes");
        // No key was learned, so no link is kept: one with an empty key would refuse every key as another station's.
        assert_eq!(setup.links().and_then(|links| links.get(ADDRESS).cloned()), None);
        let check = setup.service.check(ADDRESS, None).await.unwrap();
        assert_eq!(check.problem, None);
        assert_eq!(setup.link().fingerprint, "SHA256:fake-key");
    }

    #[tokio::test]
    async fn takes_a_link_kept_without_its_key_for_none() {
        let setup = Setup::new(fast());
        let keyless = StationLink {
            fingerprint: String::new(),
            original: None,
            previous: None,
            gateway: None,
            device: None,
            connected_at: None,
        };
        setup.settings.set(&STATION_LINKS, &BTreeMap::from([(ADDRESS.to_string(), keyless)]), None).unwrap();
        let check = setup.service.check(ADDRESS, None).await.unwrap();
        assert_eq!(check.problem, None, "an empty key pins nothing");
        assert_eq!(setup.link().fingerprint, "SHA256:fake-key");
    }

    #[tokio::test]
    async fn a_links_read_that_fails_is_an_error_not_a_station_without_a_key() {
        let setup = Setup::new(fast());
        setup.service.check(ADDRESS, None).await.unwrap();
        let looked = setup.station.state().commands.len();
        setup.db.with(|c| c.execute_batch("ALTER TABLE settings RENAME TO settings_away")).unwrap();
        // The look answers the server's error, as the TS check rejected, instead of opening with no key to hold to.
        let error = setup.service.check(ADDRESS, None).await.unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::INTERNAL_SERVER_ERROR);
        // The jobs stop before any SSH.
        let job = setup.service.connect(ADDRESS, &gateway(), None).unwrap();
        assert_eq!(setup.ended(&job).await.status, JobStatus::Failed);
        let back = setup.service.restore(ADDRESS, RestoreTo::Yandex, None).unwrap();
        assert_eq!(setup.ended(&back).await.status, JobStatus::Failed);
        assert_eq!(setup.station.state().commands.len(), looked, "nothing more ran on the station");
    }

    #[tokio::test]
    async fn changes_nothing_on_the_station_when_the_way_back_is_not_kept() {
        let setup = Setup::new(fast());
        // The disk is full: the links read, but nothing is written.
        let full =
            "CREATE TRIGGER full BEFORE INSERT ON settings BEGIN SELECT RAISE(ABORT, 'database or disk is full'); END;";
        setup.db.with(|c| c.execute_batch(full)).unwrap();
        let job = setup.service.connect(ADDRESS, &gateway(), None).unwrap();
        let failed = setup.ended(&job).await;
        assert_eq!(failed.status, JobStatus::Failed);
        assert_eq!(failed.steps[0], Step { id: StepId::Copy, state: StepState::Failed });
        let state = setup.station.state();
        assert_eq!((state.restore(), state.reboots), (None, 0), "no restore.sh, no reboot");
        assert_eq!(state.config(), config_text());
        assert_eq!(state.commands, vec![STEPS.read.clone(), STEPS.firmware.clone()], "only the reading");
    }

    #[tokio::test]
    async fn connects_keeping_the_way_back_then_waits_for_the_station() {
        let setup = Setup::new(fast());
        let job = setup.service.connect(ADDRESS, &gateway(), None).unwrap();
        assert_eq!(job.status, JobStatus::Running);
        until(|| setup.station.state().reboots == 1).await;
        assert_eq!(
            read_config(&setup.station.state().config()).unwrap().addresses,
            VoiceAddresses { url: "ws://192.0.2.10/uni.ws".into(), quic: String::new() }
        );
        assert_eq!(setup.station.state().firmware, config_text(), "the firmware's own file is not touched");
        let restore = crate::stations::connect::station_scripts::restore_script();
        assert_eq!(setup.station.state().restore(), Some(&restore));
        let voice = crate::stations::connect::station_scripts::VOICE_SCRIPT;
        assert!(setup.station.state().files.contains_key(voice), "voice.sh is on the station");
        assert_eq!(*setup.stations.expected.lock().unwrap(), vec![ADDRESS.to_string()]);
        setup.arrive("test-device");
        until(|| setup.service.job(&job.id).unwrap().status != JobStatus::Running).await;
        let done = setup.service.job(&job.id).unwrap();
        assert_eq!(done.status, JobStatus::Done);
        assert_eq!(done.device.as_deref(), Some("test-device"));
        assert_eq!(done.copy.as_deref(), Some("/data/opt/duowave/voice/quasar.cfg"));
        assert!(done.steps.iter().all(|step| step.state == StepState::Done));
        let link = setup.link();
        assert_eq!(link.original, Some(LinkAddresses { url: YANDEX_URL.into(), quic: YANDEX_QUIC_URL.into() }));
        assert_eq!(link.device.as_deref(), Some("test-device"));
        until(|| setup.switched.list().len() == 1).await;
        assert_eq!(
            setup.switched.list()[0],
            (ADDRESS.into(), "gateway".into(), "done".into(), "ws://192.0.2.10/uni.ws".into())
        );
    }

    #[tokio::test]
    async fn says_so_when_the_station_does_not_come_and_gives_it_back() {
        let setup = Setup::new(ConnectTiming { arrival_ms: 50, invitation_ms: 1000 });
        let job = setup.service.connect(ADDRESS, &gateway(), None).unwrap();
        until(|| setup.service.job(&job.id).unwrap().status != JobStatus::Running).await;
        let timed = setup.service.job(&job.id).unwrap();
        assert_eq!(timed.status, JobStatus::Timeout);
        assert_eq!(timed.error.as_deref(), Some("no-arrival"));

        let back = setup.service.restore(ADDRESS, RestoreTo::Yandex, None).unwrap();
        until(|| setup.service.job(&back.id).unwrap().status != JobStatus::Running).await;
        assert_eq!(setup.service.job(&back.id).unwrap().status, JobStatus::Done);
        assert_eq!(setup.station.state().config(), config_text());
        assert_eq!(setup.station.state().reboots, 2);
        until(|| setup.switched.list().len() == 2).await;
        let results: Vec<String> = setup.switched.list().iter().map(|entry| entry.2.clone()).collect();
        assert_eq!(results, vec!["timeout".to_string(), "done".into()]);
    }

    #[tokio::test]
    async fn uninstall_takes_duowave_off_checks_and_reboots() {
        let setup = Setup::new(fast());
        let job = setup.service.connect(ADDRESS, &gateway(), None).unwrap();
        setup.ended(&job).await;
        assert!(setup.station.state().copy.is_some() && setup.station.state().restore().is_some());
        let reboots = setup.station.state().reboots;

        let gone = setup.service.uninstall(ADDRESS, None).unwrap();
        assert_eq!(gone.kind, JobKind::Uninstall);
        let gone = setup.ended(&gone).await;
        assert_eq!(gone.status, JobStatus::Done, "{:?}", gone.error);
        assert!(gone.steps.iter().all(|step| step.state == StepState::Done), "{:?}", gone.steps);
        {
            let state = setup.station.state();
            assert_eq!(state.config(), config_text(), "the voice back with the firmware's own config");
            assert!(state.files.is_empty(), "our files gone");
            assert_eq!(state.reboots, reboots + 1);
        }
        // The key stays for «Подключить»; the gateway it went to does not.
        let link = setup.link();
        assert_eq!((link.fingerprint.as_str(), link.gateway, link.connected_at), ("SHA256:fake-key", None, None));
        until(|| setup.switched.list().iter().any(|entry| entry.1 == "removed")).await;
    }

    #[tokio::test]
    async fn uninstall_says_what_is_left_and_does_not_reboot() {
        let setup = Setup::new(fast());
        setup.station.state().left = "/etc/init.d/S95duowave\n".into();
        let gone = setup.service.uninstall(ADDRESS, None).unwrap();
        let gone = setup.ended(&gone).await;
        assert_eq!(gone.status, JobStatus::Failed);
        assert!(gone.error.as_deref().unwrap_or_default().contains("S95duowave"), "{:?}", gone.error);
        assert_eq!(setup.station.state().reboots, 0);
        let states: Vec<StepState> = gone.steps.iter().map(|step| step.state).collect();
        assert_eq!(states, vec![StepState::Done, StepState::Failed, StepState::Pending]);
    }

    #[tokio::test]
    async fn moving_a_station_from_another_gateway() {
        let setup = Setup::new(ConnectTiming { arrival_ms: 50, invitation_ms: 1000 });
        let other = VoiceAddresses { url: "ws://192.0.2.99/uni.ws".into(), quic: String::new() };
        setup.station.state().copy = Some(with_addresses(&config_text(), &other).unwrap());
        let job = setup.service.connect(ADDRESS, &gateway(), None).unwrap();
        until(|| setup.service.job(&job.id).unwrap().status != JobStatus::Running).await;
        assert_eq!(setup.service.job(&job.id).unwrap().status, JobStatus::Timeout);
        let link = setup.link();
        assert_eq!(link.previous, Some(other.clone()));
        assert_eq!(link.original, Some(LinkAddresses { url: YANDEX_URL.into(), quic: YANDEX_QUIC_URL.into() }));
        // restore.sh on the station is the way without a gateway.
        assert!(setup.station.state().restore().is_some());

        let back = setup.service.restore(ADDRESS, RestoreTo::Previous, None).unwrap();
        until(|| setup.service.job(&back.id).unwrap().status != JobStatus::Running).await;
        assert_eq!(read_config(&setup.station.state().config()).unwrap().addresses, other);

        let yandex = setup.service.restore(ADDRESS, RestoreTo::Yandex, None).unwrap();
        until(|| setup.service.job(&yandex.id).unwrap().status != JobStatus::Running).await;
        assert_eq!(setup.station.state().config(), config_text());
        until(|| setup.switched.list().len() == 3).await;
        let tos: Vec<String> = setup.switched.list().iter().map(|entry| entry.1.clone()).collect();
        assert_eq!(tos, vec!["gateway".to_string(), "gateway".into(), "yandex".into()]);
    }

    #[tokio::test]
    async fn undoes_a_write_that_did_not_land_without_a_reboot() {
        let setup = Setup::new(fast());
        {
            let mut state = setup.station.state();
            state.firmware = config_text().replace("\"apiKey\"", "\"vadEnabled\" : true,\n        \"apiKey\"");
            state.corrupt = true;
        }
        let job = setup.service.connect(ADDRESS, &gateway(), None).unwrap();
        until(|| setup.service.job(&job.id).unwrap().status != JobStatus::Running).await;
        let failed = setup.service.job(&job.id).unwrap();
        assert_eq!(failed.status, JobStatus::Failed);
        assert_eq!(
            failed.steps.iter().find(|step| step.id == StepId::Verify).map(|step| step.state),
            Some(StepState::Failed)
        );
        assert_eq!(read_config(&setup.station.state().config()).unwrap().addresses.url, YANDEX_URL);
        assert_eq!(setup.station.state().reboots, 0);
    }

    #[tokio::test]
    async fn says_the_old_addresses_are_back_only_when_the_undo_put_them_back() {
        for (undo, said) in [
            (Undo::Lost, "no answer in 20 s"),
            (
                Undo::Fails,
                "the written file is not the one expected, and the old addresses are not back: mount: permission denied",
            ),
        ] {
            let setup = Setup::new(fast());
            {
                let mut state = setup.station.state();
                state.firmware = config_text().replace("\"apiKey\"", "\"vadEnabled\" : true,\n        \"apiKey\"");
                state.corrupt = true;
                state.undo = undo;
            }
            let job = setup.service.connect(ADDRESS, &gateway(), None).unwrap();
            let failed = setup.ended(&job).await;
            assert_eq!(failed.status, JobStatus::Failed);
            assert_eq!(failed.error.as_deref(), Some(said));
            assert_eq!(setup.station.state().reboots, 0);
        }
    }

    /// SSH banners that take their time, as a port that drops packets does.
    struct SlowBanners;

    impl ShellOpener for SlowBanners {
        fn banner<'a>(&'a self, address: &'a str, _port: u16) -> BoxFuture<'a, Result<String, NoBanner>> {
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(1500)).await;
                Ok(format!("dropbear at {address}"))
            })
        }

        fn open<'a>(
            &'a self,
            _address: &'a str,
            _options: ShellOptions,
        ) -> BoxFuture<'a, anyhow::Result<Box<dyn StationShell>>> {
            Box::pin(async { anyhow::bail!("no shell here") })
        }
    }

    #[tokio::test(start_paused = true)]
    async fn asks_every_station_found_for_its_ssh_at_once() {
        let setup = Setup::with_opener(fast(), Some(Arc::new(SlowBanners)));
        let found = (1..=4)
            .map(|n| FoundStation { address: format!("192.0.2.{n}"), name: None, txt: BTreeMap::new(), ssh: None })
            .collect();
        let started = Instant::now();
        let probed = setup.service.with_banners(found).await;
        assert!(started.elapsed() < Duration::from_millis(3000), "one after another: {:?}", started.elapsed());
        let banners: Vec<Option<&str>> = probed.iter().map(|station| station.ssh.as_deref()).collect();
        assert_eq!(banners[3], Some("dropbear at 192.0.2.4"));
        assert_eq!(banners.len(), 4);
    }

    #[tokio::test]
    async fn offers_its_home_addresses_and_none_inside_a_container() {
        let setup = Setup::new(fast());
        let interfaces = vec![
            Iface { ipv4: true, internal: true, address: "127.0.0.1".into(), ..Default::default() },
            Iface { ipv4: true, internal: false, address: "192.0.2.10".into(), ..Default::default() },
            Iface { ipv4: true, internal: false, address: "192.168.7.20".into(), ..Default::default() },
            Iface { ipv4: false, internal: false, address: "fe80::1".into(), ..Default::default() },
        ];
        assert_eq!(setup.service.home_addresses(&interfaces, false), vec!["192.168.7.20".to_string()]);
        assert_eq!(setup.service.home_addresses(&interfaces, true), Vec::<String>::new());
    }

    /// Windows test 21.11: a PC with WSL, Hyper-V and a VPN. The Wi-Fi's address comes first — the wizard offers it to
    /// the station — whatever order the system lists the adapters in.
    #[tokio::test]
    async fn offers_the_home_adapter_before_wsl_hyper_v_and_a_vpn() {
        let setup = Setup::new(fast());
        let iface = |name: &str, address: &str| Iface {
            ipv4: true,
            internal: false,
            address: address.into(),
            name: name.into(),
            ..Default::default()
        };
        let interfaces = vec![
            iface("vEthernet (WSL (Hyper-V firewall))", "172.27.240.1"),
            iface("OpenVPN TAP-Windows6", "10.8.0.6"),
            iface("Ethernet 2", "172.20.1.15"),
            iface("Wi-Fi", "192.168.50.70"),
            iface("vEthernet (Default Switch)", "192.168.80.1"),
        ];
        assert_eq!(
            setup.service.home_addresses(&interfaces, false),
            ["192.168.50.70", "172.20.1.15", "192.168.80.1", "10.8.0.6", "172.27.240.1"]
        );
        assert!(virtual_adapter("vEthernet (WSL)") && virtual_adapter("utun3") && !virtual_adapter("Wi-Fi"));
    }

    /// Step 11 check: a Mac with the pretend stations' aliases on lo0 (10.77.0.x) listed before its Wi-Fi — the key
    /// sheet offered https://10.77.0.1:7781 first, and the hub could not reach it. The loopback adapter's addresses are
    /// never offered; a tunnel (point-to-point, whatever its name) and Parallels' adapters go after the home's own.
    #[test]
    fn leaves_out_the_loopbacks_aliases_and_puts_tunnels_last() {
        let iface = |name: &str, address: &str, internal: bool, tunnel: bool| Iface {
            ipv4: !address.contains(':'),
            internal,
            address: address.into(),
            name: name.into(),
            tunnel,
        };
        let interfaces = vec![
            iface("lo0", "127.0.0.1", true, false),
            iface("lo0", "::1", true, false),
            iface("lo0", "10.77.0.1", false, false),
            iface("lo0", "10.77.0.2", false, false),
            iface("wg0", "10.6.0.2", false, true),
            iface("vnic0", "10.211.55.2", false, false),
            iface("en0", "10.0.0.14", false, false),
        ];
        let ranked: Vec<&str> = home_ranked(&interfaces).iter().map(|entry| entry.address.as_str()).collect();
        assert_eq!(ranked, ["10.0.0.14", "10.6.0.2", "10.211.55.2"]);
    }

    #[tokio::test]
    async fn names_the_stations_that_dial_this_computer_on_an_old_port() {
        // The gateway listens on 80 here; it was on 7780 when one station was connected.
        let setup = Setup::new(fast());
        let link = |gateway: &str| StationLink {
            fingerprint: "SHA256:key".into(),
            original: None,
            previous: None,
            gateway: Some(gateway.into()),
            device: Some("lite2".into()),
            connected_at: None,
        };
        let links = BTreeMap::from([
            ("192.168.7.31".to_string(), link("ws://192.168.7.20:7780/uni.ws")),
            ("192.168.7.32".to_string(), link("ws://192.168.7.20/uni.ws")),
            ("192.168.7.33".to_string(), link("ws://192.168.7.33:8790/uni.ws")),
        ]);
        setup.settings.set(&STATION_LINKS, &links, None).unwrap();
        let interfaces =
            vec![Iface { ipv4: true, internal: false, address: "192.168.7.20".into(), ..Default::default() }];
        assert_eq!(
            setup.service.moved(&interfaces, false),
            vec![MovedGateway {
                address: "192.168.7.31".into(),
                device: Some("lite2".into()),
                was: "192.168.7.20:7780".into(),
                now: "192.168.7.20".into(),
            }],
            "the old port only; the right one and another gateway (the station's own) are not moved"
        );
        assert_eq!(setup.service.moved(&interfaces, true), Vec::new(), "a container cannot tell its host's address");
    }

    #[tokio::test]
    async fn runs_one_job_per_station_at_a_time() {
        let setup = Setup::new(fast());
        setup.service.connect(ADDRESS, &gateway(), None).unwrap();
        let second = setup.service.connect(ADDRESS, &gateway(), None);
        assert!(second.is_err());
        assert!(second.unwrap_err().message.contains("уже идёт"));
    }
}
