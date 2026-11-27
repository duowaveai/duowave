//! The built-in helper (docs/plans/home-and-helpers.md, block 1, «Встроенный помощник»): the heavy part of the
//! computer's own program behind the helpers' contract (contract.rs) — its hearing (the recognition engine of this
//! program, or its recognition server at home), its voice (the own voice's server, the voice lab's voices too), its
//! models (the model servers of «Мозг» on this computer: Ollama, LM Studio), its Telegram bot, its apps with their
//! history, its copy of the catalog, and the works in work folders (works, `works` — docs/plans/router-and-works.md,
//! stage 5). Inside it is still one program; splitting it in two is for later.
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
use super::{Local, OwnModel, OwnParts, SELF};
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
use crate::talk::folder::WorksPort;

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

/// What the built-in helper asks of the parts made after it — the Telegram bot, the apps and their history, the
/// catalog, the devices, the home folder, the regulator —, answered by the gateway once it is built (`attach`,
/// src/links/helpers.rs). The gateway holds itself weakly there: once it is gone, it answers no.
pub trait LateParts: Send + Sync {
    /// The gateway is still there: the late parts are declared only while it is.
    fn here(&self) -> bool;
    /// «Что может уходить из дома» lets the clouds out now (`Helpers::lets`).
    fn clouds_out(&self) -> bool;
    /// Its Telegram bot is set up.
    fn messages(&self) -> bool;
    /// The apps keep their history.
    fn history(&self) -> bool;
    /// The catalog keeps a copy of its feed for a station.
    fn catalog(&self) -> bool;
    /// The kinds of its devices it shares now, as its contract's `devices` carries them (`kinds`).
    fn devices(&self) -> serde_json::Value;
    /// Its home folder is there to give.
    fn folders(&self) -> bool;
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
    late: OnceLock<Arc<dyn LateParts>>,
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
    /// The works in work folders (works), once made: its ability `works`, behind the port the talk layer's executor
    /// «папка» speaks to.
    works: OnceLock<Arc<dyn WorksPort>>,
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
pub async fn answers(base_url: &str) -> bool {
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
            late: OnceLock::new(),
            addresses: RwLock::new(Vec::new()),
            fixed: AtomicBool::new(false),
            looks: Mutex::new(BTreeMap::new()),
            parts: Mutex::new(BTreeMap::new()),
            looking: tokio::sync::Mutex::new(()),
            routed: AtomicBool::new(false),
            told: Mutex::new(None),
            works: OnceLock::new(),
        });
        helper.routed.store(helper.route().is_some(), Ordering::Relaxed);
        helper
    }

    /// The parts made after it: the Telegram bot, the plugins, their history, the catalog — and this computer's home
    /// addresses.
    pub fn attach(&self, late: Arc<dyn LateParts>) {
        let _ = self.late.set(late);
        self.find_addresses();
    }

    /// The works in work folders join it: from now on it gives `works`.
    pub fn set_works(&self, works: Arc<dyn WorksPort>) {
        let _ = self.works.set(works);
    }

    /// The port of its works in folders, when it has them.
    pub fn works(&self) -> Option<Arc<dyn WorksPort>> {
        self.works.get().cloned()
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
        !is_local_url(Some(&speech.base_url())) && !self.late.get().is_some_and(|late| late.clouds_out())
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
    #[cfg(any(test, feature = "testing"))]
    pub fn set_addresses(&self, addresses: Vec<Ipv4Addr>) {
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
fn devices_state(kinds: serde_json::Value) -> AbilityState {
    static MACS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    let macs = MACS.get_or_init(crate::helpers::wake::hardware_addresses);
    let mut state = AbilityState::new(Ability::Devices, true);
    state.details.insert("kinds".into(), kinds);
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
        if let Some(late) = self.late.get().filter(|late| late.here()) {
            // The calls are in every computer's build: ready while its Telegram bot is set up.
            abilities.push(AbilityState::new(Ability::Messages, late.messages()));
            abilities.push(AbilityState::new(Ability::Apps, true));
            if late.history() {
                abilities.push(AbilityState::new(Ability::History, true));
            }
            if late.catalog() {
                abilities.push(AbilityState::new(Ability::Catalog, true));
            }
            abilities.push(devices_state(late.devices()));
            // Its home folder for its home (docs/plans/turbo-devices.md, «Папки Турбо»), while it gives it.
            let shares = self.settings.get(&crate::settings::definitions::TURBO_SHARES, None).unwrap_or_default();
            if late.folders() && shares.gives("folders", false) {
                abilities.push(AbilityState::new(Ability::Files, true));
            }
        }
        if let Some(works) = self.works.get() {
            // Ready while it has a folder to work in; each agent says at its run whether it is signed in.
            let folders = works.folders().len();
            let mut state = AbilityState::new(Ability::Works, folders > 0);
            let agents: Vec<String> = works.agents().into_iter().map(|(id, _)| id).collect();
            state.details.insert("agents".into(), json!(agents));
            state.details.insert("folders".into(), json!(folders));
            state.details.insert("atOnce".into(), json!(crate::talk::works::FOLDERS_AT_ONCE));
            abilities.push(state);
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
            Ability::Messages => self.late.get().is_some_and(|late| late.messages()),
            Ability::Apps | Ability::History | Ability::Catalog => self.late.get().is_some(),
            Ability::Works => self.works.get().is_some(),
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
