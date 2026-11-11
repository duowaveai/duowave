//! Voice messages through a Telegram bot: "передай маме…" sends a message to a contact, and a contact's voice or
//! text messages come to the station. Only the bot's contacts (set up in the panel) are heard: a stranger cannot
//! play anything on the station — the panel only sees that someone wrote, to add them in a tap. The bot is set up
//! in the panel and changes on the go (`reconfigure`). Calls in Telegram need a user account (MTProto), not a bot:
//! an open decision.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::Duration;

use anyhow::{anyhow, bail, Context as _, Result};
use serde::Serialize;
use serde_json::{json, Value};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::config::TelegramConfig;
use crate::skills::media::sources::match_score;
use crate::utils::time::now_iso;

pub const TELEGRAM_API: &str = "https://api.telegram.org";
/// 25 s, not longer: some networks reset a connection silent for ~30 s (192.168.8.x, read 02.11: ECONNRESET at
/// 31 s on an idle TLS connection to the API), and a long poll is silent until its end.
const POLL_SECONDS: u64 = 25;

/// A contact's message: a voice message's file (Ogg/Opus, as Telegram keeps them), or a text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingMessage {
    /// The contact's name as TELEGRAM_CONTACTS writes it (lower case).
    pub from: String,
    pub chat_id: String,
    pub file: Option<PathBuf>,
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contact {
    pub name: String,
    pub chat_id: String,
}

/// A link button under a text («Ответить» on a call from the station).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkButton {
    pub text: String,
    pub url: String,
}

type Listener = Arc<dyn Fn(&IncomingMessage) + Send + Sync>;

/// Someone who wrote to the bot and is no contact: the panel offers to add them. Kept in memory only, never logged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Stranger {
    /// The panel's handle for them: the chat itself never leaves the gateway.
    pub id: String,
    /// The name they gave Telegram.
    pub name: String,
    /// The chat's last digits.
    pub hint: String,
    pub at: String,
    #[serde(skip)]
    pub chat_id: String,
}

/// The strangers kept: the latest ones.
const STRANGERS: usize = 10;

pub struct TelegramMessenger {
    config: RwLock<Option<TelegramConfig>>,
    api: String,
    client: reqwest::Client,
    running: AtomicBool,
    offset: AtomicI64,
    stop: Mutex<CancellationToken>,
    /// The polling task: another bot polls only once it has ended.
    poll: Mutex<Option<JoinHandle<()>>>,
    listeners: Mutex<Vec<Listener>>,
    strangers: Mutex<VecDeque<Stranger>>,
    next_stranger: AtomicU64,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A chat's name as Telegram gives it: a person's first and last name, a group's title.
fn chat_name(message: &Value) -> String {
    let text =
        |pointer: &str| message.pointer(pointer).and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty());
    let person =
        [text("/chat/first_name"), text("/chat/last_name")].into_iter().flatten().collect::<Vec<_>>().join(" ");
    if !person.is_empty() {
        return person;
    }
    text("/chat/title").or_else(|| text("/from/first_name")).unwrap_or("Без имени").to_string()
}

/// A request's failure without its URL: the URL carries the bot's token.
fn reason(error: reqwest::Error) -> anyhow::Error {
    anyhow::Error::new(error.without_url())
}

impl TelegramMessenger {
    pub fn new(config: Option<TelegramConfig>) -> Arc<TelegramMessenger> {
        TelegramMessenger::with_api(config, TELEGRAM_API)
    }

    /// Against another Bot API server: a test's fake, a local Bot API server.
    pub fn with_api(config: Option<TelegramConfig>, api: &str) -> Arc<TelegramMessenger> {
        Arc::new(TelegramMessenger {
            config: RwLock::new(config),
            api: api.trim_end_matches('/').to_string(),
            client: crate::utils::http::client(),
            running: AtomicBool::new(false),
            offset: AtomicI64::new(0),
            stop: Mutex::new(CancellationToken::new()),
            poll: Mutex::new(None),
            listeners: Mutex::default(),
            strangers: Mutex::default(),
            next_stranger: AtomicU64::new(1),
        })
    }

    fn current(&self) -> Option<TelegramConfig> {
        self.config.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    pub fn enabled(&self) -> bool {
        self.current().is_some_and(|config| !config.token.is_empty())
    }

    fn config(&self) -> Result<TelegramConfig> {
        self.current()
            .filter(|config| !config.token.is_empty())
            .ok_or_else(|| anyhow!("no Telegram bot: set it up in the panel («Навыки» → «Подключения»)"))
    }

    /// Where the station's alerts go, when the bot has a chat for them.
    pub fn alert_chat(&self) -> Option<String> {
        self.current().and_then(|config| config.alert_chat).filter(|chat| !chat.is_empty())
    }

    /// The panel's change. The same bot keeps polling with its new contacts and alert chat; another token (or
    /// none) ends the polling first — waits for its task, so two bots never poll at once — and starts it again from
    /// the new bot's first update: update numbers are each bot's own.
    pub async fn reconfigure(self: &Arc<Self>, config: Option<TelegramConfig>) {
        let config = config.filter(|config| !config.token.is_empty());
        let before = self.current();
        let same_bot = before.as_ref().map(|config| &config.token) == config.as_ref().map(|config| &config.token);
        if same_bot {
            *self.config.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = config;
            return;
        }
        self.stop();
        let task = lock(&self.poll).take();
        if let Some(mut task) = task {
            if tokio::time::timeout(Duration::from_secs(10), &mut task).await.is_err() {
                task.abort();
            }
        }
        self.offset.store(0, Ordering::SeqCst);
        lock(&self.strangers).clear();
        *self.config.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = config;
        self.start();
    }

    /// Who wrote to the bot lately and is no contact, the latest first.
    pub fn strangers(&self) -> Vec<Stranger> {
        lock(&self.strangers).iter().rev().cloned().collect()
    }

    /// A stranger by the panel's handle.
    pub fn stranger(&self, id: &str) -> Option<Stranger> {
        lock(&self.strangers).iter().find(|stranger| stranger.id == id).cloned()
    }

    fn note_stranger(&self, chat_id: &str, name: String) {
        let mut strangers = lock(&self.strangers);
        strangers.retain(|known| known.chat_id != chat_id);
        let id = format!("s{}", self.next_stranger.fetch_add(1, Ordering::Relaxed));
        let hint = crate::connectors::settings::contact_hint(chat_id);
        strangers.push_back(Stranger { id, name, hint, at: now_iso(), chat_id: chat_id.to_string() });
        while strangers.len() > STRANGERS {
            strangers.pop_front();
        }
    }

    fn method(&self, token: &str, method: &str) -> String {
        format!("{}/bot{token}/{method}", self.api)
    }

    /// Every contact's message from now on, as it comes (the TS messenger's 'message' event): at once, from the
    /// polling task; long work goes to a task of its own.
    pub fn on_message(&self, listener: impl Fn(&IncomingMessage) + Send + Sync + 'static) {
        lock(&self.listeners).push(Arc::new(listener));
    }

    /// A contact by the name people say («маме» finds «мама»).
    pub fn contact(&self, name: &str) -> Option<Contact> {
        let config = self.current()?;
        let mut scored: Vec<(f64, &String, &String)> = config
            .contacts
            .iter()
            .map(|(contact, chat_id)| (match_score(name, contact), contact, chat_id))
            .filter(|(score, _, _)| *score >= 0.5)
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.first().map(|(_, name, chat_id)| Contact { name: (*name).clone(), chat_id: (*chat_id).clone() })
    }

    /// The contacts' names as set up, without their chats: what the station's gateway may know (src/home_server).
    pub fn contact_names(&self) -> Vec<String> {
        self.current().map(|config| config.contacts.into_iter().map(|(name, _)| name).collect()).unwrap_or_default()
    }

    pub fn start(self: &Arc<Self>) {
        let Some(config) = self.current().filter(|_| self.enabled()) else { return };
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }
        let stop = CancellationToken::new();
        *lock(&self.stop) = stop.clone();
        tracing::info!("bot polling; contacts: {}", config.contacts.len());
        *lock(&self.poll) = Some(tokio::spawn(self.clone().poll(stop)));
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
        lock(&self.stop).cancel();
    }

    /// A text, with a link button under it when given.
    pub async fn send_text(&self, chat_id: &str, text: &str, button: Option<&LinkButton>) -> Result<()> {
        let config = self.config()?;
        let mut body = json!({ "chat_id": chat_id, "text": text });
        if let Some(button) = button {
            body["reply_markup"] = json!({ "inline_keyboard": [[{ "text": button.text, "url": button.url }]] });
        }
        let response = self
            .client
            .post(self.method(&config.token, "sendMessage"))
            .json(&body)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(reason)?;
        if !response.status().is_success() {
            bail!("Telegram sendMessage: HTTP {}", response.status().as_u16());
        }
        Ok(())
    }

    pub async fn send_voice(&self, chat_id: &str, ogg: Vec<u8>, caption: Option<&str>) -> Result<()> {
        let config = self.config()?;
        let voice = reqwest::multipart::Part::bytes(ogg).file_name("message.ogg").mime_str("audio/ogg")?;
        let mut form = reqwest::multipart::Form::new().text("chat_id", chat_id.to_string());
        if let Some(caption) = caption.filter(|caption| !caption.is_empty()) {
            form = form.text("caption", caption.to_string());
        }
        let form = form.part("voice", voice);
        let response = self
            .client
            .post(self.method(&config.token, "sendVoice"))
            .multipart(form)
            .timeout(Duration::from_secs(60))
            .send()
            .await
            .map_err(reason)?;
        if !response.status().is_success() {
            bail!("Telegram sendVoice: HTTP {}", response.status().as_u16());
        }
        Ok(())
    }

    async fn poll(self: Arc<Self>, stop: CancellationToken) {
        while self.running.load(Ordering::SeqCst) {
            let updates = tokio::select! {
                _ = stop.cancelled() => return,
                updates = self.updates() => updates,
            };
            match updates {
                Ok(updates) => {
                    for update in updates {
                        if let Some(id) = update.get("update_id").and_then(Value::as_i64) {
                            self.offset.store(id + 1, Ordering::SeqCst);
                        }
                        if let Err(error) = self.on_update(&update).await {
                            tracing::warn!("message: {error:#}");
                        }
                    }
                }
                Err(error) => {
                    if !self.running.load(Ordering::SeqCst) {
                        return;
                    }
                    tracing::warn!("polling: {error:#}");
                    tokio::select! {
                        _ = stop.cancelled() => return,
                        _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                    }
                }
            }
        }
    }

    async fn updates(&self) -> Result<Vec<Value>> {
        let config = self.config()?;
        let url = format!(
            "{}?timeout={POLL_SECONDS}&offset={}",
            self.method(&config.token, "getUpdates"),
            self.offset.load(Ordering::SeqCst)
        );
        let response =
            self.client.get(url).timeout(Duration::from_secs(POLL_SECONDS + 15)).send().await.map_err(reason)?;
        let body: Value = response.json().await.map_err(reason)?;
        // A refusal (a wrong token) answers at once: as a failure, so the loop waits before it asks again.
        if body.get("ok") == Some(&Value::Bool(false)) {
            let description = body.get("description").and_then(Value::as_str).unwrap_or("refused");
            bail!("Telegram getUpdates: {description}");
        }
        Ok(body.get("result").and_then(Value::as_array).cloned().unwrap_or_default())
    }

    async fn on_update(&self, update: &Value) -> Result<()> {
        let config = self.config()?;
        let Some(message) = update.get("message") else { return Ok(()) };
        let chat_id = match message.pointer("/chat/id") {
            Some(Value::Number(id)) => id.to_string(),
            Some(Value::String(id)) => id.clone(),
            _ => return Ok(()),
        };
        let Some(from) = config.contacts.iter().find(|(_, id)| **id == chat_id).map(|(name, _)| name.clone()) else {
            tracing::warn!("a message from a chat that is not a contact: ignored (the panel offers to add it)");
            self.note_stranger(&chat_id, chat_name(message));
            return Ok(());
        };
        let file_id =
            message.pointer("/voice/file_id").or_else(|| message.pointer("/audio/file_id")).and_then(Value::as_str);
        let incoming = if let Some(file_id) = file_id {
            let file = self.download(&config, file_id, &from).await?;
            IncomingMessage { from, chat_id, file: Some(file), text: None }
        } else if let Some(text) = message.get("text").and_then(Value::as_str).filter(|text| !text.is_empty()) {
            IncomingMessage { from, chat_id, file: None, text: Some(text.to_string()) }
        } else {
            return Ok(());
        };
        let listeners = lock(&self.listeners).clone();
        for listener in listeners {
            listener(&incoming);
        }
        Ok(())
    }

    async fn download(&self, config: &TelegramConfig, file_id: &str, from: &str) -> Result<PathBuf> {
        let info: Value = self
            .client
            .get(self.method(&config.token, "getFile"))
            .query(&[("file_id", file_id)])
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(reason)?
            .json()
            .await
            .map_err(reason)?;
        let Some(path) = info.pointer("/result/file_path").and_then(Value::as_str) else {
            bail!("Telegram gave no file path");
        };
        let response = self
            .client
            .get(format!("{}/file/bot{}/{path}", self.api, config.token))
            .timeout(Duration::from_secs(60))
            .send()
            .await
            .map_err(reason)?;
        if !response.status().is_success() {
            bail!("Telegram file: HTTP {}", response.status().as_u16());
        }
        let bytes = response.bytes().await.map_err(reason)?;
        std::fs::create_dir_all(&config.inbox_dir)
            .with_context(|| format!("the inbox {}", config.inbox_dir.display()))?;
        let file = config.inbox_dir.join(inbox_name(&now_iso(), from));
        std::fs::write(&file, &bytes).with_context(|| format!("{}", file.display()))?;
        // The family's voice: for the gateway alone.
        crate::utils::files::owner_only(&file);
        Ok(file)
    }
}

/// `2026-11-06T02-11-00-331Z_мама.ogg`: the time without colons and dots, the contact's letters and digits.
fn inbox_name(iso: &str, from: &str) -> String {
    let time: String = iso.chars().map(|c| if c == ':' || c == '.' { '-' } else { c }).collect();
    let mut name = String::new();
    let mut gap = false;
    for c in from.chars() {
        if c.is_alphanumeric() {
            if gap {
                name.push('-');
            }
            gap = false;
            name.push(c);
        } else {
            gap = true;
        }
    }
    if gap {
        name.push('-');
    }
    format!("{time}_{name}.ogg")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use axum::extract::{Query, State};
    use axum::routing::{get, post};
    use axum::{Json, Router};

    use super::*;

    #[derive(Default)]
    struct Fake {
        texts: Mutex<Vec<Value>>,
        voices: Mutex<Vec<(String, Vec<u8>)>>,
        offsets: Mutex<Vec<i64>>,
    }

    /// The Bot API as far as the messenger uses it, for the bot "T": one batch of updates, then nothing.
    async fn fake_telegram() -> (String, Arc<Fake>) {
        let fake = Arc::new(Fake::default());
        let app = Router::new()
            .route(
                "/botT/sendMessage",
                post(|State(fake): State<Arc<Fake>>, Json(body): Json<Value>| async move {
                    lock(&fake.texts).push(body);
                    Json(json!({"ok": true}))
                }),
            )
            .route(
                "/botT/sendVoice",
                post(|State(fake): State<Arc<Fake>>, headers: axum::http::HeaderMap, body: axum::body::Bytes| async move {
                    let kind = headers.get("content-type").and_then(|value| value.to_str().ok()).unwrap_or_default();
                    lock(&fake.voices).push((kind.to_string(), body.to_vec()));
                    Json(json!({"ok": true}))
                }),
            )
            .route(
                "/botT/getUpdates",
                get(|State(fake): State<Arc<Fake>>, Query(query): Query<BTreeMap<String, String>>| async move {
                    let offset: i64 = query.get("offset").and_then(|offset| offset.parse().ok()).unwrap_or(0);
                    lock(&fake.offsets).push(offset);
                    if offset > 0 {
                        tokio::time::sleep(Duration::from_millis(200)).await;
                        return Json(json!({"ok": true, "result": []}));
                    }
                    Json(json!({"ok": true, "result": [
                        {"update_id": 7, "message": {"chat": {"id": 42}, "voice": {"file_id": "v1"}}},
                        {"update_id": 8, "message": {"chat": {"id": 66_600_123, "first_name": "Сосед", "last_name": "Пётр"}, "text": "я не контакт"}},
                        {"update_id": 9, "message": {"chat": {"id": 42}, "text": "Приду в семь"}},
                    ]}))
                }),
            )
            // Another bot, «U»: its own update numbers, from 3.
            .route(
                "/botU/getUpdates",
                get(|State(fake): State<Arc<Fake>>, Query(query): Query<BTreeMap<String, String>>| async move {
                    let offset: i64 = query.get("offset").and_then(|offset| offset.parse().ok()).unwrap_or(0);
                    lock(&fake.offsets).push(-offset - 1000);
                    if offset > 0 {
                        tokio::time::sleep(Duration::from_millis(200)).await;
                        return Json(json!({"ok": true, "result": []}));
                    }
                    Json(json!({"ok": true, "result": [
                        {"update_id": 3, "message": {"chat": {"id": 42}, "text": "Это новый бот"}},
                    ]}))
                }),
            )
            .route(
                "/botT/getFile",
                get(|Query(query): Query<BTreeMap<String, String>>| async move {
                    assert_eq!(query.get("file_id").map(String::as_str), Some("v1"));
                    Json(json!({"ok": true, "result": {"file_path": "voice/file_1.oga"}}))
                }),
            )
            .route("/file/botT/voice/file_1.oga", get(|| async { b"OggS-voice".to_vec() }))
            .with_state(fake.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (url, fake)
    }

    fn config(inbox: PathBuf) -> TelegramConfig {
        TelegramConfig {
            token: "T".into(),
            contacts: vec![("мама".to_string(), "42".to_string()), ("папа".to_string(), "43".to_string())],
            inbox_dir: inbox,
            alert_chat: Some("43".into()),
        }
    }

    #[tokio::test]
    async fn sends_texts_with_a_link_button_and_voice_messages() {
        let (api, fake) = fake_telegram().await;
        let dir = tempfile::tempdir().unwrap();
        let telegram = TelegramMessenger::with_api(Some(config(dir.path().into())), &api);
        let button =
            LinkButton { text: "Ответить".into(), url: "https://calls.example/calls/livekit#access=x".into() };
        telegram.send_text("42", "Вам звонит Колонка.", Some(&button)).await.unwrap();
        telegram.send_text("43", "Передала.", None).await.unwrap();
        assert_eq!(
            *lock(&fake.texts),
            vec![
                json!({"chat_id": "42", "text": "Вам звонит Колонка.", "reply_markup": {"inline_keyboard": [[{"text": "Ответить", "url": "https://calls.example/calls/livekit#access=x"}]]}}),
                json!({"chat_id": "43", "text": "Передала."}),
            ]
        );
        telegram.send_voice("42", b"OggS".to_vec(), Some("от колонки")).await.unwrap();
        let (kind, body) = lock(&fake.voices)[0].clone();
        assert!(kind.starts_with("multipart/form-data; boundary="), "{kind}");
        let body = String::from_utf8_lossy(&body).to_string();
        for part in [
            "name=\"chat_id\"\r\n\r\n42\r\n",
            "name=\"caption\"\r\n\r\nот колонки\r\n",
            "name=\"voice\"; filename=\"message.ogg\"\r\nContent-Type: audio/ogg\r\n\r\nOggS\r\n",
        ] {
            assert!(body.contains(part), "{part:?} in {body:?}");
        }

        let missing = TelegramMessenger::with_api(None, &api);
        assert!(!missing.enabled());
        assert!(missing.send_text("42", "x", None).await.is_err());
    }

    #[tokio::test]
    async fn hears_only_its_contacts_and_keeps_their_voice_messages_in_the_inbox() {
        let (api, fake) = fake_telegram().await;
        let dir = tempfile::tempdir().unwrap();
        let telegram = TelegramMessenger::with_api(Some(config(dir.path().join("messages"))), &api);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        telegram.on_message(move |message| {
            let _ = tx.send(message.clone());
        });
        telegram.start();
        let voice = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap();
        let text = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap();
        telegram.stop();
        assert_eq!((voice.from.as_str(), voice.chat_id.as_str()), ("мама", "42"));
        let file = voice.file.unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"OggS-voice");
        assert!(file.starts_with(dir.path().join("messages")));
        assert!(file.to_string_lossy().ends_with("_мама.ogg"));
        assert_eq!(
            text,
            IncomingMessage {
                from: "мама".into(),
                chat_id: "42".into(),
                file: None,
                text: Some("Приду в семь".into())
            }
        );
        // The stranger's message was read past, and the next poll asks after the last update.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(lock(&fake.offsets).first(), Some(&0));
        assert!(lock(&fake.offsets).iter().skip(1).all(|offset| *offset == 10));
        // The panel sees who wrote: a name and the chat's last digits, never the chat.
        let strangers = telegram.strangers();
        assert_eq!(strangers.len(), 1);
        assert_eq!((strangers[0].name.as_str(), strangers[0].hint.as_str()), ("Сосед Пётр", "…0123"));
        assert_eq!(telegram.stranger(&strangers[0].id).unwrap().chat_id, "66600123");
        assert!(!serde_json::to_string(&strangers).unwrap().contains("66600123"));
    }

    #[tokio::test]
    async fn another_bot_polls_from_its_own_first_update_and_the_listeners_stay() {
        let (api, fake) = fake_telegram().await;
        let dir = tempfile::tempdir().unwrap();
        let telegram = TelegramMessenger::with_api(Some(config(dir.path().join("messages"))), &api);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        telegram.on_message(move |message| {
            let _ = tx.send(message.clone());
        });
        telegram.start();
        for _ in 0..2 {
            tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap();
        }
        // Contacts and the alert chat change in place: the same bot goes on.
        let mut same = config(dir.path().join("messages"));
        same.alert_chat = Some("42".into());
        telegram.reconfigure(Some(same.clone())).await;
        assert_eq!(telegram.alert_chat().as_deref(), Some("42"));
        assert!(!telegram.strangers().is_empty(), "the same bot keeps who wrote to it");
        // Another token: the old polling ends, the new bot starts from its first update.
        let mut other = same;
        other.token = "U".into();
        telegram.reconfigure(Some(other)).await;
        let message = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap();
        assert_eq!(message.text.as_deref(), Some("Это новый бот"));
        assert!(telegram.strangers().is_empty(), "another bot: nobody wrote to it yet");
        assert_eq!(lock(&fake.offsets).iter().find(|offset| **offset <= -1000), Some(&-1000), "asked from 0");
        // No bot at all: nothing polls.
        telegram.reconfigure(None).await;
        assert!(!telegram.enabled());
        tokio::time::sleep(Duration::from_millis(100)).await;
        let polls = lock(&fake.offsets).len();
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(lock(&fake.offsets).len(), polls);
    }

    #[test]
    fn finds_a_contact_by_the_name_people_say() {
        let telegram = TelegramMessenger::new(Some(config(PathBuf::from("/nowhere"))));
        assert_eq!(telegram.contact("маме"), Some(Contact { name: "мама".into(), chat_id: "42".into() }));
        assert_eq!(telegram.contact("бабушке"), None);
        assert!(TelegramMessenger::new(None).contact("маме").is_none());
        // A tie goes to the contact TELEGRAM_CONTACTS lists first, as in the TS: not the first by the alphabet.
        let mut tied = config(PathBuf::from("/nowhere"));
        tied.contacts = vec![("мама света".to_string(), "1".to_string()), ("мама".to_string(), "2".to_string())];
        assert_eq!(match_score("маме", "мама света"), match_score("маме", "мама"));
        assert_eq!(TelegramMessenger::new(Some(tied)).contact("маме").map(|found| found.chat_id).as_deref(), Some("1"));
        assert_eq!(inbox_name("2026-11-06T02:11:00.331Z", "тётя Валя!"), "2026-11-06T02-11-00-331Z_тётя-Валя-.ogg");
    }
}
