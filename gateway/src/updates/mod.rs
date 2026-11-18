//! Updates (new in the Rust gateway): the gateway learns of a new version from the release feed, offers it — in the
//! panel and, in the desktop app, in the tray menu — and installs it only when a person with `system.update` agrees.
//! An urgent release is the exception: the feed marks the versions it fixes (critical_below), and a gateway below
//! that installs it by itself on its next check (UPDATE_AUTO_CRITICAL turns that off) — the people who never open the
//! panel get the fix too. A gateway that something else manages (the desktop app, a Docker image, a Home Assistant
//! add-on) offers the update and leaves installing it to that manager: the app installs through its own updater (a
//! hook it sets, urgent releases included), an image or an add-on is updated where it came from.
//!
//! The feed is not signed, the archives are: each carries release.json with its version and platform, under the same
//! signature as the program, and a gateway installs an archive only as that very version, built for it, and newer
//! than its own (`install::check_release`) — a feed in the wrong hands cannot take it back to an older release.
//! Feed and archives come over https only (`feed::secure`).

pub mod feed;
pub mod install;
pub mod trial;
pub mod verify;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use axum::extract::State;
use axum::routing::{get, post};
use axum::Router;
use futures::future::BoxFuture;
use minisign_verify::PublicKey;
use serde::Serialize;

use crate::app::App;
use crate::config::Config;
use crate::core::events::{EventBus, GatewayEvent};
use crate::core::identity::permissions::{STATION_VIEW, SYSTEM_UPDATE};
use crate::placement::station_fs::{self, StationFs};
use crate::platform::http::{ok, ApiError, ApiResult, Caller};
use crate::utils::time::now_iso;

pub use feed::{Asset, Feed};

/// The feed this build checks when UPDATE_FEED_URL is not set: given at build time (the release workflow names the
/// repository's latest.json), none in a local build.
const BUILT_IN_FEED: Option<&str> = option_env!("DUOWAVE_UPDATE_FEED");

/// The release feed this gateway reads: UPDATE_FEED_URL, else the one built in.
pub fn feed_url(config: &Config) -> Option<String> {
    config.updates.feed_url.clone().or(BUILT_IN_FEED.filter(|url| !url.is_empty()).map(str::to_string))
}

/// What an update on the station asks of the rest of the gateway (src/updates/trial.rs): whether the station is quiet
/// now, and its apps switched off for a while and back.
pub trait StationHooks: Send + Sync {
    /// Nobody talks to the station and nothing plays: the gateway may restart for a moment.
    fn quiet(&self) -> bool;
    /// The station's apps switched off for the trial, not as the owner's choice: their memory back. The ids.
    fn pause_apps(&self) -> BoxFuture<'static, Vec<String>>;
    /// They may start again.
    fn resume_apps(&self);
}

/// Installs a version through whoever manages this gateway: the desktop app's updater.
pub trait Installer: Send + Sync {
    fn install(&self, version: String) -> BoxFuture<'static, Result<(), String>>;

    /// The version of what gets installed, when it is not this gateway's own: the app's. The feed is the app's, so
    /// it is compared with the app's version — the gateway inside has its own crate version, and a release that
    /// moves one and not the other would otherwise offer the same app again and again.
    fn version(&self) -> Option<String> {
        None
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Offer {
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    /// Urgent for this version (`feed::is_critical`): the panel says so loudly, and a gateway that installs its own
    /// updates installs it by itself.
    pub critical: bool,
    #[serde(skip)]
    asset: Option<Asset>,
}

/// What the panel shows.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    pub current: String,
    /// This build's key in the feed.
    pub platform: String,
    /// Whether there is a feed to check.
    pub feed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<String>,
    pub available: Option<Offer>,
    pub installing: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// desktop, docker, home-assistant: who installs updates of this gateway.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub managed_by: Option<String>,
    /// Whether «Обновить» can install the offered version here.
    pub can_install: bool,
    /// Whether an urgent release installs here by itself: UPDATE_AUTO_CRITICAL is on and this gateway (or the app
    /// around it) installs its own updates. Off in Docker, Home Assistant, a package: there the panel says what to do.
    pub auto_critical: bool,
    /// What to do instead, when the gateway cannot install it itself.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// On the station: the new version tried from memory now (src/updates/trial.rs).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trial: Option<trial::TrialView>,
}

#[derive(Default)]
struct Progress {
    checked_at: Option<String>,
    available: Option<Offer>,
    installing: bool,
    /// Why the feed did not answer; the next check that reads it clears this.
    error: Option<String>,
    /// The install that failed — its version and why. A check of the feed keeps it (it is not the feed's error);
    /// another version offered, or the next install, clears it.
    failed: Option<(String, String)>,
    /// The last version announced on the event bus: each is announced once.
    announced: Option<String>,
    /// On the station: the new version tried from memory now.
    trial: Option<trial::TrialView>,
}

pub struct UpdatesService {
    current: String,
    platform: String,
    feed_url: Option<String>,
    /// The keys a release's signature may carry (one per line of updates.pub — a rotation adds its key).
    keys: Result<Vec<PublicKey>, String>,
    /// desktop, docker, home-assistant, pc (the computer that installed the station's gateway), yam, yse.
    managed_by: std::sync::RwLock<Option<String>>,
    /// The gateway on the station: its updates come from the computer, from the feed, or from its package (yam).
    station: std::sync::atomic::AtomicBool,
    downloads: PathBuf,
    check_every: Option<Duration>,
    /// UPDATE_AUTO_CRITICAL.
    auto_critical: bool,
    events: EventBus,
    state: Mutex<Progress>,
    installer: OnceLock<Arc<dyn Installer>>,
    /// The memory the system and Alice keep (PLUGINS_RESERVE_MB): a trial from memory leaves it.
    memory_reserve_kb: u64,
    /// The rest of the gateway, for an update on the station: set once it stands.
    hooks: OnceLock<Arc<dyn StationHooks>>,
    /// What this start found of a trial on the station (`set_station`), acted on at `start_on_station`.
    at_start: Mutex<Option<trial::AtStart>>,
}

impl UpdatesService {
    pub fn new(config: &Config, events: EventBus) -> UpdatesService {
        let managed_by = config
            .placement
            .managed_by
            .clone()
            .or_else(|| std::env::var("DUOWAVE_MANAGED").ok())
            .filter(|value| !value.is_empty())
            .or_else(|| {
                if std::path::Path::new("/.dockerenv").exists() {
                    Some("docker".into())
                } else if std::env::var_os("SUPERVISOR_TOKEN").is_some() {
                    Some("home-assistant".into())
                } else {
                    None
                }
            });
        let downloads = match &config.data_dir {
            Some(dir) => dir.join("updates"),
            None => std::env::temp_dir().join("duowave-updates"),
        };
        UpdatesService {
            current: env!("CARGO_PKG_VERSION").to_string(),
            platform: feed::platform_key(),
            feed_url: feed_url(config),
            keys: verify::public_keys(config.updates.public_key.as_deref().unwrap_or(verify::BUILT_IN_KEY)),
            managed_by: std::sync::RwLock::new(managed_by),
            station: std::sync::atomic::AtomicBool::new(false),
            downloads,
            check_every: (config.updates.check_hours > 0)
                .then(|| Duration::from_secs(config.updates.check_hours * 3600)),
            auto_critical: config.updates.auto_critical,
            events,
            state: Mutex::new(Progress::default()),
            installer: OnceLock::new(),
            memory_reserve_kb: config.plugins.reserve_mb * 1024,
            hooks: OnceLock::new(),
            at_start: Mutex::new(None),
        }
    }

    /// The desktop app installs through its own updater.
    /// The version the feed is compared with: the installer's (the app's) when it names one, else this gateway's.
    fn current(&self) -> String {
        self.installer.get().and_then(|installer| installer.version()).unwrap_or_else(|| self.current.clone())
    }

    pub fn set_installer(&self, installer: Arc<dyn Installer>) {
        let _ = self.installer.set(installer);
    }

    /// The gateway on the station: who updates it (the station's mark: pc, yam, yse), and the station's own checks.
    /// What this start is to a trial of a new version is read here, before the apps start: the trial with the apps
    /// switched off does not start them (`holds_apps`).
    pub fn set_station(&self, managed_by: Option<String>) {
        self.station.store(true, std::sync::atomic::Ordering::Relaxed);
        if managed_by.is_some() {
            *self.managed_by.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = managed_by;
        }
        let fs = StationFs::real();
        let found = std::env::current_exe().ok().map(|running| {
            trial::at_start(fs.trial(), &running, &fs.program_on_disk(), &self.current, fs.boot_id().as_deref())
        });
        *self.at_start.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = found;
    }

    pub fn set_station_hooks(&self, hooks: Arc<dyn StationHooks>) {
        let _ = self.hooks.set(hooks);
    }

    /// This process is a trial for which the station's apps were switched off: they do not start until the gateway
    /// restarts from the disk.
    pub fn holds_apps(&self) -> bool {
        matches!(
            self.at_start.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).as_ref(),
            Some(trial::AtStart::Trying(mark)) if mark.apps_paused
        )
    }

    fn managed(&self) -> Option<String> {
        self.managed_by.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    fn on_station(&self) -> bool {
        self.station.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn status(&self) -> UpdateStatus {
        let state = self.state.lock().unwrap();
        let (can_install, hint) = self.how_to_install();
        UpdateStatus {
            current: self.current(),
            platform: self.platform.clone(),
            feed: self.feed_url.is_some(),
            checked_at: state.checked_at.clone(),
            available: state.available.clone(),
            installing: state.installing || state.trial.is_some(),
            error: state
                .failed
                .as_ref()
                .map(|(version, error)| {
                    crate::tr!("Версия {version} не поставилась: {error}", version = version, error = error)
                })
                .or_else(|| state.error.clone()),
            managed_by: self.managed(),
            can_install: can_install && state.available.is_some() && !state.installing && state.trial.is_none(),
            auto_critical: self.auto_critical && can_install,
            hint,
            trial: state.trial.clone(),
        }
    }

    fn how_to_install(&self) -> (bool, Option<String>) {
        if self.installer.get().is_some() {
            return (true, None);
        }
        match self.managed().as_deref() {
            Some("pc") => (
                false,
                Some(
                    crate::tr!("DuoWave на колонке обновляет компьютер, который его поставил: в его пульте «Настройки» → «Обновления и копии» → «Обновить до …»."),
                ),
            ),
            Some("yam") => (
                false,
                Some(
                    crate::tr!("DuoWave поставлен через YAM: обновите его на колонке — yam install duowave/gateway@<версия>."),
                ),
            ),
            Some("yse") => (false, Some(crate::tr!("DuoWave поставлен через yse: обновите его — yse upgrade duowave."))),
            Some("docker") => (false, Some(crate::tr!("Обновите образ: docker compose pull && docker compose up -d"))),
            Some("home-assistant") => (false, Some(crate::tr!("Обновите дополнение в Home Assistant."))),
            Some("desktop") => (false, Some(crate::tr!("Обновите приложение из меню в трее."))),
            Some(other) => (false, Some(crate::tr!("Этим DuoWave управляет {other}: обновите его там.", other = other))),
            None if self.keys.is_err() => (false, Some(crate::tr!("Нет ключа подписи обновлений: установка выключена."))),
            None => (true, None),
        }
    }

    /// Reads the feed and offers what is newer than this version.
    pub async fn check(&self) -> Result<UpdateStatus, ApiError> {
        let Some(url) = self.feed_url.clone() else {
            return Ok(self.status());
        };
        let current = self.current();
        let outcome = ask_feed(&url, &current).await;
        let announce = {
            let mut state = self.state.lock().unwrap();
            state.checked_at = Some(now_iso());
            match outcome {
                Ok(feed) => {
                    state.error = None;
                    state.available = feed::is_newer(&feed.version, &current).then(|| Offer {
                        version: feed.version.trim_start_matches('v').to_string(),
                        notes: feed.notes.clone(),
                        date: feed.pub_date.clone(),
                        critical: feed::is_critical(&feed, &current),
                        asset: feed.platforms.get(&self.platform).cloned(),
                    });
                }
                Err(error) => state.error = Some(error),
            }
            if state
                .failed
                .as_ref()
                .is_some_and(|(version, _)| state.available.as_ref().map(|offer| &offer.version) != Some(version))
            {
                state.failed = None;
            }
            let fresh = state
                .available
                .as_ref()
                .filter(|offer| state.announced.as_deref() != Some(offer.version.as_str()))
                .map(|offer| (offer.version.clone(), offer.notes.clone()));
            if let Some((version, _)) = &fresh {
                state.announced = Some(version.clone());
            }
            fresh
        };
        if let Some((version, notes)) = announce {
            tracing::info!("update: version {version} is offered");
            self.events.emit(GatewayEvent::UpdateAvailable { version, notes });
        }
        Ok(self.status())
    }

    /// Starts installing the offered version; the status says how it goes. Answers at once.
    pub fn install(self: &Arc<Self>) -> Result<UpdateStatus, ApiError> {
        let (can_install, hint) = self.how_to_install();
        if !can_install {
            return Err(ApiError::conflict(hint.unwrap_or_else(|| crate::tr!("Здесь обновление не ставится."))));
        }
        let offer = {
            let mut state = self.state.lock().unwrap();
            if state.installing {
                return Err(ApiError::conflict(crate::tr!("Обновление уже ставится.")));
            }
            let Some(offer) = state.available.clone() else {
                return Err(ApiError::conflict(crate::tr!("Новой версии нет.")));
            };
            state.installing = true;
            state.error = None;
            state.failed = None;
            offer
        };
        let service = self.clone();
        tokio::spawn(async move {
            let result = match service.installer.get().cloned() {
                Some(installer) => installer.install(offer.version.clone()).await,
                None => service.install_here(&offer, true).await,
            };
            let mut state = service.state.lock().unwrap();
            state.installing = false;
            if let Err(error) = result {
                tracing::warn!("update {}: {error}", offer.version);
                state.failed = Some((offer.version, error));
            }
        });
        Ok(self.status())
    }

    /// Downloads, checks and puts the new program in place, then starts it. On the station the way of
    /// `install_on_station`: tried from memory first.
    async fn install_here(self: &Arc<Self>, offer: &Offer, restart: bool) -> Result<(), String> {
        let asset = offer.asset.as_ref().ok_or_else(|| {
            crate::tr!(
                "В выпуске {version} нет сборки для {platform}.",
                version = offer.version,
                platform = self.platform
            )
        })?;
        let keys = self.keys.as_ref().map_err(Clone::clone)?;
        let archive = install::download(&asset.url).await?;
        verify::verify_any(keys, &archive, &asset.signature)?;
        install::check_release(&archive, &self.platform, &offer.version, &self.current())?;
        if self.on_station() && restart {
            let (service, version) = (self.clone(), offer.version.clone());
            return tokio::task::spawn_blocking(move || service.install_on_station(&version, &archive))
                .await
                .map_err(|error| error.to_string())?
                .map(|_| ())
                .map_err(|error| error.message);
        }
        // A copy of the archive beside the data, where there is room for it: not on the station.
        if !self.on_station() {
            std::fs::create_dir_all(&self.downloads)
                .map_err(|error| format!("{}: {error}", self.downloads.display()))?;
            let _ = std::fs::write(self.downloads.join(format!("duowave-{}.tar.gz", offer.version)), &archive);
        }
        let current = std::env::current_exe().map_err(|error| format!("the running program: {error}"))?;
        if self.on_station() {
            install::remove_leftovers(&current);
            let room = self.room_on_station(&archive, &current, &current)?;
            if room.free_kb.unwrap_or(u64::MAX) < room.disk_beside() {
                return Err(room.no_room_beside());
            }
        }
        let previous = install::replace_from_archive(&current, &archive, install::program_name())?;
        if self.on_station() {
            // The service's safety net may bring the one before back if this one does not come up.
            StationFs::real().mark_updated(&offer.version);
        }
        tracing::info!(
            "update {} installed; the one before is {} — starting the new one",
            offer.version,
            previous.display()
        );
        if !restart {
            return Ok(());
        }
        // Give the answer to the panel a moment to leave, then become the new program.
        tokio::time::sleep(Duration::from_millis(500)).await;
        Err(format!("restart: {}", install::restart(&current)))
    }

    /// The room an update has on the station: the new program's size from the archive, the one on the disk, the
    /// disk's free room with a kept `.previous` counted in (it goes first), the memory available.
    fn room_on_station(&self, archive: &[u8], program: &Path, running: &Path) -> Result<trial::Room, String> {
        let kb = |path: &Path| std::fs::metadata(path).map(|meta| meta.len().div_ceil(1024)).unwrap_or(0);
        let kept_kb = kb(&install::previous_of(running));
        let fs = StationFs::real();
        Ok(trial::Room {
            new_kb: install::program_size(archive, "duowave")?.div_ceil(1024),
            old_kb: kb(program),
            free_kb: station_fs::space_of(program.parent().unwrap_or(Path::new("/data")))
                .map(|space| space.free_kb + kept_kb),
            available_kb: station_fs::memory_of(&fs.path("/proc/meminfo"), None).map(|memory| memory.available_kb),
            reserve_kb: self.memory_reserve_kb,
        })
    }

    /// The archives of updates a gateway once kept beside its data: none on the station any more. The bytes freed.
    fn remove_downloads(&self) -> u64 {
        std::fs::read_dir(&self.downloads)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.file_name().to_str().is_some_and(|name| name.ends_with(".tar.gz")))
            .map(|entry| install::remove_counted(&entry.path()))
            .sum()
    }

    /// The computer sends the station's gateway its update (`POST /api/v1/updates/archive`, the pair's key): the archive
    /// itself, its version and signature beside it. The signature is checked with this gateway's own key, the archive's
    /// release.json too (this platform, the version sent, newer than this one: no way back to an older release from
    /// here either), then it goes the station's way (`install_on_station`): tried from memory, or beside the old
    /// program — never whole in memory as an archive and a program at once. No internet on the station needed.
    /// Blocking: the route runs it off the threads that answer requests, so /health keeps answering the station's
    /// watchdog meanwhile.
    pub fn install_upload(
        self: &Arc<Self>,
        version: &str,
        archive: &[u8],
        signature: &str,
    ) -> Result<UpdateStatus, ApiError> {
        let keys = self.keys.as_ref().map_err(|error| ApiError::conflict(error.clone()))?;
        verify::verify_any(keys, archive, signature).map_err(ApiError::bad_request)?;
        install::check_release(archive, &self.platform, version, &self.current()).map_err(ApiError::bad_request)?;
        if self.on_station() {
            return self.install_on_station(version, archive);
        }
        let current = std::env::current_exe().map_err(ApiError::internal)?;
        let version = version.trim_start_matches('v').to_string();
        self.begin_install()?;
        let previous = match install::replace_from_archive(&current, archive, "duowave") {
            Ok(previous) => previous,
            Err(error) => return Err(self.install_failed(&version, error)),
        };
        tracing::info!("update {version} from the computer installed; the one before is {}", previous.display());
        self.become_later(current, version, None);
        Ok(self.status())
    }

    /// An update on the station (docs/plans/station-update-space.md): the room counted from the program itself, then
    /// tried from memory — or put beside the old program as before when memory is short and the disk roomy, or tried
    /// once the station's apps give their memory back; else refused with what to free. Nothing of an update that does
    /// not go on is left: the program on the disk and the apps stay as they were. Blocking: run off the async threads.
    pub fn install_on_station(self: &Arc<Self>, version: &str, archive: &[u8]) -> Result<UpdateStatus, ApiError> {
        let version = version.trim_start_matches('v').to_string();
        let fs = StationFs::real();
        let running = std::env::current_exe().map_err(ApiError::internal)?;
        let program = fs.program_on_disk();
        if self.state.lock().unwrap().trial.is_some() || !install::same_file(&running, &program) {
            return Err(ApiError::conflict(crate::tr!("Идёт проба новой версии: следующее обновление — после неё.")));
        }
        self.begin_install()?;
        match self.station_way(&fs, &version, archive, &running, &program) {
            Ok(()) => Ok(self.status()),
            Err(error) => Err(self.install_failed(&version, error)),
        }
    }

    fn begin_install(&self) -> Result<(), ApiError> {
        let mut state = self.state.lock().unwrap();
        if state.installing {
            return Err(ApiError::conflict(crate::tr!("Обновление уже ставится.")));
        }
        state.installing = true;
        state.failed = None;
        Ok(())
    }

    fn install_failed(&self, version: &str, error: String) -> ApiError {
        let mut state = self.state.lock().unwrap();
        state.installing = false;
        state.failed = Some((version.to_string(), error.clone()));
        ApiError::conflict(error)
    }

    /// The station's way for this update, taken: see `trial::Room::way`.
    fn station_way(
        self: &Arc<Self>,
        fs: &StationFs,
        version: &str,
        archive: &[u8],
        running: &Path,
        program: &Path,
    ) -> Result<(), String> {
        let freed = install::remove_leftovers(program) + self.remove_downloads();
        if freed > 0 {
            tracing::info!("update: {} KB that earlier updates left removed", freed / 1024);
        }
        let room = self.room_on_station(archive, program, running)?;
        let mut paused = false;
        let way = match room.way() {
            trial::Way::PauseApps => {
                let Some(hooks) = self.hooks.get().cloned() else { return Err(room.no_memory()) };
                let stopped = tokio::runtime::Handle::current().block_on(hooks.pause_apps());
                tracing::warn!(
                    "update {version}: memory is short for the trial — the station's apps are off for it: {}",
                    if stopped.is_empty() { "none was running".to_string() } else { stopped.join(", ") }
                );
                paused = true;
                // An app's memory is back once its program has ended.
                std::thread::sleep(Duration::from_secs(2));
                let again = self.room_on_station(archive, program, running)?;
                if !again.memory_fits() {
                    hooks.resume_apps();
                    return Err(again.no_memory());
                }
                trial::Way::Trial
            }
            way => way,
        };
        let outcome = match way {
            trial::Way::Trial => self.begin_trial(fs, version, archive, program, paused, &room),
            trial::Way::Beside => self.beside(fs, version, archive, running),
            _ => Err(room.no_room()),
        };
        if outcome.is_err() && paused {
            if let Some(hooks) = self.hooks.get() {
                hooks.resume_apps();
            }
        }
        outcome
    }

    /// As before the trial: the new program written beside the running one, which stays as `.previous` — the safety
    /// net's way back (station-client/gateway.sh) — until the new one has run ten minutes.
    fn beside(self: &Arc<Self>, fs: &StationFs, version: &str, archive: &[u8], running: &Path) -> Result<(), String> {
        let previous = install::replace_from_archive(running, archive, "duowave")?;
        fs.mark_updated(version);
        tracing::info!("update {version} installed beside the one before ({})", previous.display());
        self.become_later(running.to_path_buf(), version.to_string(), None);
        Ok(())
    }

    /// The new program into memory, asked its version, its mark written: the gateway becomes it in a moment, and the
    /// program on the disk stays the way back (src/updates/trial.rs).
    fn begin_trial(
        self: &Arc<Self>,
        fs: &StationFs,
        version: &str,
        archive: &[u8],
        program: &Path,
        paused: bool,
        room: &trial::Room,
    ) -> Result<(), String> {
        let target = fs.trial_dir().join(format!("duowave-{version}"));
        let (_, sha256) = install::unpack_to(archive, "duowave", &target)
            .map_err(|error| crate::tr!("Новая версия не распаковалась в ОЗУ колонки: {error}", error = error))?;
        if let Err(problem) = install::answers_version(&target, version, trial::ANSWER_WAIT) {
            let _ = std::fs::remove_file(&target);
            return match problem {
                // Programs are not run from memory here: the update goes beside the old program, if the disk allows.
                install::NotAnswered::NotRun(why) if room.free_kb.unwrap_or(u64::MAX) >= room.disk_beside() => {
                    tracing::warn!("update {version}: no trial from memory ({why}) — beside the one before");
                    self.beside(fs, version, archive, &std::env::current_exe().map_err(|error| error.to_string())?)
                }
                problem => Err(crate::tr!("Новая версия не прошла проверку: {why}.", why = problem.message())),
            };
        }
        // The running program is the way back now: the one kept before it, and the mark that named it, go.
        let _ = std::fs::remove_file(install::previous_of(program));
        let _ = std::fs::remove_file(fs.path(station_fs::UPDATED));
        let target = std::fs::canonicalize(&target).unwrap_or(target);
        let mark = trial::Trying {
            version: version.to_string(),
            program: target.clone(),
            sha256,
            started: trial::now_seconds(),
            boot: fs.boot_id(),
            apps_paused: paused,
        };
        if let Err(error) = fs.set_trial(&mark) {
            let _ = std::fs::remove_file(&target);
            return Err(error);
        }
        fs.clear_trial_failed();
        tracing::info!(
            "update {version}: tried from memory for {} minutes; the program on the disk stays the way back",
            trial::TRIAL.as_secs() / 60
        );
        self.become_later(target, version.to_string(), Some(mark));
        Ok(())
    }

    /// The answer leaves first, then the gateway becomes `program` (exec). A failed exec of a trial takes its mark and
    /// its memory back.
    fn become_later(self: &Arc<Self>, program: PathBuf, version: String, trying: Option<trial::Trying>) {
        let service = self.clone();
        tokio::runtime::Handle::current().spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let error = install::restart(&program);
            if let Some(trying) = trying {
                let fs = StationFs::real();
                fs.clear_trial();
                let _ = std::fs::remove_file(&trying.program);
                if trying.apps_paused {
                    if let Some(hooks) = service.hooks.get() {
                        hooks.resume_apps();
                    }
                }
            }
            let mut state = service.state.lock().unwrap();
            state.installing = false;
            state.failed = Some((version, format!("restart: {error}")));
        });
    }

    /// At the start on the station, once the gateway serves: what an update left is removed, and what a trial left
    /// is acted on — this process is the trial (watched until it goes onto the disk), or the trial ended without it
    /// (noted for the panel and the computer, its memory freed).
    pub fn start_on_station(self: &Arc<Self>) {
        if !self.on_station() {
            return;
        }
        let fs = StationFs::real();
        let found = self.at_start.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take();
        if !matches!(found, Some(trial::AtStart::Trying(_))) {
            let freed = install::remove_leftovers(&fs.program_on_disk()) + self.remove_downloads();
            if freed > 0 {
                tracing::info!(
                    "update: {} KB that earlier updates left removed (duowave.failed and the like)",
                    freed / 1024
                );
            }
        }
        match found {
            Some(trial::AtStart::Trying(mark)) => {
                self.state.lock().unwrap().trial = Some(trial::TrialView::of(&mark));
                self.watch_trial(mark);
            }
            Some(trial::AtStart::Ended(mark, why)) => {
                let failed = trial::TrialFailed { version: mark.version.clone(), at: now_iso(), why: why.to_string() };
                tracing::warn!("update {}: {}", mark.version, failed.text());
                let _ = std::fs::remove_file(&mark.program);
                fs.clear_trial();
                fs.set_trial_failed(&failed);
                self.state.lock().unwrap().failed = Some((mark.version.clone(), failed.reason()));
            }
            Some(trial::AtStart::Done(mark)) => {
                let _ = std::fs::remove_file(&mark.program);
                fs.clear_trial();
            }
            Some(trial::AtStart::None) | None => {}
        }
    }

    /// The trial watched: after `TRIAL` the new program goes onto the disk in the old one's place (tried again while it
    /// cannot), then after a quiet minute the gateway restarts from the disk.
    fn watch_trial(self: &Arc<Self>, mark: trial::Trying) {
        let service = self.clone();
        tokio::spawn(async move {
            let wait = mark.until() - trial::now_seconds();
            if wait > 0 {
                tokio::time::sleep(Duration::from_secs(wait as u64)).await;
            }
            loop {
                let program = StationFs::real().program_on_disk();
                let free =
                    station_fs::space_of(program.parent().unwrap_or(Path::new("/data"))).map(|space| space.free_kb);
                let (source, sha256, target) = (mark.program.clone(), mark.sha256.clone(), program.clone());
                let moved = tokio::task::spawn_blocking(move || install::move_to_disk(&source, &target, &sha256, free))
                    .await
                    .map_err(|error| error.to_string())
                    .and_then(|moved| moved);
                match moved {
                    Ok(moved) => {
                        tracing::info!(
                            "update {}: ten minutes from memory went well — on the disk now{}",
                            mark.version,
                            if moved.tight { " (in the old one's place: no room for both)" } else { "" }
                        );
                        break;
                    }
                    Err(error) => {
                        tracing::warn!("update {}: not onto the disk yet: {error}", mark.version);
                        service.trial_phase("stuck", Some(error));
                        tokio::time::sleep(trial::RETRY_MOVE).await;
                    }
                }
            }
            let fs = StationFs::real();
            fs.clear_trial();
            fs.clear_trial_failed();
            // The program runs on from memory; its file there goes, and the memory with it at the restart.
            let _ = std::fs::remove_file(&mark.program);
            service.trial_phase("waiting", None);
            let mut quiet = 0;
            while quiet < trial::QUIET_LOOKS {
                tokio::time::sleep(trial::LOOK_EVERY).await;
                let now = service.hooks.get().is_none_or(|hooks| hooks.quiet());
                quiet = if now { quiet + 1 } else { 0 };
            }
            let program = fs.program_on_disk();
            tracing::info!("update {}: a quiet minute — the gateway starts from the disk", mark.version);
            let error = install::restart(&program);
            tracing::warn!("update {}: the start from the disk failed: {error}", mark.version);
            service.trial_phase("stuck", Some(format!("restart: {error}")));
        });
    }

    fn trial_phase(&self, phase: &'static str, message: Option<String>) {
        if let Some(trial) = self.state.lock().unwrap().trial.as_mut() {
            trial.phase = phase;
            trial.message = message;
        }
    }

    /// From the console (`duowave update`): checks, and installs what is offered without restarting — the service
    /// manager (or the person) starts the new program. Returns what happened, for people.
    pub async fn update_from_console(self: &Arc<Self>, check_only: bool) -> Result<String, String> {
        if self.feed_url.is_none() {
            return Err("no feed: set UPDATE_FEED_URL".into());
        }
        let status = self.check().await.map_err(|error| error.message)?;
        if let Some(error) = status.error {
            return Err(error);
        }
        let Some(offer) = self.state.lock().unwrap().available.clone() else {
            return Ok(format!("duowave {} is the latest", self.current()));
        };
        if check_only {
            return Ok(format!("duowave {} is offered (this is {})", offer.version, self.current()));
        }
        if let Some(hint) = self.how_to_install().1 {
            return Err(hint);
        }
        self.install_here(&offer, false).await?;
        Ok(format!("duowave {} installed; restart the gateway to run it", offer.version))
    }

    /// Checks half a minute after start (the gateway is up by then) and then every UPDATE_CHECK_HOURS, while there is a feed.
    /// An urgent release found there installs at once (`install_if_critical`).
    pub fn spawn_checks(self: &Arc<Self>) {
        if self.feed_url.is_none() {
            return;
        }
        let service = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(30)).await;
            loop {
                let _ = service.check().await;
                service.install_if_critical();
                match service.check_every {
                    Some(every) => tokio::time::sleep(every).await,
                    None => break,
                }
            }
        });
    }

    /// An urgent release offered here installs without waiting for «Обновить» — where this gateway (or the app around
    /// it) installs its own updates and UPDATE_AUTO_CRITICAL is on. One that fails is tried again at the next check.
    /// True when an install started.
    fn install_if_critical(self: &Arc<Self>) -> bool {
        let status = self.status();
        let Some(offer) = status.available.filter(|offer| offer.critical) else {
            return false;
        };
        if !status.auto_critical || !status.can_install {
            return false;
        }
        tracing::warn!(
            "update: {} is urgent for {} — installing it without waiting for a click",
            offer.version,
            status.current
        );
        match self.install() {
            Ok(_) => true,
            Err(error) => {
                tracing::warn!("update {}: {}", offer.version, error.message);
                false
            }
        }
    }
}

/// The feed to offer: the addresses of this version (`feed::urls_for`), best first — the first one that offers
/// something newer (a rotation pins a cohort to the release it can still take), else the first that answers.
async fn ask_feed(url: &str, current: &str) -> Result<Feed, String> {
    let mut answered: Option<Feed> = None;
    let mut error: Option<String> = None;
    for url in feed::urls_for(url, current) {
        match fetch_feed(&url).await {
            Ok(feed) if feed::is_newer(&feed.version, current) => return Ok(feed),
            Ok(feed) => answered = answered.or(Some(feed)),
            Err(why) => error = error.or(Some(why)),
        }
    }
    answered.ok_or_else(|| error.unwrap_or_else(|| crate::tr!("лента обновлений не задана")))
}

async fn fetch_feed(url: &str) -> Result<Feed, String> {
    feed::secure(url).map_err(|error| format!("feed: {error}"))?;
    let response = feed::client()
        .get(url)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|error| format!("feed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("feed: HTTP {}", response.status()));
    }
    response.json::<Feed>().await.map_err(|error| format!("feed: {error}"))
}

/// `/api/v1/updates`: the status (who can see the station), a check now and the install (who may update).
pub fn router() -> Router<Arc<App>> {
    Router::new()
        .route("/api/v1/updates", get(status))
        .route("/api/v1/updates/check", post(check))
        .route("/api/v1/updates/install", post(install_now))
        .route("/api/v1/updates/archive", post(archive).layer(axum::extract::DefaultBodyLimit::max(48 * 1024 * 1024)))
}

/// The update's version beside the archive the computer sends (`POST /api/v1/updates/archive`).
pub const VERSION_HEADER: &str = "x-duowave-version";
/// Its signature: Tauri's form, one line of base64.
pub const SIGNATURE_HEADER: &str = "x-duowave-signature";

/// The computer of the pair sends the station's gateway its update: the archive itself as the body, its version and
/// signature in headers (PEER_API 2). Before, it came as base64 in JSON: the station held it four times over, and the
/// kernel killed the gateway midway (11.11.2026). The work runs on a blocking thread; its answer comes when the new
/// program is in place, just before the gateway becomes it.
async fn archive(
    State(app): State<Arc<App>>,
    caller: Caller,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> ApiResult<UpdateStatus> {
    if !crate::placement::peer::from_peer(&caller, app.placement.peer_key()) {
        return Err(ApiError::forbidden(crate::tr!("Обновление колонке присылает только компьютер пары.")));
    }
    let header = |name: &str| headers.get(name).and_then(|value| value.to_str().ok()).unwrap_or("").trim().to_string();
    let (version, signature) = (header(VERSION_HEADER), header(SIGNATURE_HEADER));
    let updates = app.updates.clone();
    let status = tokio::task::spawn_blocking(move || updates.install_upload(&version, &body, &signature))
        .await
        .map_err(ApiError::internal)??;
    Ok(crate::platform::http::created(status))
}

async fn status(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<UpdateStatus> {
    caller.permitted(STATION_VIEW)?;
    Ok(ok(app.updates.status()))
}

async fn check(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<UpdateStatus> {
    caller.permitted(SYSTEM_UPDATE)?;
    Ok(ok(app.updates.check().await?))
}

async fn install_now(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<UpdateStatus> {
    caller.permitted(SYSTEM_UPDATE)?;
    Ok(ok(app.updates.install()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Env;

    fn service(feed: &str) -> Arc<UpdatesService> {
        let config = Config::for_tests(&Env::from_pairs([("UPDATE_FEED_URL", feed)]));
        Arc::new(UpdatesService::new(&config, EventBus::new()))
    }

    async fn feed_server(body: String) -> String {
        let router = Router::new().route("/latest.json", get(move || async move { body }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        format!("http://{address}/latest.json")
    }

    #[tokio::test]
    async fn offers_a_newer_version_once_and_nothing_older() {
        let platform = feed::platform_key();
        let url = feed_server(format!(
            r#"{{"version":"v99.0.0","notes":"Новое","pub_date":"2026-11-06T00:00:00Z","platforms":{{"{platform}":{{"signature":"x","url":"http://127.0.0.1:9/v.tar.gz"}}}}}}"#
        ))
        .await;
        let updates = service(&url);
        let mut events = updates.events.subscribe();
        let status = updates.check().await.unwrap();
        let offer = status.available.clone().expect("99.0.0 is newer");
        assert_eq!(offer.version, "99.0.0");
        assert_eq!(offer.notes.as_deref(), Some("Новое"));
        assert!(status.checked_at.is_some());
        updates.check().await.unwrap();
        let first = events.recv().await.unwrap();
        assert_eq!(first.name(), "update.available");
        assert!(events.try_recv().is_err(), "announced once");

        let old = service(&feed_server(r#"{"version":"0.0.1","platforms":{}}"#.into()).await);
        assert_eq!(old.check().await.unwrap().available, None);
    }

    #[tokio::test]
    async fn the_versions_own_feed_comes_first_the_plain_one_answers_without_it() {
        let platform = feed::platform_key();
        let feed = |version: &str| {
            format!(
                r#"{{"version":"{version}","platforms":{{"{platform}":{{"signature":"x","url":"http://127.0.0.1:9/v.tar.gz"}}}}}}"#
            )
        };
        let plain = feed("v99.0.0");
        let own = feed("v98.0.0");
        let own_path = format!("/{}/latest.json", env!("CARGO_PKG_VERSION"));
        let router = Router::new()
            .route("/latest.json", get(move || async move { plain.clone() }))
            .route(&own_path, get(move || async move { own.clone() }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

        // The version's own feed (98) shadows the plain one (99). Without it the plain feed answers: the servers of
        // the other tests have /latest.json only, and every check there runs through that fall.
        let updates = service(&format!("http://{address}/latest.json"));
        let offered = updates.check().await.unwrap().available.expect("offered");
        assert_eq!(offered.version, "98.0.0");
    }

    #[tokio::test]
    async fn a_feed_that_fails_is_an_error_in_the_status_not_a_failure() {
        let updates = service("http://127.0.0.1:9/latest.json");
        let status = updates.check().await.unwrap();
        assert!(status.error.is_some());
        assert_eq!(status.available, None);
        assert!(!status.can_install);
        assert!(updates.install().is_err(), "nothing to install");
    }

    #[tokio::test]
    async fn a_failed_install_stays_in_the_status_until_another_version_is_offered() {
        // Live 06.11: the install failed on the signature, and the check a moment later wiped the reason — the
        // panel never said why. The feed's body changes between checks here.
        let platform = feed::platform_key();
        let release = move |version: &str| {
            format!(
                r#"{{"version":"{version}","platforms":{{"{platform}":{{"signature":"x","url":"http://127.0.0.1:9/v.tar.gz"}}}}}}"#
            )
        };
        let body = Arc::new(Mutex::new(release("v99.0.0")));
        let served = body.clone();
        let router = Router::new().route("/latest.json", get(move || async move { served.lock().unwrap().clone() }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let updates = service(&format!("http://{address}/latest.json"));
        struct Broken;
        impl Installer for Broken {
            fn install(&self, _version: String) -> BoxFuture<'static, Result<(), String>> {
                Box::pin(async { Err("подпись не сошлась".into()) })
            }
        }
        updates.set_installer(Arc::new(Broken));
        updates.check().await.unwrap();
        assert!(updates.install().unwrap().installing);
        for _ in 0..100 {
            if !updates.status().installing {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let failed = updates.status();
        assert!(!failed.installing);
        assert_eq!(failed.error.as_deref(), Some("Версия 99.0.0 не поставилась: подпись не сошлась"));
        assert!(failed.can_install, "may try again");

        let checked = updates.check().await.unwrap();
        assert_eq!(checked.error, failed.error, "a check of the same release keeps why it failed");

        *body.lock().unwrap() = release("v99.0.1");
        let newer = updates.check().await.unwrap();
        assert_eq!(newer.available.unwrap().version, "99.0.1");
        assert_eq!(newer.error, None, "a new release is a new try");
    }

    #[tokio::test]
    async fn the_apps_gateway_compares_the_feed_with_the_apps_version() {
        // Live 06.11: the app updated itself to 0.2.0, and the gateway inside — crate 0.1.0 — kept offering 0.2.0.
        let platform = feed::platform_key();
        let url = feed_server(format!(
            r#"{{"version":"0.2.0","platforms":{{"{platform}":{{"signature":"x","url":"http://127.0.0.1:9/v.tar.gz"}}}}}}"#
        ))
        .await;
        let updates = service(&url);
        struct App(&'static str);
        impl Installer for App {
            fn install(&self, _version: String) -> BoxFuture<'static, Result<(), String>> {
                Box::pin(async { Ok(()) })
            }
            fn version(&self) -> Option<String> {
                Some(self.0.into())
            }
        }
        updates.set_installer(Arc::new(App("0.2.0")));
        let status = updates.check().await.unwrap();
        assert_eq!(status.current, "0.2.0", "the app's version, not the crate's");
        assert_eq!(status.available, None, "the app is already 0.2.0");

        let older = service(&url);
        older.set_installer(Arc::new(App("0.1.0")));
        assert_eq!(older.check().await.unwrap().available.map(|offer| offer.version).as_deref(), Some("0.2.0"));
    }

    #[tokio::test]
    async fn an_urgent_release_installs_by_itself_where_the_gateway_installs_its_own() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let platform = feed::platform_key();
        let release = |floor: &str| {
            format!(
                r#"{{"version":"99.0.0",{floor}"platforms":{{"{platform}":{{"signature":"x","url":"http://127.0.0.1:9/v.tar.gz"}}}}}}"#
            )
        };
        let url = feed_server(release(r#""critical_below":"99.0.0","#)).await;
        struct Counting(Arc<AtomicUsize>);
        impl Installer for Counting {
            fn install(&self, _version: String) -> BoxFuture<'static, Result<(), String>> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { Ok(()) })
            }
        }
        let installs = Arc::new(AtomicUsize::new(0));

        // The app installs its own updates: the urgent one goes at once, without a click.
        let app = service(&url);
        app.set_installer(Arc::new(Counting(installs.clone())));
        let status = app.check().await.unwrap();
        assert!(status.available.as_ref().unwrap().critical);
        assert!(status.auto_critical);
        assert!(app.install_if_critical());
        for _ in 0..100 {
            if installs.load(Ordering::SeqCst) == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(installs.load(Ordering::SeqCst), 1);

        // UPDATE_AUTO_CRITICAL=false: offered as urgent, installed only by a click.
        let config =
            Config::for_tests(&Env::from_pairs([("UPDATE_FEED_URL", url.as_str()), ("UPDATE_AUTO_CRITICAL", "false")]));
        let off = Arc::new(UpdatesService::new(&config, EventBus::new()));
        off.set_installer(Arc::new(Counting(installs.clone())));
        let status = off.check().await.unwrap();
        assert!(status.available.unwrap().critical);
        assert!(!status.auto_critical);
        assert!(!off.install_if_critical());

        // In Docker the image is updated where it came from: urgent, but only said, never installed from here.
        let docker = service(&url);
        *docker.managed_by.write().unwrap() = Some("docker".into());
        let status = docker.check().await.unwrap();
        assert!(status.available.unwrap().critical);
        assert!(!status.auto_critical);
        assert!(!docker.install_if_critical());

        // An ordinary release waits for a click, as before.
        let ordinary = service(&feed_server(release("")).await);
        ordinary.set_installer(Arc::new(Counting(installs.clone())));
        assert!(!ordinary.check().await.unwrap().available.unwrap().critical);
        assert!(!ordinary.install_if_critical());
        assert_eq!(installs.load(Ordering::SeqCst), 1, "only the urgent one installed by itself");
    }

    #[tokio::test]
    async fn a_feed_over_plain_http_elsewhere_is_refused() {
        let status = service("http://updates.example.com/latest.json").check().await.unwrap();
        assert!(status.error.unwrap().contains("https"), "not even asked");
        assert_eq!(status.available, None);
    }

    #[test]
    fn a_managed_gateway_leaves_installing_to_its_manager() {
        let updates = UpdatesService::new(&Config::for_tests(&Env::default()), EventBus::new());
        *updates.managed_by.write().unwrap() = Some("docker".into());
        let (can, hint) = updates.how_to_install();
        assert!(!can);
        assert!(hint.unwrap().contains("docker compose pull"));
        struct Desktop;
        impl Installer for Desktop {
            fn install(&self, _version: String) -> BoxFuture<'static, Result<(), String>> {
                Box::pin(async { Ok(()) })
            }
        }
        updates.set_installer(Arc::new(Desktop));
        assert!(updates.how_to_install().0, "the app's updater installs");
    }
}
