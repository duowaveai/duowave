//! The computer's watch over the station's gateway (docs/plans/station-placement.md, sections 6, 7 and 10): every
//! 30 s it asks the station's /health with the pair's key — whether it answers, its version and API, memory and
//! space, its home server, where the station's voice goes, its settings revision, the settings schema its import
//! knows. The sync is the computer's, the station stays passive: a new revision there and the settings live on the
//! station — the computer takes a copy; the settings live on the computer and its own changed — it sends them, the way
//! the station's build takes them. A newer station build inside this one — it offers the update, or installs it when
//! the household said «автоматически».
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use super::backup::Backups;
use super::payload::{self, PayloadInfo};
use super::peer::{self, station_secret, PeerClient, PEER_SECRET};
use crate::config::Config;
use crate::core::availability::Placement;
use crate::core::build::PEER_API;
use crate::core::events::{Actor, EventBus, GatewayEvent};
use crate::platform::http::ApiError;
use crate::secrets::SecretsService;
use crate::settings::definitions::{StationPlacement, GATEWAY_SELF, STATION_PLACEMENT, STATION_PLACEMENTS};
use crate::settings::SettingsService;
use crate::storage::Db;
use crate::utils::time::now_iso;

/// How often the station's gateway is asked.
pub const POLL: Duration = Duration::from_secs(30);

/// What the computer knows of the station's gateway now.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Live {
    pub reachable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<String>,
    /// When it last answered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seen_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The station gateway's /health, as it answered last.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health: Option<Value>,
    /// Why the home's copy was not kept up last time (src/home/copy.rs).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sync_error: Option<String>,
    /// The station's gateway being updated from here, and how the last one went.
    pub updating: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update_error: Option<String>,
}

/// Something about the station's gateway the household should know.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Warning {
    /// silent, voiceAway, safety.
    pub kind: &'static str,
    pub text: String,
}

/// After this long without an answer the station's gateway is «не отвечает» (a reboot takes a minute).
pub const SILENT_AFTER: Duration = Duration::from_secs(120);

/// What tells the household (the station watch's ways: macOS, Telegram, the webhook).
pub type Teller = Arc<dyn Fn(String) + Send + Sync>;

/// This computer's addresses on the home network as a station dials them: «192.168.1.20», «192.168.1.20:3000».
pub type HomeAddresses = Arc<dyn Fn() -> Vec<String> + Send + Sync>;

/// Looks in a row without an answer before the computer looks for the station anew (its address may have changed).
pub const SEARCH_AFTER: u32 = 3;
/// And not more often than this.
pub const SEARCH_EVERY: Duration = Duration::from_secs(300);

/// The station's gateway as the computer's panel shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StationGatewayView {
    /// The station: its device, else «at:<address>» (linked by hand).
    pub key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record: Option<StationPlacement>,
    pub live: Live,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// It speaks this computer's pair API; when not — update it first.
    pub compatible: bool,
    /// A newer build of the station's gateway inside this computer's: «Обновить колонку до X».
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update: Option<String>,
    pub payload: PayloadInfo,
    /// When this computer last took the home's copy from the centre on the station (docs/plans/home-and-devices.md,
    /// stage 3).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copied_at: Option<String>,
    /// What the household should know now: the station's gateway silent, the voice away from it, the safety net.
    pub warnings: Vec<Warning>,
    /// The home's copy is older than a day: «Mac не видел колонку …».
    pub copy_stale: bool,
}

pub struct StationGateway {
    /// Which station this watch is over (see StationGatewayView::key).
    key: String,
    config: Arc<Config>,
    db: Arc<Db>,
    settings: Arc<SettingsService>,
    secrets: Arc<SecretsService>,
    events: EventBus,
    pub backups: Backups,
    live: Mutex<Live>,
    /// The versions offered to update to automatically: each once.
    auto_tried: Mutex<Option<String>>,
    teller: std::sync::OnceLock<Teller>,
    /// The warnings already told: each once while it holds.
    told: Mutex<Vec<String>>,
    home_addresses: std::sync::OnceLock<HomeAddresses>,
    /// Looks without an answer in a row, and when the station was last looked for on the network.
    misses: Mutex<(u32, Option<std::time::Instant>)>,
}

impl StationGateway {
    pub fn new(
        config: Arc<Config>,
        db: Arc<Db>,
        settings: Arc<SettingsService>,
        secrets: Arc<SecretsService>,
        events: EventBus,
        key: &str,
    ) -> Arc<StationGateway> {
        let backups = Backups::for_station(config.skills.state_dir.as_deref(), key);
        Arc::new(StationGateway {
            key: key.to_string(),
            config,
            db,
            settings,
            secrets,
            events,
            backups,
            live: Mutex::new(Live::default()),
            auto_tried: Mutex::new(None),
            teller: std::sync::OnceLock::new(),
            told: Mutex::new(Vec::new()),
            home_addresses: std::sync::OnceLock::new(),
            misses: Mutex::new((0, None)),
        })
    }

    pub fn set_home_addresses(&self, addresses: HomeAddresses) {
        let _ = self.home_addresses.set(addresses);
    }

    /// This computer as the station's home server: of its addresses on the home network, the one in the station's
    /// own network (the same first three numbers), else the first.
    pub fn home_url(&self) -> Option<String> {
        let record = self.record()?;
        let addresses = self.home_addresses.get().map(|addresses| addresses()).unwrap_or_default();
        pick_home(&addresses, &record.address).map(|best| format!("http://{best}"))
    }

    /// The computer's key with the station's home server is not there yet: the switch has to bring one.
    pub fn needs_home_key(&self) -> bool {
        let known = self.lock().health.as_ref().and_then(|health| health.pointer("/placement/homeServer/url").cloned());
        self.record().is_none_or(|record| record.home_key_id.is_none()) || known.is_none()
    }

    /// «ПК помогает колонке»: the station leans on this computer or works alone; with a new key (secret, id) the
    /// address goes along. No move, no reboot: the station's switch only.
    pub async fn set_helper(
        &self,
        on: bool,
        key: Option<(String, String)>,
        actor: Option<Actor>,
    ) -> Result<(), String> {
        let mut record = self.on_station().ok_or(crate::tr!("центр не на колонке"))?;
        let client = self.client().ok_or(crate::tr!("нет ключа пары"))?;
        let mut body = serde_json::json!({ "enabled": on });
        self.helper_body(&mut body, key, &mut record)?;
        client.post("/api/v1/home-server/link", body).await?;
        record.placement = if on { Placement::StationServer } else { Placement::Station };
        self.store(&record, actor).map_err(|error| error.message)?;
        self.events.emit(GatewayEvent::PlacementChanged { what: "helper".into() });
        self.poll().await;
        Ok(())
    }

    /// «Что даю колонке»: what the station does not take from this computer — "asr", "tts", "llm", "messages"
    /// (docs/plans/desk.md, stage 4). The switch itself stays as it is.
    pub async fn set_helper_parts(&self, without: Vec<String>, actor: Option<Actor>) -> Result<(), String> {
        let _ = actor;
        let client = self.client().ok_or(crate::tr!("нет ключа пары"))?;
        client.post("/api/v1/home-server/link", serde_json::json!({ "without": without })).await?;
        self.events.emit(GatewayEvent::PlacementChanged { what: "helper".into() });
        self.poll().await;
        Ok(())
    }

    fn helper_body(
        &self,
        body: &mut Value,
        key: Option<(String, String)>,
        record: &mut StationPlacement,
    ) -> Result<(), String> {
        if let Some((secret, id)) = key {
            body["url"] =
                Value::String(self.home_url().ok_or(crate::tr!("не вижу адрес этого компьютера в домашней сети"))?);
            body["key"] = Value::String(secret);
            record.home_key_id = Some(id);
        }
        Ok(())
    }

    /// After an answer: the station's switch turned on its own panel shows here; this computer's address, changed
    /// on the home network, goes to the station — only where this computer gave it its key.
    async fn keep_links(&self, client: &PeerClient) {
        let Some(mut record) = self.on_station() else { return };
        let Some(health) = self.lock().health.clone() else { return };
        // A station with helpers (docs/plans/home-and-helpers.md) may lean on others while this computer's help is
        // off: the switch is its entry of this computer, `homeServer.enabled`. A 0.11 station says it in `placement`.
        let says = if health.pointer("/placement/helpers").is_some() {
            match health.pointer("/placement/homeServer/enabled").and_then(Value::as_bool) {
                Some(true) => Placement::StationServer,
                _ => Placement::Station,
            }
        } else {
            match health.pointer("/placement/placement").and_then(Value::as_str) {
                Some("stationServer") => Placement::StationServer,
                Some("station") => Placement::Station,
                _ => return,
            }
        };
        if says != record.placement {
            record.placement = says;
            if self.store(&record, None).is_ok() {
                self.events.emit(GatewayEvent::PlacementChanged { what: "helper".into() });
            }
        }
        if record.home_key_id.is_none() {
            return;
        }
        let Some(known) = health.pointer("/placement/homeServer/url").and_then(Value::as_str) else { return };
        let Some(ours) = self.home_url() else { return };
        if known.trim_end_matches('/') == ours {
            return;
        }
        match client.post("/api/v1/home-server/link", serde_json::json!({ "url": ours })).await {
            Ok(_) => tracing::info!("placement: told the station this computer's new address {ours} (was {known})"),
            Err(error) => tracing::warn!("placement: the station not told this computer's new address: {error}"),
        }
    }

    /// The station silent a few looks in a row: its address may have changed. The stations that announce themselves
    /// on the network are asked — the one that opens with the pair's key is ours.
    async fn maybe_search(&self) {
        let due = {
            let mut misses = self.misses.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            misses.0 += 1;
            let due = misses.0 >= SEARCH_AFTER && misses.1.is_none_or(|at| at.elapsed() >= SEARCH_EVERY);
            if due {
                misses.1 = Some(std::time::Instant::now());
            }
            due
        };
        if !due {
            return;
        }
        let found = crate::stations::connect::discovery::discover_stations(1500).await;
        self.find_among(found.into_iter().map(|station| station.address).collect()).await;
    }

    /// Of these addresses, the one where the station's gateway opens with the pair's key becomes the record's.
    pub async fn find_among(&self, addresses: Vec<String>) -> Option<String> {
        let mut record = self.on_station()?;
        let key = self.pair_key()?;
        for address in addresses.into_iter().filter(|address| *address != record.address) {
            let client = PeerClient::new(&address, record.port, &key);
            let Ok(health) = client.health().await else { continue };
            if health.pointer("/placement/role").and_then(Value::as_str) != Some("station") {
                continue;
            }
            if client.settings().await.is_err() {
                continue;
            }
            tracing::info!("placement: the station's gateway found at {address} (was {})", record.address);
            record.address = address.clone();
            self.store(&record, None).ok()?;
            *self.misses.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = (0, None);
            self.events.emit(GatewayEvent::PlacementChanged { what: "address".into() });
            return Some(address);
        }
        None
    }

    pub fn set_teller(&self, teller: Teller) {
        let _ = self.teller.set(teller);
    }

    /// What the household should know about the station's gateway now.
    pub fn warnings(&self) -> Vec<Warning> {
        let mut warnings = Vec::new();
        let Some(record) = self.on_station() else { return warnings };
        let live = self.live();
        // The 2 minutes count from the move too: a fresh record has not heard the station yet, and it reboots for a
        // minute after a move (live, 10.11: «не отвечает» came at once).
        let quiet = |at: Option<&str>| at.is_none_or(|at| older_than(at, SILENT_AFTER));
        let silent = live.checked_at.is_some()
            && !live.reachable
            && quiet(live.seen_at.as_deref())
            && quiet(record.moved_at.as_deref());
        if silent {
            let since =
                live.seen_at.as_deref().map(local_time).map(|at| crate::tr!(" с {at}", at = at)).unwrap_or_default();
            warnings.push(Warning {
                kind: "silent",
                text: crate::tr!(
                    "Центр на колонке не отвечает{since}. Колонка выключена, перезагружается, её сбросили — или DuoWave не поднимается и страховка вернула её Яндексу. «Проверить» скажет.",
                    since = since
                ),
            });
        }
        if let (true, Some(health)) = (live.reachable, live.health.as_ref()) {
            if health.pointer("/placement/voice/local").and_then(Value::as_bool) == Some(false) {
                let text = match health.pointer("/placement/voice/pointsTo").and_then(Value::as_str) {
                    Some("yandex") => crate::tr!("Голос колонки снова у Яндекса, мимо её центра: её вернула прошивка, страховка или кнопка «Отдать голос Яндексу». «Вернуть голос DuoWave» — обратно."),
                    _ => crate::tr!("Голос колонки идёт не в её центр."),
                };
                warnings.push(Warning { kind: "voiceAway", text });
            }
            if let Some(case) = health.pointer("/placement/safetyLast") {
                let at = case.get("at").and_then(Value::as_str).map(local_time).unwrap_or_default();
                let what = match case.get("what").and_then(Value::as_str) {
                    Some("previous-version") => crate::tr!("новая версия не поднялась — вернулась прежняя"),
                    _ => crate::tr!("DuoWave не поднимался — колонка возвращалась к прежнему адресу"),
                };
                warnings.push(Warning {
                    kind: "safety",
                    text: crate::tr!("Страховка сработала {at}: {what}.", at = at, what = what),
                });
            }
        }
        warnings
    }

    /// Tells each new warning once, by the station watch's ways.
    fn tell_news(&self) {
        let warnings = self.warnings();
        let fresh: Vec<String> = {
            let mut told = self.told.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let now: Vec<String> = warnings.iter().map(|warning| warning.text.clone()).collect();
            let fresh = now.iter().filter(|text| !told.contains(text)).cloned().collect();
            *told = now;
            fresh
        };
        if let Some(teller) = self.teller.get() {
            for text in fresh {
                teller(text);
            }
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Live> {
        self.live.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn record(&self) -> Option<StationPlacement> {
        self.settings.get(&STATION_PLACEMENTS, None).and_then(|all| all.get(&self.key).cloned())
    }

    /// This station's record kept, beside the other stations'.
    fn store(&self, record: &StationPlacement, actor: Option<Actor>) -> Result<(), ApiError> {
        let mut all = self.settings.get(&STATION_PLACEMENTS, None).unwrap_or_default();
        all.insert(self.key.clone(), record.clone());
        self.settings.set(&STATION_PLACEMENTS, &all, actor)
    }

    /// The gateway runs on the station (alone or with this computer as its server).
    pub fn on_station(&self) -> Option<StationPlacement> {
        self.record().filter(|record| record.placement.on_station())
    }

    /// The station's name as this computer's list of stations has it («Детская»): /health says it beside the
    /// placement, the panels name the station by it (the helper's widgets, the station's window).
    pub fn name(&self) -> Option<String> {
        let device = self.record()?.device?;
        super::service::station_name(&self.db, &device)
    }

    pub fn set_record(&self, record: Option<&StationPlacement>, actor: Option<Actor>) -> Result<(), ApiError> {
        match record {
            Some(record) => self.store(record, actor)?,
            None => {
                let mut all = self.settings.get(&STATION_PLACEMENTS, None).unwrap_or_default();
                if all.remove(&self.key).is_some() {
                    self.settings.set(&STATION_PLACEMENTS, &all, actor)?;
                    // What the station showed through the helper's pass goes with it.
                    super::proxy::forget_copies(self.config.skills.state_dir.as_deref(), &self.key);
                }
            }
        }
        *self.lock() = Live::default();
        self.events.emit(GatewayEvent::PlacementChanged { what: "record".into() });
        Ok(())
    }

    /// This station's pair key: its own — the one it gave when it was linked (`link`), or the one a move from here
    /// installed on it — else the computer's one key from before, which a station moved or linked before each had its
    /// own still opens with.
    pub fn pair_key(&self) -> Option<String> {
        self.secrets.get(&station_secret(&self.key)).or_else(|| self.secrets.get(PEER_SECRET))
    }

    /// The station's own pair key, when it has one: the one it gave when it was linked, or the one a move installed.
    /// A move installs it again (a station installed again after a reset gets the same); a station without one gets a
    /// new one, kept once it has it (`keep_key`). Never the computer's one key (`PEER_SECRET`): every station moved
    /// before opens with that one, and the admin of one station, who may take its key (`/api/v1/placement/pair`),
    /// would be the admin of them all (review S2). A station on that key gets its own at its next move.
    pub fn own_key(&self) -> Option<String> {
        self.secrets.get(&station_secret(&self.key))
    }

    /// The key a move installed on the station, kept as its own: this computer opens it with it from now.
    pub fn keep_key(&self, key: &str) -> Result<(), ApiError> {
        self.secrets.set(&station_secret(&self.key), Some(key))
    }

    /// The station's own key, made now and kept when it has none — for a station set up with it (the tests' pretend
    /// ones). Not for a station that opens with the computer's one key: it would not open with this.
    pub fn peer_key(&self) -> Result<String, ApiError> {
        if let Some(key) = self.own_key() {
            return Ok(key);
        }
        let key = super::peer::new_key();
        self.keep_key(&key)?;
        Ok(key)
    }

    /// Links to a gateway on the station put there by hand: it must answer as the station's, with this key.
    pub async fn link(
        &self,
        address: &str,
        port: u16,
        key: &str,
        actor: Option<Actor>,
    ) -> Result<StationGatewayView, String> {
        let client = PeerClient::new(address, port, key);
        let health = client.health().await?;
        if health.pointer("/placement/role").and_then(Value::as_str) != Some("station") {
            return Err(crate::tr!("по этому адресу отвечает не DuoWave на колонке"));
        }
        // The key is checked by what only the pair may read.
        client.settings().await.map_err(|_| crate::tr!("колонка не приняла ключ пары"))?;
        let placement = match health.pointer("/placement/placement").and_then(Value::as_str) {
            Some("stationServer") => Placement::StationServer,
            _ => Placement::Station,
        };
        // The home's settings live at its centre, the station (docs/plans/home-and-devices.md, stage 3).
        let settings_home = crate::settings::definitions::SettingsHome::Station;
        // This station's own: another station linked later keeps its own beside it.
        self.secrets.set(&station_secret(&self.key), Some(key)).map_err(|error| error.message)?;
        let record = StationPlacement {
            placement,
            address: address.to_string(),
            port,
            device: None,
            settings_home,
            auto_update: false,
            managed: false,
            home_key_id: None,
            moved_at: Some(now_iso()),
        };
        self.set_record(Some(&record), actor).map_err(|error| error.message)?;
        self.poll().await;
        Ok(self.view())
    }

    pub fn client(&self) -> Option<PeerClient> {
        let record = self.on_station()?;
        let key = self.pair_key()?;
        Some(PeerClient::new(&record.address, record.port, &key))
    }

    pub fn live(&self) -> Live {
        self.lock().clone()
    }

    /// The station gateway's version as it said last.
    pub fn station_version(&self) -> Option<String> {
        let live = self.lock();
        live.health.as_ref()?.pointer("/build/version")?.as_str().map(str::to_string)
    }

    fn station_peer_api(&self) -> Option<u64> {
        self.lock().health.as_ref()?.pointer("/build/peerApi")?.as_u64()
    }

    /// The station's gateway answered with a PEER_API from before updates as the archive itself (0.5.1): it would
    /// not know the route, so it is updated by hand once.
    fn too_old_for_archive(&self) -> bool {
        self.station_peer_api().is_some_and(|api| api < peer::ARCHIVE_SINCE_PEER_API)
    }

    /// The version a trial on the station ended without — the new version fell or hung there, not a power cut —
    /// as its /health says (src/updates/trial.rs): the computer does not install it by itself again.
    fn failed_on_trial(&self) -> Option<String> {
        let live = self.lock();
        let failed = live.health.as_ref()?.pointer("/placement/update/trialFailed")?;
        (failed.get("why").and_then(Value::as_str) != Some("rebooted"))
            .then(|| failed.get("version").and_then(Value::as_str).map(str::to_string))
            .flatten()
    }

    fn station_revision(&self) -> Option<u64> {
        self.lock().health.as_ref()?.pointer("/placement/settingsRevision")?.as_u64()
    }

    /// What the panel hears about when it changes: reachable, the version, the revision. Each lock taken alone.
    fn signature(&self) -> (bool, Option<String>, Option<u64>) {
        let reachable = self.lock().reachable;
        (reachable, self.station_version(), self.station_revision())
    }

    /// A newer station build is inside this computer's (or in the release of this version): the version to offer.
    pub fn update_offer(&self) -> Option<String> {
        let station = self.station_version()?;
        let info = payload::info(&self.config);
        if info.source == payload::PayloadSource::None {
            return None;
        }
        crate::updates::feed::is_newer(info.version, &station).then(|| info.version.to_string())
    }

    pub fn view(&self) -> StationGatewayView {
        let record = self.record();
        let live = self.live();
        // The home's copy older than a day while the centre is on the station: «Mac не видел колонку …».
        let copy_stale = record.as_ref().is_some_and(|record| record.placement.on_station())
            && crate::home::copy::copied_at(&self.db).is_none_or(|at| older_than(&at, Duration::from_secs(86_400)));
        StationGatewayView {
            key: self.key.clone(),
            warnings: self.warnings(),
            version: self.station_version(),
            compatible: self.station_peer_api().is_none_or(|api| api == PEER_API as u64),
            update: self.update_offer(),
            payload: payload::info(&self.config),
            copied_at: crate::home::copy::copied_at(&self.db),
            record,
            live,
            copy_stale,
        }
    }

    /// One look at the station's gateway, and the sync it calls for.
    pub async fn poll(&self) {
        let Some(client) = self.client() else { return };
        let answered = self.look(&client).await.is_ok();
        self.tell_news();
        if !answered {
            self.maybe_search().await;
            return;
        }
        self.misses.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).0 = 0;
        // The home's centre moved (docs/plans/home-and-devices.md, stage 4): this station is its ear now — the computer
        // follows the centre, its address and key, as its Турбо and copy holder.
        if self.follow_centre(&client).await {
            return;
        }
        self.keep_links(&client).await;
        match self.sync(&client).await {
            Ok(()) => self.lock().sync_error = None,
            Err(error) => {
                // Into the journal once while it holds: a sending held back says the same every look.
                let again = self.lock().sync_error.as_deref() == Some(error.as_str());
                if !again {
                    tracing::warn!("placement: the settings sync with the station: {error}");
                }
                self.lock().sync_error = Some(error);
            }
        }
        self.auto_update().await;
    }

    /// The watched station became an ear of another centre: the record moves to the centre with its key. True when it
    /// moved: the next look is at the centre.
    async fn follow_centre(&self, client: &PeerClient) -> bool {
        let ear = self.live().health.as_ref().and_then(|health| health.pointer("/placement/earOf").cloned());
        if ear.is_none() {
            return false;
        }
        let Ok(answer) = client.get("/api/v1/home/ear-centre").await else { return false };
        let Some(centre) = answer.get("centre").filter(|centre| !centre.is_null()) else { return false };
        let (Some(address), Some(port), Some(key)) = (
            centre.get("address").and_then(Value::as_str),
            centre.get("port").and_then(Value::as_u64).and_then(|port| u16::try_from(port).ok()),
            centre.get("key").and_then(Value::as_str),
        ) else {
            return false;
        };
        let Some(mut record) = self.record() else { return false };
        if record.address == address && record.port == port {
            return false;
        }
        if self.keep_key(key).is_err() {
            return false;
        }
        tracing::info!("placement: the home's centre moved to {address}:{port}; this computer follows it");
        record.address = address.to_string();
        record.port = port;
        record.device = None;
        record.moved_at = Some(now_iso());
        let _ = self.set_record(Some(&record), None);
        true
    }

    /// The station gateway's /health, asked now and kept: whether it answers and what it says. The panel hears when
    /// that changed (`signature`).
    async fn look(&self, client: &PeerClient) -> Result<(), String> {
        let before = self.signature();
        let answer = client.health().await;
        let outcome = {
            let mut live = self.lock();
            live.checked_at = Some(now_iso());
            match answer {
                Ok(health) => {
                    live.reachable = true;
                    live.seen_at = live.checked_at.clone();
                    live.error = None;
                    live.health = Some(health);
                    Ok(())
                }
                Err(error) => {
                    live.reachable = false;
                    live.error = Some(error.clone());
                    Err(error)
                }
            }
        };
        if before != self.signature() {
            self.events.emit(GatewayEvent::PlacementChanged { what: "station".into() });
        }
        outcome
    }

    /// The home's copy (docs/plans/home-and-devices.md, stage 3): the station's gateway is the home's centre and the
    /// only one that writes the home; this computer keeps a copy — a new revision applied here at once, the memory
    /// merged both ways, the changes made while the centre was away sent (src/home/copy.rs). Settings never live here:
    /// a clean install has no «настройки на компьютере».
    async fn sync(&self, client: &PeerClient) -> Result<(), String> {
        if self.on_station().is_none() {
            return Ok(());
        }
        let me = self.settings.get(&GATEWAY_SELF, None).map(|me| me.id).filter(|id| !id.is_empty());
        crate::home::copy::Keeper {
            db: &self.db,
            secrets: &self.secrets,
            settings: &self.settings,
            events: &self.events,
            client,
            revision: self.station_revision(),
            me,
        }
        .keep()
        .await
    }

    pub fn set_auto_update(&self, on: bool, actor: Option<Actor>) -> Result<(), ApiError> {
        let Some(mut record) = self.on_station() else {
            return Err(ApiError::conflict(crate::tr!("Центр не на колонке.")));
        };
        record.auto_update = on;
        self.store(&record, actor)?;
        self.events.emit(GatewayEvent::PlacementChanged { what: "autoUpdate".into() });
        Ok(())
    }

    /// «Обновить колонку»: in the background, the panel follows it in the view.
    pub fn start_update(self: &Arc<Self>, actor: Option<Actor>) -> Result<(), ApiError> {
        if self.update_offer().is_none() {
            return Err(ApiError::conflict(crate::tr!("У колонки последняя версия.")));
        }
        if self.too_old_for_archive() {
            return Err(ApiError::conflict(peer::old_station()));
        }
        {
            let mut live = self.lock();
            if live.updating {
                return Err(ApiError::conflict(crate::tr!("Колонка уже обновляется.")));
            }
            live.updating = true;
            live.update_error = None;
        }
        let me = self.clone();
        tokio::spawn(async move {
            let outcome = me.update_station(actor).await;
            {
                let mut live = me.lock();
                live.updating = false;
                live.update_error = outcome.err();
            }
            me.events.emit(GatewayEvent::PlacementChanged { what: "update".into() });
        });
        Ok(())
    }

    /// The station's gateway updated from this computer (the home's copy is here already, src/home/copy.rs): the signed
    /// archive over the home network; the station checks the signature with its own key, puts it in place and starts again.
    pub async fn update_station(&self, actor: Option<Actor>) -> Result<String, String> {
        let client = self.client().ok_or(crate::tr!("центр не на колонке"))?;
        let version = self.update_offer().ok_or(crate::tr!("у колонки последняя версия"))?;
        if self.too_old_for_archive() {
            return Err(peer::old_station());
        }
        let payload = payload::load(&self.config).await?;
        let outcome = client.upload_update(&payload.version, &payload.archive, &payload.signature).await;
        let (result, error) = match &outcome {
            Ok(_) => ("done", None),
            Err(error) => ("failed", Some(error.clone())),
        };
        self.events.emit(GatewayEvent::StationGatewayUpdated {
            version: version.clone(),
            result: result.into(),
            error,
            actor,
        });
        outcome.map(|_| version)
    }

    async fn auto_update(&self) {
        let Some(record) = self.on_station() else { return };
        if !record.auto_update || !record.managed {
            return;
        }
        let Some(version) = self.update_offer() else { return };
        // It did not make it through its trial on the station: «Обновить» by hand still may, not this.
        if self.failed_on_trial().is_some_and(|failed| !crate::updates::feed::is_newer(&version, &failed)) {
            return;
        }
        {
            let mut tried = self.auto_tried.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if tried.as_deref() == Some(version.as_str()) {
                return;
            }
            *tried = Some(version.clone());
        }
        match self.update_station(None).await {
            Ok(version) => tracing::info!("placement: the station's gateway updated to {version} by itself"),
            Err(error) => tracing::warn!("placement: the station's gateway not updated: {error}"),
        }
    }

    /// The station's placement, for the gateway's /health and the tray.
    pub fn placement(&self) -> Placement {
        self.record().map(|record| record.placement).unwrap_or(Placement::Pc)
    }
}

/// The key a station's placement is kept under: its device when known, else its address — a gateway linked by hand
/// before the station ever spoke to this computer.
pub fn key_of(device: Option<&str>, address: &str) -> String {
    match device.filter(|device| !device.is_empty()) {
        Some(device) => device.to_string(),
        None => format!("at:{address}"),
    }
}

/// The computer's watches over the stations' gateways, one per station with a placement of its own
/// (docs/plans/placement-per-station.md). What the panel and the moves ask about a station is asked of its watch.
pub struct StationGateways {
    config: Arc<Config>,
    db: Arc<Db>,
    settings: Arc<SettingsService>,
    secrets: Arc<SecretsService>,
    events: EventBus,
    watches: Mutex<BTreeMap<String, Arc<StationGateway>>>,
    teller: OnceLock<Teller>,
    home_addresses: OnceLock<HomeAddresses>,
}

impl StationGateways {
    pub fn new(
        config: Arc<Config>,
        db: Arc<Db>,
        settings: Arc<SettingsService>,
        secrets: Arc<SecretsService>,
        events: EventBus,
    ) -> Arc<StationGateways> {
        let gateways = Arc::new(StationGateways {
            config,
            db,
            settings,
            secrets,
            events,
            watches: Mutex::new(BTreeMap::new()),
            teller: OnceLock::new(),
            home_addresses: OnceLock::new(),
        });
        gateways.adopt_the_one_before();
        for key in gateways.settings.get(&STATION_PLACEMENTS, None).unwrap_or_default().into_keys() {
            gateways.watch(&key);
        }
        gateways
    }

    /// The one placement kept before each station had its own becomes its station's, with its copies.
    fn adopt_the_one_before(&self) {
        let Some(old) = self.settings.get(&STATION_PLACEMENT, None) else { return };
        let key = key_of(old.device.as_deref(), &old.address);
        let mut all = self.settings.get(&STATION_PLACEMENTS, None).unwrap_or_default();
        all.entry(key.clone()).or_insert(old);
        if self.settings.set(&STATION_PLACEMENTS, &all, None).is_ok() {
            let _ = self.settings.forget(&STATION_PLACEMENT);
            let state = self.config.skills.state_dir.as_deref();
            Backups::for_station(state, &key).adopt_common(state);
            tracing::info!("placement: the computer's one placement is now the station's {key}");
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Arc<StationGateway>>> {
        self.watches.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The station's watch, made when there is none yet (a move to a station new to this computer).
    pub fn watch(&self, key: &str) -> Arc<StationGateway> {
        let mut watches = self.lock();
        if let Some(watch) = watches.get(key) {
            return watch.clone();
        }
        let watch = StationGateway::new(
            self.config.clone(),
            self.db.clone(),
            self.settings.clone(),
            self.secrets.clone(),
            self.events.clone(),
            key,
        );
        if let Some(teller) = self.teller.get() {
            watch.set_teller(teller.clone());
        }
        if let Some(addresses) = self.home_addresses.get() {
            watch.set_home_addresses(addresses.clone());
        }
        watches.insert(key.to_string(), watch.clone());
        watch
    }

    /// The stations with a placement here.
    pub fn all(&self) -> Vec<Arc<StationGateway>> {
        self.lock().values().filter(|watch| watch.record().is_some()).cloned().collect()
    }

    /// The station with a placement here under this key, without making a watch for a key nobody knows (the helper's
    /// pass takes any key from the address).
    pub fn find(&self, key: &str) -> Option<Arc<StationGateway>> {
        self.all().into_iter().find(|watch| watch.key() == key)
    }

    /// The station asked about; with none named, the one with a placement here — when it is the only one.
    pub fn pick(&self, key: Option<&str>) -> Option<Arc<StationGateway>> {
        match key.filter(|key| !key.is_empty()) {
            Some(key) => Some(self.watch(key)),
            None => {
                let all = self.all();
                (all.len() == 1).then(|| all[0].clone())
            }
        }
    }

    /// The station whose gateway runs on it at this address (the move back finds it so).
    pub fn at_address(&self, address: &str) -> Option<Arc<StationGateway>> {
        self.all().into_iter().find(|watch| watch.record().is_some_and(|record| record.address == address))
    }

    pub fn views(&self) -> Vec<StationGatewayView> {
        self.all().iter().map(|watch| watch.view()).collect()
    }

    /// No station of this computer's is to come to its /uni.ws: some station's record says its gateway runs on it, and
    /// every station approved here has such a record. The station watch is silent about «не подключена» then
    /// (src/operations/station_watch.rs): the station talks to its own gateway (live, 10.11, it warned all the same).
    pub fn none_here(&self) -> bool {
        let records = self.settings.get(&STATION_PLACEMENTS, None).unwrap_or_default();
        let elsewhere: Vec<(&String, &StationPlacement)> =
            records.iter().filter(|(_, record)| record.placement.on_station()).collect();
        if elsewhere.is_empty() {
            return false;
        }
        let approved: Vec<String> = self
            .db
            .with(|connection| {
                connection
                    .prepare("SELECT device FROM stations WHERE status = 'approved'")?
                    .query_map([], |row| row.get::<_, String>(0))?
                    .collect()
            })
            .unwrap_or_default();
        approved.iter().all(|device| {
            elsewhere.iter().any(|(key, record)| *key == device || record.device.as_deref() == Some(device.as_str()))
        })
    }

    pub fn set_teller(&self, teller: Teller) {
        let _ = self.teller.set(teller.clone());
        for watch in self.lock().values() {
            watch.set_teller(teller.clone());
        }
    }

    pub fn set_home_addresses(&self, addresses: HomeAddresses) {
        let _ = self.home_addresses.set(addresses.clone());
        for watch in self.lock().values() {
            watch.set_home_addresses(addresses.clone());
        }
    }

    /// A gateway put on a station by hand: its watch is by its address until the station is known by its device.
    pub async fn link(
        &self,
        address: &str,
        port: u16,
        key: &str,
        actor: Option<Actor>,
    ) -> Result<StationGatewayView, String> {
        self.watch(&key_of(None, address)).link(address, port, key, actor).await
    }

    /// Every 30 s, each station whose gateway runs on it.
    pub fn start(self: &Arc<Self>) {
        let me = self.clone();
        tokio::spawn(async move {
            loop {
                for watch in me.all() {
                    if watch.on_station().is_some() {
                        watch.poll().await;
                    }
                }
                tokio::time::sleep(POLL).await;
            }
        });
    }
}

/// Of this computer's addresses («192.168.1.20», «…:3000»), the one in the station's network — the same first three
/// numbers — else the first: the station dials it without anybody typing it.
pub fn pick_home(addresses: &[String], station: &str) -> Option<String> {
    let net = |address: &str| address.split(':').next().unwrap_or("").rsplit_once('.').map(|(net, _)| net.to_string());
    let station_net = net(station);
    addresses.iter().find(|address| station_net.is_some() && net(address) == station_net).or(addresses.first()).cloned()
}

/// «14:02» today, else with the date: as the household reads it.
fn local_time(at: &str) -> String {
    match at.parse::<chrono::DateTime<chrono::Utc>>() {
        Ok(at) => {
            let local = at.with_timezone(&chrono::Local);
            if local.date_naive() == chrono::Local::now().date_naive() {
                local.format("%H:%M").to_string()
            } else {
                local.format("%d.%m %H:%M").to_string()
            }
        }
        Err(_) => at.to_string(),
    }
}

fn older_than(at: &str, age: Duration) -> bool {
    at.parse::<chrono::DateTime<chrono::Utc>>()
        .map(|at| chrono::Utc::now().signed_duration_since(at).to_std().unwrap_or_default() > age)
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::routing::{get, post};
    use axum::Router;

    use super::*;
    use crate::config::Env;
    use crate::placement::transfer;
    use crate::settings::definitions::SettingsHome;
    use crate::settings::definitions::{ModeSetting, PanelMode, KEEP_REQUESTS, MODE};

    /// A pretend station gateway of this build: /health with a revision, its settings sealed with the pair's key, and
    /// what it was sent.
    async fn station(key: &str, revision: Arc<AtomicUsize>, received: Arc<Mutex<Vec<transfer::Bundle>>>) -> u16 {
        pretend(key, Pretend::current(revision, received)).await
    }

    /// What a pretend station gateway says and holds, and what the computer asked of it.
    struct Pretend {
        revision: Arc<AtomicUsize>,
        received: Arc<Mutex<Vec<transfer::Bundle>>>,
        version: Mutex<String>,
        /// The schema its import knows: a package that needs a newer one is refused as every build's route refuses it
        /// (routes.rs, `import`), before anything is kept.
        knows: AtomicUsize,
        /// What its /health says it knows (`build.settingsSchema`): a build from before it was said, nothing.
        says: Mutex<Option<u32>>,
        /// Its own settings as its package carries them. At an earlier build a package taken goes into them as that
        /// build's import applies it (`transfer::earlier_import`).
        own: Mutex<BTreeMap<String, Value>>,
        /// «GET transfer», «POST transfer», «refused», «home station»: what the computer asked of it, in order.
        calls: Mutex<Vec<String>>,
    }

    impl Pretend {
        /// A station's gateway of this build: it knows this build's schema and says so.
        fn current(revision: Arc<AtomicUsize>, received: Arc<Mutex<Vec<transfer::Bundle>>>) -> Arc<Pretend> {
            Arc::new(Pretend {
                revision,
                received,
                version: Mutex::new("0.0.1".into()),
                knows: AtomicUsize::new(transfer::SCHEMA as usize),
                says: Mutex::new(Some(transfer::SCHEMA)),
                own: Mutex::new(BTreeMap::new()),
                calls: Mutex::new(Vec::new()),
            })
        }

        fn fresh() -> Arc<Pretend> {
            Pretend::current(Arc::new(AtomicUsize::new(1)), Arc::new(Mutex::new(Vec::new())))
        }

        fn received(&self) -> Vec<transfer::Bundle> {
            self.received.lock().unwrap().clone()
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    async fn pretend(key: &str, station: Arc<Pretend>) -> u16 {
        let db = Arc::new(Db::memory());
        let secrets = Arc::new(SecretsService::new(db.clone(), None).unwrap());
        let (key_out, key_in) = (key.to_string(), key.to_string());
        let (seen, read, sent, told) = (station.clone(), station.clone(), station.clone(), station);
        // The home's copy beside the package (src/home/copy.rs): its memory both ways and who holds a copy.
        let router = Router::new()
            .route(
                "/api/v1/home/memory",
                get(|| async { axum::Json(serde_json::json!({"message": {"rows": {}, "newest": null}})) })
                    .post(|| async { axum::Json(serde_json::json!({"message": {"merged": 0}})) }),
            )
            .route("/api/v1/home/held", post(|| async { axum::Json(serde_json::json!({"message": {"ok": true}})) }))
            .route(
                "/health",
                get(move || {
                    let version = seen.version.lock().unwrap().clone();
                    let mut build = serde_json::json!({"version": version, "peerApi": PEER_API});
                    if let Some(schema) = *seen.says.lock().unwrap() {
                        build["settingsSchema"] = serde_json::json!(schema);
                    }
                    let revision = seen.revision.load(Ordering::Relaxed);
                    async move {
                        axum::Json(serde_json::json!({"message": {
                            "status": "ok",
                            "build": build,
                            "placement": {"role": "station", "settingsRevision": revision}
                        }}))
                    }
                }),
            )
            .route(
                "/api/v1/transfer",
                get(move || {
                    read.calls.lock().unwrap().push("GET transfer".into());
                    let mut bundle = transfer::export(&db, &secrets, "station").unwrap();
                    bundle.revision = read.revision.load(Ordering::Relaxed) as u64;
                    bundle.settings.extend(read.own.lock().unwrap().clone());
                    let sealed = transfer::seal(&bundle, &key_out);
                    async move { axum::Json(serde_json::json!({ "message": sealed })) }
                })
                .post(move |axum::Json(sealed): axum::Json<Value>| {
                    let bundle = transfer::open(&sealed, &key_in).unwrap();
                    let knows = sent.knows.load(Ordering::Relaxed);
                    let newer = bundle.schema as usize > knows;
                    sent.calls.lock().unwrap().push(if newer { "refused" } else { "POST transfer" }.into());
                    if !newer {
                        if knows < transfer::KEYS_IN_SETTINGS_SCHEMA as usize {
                            transfer::earlier_import(&mut sent.own.lock().unwrap(), &bundle).unwrap();
                        }
                        sent.received.lock().unwrap().push(bundle);
                    }
                    async move {
                        if newer {
                            let refusal = serde_json::json!({ "message": transfer::NOT_FOR_THIS_GATEWAY });
                            (axum::http::StatusCode::BAD_REQUEST, axum::Json(refusal))
                        } else {
                            let applied = serde_json::json!({"message": {"applied": "restart"}});
                            (axum::http::StatusCode::CREATED, axum::Json(applied))
                        }
                    }
                }),
            )
            .route(
                "/api/v1/placement/settings-home",
                post(move |axum::Json(body): axum::Json<Value>| {
                    told.calls.lock().unwrap().push(format!("home {}", body["home"].as_str().unwrap_or("?")));
                    async { axum::Json(serde_json::json!({"message": {}})) }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        port
    }

    fn computer(dir: &std::path::Path) -> (Arc<StationGateway>, Arc<SettingsService>) {
        let config = Arc::new(Config::for_tests(&Env::from_pairs([("STATE_DIR", dir.to_string_lossy().into_owned())])));
        let db = Arc::new(Db::memory());
        let secrets = Arc::new(SecretsService::new(db.clone(), None).unwrap());
        let settings = Arc::new(SettingsService::new(db.clone(), EventBus::new()));
        (StationGateway::new(config, db, settings.clone(), secrets, EventBus::new(), "dev1"), settings)
    }

    fn placed(address: &str, device: Option<&str>, placement: Placement) -> StationPlacement {
        StationPlacement {
            placement,
            address: address.into(),
            port: 8790,
            device: device.map(str::to_string),
            settings_home: SettingsHome::Station,
            auto_update: false,
            managed: true,
            home_key_id: None,
            moved_at: None,
        }
    }

    fn gateways(dir: &std::path::Path, settings: Arc<SettingsService>, db: Arc<Db>) -> Arc<StationGateways> {
        let config = Arc::new(Config::for_tests(&Env::from_pairs([("STATE_DIR", dir.to_string_lossy().into_owned())])));
        let secrets = Arc::new(SecretsService::new(db.clone(), None).unwrap());
        StationGateways::new(config, db, settings, secrets, EventBus::new())
    }

    #[test]
    fn the_computers_one_placement_becomes_its_stations_with_its_copies() {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Db::memory());
        let settings = Arc::new(SettingsService::new(db.clone(), EventBus::new()));
        settings.set(&STATION_PLACEMENT, &placed("192.0.2.23", Some("h:kitchen"), Placement::Station), None).unwrap();
        let common = dir.path().join("station-backups");
        std::fs::create_dir_all(&common).unwrap();
        std::fs::write(common.join("20261107-010000-r3.json"), b"{}").unwrap();

        let all = gateways(dir.path(), settings.clone(), db);
        assert!(settings.get(&STATION_PLACEMENT, None).is_none(), "the old record is gone");
        let kitchen = all.pick(None).expect("the only one with a placement");
        assert_eq!((kitchen.key(), kitchen.record().unwrap().address.as_str()), ("h:kitchen", "192.0.2.23"));
        assert_eq!(kitchen.backups.list().len(), 1, "its copies moved into its folder");
        assert!(!common.join("20261107-010000-r3.json").exists());
    }

    #[test]
    fn two_stations_each_its_own_placement_and_a_move_of_one_leaves_the_other() {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Db::memory());
        let settings = Arc::new(SettingsService::new(db.clone(), EventBus::new()));
        let all = gateways(dir.path(), settings.clone(), db);
        let kitchen = all.watch("h:kitchen");
        let bedroom = all.watch("h:bedroom");
        kitchen.set_record(Some(&placed("192.0.2.23", Some("h:kitchen"), Placement::Station)), None).unwrap();
        bedroom.set_record(Some(&placed("192.0.2.24", Some("h:bedroom"), Placement::Pc)), None).unwrap();

        assert!(all.pick(None).is_none(), "two stations: which one must be said");
        assert_eq!(all.pick(Some("h:bedroom")).unwrap().record().unwrap().placement, Placement::Pc);
        assert_eq!(all.at_address("192.0.2.23").unwrap().key(), "h:kitchen");
        assert!(kitchen.on_station().is_some() && bedroom.on_station().is_none());

        kitchen.set_record(None, None).unwrap();
        assert_eq!(bedroom.record().unwrap().address, "192.0.2.24", "the other station's stays");
        assert_eq!(all.views().len(), 1);
        assert_eq!(key_of(None, "192.0.2.25"), "at:192.0.2.25");
    }

    #[tokio::test]
    async fn a_station_that_does_not_answer_is_said_so() {
        let dir = tempfile::tempdir().unwrap();
        let (watch, _) = computer(dir.path());
        watch.peer_key().unwrap();
        let record = StationPlacement {
            placement: Placement::StationServer,
            address: "127.0.0.1".into(),
            port: 9,
            device: None,
            settings_home: SettingsHome::Station,
            auto_update: false,
            managed: true,
            home_key_id: None,
            moved_at: None,
        };
        watch.set_record(Some(&record), None).unwrap();
        let told = Arc::new(Mutex::new(Vec::new()));
        let heard = told.clone();
        watch.set_teller(Arc::new(move |text| heard.lock().unwrap().push(text)));
        watch.poll().await;
        watch.poll().await;
        let view = watch.view();
        assert!(!view.live.reachable);
        assert!(view.live.error.is_some());
        assert!(view.copy_stale, "no copy at all");
        assert_eq!(view.warnings.iter().map(|warning| warning.kind).collect::<Vec<_>>(), vec!["silent"]);
        assert_eq!(told.lock().unwrap().len(), 1, "told once while it holds");
    }

    /// Live, 10.11: «Шлюз на колонке не отвечает» came right after the move — the fresh record had heard nothing yet,
    /// and the station reboots for a minute. The 2 minutes count from the move.
    #[tokio::test]
    async fn a_station_just_moved_is_not_silent_before_two_minutes() {
        let dir = tempfile::tempdir().unwrap();
        let (watch, _) = computer(dir.path());
        watch.peer_key().unwrap();
        let told = Arc::new(Mutex::new(Vec::new()));
        let heard = told.clone();
        watch.set_teller(Arc::new(move |text| heard.lock().unwrap().push(text)));
        let mut record = placed("127.0.0.1", Some("dev1"), Placement::StationServer);
        record.port = 9;
        record.moved_at = Some(now_iso());
        watch.set_record(Some(&record), None).unwrap();
        watch.poll().await;
        assert!(!watch.live().reachable);
        assert!(watch.warnings().is_empty(), "{:?}", watch.warnings());
        assert!(told.lock().unwrap().is_empty());

        let long_ago = (chrono::Utc::now() - chrono::Duration::minutes(3)).to_rfc3339();
        record.moved_at = Some(long_ago);
        watch.set_record(Some(&record), None).unwrap();
        watch.poll().await;
        assert_eq!(watch.warnings().iter().map(|warning| warning.kind).collect::<Vec<_>>(), vec!["silent"]);
        assert_eq!(told.lock().unwrap().len(), 1);
    }

    /// Live, 10.11: the computer's station watch told «не подключена» while the station talked to its own gateway.
    /// No station is to come to this computer's /uni.ws while each approved one has its gateway on it.
    #[test]
    fn no_station_is_expected_here_while_each_runs_its_own_gateway() {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Db::memory());
        let settings = Arc::new(SettingsService::new(db.clone(), EventBus::new()));
        let approve = |device: &str| {
            db.with(|c| {
                c.execute(
                    "INSERT INTO stations (device, name, status, first_seen, last_seen) VALUES (?1, 'Детская', 'approved', 'then', 'then')",
                    rusqlite::params![device],
                )
            })
            .unwrap();
        };
        approve("h:nursery");
        let all = gateways(dir.path(), settings.clone(), db.clone());
        assert!(!all.none_here(), "no record: the station comes here");
        all.watch("h:nursery").set_record(Some(&placed("192.0.2.23", Some("h:nursery"), Placement::Pc)), None).unwrap();
        assert!(!all.none_here(), "its record says «pc»");
        all.watch("h:nursery")
            .set_record(Some(&placed("192.0.2.23", Some("h:nursery"), Placement::StationServer)), None)
            .unwrap();
        assert!(all.none_here(), "its gateway is on it");
        assert_eq!(all.watch("h:nursery").name().as_deref(), Some("Детская"));
        // A second station approved here, its voice at this computer: it is missed again.
        approve("h:kitchen");
        assert!(!all.none_here());
    }

    #[tokio::test]
    async fn a_second_station_linked_keeps_its_own_pair_key_beside_the_firsts() {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Db::memory());
        let settings = Arc::new(SettingsService::new(db.clone(), EventBus::new()));
        let all = gateways(dir.path(), settings, db);
        let keys = ["the-kitchens-own-pair-key-0123456789", "the-bedrooms-own-pair-key-0123456789"];
        let mut watches = Vec::new();
        for (name, key) in ["h:kitchen", "h:bedroom"].into_iter().zip(keys) {
            let port = station(key, Arc::new(AtomicUsize::new(1)), Arc::new(Mutex::new(Vec::new()))).await;
            let watch = all.watch(name);
            watch.link("127.0.0.1", port, key, None).await.unwrap();
            watches.push(watch);
        }
        for (watch, key) in watches.iter().zip(keys) {
            assert_eq!(watch.pair_key().as_deref(), Some(key));
            assert!(watch.client().unwrap().settings().await.is_ok(), "{} opens with its own key", watch.key());
            watch.poll().await;
            assert!(watch.live().reachable && watch.live().sync_error.is_none(), "{:?}", watch.live());
        }
        assert!(watches[0].secrets.get(PEER_SECRET).is_none(), "the computer's own key is not touched");

        // A station from before each kept its own: it opens with the computer's key, so do its copies.
        let before = "the-computers-one-key-from-before-0123";
        let port = station(before, Arc::new(AtomicUsize::new(1)), Arc::new(Mutex::new(Vec::new()))).await;
        watches[0].secrets.set(PEER_SECRET, Some(before)).unwrap();
        let hall = all.watch("h:hall");
        hall.set_record(Some(&on_station(port, Placement::Station, None)), None).unwrap();
        assert_eq!(hall.pair_key().as_deref(), Some(before));
        assert!(hall.client().unwrap().settings().await.is_ok());
        hall.poll().await;
        assert_eq!(watches[0].pair_key().as_deref(), Some(keys[0]), "its own before the computer's");

        // Its next move gives it a key of its own (kept once installed), never the computer's one: whoever is the
        // admin of one station (and may take its key) is not the admin of every station this computer moved (S2).
        assert_eq!(hall.own_key(), None, "none of its own yet");
        let own = crate::placement::peer::new_key();
        hall.keep_key(&own).unwrap();
        assert_eq!(hall.pair_key().as_deref(), Some(own.as_str()));
        assert_eq!(hall.secrets.get(PEER_SECRET).as_deref(), Some(before), "the computer's one key is not touched");
        let porch = all.watch("h:porch");
        let porchs = porch.peer_key().unwrap();
        assert!(porchs != own && porchs != before, "each station its own");
        assert_eq!(porch.own_key(), Some(porchs));

        // The pass asks by any key: one nobody linked makes no watch.
        assert!(all.find("h:nobody").is_none());
        assert!(!all.lock().contains_key("h:nobody"));
        assert_eq!(all.find("h:hall").map(|watch| watch.key().to_string()).as_deref(), Some("h:hall"));
    }

    /// The home's copy (docs/plans/home-and-devices.md, stage 3): a new revision at the centre is taken and applied
    /// here at once — the station's settings become the computer's, nothing goes the other way; the same revision is
    /// not taken twice.
    #[tokio::test]
    async fn takes_the_homes_copy_when_the_centre_names_a_new_revision_and_sends_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (watch, settings) = computer(dir.path());
        let key = watch.peer_key().unwrap();
        let station = Pretend::fresh();
        station.own.lock().unwrap().insert("requests_keep_days".into(), serde_json::json!(7));
        station.own.lock().unwrap().insert("gateway_mode".into(), serde_json::json!({ "mode": "relay" }));
        let port = pretend(&key, station.clone()).await;
        watch.set_record(Some(&on_station(port, Placement::StationServer, None)), None).unwrap();
        settings.set(&MODE, &ModeSetting { mode: PanelMode::Own }, None).unwrap();

        watch.poll().await;
        assert_eq!(settings.get(&KEEP_REQUESTS, None), Some(7), "{:?}", watch.live().sync_error);
        assert_eq!(settings.get(&MODE, None).map(|mode| mode.mode), Some(PanelMode::Relay), "the home's mode with it");
        assert!(watch.view().copied_at.is_some());
        watch.poll().await;
        assert_eq!(station.calls(), vec!["GET transfer"], "the same revision: taken once, nothing sent");
        assert!(station.received().is_empty());
        station.revision.store(2, Ordering::Relaxed);
        station.own.lock().unwrap().insert("requests_keep_days".into(), serde_json::json!(9));
        watch.poll().await;
        assert_eq!(settings.get(&KEEP_REQUESTS, None), Some(9), "a change at the centre reaches the copy");
    }

    #[tokio::test]
    async fn links_to_a_station_gateway_put_there_by_hand_only_with_its_key() {
        let dir = tempfile::tempdir().unwrap();
        let (watch, _) = computer(dir.path());
        let revision = Arc::new(AtomicUsize::new(1));
        let port = station("the-pairs-key-by-hand-0123456789", revision, Arc::new(Mutex::new(Vec::new()))).await;
        let refused = watch.link("127.0.0.1", port, "another-key-entirely-0123456789", None).await;
        assert_eq!(refused.unwrap_err(), "колонка не приняла ключ пары");
        assert!(watch.record().is_none(), "nothing kept");
        let view = watch.link("127.0.0.1", port, "the-pairs-key-by-hand-0123456789", None).await.unwrap();
        let record = view.record.unwrap();
        assert_eq!((record.placement, record.port, record.managed), (Placement::Station, port, false));
        assert!(view.live.reachable);
        assert!(view.copied_at.is_some(), "the home's copy at once: {:?}", view.live.sync_error);
    }

    /// A pretend station gateway with a home server: its placement and the computer's address as /health tells
    /// them, and what /api/v1/home-server/link was sent.
    #[derive(Default)]
    struct Helped {
        placement: Mutex<String>,
        url: Mutex<Option<String>>,
        links: Mutex<Vec<Value>>,
        /// A station of this build: its helpers in /health, and its entry of this computer switched on or not.
        helpers: Mutex<Option<bool>>,
    }

    async fn helped_station(key: &str, state: Arc<Helped>) -> u16 {
        let db = Arc::new(Db::memory());
        let secrets = Arc::new(SecretsService::new(db.clone(), None).unwrap());
        let key_out = key.to_string();
        let (seen, linked) = (state.clone(), state);
        let router = Router::new()
            .route(
                "/health",
                get(move || {
                    let placement = seen.placement.lock().unwrap().clone();
                    let url = seen.url.lock().unwrap().clone();
                    let helpers = *seen.helpers.lock().unwrap();
                    async move {
                        let mut health = serde_json::json!({"message": {
                            "build": {"version": "0.0.1", "peerApi": PEER_API},
                            "placement": {"role": "station", "placement": placement, "settingsRevision": 1,
                                          "homeServer": url.map(|url| serde_json::json!({"url": url}))}
                        }});
                        if let Some(enabled) = helpers {
                            let placement = &mut health["message"]["placement"];
                            placement["helpers"] = serde_json::json!({"away": "any", "list": []});
                            placement["homeServer"]["enabled"] = serde_json::json!(enabled);
                        }
                        axum::Json(health)
                    }
                }),
            )
            .route(
                "/api/v1/transfer",
                get(move || {
                    let bundle = transfer::export(&db, &secrets, "station").unwrap();
                    let sealed = transfer::seal(&bundle, &key_out);
                    async move { axum::Json(serde_json::json!({ "message": sealed })) }
                }),
            )
            .route(
                "/api/v1/home-server/link",
                post(move |axum::Json(body): axum::Json<Value>| {
                    if let Some(url) = body.get("url").and_then(Value::as_str) {
                        *linked.url.lock().unwrap() = Some(url.to_string());
                    }
                    if let Some(on) = body.get("enabled").and_then(Value::as_bool) {
                        *linked.placement.lock().unwrap() = if on { "stationServer" } else { "station" }.into();
                    }
                    linked.links.lock().unwrap().push(body);
                    async { axum::Json(serde_json::json!({"message": {}})) }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        port
    }

    fn on_station(port: u16, placement: Placement, home_key_id: Option<&str>) -> StationPlacement {
        StationPlacement {
            placement,
            address: "127.0.0.1".into(),
            port,
            device: None,
            settings_home: SettingsHome::Station,
            auto_update: false,
            managed: true,
            home_key_id: home_key_id.map(str::to_string),
            moved_at: None,
        }
    }

    #[test]
    fn the_computers_address_in_the_stations_own_network_is_picked_nobody_types_it() {
        let addresses = vec!["10.8.0.2".to_string(), "192.0.2.20:3000".to_string()];
        assert_eq!(pick_home(&addresses, "192.0.2.23").as_deref(), Some("192.0.2.20:3000"));
        assert_eq!(pick_home(&addresses, "172.16.0.5").as_deref(), Some("10.8.0.2"), "none in its network: the first");
        assert_eq!(pick_home(&[], "192.0.2.23"), None);
    }

    #[tokio::test]
    async fn the_switch_brings_the_address_and_a_key_once_then_only_turns_on_and_off() {
        let dir = tempfile::tempdir().unwrap();
        let (watch, _) = computer(dir.path());
        watch.set_home_addresses(Arc::new(|| vec!["127.0.0.5:3000".to_string()]));
        let key = watch.peer_key().unwrap();
        let state = Arc::new(Helped::default());
        *state.placement.lock().unwrap() = "station".into();
        let port = helped_station(&key, state.clone()).await;
        watch.set_record(Some(&on_station(port, Placement::Station, None)), None).unwrap();
        watch.poll().await;
        assert!(watch.needs_home_key(), "the station does not know this computer yet");

        watch.set_helper(true, Some(("vgw_home_secret".into(), "key-1".into())), None).await.unwrap();
        let sent = state.links.lock().unwrap()[0].clone();
        assert_eq!(
            sent,
            serde_json::json!({"enabled": true, "url": "http://127.0.0.5:3000", "key": "vgw_home_secret"})
        );
        let record = watch.record().unwrap();
        assert_eq!((record.placement, record.home_key_id.as_deref()), (Placement::StationServer, Some("key-1")));
        assert!(!watch.needs_home_key());

        watch.set_helper(false, None, None).await.unwrap();
        assert_eq!(state.links.lock().unwrap()[1], serde_json::json!({"enabled": false}), "no address, no key again");
        assert_eq!(watch.record().unwrap().placement, Placement::Station);
    }

    #[tokio::test]
    async fn the_computers_new_address_goes_to_the_station_and_its_own_switch_shows_here() {
        let dir = tempfile::tempdir().unwrap();
        let (watch, _) = computer(dir.path());
        watch.set_home_addresses(Arc::new(|| vec!["127.0.0.35".to_string()]));
        let key = watch.peer_key().unwrap();
        let state = Arc::new(Helped::default());
        // The station's panel turned the switch on; it still dials the computer's old address.
        *state.placement.lock().unwrap() = "stationServer".into();
        *state.url.lock().unwrap() = Some("http://127.0.0.20".into());
        let port = helped_station(&key, state.clone()).await;
        watch.set_record(Some(&on_station(port, Placement::Station, Some("key-1"))), None).unwrap();
        watch.poll().await;
        assert_eq!(watch.record().unwrap().placement, Placement::StationServer);
        assert_eq!(state.links.lock().unwrap().clone(), vec![serde_json::json!({"url": "http://127.0.0.35"})]);
        watch.poll().await;
        assert_eq!(state.links.lock().unwrap().len(), 1, "told once: the station dials the new one now");
    }

    /// A station of this build leans on other helpers too: its «the computer helps» is its entry of this computer,
    /// not its placement — the switch here follows `homeServer.enabled`.
    #[tokio::test]
    async fn a_new_stations_switch_follows_its_home_computer_not_its_other_helpers() {
        let dir = tempfile::tempdir().unwrap();
        let (watch, _) = computer(dir.path());
        let key = watch.peer_key().unwrap();
        let state = Arc::new(Helped::default());
        *state.placement.lock().unwrap() = "stationServer".into();
        *state.url.lock().unwrap() = Some("http://127.0.0.20".into());
        *state.helpers.lock().unwrap() = Some(false);
        let port = helped_station(&key, state.clone()).await;
        watch.set_record(Some(&on_station(port, Placement::StationServer, None)), None).unwrap();
        watch.poll().await;
        assert_eq!(watch.record().unwrap().placement, Placement::Station, "its computer's help is off");
        *state.helpers.lock().unwrap() = Some(true);
        watch.poll().await;
        assert_eq!(watch.record().unwrap().placement, Placement::StationServer);
    }

    #[tokio::test]
    async fn a_station_whose_key_is_another_keeps_its_address_of_another_computer() {
        let dir = tempfile::tempdir().unwrap();
        let (watch, _) = computer(dir.path());
        watch.set_home_addresses(Arc::new(|| vec!["127.0.0.35".to_string()]));
        let key = watch.peer_key().unwrap();
        let state = Arc::new(Helped::default());
        *state.placement.lock().unwrap() = "stationServer".into();
        *state.url.lock().unwrap() = Some("http://127.0.0.20".into());
        let port = helped_station(&key, state.clone()).await;
        // Linked by hand: this computer never gave the station a key.
        watch.set_record(Some(&on_station(port, Placement::StationServer, None)), None).unwrap();
        watch.poll().await;
        assert!(state.links.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_station_found_anew_on_the_network_is_ours_only_when_it_opens_with_the_pairs_key() {
        let dir = tempfile::tempdir().unwrap();
        let (watch, _) = computer(dir.path());
        let key = watch.peer_key().unwrap();
        let state = Arc::new(Helped::default());
        *state.placement.lock().unwrap() = "station".into();
        let port = helped_station(&key, state).await;
        let mut record = on_station(port, Placement::Station, None);
        record.address = "192.0.2.113".into();
        watch.set_record(Some(&record), None).unwrap();
        assert_eq!(watch.find_among(vec!["127.0.0.1".into()]).await.as_deref(), Some("127.0.0.1"));
        assert_eq!(watch.record().unwrap().address, "127.0.0.1");

        let dir = tempfile::tempdir().unwrap();
        let (stranger, _) = computer(dir.path());
        stranger.peer_key().unwrap();
        record.address = "192.0.2.113".into();
        stranger.set_record(Some(&record), None).unwrap();
        assert_eq!(stranger.find_among(vec!["127.0.0.1".into()]).await, None, "another pair's station");
        assert_eq!(stranger.record().unwrap().address, "192.0.2.113");
    }
}
