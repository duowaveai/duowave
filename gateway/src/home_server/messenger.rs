//! «передай маме, что …» on the station: its build has no Telegram bot (the calls module is the computer's), so while
//! the computer helps it («Колонка + ПК», «Что даю колонке» → «Сообщения») the message goes through the computer's
//! bot: `POST /home/v1/messenger/send` with the station's home-server key (serve.rs). The names of the computer's
//! contacts come with each look at the computer (`HomeServerService::check`), so the skill knows at once whether a
//! name is ours; their chats stay on the computer.
//!
//! A gateway with a bot of its own (the computer's build) uses its own: this one stands aside then.
//!
//! The other way: the bot's messages from the contacts come from the computer (src/placement/messages.rs) to
//! `POST /api/v1/home-server/message` here, with the pair's key, and the calls skill says or plays them. The same part
//! of «Что даю колонке», «Сообщения», governs both ways.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use futures::future::BoxFuture;
use serde_json::{json, Value};

use super::{HomeServerService, Messenger};
use crate::app::App;
use crate::core::availability::HomeServer;
use crate::core::identity::permissions::STATIONS_MANAGE;
use crate::platform::http::{created, ApiError, ApiResult, Caller};
use crate::skills::media::sources::match_score;
use crate::skills::own::calls_skill::{Contact, IncomingMessage, LinkButton, MessengerAccess, NotSent, NO_RECIPIENT};

/// Where the computer hands the station its bot's messages.
pub const MESSAGE_PATH: &str = "/api/v1/home-server/message";
/// A voice message the computer hands on, at most (Ogg/Opus: some ten minutes of speech). Base64 in JSON is a third
/// more: the route takes up to `BODY_LIMIT`.
pub const VOICE_LIMIT: usize = 2 * 1024 * 1024;
pub const BODY_LIMIT: usize = VOICE_LIMIT / 3 * 4 + 64 * 1024;
/// Voice messages waiting to be played, at most: the station's /data is small (~46 MB free).
const WAITING: usize = 5;
/// A copy left behind (a restart while it played) is removed after this long.
const STALE: Duration = Duration::from_secs(600);
/// The station's answers to the computer.
pub const NOT_TAKEN: &str = "Колонка не берёт сообщения у компьютера: «Сообщения» в «Что даю колонке» выключены.";
pub const ONLY_PAIR: &str = "Сообщения колонке передаёт только компьютер пары.";
pub const BUSY: &str = "Колонка ещё не проиграла прежние голосовые сообщения.";

/// What the station says when the message cannot go.
pub const AWAY: &str = "Компьютер не на связи — передать сообщение некому.";
pub const NO_BOT: &str = "На компьютере не настроен бот Telegram.";
pub const NOT_ACCEPTED: &str = "Telegram не принял сообщение.";
pub const KEY_REFUSED: &str = "Компьютер не принимает ключ колонки.";
pub const LATE: &str = "Компьютер не ответил вовремя — не знаю, ушло ли сообщение.";
pub const FAILED: &str = "Компьютер не смог передать сообщение.";

/// Telegram's own wait on the computer is 30 s; a message rarely takes a second.
const SEND_WAIT: Duration = Duration::from_secs(20);

/// The skill's messenger: this gateway's own bot when it has one, otherwise the computer's through the home server.
pub struct HomeMessenger {
    local: Arc<dyn MessengerAccess>,
    home: Arc<HomeServerService>,
}

impl HomeMessenger {
    pub fn new(local: Arc<dyn MessengerAccess>, home: Arc<HomeServerService>) -> Arc<HomeMessenger> {
        Arc::new(HomeMessenger { local, home })
    }

    /// The computer's bot is the one: none here, the station leans on the computer and takes «Сообщения» from it.
    fn through_home(&self) -> bool {
        !self.local.enabled() && self.home.configured().is_some() && self.home.serves("messages")
    }

    /// The names of the computer's contacts, when the station takes messages through it and knows them.
    fn names(&self) -> Option<Vec<String>> {
        match self.home.messenger() {
            Messenger::Contacts(names) if self.through_home() => Some(names),
            _ => None,
        }
    }
}

/// The best of the names for the one heard («Бабушке» finds «бабушка»), as the bot itself matches them.
fn best(heard: &str, names: &[String]) -> Option<String> {
    let mut scored: Vec<(f64, &String)> =
        names.iter().map(|name| (match_score(heard, name), name)).filter(|(score, _)| *score >= 0.5).collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored.first().map(|(_, name)| (*name).clone())
}

impl MessengerAccess for HomeMessenger {
    fn enabled(&self) -> bool {
        self.local.enabled() || self.names().is_some()
    }

    /// The contact's «chat» is its name on the computer: the computer's bot finds the chat by it.
    fn contact(&self, name: &str) -> Option<Contact> {
        if !self.through_home() {
            return self.local.contact(name);
        }
        best(name, &self.names()?).map(|name| Contact { name: name.clone(), chat_id: name })
    }

    fn unavailable(&self) -> Option<String> {
        if !self.through_home() {
            return self.local.unavailable();
        }
        if self.home.state() != HomeServer::Up {
            return Some(AWAY.into());
        }
        (self.home.messenger() == Messenger::NoBot).then(|| NO_BOT.into())
    }

    fn send_text(
        &self,
        chat_id: String,
        text: String,
        button: Option<LinkButton>,
    ) -> BoxFuture<'static, anyhow::Result<()>> {
        if !self.through_home() {
            return self.local.send_text(chat_id, text, button);
        }
        let home = self.home.configured();
        Box::pin(async move {
            // A call's link is the calls module's, and that stays on the computer.
            if button.is_some() {
                anyhow::bail!("a call's link through the computer: not supported");
            }
            let Some((url, key)) = home else { return Err(NotSent(AWAY.into()).into()) };
            let answer = crate::utils::http::client()
                .post(format!("{url}/home/v1/messenger/send"))
                .bearer_auth(key)
                .json(&json!({ "contact": chat_id, "text": text }))
                .timeout(SEND_WAIT)
                .send()
                .await;
            let response = match answer {
                Ok(response) => response,
                Err(error) if error.is_timeout() => return Err(NotSent(LATE.into()).into()),
                Err(_) => return Err(NotSent(AWAY.into()).into()),
            };
            let status = response.status().as_u16();
            if response.status().is_success() {
                let body: Value = response.json().await.unwrap_or(Value::Null);
                if body.get("sent").and_then(Value::as_bool) == Some(true) {
                    return Ok(());
                }
                return Err(NotSent(FAILED.into()).into());
            }
            let message = response.json::<Value>().await.ok();
            let message = message.as_ref().and_then(|body| body.get("message")).and_then(Value::as_str);
            tracing::warn!("home server: a message not passed on: HTTP {status}: {}", message.unwrap_or("-"));
            let said = match status {
                404 => NO_RECIPIENT,
                503 => NO_BOT,
                502 => NOT_ACCEPTED,
                401 | 403 => KEY_REFUSED,
                _ => FAILED,
            };
            Err(NotSent(said.into()).into())
        })
    }
}

/// Where the voice messages from the computer wait to be played: under the gateway's data folder, not in memory.
fn waiting_room(app: &App) -> PathBuf {
    app.config
        .data_dir
        .as_ref()
        .map(|dir| dir.join("tmp").join("messages"))
        .unwrap_or_else(|| std::env::temp_dir().join("duowave-messages"))
}

/// The copies waiting now; one older than `STALE` is removed (its playing was cut short).
fn waiting(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let now = SystemTime::now();
    let mut count = 0;
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .is_ok_and(|at| now.duration_since(at).is_ok_and(|age| age > STALE));
        if old {
            let _ = std::fs::remove_file(entry.path());
        } else {
            count += 1;
        }
    }
    count
}

/// `POST /api/v1/home-server/message` `{from, text | voice (base64 Ogg/Opus), at}`: a contact's message from the
/// computer's bot, said («Мама пишет: …») or played after «Голосовое сообщение: мама.». Only the computer of the pair
/// sends it — its own call, or a person at it with the right to manage the stations — and only while the station
/// takes «Сообщения» from it.
pub async fn receive(State(app): State<Arc<App>>, caller: Caller, body: Bytes) -> ApiResult<Value> {
    if !crate::placement::peer::from_peer(&caller, app.placement.peer_key()) {
        return Err(ApiError::forbidden(ONLY_PAIR));
    }
    if crate::placement::peer::through_pass(&caller) {
        caller.permitted(STATIONS_MANAGE)?;
    }
    if app.home_server.configured().is_none() || !app.home_server.serves("messages") {
        return Err(ApiError::conflict(NOT_TAKEN));
    }
    let request: Value =
        serde_json::from_slice(&body).map_err(|_| ApiError::bad_request("JSON: {from, text | voice, at}"))?;
    let field = |name: &str| request.get(name).and_then(Value::as_str).map(str::trim).filter(|value| !value.is_empty());
    let from = field("from").ok_or_else(|| ApiError::bad_request("from: от кого сообщение"))?.to_string();
    let text = field("text").map(str::to_string);
    let at = field("at").unwrap_or("-").to_string();
    let file = match field("voice") {
        None => None,
        Some(voice) => {
            use base64::Engine as _;
            let audio = base64::engine::general_purpose::STANDARD
                .decode(voice)
                .map_err(|_| ApiError::bad_request("voice: base64 голосового сообщения"))?;
            if audio.len() > VOICE_LIMIT {
                return Err(ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, "Голосовое сообщение больше 2 МБ."));
            }
            let dir = waiting_room(&app);
            if waiting(&dir) >= WAITING {
                return Err(ApiError::unavailable(BUSY));
            }
            let file = dir.join(format!("{}.ogg", hex::encode(crate::utils::ids::random_bytes(8))));
            tokio::fs::create_dir_all(&dir).await.map_err(ApiError::internal)?;
            tokio::fs::write(&file, &audio).await.map_err(ApiError::internal)?;
            Some(file)
        }
    };
    if file.is_none() && text.is_none() {
        return Err(ApiError::bad_request("text или voice: что сказать"));
    }
    tracing::info!(
        "home server: a {} from «{from}» (at {at}) came from the computer's bot",
        if file.is_some() { "voice message" } else { "message" }
    );
    app.skills.calls.on_message(IncomingMessage { from, chat_id: String::new(), file, text, discard: true });
    Ok(created(json!({ "accepted": true })))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::{get, post};
    use axum::Router;
    use chrono::Utc;

    use super::*;
    use crate::config::{Config, Env};
    use crate::core::events::EventBus;
    use crate::secrets::SecretsService;
    use crate::settings::SettingsService;
    use crate::skills::capabilities::{CapabilityName, Intent};
    use crate::skills::own::calls_skill::{CallsSkill, NoCalls};
    use crate::skills::skill::{ExecutionContext, OwnSkill};
    use crate::storage::Db;

    /// The computer as the station sees it: its status, its bot's contacts, its sends.
    #[derive(Default)]
    struct Computer {
        away: AtomicBool,
        /// None: no bot set up.
        contacts: Mutex<Option<Vec<&'static str>>>,
        /// What the bot was asked to pass on.
        sent: Mutex<Vec<(String, String)>>,
        /// Telegram refuses.
        refuses: AtomicBool,
    }

    fn keyed(headers: &HeaderMap) -> bool {
        headers.get("authorization").and_then(|value| value.to_str().ok()) == Some("Bearer vgw_home")
    }

    async fn computer(state: Arc<Computer>) -> String {
        let status = {
            let state = state.clone();
            move |headers: HeaderMap| {
                let state = state.clone();
                async move {
                    if state.away.load(Ordering::Relaxed) {
                        return Err(StatusCode::SERVICE_UNAVAILABLE);
                    }
                    if !keyed(&headers) {
                        return Err(StatusCode::UNAUTHORIZED);
                    }
                    Ok(axum::Json(json!({ "asr": true, "tts": true })))
                }
            }
        };
        let contacts = {
            let state = state.clone();
            move |headers: HeaderMap| {
                let state = state.clone();
                async move {
                    if !keyed(&headers) {
                        return Err(StatusCode::UNAUTHORIZED);
                    }
                    let contacts = state.contacts.lock().unwrap().clone().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
                    Ok(axum::Json(json!({ "contacts": contacts })))
                }
            }
        };
        let send = move |headers: HeaderMap, axum::Json(body): axum::Json<Value>| {
            let state = state.clone();
            async move {
                if !keyed(&headers) {
                    return Err(StatusCode::UNAUTHORIZED);
                }
                let Some(contacts) = state.contacts.lock().unwrap().clone() else {
                    return Err(StatusCode::SERVICE_UNAVAILABLE);
                };
                let contact = body["contact"].as_str().unwrap_or_default().to_string();
                if !contacts.iter().any(|known| *known == contact) {
                    return Err(StatusCode::NOT_FOUND);
                }
                if state.refuses.load(Ordering::Relaxed) {
                    return Err(StatusCode::BAD_GATEWAY);
                }
                state.sent.lock().unwrap().push((contact.clone(), body["text"].as_str().unwrap_or_default().into()));
                Ok(axum::Json(json!({ "sent": true, "contact": contact })))
            }
        };
        let router = Router::new()
            .route("/home/v1/status", get(status))
            .route("/home/v1/messenger/contacts", get(contacts))
            .route("/home/v1/messenger/send", post(send));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        format!("http://{address}")
    }

    /// The station leaning on the computer at `url`, and its calls skill with no bot of its own.
    fn station(url: &str) -> (Arc<HomeServerService>, Arc<CallsSkill>) {
        let config = Arc::new(Config::for_tests(&Env::from_pairs([
            ("HOME_SERVER_URL", url),
            ("HOME_SERVER_KEY", "vgw_home"),
            ("DUOWAVE_ROLE", "station"),
        ])));
        let db = Arc::new(Db::memory());
        let events = EventBus::new();
        let settings = Arc::new(SettingsService::new(db.clone(), events.clone()));
        let secrets = Arc::new(SecretsService::new(db, None).unwrap());
        let home = HomeServerService::new(config, settings, secrets, events);
        let none = Arc::new(NoCalls);
        let skill = CallsSkill::new(none.clone(), none.clone(), HomeMessenger::new(none, home.clone()));
        (home, skill)
    }

    fn message(contact: &str, text: &str) -> Intent {
        Intent {
            capability: CapabilityName::Calls,
            action: "message".into(),
            params: json!({ "contact": contact, "text": text }).as_object().cloned().unwrap(),
        }
    }

    async fn said(skill: &CallsSkill, intent: &Intent) -> String {
        let outcome = skill.execute(intent, &ExecutionContext::new("default", Utc::now())).await.unwrap();
        outcome.speech.unwrap_or_default()
    }

    /// Live 10.11 on the station: «Передай бабушке: купи хлеба.» answered «Не знаю, кому передать.» — the
    /// station's build has no bot. Through the computer's it goes.
    #[tokio::test]
    async fn the_station_passes_a_message_on_through_the_computers_bot() {
        let pc = Arc::new(Computer::default());
        *pc.contacts.lock().unwrap() = Some(vec!["мама", "бабушка"]);
        let (home, skill) = station(&computer(pc.clone()).await);
        let to_granny = message("Бабушке", "Купи хлеба.");
        assert!(!skill.ours(&to_granny, "default"), "the computer's contacts are not read yet");

        home.check().await;
        assert_eq!(home.messenger(), Messenger::Contacts(vec!["мама".into(), "бабушка".into()]));
        assert!(skill.ours(&to_granny, "default"), "Бабушка is the computer's contact: ours, not Yandex's");
        let outcome = skill.execute(&to_granny, &ExecutionContext::new("default", Utc::now())).await.unwrap();
        assert_eq!(outcome.speech.as_deref(), Some("Передала."));
        assert_eq!(outcome.done.as_deref(), Some("контакту «бабушка» отправлено в Telegram сообщение «Купи хлеба.»"));
        assert_eq!(*pc.sent.lock().unwrap(), [("бабушка".to_string(), "Купи хлеба.".to_string())]);

        // Telegram refuses on the computer: said so.
        pc.refuses.store(true, Ordering::Relaxed);
        assert_eq!(said(&skill, &message("маме", "Я задержусь")).await, NOT_ACCEPTED);
    }

    #[tokio::test]
    async fn a_name_the_computer_does_not_know_is_not_ours_and_is_said_so() {
        let pc = Arc::new(Computer::default());
        *pc.contacts.lock().unwrap() = Some(vec!["мама"]);
        let (home, skill) = station(&computer(pc.clone()).await);
        home.check().await;
        let to_petya = message("Пете", "Привет");
        assert!(!skill.ours(&to_petya, "default"), "Yandex's, while it is up");
        assert_eq!(said(&skill, &to_petya).await, NO_RECIPIENT, "«Свой»: ours all the same");
        // The contact gone from the computer's bot since the last look: the computer's answer says so.
        *pc.contacts.lock().unwrap() = Some(vec!["папа"]);
        assert_eq!(said(&skill, &message("маме", "Привет")).await, NO_RECIPIENT);
        assert!(pc.sent.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_computer_away_is_said_so_and_its_contacts_are_still_ours() {
        let pc = Arc::new(Computer::default());
        *pc.contacts.lock().unwrap() = Some(vec!["мама"]);
        let (home, skill) = station(&computer(pc.clone()).await);
        home.check().await;
        pc.away.store(true, Ordering::Relaxed);
        home.check().await;
        home.check().await;
        assert_eq!(home.state(), HomeServer::Down);
        let to_mom = message("маме", "Я задержусь");
        assert!(skill.ours(&to_mom, "default"), "a contact seen before: ours, to say the computer is away");
        assert_eq!(said(&skill, &to_mom).await, AWAY);
        assert_eq!(said(&skill, &message("Пете", "Привет")).await, AWAY, "whoever it is for");

        // Up by the last look, gone by the send: the same words.
        let closed = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://{}", listener.local_addr().unwrap())
        };
        let (home, skill) = station(&closed);
        home.seen(&closed, Ok(()));
        *home.messenger.lock().unwrap() = Messenger::Contacts(vec!["мама".into()]);
        assert_eq!(said(&skill, &to_mom).await, AWAY);
    }

    #[tokio::test]
    async fn no_bot_on_the_computer_is_said_so_and_messages_not_taken_leave_it_alone() {
        let pc = Arc::new(Computer::default());
        let (home, skill) = station(&computer(pc.clone()).await);
        home.check().await;
        assert_eq!((home.state(), home.messenger()), (HomeServer::Up, Messenger::NoBot));
        let to_mom = message("маме", "Я задержусь");
        assert!(!skill.ours(&to_mom, "default"));
        assert_eq!(said(&skill, &to_mom).await, NO_BOT);

        // «Что даю колонке» without «Сообщения»: the station does not ask the computer, as with no computer.
        *pc.contacts.lock().unwrap() = Some(vec!["мама"]);
        home.check().await;
        assert!(skill.ours(&to_mom, "default"));
        home.link(None, None, None, Some(vec!["messages".into()]), None).unwrap();
        assert!(!skill.ours(&to_mom, "default"));
        assert_eq!(said(&skill, &to_mom).await, NO_RECIPIENT);
        assert!(pc.sent.lock().unwrap().is_empty());
    }

    // ── The other way: the computer's bot's messages, said or played on the station. ──

    const PAIR: &str = "0123456789abcdef0123456789abcdef";

    /// What the station said, and the audio after it.
    #[derive(Default)]
    struct Spoken(Mutex<Vec<(String, Option<Vec<u8>>)>>);

    impl crate::skills::own::reminders_skill::Speaker for Arc<Spoken> {
        fn say(
            &self,
            _device: String,
            text: String,
            clip: Option<crate::skills::own::reminders_skill::AudioClip>,
        ) -> BoxFuture<'static, bool> {
            self.0.lock().unwrap().push((text, clip.map(|clip| clip.audio.to_vec())));
            Box::pin(async { true })
        }
    }

    /// The station's gateway (the build playing it), leaning on a computer, with the pair's key; its data in `dir`.
    fn station_gateway(dir: &Path) -> (Arc<App>, Arc<Spoken>) {
        let data = dir.to_string_lossy().into_owned();
        let env = Env::from_pairs([
            ("STATE_DIR", ""),
            ("DATA_DIR", data.as_str()),
            ("PLUGINS_BUNDLED", "false"),
            ("DUOWAVE_ROLE", "station"),
            ("PEER_KEY", PAIR),
            ("HOME_SERVER_URL", "http://192.0.2.10"),
            ("HOME_SERVER_KEY", "vgw_home"),
        ]);
        let app = App::build(Config::for_tests(&env)).unwrap();
        let spoken = Arc::new(Spoken::default());
        app.skills.attach_speaker(Arc::new(spoken.clone()));
        (app, spoken)
    }

    /// The computer's call from the home network: the pair's key, and the pass's headers when given.
    async fn hand(app: &Arc<App>, key: Option<&str>, pass: &[(&str, &str)], body: Value) -> (StatusCode, Value) {
        use axum::extract::ConnectInfo;
        use http_body_util::BodyExt;
        use tower::ServiceExt;
        let mut request =
            axum::http::Request::builder().method("POST").uri(MESSAGE_PATH).header("content-type", "application/json");
        if let Some(key) = key {
            request = request.header("authorization", format!("Bearer {key}"));
        }
        for (name, value) in pass {
            request = request.header(*name, *value);
        }
        let mut request = request.body(axum::body::Body::from(body.to_string())).unwrap();
        let home = std::net::SocketAddr::new("192.168.1.20".parse().unwrap(), 5000);
        request.extensions_mut().insert(ConnectInfo(home));
        let response = crate::app::router(app.clone()).oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn until(what: impl Fn() -> bool) -> bool {
        for _ in 0..300 {
            if what() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        false
    }

    /// Live 11.11: a text and a voice message to the bot, nothing played on the station. The computer hands them to
    /// the station's gateway, which says the text and plays the voice after a word of who sent it.
    #[tokio::test]
    async fn the_station_says_a_text_and_plays_a_voice_message_the_computer_hands_it() {
        let dir = tempfile::tempdir().unwrap();
        let (app, spoken) = station_gateway(dir.path());
        let text = json!({ "from": "мама", "text": "Скоро буду", "at": "2026-11-11T10:00:00Z" });
        let (status, body) = hand(&app, Some(PAIR), &[], text).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        assert!(until(|| spoken.0.lock().unwrap().len() == 1).await);
        assert_eq!(spoken.0.lock().unwrap()[0], ("Мама пишет: Скоро буду".to_string(), None));

        use base64::Engine as _;
        let voice = json!({ "from": "мама", "voice": base64::engine::general_purpose::STANDARD.encode(b"OggS-voice") });
        let (status, body) = hand(&app, Some(PAIR), &[], voice).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        assert!(until(|| spoken.0.lock().unwrap().len() == 2).await);
        assert_eq!(
            spoken.0.lock().unwrap()[1],
            ("Голосовое сообщение: мама.".to_string(), Some(b"OggS-voice".to_vec()))
        );
        // Its copy waited under the gateway's data folder, and is gone once played.
        let room = dir.path().join("tmp").join("messages");
        assert!(until(|| waiting(&room) == 0).await, "the copy is removed");
    }

    #[tokio::test]
    async fn only_the_computer_of_the_pair_hands_messages_and_only_while_the_station_takes_them() {
        let dir = tempfile::tempdir().unwrap();
        let (app, spoken) = station_gateway(dir.path());
        let text = json!({ "from": "мама", "text": "Скоро буду" });
        let (status, body) = hand(&app, None, &[], text.clone()).await;
        assert_eq!((status, body["message"].as_str()), (StatusCode::FORBIDDEN, Some(ONLY_PAIR)));
        let (status, _) = hand(&app, Some("another-key-0123456789abcdef0123"), &[], text.clone()).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        // A guest at the computer, through the pass: not one who manages the stations.
        let guest = [(crate::placement::peer::HELPER_PERSON, "guest"), (crate::placement::peer::HELPER_ROLE, "guest")];
        let (status, _) = hand(&app, Some(PAIR), &guest, text.clone()).await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        let (status, body) = hand(&app, Some(PAIR), &[], json!({ "from": "мама" })).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        use base64::Engine as _;
        let long = base64::engine::general_purpose::STANDARD.encode(vec![0u8; VOICE_LIMIT + 1]);
        let (status, _) = hand(&app, Some(PAIR), &[], json!({ "from": "мама", "voice": long })).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);

        // One switch for both ways: «Сообщения» off in «Что даю колонке» — the station does not take them.
        app.home_server.link(None, None, None, Some(vec!["messages".into()]), None).unwrap();
        let (status, body) = hand(&app, Some(PAIR), &[], text).await;
        assert_eq!((status, body["message"].as_str()), (StatusCode::CONFLICT, Some(NOT_TAKEN)));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(spoken.0.lock().unwrap().is_empty(), "nothing said");
    }
}
