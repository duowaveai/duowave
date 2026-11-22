//! The built-in helper (docs/plans/home-and-helpers.md, block 1, «Встроенный помощник»): the heavy part of the
//! computer's own program behind the helpers' contract (contract.rs) — its hearing (the recognition engine of this
//! program, or its recognition server at home), its voice (the own voice's server, the voice lab's voices too), its
//! models (the model servers of «Мозг» on this computer: Ollama, LM Studio), its Telegram bot, its apps with their
//! history, and its copy of the catalog. Inside it is still one program; splitting it in two is for later.
//!
//! Made on every computer's build: `/home/v1` (home_server/serve.rs) answers through it, one answer for every gateway it
//! helps. A computer that is not in the station role also takes its own parts through it (`Helpers::set_own`): first,
//! before the helpers of its list, and without the network — its hearing, voice and model go the way a station's go. A
//! computer in the station role takes nothing from itself: it leans on its helpers, as a speaker does.
//!
//! Its liveness is its model servers': each of «Мозг»'s rows on this computer is looked at on the helpers' tick (a TCP
//! connection to its port, never a request and never its own `/home/v1`), so a closed Ollama gives way to «Запасная»
//! at once. Like any helper it has no «я»: the home's is the gateway's.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, RwLock, Weak};
use std::time::Duration;

use futures::future::BoxFuture;
use serde_json::json;

use super::contract::{Ability, AbilityState, Contract, HelperIdentity};
use super::{Local, OwnModel, OwnParts, Reach, SELF};
use crate::app::App;
use crate::config::Config;
use crate::core::events::{EventBus, GatewayEvent};
use crate::core::identity::home::Identity;
use crate::operations::networks::is_local_url;
use crate::secrets::SecretsService;
use crate::settings::SettingsService;
use crate::speech::asr::local::recognizer::LocalRecognizer;
use crate::speech::asr::local::supervisor::LocalAsr;
use crate::speech::asr::speech_recognizer::{create_recognizer, SpeechRecognizer};
use crate::speech::tts::own_speech::OwnSpeech;
use crate::speech::VoiceService;
use crate::stations::connect::station_connect::{host_interfaces, in_container};
use crate::storage::Db;

/// How long a look at a model server waits for its port to answer.
const LOOK: Duration = Duration::from_millis(500);

/// What hears for the gateways this computer helps: its own engine (src/speech/asr/local), or its recognition server
/// at home — a cloud's is never handed on.
#[derive(Clone)]
pub enum Hearing {
    Local(Arc<LocalAsr>),
    Server(String, Option<String>),
}

/// What the built-in helper is made of, from `App::build`.
pub struct BuiltinParts {
    pub config: Arc<Config>,
    pub db: Arc<Db>,
    pub settings: Arc<SettingsService>,
    pub secrets: Arc<SecretsService>,
    pub events: EventBus,
    pub identity: Arc<Identity>,
    pub local_asr: Option<Arc<LocalAsr>>,
    pub voice: Arc<VoiceService>,
    /// The port this gateway really listens on (`Helpers::local`): an entry at it is this computer itself.
    pub local: Arc<Local>,
}

/// The computer's heavy part as a helper.
pub struct BuiltinHelper {
    config: Arc<Config>,
    db: Arc<Db>,
    settings: Arc<SettingsService>,
    secrets: Arc<SecretsService>,
    events: EventBus,
    identity: Arc<Identity>,
    local: Arc<Local>,
    hearing: Option<Hearing>,
    /// What hears here: the engine, or the same transcriber the gateway always had for its server.
    recognizer: Option<Arc<dyn SpeechRecognizer>>,
    /// The own voice: the very engine the voice service speaks with.
    speech: Option<Arc<OwnSpeech>>,
    /// The voice service, for the voice's name (it holds the helpers, which hold this: no cycle).
    voice: Weak<VoiceService>,
    /// The parts made after it (`attach`): the bot, the apps, their history, the catalog.
    app: OnceLock<Weak<App>>,
    /// This computer's addresses in the home networks: an entry or a model server there is this computer.
    addresses: RwLock<Vec<Ipv4Addr>>,
    /// The addresses are a test's, not looked up again.
    fixed: AtomicBool,
    /// Each model server of this computer as last looked at — by its «Мозг» row and address — and whether it answered.
    looks: Mutex<BTreeMap<(String, String), bool>>,
    /// Its recognition server and its voice server at home as last looked at, by address, and whether each answered
    /// (its own engine is no server, a cloud's voice the regulator's): one seen closed is not ready for the gateways it
    /// helps, and «Где сейчас» says why here.
    parts: Mutex<BTreeMap<String, bool>>,
    /// One look at a time: a look begun before a server closed never ends after one that saw it closed.
    looking: tokio::sync::Mutex<()>,
    /// «Мозг» has a model this computer hands on (`model_route`), as last read: the choice of road reads no table.
    routed: AtomicBool,
    /// The contract as last told: a change of it is `helpers.changed {id: "self", change: "contract"}`.
    told: Mutex<Option<Contract>>,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A row that is a helper's model, never this computer's own: the hidden preset `helper`, or a helper's model route. A
/// computer does not pass a station's questions on to another helper, nor to a link's bridge.
fn helpers_row(preset: &str, url: &str) -> bool {
    preset == crate::dialog::brain::presets::HELPER_PRESET
        || url::Url::parse(url.trim()).is_ok_and(|url| url.path().trim_end_matches('/').ends_with("/home/v1/llm"))
}

/// Whether a server's port takes a connection now — at any of its host's addresses, tried at once. One after another
/// it was wrong on Windows: «localhost» is ::1 first there, a refused ::1 takes Windows about two seconds to say so,
/// and LOOK ran out before 127.0.0.1 was tried — LM Studio at its own http://localhost:1234 looked down (CI 0.18.0,
/// 22.11).
async fn answers(base_url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(base_url) else { return false };
    let (Some(host), Some(port)) = (parsed.host_str(), parsed.port_or_known_default()) else { return false };
    let host = host.trim_matches(['[', ']']).to_string();
    let look = async {
        let addresses: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host.as_str(), port)).await.ok()?.collect();
        let tries = addresses.into_iter().map(|address| Box::pin(tokio::net::TcpStream::connect(address)));
        futures::future::select_ok(tries).await.ok()
    };
    matches!(tokio::time::timeout(LOOK, look).await, Ok(Some(_)))
}

impl BuiltinHelper {
    /// The parts it has from the start: its hearing exactly as `/home/v1` always chose it — the engine of this program
    /// first, else the recognition server while recognition is on and the server is at home —, its voice, its models.
    pub fn new(parts: BuiltinParts) -> Arc<BuiltinHelper> {
        let asr = &parts.config.speech.asr;
        let hearing = match &parts.local_asr {
            Some(local) => Some(Hearing::Local(local.clone())),
            None => (asr.enabled && asr.local.is_none() && is_local_url(Some(&asr.base_url)))
                .then(|| Hearing::Server(asr.base_url.trim_end_matches('/').to_string(), asr.api_key.clone())),
        };
        let recognizer: Option<Arc<dyn SpeechRecognizer>> = match &hearing {
            Some(Hearing::Local(local)) => Some(Arc::new(LocalRecognizer::new(local.clone()))),
            Some(Hearing::Server(..)) => create_recognizer(asr),
            None => None,
        };
        let helper = Arc::new(BuiltinHelper {
            hearing,
            recognizer,
            speech: parts.voice.own_speech().cloned(),
            voice: Arc::downgrade(&parts.voice),
            config: parts.config,
            db: parts.db,
            settings: parts.settings,
            secrets: parts.secrets,
            events: parts.events,
            identity: parts.identity,
            local: parts.local,
            app: OnceLock::new(),
            addresses: RwLock::new(Vec::new()),
            fixed: AtomicBool::new(false),
            looks: Mutex::new(BTreeMap::new()),
            parts: Mutex::new(BTreeMap::new()),
            looking: tokio::sync::Mutex::new(()),
            routed: AtomicBool::new(false),
            told: Mutex::new(None),
        });
        helper.routed.store(helper.route().is_some(), Ordering::Relaxed);
        helper
    }

    /// The parts made after it: the Telegram bot, the plugins, their history, the catalog — and this computer's home
    /// addresses.
    pub fn attach(&self, app: Weak<App>) {
        let _ = self.app.set(app);
        self.find_addresses();
    }

    fn app(&self) -> Option<Arc<App>> {
        self.app.get().and_then(Weak::upgrade)
    }

    /// What it declares is followed: a change of the voice, of «Мозг», of a connector (the bot), of a server of this
    /// computer is told as `helpers.changed` — the panels and «Мозг» hear it. The contract as it is now is the one
    /// told; the listener holds the helper weakly.
    pub fn start(self: &Arc<Self>) {
        {
            let contract = self.contract();
            locked(&self.told).get_or_insert(contract);
        }
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let me = Arc::downgrade(self);
        self.events.on("builtin-helper", move |event| {
            // The regulator too: its voice at a cloud's address is ready for others only while the clouds are let out.
            // The panel's own move is told once the helpers took it (`away`); another writer's, by its setting.
            let follows = match &*event {
                GatewayEvent::VoiceChanged { .. }
                | GatewayEvent::BrainChanged { .. }
                | GatewayEvent::ConnectorChanged { .. }
                | GatewayEvent::ModelServerRestarted { .. }
                | GatewayEvent::ModelInstalled { .. } => true,
                GatewayEvent::HelpersChanged { id: None, change, .. } => change == "away",
                GatewayEvent::SettingsChanged { key, .. } => key == crate::settings::definitions::AWAY.key,
                _ => false,
            };
            if let Some(helper) = me.upgrade().filter(|_| follows) {
                helper.routed.store(helper.route().is_some(), Ordering::Relaxed);
                helper.tell(false);
            }
        });
    }

    /// This computer as a helper names itself: its gateway's id and name.
    pub fn identity(&self) -> HelperIdentity {
        self.identity.as_helper()
    }

    pub fn hearing(&self) -> Option<&Hearing> {
        self.hearing.as_ref()
    }

    /// Its hearing is ready for others: a server unless its last look found it closed, its own engine once the model
    /// is in.
    pub fn hears(&self) -> bool {
        match &self.hearing {
            Some(Hearing::Local(local)) => local.ready(),
            Some(Hearing::Server(url, _)) => !self.closed(url),
            None => false,
        }
    }

    /// The servers of its hearing and voice that its look connects to: the recognition server, and the voice's while
    /// it is at home.
    fn part_servers(&self) -> Vec<String> {
        let hearing = match &self.hearing {
            Some(Hearing::Server(url, _)) => Some(url.clone()),
            _ => None,
        };
        let voice = self.speech.as_ref().map(|speech| speech.base_url()).filter(|url| is_local_url(Some(url.as_str())));
        hearing.into_iter().chain(voice).collect()
    }

    /// A server of its parts that its last look found closed.
    fn closed(&self, url: &str) -> bool {
        locked(&self.parts).get(url) == Some(&false)
    }

    /// Its voice server at home was found closed.
    fn voice_closed(&self) -> bool {
        self.speech.as_ref().is_some_and(|speech| {
            let url = speech.base_url();
            is_local_url(Some(&url)) && self.closed(&url)
        })
    }

    /// Its voice would take another gateway's text out of the house against the regulator: its server — the env's, or
    /// a voice lab's own — is at a cloud's address, and «Что может уходить из дома» keeps the clouds in now. Declared
    /// all the same (the old `tts` stays), not ready, and `/home/v1/tts` refuses.
    pub fn voice_kept_in(&self) -> bool {
        let Some(speech) = &self.speech else { return false };
        !is_local_url(Some(&speech.base_url())) && !self.app().is_some_and(|app| app.helpers.lets(Reach::Foreign))
    }

    /// The voice it speaks with now, as the voice server names it: «model/voice».
    pub fn voice_choice(&self) -> Option<String> {
        let pair = self.voice.upgrade()?.own_choice()?;
        Some(format!("{}/{}", pair.model, pair.voice))
    }

    /// «Мозг»'s rows of OpenAI kind with an address, in their order: (id, address, preset).
    fn rows(&self) -> Vec<(String, String, String)> {
        self.db
            .with(|connection| {
                connection
                    .prepare(
                        "SELECT id, base_url, preset FROM llm_providers WHERE kind = 'openai' AND base_url IS NOT NULL
                         ORDER BY created_at, rowid",
                    )?
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The model this computer hands on: the first OpenAI-compatible row of «Мозг» at home that is no helper's — its
    /// row, its address.
    fn route(&self) -> Option<(String, String)> {
        self.rows()
            .into_iter()
            .find(|(_, url, preset)| !helpers_row(preset, url) && is_local_url(Some(url)))
            .map(|(id, url, _)| (id, url.trim_end_matches('/').to_string()))
    }

    /// This computer's own model for the gateways it helps (`/home/v1/llm`): the first provider of «Мозг» at home and
    /// its key — never a helper's model.
    pub fn model_route(&self) -> Option<(String, Option<String>)> {
        let (id, url) = self.route()?;
        Some((url, self.secrets.get(&format!("llm:{id}:key"))))
    }

    /// Whether an address is this computer: a loopback name or address, or one of its home addresses.
    fn here(&self, url: &url::Url) -> bool {
        let addresses = self.addresses.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        match url.host() {
            Some(url::Host::Ipv4(ip)) => ip.is_loopback() || addresses.contains(&ip),
            Some(url::Host::Ipv6(ip)) => {
                ip.is_loopback() || ip.to_ipv4_mapped().is_some_and(|ip| ip.is_loopback() || addresses.contains(&ip))
            }
            Some(url::Host::Domain(name)) => {
                let name = name.to_ascii_lowercase();
                name == "localhost" || name.ends_with(".localhost")
            }
            None => false,
        }
    }

    /// This computer's addresses in the home networks, read again (none in a container: its addresses are the
    /// bridge's). A test's stay.
    fn find_addresses(&self) {
        if self.fixed.load(Ordering::Relaxed) {
            return;
        }
        let found: Vec<Ipv4Addr> = if in_container() {
            Vec::new()
        } else {
            host_interfaces()
                .into_iter()
                .filter(|iface| iface.ipv4 && !iface.internal)
                .filter_map(|iface| iface.address.parse::<Ipv4Addr>().ok())
                .filter(Ipv4Addr::is_private)
                .collect()
        };
        *self.addresses.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = found;
    }

    /// For tests: this computer's home addresses are these.
    #[cfg(test)]
    pub(crate) fn set_addresses(&self, addresses: Vec<Ipv4Addr>) {
        self.fixed.store(true, Ordering::Relaxed);
        *self.addresses.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = addresses;
    }

    /// The model servers of this computer among «Мозг»'s rows: (row, address).
    fn servers(&self) -> Vec<(String, String)> {
        self.rows()
            .into_iter()
            .filter(|(_, url, preset)| {
                !helpers_row(preset, url) && url::Url::parse(url).is_ok_and(|url| self.here(&url))
            })
            .map(|(id, url, _)| (id, url.trim_end_matches('/').to_string()))
            .collect()
    }

    /// What it declares now, and the panels and «Мозг» told when it changed — or when a server of it came or went
    /// (`looked`), which «Мозг» needs though the contract stays.
    fn tell(&self, looked: bool) {
        let contract = self.contract();
        let changed = {
            let mut told = locked(&self.told);
            let changed = told.as_ref() != Some(&contract);
            *told = Some(contract);
            changed
        };
        if changed || looked {
            self.events.emit(GatewayEvent::HelpersChanged {
                id: Some(SELF.to_string()),
                change: "contract".into(),
                actor: None,
            });
        }
    }
}

/// What this computer gives its home of its devices (docs/plans/turbo-devices.md, contract 3): the kinds it shares with
/// what is plugged in now, and its hardware addresses for a wake over the home network. Whether the home reaches it from
/// home is said per caller (`home_server::serve::status_of`).
fn devices_state(app: &App) -> AbilityState {
    static MACS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    let macs = MACS.get_or_init(crate::helpers::wake::hardware_addresses);
    let mut state = AbilityState::new(Ability::Devices, true);
    state.details.insert("kinds".into(), json!(app.house.given()));
    if !macs.is_empty() {
        state.details.insert("wake".into(), json!({ "macs": macs }));
    }
    state
}

impl OwnParts for BuiltinHelper {
    /// What this computer gives as a helper (`/home/v1/status`): the table of the plan's stage 1, the model, the
    /// recognition server and the voice server ready only while their look does not find them closed, the voice only
    /// while the regulator does not keep it in (`voice_kept_in`). Before `attach` the late parts — messages, apps,
    /// history, catalog — are left out.
    fn contract(&self) -> Contract {
        let mut abilities = Vec::new();
        // The station may lean on this computer's hearing once its own engine has its model in; a server, while it
        // takes a connection.
        match &self.hearing {
            Some(Hearing::Local(local)) => {
                let model = local.model();
                let mut state = AbilityState::new(Ability::Hearing, self.hears());
                if let Some(known) = crate::speech::asr::local::catalog::find(&model) {
                    state.details.insert("languages".into(), json!(known.languages));
                }
                state.details.insert("model".into(), json!(model));
                abilities.push(state);
            }
            Some(Hearing::Server(..)) => {
                let mut state = AbilityState::new(Ability::Hearing, self.hears());
                let asked = self.config.speech.asr.language.as_deref();
                if let Some(language) =
                    crate::speech::asr::wanted_language(asked, crate::locale::household(&self.settings))
                {
                    state.details.insert("languages".into(), json!([language]));
                }
                abilities.push(state);
            }
            None => {}
        }
        if self.speech.is_some() {
            let mut state = AbilityState::new(Ability::Voice, !self.voice_kept_in() && !self.voice_closed());
            if let Some(pair) = self.voice_choice() {
                if let Some(language) = crate::home_server::serve::piper_language(&pair) {
                    state.details.insert("languages".into(), json!([language]));
                }
                state.details.insert("voice".into(), json!(pair));
            }
            abilities.push(state);
        }
        if let Some((id, url)) = self.route() {
            let closed = locked(&self.looks).get(&(id, url)) == Some(&false);
            abilities.push(AbilityState::new(Ability::Model, !closed));
        }
        if let Some(app) = self.app() {
            // The calls are in every computer's build: ready while its Telegram bot is set up.
            abilities.push(AbilityState::new(Ability::Messages, app.calls.telegram.enabled()));
            abilities.push(AbilityState::new(Ability::Apps, true));
            if app.history.is_some() {
                abilities.push(AbilityState::new(Ability::History, true));
            }
            if app.catalog.has_copy_for_station() {
                abilities.push(AbilityState::new(Ability::Catalog, true));
            }
            abilities.push(devices_state(&app));
            // Its home folder for its home (docs/plans/turbo-devices.md, «Папки Турбо»), while it gives it.
            let shares = app.settings.get(&crate::settings::definitions::TURBO_SHARES, None).unwrap_or_default();
            if app.bus.files().is_some() && shares.gives("folders", false) {
                abilities.push(AbilityState::new(Ability::Files, true));
            }
        }
        Contract::new(self.identity(), abilities)
    }

    /// The part is there, ready or not: its own gateway takes a loading engine (the request waits for it), the voice
    /// while it is at home (a cloud's is this computer's own road out, under the regulator), its model while «Мозг»
    /// has one at home, the bot while it is set up, the apps, their history and the catalog always.
    fn gives_here(&self, ability: Ability) -> bool {
        match ability {
            Ability::Hearing => self.recognizer.is_some(),
            Ability::Voice => self.speech.as_ref().is_some_and(|speech| is_local_url(Some(&speech.base_url()))),
            Ability::Model => self.routed.load(Ordering::Relaxed),
            Ability::Messages => self.app().is_some_and(|app| app.calls.telegram.enabled()),
            Ability::Apps | Ability::History | Ability::Catalog => self.app.get().is_some(),
            _ => false,
        }
    }

    fn recognizer(&self) -> Option<Arc<dyn SpeechRecognizer>> {
        self.recognizer.clone()
    }

    fn speech(&self) -> Option<Arc<OwnSpeech>> {
        self.speech.clone()
    }

    /// Every OpenAI-compatible row of «Мозг» whose server is this computer — Ollama and LM Studio both, whatever their
    /// order —, never a helper's; one elsewhere in the home (a NAS) is no model of this computer.
    fn models(&self) -> Vec<OwnModel> {
        let servers = self.servers();
        let looks = locked(&self.looks);
        servers
            .into_iter()
            .map(|(provider, base_url)| {
                let up = looks.get(&(provider.clone(), base_url.clone())).copied();
                OwnModel { provider, base_url, up }
            })
            .collect()
    }

    /// Its host and its port: the host this computer (loopback, a home address of its), the port the one this gateway
    /// listens on, the one published for it, or the one it really listens on.
    fn is_me(&self, url: &str) -> bool {
        let Ok(parsed) = url::Url::parse(url) else { return false };
        let Some(port) = parsed.port_or_known_default() else { return false };
        let app = &self.config.app;
        let ours = port == app.port || app.public_port == Some(port) || port == self.local.port();
        ours && self.here(&parsed)
    }

    /// Its recognition or voice server was found closed at the last look (its own engine, a cloud's voice: never).
    fn silent(&self, ability: Ability) -> bool {
        match ability {
            Ability::Hearing => {
                matches!(&self.hearing, Some(Hearing::Server(url, _)) if self.closed(url))
            }
            Ability::Voice => self.voice_closed(),
            _ => false,
        }
    }

    /// Each model server of this computer looked at, and its recognition and voice servers at home, all at once — a
    /// connection to its port —; then what it declares, told when it changed or a server came or went. A look asked
    /// while another goes waits for it and looks again.
    fn check(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let _one = self.looking.lock().await;
            self.find_addresses();
            self.routed.store(self.route().is_some(), Ordering::Relaxed);
            let servers = self.servers();
            let parts = self.part_servers();
            let (answered, heard) = futures::future::join(
                futures::future::join_all(servers.iter().map(|(_, url)| answers(url))),
                futures::future::join_all(parts.iter().map(|url| answers(url))),
            )
            .await;
            let looked = {
                let mut looks = locked(&self.looks);
                let now: BTreeMap<(String, String), bool> = servers.into_iter().zip(answered).collect();
                let changed = *looks != now;
                *looks = now;
                changed
            };
            *locked(&self.parts) = parts.into_iter().zip(heard).collect();
            self.tell(looked);
        })
    }
}

/// The built-in helper's tests, and what other modules' tests share: a computer's gateway, a model server on it, a row
/// of «Мозг».
#[cfg(test)]
pub(crate) mod tests {
    use std::net::SocketAddr;
    use std::sync::atomic::AtomicUsize;

    use axum::body::Body as HttpBody;
    use axum::extract::ConnectInfo;
    use axum::http::{Request, StatusCode};
    use axum::routing::{get, post};
    use http_body_util::BodyExt;
    use serde_json::{Map, Value};
    use tower::ServiceExt;

    use super::*;
    use crate::config::{Env, LocalAsrConfig};
    use crate::dialog::brain::ProviderInput;
    use crate::helpers::{key_secret, Change, Giver, HelperKind, HelperSetting, HelperState};

    /// A computer's gateway (not in the station role), its .env these, its data in a folder of its own: built, not
    /// started — its tick does not run, a test looks itself (`check`).
    pub(crate) fn computer(vars: &[(&str, &str)]) -> (Arc<App>, tempfile::TempDir) {
        computer_with(vars, |_| {})
    }

    /// The same, its config changed first.
    pub(crate) fn computer_with(
        vars: &[(&str, &str)],
        change: impl FnOnce(&mut Config),
    ) -> (Arc<App>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().to_string_lossy().to_string();
        let base = [("STATE_DIR", ""), ("DATA_DIR", data.as_str()), ("PLUGINS_BUNDLED", "false")];
        let env = Env::from_pairs(base.into_iter().chain(vars.iter().copied()));
        let mut config = Config::for_tests(&env);
        change(&mut config);
        (App::build(config).unwrap(), dir)
    }

    /// A model server on this computer (Ollama, LM Studio): each chat answered with `answer`, counted; closed, its
    /// port takes no connection.
    pub(crate) struct Server {
        /// `http://127.0.0.1:<port>/v1`.
        pub url: String,
        pub chats: Arc<AtomicUsize>,
        task: tokio::task::JoinHandle<()>,
    }

    impl Server {
        pub(crate) fn chats(&self) -> usize {
            self.chats.load(Ordering::SeqCst)
        }

        /// It stops listening — for good once a connection to its port is refused: a connection may still complete
        /// for a moment after the listener is gone.
        pub(crate) async fn close(self) {
            let address = self.url.trim_start_matches("http://").trim_end_matches("/v1").to_string();
            self.task.abort();
            let _ = self.task.await;
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while tokio::net::TcpStream::connect(address.as_str()).await.is_ok() && std::time::Instant::now() < deadline
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
    }

    pub(crate) async fn server(answer: &'static str) -> Server {
        let chats = Arc::new(AtomicUsize::new(0));
        let counted = chats.clone();
        let router = axum::Router::new()
            .route(
                "/v1/chat/completions",
                post(move || {
                    let chats = counted.clone();
                    async move {
                        chats.fetch_add(1, Ordering::SeqCst);
                        let chunk = json!({ "choices": [{ "delta": { "content": answer } }] });
                        ([("content-type", "text/event-stream")], format!("data: {chunk}\n\ndata: [DONE]\n\n"))
                    }
                }),
            )
            .route("/v1/models", get(|| async { axum::Json(json!({ "data": [{ "id": "local-model" }] })) }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Server { url, chats, task }
    }

    /// An address whose port takes connections while the listener lives (`http://127.0.0.1:<port>/v1`): a server of
    /// this computer that nothing here asks.
    pub(crate) async fn listening() -> (String, tokio::net::TcpListener) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        (format!("http://{}/v1", listener.local_addr().unwrap()), listener)
    }

    /// Waits until a connection to a closed listener's port is refused: one may still complete for a moment after it.
    async fn refused(url: &str) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while answers(url).await && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// What a model's slot answers to a question.
    pub(crate) async fn said(slot: &crate::dialog::llm::brain::ModelSlot) -> String {
        use crate::dialog::llm::{LlmEvent, LlmMessage, LlmRequest};
        use futures::StreamExt;
        let request = LlmRequest {
            model: Some(slot.model.clone()),
            system: "s".into(),
            messages: vec![LlmMessage::User("Который час?".into())],
            ..LlmRequest::default()
        };
        let mut text = String::new();
        let mut events = slot.client.stream(request, tokio_util::sync::CancellationToken::new());
        while let Some(event) = events.next().await {
            if let Ok(LlmEvent::Text(piece)) = event {
                text.push_str(&piece);
            }
        }
        text
    }

    /// A row of «Мозг» as the panel adds it: its id.
    pub(crate) fn provider(app: &App, preset: &str, url: &str) -> String {
        let input = ProviderInput::from_value(json!({ "preset": preset, "baseUrl": url }));
        app.brain.add(&input, None).unwrap().id
    }

    /// A request to `/home/v1` as a station sends it: from the home network, with a key that has `home.models`.
    async fn from_station(
        app: &Arc<App>,
        request: axum::http::request::Builder,
        body: HttpBody,
    ) -> (StatusCode, Value) {
        let grants = Map::from_iter([
            ("name".into(), json!("Колонка: домашний сервер")),
            ("grants".into(), json!(["home.models"])),
        ]);
        let key = app.access.keys.create(&grants, None).unwrap().secret;
        let mut request = request.header("authorization", format!("Bearer {key}")).body(body).unwrap();
        request.extensions_mut().insert(ConnectInfo(SocketAddr::new("192.168.1.20".parse().unwrap(), 5000)));
        let response = crate::app::router(app.clone()).oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    /// `/home/v1/status` as a station asks it.
    async fn status(app: &Arc<App>) -> Value {
        let (code, body) = from_station(app, Request::builder().uri("/home/v1/status"), HttpBody::empty()).await;
        assert_eq!(code, StatusCode::OK, "{body}");
        body
    }

    /// What a station's question for this computer's voice gets: the status, and the body when it is words.
    async fn speak(app: &Arc<App>, text: &str) -> (StatusCode, Value) {
        let request = Request::builder().method("POST").uri("/home/v1/tts/audio/speech");
        from_station(app, request, HttpBody::from(json!({ "input": text }).to_string())).await
    }

    /// Waits for `helpers.changed {id: "self", change: "contract"}` (up to five seconds): the panels and «Мозг» read
    /// again.
    async fn told(events: &mut tokio::sync::broadcast::Receiver<Arc<GatewayEvent>>) -> bool {
        let contract = |event: &GatewayEvent| matches!(event, GatewayEvent::HelpersChanged { id: Some(id), change, .. } if id == SELF && change == "contract");
        let wait = async {
            loop {
                match events.recv().await {
                    Ok(event) if contract(&event) => return true,
                    Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return false,
                }
            }
        };
        tokio::time::timeout(Duration::from_secs(5), wait).await.unwrap_or(false)
    }

    /// The fields a station of 0.11 reads: the answer without its contract.
    /// The fields a station of 0.11 and before reads: without the contract and the computer's state
    /// (`system/device`, operations/device_state.rs), which the station's own home is told besides.
    fn old_fields(status: &Value) -> Value {
        let mut old = status.clone();
        old.as_object_mut().unwrap().remove("contract");
        old.as_object_mut().unwrap().remove("device");
        old
    }

    fn old(asr: bool, tts: bool, voice: Value, llm: bool) -> Value {
        json!({
            "version": crate::VERSION, "peerApi": crate::core::build::PEER_API,
            "asr": asr, "tts": tts, "voice": voice, "llm": llm,
        })
    }

    /// Its own engine, never started here: its model not in.
    fn loading_engine(dir: &std::path::Path) -> impl FnOnce(&mut Config) + '_ {
        move |config: &mut Config| {
            config.speech.asr.local = Some(LocalAsrConfig { dir: dir.to_path_buf(), mirror: None, gpu: false });
        }
    }

    #[tokio::test]
    async fn the_built_in_helper_gives_what_the_computer_has() {
        let ollama = server("Ответ Ollama.").await;
        // Its recognition and voice servers take connections: their look finds them open.
        let ((asr, _asr_port), (tts, _tts_port)) = (listening().await, listening().await);
        let (app, _dir) = computer(&[("ASR_BASE_URL", asr.as_str()), ("TTS_OWN_URL", tts.as_str())]);
        provider(&app, "ollama", &ollama.url);
        app.builtin.check().await;
        let contract = app.builtin.contract();
        assert_eq!(contract.helper, app.identity.as_helper(), "the computer names itself");
        for ability in [Ability::Hearing, Ability::Voice, Ability::Model, Ability::Apps] {
            assert!(contract.ready(ability), "{ability:?}");
            assert!(app.builtin.gives_here(ability), "{ability:?}");
        }
        assert!(contract.declares(Ability::Messages) && !contract.ready(Ability::Messages), "no bot set up here");
        assert!(!app.builtin.gives_here(Ability::Messages));
        assert!(!contract.declares(Ability::Catalog), "no copy of a feed here");
        // The computer takes its parts from itself, first, without the network.
        for ability in [Ability::Hearing, Ability::Voice, Ability::Model, Ability::Apps, Ability::History] {
            assert!(matches!(app.helpers.pick(ability), Some(Giver::Itself)), "{ability:?}");
        }
        assert!(app.helpers.pick(Ability::Messages).is_none(), "no bot, no helper with one");
        let hears = app.helpers.own_recognizer().unwrap().description();
        assert!(hears.ends_with(&format!("via {asr}/audio/transcriptions")), "{hears}");
        assert!(Arc::ptr_eq(&app.builtin.speech().unwrap(), app.voice.own_speech().unwrap()), "the very engine");

        // A cloud's recognition server is never handed on, nor the built-in helper's: the cloud stays the computer's
        // own road out, under the regulator. Its voice at a cloud is handed on as ever, and not taken as the built-in's.
        let (cloud, _cloud_dir) = computer(&[
            ("ASR_BASE_URL", "https://api.example.invalid/v1"),
            ("TTS_OWN_URL", "https://tts.example.invalid/v1"),
        ]);
        assert!(cloud.builtin.hearing().is_none() && !cloud.builtin.gives_here(Ability::Hearing));
        assert!(!cloud.builtin.contract().declares(Ability::Hearing));
        assert!(cloud.builtin.contract().declares(Ability::Voice), "/home/v1 answers as before");
        assert!(!cloud.builtin.gives_here(Ability::Voice));
        ollama.close().await;
    }

    #[tokio::test]
    async fn the_status_answers_as_before_and_adds_the_contract() {
        // Its recognition server, a voice and a model of its own.
        let ollama = server("Ответ Ollama.").await;
        let (app, _dir) = computer(&[
            ("ASR_BASE_URL", "http://127.0.0.1:8090/v1"),
            ("TTS_OWN_URL", "http://127.0.0.1:8091/v1"),
            ("TTS_OWN_MODEL", "piper"),
            ("TTS_OWN_VOICE", "ru_RU-irina-medium"),
        ]);
        provider(&app, "ollama", &ollama.url);
        let body = status(&app).await;
        assert_eq!(old_fields(&body), old(true, true, json!("piper/ru_RU-irina-medium"), true));
        assert_eq!(body["device"]["kind"], "computer", "its own home's centre reads the computer's state with it");
        assert_eq!(body["contract"]["version"], crate::helpers::contract::VERSION);
        // The status says besides that this computer speaks the link while it helps (stage 7).
        let mut said = body["contract"].clone();
        assert_eq!(said.as_object_mut().unwrap().remove("link"), Some(json!(crate::helpers::link::VERSION)));
        // A home at the home network's address reaches its things at hand (docs/plans/turbo-devices.md): said per caller.
        let devices =
            said["abilities"].as_array_mut().unwrap().iter_mut().find(|state| state["id"] == "devices").unwrap();
        assert_eq!(devices["details"].as_object_mut().unwrap().remove("home"), Some(json!(true)));
        assert_eq!(said, serde_json::to_value(app.builtin.contract()).unwrap(), "the built-in's");

        // Its own engine, its model still loading: nothing ready for a station yet.
        let engine = tempfile::tempdir().unwrap();
        let (app, _dir) = computer_with(&[], loading_engine(engine.path()));
        let body = status(&app).await;
        assert_eq!(old_fields(&body), old(false, false, Value::Null, false));
        assert_eq!(body["contract"]["abilities"][0]["id"], "hearing");

        // Nothing of its own: recognition off, no voice, no model.
        let (app, _dir) = computer(&[("ASR_PROVIDER", "none")]);
        let body = status(&app).await;
        assert_eq!(old_fields(&body), old(false, false, Value::Null, false));
        let declared: Vec<&str> = body["contract"]["abilities"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|ability| ability["id"].as_str())
            .collect();
        assert!(!declared.contains(&"hearing") && !declared.contains(&"voice") && !declared.contains(&"model"));
        ollama.close().await;
    }

    #[tokio::test]
    async fn its_own_gateway_takes_a_loading_engine_others_see_it_not_ready() {
        let engine = tempfile::tempdir().unwrap();
        let (app, _dir) = computer_with(&[], loading_engine(engine.path()));
        assert!(app.builtin.gives_here(Ability::Hearing) && !app.builtin.hears());
        assert!(!app.builtin.contract().ready(Ability::Hearing), "a station does not lean on it yet");
        assert!(matches!(app.helpers.pick(Ability::Hearing), Some(Giver::Itself)), "its own gateway waits for it");
        assert!(app.speech.recognizes());
        let asr = app.speech.status().asr;
        assert!(asr.contains("on this computer"), "the engine's own words: {asr}");
    }

    /// Its voice server at a cloud's address is handed on only while the regulator lets the clouds out: «Только дома»
    /// keeps a station's text home through this computer too — declared as before (the old `tts`), not ready, its door
    /// refusing — and the panels and «Мозг» are told. A voice at home is handed on under every position.
    #[tokio::test]
    async fn its_voice_at_a_cloud_is_not_handed_on_while_the_regulator_keeps_the_clouds_in() {
        let (app, _dir) = computer(&[("ASR_PROVIDER", "none"), ("TTS_OWN_URL", "https://tts.example.invalid/v1")]);
        app.builtin.start();
        let mut events = app.events.subscribe();
        assert!(app.builtin.contract().ready(Ability::Voice) && !app.builtin.voice_kept_in(), "«Любые облака»");

        app.helpers.set_away(crate::helpers::Away::Home, None).unwrap();
        assert!(told(&mut events).await, "what it gives changed");
        let contract = app.builtin.contract();
        assert!(contract.declares(Ability::Voice) && !contract.ready(Ability::Voice));
        let body = status(&app).await;
        assert_eq!(body["tts"], true, "a station of 0.11 reads only this: its door refuses instead");
        let (code, refused) = speak(&app, "Привет").await;
        assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(refused["message"], "Регулятор не пускает облако", "{refused}");

        // At home: handed on under «Только дома» too.
        let (home, _home_dir) = computer(&[("ASR_PROVIDER", "none"), ("TTS_OWN_URL", "http://127.0.0.1:8091/v1")]);
        home.helpers.set_away(crate::helpers::Away::Home, None).unwrap();
        assert!(home.builtin.contract().ready(Ability::Voice) && !home.builtin.voice_kept_in());
    }

    /// What it declares is followed as it changes — here a model of «Мозг» added on this computer —: taken at once
    /// without a look, and told (`helpers.changed {id: "self", change: "contract"}`).
    #[tokio::test]
    async fn a_change_of_what_it_declares_is_followed_and_told_at_once() {
        let ollama = server("Ответ Ollama.").await;
        let (app, _dir) = computer(&[]);
        app.builtin.start();
        let mut events = app.events.subscribe();
        assert!(!app.builtin.gives_here(Ability::Model) && !app.builtin.contract().declares(Ability::Model));
        provider(&app, "ollama", &ollama.url);
        assert!(told(&mut events).await, "the panels and «Мозг» read again");
        assert!(app.builtin.gives_here(Ability::Model) && app.builtin.contract().ready(Ability::Model), "no look yet");
        assert!(matches!(app.helpers.pick(Ability::Model), Some(Giver::Itself)));
        ollama.close().await;
    }

    #[tokio::test]
    async fn a_closed_local_model_server_is_seen_at_the_next_look_and_a_cloud_one_is_not_ours() {
        let ollama = server("Ответ Ollama.").await;
        let (app, _dir) = computer(&[]);
        app.builtin.set_addresses(vec![Ipv4Addr::new(192, 168, 1, 20)]);
        let row = provider(&app, "ollama", &ollama.url);
        // Another machine of the home (a NAS), a cloud: models of «Мозг», none of them this computer's.
        provider(&app, "custom-openai", "http://192.168.1.50:11434/v1");
        provider(&app, "custom-openai", "https://llm.example.invalid/v1");
        let mut events = app.events.subscribe();
        app.builtin.check().await;
        let looked = OwnModel { provider: row.clone(), base_url: ollama.url.clone(), up: Some(true) };
        assert_eq!(app.builtin.models(), std::slice::from_ref(&looked));
        assert!(app.builtin.contract().ready(Ability::Model));

        ollama.close().await;
        app.builtin.check().await;
        assert_eq!(app.builtin.models(), [OwnModel { up: Some(false), ..looked }]);
        let contract = app.builtin.contract();
        assert!(contract.declares(Ability::Model) && !contract.ready(Ability::Model), "its server is closed");
        let body = status(&app).await;
        assert_eq!(body["llm"], true, "the old field stays: a model is set up here");
        let model = body["contract"]["abilities"].as_array().unwrap().iter().find(|ability| ability["id"] == "model");
        assert_eq!(model.map(|model| &model["ready"]), Some(&json!(false)), "a station reads it closed: {body}");
        let mut told = 0;
        while let Ok(event) = events.try_recv() {
            told += usize::from(matches!(
                &*event,
                GatewayEvent::HelpersChanged { id: Some(id), change, .. } if id == SELF && change == "contract"
            ));
        }
        assert_eq!(told, 2, "the first look, then the server closed");
    }

    /// Its recognition and voice servers are looked at with its model servers (review of the merge with main, 16.11).
    /// Closed, they are not ready for the gateways it helps — a station of 0.11 reads `asr: false` too —, and «Где
    /// сейчас» here names the road that answers and says why; the computer itself still tries them first.
    #[tokio::test]
    async fn a_closed_recognition_or_voice_server_is_seen_at_the_next_look() {
        let ((asr, asr_port), (tts, tts_port)) = (listening().await, listening().await);
        let (app, _dir) =
            computer(&[("ASR_BASE_URL", asr.as_str()), ("TTS_OWN_URL", tts.as_str()), ("TTS_PROVIDER", "own")]);
        let row = |ability: Ability| {
            crate::helpers::now::rows(&app).into_iter().find(|row| row.ability == ability).expect("a row")
        };
        app.builtin.check().await;
        let contract = app.builtin.contract();
        assert!(contract.ready(Ability::Hearing) && contract.ready(Ability::Voice));
        assert!(row(Ability::Hearing).first && row(Ability::Voice).first, "this computer gives both");

        drop(asr_port);
        refused(&asr).await;
        app.builtin.check().await;
        let contract = app.builtin.contract();
        assert!(!contract.ready(Ability::Hearing) && contract.ready(Ability::Voice), "its voice server still answers");
        assert!(app.builtin.gives_here(Ability::Hearing), "tried first here all the same");
        let hearing = row(Ability::Hearing);
        assert_eq!((hearing.first, hearing.why.as_deref()), (false, Some("слух на этом компьютере не отвечает")));
        assert_eq!(status(&app).await["asr"], false);

        drop(tts_port);
        refused(&tts).await;
        app.builtin.check().await;
        assert!(!app.builtin.contract().ready(Ability::Voice));
        let voice = row(Ability::Voice);
        assert_eq!((voice.first, voice.why.as_deref()), (false, Some("голос на этом компьютере не отвечает")));
    }

    #[tokio::test]
    async fn every_model_server_of_this_computer_is_looked_at() {
        let (ollama, lmstudio) = (server("Ollama.").await, server("LM Studio.").await);
        let (app, _dir) = computer(&[]);
        let first = provider(&app, "ollama", &ollama.url);
        let second = provider(&app, "lmstudio", &lmstudio.url.replace("127.0.0.1", "localhost"));
        app.builtin.check().await;
        let rows: Vec<(String, Option<bool>)> =
            app.builtin.models().into_iter().map(|model| (model.provider, model.up)).collect();
        assert_eq!(rows, [(first.clone(), Some(true)), (second.clone(), Some(true))], "both, in the rows' order");

        lmstudio.close().await;
        app.builtin.check().await;
        let rows: Vec<(String, Option<bool>)> =
            app.builtin.models().into_iter().map(|model| (model.provider, model.up)).collect();
        assert_eq!(rows, [(first, Some(true)), (second, Some(false))], "the first unchanged");
        assert!(app.builtin.contract().ready(Ability::Model), "the model it hands on is the first's");
        ollama.close().await;
    }

    #[tokio::test]
    async fn an_entry_pointing_at_this_computer_is_itself() {
        let (app, _dir) = computer(&[("PORT", "7780"), ("PUBLIC_PORT", "7790")]);
        app.helpers.set_local_port(54321);
        app.builtin.set_addresses(vec![Ipv4Addr::new(192, 168, 1, 20)]);
        let mine = [
            "http://127.0.0.1:7780",
            "http://localhost:7780",
            "http://192.168.1.20:7780",
            "http://127.0.0.1:7790",
            "http://[::1]:54321",
            "http://127.0.0.1:54321/",
        ];
        for url in mine {
            assert!(app.builtin.is_me(url), "{url}");
        }
        let others = [
            "http://127.0.0.1:7781",
            "http://192.168.1.21:7780",
            "https://helper.example.com:7780",
            "http://192.168.1.20",
        ];
        for url in others.into_iter().chain(["not an address"]) {
            assert!(!app.builtin.is_me(url), "{url}");
        }
        // In the list it is this gateway itself: shown so, never looked at nor picked.
        let entry = HelperSetting {
            id: "home-me".into(),
            name: "Я".into(),
            url: "http://192.168.1.20:7780".into(),
            kind: HelperKind::Home,
            enabled: true,
            without: Vec::new(),
            helper_id: None,
        };
        app.secrets.set(&key_secret("home-me"), Some("vgw_me")).unwrap();
        app.helpers
            .write(None, |list| {
                list.push(entry.clone());
                Ok(Some(()))
            })
            .unwrap();
        assert!(app.helpers.is_self(&entry));
        assert_eq!(app.helpers.views()[1].state, HelperState::Itself);
        assert!(app.helpers.offering(Ability::Hearing).is_empty());
    }

    #[tokio::test]
    async fn the_computers_list_starts_with_itself_and_its_own_parts_come_first() {
        let (app, _dir) = computer(&[
            ("ASR_BASE_URL", "http://127.0.0.1:8090/v1"),
            ("TTS_OWN_URL", "http://127.0.0.1:8091/v1"),
            ("TTS_PROVIDER", "mixed"),
        ]);
        let views = app.helpers.views();
        let me = &views[0];
        assert_eq!(
            (me.id.as_str(), me.name.as_str(), me.builtin, me.kind, me.state),
            (SELF, "Этот компьютер", true, HelperKind::Home, HelperState::Up)
        );
        assert!(me.enabled && me.allowed && me.without.is_empty() && me.url.is_none() && me.key.is_none());
        assert_eq!(me.contract, Some(crate::helpers::contract::VERSION));
        assert_eq!(me.helper.as_ref().map(|helper| helper.id.clone()), app.identity.gateway().map(|me| me.id));
        assert_eq!(me.abilities, app.builtin.contract().abilities);
        let health = app.helpers.health();
        let brief = &health["list"][0];
        assert_eq!(
            (&brief["id"], &brief["builtin"], &brief["state"], &brief["allowed"]),
            (&json!("self"), &json!(true), &json!("up"), &json!(true))
        );
        // Nothing to move, rename, switch off or remove.
        assert!(app.helpers.order(&[SELF.to_string()], None).is_err());
        assert_eq!(app.helpers.update(SELF, Change::default(), None).unwrap_err().status, StatusCode::NOT_FOUND);
        assert_eq!(app.helpers.remove(SELF, None).unwrap_err().status, StatusCode::NOT_FOUND);
        // «Где сейчас»: its hearing and voice are the built-in helper's.
        let rows = crate::helpers::now::rows(&app);
        for ability in [Ability::Hearing, Ability::Voice] {
            let row = rows.iter().find(|row| row.ability == ability).unwrap();
            assert_eq!(
                (row.by, row.helper.as_deref(), row.name.as_deref(), row.reach, row.first),
                (
                    crate::helpers::now::By::Helper,
                    Some(SELF),
                    Some("Этот компьютер"),
                    Some(crate::helpers::Reach::Home),
                    true
                ),
                "{ability:?}"
            );
        }
    }
}
