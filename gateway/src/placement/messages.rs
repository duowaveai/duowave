//! The bot's messages for the stations this computer helps (docs/plans/station-placement.md, section 5). The Telegram
//! bot is this computer's; a station whose gateway runs on it is not at this computer's /uni.ws, so what the calls
//! skill says here reaches nobody there (live 11.11: a text and a voice message, nothing played, nothing in the log).
//! Each contact's message goes to that station's gateway too, with the pair's key, as the pass and the relay go
//! (proxy.rs, relay.rs): `POST /api/v1/home-server/message` there hands it to its own calls skill
//! (src/home_server/messenger.rs).
//!
//! Only to a station that takes «Сообщения» from this computer — «Колонка + ПК», and «Что даю колонке» gives it
//! «Сообщения» as its /health says (not heard yet: the station decides, and refuses when it does not take them). The
//! one switch governs both ways. A station away or busy is tried five times over about a minute; then the log says
//! whose message did not reach it. A voice message larger than 2 MB stays on this computer, said in the log too.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::watch::StationGateways;
use crate::core::availability::Placement;
use crate::home_server::messenger::{MESSAGE_PATH, VOICE_LIMIT};
use crate::settings::definitions::StationPlacement;
use crate::skills::own::calls_skill::{IncomingMessage, MessageRelay};
use crate::utils::time::now_iso;

/// The pauses between the tries: five tries over about a minute.
const PAUSES: [Duration; 4] =
    [Duration::from_secs(5), Duration::from_secs(10), Duration::from_secs(15), Duration::from_secs(30)];
/// One try: a voice message is a couple of MB over the home network.
const WAIT: Duration = Duration::from_secs(20);

/// A station's gateway to hand a message to: its name for the log, its address and the pair's key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub name: String,
    pub base: String,
    pub key: String,
}

type Targets = Arc<dyn Fn() -> Vec<Target> + Send + Sync>;

pub struct StationMessages {
    targets: Targets,
    pauses: Vec<Duration>,
}

impl StationMessages {
    /// The stations with a placement here that take «Сообщения» from this computer, as they are at each message.
    pub fn new(stations: &Arc<StationGateways>) -> Arc<StationMessages> {
        let stations = Arc::downgrade(stations);
        let targets: Targets =
            Arc::new(move || stations.upgrade().map(|stations| targets(&stations)).unwrap_or_default());
        StationMessages::with(targets, PAUSES.to_vec())
    }

    fn with(targets: Targets, pauses: Vec<Duration>) -> Arc<StationMessages> {
        Arc::new(StationMessages { targets, pauses })
    }
}

fn targets(stations: &StationGateways) -> Vec<Target> {
    stations
        .all()
        .into_iter()
        .filter_map(|watch| {
            let record = watch.on_station()?;
            if !takes_messages(&record, watch.live().health.as_ref()) {
                return None;
            }
            let client = watch.client()?;
            Some(Target {
                name: watch.name().unwrap_or_else(|| watch.key().to_string()),
                base: client.base().to_string(),
                key: client.key().to_string(),
            })
        })
        .collect()
}

/// Whether this station takes the bot's messages from this computer: its gateway runs on it with this computer as its
/// home server, and its /health does not say «Сообщения» are off — or the help is. A station not heard yet is asked
/// all the same: it refuses when it does not take them.
pub fn takes_messages(record: &StationPlacement, health: Option<&Value>) -> bool {
    if record.placement != Placement::StationServer {
        return false;
    }
    let Some(health) = health else { return true };
    let Some(home) = health.pointer("/placement/homeServer").filter(|home| home.is_object()) else { return false };
    let enabled = home.get("enabled").and_then(Value::as_bool) != Some(false);
    let without = home.get("without").and_then(Value::as_array);
    enabled && !without.is_some_and(|parts| parts.iter().any(|part| part.as_str() == Some("messages")))
}

impl MessageRelay for StationMessages {
    fn pass_on(&self, message: &IncomingMessage) -> usize {
        if message.file.is_none() && message.text.as_deref().is_none_or(str::is_empty) {
            return 0;
        }
        let targets = (self.targets)();
        let at = now_iso();
        for target in &targets {
            tokio::spawn(deliver(target.clone(), message.clone(), at.clone(), self.pauses.clone()));
        }
        targets.len()
    }
}

/// How a try ended, when not with the station's yes.
enum Failure {
    /// Worth another try: the station away, busy, starting again.
    Again(String),
    /// It will not take it: another try says the same.
    Final(String),
}

/// The message as the station takes it: who, the text or the voice message (base64 Ogg/Opus), when it came.
async fn body_of(message: &IncomingMessage, at: &str) -> Result<Value, String> {
    let mut body = json!({ "from": message.from, "at": at });
    if let Some(file) = &message.file {
        let audio =
            tokio::fs::read(file).await.map_err(|error| format!("голосовое сообщение не прочитать: {error}"))?;
        if audio.len() > VOICE_LIMIT {
            return Err(format!(
                "голосовое сообщение больше 2 МБ ({} КБ) — оно осталось на компьютере",
                audio.len() / 1024
            ));
        }
        use base64::Engine as _;
        body["voice"] = Value::String(base64::engine::general_purpose::STANDARD.encode(audio));
    }
    if let Some(text) = message.text.as_deref().filter(|text| !text.is_empty()) {
        body["text"] = Value::String(text.to_string());
    }
    Ok(body)
}

async fn send(target: &Target, body: &Value) -> Result<(), Failure> {
    let answer = super::proxy::client()
        .post(format!("{}{MESSAGE_PATH}", target.base))
        .bearer_auth(&target.key)
        .json(body)
        .timeout(WAIT)
        .send()
        .await;
    let response = match answer {
        Ok(response) => response,
        Err(error) if error.is_timeout() => return Err(Failure::Again("шлюз колонки не ответил вовремя".into())),
        Err(error) if error.is_connect() => return Err(Failure::Again("шлюз колонки не отвечает".into())),
        Err(error) => return Err(Failure::Again(format!("шлюз колонки: {}", error.without_url()))),
    };
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let said: Value = response.json().await.unwrap_or(Value::Null);
    let message = said.get("message").and_then(Value::as_str).unwrap_or_default();
    let why = format!("шлюз колонки ответил {}: {message}", status.as_u16());
    match status.as_u16() {
        404 => Err(Failure::Final(format!("шлюз колонки старше и не принимает сообщения ({})", status.as_u16()))),
        400..=499 if status.as_u16() != 408 && status.as_u16() != 429 => Err(Failure::Final(why)),
        _ => Err(Failure::Again(why)),
    }
}

/// One station's copy of the message: tried until it takes it, refuses it, or the tries run out — never dropped
/// without a word in the log.
async fn deliver(target: Target, message: IncomingMessage, at: String, pauses: Vec<Duration>) {
    let from = message.from.as_str();
    let name = target.name.as_str();
    let body = match body_of(&message, &at).await {
        Ok(body) => body,
        Err(why) => {
            tracing::warn!(target: "placement", "сообщение от «{from}» не передано колонке «{name}»: {why}");
            return;
        }
    };
    let started = Instant::now();
    let mut pauses = pauses.into_iter();
    let mut tries = 1;
    loop {
        match send(&target, &body).await {
            Ok(()) => {
                crate::home_server::serve::count_message(started);
                tracing::info!(target: "placement", "сообщение от «{from}» передано колонке «{name}»");
                return;
            }
            Err(Failure::Final(why)) => {
                tracing::warn!(target: "placement", "сообщение от «{from}» не передано колонке «{name}»: {why}");
                return;
            }
            Err(Failure::Again(why)) => {
                let Some(pause) = pauses.next() else {
                    tracing::warn!(
                        target: "placement",
                        "сообщение от «{from}» не передано колонке «{name}» за {tries} попыток: {why}"
                    );
                    return;
                };
                tracing::debug!(target: "placement", "a message from «{from}» to «{name}», try {tries}: {why}");
                tries += 1;
                tokio::time::sleep(pause).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU16, AtomicUsize, Ordering};
    use std::sync::Mutex;

    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::post;

    use super::*;
    use crate::settings::definitions::SettingsHome;

    const PAIR: &str = "0123456789abcdef0123456789abcdef";

    /// The station's gateway as the computer sees it: what reached its route, and how it answers.
    #[derive(Default)]
    struct Station {
        heard: Mutex<Vec<Value>>,
        tries: AtomicUsize,
        /// 0: 201.
        answer: AtomicU16,
    }

    async fn station(state: Arc<Station>) -> String {
        let route = move |headers: HeaderMap, axum::Json(body): axum::Json<Value>| {
            let state = state.clone();
            async move {
                state.tries.fetch_add(1, Ordering::SeqCst);
                let expected = format!("Bearer {PAIR}");
                if headers.get("authorization").and_then(|value| value.to_str().ok()) != Some(expected.as_str()) {
                    return Err(StatusCode::FORBIDDEN);
                }
                match state.answer.load(Ordering::SeqCst) {
                    0 => {
                        state.heard.lock().unwrap().push(body);
                        Ok((StatusCode::CREATED, axum::Json(json!({ "accepted": true }))))
                    }
                    code => Err(StatusCode::from_u16(code).unwrap()),
                }
            }
        };
        let router = axum::Router::new().route(MESSAGE_PATH, post(route));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router).await });
        base
    }

    fn relay(base: &str, pauses: usize) -> Arc<StationMessages> {
        let target = Target { name: "Детская".into(), base: base.to_string(), key: PAIR.into() };
        StationMessages::with(Arc::new(move || vec![target.clone()]), vec![Duration::from_millis(20); pauses])
    }

    fn text(from: &str, text: &str) -> IncomingMessage {
        IncomingMessage { from: from.into(), chat_id: "42".into(), file: None, text: Some(text.into()), discard: false }
    }

    async fn until(what: impl Fn() -> bool) -> bool {
        for _ in 0..200 {
            if what() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        false
    }

    #[tokio::test]
    async fn the_computer_hands_a_text_and_a_voice_message_to_the_station_it_helps() {
        let there = Arc::new(Station::default());
        let relay = relay(&station(there.clone()).await, 0);
        assert_eq!(relay.pass_on(&text("мама", "Скоро буду")), 1);
        assert!(until(|| there.heard.lock().unwrap().len() == 1).await);
        let heard = there.heard.lock().unwrap()[0].clone();
        assert_eq!((&heard["from"], &heard["text"]), (&json!("мама"), &json!("Скоро буду")));
        assert!(heard["at"].as_str().is_some_and(|at| at.starts_with("20")), "{heard}");
        assert!(heard.get("voice").is_none());

        // A voice message: its Ogg/Opus as it is, from the bot's inbox (which keeps it).
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("voice.ogg");
        std::fs::write(&file, b"OggS-voice").unwrap();
        let voice = IncomingMessage { file: Some(file.clone()), text: None, ..text("папа", "") };
        assert_eq!(relay.pass_on(&voice), 1);
        assert!(until(|| there.heard.lock().unwrap().len() == 2).await);
        let heard = there.heard.lock().unwrap()[1].clone();
        use base64::Engine as _;
        let audio = base64::engine::general_purpose::STANDARD.decode(heard["voice"].as_str().unwrap()).unwrap();
        assert_eq!((heard["from"].as_str(), audio.as_slice()), (Some("папа"), b"OggS-voice".as_slice()));
        assert!(file.exists(), "the bot's inbox keeps it");

        // Nothing to say: nothing handed on.
        assert_eq!(relay.pass_on(&IncomingMessage { text: None, ..text("мама", "") }), 0);
    }

    #[tokio::test]
    async fn a_station_away_or_busy_is_tried_again_one_that_refuses_is_not_and_neither_panics() {
        // Away: nobody at the port. The tries run out, the log says whose message it was.
        let closed = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://{}", listener.local_addr().unwrap())
        };
        let target = Target { name: "Детская".into(), base: closed, key: PAIR.into() };
        deliver(target, text("мама", "Скоро буду"), now_iso(), vec![Duration::from_millis(10); 2]).await;

        // Busy (503), then back: taken on a later try.
        let busy = Arc::new(Station::default());
        busy.answer.store(503, Ordering::SeqCst);
        let base = station(busy.clone()).await;
        let target = Target { name: "Детская".into(), base: base.clone(), key: PAIR.into() };
        let waiting =
            tokio::spawn(deliver(target, text("мама", "Скоро буду"), now_iso(), vec![Duration::from_millis(50); 4]));
        assert!(until(|| busy.tries.load(Ordering::SeqCst) >= 1).await);
        busy.answer.store(0, Ordering::SeqCst);
        waiting.await.unwrap();
        assert_eq!(busy.heard.lock().unwrap().len(), 1);

        // Refused (409: «Сообщения» off there): one try only.
        let refusing = Arc::new(Station::default());
        refusing.answer.store(409, Ordering::SeqCst);
        let target = Target { name: "Детская".into(), base: station(refusing.clone()).await, key: PAIR.into() };
        deliver(target, text("мама", "Скоро буду"), now_iso(), vec![Duration::from_millis(10); 4]).await;
        assert_eq!(refusing.tries.load(Ordering::SeqCst), 1);

        // A voice message over the cap stays on the computer: no try at all.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("long.ogg");
        std::fs::write(&file, vec![0u8; VOICE_LIMIT + 1]).unwrap();
        let fresh = Arc::new(Station::default());
        let target = Target { name: "Детская".into(), base: station(fresh.clone()).await, key: PAIR.into() };
        let long = IncomingMessage { file: Some(file), text: None, ..text("мама", "") };
        deliver(target, long, now_iso(), Vec::new()).await;
        assert_eq!(fresh.tries.load(Ordering::SeqCst), 0);
    }

    fn record(placement: Placement) -> StationPlacement {
        StationPlacement {
            placement,
            address: "192.0.2.5".into(),
            port: 8790,
            device: None,
            settings_home: SettingsHome::Station,
            auto_update: false,
            managed: true,
            home_key_id: None,
            moved_at: None,
        }
    }

    /// One switch for both ways: «Сообщения» off in «Что даю колонке» — nothing goes to the station.
    #[test]
    fn only_a_station_that_takes_messages_from_this_computer_gets_them() {
        let helped = record(Placement::StationServer);
        let health = |home: Value| json!({ "placement": { "homeServer": home } });
        assert!(takes_messages(&helped, Some(&health(json!({ "enabled": true, "state": "up" })))));
        assert!(takes_messages(&helped, Some(&health(json!({ "enabled": true, "without": ["llm"] })))));
        assert!(!takes_messages(&helped, Some(&health(json!({ "enabled": true, "without": ["messages"] })))));
        assert!(!takes_messages(&helped, Some(&health(json!({ "enabled": false })))), "the help off");
        assert!(!takes_messages(&helped, Some(&health(Value::Null))), "no home server there");
        assert!(takes_messages(&helped, None), "not heard yet: the station decides");
        assert!(!takes_messages(&record(Placement::Station), None), "alone on the station");
        assert!(!takes_messages(&record(Placement::Pc), None), "at this computer's /uni.ws: said here");
    }
}
