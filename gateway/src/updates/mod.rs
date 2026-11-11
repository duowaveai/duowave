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
pub mod verify;

use std::path::PathBuf;
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
}

/// The station needs this much free in /data for an update: the archive, the program beside the running one.
pub const STATION_FREE_KB: u64 = 30 * 1024;

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
    pub fn set_station(&self, managed_by: Option<String>) {
        self.station.store(true, std::sync::atomic::Ordering::Relaxed);
        if managed_by.is_some() {
            *self.managed_by.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = managed_by;
        }
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
            installing: state.installing,
            error: state
                .failed
                .as_ref()
                .map(|(version, error)| format!("Версия {version} не поставилась: {error}"))
                .or_else(|| state.error.clone()),
            managed_by: self.managed(),
            can_install: can_install && state.available.is_some() && !state.installing,
            auto_critical: self.auto_critical && can_install,
            hint,
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
                    "Шлюз на колонке обновляет компьютер, который его поставил: «Настройки» → «Компьютер-помощник» → \
                     «Обновить до …»."
                        .into(),
                ),
            ),
            Some("yam") => (
                false,
                Some(
                    "Шлюз пришёл пакетом YAM: обновите пакет на колонке — yam install файла новой версии из выпуска."
                        .into(),
                ),
            ),
            Some("yse") => (false, Some("Шлюз пришёл пакетом yse: обновите пакет — yse upgrade duowave.".into())),
            Some("docker") => (false, Some("Обновите образ: docker compose pull && docker compose up -d".into())),
            Some("home-assistant") => (false, Some("Обновите дополнение в Home Assistant.".into())),
            Some("desktop") => (false, Some("Обновите приложение из меню в трее.".into())),
            Some(other) => (false, Some(format!("Шлюзом управляет {other}: обновите его там."))),
            None if self.keys.is_err() => (false, Some("Нет ключа подписи обновлений: установка выключена.".into())),
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
            return Err(ApiError::conflict(hint.unwrap_or_else(|| "Здесь обновление не ставится.".into())));
        }
        let offer = {
            let mut state = self.state.lock().unwrap();
            if state.installing {
                return Err(ApiError::conflict("Обновление уже ставится."));
            }
            let Some(offer) = state.available.clone() else {
                return Err(ApiError::conflict("Новой версии нет."));
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

    /// Downloads, checks and puts the new program in place, then starts it.
    async fn install_here(&self, offer: &Offer, restart: bool) -> Result<(), String> {
        let asset = offer
            .asset
            .as_ref()
            .ok_or_else(|| format!("в выпуске {} нет сборки для {}", offer.version, self.platform))?;
        let keys = self.keys.as_ref().map_err(Clone::clone)?;
        let archive = install::download(&asset.url).await?;
        verify::verify_any(keys, &archive, &asset.signature)?;
        install::check_release(&archive, &self.platform, &offer.version, &self.current())?;
        std::fs::create_dir_all(&self.downloads).map_err(|error| format!("{}: {error}", self.downloads.display()))?;
        let _ = std::fs::write(self.downloads.join(format!("duowave-{}.tar.gz", offer.version)), &archive);
        let current = std::env::current_exe().map_err(|error| format!("the running program: {error}"))?;
        if self.on_station() {
            self.station_room(&current)?;
        }
        let previous = install::replace_from_archive(&current, &archive, install::program_name())?;
        if self.on_station() {
            // The service's safety net may bring the one before back if this one does not come up.
            crate::placement::station_fs::StationFs::real().mark_updated(&offer.version);
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

    /// The station's /data has room for the new program beside the running one.
    fn station_room(&self, current: &std::path::Path) -> Result<(), String> {
        let folder = current.parent().unwrap_or(std::path::Path::new("/data"));
        match crate::placement::station_fs::space_of(folder) {
            Some(space) if space.free_kb < STATION_FREE_KB => Err(format!(
                "на колонке свободно {} МБ, а обновлению нужно {} МБ",
                space.free_kb / 1024,
                STATION_FREE_KB / 1024
            )),
            _ => Ok(()),
        }
    }

    /// The computer sends the station's gateway its update (`POST /api/v1/updates/archive`, the pair's key): the archive
    /// itself, its version and signature beside it. The signature is checked with this gateway's own key, the archive's
    /// release.json too (this platform, the version sent, newer than this one: no way back to an older release from
    /// here either), room checked, the program streamed into place beside the one before — never whole in memory —
    /// then the gateway becomes the new program. No internet on the station needed. Blocking: the route runs it off
    /// the threads that answer requests, so /health keeps answering the station's watchdog meanwhile.
    pub fn install_upload(
        self: &Arc<Self>,
        version: &str,
        archive: &[u8],
        signature: &str,
    ) -> Result<UpdateStatus, ApiError> {
        let keys = self.keys.as_ref().map_err(|error| ApiError::conflict(error.clone()))?;
        verify::verify_any(keys, archive, signature).map_err(ApiError::bad_request)?;
        install::check_release(archive, &self.platform, version, &self.current()).map_err(ApiError::bad_request)?;
        let current = std::env::current_exe().map_err(ApiError::internal)?;
        if self.on_station() {
            self.station_room(&current).map_err(ApiError::conflict)?;
        }
        let version = version.trim_start_matches('v').to_string();
        {
            let mut state = self.state.lock().unwrap();
            if state.installing {
                return Err(ApiError::conflict("Обновление уже ставится."));
            }
            state.installing = true;
            state.failed = None;
        }
        let previous = match install::replace_from_archive(&current, archive, "duowave") {
            Ok(previous) => previous,
            Err(error) => {
                let mut state = self.state.lock().unwrap();
                state.installing = false;
                state.failed = Some((version, error.clone()));
                return Err(ApiError::conflict(error));
            }
        };
        if self.on_station() {
            crate::placement::station_fs::StationFs::real().mark_updated(&version);
        }
        tracing::info!("update {version} from the computer installed; the one before is {}", previous.display());
        let service = self.clone();
        tokio::spawn(async move {
            // The answer to the computer leaves first, then the gateway becomes the new program.
            tokio::time::sleep(Duration::from_millis(500)).await;
            let error = install::restart(&current);
            let mut state = service.state.lock().unwrap();
            state.installing = false;
            state.failed = Some((version, format!("restart: {error}")));
        });
        Ok(self.status())
    }

    /// From the console (`duowave update`): checks, and installs what is offered without restarting — the service
    /// manager (or the person) starts the new program. Returns what happened, for people.
    pub async fn update_from_console(&self, check_only: bool) -> Result<String, String> {
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
    answered.ok_or_else(|| error.unwrap_or_else(|| "лента обновлений не задана".to_string()))
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
        return Err(ApiError::forbidden("Обновление колонке присылает только компьютер пары."));
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
