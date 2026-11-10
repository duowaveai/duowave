//! Moving the gateway between the computer and the station, from the computer's panel (docs/plans/station-placement.md,
//! section 4): a job of steps the panel follows, over the station's SSH as the connect wizard does it.
//!
//! - **To the station** (alone or with this computer as its home server): the station looked at (aarch64, 30 MB free
//!   in /data, memory), the way back kept (restore.sh, the station's link), this computer's station build put in
//!   place and its own `duowave station install` run, the settings sent, the service started, the voice pointed at
//!   the station's own gateway and the station rebooted, then waited for — its gateway answering with the station in.
//! - **To the computer**: the settings taken from the station when they lived there, the voice pointed back here, the
//!   station rebooted and waited for at /uni.ws, the station's gateway stopped. This gateway then starts again on the
//!   settings it took.
//! - **«Вернуть как было»**: the station back to Yandex, its gateway stopped; nothing of ours answers it.
//!
//! The station changes only at the household's press, every move is in «Кто что менял», and quasar.cfg is changed by
//! one at a time (the station's lock).

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tokio::sync::broadcast;
use tokio::time::{timeout_at, Instant};

use super::payload;
use super::peer::PeerClient;
use super::service::Restarter;
use super::station_fs::{BIN, DIR, SERVICE, SERVICE_COMMAND};
use super::transfer;
use super::watch::{key_of, StationGateway, StationGateways};
use crate::access::AccessService;
use crate::config::{Config, TtsProvider};
use crate::core::availability::Placement;
use crate::core::events::{Actor, EventBus, GatewayEvent};
use crate::platform::http::ApiError;
use crate::secrets::SecretsService;
use crate::settings::definitions::{
    SettingsHome, StationLink, StationPlacement, StationSelf, STATION_LINKS, VOICE_PROVIDER,
};
use crate::settings::SettingsService;
use crate::stations::connect::quasar_config::{
    gateway_addresses, points_to, read_config, yandex, GatewayHost, PointsTo, VoiceAddresses,
};
use crate::stations::connect::station_connect::{problem_of, ShellConfig, Stations};
use crate::stations::connect::station_scripts::{deliveries, switch_to, STEPS};
use crate::stations::connect::station_shell::{ShellOpener, ShellOptions, StationShell};
use crate::storage::Db;
use crate::utils::time::now_iso;

/// Free space a station needs in /data for its gateway: the program (12 MB), its update beside it, the database and
/// a journal of 10 MB.
pub const NEEDED_KB: u64 = 30 * 1024;
/// Memory the station should have free for a gateway that holds ~10 MB.
pub const NEEDED_MEMORY_KB: u64 = 20 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MoveTo {
    Station,
    StationServer,
    Pc,
    /// «Вернуть как было»: the station as before any gateway.
    Yandex,
}

impl MoveTo {
    pub fn as_str(self) -> &'static str {
        match self {
            MoveTo::Station => "station",
            MoveTo::StationServer => "stationServer",
            MoveTo::Pc => "pc",
            MoveTo::Yandex => "yandex",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MoveStep {
    /// The station looked at: SSH, its machine, space, memory, where its voice goes.
    Check,
    /// The way back kept: the station's link and restore.sh.
    Copy,
    /// This computer's station build put in place and installed.
    Install,
    /// The settings sent (or taken back).
    Settings,
    /// The voice pointed, the station rebooted.
    Point,
    /// The station back and talking to its gateway.
    Arrive,
    /// The station's gateway stopped.
    Stop,
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
pub enum MoveStatus {
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Step {
    pub id: MoveStep,
    pub state: StepState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveJob {
    pub id: String,
    pub to: MoveTo,
    pub address: String,
    pub status: MoveStatus,
    pub steps: Vec<Step>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// What the household should know as it goes: the port taken, the restart to come.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub started_at: String,
}

/// What a move asks.
#[derive(Debug, Clone)]
pub struct MoveRequest {
    pub to: MoveTo,
    pub address: String,
    /// This computer as the station dials it: for «на компьютере» and as the home server.
    pub gateway: Option<GatewayHost>,
    pub settings_home: SettingsHome,
    /// To the computer: take the station's gateway off it (its data kept), not only stop it.
    pub remove: bool,
}

/// How long things may take.
#[derive(Debug, Clone, Copy)]
pub struct MoveTiming {
    /// The station's gateway answering after its start.
    pub start_ms: u64,
    /// The station back after its reboot, its voice at the gateway.
    pub arrival_ms: u64,
    pub poll_ms: u64,
}

impl Default for MoveTiming {
    fn default() -> MoveTiming {
        MoveTiming { start_ms: 60_000, arrival_ms: 3 * 60_000, poll_ms: 3000 }
    }
}

/// Where the station's build comes from: this computer's (payload::load); a pretend one in the tests.
pub type PayloadLoader =
    Arc<dyn Fn(Arc<Config>) -> futures::future::BoxFuture<'static, Result<payload::Payload, String>> + Send + Sync>;

pub fn this_computers_build() -> PayloadLoader {
    Arc::new(|config: Arc<Config>| Box::pin(async move { payload::load(&config).await }))
}

pub struct MovesDeps {
    pub config: Arc<Config>,
    pub db: Arc<Db>,
    pub settings: Arc<SettingsService>,
    pub secrets: Arc<SecretsService>,
    pub events: EventBus,
    pub access: Arc<AccessService>,
    pub opener: Arc<dyn ShellOpener>,
    pub ssh: ShellConfig,
    /// The computer's watches over the stations' gateways, one per station.
    pub station: Arc<StationGateways>,
    pub stations: Arc<dyn Stations>,
    pub restarter: Restarter,
    pub timing: MoveTiming,
    pub payload: PayloadLoader,
}

pub struct MovesService {
    deps: MovesDeps,
    jobs: Mutex<HashMap<String, MoveJob>>,
}

impl MovesService {
    pub fn new(deps: MovesDeps) -> Arc<MovesService> {
        Arc::new(MovesService { deps, jobs: Mutex::new(HashMap::new()) })
    }

    fn jobs(&self) -> std::sync::MutexGuard<'_, HashMap<String, MoveJob>> {
        self.jobs.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn job(&self, id: &str) -> Result<MoveJob, ApiError> {
        self.jobs().get(id).cloned().ok_or_else(|| ApiError::not_found("Нет такого перехода."))
    }

    /// The move going now, if any: the panel shows it after a reload.
    pub fn running(&self) -> Option<MoveJob> {
        self.jobs().values().find(|job| job.status == MoveStatus::Running).cloned()
    }

    /// Starts a move; the panel follows it by its id.
    pub fn start(self: &Arc<Self>, request: MoveRequest, actor: Option<Actor>) -> Result<MoveJob, ApiError> {
        if matches!(request.to, MoveTo::Pc | MoveTo::StationServer) && request.gateway.is_none() {
            return Err(ApiError::bad_request("Нужен адрес этого компьютера, как его видит колонка."));
        }
        if matches!(request.to, MoveTo::Station | MoveTo::StationServer)
            && payload::info(&self.deps.config).source == payload::PayloadSource::None
        {
            return Err(ApiError::conflict(payload::info(&self.deps.config).note));
        }
        let steps: &[MoveStep] = match request.to {
            MoveTo::Station | MoveTo::StationServer => &[
                MoveStep::Check,
                MoveStep::Copy,
                MoveStep::Install,
                MoveStep::Settings,
                MoveStep::Point,
                MoveStep::Arrive,
            ],
            MoveTo::Pc => &[MoveStep::Check, MoveStep::Settings, MoveStep::Point, MoveStep::Arrive, MoveStep::Stop],
            MoveTo::Yandex => &[MoveStep::Check, MoveStep::Point, MoveStep::Stop],
        };
        let job = {
            let mut jobs = self.jobs();
            if jobs.values().any(|job| job.status == MoveStatus::Running) {
                return Err(ApiError::conflict("Переход уже идёт."));
            }
            let job = MoveJob {
                id: crate::utils::ids::uuid(),
                to: request.to,
                address: request.address.clone(),
                status: MoveStatus::Running,
                steps: steps.iter().map(|id| Step { id: *id, state: StepState::Pending }).collect(),
                error: None,
                note: None,
                started_at: now_iso(),
            };
            let now = crate::utils::time::now_ms();
            jobs.retain(|_, old| {
                now.saturating_sub(parse_ms(&old.started_at)) <= 3_600_000 || old.status == MoveStatus::Running
            });
            jobs.insert(job.id.clone(), job.clone());
            job
        };
        self.audit(&job.id, "started", actor.clone());
        let me = self.clone();
        let id = job.id.clone();
        tokio::spawn(async move { me.run(id, request, actor).await });
        Ok(job)
    }

    fn step(&self, id: &str, step: MoveStep, state: StepState) {
        if let Some(job) = self.jobs().get_mut(id) {
            if let Some(entry) = job.steps.iter_mut().find(|entry| entry.id == step) {
                entry.state = state;
            }
        }
    }

    fn note(&self, id: &str, note: &str) {
        if let Some(job) = self.jobs().get_mut(id) {
            job.note = Some(note.to_string());
        }
    }

    fn audit(&self, id: &str, result: &str, actor: Option<Actor>) {
        let job = self.jobs().get(id).cloned();
        self.deps.events.emit(GatewayEvent::PlacementMoved {
            to: job.as_ref().map(|job| job.to.as_str().to_string()).unwrap_or_default(),
            result: result.into(),
            address: job.as_ref().map(|job| job.address.clone()),
            error: job.and_then(|job| job.error),
            actor,
        });
    }

    async fn run(self: Arc<Self>, id: String, request: MoveRequest, actor: Option<Actor>) {
        let mut shell: Option<Box<dyn StationShell>> = None;
        let outcome = match request.to {
            MoveTo::Station | MoveTo::StationServer => self.to_station(&id, &request, actor.clone(), &mut shell).await,
            MoveTo::Pc => self.to_pc(&id, &request, actor.clone(), &mut shell).await,
            MoveTo::Yandex => self.to_yandex(&id, &request, actor.clone(), &mut shell).await,
        };
        if let Some(shell) = shell.take() {
            shell.close().await;
        }
        let restart = match outcome {
            Ok(restart) => {
                if let Some(job) = self.jobs().get_mut(&id) {
                    job.status = MoveStatus::Done;
                }
                self.audit(&id, "done", actor);
                restart
            }
            Err(error) => {
                let problem = problem_of(&error);
                if let Some(job) = self.jobs().get_mut(&id) {
                    if let Some(running) = job.steps.iter_mut().find(|step| step.state == StepState::Running) {
                        running.state = StepState::Failed;
                    }
                    job.status = MoveStatus::Failed;
                    job.error = Some(problem.clone());
                }
                tracing::warn!("placement: the move to {} failed: {problem}", request.to.as_str());
                self.audit(&id, "failed", actor);
                false
            }
        };
        self.deps.events.emit(GatewayEvent::PlacementChanged { what: "move".into() });
        if restart {
            (self.deps.restarter)();
        }
    }

    fn links(&self) -> anyhow::Result<BTreeMap<String, StationLink>> {
        let mut links = self.deps.settings.try_entry(&STATION_LINKS)?.map(|entry| entry.value).unwrap_or_default();
        links.retain(|_, link| !link.fingerprint.is_empty());
        Ok(links)
    }

    fn keep_link(&self, address: &str, edit: impl FnOnce(&mut StationLink)) -> anyhow::Result<()> {
        let mut links = self.links()?;
        let Some(link) = links.get_mut(address) else { return Ok(()) };
        edit(link);
        self.deps
            .settings
            .set(&STATION_LINKS, &links, None)
            .map_err(|_| anyhow::anyhow!("the station link is not saved: the gateway's database failed"))
    }

    /// The station's shell, held to the key learned before; a first look learns it, as the connect wizard does.
    async fn open(&self, address: &str) -> anyhow::Result<Box<dyn StationShell>> {
        let trust = self.links()?.get(address).map(|link| link.fingerprint.clone());
        let options = ShellOptions {
            port: self.deps.ssh.port,
            user: self.deps.ssh.user.clone(),
            password: self.deps.ssh.password.clone(),
            trust: trust.clone(),
            timeout_ms: None,
        };
        let shell = self.deps.opener.open(address, options).await?;
        if trust.is_none() {
            let mut links = self.links()?;
            links.insert(
                address.to_string(),
                StationLink {
                    fingerprint: shell.fingerprint().to_string(),
                    original: None,
                    previous: None,
                    gateway: None,
                    device: None,
                    connected_at: None,
                },
            );
            self.deps.settings.set(&STATION_LINKS, &links, None).map_err(|error| anyhow::anyhow!(error.message))?;
        }
        Ok(shell)
    }

    /// voice.sh, the boot script's source and restore.sh onto the station.
    async fn deliver(&self, shell: &dyn StationShell) -> anyhow::Result<()> {
        for (command, input) in deliveries() {
            self.run_ok(shell, &command, Some(&input)).await?;
        }
        Ok(())
    }

    async fn run_ok(&self, shell: &dyn StationShell, command: &str, input: Option<&str>) -> anyhow::Result<String> {
        let result = shell.run(command, input).await?;
        if result.code != 0 {
            let said = if result.stderr.trim().is_empty() { result.stdout.trim() } else { result.stderr.trim() };
            anyhow::bail!("{}", if said.is_empty() { format!("ended with {}", result.code) } else { said.to_string() });
        }
        Ok(result.stdout)
    }

    /// The station looked at: what it is, how much room, where its voice goes. Changes nothing.
    async fn check(&self, shell: &dyn StationShell, to_station: bool) -> anyhow::Result<VoiceAddresses> {
        let config = self.run_ok(shell, &STEPS.read, None).await?;
        let facts = read_config(&config)?;
        if to_station {
            let machine = self.run_ok(shell, "uname -m", None).await?;
            if machine.trim() != "aarch64" {
                anyhow::bail!("колонка — {}, а сборка для колонки — aarch64", machine.trim());
            }
            let space = self.run_ok(shell, "df -k /data | tail -n 1", None).await?;
            let free = space.split_whitespace().nth(3).and_then(|kb| kb.parse::<u64>().ok()).unwrap_or(0);
            if free < NEEDED_KB {
                anyhow::bail!("в /data свободно {} МБ, а шлюзу нужно {} МБ", free / 1024, NEEDED_KB / 1024);
            }
            let memory = self.run_ok(shell, "grep MemAvailable /proc/meminfo", None).await?;
            let available = memory.split_whitespace().nth(1).and_then(|kb| kb.parse::<u64>().ok()).unwrap_or(u64::MAX);
            if available < NEEDED_MEMORY_KB {
                anyhow::bail!(
                    "у колонки свободно {} МБ памяти, а шлюзу нужно {} МБ",
                    available / 1024,
                    NEEDED_MEMORY_KB / 1024
                );
            }
        }
        Ok(facts.addresses)
    }

    async fn to_station(
        &self,
        id: &str,
        request: &MoveRequest,
        actor: Option<Actor>,
        shell: &mut Option<Box<dyn StationShell>>,
    ) -> anyhow::Result<bool> {
        let address = request.address.as_str();
        let watch = self.watch_for(address);
        self.step(id, MoveStep::Check, StepState::Running);
        *shell = Some(self.open(address).await?);
        let now = self.check(shell.as_deref().unwrap(), true).await?;
        let payload = (self.deps.payload)(self.deps.config.clone()).await.map_err(|error| anyhow::anyhow!(error))?;
        let program = payload.program().map_err(|error| anyhow::anyhow!(error))?;
        self.step(id, MoveStep::Check, StepState::Done);

        // The way back, before anything changes: what the firmware's own file says (under our copy, when one is
        // mounted), and voice.sh with restore.sh on the station.
        self.step(id, MoveStep::Copy, StepState::Running);
        let firmware = self.run_ok(shell.as_deref().unwrap(), &STEPS.firmware, None).await?;
        let original = read_config(&firmware)?.addresses;
        self.keep_link(address, |link| {
            link.original = Some(original.clone());
            link.previous = Some(now.clone());
        })?;
        self.deliver(shell.as_deref().unwrap()).await?;
        self.step(id, MoveStep::Copy, StepState::Done);

        self.step(id, MoveStep::Install, StepState::Running);
        // The station's own key: the one it was linked or moved with, else a new one, kept once the station has it —
        // never the computer's one key, which every station moved before opens with (review S2). Its watch opens it
        // with the same.
        let own = watch.own_key();
        let key = own.clone().unwrap_or_else(super::peer::new_key);
        let mut home = None;
        // The computer's address and its key travel on every move to the station: alone, the station keeps them
        // off, and «ПК помогает колонке» is a switch later — no addresses, no keys (decision 7 of
        // docs/plans/duowave-placement-ui.md).
        if let Some(gateway) = request.gateway.as_ref() {
            let created = self
                .deps
                .access
                .keys
                .create(
                    &Map::from_iter([
                        ("name".into(), json!("Колонка: домашний сервер")),
                        ("grants".into(), json!(["home.models"])),
                    ]),
                    actor.clone(),
                )
                .map_err(|error| anyhow::anyhow!(error.message))?;
            home = Some((format!("http://{}:{}", gateway.host, gateway.port), created.secret, created.key.id));
        }
        let shell_ref = shell.as_deref().unwrap();
        self.run_ok(shell_ref, &format!("mkdir -p {DIR}/bin"), None).await?;
        shell_ref.upload(BIN, &program).await?;
        self.run_ok(shell_ref, &format!("chmod 755 {BIN}"), None).await?;
        let mut install = format!("{BIN} station install --managed pc --no-start --peer-key {key}");
        if let Some((url, secret, _)) = &home {
            install.push_str(&format!(" --home-server {url} --key {secret}"));
        }
        self.run_ok(shell_ref, &install, None).await?;
        if own.is_none() {
            watch.keep_key(&key).map_err(|error| anyhow::anyhow!(error.message))?;
        }
        let status: Value =
            serde_json::from_str(&self.run_ok(shell_ref, &format!("{BIN} station status --json"), None).await?)
                .unwrap_or(Value::Null);
        let port = status.get("port").and_then(Value::as_u64).unwrap_or(super::station_fs::DEFAULT_PORT as u64) as u16;
        if port != super::station_fs::DEFAULT_PORT {
            self.note(id, &format!("Порт {} на колонке занят: шлюз слушает {port}.", super::station_fs::DEFAULT_PORT));
        }
        self.step(id, MoveStep::Install, StepState::Done);

        // The settings go before its first start: it starts on them.
        self.step(id, MoveStep::Settings, StepState::Running);
        let mut bundle = transfer::export(&self.deps.db, &self.deps.secrets, "pc")?;
        bundle.revision = transfer::revision(&self.deps.db, &self.deps.secrets)?;
        // The mode in force goes too, the one .env set included: the station starts in it (live, 07.11 — a mode only
        // in .env did not travel, the station started in «Яндекс»). Only a move carries it (`transfer::MOVE_ONLY`):
        // from then on the station switches its own. A whole package (`transfer::WHOLE`): the program just put there
        // imports it at its first start, and a build older than this computer's (a file, the release of this version
        // number) takes it too.
        let mode =
            self.deps.settings.get(&crate::settings::definitions::MODE, None).map(|setting| setting.mode).unwrap_or(
                match self.deps.config.gateway.mode {
                    crate::config::UniproxyMode::Gateway => crate::settings::definitions::PanelMode::Gateway,
                    crate::config::UniproxyMode::Relay => crate::settings::definitions::PanelMode::Relay,
                },
            );
        bundle.settings.insert("gateway_mode".into(), json!({ "mode": mode.as_str() }));
        // The plugins' switches and their data go with the gateway: they live where it runs (transfer.rs), and the
        // station takes them for the packages it has too (live, 10.11: «Список покупок» was off there, its list here).
        bundle.plugins = Some(transfer::plugins(&self.deps.db)?);
        // With this computer as its server the station speaks this computer's voice as this computer does — in
        // «Шлюз» too (live, 10.11: Alice's voice there, the station's own choice was «yandex»).
        if request.to == MoveTo::StationServer {
            if let Some(provider) = self.voice_given() {
                bundle.settings.insert("tts_provider".into(), json!(provider.as_str()));
            }
        }
        // The station's name as this computer knows it: its own windows say it, not «Колонка».
        let device = self.deps.stations.device_for_address(address);
        let name = device.as_deref().and_then(|device| self.deps.stations.find(device)).map(|station| station.name);
        if device.is_some() || name.is_some() {
            bundle.settings.insert("station_self".into(), serde_json::to_value(StationSelf { device, name })?);
        }
        let text = serde_json::to_string(&bundle)?;
        self.run_ok(shell_ref, &format!("{BIN} station receive-settings"), Some(&text)).await?;
        self.run_ok(shell_ref, &format!("{SERVICE_COMMAND} start"), None).await?;
        let client = PeerClient::new(address, port, &key);
        self.wait(self.deps.timing.start_ms, || async { client.health().await.is_ok() })
            .await
            .ok_or_else(|| anyhow::anyhow!("шлюз на колонке не запустился: duowave station log"))?;
        if request.to == MoveTo::Station && home.is_some() {
            // Alone: the computer stays known, the station does not lean on it.
            client
                .post("/api/v1/home-server/link", json!({ "enabled": false }))
                .await
                .map_err(|error| anyhow::anyhow!("помощь ПК не выключилась: {error}"))?;
        }
        self.step(id, MoveStep::Settings, StepState::Done);

        self.step(id, MoveStep::Point, StepState::Running);
        self.run_ok(shell_ref, &format!("{BIN} station point local --reboot"), None).await?;
        if let Some(shell) = shell.take() {
            shell.close().await;
        }
        self.step(id, MoveStep::Point, StepState::Done);

        self.step(id, MoveStep::Arrive, StepState::Running);
        let record = StationPlacement {
            placement: if request.to == MoveTo::StationServer { Placement::StationServer } else { Placement::Station },
            address: address.to_string(),
            port,
            device: self.deps.stations.device_for_address(address),
            settings_home: request.settings_home,
            auto_update: false,
            managed: true,
            home_key_id: home.map(|(_, _, id)| id),
            moved_at: Some(now_iso()),
        };
        watch.set_record(Some(&record), actor).map_err(|error| anyhow::anyhow!(error.message))?;
        let arrived = self
            .wait(self.deps.timing.arrival_ms, || async {
                client.health().await.ok().and_then(|health| health.pointer("/station/connected")?.as_bool())
                    == Some(true)
            })
            .await;
        if arrived.is_none() {
            anyhow::bail!("колонка не вернулась к своему шлюзу за 3 минуты: «Вернуть как было» вернёт её Яндексу");
        }
        // Where the main settings live from now: the station's gateway is told (it keeps them, or takes them).
        let home = if request.settings_home == SettingsHome::Pc { "pc" } else { "station" };
        let _ = client.post("/api/v1/placement/settings-home", json!({ "home": home })).await;
        self.step(id, MoveStep::Arrive, StepState::Done);
        watch.poll().await;
        Ok(false)
    }

    async fn to_pc(
        &self,
        id: &str,
        request: &MoveRequest,
        actor: Option<Actor>,
        shell: &mut Option<Box<dyn StationShell>>,
    ) -> anyhow::Result<bool> {
        let address = request.address.as_str();
        let watch = self.watch_for(address);
        let gateway = request.gateway.clone().expect("checked at the start");
        self.step(id, MoveStep::Check, StepState::Running);
        *shell = Some(self.open(address).await?);
        self.check(shell.as_deref().unwrap(), false).await?;
        let ours =
            self.run_ok(shell.as_deref().unwrap(), &format!("[ -x {BIN} ] && echo ours || echo none"), None).await?;
        self.step(id, MoveStep::Check, StepState::Done);

        // The settings come back when they lived on the station, the plugins' switches and data always — they live
        // where the gateway runs: applied at this gateway's restart.
        self.step(id, MoveStep::Settings, StepState::Running);
        let record = watch.record();
        let mut restart = false;
        match (record.as_ref().map(|record| record.settings_home), watch.client()) {
            (Some(SettingsHome::Station), Some(client)) => {
                let bundle = client.settings_and_plugins().await.map_err(|error| anyhow::anyhow!(error))?;
                // Sealed with the key the station opens with now, as its other copies are. A copy is the settings':
                // its restore leaves the plugins' data as it is then.
                let key = watch.pair_key().ok_or_else(|| anyhow::anyhow!("нет ключа пары"))?;
                let copy = transfer::Bundle { plugins: None, ..bundle.clone() };
                let _ = watch.backups.save(&transfer::seal(&copy, &key), bundle.revision);
                let dir =
                    self.deps.config.skills.state_dir.clone().ok_or_else(|| anyhow::anyhow!("no state folder"))?;
                std::fs::write(transfer::incoming_file(&dir), serde_json::to_vec(&bundle)?)?;
                restart = true;
                self.note(id, "Настройки взяты с колонки: шлюз перезапустится на них.");
                self.step(id, MoveStep::Settings, StepState::Done);
            }
            (Some(SettingsHome::Pc), Some(client)) => {
                // The settings live here already; the plugins' state comes back alone. Not taken (the station silent,
                // a build before it): the move goes on, the plugins' data stays on the station.
                let taken = match client.settings_and_plugins().await {
                    Ok(bundle) => self.take_plugins(bundle).map_err(|error| error.to_string()),
                    Err(error) => Err(error),
                };
                match taken {
                    Ok(true) => {
                        restart = true;
                        self.note(id, "Плагины и их данные взяты с колонки: шлюз перезапустится на них.");
                        self.step(id, MoveStep::Settings, StepState::Done);
                    }
                    Ok(false) => self.step(id, MoveStep::Settings, StepState::Skipped),
                    Err(error) => {
                        tracing::warn!("placement: the plugins' state stays on the station: {error}");
                        self.note(id, &format!("Данные плагинов остались на колонке: {error}"));
                        self.step(id, MoveStep::Settings, StepState::Skipped);
                    }
                }
            }
            _ => self.step(id, MoveStep::Settings, StepState::Skipped),
        }

        self.step(id, MoveStep::Point, StepState::Running);
        let target = gateway_addresses(&gateway, &self.deps.config.gateway.uniproxy_path);
        self.deps.stations.expect(address, 5 * 60_000);
        let receiver = self.deps.events.subscribe();
        if ours.trim() == "ours" {
            let host =
                if gateway.port == 80 { gateway.host.clone() } else { format!("{}:{}", gateway.host, gateway.port) };
            self.run_ok(shell.as_deref().unwrap(), &format!("{BIN} station point {host} --reboot"), None).await?;
        } else {
            self.deliver(shell.as_deref().unwrap()).await?;
            self.run_ok(shell.as_deref().unwrap(), &switch_to(&target, false)?, None).await?;
            let _ = shell.as_deref().unwrap().run(STEPS.reboot, None).await;
        }
        if let Some(shell) = shell.take() {
            shell.close().await;
        }
        self.keep_link(address, |link| {
            link.gateway = Some(target.url.clone());
            link.connected_at = None;
        })?;
        self.step(id, MoveStep::Point, StepState::Done);

        self.step(id, MoveStep::Arrive, StepState::Running);
        let device = self.await_arrival(receiver, address).await;
        if device.is_none() {
            anyhow::bail!("колонка не пришла к этому шлюзу за 3 минуты");
        }
        self.step(id, MoveStep::Arrive, StepState::Done);

        self.step(id, MoveStep::Stop, StepState::Running);
        if ours.trim() == "ours" {
            *shell = Some(self.open(address).await?);
            let command = if request.remove {
                format!("{BIN} station remove --keep-data")
            } else {
                format!("{SERVICE_COMMAND} stop")
            };
            self.run_ok(shell.as_deref().unwrap(), &command, None).await?;
            self.step(id, MoveStep::Stop, StepState::Done);
        } else {
            self.step(id, MoveStep::Stop, StepState::Skipped);
        }
        let record = StationPlacement {
            placement: Placement::Pc,
            address: address.to_string(),
            port: record.as_ref().map(|record| record.port).unwrap_or(super::station_fs::DEFAULT_PORT),
            device,
            settings_home: SettingsHome::Station,
            auto_update: false,
            managed: false,
            home_key_id: None,
            moved_at: Some(now_iso()),
        };
        if let Some(key) = watch.record().and_then(|record| record.home_key_id) {
            let _ = self.deps.access.keys.revoke(&key, actor.clone());
        }
        watch.set_record(Some(&record), actor).map_err(|error| anyhow::anyhow!(error.message))?;
        Ok(restart)
    }

    async fn to_yandex(
        &self,
        id: &str,
        request: &MoveRequest,
        actor: Option<Actor>,
        shell: &mut Option<Box<dyn StationShell>>,
    ) -> anyhow::Result<bool> {
        let address = request.address.as_str();
        let watch = self.watch_for(address);
        self.step(id, MoveStep::Check, StepState::Running);
        *shell = Some(self.open(address).await?);
        let now = self.check(shell.as_deref().unwrap(), false).await?;
        let ours =
            self.run_ok(shell.as_deref().unwrap(), &format!("[ -x {BIN} ] && echo ours || echo none"), None).await?;
        self.step(id, MoveStep::Check, StepState::Done);

        self.step(id, MoveStep::Point, StepState::Running);
        // As «Вернуть к Яндексу» does: the station's own addresses when they are Yandex's, else Yandex's.
        let original = self
            .links()?
            .get(address)
            .and_then(|link| link.original.clone())
            .filter(|own| points_to(&own.url) == PointsTo::Yandex)
            .unwrap_or_else(yandex);
        if now == original {
            self.step(id, MoveStep::Point, StepState::Skipped);
        } else {
            if ours.trim() == "ours" {
                // Its own gateway goes off first: nothing starts it before the reboot.
                let _ = self.run_ok(shell.as_deref().unwrap(), &format!("{SERVICE_COMMAND} stop"), None).await;
            }
            self.deliver(shell.as_deref().unwrap()).await?;
            self.run_ok(shell.as_deref().unwrap(), &switch_to(&original, true)?, None).await?;
            let written = read_config(&self.run_ok(shell.as_deref().unwrap(), &STEPS.read, None).await?)?;
            if written.addresses != original {
                anyhow::bail!("the written file is not the one expected");
            }
            self.step(id, MoveStep::Point, StepState::Done);
        }
        self.step(id, MoveStep::Stop, StepState::Running);
        if ours.trim() == "ours" {
            self.run_ok(shell.as_deref().unwrap(), &format!("{BIN} station remove --keep-data"), None).await?;
            self.step(id, MoveStep::Stop, StepState::Done);
        } else {
            self.step(id, MoveStep::Stop, StepState::Skipped);
        }
        if now != original {
            let _ = shell.as_deref().unwrap().run(STEPS.reboot, None).await;
        }
        self.keep_link(address, |link| {
            link.gateway = None;
            link.connected_at = None;
        })?;
        if let Some(key) = watch.record().and_then(|record| record.home_key_id) {
            let _ = self.deps.access.keys.revoke(&key, actor.clone());
        }
        watch.set_record(None, actor).map_err(|error| anyhow::anyhow!(error.message))?;
        Ok(false)
    }

    /// This computer's choice of voice when it gives a voice of its own (`own`, `mixed`), as it speaks now: what the
    /// station speaks with while this computer is its server. None: Alice's here — the station keeps its own choice.
    fn voice_given(&self) -> Option<TtsProvider> {
        let stored = self.deps.settings.get(&VOICE_PROVIDER, None).and_then(|provider| TtsProvider::parse(&provider));
        let provider = crate::speech::voice_service::provider_at_start(&self.deps.config.gateway.tts, stored);
        matches!(provider, TtsProvider::Own | TtsProvider::Mixed).then_some(provider)
    }

    /// The station's plugins' state, left for this gateway's restart when it carries any: true then.
    fn take_plugins(&self, bundle: transfer::Bundle) -> anyhow::Result<bool> {
        let Some(plugins) = bundle.plugins.filter(|plugins| !plugins.is_empty()) else { return Ok(false) };
        let dir = self.deps.config.skills.state_dir.clone().ok_or_else(|| anyhow::anyhow!("no state folder"))?;
        let package = transfer::PluginsPackage { from: bundle.from, at: bundle.at, plugins };
        super::station_fs::write_atomic(&transfer::incoming_plugins_file(&dir), &serde_json::to_vec(&package)?)
            .map_err(|error| anyhow::anyhow!(error))?;
        Ok(true)
    }

    /// Asks `check` until it says yes or the time is up.
    async fn wait<F, Fut>(&self, ms: u64, mut check: F) -> Option<()>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        let deadline = Instant::now() + Duration::from_millis(ms);
        loop {
            if check().await {
                return Some(());
            }
            if Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(self.deps.timing.poll_ms)).await;
        }
    }

    /// The device that comes to /uni.ws from this address next, or None when none does in time.
    async fn await_arrival(
        &self,
        mut receiver: broadcast::Receiver<Arc<GatewayEvent>>,
        address: &str,
    ) -> Option<String> {
        let deadline = Instant::now() + Duration::from_millis(self.deps.timing.arrival_ms);
        loop {
            match timeout_at(deadline, receiver.recv()).await {
                Ok(Ok(event)) => {
                    if let GatewayEvent::StationConnected { device_tag, .. } = &*event {
                        if self.deps.stations.find(device_tag).and_then(|station| station.last_address).as_deref()
                            == Some(address)
                        {
                            return Some(device_tag.clone());
                        }
                    }
                }
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
                _ => return None,
            }
        }
    }

    /// «Проверить»: the station's own account of its gateway, over SSH — by its shell, not the program: after the
    /// safety net the program may be the very thing that does not start (live, 07.11). The service, the safety
    /// net's marks, where the voice goes, the end of the gateway's log; the program's own status when it answers.
    pub async fn inspect(&self, address: &str) -> Result<Value, String> {
        let shell = self.open(address).await.map_err(|error| problem_of(&error))?;
        let marks = format!(
            "[ -f {SERVICE} ] && echo installed; [ -f {DIR}/disabled ] && echo disabled; \
             [ -f {DIR}/safety-fired ] && echo \"fired $(cat {DIR}/safety-fired)\"; \
             grep -q '\"uniProxyUrl\" *: *\"ws://127\\.0\\.0\\.1:' {config} && echo local; \
             [ -f {SERVICE} ] && {SERVICE_COMMAND} status",
            config = crate::stations::connect::quasar_config::STATION_CONFIG,
        );
        let found = shell.run(&marks, None).await;
        let log = shell.run(&format!("tail -n 40 {}/gateway.log 2>/dev/null", super::station_fs::DATA), None).await;
        let status = shell.run(&format!("{BIN} station status --json 2>/dev/null"), None).await;
        shell.close().await;
        let found = found.map_err(|error| problem_of(&error))?.stdout;
        let lines: Vec<&str> = found.lines().map(str::trim).collect();
        let fired = lines.iter().find_map(|line| line.strip_prefix("fired ")).map(|what| {
            match what.split_whitespace().next() {
                Some("previous-version") => "новая версия не поднялась — вернулась прежняя".to_string(),
                Some("previous-address") => "шлюз не поднимался — колонка вернулась к прежнему адресу".to_string(),
                _ => what.to_string(),
            }
        });
        Ok(json!({
            "installed": lines.contains(&"installed"),
            "running": lines.iter().any(|line| line.contains(": running")),
            "disabled": lines.contains(&"disabled"),
            "local": lines.contains(&"local"),
            "fired": fired,
            "status": status.ok().filter(|status| status.code == 0).and_then(|status| serde_json::from_str::<Value>(&status.stdout).ok()),
            "log": log.map(|log| log.stdout).unwrap_or_default(),
        }))
    }

    /// The watch over the station at this address: the one whose placement has it, else its station's by device.
    fn watch_for(&self, address: &str) -> Arc<StationGateway> {
        self.deps.station.at_address(address).unwrap_or_else(|| {
            self.deps.station.watch(&key_of(self.deps.stations.device_for_address(address).as_deref(), address))
        })
    }

    /// «Попробовать снова» after the safety net: by the station's shell (the program may be the broken part) the
    /// safety net's mark goes and the service starts; once the gateway answers, its voice goes back to it.
    pub async fn retry(&self, address: &str) -> Result<(), String> {
        let watch = self.watch_for(address);
        let shell = self.open(address).await.map_err(|error| problem_of(&error))?;
        let started = self
            .run_ok(
                shell.as_ref(),
                &format!("rm -f {DIR}/disabled {DIR}/safety-fired && {SERVICE_COMMAND} start"),
                None,
            )
            .await;
        let outcome = async {
            started.map_err(|error| error.to_string())?;
            let client = watch.client().ok_or("шлюз не на колонке")?;
            self.wait(self.deps.timing.start_ms, || async { client.health().await.is_ok() })
                .await
                .ok_or("шлюз на колонке снова не запустился: смотрите «Что случилось»")?;
            self.run_ok(shell.as_ref(), &format!("{BIN} station point local --reboot"), None)
                .await
                .map(|_| ())
                .map_err(|error| error.to_string())
        }
        .await;
        shell.close().await;
        outcome
    }
}

fn parse_ms(iso: &str) -> u64 {
    iso.parse::<chrono::DateTime<chrono::Utc>>().map(|at| at.timestamp_millis().max(0) as u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    //! A move against a pretend station: its files in a folder, run through the station's own commands
    //! (station_fs.rs) — the same code the program runs there — and its gateway a pretend HTTP server. Never a station.

    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::LazyLock;

    use axum::routing::{get, post};
    use axum::Router;
    use futures::future::BoxFuture;
    use regex::Regex;

    use super::*;
    use crate::config::Env;
    use crate::placement::station_fs::{InstallOptions, StationFs, StationStatus, System, Target};
    use crate::stations::connect::quasar_config::{YANDEX_QUIC_URL, YANDEX_URL};
    use crate::stations::connect::station_shell::ShellResult;
    use crate::stations::service::{StationStatus as RecordStatus, StationView};

    /// voice.sh's switch: `sh …/voice.sh point '<url>' '<quic>'[ --force]`.
    static POINT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^sh \S+/voice\.sh point '([^']*)' '([^']*)'( --force)?$").unwrap());

    /// The station's commands that do not run here: recorded.
    #[derive(Default)]
    struct Quiet {
        reboots: AtomicUsize,
        /// The pretend station's folder: voice.sh runs with its pretend tools.
        root: std::path::PathBuf,
    }

    impl System for Quiet {
        fn remount(&self, _writable: bool) -> std::io::Result<()> {
            Ok(())
        }
        fn sync(&self) {}
        fn reboot(&self) -> std::io::Result<()> {
            self.reboots.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
        fn service(&self, _script: &std::path::Path, _action: &str) -> std::io::Result<String> {
            Ok("gateway: not running".into())
        }
        fn port_free(&self, _port: u16) -> bool {
            true
        }
        fn healthy(&self, _port: u16) -> bool {
            true
        }
        fn run(&self, command: &mut std::process::Command) -> std::io::Result<std::process::Output> {
            let inherited = std::env::var("PATH").unwrap_or_default();
            command
                .env("PATH", format!("{}:{inherited}", self.root.join("bin").display()))
                .env("PRETEND", &self.root)
                .output()
        }
    }

    /// The pretend station: its files, its gateway's state, what was run.
    struct Station {
        _dir: tempfile::TempDir,
        fs: StationFs,
        system: Quiet,
        /// The port its pretend gateway answers on.
        port: u16,
        started: AtomicBool,
        /// Its voice goes to its own gateway and it rebooted: the gateway hears it.
        connected: AtomicBool,
        commands: Mutex<Vec<String>>,
        received: Mutex<Option<String>>,
        /// What its gateway's /api/v1/home-server/link was sent.
        links: Mutex<Vec<Value>>,
    }

    impl Station {
        fn status(&self) -> StationStatus {
            self.fs.status(&self.system)
        }

        fn run(&self, command: &str, input: Option<&str>) -> ShellResult {
            self.commands.lock().unwrap().push(command.to_string());
            let ok = |stdout: String| ShellResult { code: 0, stdout, stderr: String::new() };
            let fail = |stderr: String| ShellResult { code: 1, stdout: String::new(), stderr };
            let bin = format!("{BIN} station ");
            if command == STEPS.read {
                return ok(std::fs::read_to_string(self.fs.path("/system/vendor/quasar/quasar.cfg")).unwrap());
            }
            if command == STEPS.firmware {
                let config = self.fs.path("/system/vendor/quasar/quasar.cfg");
                let mounted = std::fs::read_to_string(self.fs.path("/proc/mounts"))
                    .unwrap_or_default()
                    .contains(&format!(" {} ", config.display()));
                let file = if mounted { self.fs.path("/data/opt/duowave/voice/firmware.cfg") } else { config };
                return ok(std::fs::read_to_string(file).unwrap());
            }
            if let Some((step, _)) = deliveries().into_iter().find(|(step, _)| step == command) {
                let path = step.split("mv -f ").nth(1).and_then(|rest| rest.split(' ').nth(1)).unwrap().to_string();
                std::fs::create_dir_all(self.fs.path(DIR)).unwrap();
                std::fs::write(self.fs.path(&path), input.unwrap_or_default()).unwrap();
                return ok(String::new());
            }
            if command == STEPS.reboot {
                self.system.reboot().unwrap();
                return ok(String::new());
            }
            match command {
                "uname -m" => {
                    let machine = if self.fs.path("/data/not-aarch64").exists() { "armv7l" } else { "aarch64" };
                    return ok(format!("{machine}\n"));
                }
                "df -k /data | tail -n 1" => return ok("/dev/ubi0_5 61000 2000 59000 4% /data\n".into()),
                "grep MemAvailable /proc/meminfo" => return ok("MemAvailable:      61234 kB\n".into()),
                _ => {}
            }
            if command == format!("mkdir -p {DIR}/bin") {
                std::fs::create_dir_all(self.fs.path(&format!("{DIR}/bin"))).unwrap();
                return ok(String::new());
            }
            if command.starts_with("chmod ") {
                return ok(String::new());
            }
            if command == format!("[ -x {BIN} ] && echo ours || echo none") {
                return ok(if self.fs.path(BIN).exists() { "ours\n" } else { "none\n" }.into());
            }
            if command.contains("echo installed") {
                // «Проверить»: the shell's own account of the service and the safety net's marks.
                let mut lines = Vec::new();
                if self.fs.path(SERVICE).exists() {
                    lines.push("installed".to_string());
                }
                if self.fs.path(&format!("{DIR}/disabled")).exists() {
                    lines.push("disabled".to_string());
                }
                if let Ok(fired) = std::fs::read_to_string(self.fs.path(&format!("{DIR}/safety-fired"))) {
                    lines.push(format!("fired {}", fired.trim()));
                }
                if self.fs.status(&self.system).local {
                    lines.push("local".to_string());
                }
                return ok(lines.join("\n"));
            }
            if command.starts_with("tail -n 40 ") {
                return ok("safety net: the gateway does not come up\n".into());
            }
            if command == format!("rm -f {DIR}/disabled {DIR}/safety-fired && {SERVICE_COMMAND} start") {
                let _ = std::fs::remove_file(self.fs.path(&format!("{DIR}/disabled")));
                let _ = std::fs::remove_file(self.fs.path(&format!("{DIR}/safety-fired")));
                self.started.store(true, Ordering::Relaxed);
                return ok(String::new());
            }
            if command == format!("{SERVICE_COMMAND} start") {
                self.started.store(true, Ordering::Relaxed);
                return ok(String::new());
            }
            if command == format!("{SERVICE_COMMAND} stop") {
                self.started.store(false, Ordering::Relaxed);
                return ok(String::new());
            }
            if let Some(rest) = command.strip_prefix(&bin) {
                let words: Vec<&str> = rest.split_whitespace().collect();
                let value =
                    |name: &str| words.iter().position(|word| *word == name).map(|at| words[at + 1].to_string());
                return match words.first().copied() {
                    Some("install") => {
                        let options = InstallOptions {
                            port: Some(self.port),
                            home_server: value("--home-server").zip(value("--key")),
                            peer_key: value("--peer-key"),
                            managed: value("--managed"),
                            program: Some(self.fs.path(BIN)),
                            in_place: false,
                            start: false,
                            wait: None,
                        };
                        match self.fs.install(&options, &self.system) {
                            Ok(_) => ok("installed\n".into()),
                            Err(error) => fail(error),
                        }
                    }
                    Some("status") => ok(serde_json::to_string(&self.status()).unwrap()),
                    Some("receive-settings") => match self.fs.receive_settings(input.unwrap_or_default().as_bytes()) {
                        Ok(()) => {
                            *self.received.lock().unwrap() = input.map(str::to_string);
                            ok(String::new())
                        }
                        Err(error) => fail(error),
                    },
                    Some("point") => {
                        let target = Target::parse(words[1]).unwrap();
                        match self.fs.point(&target, words.contains(&"--reboot"), &self.system) {
                            Ok(_) => {
                                self.connected.store(self.fs.status(&self.system).local, Ordering::Relaxed);
                                ok(String::new())
                            }
                            Err(error) => fail(error),
                        }
                    }
                    Some("remove") => match self.fs.remove(words.contains(&"--keep-data"), &self.system) {
                        Ok(()) => ok(String::new()),
                        Err(error) => fail(error),
                    },
                    _ => fail(format!("unexpected: {command}")),
                };
            }
            if let Some(point) = POINT.captures(command) {
                // voice.sh itself, through the station's files: the target as `duowave station point` names it.
                let url = &point[1];
                let target = if url == YANDEX_URL {
                    Target::Yandex
                } else {
                    let authority = url.trim_start_matches("ws://").trim_end_matches("/uni.ws");
                    Target::Gateway(crate::stations::connect::quasar_config::parse_gateway_host(authority).unwrap())
                };
                return match self.fs.point(&target, false, &self.system) {
                    Ok(pointed) => ok(format!("copy: {}\n", pointed.copy.unwrap_or_default())),
                    Err(error) => fail(error),
                };
            }
            fail(format!("unexpected: {command}"))
        }
    }

    struct Shell(Arc<Station>);

    impl StationShell for Shell {
        fn run<'a>(&'a self, command: &'a str, input: Option<&'a str>) -> BoxFuture<'a, anyhow::Result<ShellResult>> {
            let result = self.0.run(command, input);
            Box::pin(async move { Ok(result) })
        }
        fn fingerprint(&self) -> &str {
            "SHA256:station"
        }
        fn close(&self) -> BoxFuture<'_, ()> {
            Box::pin(async {})
        }
        fn upload<'a>(&'a self, path: &'a str, bytes: &'a [u8]) -> BoxFuture<'a, anyhow::Result<()>> {
            std::fs::write(self.0.fs.path(path), bytes).unwrap();
            Box::pin(async { Ok(()) })
        }
    }

    struct Opener(Arc<Station>);

    impl ShellOpener for Opener {
        fn banner<'a>(&'a self, _address: &'a str, _port: u16) -> BoxFuture<'a, Option<String>> {
            Box::pin(async { Some("SSH-2.0-dropbear".into()) })
        }
        fn open<'a>(
            &'a self,
            _address: &'a str,
            options: ShellOptions,
        ) -> BoxFuture<'a, anyhow::Result<Box<dyn StationShell>>> {
            let station = self.0.clone();
            Box::pin(async move {
                if options.trust.as_deref().is_some_and(|trust| trust != "SHA256:station") {
                    anyhow::bail!("the station’s SSH key changed");
                }
                Ok(Box::new(Shell(station)) as Box<dyn StationShell>)
            })
        }
    }

    struct Records;

    impl Stations for Records {
        fn find(&self, device: &str) -> Option<StationView> {
            Some(StationView {
                device: device.into(),
                name: "Детская".into(),
                status: RecordStatus::Approved,
                last_address: Some("127.0.0.1".into()),
                first_seen: now_iso(),
                last_seen: now_iso(),
                device_model: None,
                device_color: None,
                firmware: None,
                model: None,
                color: None,
                sticker: None,
            })
        }
        fn device_for_address(&self, _address: &str) -> Option<String> {
            Some("dev1".into())
        }
        fn expect(&self, _address: &str, _ms: u64) {}
    }

    /// The station's gateway: answers once the service started it, says the station is in once its voice came.
    async fn gateway(station: Arc<Station>, listener: tokio::net::TcpListener) {
        let health = station.clone();
        let router = Router::new()
            .route(
                "/health",
                get(move || {
                    let station = health.clone();
                    async move {
                        if !station.started.load(Ordering::Relaxed) {
                            return Err(axum::http::StatusCode::SERVICE_UNAVAILABLE);
                        }
                        Ok(axum::Json(json!({"message": {
                            "status": "ok",
                            "build": {"version": crate::VERSION, "peerApi": crate::core::build::PEER_API},
                            "station": {"connected": station.connected.load(Ordering::Relaxed), "lastSeenAt": now_iso()},
                            "placement": {"role": "station", "settingsRevision": 1}
                        }})))
                    }
                }),
            )
            .route("/api/v1/placement/settings-home", post(|| async { axum::Json(json!({"message": {}})) }))
            .route(
                "/api/v1/home-server/link",
                post(move |axum::Json(body): axum::Json<Value>| {
                    station.links.lock().unwrap().push(body);
                    async { axum::Json(json!({"message": {}})) }
                }),
            );
        axum::serve(listener, router).await.unwrap();
    }

    fn pretend_build() -> PayloadLoader {
        Arc::new(|_config| {
            Box::pin(async {
                let mut builder =
                    tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
                let program = vec![0x7fu8; 4096];
                let mut header = tar::Header::new_gnu();
                header.set_size(program.len() as u64);
                header.set_mode(0o755);
                header.set_cksum();
                builder.append_data(&mut header, "duowave", program.as_slice()).unwrap();
                let archive = builder.into_inner().unwrap().finish().unwrap();
                Ok(payload::Payload {
                    archive,
                    signature: "checked by the real loader".into(),
                    version: crate::VERSION.into(),
                    source: payload::PayloadSource::BuiltIn,
                })
            })
        })
    }

    struct Computer {
        moves: Arc<MovesService>,
        watch: Arc<StationGateway>,
        secrets: Arc<SecretsService>,
        events: EventBus,
        restarts: Arc<AtomicUsize>,
        _state: tempfile::TempDir,
    }

    async fn setup() -> (Computer, Arc<Station>) {
        setup_with(&[]).await
    }

    /// The computer with these in its .env besides.
    async fn setup_with(env: &[(&str, &str)]) -> (Computer, Arc<Station>) {
        let dir = tempfile::Builder::new().prefix("station-").tempdir().unwrap();
        let dir_path = dir.path().to_path_buf();
        let fs = StationFs::at(dir.path());
        for folder in ["/system/vendor/quasar", "/etc/init.d", "/data", "/proc", "/bin"] {
            std::fs::create_dir_all(fs.path(folder)).unwrap();
        }
        std::fs::write(fs.path("/proc/mounts"), "").unwrap();
        crate::stations::connect::station_scripts::tests::pretend_tools(&fs.path("/bin"));
        std::fs::write(
            fs.path("/system/vendor/quasar/quasar.cfg"),
            crate::placement::station_fs::tests::config_text(YANDEX_URL, YANDEX_QUIC_URL),
        )
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let station = Arc::new(Station {
            _dir: dir,
            fs,
            system: Quiet { reboots: AtomicUsize::new(0), root: dir_path.clone() },
            port: listener.local_addr().unwrap().port(),
            started: AtomicBool::new(false),
            connected: AtomicBool::new(false),
            commands: Mutex::new(Vec::new()),
            received: Mutex::new(None),
            links: Mutex::new(Vec::new()),
        });
        tokio::spawn(gateway(station.clone(), listener));

        let state = tempfile::tempdir().unwrap();
        let config = Arc::new(Config::for_tests(&Env::from_pairs(
            [
                ("STATE_DIR", state.path().to_string_lossy().into_owned()),
                ("UPDATE_FEED_URL", "https://github.com/o/r/releases/latest/download/latest.json".into()),
            ]
            .into_iter()
            .chain(env.iter().map(|(name, value)| (*name, value.to_string()))),
        )));
        let db = Arc::new(Db::memory());
        let events = EventBus::new();
        let secrets = Arc::new(SecretsService::new(db.clone(), None).unwrap());
        let settings = Arc::new(SettingsService::new(db.clone(), events.clone()));
        let access = Arc::new(AccessService::new(db.clone(), events.clone(), settings.clone(), &secrets).unwrap());
        let gateways =
            StationGateways::new(config.clone(), db.clone(), settings.clone(), secrets.clone(), events.clone());
        // The pretend stations' list names every address «dev1».
        let watch = gateways.watch("dev1");
        let restarts = Arc::new(AtomicUsize::new(0));
        let counted = restarts.clone();
        let moves = MovesService::new(MovesDeps {
            config,
            db,
            settings: settings.clone(),
            secrets: secrets.clone(),
            events: events.clone(),
            access,
            opener: Arc::new(Opener(station.clone())),
            ssh: ShellConfig { port: 2222, user: "root".into(), password: String::new() },
            station: gateways,
            stations: Arc::new(Records),
            restarter: Arc::new(move || {
                counted.fetch_add(1, Ordering::Relaxed);
            }),
            timing: MoveTiming { start_ms: 3000, arrival_ms: 3000, poll_ms: 20 },
            payload: pretend_build(),
        });
        (Computer { moves, watch, secrets, events, restarts, _state: state }, station)
    }

    async fn finished(moves: &MovesService, id: &str) -> MoveJob {
        for _ in 0..500 {
            let job = moves.job(id).unwrap();
            if job.status != MoveStatus::Running {
                return job;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the move did not end");
    }

    fn states(job: &MoveJob) -> Vec<(MoveStep, StepState)> {
        job.steps.iter().map(|step| (step.id, step.state)).collect()
    }

    #[tokio::test]
    async fn moves_the_gateway_onto_the_station_with_the_computer_as_its_server_and_back_to_yandex() {
        let (computer, station) = setup().await;
        // A computer that moved stations before keeps its one key: this station gets a key of its own all the same.
        let before = "the-computers-one-key-from-before-0123";
        computer.secrets.set(crate::placement::peer::PEER_SECRET, Some(before)).unwrap();
        let gateway = crate::stations::connect::quasar_config::parse_gateway_host("192.0.2.10:3000");
        let job = computer
            .moves
            .start(
                MoveRequest {
                    to: MoveTo::StationServer,
                    address: "127.0.0.1".into(),
                    gateway,
                    settings_home: SettingsHome::Station,
                    remove: false,
                },
                None,
            )
            .unwrap();
        let job = finished(&computer.moves, &job.id).await;
        assert_eq!(job.status, MoveStatus::Done, "{:?}", job.error);
        assert!(states(&job).iter().all(|(_, state)| *state == StepState::Done));

        let status = station.status();
        assert!(status.installed && status.local, "the program, the service, the voice at its own gateway");
        assert_eq!(status.home_server.as_deref(), Some("http://192.0.2.10:3000"));
        assert_eq!(station.fs.env_value("PEER_KEY").unwrap(), computer.watch.peer_key().unwrap());
        // Its own key, kept as its own: never the computer's one, which would open every station it moved (S2).
        let own = computer.secrets.get(&crate::placement::peer::station_secret("dev1"));
        assert_eq!(own, station.fs.env_value("PEER_KEY"));
        assert_ne!(own.as_deref(), Some(before));
        assert_eq!(computer.secrets.get(crate::placement::peer::PEER_SECRET).as_deref(), Some(before), "untouched");
        assert_eq!(status.managed_by.as_deref(), Some("pc"));
        assert!(station.fs.path("/data/opt/duowave/restore.sh").exists(), "the way back before anything changed");
        let received: transfer::Bundle =
            serde_json::from_str(&station.received.lock().unwrap().clone().unwrap()).unwrap();
        assert_eq!(received.from, "pc");
        // The mode in force travels with a move, .env's too — the routine sending leaves the station its own (G1).
        assert_eq!(received.settings["gateway_mode"], json!({ "mode": "relay" }), "the mode in force travels");
        assert_eq!(received.schema, transfer::WHOLE, "whole: a station's build older than this one takes it too");
        assert_eq!(station.system.reboots.load(Ordering::Relaxed), 1);
        let record = computer.watch.record().unwrap();
        assert_eq!((record.placement, record.port, record.managed), (Placement::StationServer, station.port, true));
        assert!(record.home_key_id.is_some(), "a key on this computer for the station's home server");
        let commands = station.commands.lock().unwrap().clone();
        let at = |needle: &str| commands.iter().position(|command| command.contains(needle)).unwrap();
        assert!(at("receive-settings") < at(&format!("{SERVICE_COMMAND} start")), "it starts on the settings");
        assert!(
            at(&format!("{SERVICE_COMMAND} start")) < at("point local"),
            "the voice moves once the gateway answers"
        );

        // «Вернуть как было»: Yandex's addresses, its gateway off the station, nothing of ours.
        let back = computer
            .moves
            .start(
                MoveRequest {
                    to: MoveTo::Yandex,
                    address: "127.0.0.1".into(),
                    gateway: None,
                    settings_home: SettingsHome::Station,
                    remove: false,
                },
                None,
            )
            .unwrap();
        let back = finished(&computer.moves, &back.id).await;
        assert_eq!(back.status, MoveStatus::Done, "{:?}", back.error);
        let voice = station.fs.voice().unwrap();
        assert_eq!((voice.url.as_str(), voice.quic.as_str()), (YANDEX_URL, YANDEX_QUIC_URL));
        assert!(!station.status().installed);
        assert!(computer.watch.record().is_none());
        assert_eq!(computer.restarts.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn alone_on_the_station_the_computers_address_and_key_travel_too_and_wait_switched_off() {
        let (computer, station) = setup().await;
        let gateway = crate::stations::connect::quasar_config::parse_gateway_host("192.0.2.10:3000");
        let job = computer
            .moves
            .start(
                MoveRequest {
                    to: MoveTo::Station,
                    address: "127.0.0.1".into(),
                    gateway,
                    settings_home: SettingsHome::Station,
                    remove: false,
                },
                None,
            )
            .unwrap();
        let job = finished(&computer.moves, &job.id).await;
        assert_eq!(job.status, MoveStatus::Done, "{:?}", job.error);
        assert_eq!(station.status().home_server.as_deref(), Some("http://192.0.2.10:3000"), "known");
        assert_eq!(station.links.lock().unwrap().clone(), vec![json!({"enabled": false})], "and off");
        let record = computer.watch.record().unwrap();
        assert_eq!(record.placement, Placement::Station);
        assert!(record.home_key_id.is_some(), "«ПК помогает колонке» later needs no new key");
    }

    #[tokio::test]
    async fn moving_back_to_the_computer_takes_the_settings_that_lived_on_the_station_and_restarts_on_them() {
        let (computer, station) = setup().await;
        let job = computer
            .moves
            .start(
                MoveRequest {
                    to: MoveTo::Station,
                    address: "127.0.0.1".into(),
                    gateway: None,
                    settings_home: SettingsHome::Station,
                    remove: false,
                },
                None,
            )
            .unwrap();
        assert_eq!(finished(&computer.moves, &job.id).await.status, MoveStatus::Done);
        // The station's gateway gives its settings: a pretend one that seals what the computer itself sent.
        let key = computer.watch.peer_key().unwrap();
        let sent: transfer::Bundle = serde_json::from_str(&station.received.lock().unwrap().clone().unwrap()).unwrap();
        let sealed = transfer::seal(&sent, &key);
        let router = Router::new().route(
            "/api/v1/transfer",
            get(move || {
                let sealed = sealed.clone();
                async move { axum::Json(json!({ "message": sealed })) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let mut record = computer.watch.record().unwrap();
        record.port = port;
        computer.watch.set_record(Some(&record), None).unwrap();

        let events = computer.events.clone();
        let job = computer
            .moves
            .start(
                MoveRequest {
                    to: MoveTo::Pc,
                    address: "127.0.0.1".into(),
                    gateway: crate::stations::connect::quasar_config::parse_gateway_host("192.0.2.10:3000"),
                    settings_home: SettingsHome::Station,
                    remove: true,
                },
                None,
            )
            .unwrap();
        // The station comes to this computer's /uni.ws after its reboot.
        for _ in 0..200 {
            if station.fs.voice().unwrap().url == "ws://192.0.2.10:3000/uni.ws" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        events.emit(GatewayEvent::StationConnected { session_id: "s".into(), device_tag: "dev1".into() });
        let job = finished(&computer.moves, &job.id).await;
        assert_eq!(job.status, MoveStatus::Done, "{:?}", job.error);
        assert_eq!(states(&job)[1], (MoveStep::Settings, StepState::Done));
        assert_eq!(computer.watch.record().unwrap().placement, Placement::Pc);
        assert!(!station.status().installed, "removed, its data kept");
        assert!(station.fs.path("/data/opt/duowave/gateway/.env").exists());
        assert_eq!(computer.restarts.load(Ordering::Relaxed), 1, "restarts on the settings it took");
        assert_eq!(computer.watch.backups.list().len(), 1, "and keeps a copy of them");
    }

    fn station_server(gateway: Option<&str>, settings_home: SettingsHome) -> MoveRequest {
        MoveRequest {
            to: MoveTo::StationServer,
            address: "127.0.0.1".into(),
            gateway: gateway.and_then(crate::stations::connect::quasar_config::parse_gateway_host),
            settings_home,
            remove: false,
        }
    }

    fn shopping_list(db: &Db, list: &str) {
        crate::plugins::registry::set_enabled(db, "shopping", true, None, None).unwrap();
        db.with(|c| {
            c.execute(
                "INSERT INTO plugin_kv (plugin_id, key, value, updated_at) VALUES ('shopping', 'list', ?1, 'then')
                 ON CONFLICT (plugin_id, key) DO UPDATE SET value = excluded.value",
                rusqlite::params![list],
            )
        })
        .unwrap();
    }

    fn list_of(db: &Db) -> Option<String> {
        use rusqlite::OptionalExtension;
        db.with(|c| {
            c.query_row("SELECT value FROM plugin_kv WHERE plugin_id = 'shopping' AND key = 'list'", [], |row| {
                row.get(0)
            })
            .optional()
        })
        .unwrap()
    }

    /// The station's gateway answering the pair's GET of its package with this one, sealed: the computer's watch
    /// asks it there from now.
    async fn station_gives(computer: &Computer, bundle: &transfer::Bundle) {
        let sealed = transfer::seal(bundle, &computer.watch.peer_key().unwrap());
        let router = Router::new().route(
            "/api/v1/transfer",
            get(move || {
                let sealed = sealed.clone();
                async move { axum::Json(json!({ "message": sealed })) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let mut record = computer.watch.record().unwrap();
        record.port = port;
        computer.watch.set_record(Some(&record), None).unwrap();
    }

    /// Back to the computer: the station's voice comes to its /uni.ws after the reboot.
    async fn back_to_the_computer(computer: &Computer, station: &Station, settings_home: SettingsHome) -> MoveJob {
        let events = computer.events.clone();
        let job = computer
            .moves
            .start(
                MoveRequest {
                    to: MoveTo::Pc,
                    address: "127.0.0.1".into(),
                    gateway: crate::stations::connect::quasar_config::parse_gateway_host("192.0.2.10:3000"),
                    settings_home,
                    remove: false,
                },
                None,
            )
            .unwrap();
        for _ in 0..200 {
            if station.fs.voice().unwrap().url == "ws://192.0.2.10:3000/uni.ws" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        events.emit(GatewayEvent::StationConnected { session_id: "s".into(), device_tag: "dev1".into() });
        finished(&computer.moves, &job.id).await
    }

    /// Live, 10.11: the computer speaks «mixed» and moved the gateway to the station as its server; the station spoke
    /// Alice's voice in «Шлюз», «Список покупок» was off there with its list on the computer, and its windows called it
    /// «Колонка». The move carries this computer's voice choice, the plugins' switches and data, and the station's name
    /// as this computer knows it; a move alone carries no voice.
    #[tokio::test]
    async fn the_move_with_the_computer_as_its_server_carries_its_voice_its_plugins_and_the_stations_name() {
        let (computer, station) = setup_with(&[("TTS_OWN_URL", "http://127.0.0.1:9/v1")]).await;
        computer.moves.deps.settings.set(&VOICE_PROVIDER, &"mixed".to_string(), None).unwrap();
        shopping_list(&computer.moves.deps.db, r#"["молоко"]"#);
        let job = computer.moves.start(station_server(Some("192.0.2.10:3000"), SettingsHome::Station), None).unwrap();
        let job = finished(&computer.moves, &job.id).await;
        assert_eq!(job.status, MoveStatus::Done, "{:?}", job.error);
        let received: transfer::Bundle =
            serde_json::from_str(&station.received.lock().unwrap().clone().unwrap()).unwrap();
        assert_eq!(received.settings["tts_provider"], json!("mixed"), "the computer's voice, in «Шлюз» too");
        assert_eq!(received.settings["station_self"], json!({ "device": "dev1", "name": "Детская" }));
        let shopping = &received.plugins.as_ref().expect("the plugins' state")["shopping"];
        assert!(shopping.enabled);
        assert_eq!(shopping.kv.iter().map(|entry| entry.value.as_str()).collect::<Vec<_>>(), vec![r#"["молоко"]"#]);

        // Alone on the station: the computer gives it no voice — the station keeps its own choice.
        let (computer, station) = setup_with(&[("TTS_OWN_URL", "http://127.0.0.1:9/v1")]).await;
        computer.moves.deps.settings.set(&VOICE_PROVIDER, &"mixed".to_string(), None).unwrap();
        let alone =
            MoveRequest { to: MoveTo::Station, ..station_server(Some("192.0.2.10:3000"), SettingsHome::Station) };
        let job = computer.moves.start(alone, None).unwrap();
        assert_eq!(finished(&computer.moves, &job.id).await.status, MoveStatus::Done);
        let received: transfer::Bundle =
            serde_json::from_str(&station.received.lock().unwrap().clone().unwrap()).unwrap();
        assert!(!received.settings.contains_key("tts_provider"));
        assert!(received.plugins.is_some(), "the plugins go along all the same");
    }

    /// Live, 10.11 (and 08.11): the move back revoked the station's key to its home server and the station's settings
    /// applied at the restart brought it back; the station's plugins' data stayed on it. The move back takes the
    /// plugins' state with the settings, and the key stays revoked.
    #[tokio::test]
    async fn the_move_back_brings_the_plugins_data_and_the_revoked_home_key_stays_revoked() {
        let (computer, station) = setup().await;
        let job = computer.moves.start(station_server(Some("192.0.2.10:3000"), SettingsHome::Station), None).unwrap();
        assert_eq!(finished(&computer.moves, &job.id).await.status, MoveStatus::Done);
        let home_key = computer.watch.record().unwrap().home_key_id.expect("a key for its home server");
        // The station's package: what it was sent, the key alive in it, and its list grown there.
        let mut theirs: transfer::Bundle =
            serde_json::from_str(&station.received.lock().unwrap().clone().unwrap()).unwrap();
        let station_db = Db::memory();
        shopping_list(&station_db, r#"["молоко","хлеб"]"#);
        theirs.plugins = Some(transfer::plugins(&station_db).unwrap());
        assert!(theirs.tables["api_keys"]
            .iter()
            .any(|row| row["id"] == json!(home_key) && row["revoked_at"].is_null()));
        station_gives(&computer, &theirs).await;

        let job = back_to_the_computer(&computer, &station, SettingsHome::Station).await;
        assert_eq!(job.status, MoveStatus::Done, "{:?}", job.error);
        assert_eq!(computer.restarts.load(Ordering::Relaxed), 1);
        let deps = &computer.moves.deps;
        let revoked = |db: &Db| -> Option<String> {
            db.with(|c| c.query_row("SELECT revoked_at FROM api_keys WHERE id = ?1", [&home_key], |row| row.get(0)))
                .unwrap()
        };
        assert!(revoked(&deps.db).is_some(), "the move back revokes it");
        // The restart: the package applied, the key as revoked as before it.
        let state = deps.config.skills.state_dir.clone().unwrap();
        let imported = transfer::apply_incoming(&state, &deps.db, &deps.secrets, false).unwrap();
        assert!(imported.plugins >= 1);
        assert!(revoked(&deps.db).is_some(), "not brought back to life by the station's settings");
        assert_eq!(list_of(&deps.db).as_deref(), Some(r#"["молоко","хлеб"]"#), "the station's list, here now");
        // The copy of the station's settings kept here is the settings': its restore leaves the plugins be.
        assert_eq!(computer.watch.backups.list().len(), 1);
    }

    /// The settings live on the computer: the move back takes the plugins' state alone, for the restart.
    #[tokio::test]
    async fn with_the_settings_on_the_computer_the_move_back_takes_the_plugins_alone() {
        let (computer, station) = setup().await;
        let job = computer.moves.start(station_server(Some("192.0.2.10:3000"), SettingsHome::Pc), None).unwrap();
        assert_eq!(finished(&computer.moves, &job.id).await.status, MoveStatus::Done);
        let mut theirs: transfer::Bundle =
            serde_json::from_str(&station.received.lock().unwrap().clone().unwrap()).unwrap();
        let station_db = Db::memory();
        shopping_list(&station_db, r#"["хлеб"]"#);
        theirs.plugins = Some(transfer::plugins(&station_db).unwrap());
        station_gives(&computer, &theirs).await;
        let mut record = computer.watch.record().unwrap();
        record.settings_home = SettingsHome::Pc;
        computer.watch.set_record(Some(&record), None).unwrap();

        let job = back_to_the_computer(&computer, &station, SettingsHome::Pc).await;
        assert_eq!(job.status, MoveStatus::Done, "{:?}", job.error);
        assert_eq!(states(&job)[1], (MoveStep::Settings, StepState::Done));
        let state = computer.moves.deps.config.skills.state_dir.clone().unwrap();
        assert!(!transfer::incoming_file(&state).exists(), "the settings are the computer's own");
        assert!(transfer::incoming_plugins_file(&state).exists());
        assert_eq!(computer.restarts.load(Ordering::Relaxed), 1);
        let deps = &computer.moves.deps;
        assert!(transfer::apply_incoming(&state, &deps.db, &deps.secrets, false).is_none());
        assert_eq!(list_of(&deps.db).as_deref(), Some(r#"["хлеб"]"#));
    }

    #[tokio::test]
    async fn refuses_a_second_move_and_a_station_that_is_not_aarch64_changes_nothing() {
        let (computer, station) = setup().await;
        std::fs::write(station.fs.path("/data/not-aarch64"), "").unwrap();
        // A computer address is needed to be its server.
        let error = computer
            .moves
            .start(
                MoveRequest {
                    to: MoveTo::StationServer,
                    address: "127.0.0.1".into(),
                    gateway: None,
                    settings_home: SettingsHome::Station,
                    remove: false,
                },
                None,
            )
            .unwrap_err();
        assert_eq!(error.status.as_u16(), 400);
        let request = MoveRequest {
            to: MoveTo::Station,
            address: "127.0.0.1".into(),
            gateway: None,
            settings_home: SettingsHome::Station,
            remove: false,
        };
        let first = computer.moves.start(request.clone(), None).unwrap();
        assert_eq!(computer.moves.start(request, None).unwrap_err().status.as_u16(), 409);
        let first = finished(&computer.moves, &first.id).await;
        assert_eq!(first.status, MoveStatus::Failed);
        assert!(first.error.as_deref().unwrap_or_default().contains("aarch64"), "{:?}", first.error);
        assert_eq!(states(&first)[0], (MoveStep::Check, StepState::Failed));
        assert_eq!(station.fs.voice().unwrap().url, YANDEX_URL, "nothing changed");
        assert!(!station.fs.path(BIN).exists() && !station.fs.path("/data/opt/duowave/restore.sh").exists());
        assert!(computer.watch.record().is_none());
        let _ = Target::Yandex;
    }

    #[tokio::test]
    async fn tells_what_happened_by_the_shell_when_the_program_is_the_broken_part_and_tries_again() {
        let (computer, station) = setup().await;
        let job = computer
            .moves
            .start(
                MoveRequest {
                    to: MoveTo::Station,
                    address: "127.0.0.1".into(),
                    gateway: None,
                    settings_home: SettingsHome::Station,
                    remove: false,
                },
                None,
            )
            .unwrap();
        assert_eq!(finished(&computer.moves, &job.id).await.status, MoveStatus::Done);
        // The safety net acted: the gateway off, the station at its previous address, the program broken.
        std::fs::write(station.fs.path(&format!("{DIR}/disabled")), "").unwrap();
        std::fs::write(station.fs.path(&format!("{DIR}/safety-fired")), "previous-address 1793991442\n").unwrap();
        station.fs.point(&crate::placement::station_fs::Target::Previous, false, &station.system).unwrap();
        station.started.store(false, Ordering::Relaxed);
        std::fs::write(station.fs.path(BIN), "#!/bin/sh\nexit 1\n").unwrap();
        let seen = computer.moves.inspect("127.0.0.1").await.unwrap();
        assert_eq!(
            (seen["installed"].as_bool(), seen["disabled"].as_bool(), seen["local"].as_bool()),
            (Some(true), Some(true), Some(false))
        );
        assert_eq!(seen["fired"], "шлюз не поднимался — колонка вернулась к прежнему адресу");
        assert!(seen["log"].as_str().unwrap().contains("safety net"));

        // «Попробовать снова»: the mark goes by the shell, the service starts, the voice comes back to the gateway.
        computer.moves.retry("127.0.0.1").await.unwrap();
        assert!(!station.fs.path(&format!("{DIR}/disabled")).exists());
        assert!(station.status().local);
    }
}
