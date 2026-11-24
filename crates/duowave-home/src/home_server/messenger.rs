//! «передай маме, что …» on the station: its build has no Telegram bot (the calls module is the computer's), so the
//! message goes through a helper's bot — the first helper that gives messages (src/helpers, «Сообщения»), the computer
//! at home now: `POST /home/v1/messenger/send` with its key (serve.rs). The names of the helper's contacts come with
//! each look at it (`RemoteHelper::read_contacts`), so the skill knows at once whether a name is ours; their chats stay
//! on the helper. A helper at home is never kept out by the regulator, so «передай …» passes under every position of
//! it; the bot's own way to Telegram is no road of the regulator's.
//!
//! A gateway with a bot of its own (the computer's build) uses its own: this one stands aside then.
//!
//! The other way: the bot's messages from the contacts come from the computer (src/placement/messages.rs) to
//! `POST /api/v1/home-server/message` here, with the pair's key, and the calls skill says or plays them. The pair's
//! door's part «Сообщения» («Что даю колонке») governs both ways.
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use axum::body::Bytes;
use axum::http::StatusCode;
use futures::future::BoxFuture;
use serde_json::{json, Value};

use crate::config::Config;
use crate::core::identity::permissions::STATIONS_MANAGE;
use crate::helpers::contract::Ability;
use crate::helpers::remote::{Messenger, RemoteHelper};
use crate::helpers::{Giver, Helpers};
use crate::home_server::HomeServerService;
use crate::placement::PlacementService;
use crate::platform::http::{created, ApiError, ApiResult, Caller, Uses};
use crate::skills::media::sources::match_score;
use crate::skills::own::calls_skill::{Contact, IncomingMessage, LinkButton, MessengerAccess, NotSent, NO_RECIPIENT};
use crate::skills::SkillsService;

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
pub fn not_taken() -> String {
    crate::tr!(
        "Колонка не берёт сообщения у компьютера: в её «Доме» у компьютера выключены «Сообщения» («Не брать у него…»)."
    )
}
pub fn only_pair() -> String {
    crate::tr!("Сообщения колонке передаёт только компьютер пары.")
}
pub fn busy() -> String {
    crate::tr!("Колонка ещё не проиграла прежние голосовые сообщения.")
}

/// What the station says when the message cannot go.
pub const AWAY: &str = "Компьютер не на связи — передать сообщение некому.";
pub const NO_BOT: &str = "На компьютере не настроен бот Telegram.";
pub const NOT_ACCEPTED: &str = "Telegram не принял сообщение.";
pub const KEY_REFUSED: &str = "Компьютер не принимает ключ колонки.";
pub const LATE: &str = "Компьютер не ответил вовремя — не знаю, ушло ли сообщение.";
pub const FAILED: &str = "Компьютер не смог передать сообщение.";

/// Telegram's own wait on the computer is 30 s; a message rarely takes a second.
const SEND_WAIT: Duration = Duration::from_secs(20);

/// The skill's messenger: this gateway's own bot when it has one, otherwise a helper's.
pub struct HomeMessenger {
    local: Arc<dyn MessengerAccess>,
    helpers: Arc<Helpers>,
}

impl HomeMessenger {
    pub fn new(local: Arc<dyn MessengerAccess>, helpers: Arc<Helpers>) -> Arc<HomeMessenger> {
        Arc::new(HomeMessenger { local, helpers })
    }

    /// The helper that would pass messages on, on the line or not.
    fn offered(&self) -> Option<Arc<RemoteHelper>> {
        match self.helpers.offer(Ability::Messages)? {
            Giver::Helper(remote) => Some(remote),
            Giver::Itself => None,
        }
    }

    /// A helper's bot is the one: none here, and a helper would pass messages on.
    fn through_home(&self) -> bool {
        !self.local.enabled() && self.offered().is_some()
    }

    /// The names of the helper's contacts: the one that passes messages now, else the last seen of the one that would.
    fn names(&self) -> Option<Vec<String>> {
        if !self.through_home() {
            return None;
        }
        let helper = self.helpers.pick_remote(Ability::Messages).or_else(|| self.offered())?;
        match helper.messenger() {
            Messenger::Contacts(names) => Some(names),
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

    /// The contact's «chat» is its name on the helper: the helper's bot finds the chat by it.
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
        if self.helpers.pick_remote(Ability::Messages).is_some() {
            return None;
        }
        match self.offered() {
            Some(helper) if helper.up() && helper.messenger() == Messenger::NoBot => Some(NO_BOT.into()),
            _ => Some(AWAY.into()),
        }
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
        let helper = self.helpers.pick_remote(Ability::Messages).map(|helper| (helper.base(), helper.credential()));
        Box::pin(async move {
            // A call's link is the calls module's, and that stays on the computer.
            if button.is_some() {
                anyhow::bail!("a call's link through the computer: not supported");
            }
            let Some((url, key)) = helper else { return Err(NotSent(AWAY.into()).into()) };
            let answer = crate::utils::http::client_without_redirects()
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
            tracing::warn!("helpers: a message not passed on: HTTP {status}: {}", message.unwrap_or("-"));
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
fn waiting_room(config: &Arc<Config>) -> PathBuf {
    config
        .data_dir
        .as_ref()
        .map(|dir| dir.join("tmp").join("messages"))
        .unwrap_or_else(|| std::env::temp_dir().join("duowave-messages"))
}

/// The copies waiting now; one older than `STALE` is removed (its playing was cut short).
pub fn waiting(dir: &Path) -> usize {
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
pub async fn receive(
    Uses(placement): Uses<Arc<PlacementService>>,
    Uses(home_server): Uses<Arc<HomeServerService>>,
    Uses(config): Uses<Arc<Config>>,
    Uses(skills): Uses<Arc<SkillsService>>,
    caller: Caller,
    body: Bytes,
) -> ApiResult<Value> {
    if !crate::placement::peer::from_peer(&caller, placement.peer_key()) {
        return Err(ApiError::forbidden(only_pair()));
    }
    if crate::placement::peer::through_pass(&caller) {
        caller.permitted(STATIONS_MANAGE)?;
    }
    if home_server.configured().is_none() || !home_server.serves("messages") {
        return Err(ApiError::conflict(not_taken()));
    }
    let request: Value = serde_json::from_slice(&body)
        .map_err(|_| ApiError::bad_request(crate::tr!("JSON: {from, text | voice, at}")))?;
    let field = |name: &str| request.get(name).and_then(Value::as_str).map(str::trim).filter(|value| !value.is_empty());
    let from = field("from").ok_or_else(|| ApiError::bad_request(crate::tr!("from: от кого сообщение")))?.to_string();
    let text = field("text").map(str::to_string);
    let at = field("at").unwrap_or("-").to_string();
    let file = match field("voice") {
        None => None,
        Some(voice) => {
            use base64::Engine as _;
            let audio = base64::engine::general_purpose::STANDARD
                .decode(voice)
                .map_err(|_| ApiError::bad_request(crate::tr!("voice: base64 голосового сообщения")))?;
            if audio.len() > VOICE_LIMIT {
                return Err(ApiError::new(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    crate::tr!("Голосовое сообщение больше 2 МБ."),
                ));
            }
            let dir = waiting_room(&config);
            if waiting(&dir) >= WAITING {
                return Err(ApiError::unavailable(busy()));
            }
            let file = dir.join(format!("{}.ogg", hex::encode(crate::utils::ids::random_bytes(8))));
            tokio::fs::create_dir_all(&dir).await.map_err(ApiError::internal)?;
            tokio::fs::write(&file, &audio).await.map_err(ApiError::internal)?;
            Some(file)
        }
    };
    if file.is_none() && text.is_none() {
        return Err(ApiError::bad_request(crate::tr!("text или voice: что сказать")));
    }
    tracing::info!(
        "home server: a {} from «{from}» (at {at}) came from the computer's bot",
        if file.is_some() { "voice message" } else { "message" }
    );
    skills.calls.on_message(IncomingMessage { from, chat_id: String::new(), file, text, discard: true });
    Ok(created(json!({ "accepted": true })))
}
