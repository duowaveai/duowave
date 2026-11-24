//! The helpers' unit tests, and what other modules' tests share: a helper's `/home/v1` on 127.0.0.1 (a stand-in at
//! home), and a station — its helpers, its mode switch — over stores in memory.

// Without `test` (the program's tests, feature `testing`) only the stand-ins are used.
#![cfg_attr(not(test), allow(dead_code, unused_imports))]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::Router;
use serde_json::{json, Value};

use super::*;
use crate::config::Env;
use crate::core::build::ModeGate;
use crate::core::identity::home::DeviceKind;
use crate::settings::definitions::{GatewaySelf, ModeSetting, GATEWAY_SELF, MODE};
use crate::speech::VoiceService;
use crate::storage::Db;

/// The key every stand-in takes.
pub const KEY: &str = "vgw_helper_key";

/// Every ability of contract 1, ready.
pub const ALL: &[(&str, bool)] = &[
    ("hearing", true),
    ("voice", true),
    ("model", true),
    ("messages", true),
    ("apps", true),
    ("history", true),
    ("catalog", true),
];

/// A status of contract 1: the helper's id and what it declares, ready or not.
pub fn contract(id: &str, abilities: &[(&str, bool)]) -> Value {
    let abilities: Vec<Value> = abilities.iter().map(|(id, ready)| json!({ "id": id, "ready": ready })).collect();
    json!({
        "version": "0.12.0", "peerApi": 2, "asr": true, "tts": true, "voice": "silero/baya", "llm": false,
        "contract": { "version": 1, "helper": { "id": id, "name": format!("Турбо {id}") }, "abilities": abilities }
    })
}

/// A helper's `/home/v1` on 127.0.0.1: its status (a switch makes it miss), its bot's contacts, its sends, its hearing.
#[derive(Default)]
pub struct Stand {
    /// `http://127.0.0.1:<port>`.
    pub url: String,
    pub port: u16,
    /// Its status answers 500: a miss.
    pub away: AtomicBool,
    pub status: Mutex<Value>,
    /// None: no bot set up (503).
    pub contacts: Mutex<Option<Vec<String>>>,
    /// No messenger's routes (404): an older helper.
    pub no_messenger: AtomicBool,
    /// What its hearing hears.
    pub heard: Mutex<String>,
    /// Its hearing answers 500.
    pub deaf: AtomicBool,
    /// The looks at its status that reached it.
    pub looks: AtomicUsize,
    /// The headers of the last look.
    pub headers: Mutex<Option<HeaderMap>>,
    /// What its bot was asked to pass on.
    pub sent: Mutex<Vec<(String, String)>>,
    /// Telegram refuses.
    pub refuses: AtomicBool,
    /// How long its status waits before it answers.
    pub delay: Mutex<Duration>,
    /// Its status is held until `release`, saying `arrived`: a look out, for as long as a test needs.
    pub hold: AtomicBool,
    pub arrived: tokio::sync::Notify,
    pub release: tokio::sync::Notify,
}

impl Stand {
    pub fn looks(&self) -> usize {
        self.looks.load(Ordering::SeqCst)
    }

    pub fn set_away(&self, away: bool) {
        self.away.store(away, Ordering::SeqCst);
    }
}

fn keyed(headers: &HeaderMap) -> bool {
    headers.get("authorization").and_then(|value| value.to_str().ok()) == Some(&format!("Bearer {KEY}"))
}

/// A stand-in helper answering this status.
pub async fn stand(status: Value) -> Arc<Stand> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let state = Arc::new(Stand {
        url: format!("http://127.0.0.1:{port}"),
        port,
        status: Mutex::new(status),
        heard: Mutex::new("слышу".into()),
        ..Stand::default()
    });
    let looked = state.clone();
    let status = move |headers: HeaderMap| {
        let state = looked.clone();
        async move {
            if state.hold.load(Ordering::SeqCst) {
                state.arrived.notify_one();
                state.release.notified().await;
            }
            let delay = *state.delay.lock().unwrap();
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            if state.away.load(Ordering::SeqCst) {
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            }
            if !keyed(&headers) {
                return Err(StatusCode::UNAUTHORIZED);
            }
            state.looks.fetch_add(1, Ordering::SeqCst);
            *state.headers.lock().unwrap() = Some(headers);
            Ok(axum::Json(state.status.lock().unwrap().clone()))
        }
    };
    let read = state.clone();
    let contacts = move |headers: HeaderMap| {
        let state = read.clone();
        async move {
            if state.away.load(Ordering::SeqCst) {
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            }
            if !keyed(&headers) {
                return Err(StatusCode::UNAUTHORIZED);
            }
            if state.no_messenger.load(Ordering::SeqCst) {
                return Err(StatusCode::NOT_FOUND);
            }
            let contacts = state.contacts.lock().unwrap().clone().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
            Ok(axum::Json(json!({ "contacts": contacts })))
        }
    };
    let bot = state.clone();
    let send = move |headers: HeaderMap, axum::Json(body): axum::Json<Value>| {
        let state = bot.clone();
        async move {
            if !keyed(&headers) {
                return Err(StatusCode::UNAUTHORIZED);
            }
            let Some(contacts) = state.contacts.lock().unwrap().clone() else {
                return Err(StatusCode::SERVICE_UNAVAILABLE);
            };
            let contact = body["contact"].as_str().unwrap_or_default().to_string();
            if !contacts.contains(&contact) {
                return Err(StatusCode::NOT_FOUND);
            }
            if state.refuses.load(Ordering::SeqCst) {
                return Err(StatusCode::BAD_GATEWAY);
            }
            state.sent.lock().unwrap().push((contact.clone(), body["text"].as_str().unwrap_or_default().into()));
            Ok(axum::Json(json!({ "sent": true, "contact": contact })))
        }
    };
    let ear = state.clone();
    let hear = move |headers: HeaderMap| {
        let state = ear.clone();
        async move {
            if !keyed(&headers) {
                return Err(StatusCode::UNAUTHORIZED);
            }
            if state.deaf.load(Ordering::SeqCst) || state.away.load(Ordering::SeqCst) {
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            }
            let heard = state.heard.lock().unwrap().clone();
            Ok(axum::Json(json!({
                "text": heard, "language": "english", "words": [{ "word": heard, "start": 0.0, "end": 0.5 }]
            })))
        }
    };
    let router = Router::new()
        .route("/home/v1/status", get(status))
        .route("/home/v1/messenger/contacts", get(contacts))
        .route("/home/v1/messenger/send", post(send))
        .route("/home/v1/asr/audio/transcriptions", post(hear))
        .route("/home/v1/asr/models", get(|| async { axum::Json(json!({ "data": [{ "id": "home" }] })) }))
        .route("/home/v1/tts/audio/speech", post(|| async { b"OggS-voice".to_vec() }));
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    state
}

/// The station's mode switch as the registry asks it: «Свой» only while its helpers are up.
struct Gate(Weak<Helpers>);

impl ModeGate for Gate {
    fn check(&self, mode: PanelMode) -> Result<(), String> {
        let up = self.0.upgrade().is_some_and(|helpers| helpers.state() == HomeServer::Up);
        if mode == PanelMode::Own && !up {
            return Err("«Ваш ИИ» сейчас не включить: Турбо не на связи.".into());
        }
        Ok(())
    }

    fn absent(&self, _mode: PanelMode) -> Option<String> {
        None
    }
}

/// A station's helpers and its mode switch, over stores in memory.
pub struct Station {
    pub helpers: Arc<Helpers>,
    pub modes: Arc<ModeService>,
    pub settings: Arc<SettingsService>,
    pub secrets: Arc<SecretsService>,
    pub identity: Arc<Identity>,
    pub events: EventBus,
    pub db: Arc<Db>,
    pub config: Arc<Config>,
}

/// A station's gateway (a computer's build playing it), its .env these over the base; `own` — started in «Свой».
pub fn station_with(extra: &[(&str, &str)], own: bool) -> Station {
    station_stored(extra, own, |_, _| {})
}

/// The same, started on what `store` keeps first — as a package of settings taken before a start leaves it.
pub fn station_stored(
    extra: &[(&str, &str)],
    own: bool,
    store: impl FnOnce(&Arc<SettingsService>, &Arc<SecretsService>),
) -> Station {
    station_gated(extra, own, store, |helpers| Arc::new(Gate(Arc::downgrade(helpers))))
}

/// The same, its mode switch asking `gate`, made over its helpers: the registry of what works where, as `App` has it.
pub fn station_gated(
    extra: &[(&str, &str)],
    own: bool,
    store: impl FnOnce(&Arc<SettingsService>, &Arc<SecretsService>),
    gate: impl FnOnce(&Arc<Helpers>) -> Arc<dyn ModeGate>,
) -> Station {
    let mut pairs = vec![("UNIPROXY_MODE", "gateway"), ("DUOWAVE_ROLE", "station")];
    pairs.extend_from_slice(extra);
    let config = Arc::new(Config::for_tests(&Env::from_pairs(pairs)));
    let db = Arc::new(Db::memory());
    let events = EventBus::new();
    let settings = Arc::new(SettingsService::new(db.clone(), events.clone()));
    let secrets = Arc::new(SecretsService::new(db.clone(), None).unwrap());
    let profile = Profile::for_role(config.placement.role.as_deref());
    let kind = DeviceKind::of(profile);
    let identity = Arc::new(Identity::new(settings.clone(), secrets.clone(), db.clone(), kind, events.clone()));
    identity.ensure().unwrap();
    store(&settings, &secrets);
    door::at_start(&config, &settings, &secrets);
    if own {
        settings.set(&MODE, &ModeSetting { mode: PanelMode::Own }, None).unwrap();
    }
    let helpers =
        Helpers::new(config.clone(), settings.clone(), secrets.clone(), events.clone(), profile, identity.clone());
    let voice = Arc::new(VoiceService::new(&config.gateway, events.clone()));
    let gate = gate(&helpers);
    let modes = Arc::new(ModeService::new(config.gateway.mode, settings.clone(), events.clone(), voice, gate));
    helpers.set_modes(modes.clone());
    Station { helpers, modes, settings, secrets, identity, events, db, config }
}

pub fn station(extra: &[(&str, &str)]) -> Station {
    station_with(extra, false)
}

impl Station {
    /// A helper stored as the list keeps it, without a look: its entry, last, and its key.
    pub fn put(&self, id: &str, url: &str, kind: HelperKind) {
        let entry = HelperSetting {
            id: id.into(),
            name: format!("Турбо {id}"),
            url: url.into(),
            kind,
            enabled: true,
            without: Vec::new(),
            helper_id: None,
        };
        self.secrets.set(&key_secret(id), Some(KEY)).unwrap();
        self.helpers
            .write(None, |list| {
                list.push(entry);
                Ok(Some(()))
            })
            .unwrap();
    }

    /// Who gives the ability now, by entry id; "self" for this gateway.
    pub fn by(&self, ability: Ability) -> Option<String> {
        self.helpers.pick(ability).map(|giver| match giver {
            Giver::Itself => "self".to_string(),
            Giver::Helper(remote) => remote.id().to_string(),
        })
    }

    pub fn mode(&self) -> (PanelMode, Option<String>) {
        let view = self.modes.view();
        (view.mode, view.auto.map(|auto| auto.reason))
    }

    pub fn change(&self, id: &str, change: Change) {
        self.helpers.update(id, change, None).unwrap();
    }
}

/// `n` looks at every helper.
async fn looks(station: &Station, n: usize) {
    for _ in 0..n {
        station.helpers.check_all().await;
    }
}

#[tokio::test]
async fn hearing_voice_and_apps_go_to_the_first_helper_up_and_down_the_list_when_it_goes() {
    let (a, b) = (stand(contract("a", ALL)).await, stand(contract("b", ALL)).await);
    let station = station(&[]);
    station.put("home-a", &a.url, HelperKind::Home);
    station.put("home-b", &b.url, HelperKind::Home);
    for ability in [Ability::Hearing, Ability::Voice, Ability::Apps] {
        assert_eq!(station.by(ability), None, "nobody before the first look");
    }
    looks(&station, 1).await;
    for ability in [Ability::Hearing, Ability::Voice, Ability::Apps] {
        assert_eq!(station.by(ability).as_deref(), Some("home-a"), "{ability:?}: the first on the line");
    }
    assert_eq!(station.helpers.state(), HomeServer::Up);

    a.set_away(true);
    looks(&station, 1).await;
    assert_eq!(station.by(Ability::Hearing).as_deref(), Some("home-a"), "one miss is not away yet");
    looks(&station, 1).await;
    for ability in [Ability::Hearing, Ability::Voice, Ability::Apps] {
        assert_eq!(station.by(ability).as_deref(), Some("home-b"), "{ability:?}: down the list");
    }
    let order: Vec<String> = station
        .helpers
        .pick_all(Ability::Hearing)
        .into_iter()
        .filter_map(|giver| match giver {
            Giver::Helper(remote) => Some(remote.id().to_string()),
            Giver::Itself => None,
        })
        .collect();
    assert_eq!(order, ["home-b"]);

    a.set_away(false);
    looks(&station, 1).await;
    assert_eq!(station.by(Ability::Voice).as_deref(), Some("home-a"), "back: the first again");
}

#[tokio::test]
async fn a_helper_switched_off_or_off_for_an_ability_gives_way_to_the_next() {
    let (a, b) = (stand(contract("a", ALL)).await, stand(contract("b", ALL)).await);
    let station = station(&[]);
    station.put("home-a", &a.url, HelperKind::Home);
    station.put("home-b", &b.url, HelperKind::Home);
    looks(&station, 1).await;
    station.change("home-a", Change { enabled: Some(false), ..Change::default() });
    for ability in [Ability::Hearing, Ability::Voice, Ability::Apps, Ability::Model] {
        assert_eq!(station.by(ability).as_deref(), Some("home-b"), "{ability:?}");
    }
    let before = a.looks();
    looks(&station, 1).await;
    assert_eq!(a.looks(), before, "a switched-off helper is not looked at");

    station.change("home-a", Change { enabled: Some(true), without: Some(vec![Ability::Voice]), ..Change::default() });
    looks(&station, 1).await;
    assert_eq!(station.by(Ability::Voice).as_deref(), Some("home-b"), "not taken from it: the next");
    assert_eq!(station.by(Ability::Hearing).as_deref(), Some("home-a"), "the rest still its");
    assert_eq!(station.helpers.entry("home-a").unwrap().without, [Ability::Voice]);
}

#[tokio::test]
async fn a_helper_that_does_not_declare_or_is_not_ready_is_passed_by() {
    let quiet: Vec<(&str, bool)> = ALL.iter().copied().filter(|(id, _)| *id != "hearing").collect();
    let (a, b) = (stand(contract("a", &quiet)).await, stand(contract("b", ALL)).await);
    let station = station(&[]);
    station.put("home-a", &a.url, HelperKind::Home);
    station.put("home-b", &b.url, HelperKind::Home);
    looks(&station, 1).await;
    assert_eq!(station.by(Ability::Hearing).as_deref(), Some("home-b"), "it does not declare hearing");
    assert_eq!(station.by(Ability::Voice).as_deref(), Some("home-a"));
    assert!(matches!(station.helpers.offer(Ability::Hearing), Some(Giver::Helper(remote)) if remote.id() == "home-b"));

    // Hearing declared, its model still loading: passed by while not ready — yet it is the one that would give it.
    let mut loading: Vec<(&str, bool)> = ALL.to_vec();
    loading[0] = ("hearing", false);
    *a.status.lock().unwrap() = contract("a", &loading);
    looks(&station, 1).await;
    assert_eq!(station.by(Ability::Hearing).as_deref(), Some("home-b"));
    assert!(matches!(station.helpers.offer(Ability::Hearing), Some(Giver::Helper(remote)) if remote.id() == "home-a"));
}

#[tokio::test]
async fn history_stays_with_its_first_helper_while_it_is_away_or_kept_out() {
    let (a, b) = (stand(contract("a", ALL)).await, stand(contract("b", ALL)).await);
    let station = station(&[]);
    station.put("own-a", &a.url, HelperKind::Own);
    station.put("home-b", &b.url, HelperKind::Home);
    looks(&station, 1).await;
    assert_eq!(station.by(Ability::History).as_deref(), Some("own-a"));
    assert_eq!(station.by(Ability::Apps).as_deref(), Some("own-a"));

    a.set_away(true);
    looks(&station, 2).await;
    assert_eq!(station.by(Ability::History), None, "its data is there: never the next one");
    assert_eq!(station.by(Ability::Apps).as_deref(), Some("home-b"), "an ability that is not sticky goes on");

    a.set_away(false);
    looks(&station, 1).await;
    assert_eq!(station.by(Ability::History).as_deref(), Some("own-a"));
    station.helpers.set_away(Away::Home, None).unwrap();
    assert_eq!(station.by(Ability::History), None, "kept out by the regulator: nobody, not the next");
    assert_eq!(station.by(Ability::Apps).as_deref(), Some("home-b"));
}

#[tokio::test]
async fn only_at_home_keeps_my_server_and_the_clouds_out() {
    let (a, b) = (stand(contract("a", ALL)).await, stand(contract("b", ALL)).await);
    let station = station(&[]);
    // One's own server here at 127.0.0.1: its kind says «мой сервер», and the regulator reads the kind too.
    station.put("own-a", &a.url, HelperKind::Own);
    station.put("home-b", &b.url, HelperKind::Home);
    assert_eq!(reach_of(&station.helpers.entry("own-a").unwrap()), Reach::Mine);
    looks(&station, 1).await;
    assert_eq!(station.by(Ability::Hearing).as_deref(), Some("own-a"), "«Любые облака»");
    station.helpers.set_away(Away::Mine, None).unwrap();
    assert_eq!(station.by(Ability::Hearing).as_deref(), Some("own-a"), "«Дом и моё облако»: my server too");
    station.helpers.set_away(Away::Home, None).unwrap();
    assert_eq!(station.by(Ability::Hearing).as_deref(), Some("home-b"), "«Только дома»: home only");
    assert!(!station.helpers.lets(Reach::Mine) && station.helpers.lets(Reach::Home));
    assert_eq!(station.helpers.away(), Away::Home);

    // The aggregate counts what the regulator lets out only; let out again, my server counts once it is read anew.
    station.helpers.remove("home-b", None).unwrap();
    assert_eq!(station.helpers.state(), HomeServer::None, "my server alone, kept out: none to lean on");
    station.helpers.set_away(Away::Any, None).unwrap();
    assert_eq!(station.helpers.state(), HomeServer::Down, "not on what it showed before it was kept out");
    looks(&station, 1).await;
    assert_eq!(station.helpers.state(), HomeServer::Up);
}

#[tokio::test]
async fn an_outside_helper_the_regulator_keeps_out_is_not_looked_at() {
    let (own, home) = (stand(contract("o", ALL)).await, stand(contract("h", ALL)).await);
    let station = station_with(&[], false);
    station.put("own-o", &own.url, HelperKind::Own);
    looks(&station, 1).await;
    station.modes.switch(Some(&json!("own")), None).unwrap();
    assert_eq!(own.looks(), 1);

    station.helpers.set_away(Away::Home, None).unwrap();
    assert_eq!(station.mode(), (PanelMode::Gateway, Some(SHUT.to_string())), "every helper kept out: «Свой» goes");
    looks(&station, 2).await;
    assert_eq!(own.looks(), 1, "a blocked helper's look would leave the home all the same");
    let refused = station.helpers.check("own-o").await.unwrap_err();
    assert_eq!(refused.status, StatusCode::CONFLICT);
    assert!(refused.message.contains("не пускает этого Турбо"), "{}", refused.message);
    assert_eq!(own.looks(), 1, "not on request either");
    let view = station.helpers.views().into_iter().find(|view| view.id == "own-o").unwrap();
    assert!(!view.allowed && view.state == HelperState::Unknown, "{view:?}");

    // The helper at home beside it is looked at.
    station.put("home-h", &home.url, HelperKind::Home);
    looks(&station, 1).await;
    assert_eq!((own.looks(), home.looks()), (1, 1));
    assert_eq!(station.by(Ability::Hearing).as_deref(), Some("home-h"));
}

/// `/health` says of each helper whether the regulator lets it out, as the list does: the panels read it there and
/// never guess it from the kind — an entry «дома» by its kind at an address outside the home (the pair's door takes
/// any address) is kept out by «Только дома» all the same.
#[tokio::test]
async fn the_health_says_which_helpers_the_regulator_keeps_out() {
    let station = station(&[]);
    station.put(HOME_COMPUTER, "http://pc.example.com:7780", HelperKind::Home);
    station.put("home-h", "http://192.168.1.20:7780", HelperKind::Home);
    let allowed = || -> Vec<(String, Value)> {
        let health = station.helpers.health();
        let list = health["list"].as_array().unwrap().clone();
        list.iter().map(|brief| (brief["id"].as_str().unwrap().to_string(), brief["allowed"].clone())).collect()
    };
    let both = |door: bool| vec![(HOME_COMPUTER.to_string(), json!(door)), ("home-h".to_string(), json!(true))];
    assert_eq!(allowed(), both(true), "«Любые облака»");
    station.helpers.set_away(Away::Home, None).unwrap();
    assert_eq!(allowed(), both(false), "«Только дома»: by its address, whatever its kind");
    let views: Vec<bool> = station.helpers.views().iter().map(|view| view.allowed).collect();
    assert_eq!(views, [false, true], "the same as the list says");
}

/// The first decided aggregate counts at the start too, with no helper to look at: a station in «Свой» leaning on an
/// outside helper takes «Только дома» in its computer's package and starts again — it leaves «Свой» at its first
/// tick instead of staying deaf and mute in it.
#[tokio::test]
async fn a_station_started_in_svoi_with_every_helper_kept_out_leaves_it_at_the_first_tick() {
    let own = stand(contract("o", ALL)).await;
    let station = station_stored(&[], true, |settings, secrets| {
        let entry = HelperSetting {
            id: "own-o".into(),
            name: "Мой сервер".into(),
            url: own.url.clone(),
            kind: HelperKind::Own,
            enabled: true,
            without: Vec::new(),
            helper_id: None,
        };
        settings.set(&HELPERS, &vec![entry], None).unwrap();
        settings.set(&AWAY_SETTING, &Away::Home, None).unwrap();
        secrets.set(&key_secret("own-o"), Some(KEY)).unwrap();
    });
    assert_eq!((station.modes.current(), station.helpers.away()), (PanelMode::Own, Away::Home));
    looks(&station, 1).await;
    assert_eq!(station.mode(), (PanelMode::Gateway, Some(SHUT.to_string())));
    assert_eq!(own.looks(), 0, "kept out: not looked at for it");
}

#[tokio::test]
async fn http_and_home_are_only_for_home_addresses() {
    let station = station(&[]);
    let add = |url: &str, kind: Option<HelperKind>| AddInput {
        url: url.into(),
        key: KEY.into(),
        kind,
        ..AddInput::default()
    };
    // Refused before any look: nothing goes over the internet in clear.
    let plain = station.helpers.add(add("http://198.51.100.7:7780", None), None).await.unwrap_err();
    assert_eq!(plain.status, StatusCode::BAD_REQUEST);
    assert!(plain.message.contains("только в домашней сети"), "{}", plain.message);
    let home = station.helpers.add(add("https://helper.example.com", Some(HelperKind::Home)), None).await.unwrap_err();
    assert_eq!(home.status, StatusCode::BAD_REQUEST);
    assert!(home.message.contains("не может быть «дома»"), "{}", home.message);
    let bad = station.helpers.add(add("ftp://192.168.1.20", None), None).await.unwrap_err();
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
    let keyless = station
        .helpers
        .add(AddInput { url: "http://127.0.0.1:9".into(), ..AddInput::default() }, None)
        .await
        .unwrap_err();
    assert!(keyless.message.contains("ключ подключения"), "{}", keyless.message);
    assert!(station.helpers.list().is_empty());

    let entry = |url: &str, kind| HelperSetting {
        id: "x".into(),
        name: "x".into(),
        url: url.into(),
        kind,
        enabled: true,
        without: Vec::new(),
        helper_id: None,
    };
    assert_eq!(reach_of(&entry("http://127.0.0.1:7780", HelperKind::Own)), Reach::Mine, "its kind's reach");
    assert_eq!(reach_of(&entry("https://helper.example.com", HelperKind::Home)), Reach::Mine, "its address's");
    assert_eq!(reach_of(&entry("http://192.168.1.20:7780", HelperKind::Home)), Reach::Home);
    assert_eq!(reach_of(&entry("https://cloud.example.com", HelperKind::Cloud)), Reach::Mine);
}

#[tokio::test]
async fn the_helpers_go_svoi_gives_way_they_come_back_and_svoi_follows_after_a_minute() {
    let computer = stand(contract("pc", ALL)).await;
    let station = station(&[("HOME_SERVER_URL", computer.url.as_str()), ("HOME_SERVER_KEY", KEY)]);
    let door = station.helpers.entry(HOME_COMPUTER).expect("the station's .env, the first helper");
    assert_eq!(door.url, computer.url);

    station.helpers.check(HOME_COMPUTER).await.unwrap();
    assert_eq!(station.helpers.state(), HomeServer::Up);
    assert_eq!(
        (station.by(Ability::Hearing).as_deref(), station.by(Ability::Voice).as_deref()),
        (Some(HOME_COMPUTER), Some(HOME_COMPUTER))
    );
    station.modes.switch(Some(&json!("own")), None).unwrap();

    computer.set_away(true);
    station.helpers.check(HOME_COMPUTER).await.unwrap();
    assert_eq!(station.helpers.state(), HomeServer::Up, "one miss is not away yet");
    station.helpers.check(HOME_COMPUTER).await.unwrap();
    assert_eq!(station.helpers.state(), HomeServer::Down);
    assert_eq!(station.by(Ability::Voice), None);
    assert_eq!(station.mode(), (PanelMode::Gateway, Some(AWAY.to_string())));

    computer.set_away(false);
    station.helpers.check(HOME_COMPUTER).await.unwrap();
    assert_eq!(
        (station.helpers.state(), station.modes.current()),
        (HomeServer::Up, PanelMode::Gateway),
        "not before a minute"
    );
    station.helpers.age_up(RETURN_AFTER);
    station.helpers.check(HOME_COMPUTER).await.unwrap();
    assert_eq!(station.modes.current(), PanelMode::Own, "back, the switch was its own");
    assert_eq!(station.modes.state().auto, None);

    // A key the helper no longer takes is said so.
    station.helpers.door(DoorInput { key: Some("vgw_revoked".into()), ..DoorInput::default() }, None).unwrap();
    station.helpers.check(HOME_COMPUTER).await.unwrap();
    assert_eq!(station.helpers.state(), HomeServer::Down);
    let view = station.helpers.views().remove(0);
    assert!(view.error.unwrap_or_default().contains("ключ"), "the key words");
    station.helpers.close_door(None).unwrap();
    assert_eq!(station.helpers.state(), HomeServer::None);
}

#[tokio::test]
async fn a_quick_refusal_does_not_leave_svoi_while_another_helper_is_still_looked_at() {
    let (a, b) = (stand(contract("a", ALL)).await, stand(contract("b", ALL)).await);
    a.set_away(true);
    *b.delay.lock().unwrap() = Duration::from_millis(300);
    // A station started in «Свой», its helpers not looked at yet.
    let station = station_with(&[], true);
    station.put("home-a", &a.url, HelperKind::Home);
    station.put("home-b", &b.url, HelperKind::Home);
    assert_eq!(station.modes.current(), PanelMode::Own);

    station.helpers.check("home-a").await.unwrap();
    assert_eq!(station.helpers.state(), HomeServer::Down, "A refused at once");
    assert_eq!(station.modes.current(), PanelMode::Own, "B is still to be looked at: nothing decided");
    station.helpers.check("home-b").await.unwrap();
    assert_eq!((station.helpers.state(), station.modes.current()), (HomeServer::Up, PanelMode::Own));

    // The tick looks at both at once and decides once, after both.
    looks(&station, 1).await;
    assert_eq!(station.modes.current(), PanelMode::Own);

    b.set_away(true);
    looks(&station, 2).await;
    assert_eq!(station.mode(), (PanelMode::Gateway, Some(AWAY.to_string())), "both away: «Свой» goes");
}

#[tokio::test]
async fn svoi_gives_way_while_no_helper_gives_hearing_or_voice_and_comes_back_with_it() {
    let computer = stand(contract("pc", ALL)).await;
    let station = station(&[("HOME_SERVER_URL", computer.url.as_str()), ("HOME_SERVER_KEY", KEY)]);
    station.helpers.check(HOME_COMPUTER).await.unwrap();
    station.helpers.age_up(RETURN_AFTER);
    station.modes.switch(Some(&json!("own")), None).unwrap();
    let door = |without: &[&str]| {
        let without = without.iter().map(|part| part.to_string()).collect();
        station.helpers.door(DoorInput { without: Some(without), ..DoorInput::default() }, None).unwrap();
    };

    // «Слух» off, and the station's recognition is the build's local default: no cloud to hear.
    door(&["asr"]);
    assert_eq!(station.mode(), (PanelMode::Gateway, Some(NO_HEARING.to_string())));
    station.helpers.check(HOME_COMPUTER).await.unwrap();
    assert_eq!(station.modes.current(), PanelMode::Gateway, "not back while it would be deaf");
    // Given again: back at once, the helper has answered for a minute.
    door(&[]);
    assert_eq!(station.modes.current(), PanelMode::Own);

    // «Голос» off: mute without a cloud voice — the same way.
    door(&["tts"]);
    assert_eq!(station.mode().1.as_deref(), Some(NO_VOICE));
    // A person switches to «Свой» all the same (the switch asks only whether a helper is up): the next look gives way.
    station.modes.switch(Some(&json!("own")), None).unwrap();
    station.helpers.check(HOME_COMPUTER).await.unwrap();
    assert_eq!(station.mode(), (PanelMode::Gateway, Some(NO_VOICE.to_string())));
    // The voice given back, only «Модель» off: hearing and voice are there, «Свой» is back.
    door(&["llm"]);
    assert_eq!((station.modes.current(), station.helpers.svoi_lacks()), (PanelMode::Own, None));

    // A second helper on the line without hearing while the one that hears is away: «Свой» is deaf — it goes.
    let deaf: Vec<(&str, bool)> = ALL.iter().copied().filter(|(id, _)| *id != "hearing").collect();
    let other = stand(contract("b", &deaf)).await;
    station.put("home-b", &other.url, HelperKind::Home);
    station.helpers.check("home-b").await.unwrap();
    computer.set_away(true);
    station.helpers.check(HOME_COMPUTER).await.unwrap();
    station.helpers.check(HOME_COMPUTER).await.unwrap();
    assert_eq!(station.helpers.state(), HomeServer::Up, "the other answers");
    assert_eq!(station.helpers.svoi_lacks(), Some(AWAY));
    assert_eq!(station.mode(), (PanelMode::Gateway, Some(AWAY.to_string())));
}

#[tokio::test]
async fn svoi_stays_without_the_helpers_hearing_when_an_allowed_cloud_hears_and_a_computer_is_never_asked() {
    let computer = stand(contract("pc", ALL)).await;
    let cloud = [("ASR_BASE_URL", "https://api.groq.com/openai/v1")];
    let env = [("HOME_SERVER_URL", computer.url.as_str()), ("HOME_SERVER_KEY", KEY), cloud[0]];
    let station = station(&env);
    station.helpers.check(HOME_COMPUTER).await.unwrap();
    station.modes.switch(Some(&json!("own")), None).unwrap();
    let door = |station: &Station, without: &[&str]| {
        let without = without.iter().map(|part| part.to_string()).collect();
        station.helpers.door(DoorInput { without: Some(without), ..DoorInput::default() }, None).unwrap();
    };
    door(&station, &["asr"]);
    assert_eq!((station.modes.current(), station.helpers.svoi_lacks()), (PanelMode::Own, None), "the cloud hears");
    // «Только дома» keeps that cloud out: deaf, «Свой» goes.
    station.helpers.set_away(Away::Home, None).unwrap();
    assert_eq!(station.mode(), (PanelMode::Gateway, Some(NO_HEARING.to_string())));
    station.helpers.set_away(Away::Any, None).unwrap();
    station.modes.switch(Some(&json!("own")), None).unwrap();
    door(&station, &["asr", "tts"]);
    assert_eq!(
        (station.modes.current(), station.helpers.svoi_lacks()),
        (PanelMode::Gateway, Some(NO_VOICE)),
        "no cloud voice"
    );

    // A computer's gateway hears and speaks itself: its helpers never send «Свой» away.
    let computer_env = [("HOME_SERVER_URL", computer.url.as_str()), ("HOME_SERVER_KEY", KEY), ("DUOWAVE_ROLE", "pc")];
    let pc = super::tests::station(&computer_env);
    pc.helpers.check(HOME_COMPUTER).await.unwrap();
    pc.modes.switch(Some(&json!("own")), None).unwrap();
    door(&pc, &["asr", "tts"]);
    computer.set_away(true);
    pc.helpers.check(HOME_COMPUTER).await.unwrap();
    pc.helpers.check(HOME_COMPUTER).await.unwrap();
    assert_eq!((pc.modes.current(), pc.helpers.svoi_lacks()), (PanelMode::Own, None));
}

/// Item 48: the home's computer under the default name a station wrote before it knew the household's language
/// («Home computer») takes the name the computer gives itself at the first look; a name the owner gave stays.
#[tokio::test]
async fn the_home_computer_under_its_default_name_takes_the_name_it_gives_itself() {
    let station = station(&[]);
    let computer = stand(contract("mac-1", ALL)).await;
    station.put(HOME_COMPUTER, &computer.url, HelperKind::Home);
    station
        .helpers
        .write(None, |list| {
            list.iter_mut().find(|entry| entry.id == HOME_COMPUTER).unwrap().name = "Home computer".into();
            Ok(Some(()))
        })
        .unwrap();
    station.helpers.check(HOME_COMPUTER).await.unwrap();
    let entry = station.helpers.entry(HOME_COMPUTER).unwrap();
    assert_eq!((entry.name.as_str(), entry.helper_id.as_deref()), ("Турбо mac-1", Some("mac-1")));

    // A name of the owner's own is never replaced.
    station
        .helpers
        .write(None, |list| {
            list.iter_mut().find(|entry| entry.id == HOME_COMPUTER).unwrap().name = "Кабинет".into();
            Ok(Some(()))
        })
        .unwrap();
    station.helpers.check(HOME_COMPUTER).await.unwrap();
    assert_eq!(station.helpers.entry(HOME_COMPUTER).unwrap().name, "Кабинет");
}

#[tokio::test]
async fn a_gateway_knows_itself_and_never_looks_at_itself() {
    let station = station(&[]);
    let me = station.identity.gateway().unwrap().id;
    let itself = stand(contract(&me, ALL)).await;
    station.put("home-me", &itself.url, HelperKind::Home);
    looks(&station, 1).await;
    assert_eq!(itself.looks(), 1);
    let entry = station.helpers.entry("home-me").unwrap();
    assert_eq!(entry.helper_id.as_deref(), Some(me.as_str()), "kept in the entry: it travels with the list");
    assert!(station.helpers.is_self(&entry));
    assert_eq!(station.helpers.views()[0].state, HelperState::Itself);
    assert_eq!(station.by(Ability::Hearing), None, "never its own helper");
    assert_eq!(station.helpers.state(), HomeServer::None);
    looks(&station, 2).await;
    station.helpers.check("home-me").await.unwrap();
    assert_eq!(itself.looks(), 1, "never looked at again");
}

/// A computer's own parts (its built-in helper, stage 6) that only count the looks at them.
#[derive(Default)]
struct Looked(AtomicUsize);

impl OwnParts for Looked {
    fn contract(&self) -> Contract {
        Contract::new(HelperIdentity::default(), Vec::new())
    }
    fn gives_here(&self, _ability: Ability) -> bool {
        false
    }
    fn recognizer(&self) -> Option<Arc<dyn SpeechRecognizer>> {
        None
    }
    fn speech(&self) -> Option<Arc<OwnSpeech>> {
        None
    }
    fn models(&self) -> Vec<OwnModel> {
        Vec::new()
    }
    fn is_me(&self, _url: &str) -> bool {
        false
    }
    fn check(&self) -> BoxFuture<'_, ()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {})
    }
}

/// A computer's tick looks at its own parts too — its model servers (stage 6): a closed Ollama is seen without anyone
/// asking —, at once and then at every tick.
#[tokio::test(start_paused = true)]
async fn a_computers_tick_looks_at_its_own_parts_at_once_and_at_every_tick() {
    let config = Arc::new(Config::for_tests(&Env::from_pairs([("STATE_DIR", "")])));
    let db = Arc::new(Db::memory());
    let events = EventBus::new();
    let settings = Arc::new(SettingsService::new(db.clone(), events.clone()));
    let secrets = Arc::new(SecretsService::new(db.clone(), None).unwrap());
    let profile = Profile::for_role(None);
    let identity =
        Arc::new(Identity::new(settings.clone(), secrets.clone(), db, DeviceKind::of(profile), events.clone()));
    identity.ensure().unwrap();
    let helpers = Helpers::new(config, settings, secrets, events, profile, identity);
    let own = Arc::new(Looked::default());
    helpers.set_own(own.clone());
    helpers.start();
    tokio::time::sleep(Duration::from_millis(1)).await;
    assert_eq!(own.0.load(Ordering::SeqCst), 1, "at once");
    tokio::time::sleep(CHECK_EVERY).await;
    assert_eq!(own.0.load(Ordering::SeqCst), 2, "at the next tick");
    tokio::time::sleep(CHECK_EVERY).await;
    assert_eq!(own.0.load(Ordering::SeqCst), 3);
    helpers.stop();
}

#[tokio::test]
async fn the_same_helper_twice_is_taken_once() {
    let computer = stand(contract("pc", ALL)).await;
    let station = station(&[]);
    station.put("home-a", &computer.url, HelperKind::Home);
    station.put("home-b", &format!("http://localhost:{}", computer.port), HelperKind::Home);
    looks(&station, 1).await;
    assert_eq!(computer.looks(), 2, "both looked at once, before either is known");
    let views = station.helpers.views();
    assert_eq!(views[1].state, HelperState::Twin);
    assert_eq!(views[1].error.as_deref(), Some("то же Турбо, что «Турбо home-a»"));
    assert!(station.helpers.is_twin(&station.helpers.entry("home-b").unwrap()));
    let givers = station.helpers.pick_all(Ability::Hearing);
    assert_eq!(givers.len(), 1, "taken once");
    looks(&station, 1).await;
    assert_eq!(computer.looks(), 3, "the second never looked at again");
    station.helpers.check("home-b").await.unwrap();
    assert_eq!(computer.looks(), 3);
}

/// The same helper by another road gives while the earlier entry cannot (review): a computer first at its tunnel, then
/// at home — kept out by «Только дома», switched off, keyless or away, the earlier one stands for nothing and the one
/// at home gives; on the line again, the earlier one is the one again.
#[tokio::test]
async fn the_same_helper_by_another_road_gives_while_the_earlier_entry_cannot() {
    // Two roads to one computer: both answer as «pc».
    let (tunnel, lan) = (stand(contract("pc", ALL)).await, stand(contract("pc", ALL)).await);
    let station = station(&[]);
    // «Мой сервер» by its kind: the regulator reads it as a road out of the home.
    station.put("own-t", &tunnel.url, HelperKind::Own);
    station.put("home-l", &lan.url, HelperKind::Home);
    looks(&station, 1).await;
    assert_eq!(station.by(Ability::Hearing).as_deref(), Some("own-t"));
    assert_eq!(station.helpers.views()[1].state, HelperState::Twin);

    // Kept out by «Только дома»: the one at home gives, and is looked at.
    station.helpers.set_away(Away::Home, None).unwrap();
    assert_eq!(station.by(Ability::Hearing).as_deref(), Some("home-l"));
    looks(&station, 1).await;
    assert_eq!((tunnel.looks(), lan.looks()), (1, 2));
    assert_eq!(station.helpers.views()[1].state, HelperState::Up);
    // Let out again: the one at home until the earlier one is read anew, then the earlier one.
    station.helpers.set_away(Away::Any, None).unwrap();
    assert_eq!(station.by(Ability::Hearing).as_deref(), Some("home-l"));
    looks(&station, 1).await;
    assert_eq!(station.by(Ability::Hearing).as_deref(), Some("own-t"));

    // Switched off, or without its key: the same.
    station.change("own-t", Change { enabled: Some(false), ..Change::default() });
    assert_eq!(station.by(Ability::Hearing).as_deref(), Some("home-l"));
    station.change("own-t", Change { enabled: Some(true), ..Change::default() });
    looks(&station, 1).await;
    assert_eq!(station.by(Ability::Hearing).as_deref(), Some("own-t"));
    station.secrets.delete(&key_secret("own-t")).unwrap();
    assert_eq!(station.by(Ability::Hearing).as_deref(), Some("home-l"));
    station.secrets.set(&key_secret("own-t"), Some(KEY)).unwrap();

    // Away: two misses, and the one at home is looked at in the same tick.
    tunnel.set_away(true);
    looks(&station, 1).await;
    assert_eq!(station.by(Ability::Hearing).as_deref(), Some("own-t"), "one miss is not away yet");
    let before = lan.looks();
    looks(&station, 1).await;
    assert_eq!(lan.looks(), before + 1, "freed and looked at in the same tick");
    assert_eq!(station.by(Ability::Hearing).as_deref(), Some("home-l"));
}

/// An entry the regulator keeps out is not looked at, so it forgets what it showed (review): the pair's old view and
/// the shell say «down» while it is kept out, and let out again it gives only after a new look, never on what it
/// showed before.
#[tokio::test]
async fn an_entry_the_regulator_keeps_out_forgets_what_it_showed() {
    let computer = stand(contract("pc", ALL)).await;
    let station = station(&[]);
    // The pair's door on a road the regulator reads as out of the home (the computer's tunnel, say).
    station.put(HOME_COMPUTER, &computer.url, HelperKind::Own);
    let home = crate::home_server::HomeServerService::over(station.helpers.clone());
    looks(&station, 1).await;
    assert_eq!((station.helpers.pair_view().unwrap()["state"].clone(), home.state()), (json!("up"), HomeServer::Up));

    station.helpers.set_away(Away::Home, None).unwrap();
    let old = station.helpers.pair_view().unwrap();
    assert_eq!((old["state"].clone(), old["since"].clone()), (json!("down"), Value::Null), "{old}");
    assert_eq!((home.state(), home.status().since), (HomeServer::Down, None));

    // It goes while kept out; let out again, nothing is taken from it on what it showed before.
    computer.set_away(true);
    station.helpers.set_away(Away::Any, None).unwrap();
    assert_eq!(station.by(Ability::Hearing), None, "not read anew yet");
    looks(&station, 1).await;
    assert_eq!((station.by(Ability::Hearing), station.helpers.views()[0].state), (None, HelperState::Down));
}

/// A twin keeps no watch (review): it is not looked at, so what it showed before it was a twin is not now. Freed, it
/// gives only after a look of its own, which decides at once: both roads to a sleeping computer dead, «Свой» goes at
/// the tick the first road goes, not one later; the first road switched off, nothing is taken from the second on a
/// look from before.
#[tokio::test]
async fn a_freed_twin_gives_only_after_a_look_of_its_own_which_decides_at_once() {
    // Two roads to one computer: at home first, its tunnel second.
    let (lan, tunnel) = (stand(contract("pc", ALL)).await, stand(contract("pc", ALL)).await);
    let station = station(&[]);
    station.put(HOME_COMPUTER, &lan.url, HelperKind::Home);
    station.put("own-t", &tunnel.url, HelperKind::Own);
    looks(&station, 2).await;
    assert_eq!((station.helpers.views()[1].state, tunnel.looks()), (HelperState::Twin, 1), "once, before it was known");
    station.helpers.age_up(RETURN_AFTER);
    station.modes.switch(Some(&json!("own")), None).unwrap();

    // The computer sleeps: both roads are dead.
    lan.set_away(true);
    tunnel.set_away(true);
    looks(&station, 1).await;
    assert_eq!(station.modes.current(), PanelMode::Own, "one miss is not away yet");
    looks(&station, 1).await;
    assert_eq!(station.helpers.state(), HomeServer::Down, "the twin freed in this tick: its first look decides");
    assert_eq!(station.by(Ability::Hearing), None);
    assert_eq!(station.mode(), (PanelMode::Gateway, Some(AWAY.to_string())));
    assert_eq!(station.helpers.views()[1].state, HelperState::Down);

    // Awake again, a twin again; then its road dies and the first road is switched off.
    lan.set_away(false);
    tunnel.set_away(false);
    looks(&station, 2).await;
    assert_eq!(station.helpers.views()[1].state, HelperState::Twin);
    tunnel.set_away(true);
    station.change(HOME_COMPUTER, Change { enabled: Some(false), ..Change::default() });
    assert_eq!(station.by(Ability::Hearing), None, "not on what it showed before it was a twin");
    looks(&station, 1).await;
    assert_eq!((station.by(Ability::Hearing), station.helpers.views()[1].state), (None, HelperState::Down));
}

/// An entry added while the regulator keeps it out keeps what it declares, not the watch (review): the add's look is
/// the person's, and weeks later, let out, the entry gives only after a new look — not on the add's.
#[tokio::test]
async fn an_entry_added_while_kept_out_gives_only_after_a_new_look_once_let_out() {
    let server = stand(contract("srv", ALL)).await;
    let station = station(&[]);
    station.helpers.set_away(Away::Home, None).unwrap();
    // One's own server, here at 127.0.0.1: «мой сервер» by its kind.
    let add = AddInput { url: server.url.clone(), key: KEY.into(), kind: Some(HelperKind::Own), ..AddInput::default() };
    station.helpers.add(add, None).await.unwrap();
    assert_eq!(station.helpers.state(), HomeServer::None, "kept out");
    assert_eq!(station.helpers.views()[0].abilities.len(), ALL.len(), "what it declares is shown");

    // The server is gone since; the regulator opens.
    server.set_away(true);
    station.helpers.set_away(Away::Any, None).unwrap();
    assert_eq!((station.helpers.state(), station.by(Ability::Hearing)), (HomeServer::Down, None), "not on the add's");
    looks(&station, 1).await;
    assert_eq!(station.helpers.views()[0].state, HelperState::Down);
}

/// A look the regulator closes on while it is out is forgotten (review): its outcome comes back after the regulator
/// forgot the watch, so nothing follows it — no «на связи», no contacts read, the pair's old view says «down» — and
/// let out again, the entry gives only after a new look.
#[tokio::test]
async fn a_look_the_regulator_closes_on_is_forgotten_and_nothing_follows_it() {
    let computer = stand(contract("pc", ALL)).await;
    let station = station(&[]);
    // The pair's door on a road the regulator reads as out of the home (the computer's tunnel, say).
    station.put(HOME_COMPUTER, &computer.url, HelperKind::Own);
    computer.hold.store(true, Ordering::SeqCst);
    let mut heard = station.events.subscribe();
    tokio::join!(station.helpers.check_all(), async {
        computer.arrived.notified().await;
        station.helpers.set_away(Away::Home, None).unwrap();
        computer.release.notify_one();
    });
    assert_eq!(computer.looks(), 1, "the look was out");
    let old = station.helpers.pair_view().unwrap();
    assert_eq!((old["state"].clone(), old["since"].clone()), (json!("down"), Value::Null), "{old}");
    let remote = station.helpers.remote(&station.helpers.entry(HOME_COMPUTER).unwrap()).unwrap();
    assert_eq!(remote.messenger(), remote::Messenger::Unknown, "its bot's contacts not read");
    let mut said = Vec::new();
    while let Ok(event) = heard.try_recv() {
        said.push(event.name());
    }
    assert!(!said.contains(&"helpers.up") && !said.contains(&"homeServer.up"), "{said:?}");

    computer.hold.store(false, Ordering::SeqCst);
    station.helpers.set_away(Away::Any, None).unwrap();
    assert_eq!((station.helpers.state(), station.by(Ability::Hearing)), (HomeServer::Down, None));
    looks(&station, 1).await;
    assert_eq!(station.by(Ability::Hearing).as_deref(), Some(HOME_COMPUTER));
}

/// The pair's door that is a twin is on the line in its old views (review): it keeps no watch of its own, and the
/// same computer answers by the earlier entry.
#[tokio::test]
async fn the_pair_door_as_a_twin_is_on_the_line_in_its_old_views() {
    let (tunnel, lan) = (stand(contract("pc", ALL)).await, stand(contract("pc", ALL)).await);
    let station = station(&[]);
    station.put("own-t", &tunnel.url, HelperKind::Own);
    station.put(HOME_COMPUTER, &lan.url, HelperKind::Home);
    let home = crate::home_server::HomeServerService::over(station.helpers.clone());
    looks(&station, 2).await;
    assert_eq!(station.helpers.views()[1].state, HelperState::Twin);
    assert_eq!((station.helpers.pair_view().unwrap()["state"].clone(), home.state()), (json!("up"), HomeServer::Up));
}

/// A list or a regulator that does not read is no empty list and no «Любые облака» (review): the snapshot keeps its
/// last read, a gateway started while they do not read has no helper and lets nothing out of the home, no change
/// writes over the stored list — and the next tick reads them.
#[tokio::test]
async fn a_list_that_does_not_read_is_never_written_over_and_is_read_again_at_the_next_tick() {
    let (a, b) = (stand(contract("a", ALL)).await, stand(contract("b", ALL)).await);
    let station = station(&[]);
    station.put("home-a", &a.url, HelperKind::Home);
    station.put("home-b", &b.url, HelperKind::Home);
    station.helpers.set_away(Away::Mine, None).unwrap();
    // Their rows no longer read (an I/O error of a small disk, say); writing still works.
    let unreadable = |broken: bool| {
        let flag = if broken { "'unreadable'" } else { "0" };
        let sql = format!("UPDATE settings SET is_default = {flag} WHERE key IN ('helpers', 'away')");
        station.db.with(|c| c.execute(&sql, [])).unwrap();
    };
    unreadable(true);
    assert!(station.helpers.reread().is_none(), "the last read stays");
    assert_eq!((station.helpers.list().len(), station.helpers.away()), (2, Away::Mine));

    let profile = Profile::for_role(station.config.placement.role.as_deref());
    let started = Helpers::new(
        station.config.clone(),
        station.settings.clone(),
        station.secrets.clone(),
        station.events.clone(),
        profile,
        station.identity.clone(),
    );
    assert_eq!((started.list().len(), started.away()), (0, Away::Home), "no helper, nothing out of the home");
    let door = DoorInput { url: Some("http://192.0.2.10:7780".into()), key: Some(KEY.into()), ..DoorInput::default() };
    assert!(started.door(door, None).is_err(), "nothing written from a list that did not read");

    unreadable(false);
    let ids: Vec<String> = station.settings.get(&HELPERS, None).unwrap().into_iter().map(|entry| entry.id).collect();
    assert_eq!(ids, ["home-a", "home-b"], "the stored list as it was");
    started.check_all().await;
    assert_eq!((started.list().len(), started.away()), (2, Away::Mine), "read again at the next tick");
    assert_eq!((a.looks(), b.looks()), (1, 1), "and looked at");
    started.stop();
}

#[tokio::test]
async fn adding_checks_the_contract_first_and_stores_nothing_on_a_wrong_key_or_itself() {
    let computer = stand(contract("pc", ALL)).await;
    let station = station(&[]);
    let input = |key: &str| AddInput { url: computer.url.clone(), key: key.into(), ..AddInput::default() };
    let wrong = station.helpers.add(input("vgw_wrong"), None).await.unwrap_err();
    assert_eq!(wrong.status, StatusCode::BAD_GATEWAY);
    assert!(wrong.message.contains("не узнал ключ"), "{}", wrong.message);
    assert!(station.helpers.list().is_empty() && station.secrets.ids("helper:").is_empty(), "nothing stored");

    let id = station.helpers.add(input(KEY), None).await.unwrap();
    let entry = station.helpers.entry(&id).unwrap();
    assert!(id.starts_with("home-") && entry.kind == HelperKind::Home, "a home address: «дома» — {id}");
    assert_eq!((entry.name.as_str(), entry.helper_id.as_deref()), ("Турбо pc", Some("pc")), "named as it names itself");
    assert_eq!(station.helpers.key(&id).as_deref(), Some(KEY));
    assert_eq!(station.by(Ability::Hearing).as_deref(), Some(id.as_str()), "on the line at once, no second look");
    assert_eq!(computer.looks(), 1);

    let again = station.helpers.add(input(KEY), None).await.unwrap_err();
    assert_eq!(again.status, StatusCode::CONFLICT);
    assert!(again.message.contains("уже в списке"), "{}", again.message);

    let me = station.identity.gateway().unwrap().id;
    let itself = stand(contract(&me, ALL)).await;
    let refused = station
        .helpers
        .add(AddInput { url: itself.url.clone(), key: KEY.into(), ..AddInput::default() }, None)
        .await
        .unwrap_err();
    assert_eq!(refused.status, StatusCode::CONFLICT);
    assert!(refused.message.contains("сам этот хаб"), "{}", refused.message);
    assert_eq!(station.helpers.list().len(), 1);

    // Nothing answering is no helper either; a dry run stores nothing.
    let page = stand(json!({ "status": "ok" })).await;
    let not_helper =
        station.helpers.add(AddInput { url: page.url.clone(), key: KEY.into(), ..AddInput::default() }, None).await;
    assert!(not_helper.unwrap_err().message.contains("не Турбо DuoWave"));
    let other = stand(contract("other", ALL)).await;
    let draft =
        AddInput {
            url: other.url.clone(), key: KEY.into(), name: Some(" Кабинет ".into()), ..AddInput::default()
        };
    let view = station.helpers.probe(&draft).await.unwrap();
    assert_eq!((view.name.as_str(), view.state, view.contract), ("Кабинет", HelperState::Up, Some(1)));
    assert_eq!(view.abilities.len(), 7);
    assert_eq!(station.helpers.list().len(), 1, "nothing stored");
}

#[tokio::test]
async fn the_order_must_name_every_helper_once_and_route_words_are_no_ids() {
    let station = station(&[]);
    station.put("home-a", "http://192.168.1.20:7780", HelperKind::Home);
    station.put("home-b", "http://192.0.2.21:7780", HelperKind::Home);
    let order = |ids: &[&str]| station.helpers.order(&ids.iter().map(|id| id.to_string()).collect::<Vec<_>>(), None);
    for wrong in [&["home-a"][..], &["home-a", "home-b", "home-c"], &["home-a", "home-a"], &["home-b", "self"]] {
        assert_eq!(order(wrong).unwrap_err().status, StatusCode::BAD_REQUEST, "{wrong:?}");
    }
    order(&["home-b", "home-a"]).unwrap();
    let ids: Vec<String> = station.helpers.list().into_iter().map(|entry| entry.id).collect();
    assert_eq!(ids, ["home-b", "home-a"]);
    for word in RESERVED {
        assert!(!valid_id(word), "{word}");
    }
    assert!(valid_id("home-computer") && valid_id("own-3fa91c07"));
    assert!(!valid_id("Home") && !valid_id("") && !valid_id(&"a".repeat(41)) && !valid_id("a b"));
    // A stored list read leniently: a broken item, a bad id, a repeated one and an unknown ability passed by.
    let read = parse_helpers(&json!([
        { "id": "home-a", "name": "A", "url": "http://192.168.1.20", "kind": "home", "without": ["voice", "gpu"] },
        { "id": "order", "name": "B", "url": "http://192.0.2.21", "kind": "home" },
        { "id": "home-a", "name": "C", "url": "http://192.0.2.22", "kind": "home" },
        { "id": "home-d", "url": "http://192.0.2.23" },
    ]))
    .unwrap();
    assert_eq!(read.len(), 1);
    assert_eq!((read[0].without.clone(), read[0].enabled), (vec![Ability::Voice], true));
}

#[tokio::test]
async fn every_look_names_this_gateway_and_its_home_to_the_helper() {
    let computer = stand(contract("pc", ALL)).await;
    let station = station(&[]);
    let me = station.identity.gateway().unwrap().id;
    // Stored as a newer writer might: a character that turns the text around in it.
    let named = GatewaySelf { id: me.clone(), name: Some("Колонка\u{202E} Лайт 2".into()) };
    station.settings.set(&GATEWAY_SELF, &named, None).unwrap();
    station.put("home-a", &computer.url, HelperKind::Home);
    looks(&station, 1).await;
    let headers = computer.headers.lock().unwrap().clone().expect("looked at");
    let header = |name: &str| String::from_utf8(headers.get(name).unwrap().as_bytes().to_vec()).unwrap();
    assert_eq!(header("x-duowave-gateway"), me);
    assert_eq!(header("x-duowave-gateway-name"), "Колонка Лайт 2", "raw UTF-8, the turning character out");
    assert_eq!(header("x-duowave-home"), station.identity.home().unwrap().id);
}

/// A helper's `/home/v1` never sends anywhere else: a redirect is its answer — the look says so, its voice fails — and
/// nothing it names is visited. A helper outside the home could otherwise point the station at a device of its home,
/// or have the household's words posted to another host.
#[tokio::test]
async fn a_helper_that_sends_elsewhere_is_not_followed() {
    let visited = Arc::new(AtomicUsize::new(0));
    let seen = visited.clone();
    let elsewhere = Router::new().fallback(move || {
        let seen = seen.clone();
        async move {
            seen.fetch_add(1, Ordering::SeqCst);
            axum::Json(contract("elsewhere", ALL))
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, elsewhere).await.unwrap() });
    let sent = |status: StatusCode, to: String| move || async move { (status, [(axum::http::header::LOCATION, to)]) };
    let sending = Router::new()
        .route("/home/v1/status", get(sent(StatusCode::FOUND, format!("{target}/home/v1/status"))))
        .route("/home/v1/messenger/contacts", get(sent(StatusCode::FOUND, format!("{target}/contacts"))))
        .route("/home/v1/tts/audio/speech", post(sent(StatusCode::TEMPORARY_REDIRECT, format!("{target}/speech"))));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, sending).await.unwrap() });

    let station = station(&[]);
    station.put("home-s", &url, HelperKind::Home);
    let remote = station.helpers.remote(&station.helpers.entry("home-s").unwrap()).unwrap();
    assert_eq!(remote.look().await.unwrap_err(), "Турбо ответило 302");
    remote.read_contacts().await;
    assert_eq!(remote.messenger(), remote::Messenger::Unknown);
    let spoken = remote.speech().synthesize("Слова дома", &tokio_util::sync::CancellationToken::new()).await;
    assert!(spoken.is_err(), "its voice answered with somewhere else");
    assert_eq!(visited.load(Ordering::SeqCst), 0, "nothing it named was visited");
}

#[tokio::test]
async fn a_home_computer_without_its_key_shows_no_old_view() {
    let station = station(&[]);
    station.put(HOME_COMPUTER, "http://192.168.1.20:7780", HelperKind::Home);
    assert!(station.helpers.pair_view().is_some());
    station.secrets.delete(&key_secret(HOME_COMPUTER)).unwrap();
    assert_eq!(station.helpers.pair_view(), None, "known, not configured: the computer brings a new key");
    let view = station.helpers.views().remove(0);
    assert_eq!((view.state, view.error.as_deref()), (HelperState::Unknown, Some("нет ключа Турбо")));
}

#[tokio::test]
async fn the_list_is_read_once_and_follows_its_changes() {
    let computer = stand(contract("pc", ALL)).await;
    let station = station(&[]);
    let id = station
        .helpers
        .add(AddInput { url: computer.url.clone(), key: KEY.into(), ..AddInput::default() }, None)
        .await
        .unwrap();
    station.db.with(|c| c.execute("DELETE FROM settings WHERE key = 'helpers'", [])).unwrap();
    assert_eq!(station.helpers.list().len(), 1, "not read again behind its back");
    assert_eq!(station.by(Ability::Hearing).as_deref(), Some(id.as_str()));
    // Another writer says so: the snapshot follows.
    station.events.emit(GatewayEvent::SettingsChanged { key: "helpers".into(), user_id: None, actor: None });
    let mut followed = false;
    for _ in 0..200 {
        if station.helpers.list().is_empty() {
            followed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(followed, "the snapshot followed the settings");
    assert_eq!(station.by(Ability::Hearing), None);
    station.helpers.set_away(Away::Mine, None).unwrap();
    assert_eq!(station.helpers.away(), Away::Mine, "its own change at once");
}

/// The helpers' tick and their listener end with `stop` (`App::shutdown`): nothing stays behind an engine started
/// again in its process.
#[tokio::test]
async fn the_looks_stop_with_the_gateway() {
    let computer = stand(contract("pc", ALL)).await;
    let station = station(&[]);
    station.put("home-a", &computer.url, HelperKind::Home);
    station.helpers.start();
    let mut looked = false;
    for _ in 0..300 {
        if computer.looks() >= 1 {
            looked = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(looked, "the first look right away");
    station.helpers.stop();
    assert!(station.helpers.stop.is_cancelled() && locked(&station.helpers.followers).is_empty());
}

/// What a pairing over the link checks before it spends the key (stage 7): this gateway itself, the pair's door
/// (D27), and a helper already listed — unless «Подключить заново» names its entry or the pin is the stored one.
#[tokio::test]
async fn the_pairing_check_refuses_this_gateway_the_pairs_door_and_a_listed_helper() {
    use crate::settings::definitions::HelperPin;
    let station = station(&[]);
    station.put(HOME_COMPUTER, "http://192.168.1.20:7780", HelperKind::Home);
    station.put("own-a", "https://192.0.2.10:7781", HelperKind::Own);
    station
        .helpers
        .write(None, |list| {
            list[0].helper_id = Some("the-computer".into());
            list[1].helper_id = Some("the-server".into());
            Ok(Some(()))
        })
        .unwrap();
    let pin = |hash: &str| HelperPin { sha256: Some(hash.repeat(64)), public: None, since: String::new() };
    station.helpers.set_pin("own-a", Some(pin("a"))).unwrap();
    let helper = |id: &str| HelperIdentity { id: id.into(), name: "Турбо".into() };
    let check = station.helpers.pair_check(None);
    let words = |result: Result<(), ApiError>| result.map_err(|error| (error.status.as_u16(), error.message));
    let me = station.helpers.me().unwrap();
    assert_eq!(
        words(check(&helper(&me), &pin("b"))),
        Err((409, "Это сам этот хаб: себя в Турбо не добавляют.".into()))
    );
    assert_eq!(
        words(check(&helper("the-computer"), &pin("b"))),
        Err((409, "Это Турбо уже в списке: «Турбо home-computer».".into()))
    );
    assert_eq!(
        words(check(&helper("the-server"), &pin("b"))),
        Err((409, "Это Турбо уже в списке: «Турбо own-a».".into()))
    );
    assert_eq!(words(check(&helper(""), &pin("b"))).unwrap_err().0, 502, "a helper that names itself not");
    assert!(check(&helper("someone-new"), &pin("b")).is_ok());
    // The very certificate pinned for it: a re-pairing of that entry.
    assert!(check(&helper("the-server"), &pin("a")).is_ok());
    // «Подключить заново» on that entry; never on the pair's door, nor on an entry not there.
    assert!(station.helpers.pair_check(Some("own-a".into()))(&helper("the-server"), &pin("b")).is_ok());
    let door = station.helpers.pair_check(Some(HOME_COMPUTER.into()));
    assert_eq!(words(door(&helper("someone-new"), &pin("b"))).unwrap_err().0, 409);
    let gone = station.helpers.pair_check(Some("own-zz".into()));
    assert_eq!(words(gone(&helper("someone-new"), &pin("b"))).unwrap_err().0, 404);
    // What a pairing renews: the entry `replace` names, or the one of the same helper with the same pin.
    let paired = |id: &str, pin: HelperPin| crate::helpers::link::Paired {
        helper: helper(id),
        key: None,
        pin,
        status: Value::Null,
    };
    assert_eq!(station.helpers.matched(None, &paired("the-server", pin("a"))).unwrap(), Some("own-a".to_string()));
    assert_eq!(station.helpers.matched(None, &paired("the-server", pin("b"))).unwrap(), None);
    assert_eq!(
        station.helpers.matched(Some("own-a"), &paired("someone-new", pin("b"))).unwrap(),
        Some("own-a".to_string())
    );
}

/// The linked helpers' pins keep the list's rule (stage 7 on stage 3's review): pins that do not read keep their last
/// read — a linked entry is never reached plainly meanwhile —, are never written over, and are read again at the next
/// tick.
#[tokio::test]
async fn pins_that_do_not_read_keep_their_last_read_and_are_never_written_over() {
    use crate::settings::definitions::HelperPin;
    let station = station(&[]);
    station.put("own-a", "https://192.0.2.10:7781", HelperKind::Own);
    let pin = |hash: &str| HelperPin { sha256: Some(hash.repeat(64)), public: None, since: String::new() };
    station.helpers.set_pin("own-a", Some(pin("a"))).unwrap();
    // Their row no longer reads (an I/O error of a small disk, say); writing still works.
    station
        .db
        .with(|c| c.execute("UPDATE settings SET is_default = 'unreadable' WHERE key = 'helper_pins'", []))
        .unwrap();
    assert!(station.helpers.reread().is_none(), "the last read stays");
    assert_eq!(station.helpers.pin("own-a"), Some(pin("a")));
    assert!(station.helpers.set_pin("own-b", Some(pin("b"))).is_err(), "nothing written over pins that did not read");
    assert_eq!(station.helpers.pin("own-b"), None);

    // Written meanwhile by another hand (a package's import, say): the next tick reads them.
    let both = json!({ "own-a": pin("a"), "own-b": pin("b") }).to_string();
    station
        .db
        .with(|c| c.execute("UPDATE settings SET value = ?1, is_default = 0 WHERE key = 'helper_pins'", [both]))
        .unwrap();
    station.helpers.check_all().await;
    assert_eq!((station.helpers.pin("own-a"), station.helpers.pin("own-b")), (Some(pin("a")), Some(pin("b"))));
}

/// This gateway's own port in a test, where a linked helper's bridge is asked: a stand-in that keeps each path it was
/// asked with its bearer, and answers 404 as the bridge does for an entry without a link. Never the default port — the
/// gateway that runs on this machine.
async fn own_port() -> (u16, Arc<Mutex<Vec<(String, String)>>>) {
    let asked: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let kept = asked.clone();
    let router = Router::new().fallback(move |request: axum::extract::Request| {
        let kept = kept.clone();
        async move {
            let bearer = request.headers().get("authorization").and_then(|value| value.to_str().ok()).unwrap_or("");
            kept.lock().unwrap().push((request.uri().path().to_string(), bearer.to_string()));
            StatusCode::NOT_FOUND
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (port, asked)
}

/// A linked entry as a pairing stores it: last in the list, its link key, its pin, its helper's id.
fn put_linked(station: &Station, id: &str, url: &str, helper: &str) -> (String, HelperPin) {
    station.put(id, url, HelperKind::Own);
    let key = format!("dwl1_0123456789ab_{}", "a".repeat(64));
    station.secrets.set(&key_secret(id), Some(&key)).unwrap();
    let pin = HelperPin { sha256: Some("ab".repeat(32)), public: None, since: String::new() };
    station.helpers.set_pin(id, Some(pin.clone())).unwrap();
    station
        .helpers
        .write(None, |list| {
            let entry = list.iter_mut().find(|entry| entry.id == id).unwrap();
            entry.helper_id = Some(helper.into());
            Ok(Some(()))
        })
        .unwrap();
    (key, pin)
}

/// The entry's link as one that never dials (`Link::idle`): what it tells is told by hand (`on_link`).
fn idle_link(station: &Station, id: &str, key: &str, pin: HelperPin) -> Arc<link::Link> {
    let entry = station.helpers.entry(id).unwrap();
    let remote = station.helpers.remote(&entry).unwrap();
    let setup = link::LinkSetup {
        entry: id.into(),
        url: entry.url.clone(),
        pin,
        key: key.into(),
        helper_id: entry.helper_id.clone().unwrap_or_default(),
        me: Arc::new(remote::Me::default_for_tests),
        up: None,
    };
    let held = link::Link::idle(setup, LinkTiming::default(), Arc::new(|_: &str, _: LinkNews| {}));
    remote.hold_link(held.clone());
    held
}

/// A linked entry whose pin does not read (review of stage 7): pins that do not read keep their last read — none at a
/// start —, and the entry stays linked by its key all the same. «Мозг» keeps its row with neither an address nor a
/// key, «Обновить» asks only this gateway's bridge (which has no link for it), the entry says why it is away, and
/// nothing reaches the helper plainly: its `dwl1_` key goes nowhere.
#[tokio::test]
async fn a_linked_entry_whose_pin_does_not_read_keeps_its_key_out_of_the_brain_and_off_the_wire() {
    use crate::dialog::brain::BrainService;
    // Where the helper would be: a listener that counts who comes.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("https://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let came = Arc::new(AtomicUsize::new(0));
    let counted = came.clone();
    tokio::spawn(async move {
        while listener.accept().await.is_ok() {
            counted.fetch_add(1, Ordering::SeqCst);
        }
    });
    let (port, asked) = own_port().await;
    let station = station(&[("DIALOG_PROVIDER", "none")]);
    station.helpers.set_local_port(port);
    let (key, _) = put_linked(&station, "own-x", &url, "srv");
    let brain_over = |helpers: Arc<Helpers>| {
        let brain = BrainService::new(
            station.config.dialog.clone(),
            station.db.clone(),
            station.settings.clone(),
            station.secrets.clone(),
            station.events.clone(),
        )
        .unwrap();
        brain.set_helpers(helpers);
        Arc::new(brain)
    };
    // Its link said what it gives: «Мозг» makes its row.
    station.helpers.remote(&station.helpers.entry("own-x").unwrap()).unwrap();
    station.helpers.on_link("own-x", LinkNews::Status(contract("srv", ALL)));
    brain_over(station.helpers.clone()).keep_helper_models().unwrap();

    // Its pins no longer read (an I/O error of a small disk, say), and the gateway starts again: no pin read yet.
    station
        .db
        .with(|c| c.execute("UPDATE settings SET is_default = 'unreadable' WHERE key = 'helper_pins'", []))
        .unwrap();
    let profile = Profile::for_role(station.config.placement.role.as_deref());
    let started = Helpers::new(
        station.config.clone(),
        station.settings.clone(),
        station.secrets.clone(),
        station.events.clone(),
        profile,
        station.identity.clone(),
    );
    started.set_local_port(port);
    assert_eq!((started.list().len(), started.pin("own-x")), (1, None));
    let brain = brain_over(started.clone());
    brain.keep_helper_models().unwrap();
    let row = brain
        .view()
        .providers
        .into_iter()
        .find(|row| row.helper.as_ref().is_some_and(|helper| helper.id == "own-x"))
        .expect("its row kept");
    assert_eq!((row.base_url.as_deref(), row.key.set), (None, false), "neither an address nor a key");
    let kept_keys: Vec<String> = station.secrets.ids("llm:").iter().filter_map(|id| station.secrets.get(id)).collect();
    assert!(!kept_keys.contains(&key), "the link key in no «Мозг» row");

    // «Обновить»: only the bridge is asked, with this start's token — and it has no link for the entry.
    assert!(brain.models(&row.id).await.is_err());
    let asked = asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert!(asked[0].0.starts_with("/via/own-x/home/v1/llm"), "{asked:?}");
    assert_eq!(asked[0].1, format!("Bearer {}", started.local().token()));
    // The looks start and the server listens: away, saying why, and never looked at plainly.
    started.started.store(true, Ordering::SeqCst);
    started.set_local_port(port);
    started.check_all().await;
    let remote = started.remote(&started.entry("own-x").unwrap()).unwrap();
    assert_eq!((remote.state(), remote.error().as_deref()), (HelperState::Down, Some(link::REFUSED_WORDS)));
    assert_eq!(came.load(Ordering::SeqCst), 0, "nothing reached the helper");
    started.stop();
}

/// Whatever writes both the list and the pins asks first whether they read (review of stage 7): while the pins do not
/// read, a pairing is refused before its key is spent and an add over the link stores nothing, and «Удалить» or
/// «Подключить заново» change nothing — no entry gone, no key left behind, no `bye`; read again, the removal goes.
#[tokio::test]
async fn pins_that_do_not_read_stop_a_pairing_before_its_key_is_spent_and_a_removal_before_anything_goes() {
    let server = stand(contract("srv", ALL)).await;
    let station = station(&[]);
    let (key, pin) = put_linked(&station, "own-a", "https://192.0.2.10:7781", "srv-a");
    let unreadable = |broken: bool| {
        let flag = if broken { "'unreadable'" } else { "0" };
        let sql = format!("UPDATE settings SET is_default = {flag} WHERE key = 'helper_pins'");
        station.db.with(|c| c.execute(&sql, [])).unwrap();
    };
    unreadable(true);
    let other = HelperPin { sha256: Some("cd".repeat(32)), public: None, since: String::new() };
    let helper = HelperIdentity { id: "srv-new".into(), name: "Сервер".into() };
    let refused = station.helpers.pair_check(None)(&helper, &other).unwrap_err();
    assert_eq!(refused.status, StatusCode::SERVICE_UNAVAILABLE, "refused before the key is spent");
    // A pairing that got through all the same (the pins went in between): nothing stored.
    let paired = Paired {
        helper: helper.clone(),
        key: Some(format!("dwl1_ba9876543210_{}", "b".repeat(64))),
        pin: other,
        status: contract("srv-new", ALL),
    };
    let added = station.helpers.add_linked("https://192.0.2.20:7781", None, None, Vec::new(), None, paired, None).await;
    assert_eq!(added.unwrap_err().status, StatusCode::SERVICE_UNAVAILABLE);
    // «Удалить» and «Подключить заново» with a plain key: nothing changes.
    assert_eq!(station.helpers.remove("own-a", None).unwrap_err().status, StatusCode::SERVICE_UNAVAILABLE);
    let renew =
        AddInput { url: server.url.clone(), key: KEY.into(), replace: Some("own-a".into()), ..AddInput::default() };
    assert_eq!(station.helpers.add(renew, None).await.unwrap_err().status, StatusCode::SERVICE_UNAVAILABLE);
    let stored: Vec<(String, String)> =
        station.settings.get(&HELPERS, None).unwrap().into_iter().map(|entry| (entry.id, entry.url)).collect();
    assert_eq!(stored, [("own-a".to_string(), "https://192.0.2.10:7781".to_string())]);
    assert_eq!(station.secrets.ids("helper:"), [key_secret("own-a")]);
    assert_eq!((station.helpers.key("own-a"), station.helpers.pin("own-a")), (Some(key), Some(pin)));

    // Read again: the removal goes, its pin and its key with it.
    unreadable(false);
    station.helpers.remove("own-a", None).unwrap();
    assert!(station.helpers.list().is_empty() && station.secrets.ids("helper:").is_empty());
    assert!(station.settings.get(&HELPER_PINS, None).unwrap_or_default().is_empty());
}

/// A linked twin's link runs on (stage 7 on stage 3's twins): while the earlier entry stands for the helper the twin
/// gives nothing, yet its link and its watch stay — the link tells how it is, no look of its own —, so freed when the
/// earlier entry goes away it gives at once.
#[tokio::test]
async fn a_linked_twin_keeps_its_link_and_freed_gives_at_once_from_its_watch() {
    let lan = stand(contract("pc", ALL)).await;
    let (port, _) = own_port().await;
    let station = station(&[]);
    // The computer at home first, then the same computer over its link from outside.
    station.put(HOME_COMPUTER, &lan.url, HelperKind::Home);
    let (key, pin) = put_linked(&station, "own-t", "https://192.0.2.10:7781", "pc");
    let link = idle_link(&station, "own-t", &key, pin);
    // The looks have started and the server listens: the links run.
    station.helpers.started.store(true, Ordering::SeqCst);
    station.helpers.set_local_port(port);
    looks(&station, 1).await;
    station.helpers.on_link("own-t", LinkNews::Up(contract("pc", ALL)));
    assert_eq!(station.helpers.views()[1].state, HelperState::Twin);
    assert_eq!(station.by(Ability::Hearing).as_deref(), Some(HOME_COMPUTER));
    // A change of the list and a tick while it is a twin: its link runs on, and its watch stays.
    station.change("own-t", Change { name: Some("Компьютер снаружи".into()), ..Change::default() });
    looks(&station, 1).await;
    assert!(!link.stopped(), "a twin's link runs on");

    // The computer's home address goes: at the tick it is away, the twin gives — its link said how it is.
    lan.set_away(true);
    looks(&station, 2).await;
    assert_eq!(station.by(Ability::Hearing).as_deref(), Some("own-t"));
    assert_eq!(station.helpers.views()[1].state, HelperState::Up);
    station.helpers.stop();
}

/// News of a linked entry the regulator keeps out now — told just as «Только дома» closed — is forgotten with the
/// watch, as a look's is (stage 7 on stage 3's review): no «на связи», no `helpers.up`; let out again, it gives only
/// once its link tells it anew.
#[tokio::test]
async fn link_news_of_an_entry_the_regulator_keeps_out_is_forgotten_and_nothing_follows_it() {
    let (port, _) = own_port().await;
    let station = station(&[]);
    station.helpers.set_local_port(port);
    put_linked(&station, "own-t", "https://192.0.2.10:7781", "srv");
    let remote = station.helpers.remote(&station.helpers.entry("own-t").unwrap()).unwrap();
    station.helpers.set_away(Away::Home, None).unwrap();
    let mut heard = station.events.subscribe();
    station.helpers.on_link("own-t", LinkNews::Up(contract("srv", ALL)));
    assert_eq!(remote.state(), HelperState::Unknown, "no watch kept");
    let mut said = Vec::new();
    while let Ok(event) = heard.try_recv() {
        said.push(event.name());
    }
    assert!(!said.contains(&"helpers.up"), "{said:?}");

    // Let out again: nothing on what it told while kept out — it gives once its link tells it anew.
    station.helpers.set_away(Away::Any, None).unwrap();
    assert_eq!((station.helpers.state(), station.by(Ability::Hearing)), (HomeServer::Down, None));
    station.helpers.on_link("own-t", LinkNews::Up(contract("srv", ALL)));
    assert_eq!(station.by(Ability::Hearing).as_deref(), Some("own-t"));
}

/// This gateway's own port in a test, where a linked helper's bot is read through the bridge: the entry's
/// `/via/<entry>/home/v1/messenger/contacts` answers its contacts, or 503 while it has no bot (None); anything else 404.
async fn bridge_with_contacts(entry: &str) -> (u16, Arc<Mutex<Option<Vec<String>>>>) {
    let contacts: Arc<Mutex<Option<Vec<String>>>> = Arc::default();
    let read = contacts.clone();
    let router = Router::new().route(
        &format!("/via/{entry}/home/v1/messenger/contacts"),
        get(move || {
            let read = read.clone();
            async move {
                let names = read.lock().unwrap().clone();
                names.map(|names| axum::Json(json!({ "contacts": names }))).ok_or(StatusCode::SERVICE_UNAVAILABLE)
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (port, contacts)
}

/// The news «what a helper gives changed» (`helpers.changed`, `contract`) since the last call.
fn contract_news(heard: &mut tokio::sync::broadcast::Receiver<Arc<GatewayEvent>>) -> usize {
    let mut news = 0;
    while let Ok(event) = heard.try_recv() {
        news += usize::from(matches!(&*event, GatewayEvent::HelpersChanged { change, .. } if change == "contract"));
    }
    news
}

/// A linked helper's bot found or gone is news, as a looked-at helper's is (review of stage 7; stage 5's rule): its
/// contacts read through the bridge when its link comes up, and at each tick, are told as `helpers.changed`
/// «contract», so the registry and the panels read the messages again; a new contact is no news.
#[tokio::test]
async fn a_linked_helpers_bot_found_or_gone_through_the_bridge_is_news() {
    use crate::core::availability::State;
    use crate::placement::AvailabilityService;
    let (port, contacts) = bridge_with_contacts("own-l").await;
    let station = station(&[]);
    station.helpers.set_local_port(port);
    put_linked(&station, "own-l", "https://192.0.2.10:7781", "srv");
    station.helpers.remote(&station.helpers.entry("own-l").unwrap()).unwrap();
    let registry = AvailabilityService::with(Profile { station: true }, false, false, EventBus::new());
    registry.set_helpers(station.helpers.clone());
    let messages = || {
        let state = registry.get("messages").unwrap();
        (state.state, state.reason)
    };
    let mut heard = station.events.subscribe();

    // The link comes up, no bot set up there (503): what it declares is news, and so is its bot's absence, once the
    // read through the bridge is back.
    station.helpers.on_link("own-l", LinkNews::Up(contract("srv", ALL)));
    let both = tokio::time::timeout(Duration::from_secs(5), async {
        let mut news = 0;
        while news < 2 {
            let event = heard.recv().await.unwrap();
            news += usize::from(matches!(&*event, GatewayEvent::HelpersChanged { change, .. } if change == "contract"));
        }
    });
    assert!(both.await.is_ok(), "its bot's absence was not told");
    assert_eq!(messages(), (State::Off, "У Турбо не настроен бот Telegram".to_string()));

    // Its bot set up there: the tick reads it through the bridge — news, and the messages are the helper's.
    *contacts.lock().unwrap() = Some(vec!["Мама".into()]);
    station.helpers.check_all().await;
    assert_eq!(contract_news(&mut heard), 1, "its bot found");
    assert_eq!(messages(), (State::Ready, "у Турбо".to_string()));
    *contacts.lock().unwrap() = Some(vec!["Мама".into(), "Папа".into()]);
    station.helpers.check_all().await;
    assert_eq!(contract_news(&mut heard), 0, "a new contact is no news to the registry");
    // Its bot gone: news again.
    *contacts.lock().unwrap() = None;
    station.helpers.check_all().await;
    assert_eq!(contract_news(&mut heard), 1, "its bot gone");
    assert_eq!(messages(), (State::Off, "У Турбо не настроен бот Telegram".to_string()));
}

/// The clean-path check of 19.11: an app on the Турбо was said to be «off» whatever stopped it. The helpers say why no
/// one gives an ability now, by the helper's name: asleep (off the line), not giving it to this home, not taken from it
/// («Не брать у него…»), switched off — or that the home has none; nothing while one gives it. And who gives it.
#[tokio::test]
async fn the_helpers_say_by_name_why_nobody_gives_an_ability_now() {
    use super::now::{giver_name, why_not};
    let station = station(&[]);
    assert_eq!(why_not(&station.helpers, Ability::Apps).as_deref(), Some("в доме нет Турбо, который это даёт"));

    let mac = stand(contract("mac", ALL)).await;
    station.put("mac", &mac.url, HelperKind::Home);
    looks(&station, 1).await;
    assert_eq!(why_not(&station.helpers, Ability::Apps), None, "on the line and giving");
    assert_eq!(giver_name(&station.helpers, Ability::Apps).as_deref(), Some("Турбо mac"));

    mac.set_away(true);
    looks(&station, 2).await;
    assert_eq!(
        why_not(&station.helpers, Ability::Apps).as_deref(),
        Some("Турбо mac не на связи — спит, выключен или без сети")
    );
    assert_eq!(giver_name(&station.helpers, Ability::Apps), None);

    // On the line, giving this home no apps.
    mac.set_away(false);
    let no_apps: Vec<(&str, bool)> = ALL.iter().copied().filter(|(id, _)| *id != "apps").collect();
    *mac.status.lock().unwrap() = contract("mac", &no_apps);
    looks(&station, 1).await;
    assert_eq!(why_not(&station.helpers, Ability::Apps).as_deref(), Some("Турбо mac не даёт этого вашему дому"));

    *mac.status.lock().unwrap() = contract("mac", ALL);
    station.change("mac", Change { without: Some(vec![Ability::Apps]), ..Change::default() });
    looks(&station, 1).await;
    assert_eq!(
        why_not(&station.helpers, Ability::Apps).as_deref(),
        Some("у Турбо mac это не берётся («Не брать у него…»)")
    );

    station.change("mac", Change { enabled: Some(false), without: Some(Vec::new()), ..Change::default() });
    assert_eq!(why_not(&station.helpers, Ability::Apps).as_deref(), Some("Турбо mac выключен в «Доме»"));
}
