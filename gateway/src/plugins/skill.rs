//! `PluginSkill`: the bridge from the router to the plugins. A plugin's phrase is an intent the router routes like
//! any quick rule's — its capability's route and the speaker's right to it — and executes through this skill (the
//! intent names the `plugins` capability as its executor). The plugin's function gets the phrase's slots and the
//! request; its answer becomes an ordinary `Outcome`:
//!
//! - a string: speech;
//! - `{say, play: [items], index, listen}`: speech, our player with those items (audio_play), listening after;
//! - `display: {temperature}`: the number on the station's display, as Yandex shows the weather;
//! - `model: {system, prompt, key?, fresh?}` (right `llm.chat`): the gateway's model says it, in the plugin's words —
//!   a game's host, a tale (Host API 1.1, stage 10);
//! - `continue: true` (or `{handler}`): the conversation goes on with the plugin without the wake word, the station
//!   listening, until it answers without `continue`;
//! - `continue: {handler, timeout: {seconds, handler | say}}`: the turn timer — the person says nothing for `seconds`
//!   after the station's speech ends, and the station says «время вышло» by itself: the plugin's function answers
//!   it as it answers words, or the ready `say` is said and the conversation ends (docs/plans/plugin-schedules.md,
//!   stage 8);
//! - nothing (`null`, `undefined`): it lets the words go — when they were a built-in rule's too, the built-in
//!   implementation serves them; so it does when the plugin fails.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use serde_json::{json, Map, Value};

use super::controller::PluginHost;
use super::manifest::Permission;
use super::media::media_items_with;
use super::phrases::REQUEST;
use super::runtime::CallKind;
use crate::skills::capabilities::{CapabilityName, Intent};
use crate::skills::media::player::MediaPlayer;
use crate::skills::router::{Router, FALLBACK};
use crate::skills::skill::{ContinuationRequest, ExecutionContext, Outcome, OwnSkill, PhraseStream};
use crate::utils::time::now_ms;

/// A conversation with a plugin goes on this long after the person's last words.
const GOING_MS: u64 = 3 * 60_000;

/// The turn timer's seconds, at least and at most.
const TURN_MIN_S: u64 = 3;
const TURN_MAX_S: u64 = 120;
/// Two «время вышло» in a row without a word from the person; the third silence ends the conversation with
/// «Похоже, никого нет — заканчиваю» (the owner's decision 7, 12.11.2026: a game must not talk to an empty room).
const TIMES_UP_IN_A_ROW: u32 = 2;
const NOBODY_THERE: &str = "Похоже, никого нет — заканчиваю.";
/// A timer that came due while the person's words are still being heard waits for them, this long at most.
const HEARING_WAIT: Duration = Duration::from_secs(20);
const HEARING_POLL: Duration = Duration::from_millis(300);

/// The turn timer («Время вышло!»): it stands apart from the scheduler (`schedule.rs`) and comes first (the owner's
/// decision 9, 12.11.2026) — it is small, and games show it at once. This is its way to the station: «время вышло» is
/// the answer to a push, so the plugin's answer to it plays, listens after and goes on as a phrase's answer does.
pub trait TurnStation: Send + Sync {
    /// Runs our `intent` as the answer to a push on the device's live station. `still` is asked when the station
    /// calls back: false, and nothing is answered. False when nothing was spoken to its end.
    fn ask(
        &self,
        device: &str,
        intent: Intent,
        label: String,
        still: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> BoxFuture<'static, bool>;
    /// The person's words are being heard on the device now.
    fn hearing(&self, device: &str) -> bool;
    /// An alarm or a timer rings on the device.
    fn ringing(&self, device: &str) -> bool;
    /// Someone is with the station or it plays: the workshop's «Готово» waits (workshop/voice.rs).
    fn busy(&self, device: &str) -> bool {
        self.hearing(device) || self.ringing(device)
    }
}

/// A device's conversation with a plugin.
#[derive(Clone)]
struct Going {
    plugin: String,
    /// The plugin's version when the conversation went on: a timer of another version is not run.
    version: Option<semver::Version>,
    handler: String,
    capability: CapabilityName,
    /// The person's last words: the conversation lasts GOING_MS after them, whatever the timer says.
    since: Instant,
    /// «Время вышло» said in a row since the person's last words.
    missed: u32,
    /// The turn timer, while one is set.
    turn: Option<Turn>,
}

#[derive(Clone)]
struct Turn {
    /// Which timer this is: a newer one, or the person's words, leave an older one with nothing to do.
    id: u64,
    seconds: u64,
    then: TimeUp,
    /// When the station stopped speaking and the count began (ms since the epoch); None while it still speaks.
    from_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
enum TimeUp {
    /// The plugin's function answers «время вышло».
    Handler(String),
    /// A ready phrase: said, and the conversation ends.
    Say(String),
}

/// The turn timer an answer's `continue` object asks for: none, or why it is not set.
fn turn_of(timeout: &Value) -> Result<Option<(u64, TimeUp)>, String> {
    if timeout.is_null() {
        return Ok(None);
    }
    let Some(seconds) = timeout.get("seconds").and_then(Value::as_f64) else {
        return Err("нет seconds".into());
    };
    if !(TURN_MIN_S as f64..=TURN_MAX_S as f64).contains(&seconds) {
        return Err(format!("seconds {seconds} — вне {TURN_MIN_S}–{TURN_MAX_S}"));
    }
    let seconds = seconds.round() as u64;
    if let Some(handler) = timeout.get("handler").and_then(Value::as_str).filter(|handler| !handler.trim().is_empty()) {
        return Ok(Some((seconds, TimeUp::Handler(handler.to_string()))));
    }
    match timeout.get("say").and_then(Value::as_str).map(str::trim).filter(|say| !say.is_empty()) {
        Some(say) => Ok(Some((seconds, TimeUp::Say(say.chars().take(1000).collect())))),
        None => Err("нужен handler или say".into()),
    }
}

/// About how long the station listens for words without the wake word: with nothing heard it closes its stream with
/// NO_SPEECH some 5 s after it opened it (station_session.rs, `send_partial`).
const STATION_LISTENS_S: u64 = 5;

/// What the workshop says of an answer's turn timer: set where it is not read, set wrong, or longer than the station
/// listens without «Алиса».
pub fn turn_warning(answer: &Value) -> Option<String> {
    if answer.get("timeout").is_some() {
        return Some("timeout — только внутри continue: {handler, timeout}; так таймер хода не встанет.".into());
    }
    match turn_of(answer.get("continue").and_then(|next| next.get("timeout"))?) {
        Err(why) => Some(format!("Таймер хода не встанет: {why}.")),
        Ok(Some((seconds, _))) if seconds > STATION_LISTENS_S => Some(format!(
            "Таймер хода {seconds} с: без «Алиса» колонка слушает около {STATION_LISTENS_S} с, под конец отвечать — с «Алиса»."
        )),
        Ok(_) => None,
    }
}

fn lock(going: &Mutex<HashMap<String, Going>>) -> MutexGuard<'_, HashMap<String, Going>> {
    going.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The turn timers' side that outlives a call: the conversations, the stations, the plugins.
#[derive(Clone)]
struct Timers {
    going: Arc<Mutex<HashMap<String, Going>>>,
    station: Arc<dyn TurnStation>,
    host: Weak<PluginHost>,
}

impl Timers {
    /// The station stopped speaking at `ended_ms`: the count begins.
    fn arm(&self, device: String, id: u64, ended_ms: u64) {
        let seconds = {
            let mut going = lock(&self.going);
            let Some(turn) = going.get_mut(&device).and_then(|going| going.turn.as_mut()).filter(|turn| turn.id == id)
            else {
                return;
            };
            turn.from_ms = Some(ended_ms);
            turn.seconds
        };
        let due_ms = ended_ms + seconds * 1000;
        let timers = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(due_ms.saturating_sub(now_ms()))).await;
            timers.fire(device, id, seconds, due_ms).await;
        });
    }

    /// The conversation, while this timer is still its own and the conversation lasts.
    fn current(&self, device: &str, id: u64) -> Option<Going> {
        lock(&self.going)
            .get(device)
            .filter(|going| {
                going.turn.as_ref().is_some_and(|turn| turn.id == id)
                    && going.since.elapsed() < Duration::from_millis(GOING_MS)
            })
            .cloned()
    }

    /// Takes this timer away and leaves the conversation as it is.
    fn drop_turn(&self, device: &str, id: u64) {
        if let Some(going) = lock(&self.going).get_mut(device) {
            if going.turn.as_ref().is_some_and(|turn| turn.id == id) {
                going.turn = None;
            }
        }
    }

    async fn fire(&self, device: String, id: u64, seconds: u64, due_ms: u64) {
        // Late by more than its own length (the Mac slept): nobody waits for this «время вышло» any more.
        if now_ms().saturating_sub(due_ms) > seconds * 1000 {
            return self.drop_turn(&device, id);
        }
        // Words are being heard: they answer the move, not the timer.
        let waited = Instant::now();
        while self.station.hearing(&device) && waited.elapsed() < HEARING_WAIT {
            tokio::time::sleep(HEARING_POLL).await;
        }
        let Some(going) = self.current(&device, id) else { return };
        if self.station.ringing(&device) {
            return self.drop_turn(&device, id);
        }
        // Off, gone or another version now: the plugin's old timer is not run.
        let live = self
            .host
            .upgrade()
            .and_then(|host| host.package(&going.plugin))
            .map(|package| package.manifest.version.clone());
        if live.is_none() || live != going.version {
            return self.drop_turn(&device, id);
        }
        let mut intent = Intent::new(CapabilityName::Plugins, "timeout");
        intent.params.insert("turn".into(), json!(id));
        let still = {
            let timers = self.clone();
            let device = device.clone();
            Arc::new(move || timers.current(&device, id).is_some()) as Arc<dyn Fn() -> bool + Send + Sync>
        };
        let label = format!("«время вышло» · {}", going.plugin);
        if !self.station.ask(&device, intent, label, still).await {
            self.drop_turn(&device, id);
        }
    }
}

/// Speech longer than this (in letters) plays before the answer's music, not with it.
const LONG_SPEECH: usize = 60;

/// About how long Alice says `letters` letters, with the time the station takes to begin.
fn speaking_time(letters: usize) -> Duration {
    Duration::from_millis(1500 + letters as u64 * 65)
}

/// The gateway's model for answers in a plugin's words: key, system, text, cancel, fresh.
pub type ModelHost =
    Arc<dyn Fn(String, String, String, tokio_util::sync::CancellationToken, bool) -> PhraseStream + Send + Sync>;

pub struct PluginSkill {
    host: Weak<PluginHost>,
    router: Weak<Router>,
    player: Arc<MediaPlayer>,
    /// The workshop by voice (workshop/voice.rs): «давай сделаем навык» and the conversation after it.
    voice: OnceLock<Arc<super::workshop::voice::VoiceWorkshop>>,
    /// The apps' history by voice (history/voice.rs): «верни погоду как было вчера» and its «да».
    history_voice: OnceLock<Arc<super::history::voice::VoiceHistory>>,
    /// The gateway's model, for `model` answers.
    model: OnceLock<ModelHost>,
    /// Devices whose conversation goes on with a plugin.
    going: Arc<Mutex<HashMap<String, Going>>>,
    /// Where the turn timer says «время вышло»: the session registry, once it stands.
    turns: OnceLock<Arc<dyn TurnStation>>,
    /// The next turn timer's number.
    next_turn: AtomicU64,
}

impl PluginSkill {
    pub fn new(host: Weak<PluginHost>, router: Weak<Router>, player: Arc<MediaPlayer>) -> PluginSkill {
        PluginSkill {
            host,
            router,
            player,
            voice: OnceLock::new(),
            history_voice: OnceLock::new(),
            model: OnceLock::new(),
            going: Arc::default(),
            turns: OnceLock::new(),
            next_turn: AtomicU64::new(1),
        }
    }

    /// The gateway's model for answers said in a plugin's words.
    pub fn set_model(&self, model: ModelHost) {
        let _ = self.model.set(model);
    }

    /// The stations, for the turn timer, once the session registry stands.
    pub fn set_turns(&self, turns: Arc<dyn TurnStation>) {
        let _ = self.turns.set(turns);
    }

    fn going(&self) -> MutexGuard<'_, HashMap<String, Going>> {
        lock(&self.going)
    }

    fn timers(&self) -> Option<Timers> {
        Some(Timers { going: self.going.clone(), station: self.turns.get()?.clone(), host: self.host.clone() })
    }

    /// The workshop by voice joins once it stands.
    pub fn set_voice(&self, voice: Arc<super::workshop::voice::VoiceWorkshop>) {
        let _ = self.voice.set(voice);
    }

    /// The history by voice joins once it stands.
    pub fn set_history_voice(&self, voice: Arc<super::history::voice::VoiceHistory>) {
        let _ = self.history_voice.set(voice);
    }

    /// The request as a plugin's function gets it, under `request`.
    fn request(context: &ExecutionContext, capability: CapabilityName) -> Value {
        json!({
            "text": context.text,
            "device": context.device_tag,
            "timezone": context.timezone,
            "capability": capability,
        })
    }

    /// The input of a plugin's function for an intent: the slots, and the request under `request`.
    fn input(intent: &Intent, context: &ExecutionContext) -> Value {
        let mut input = intent.params.get("slots").and_then(Value::as_object).cloned().unwrap_or_default();
        input.insert(REQUEST.into(), PluginSkill::request(context, intent.capability));
        Value::Object(input)
    }

    /// What the plugin's answer does; None when it lets the request go. `time_up`: the answer is to «время вышло»,
    /// the `missed`-th in a row — the conversation keeps its start and its count of silences.
    fn outcome(
        &self,
        plugin: &str,
        handler: &str,
        capability: CapabilityName,
        answer: Value,
        context: &ExecutionContext,
        time_up: Option<u32>,
    ) -> Option<Outcome> {
        let answer = match answer {
            Value::String(text) if !text.trim().is_empty() => {
                self.going().remove(&context.device_tag);
                return Some(Outcome::speech(text));
            }
            Value::Object(answer) => answer,
            _ => return None,
        };
        let host = self.host.upgrade()?;
        let manifest = host.package(plugin).map(|package| package.manifest.clone());
        let kind = super::media::kind_for(capability);
        let mut outcome = Outcome::default();
        if let Some(say) = answer.get("say").and_then(Value::as_str).map(str::trim).filter(|say| !say.is_empty()) {
            outcome.speech = Some(say.chars().take(1000).collect());
        }
        if let Some(celsius) =
            answer.get("display").and_then(|display| display.get("temperature")).and_then(Value::as_f64)
        {
            outcome.directives.extend(crate::skills::display::show_temperature(celsius));
        }
        // Said by the gateway's model, in the plugin's words: only with the right.
        if let Some(model) =
            answer.get("model").filter(|_| manifest.as_ref().is_some_and(|m| m.has(&Permission::LlmChat)))
        {
            if let Some(host_model) = self.model.get() {
                let text = |key: &str| model.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
                let key = format!("plugin:{plugin}:{}:{}", context.device_tag, text("key"));
                let prompt = if text("prompt").is_empty() { context.text.clone() } else { text("prompt") };
                let fresh = model.get("fresh").and_then(Value::as_bool).unwrap_or(false);
                outcome.phrases = Some(host_model(key, text("system"), prompt, context.cancel.clone(), fresh));
            }
        }
        if let Some(play) = answer.get("play") {
            let items =
                media_items_with(plugin, kind, play, context.media_base_url.as_deref(), &host.play_rights(plugin));
            if !items.is_empty() {
                let index = answer.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let directives = self.player.play(&context.device_tag, items, index, 0);
                // A long speech first, then the music: sent together, the station starts the music over the words.
                let spoken = outcome.speech.as_ref().map(|speech| speech.chars().count()).unwrap_or(0);
                let later = (spoken > LONG_SPEECH)
                    .then(|| host.playback_watch())
                    .flatten()
                    .zip(self.player.current(&context.device_tag).map(|(_, state)| state.play_id))
                    .is_some_and(|(watch, play_id)| {
                        watch.start_later(&context.device_tag, &play_id, speaking_time(spoken))
                    });
                if !later {
                    outcome.directives.extend(directives);
                }
            } else if outcome.speech.is_none() {
                outcome.speech = Some("Не нашла, что включить.".into());
            }
        }
        outcome.listen = answer.get("listen").and_then(Value::as_bool).unwrap_or(false);
        if answer.contains_key("timeout") {
            host.note_line(
                plugin,
                "warn",
                "таймер хода не встал: timeout — только внутри continue: {handler, timeout}",
            );
        }
        // The conversation goes on with the plugin: the next words come back to it without the wake word.
        let next = match answer.get("continue") {
            Some(Value::Bool(true)) => Some((handler.to_string(), None)),
            Some(Value::Object(next)) => {
                Some((next.get("handler").and_then(Value::as_str).unwrap_or(handler).to_string(), next.get("timeout")))
            }
            _ => None,
        };
        let Some((next, timeout)) = next else {
            self.going().remove(&context.device_tag);
            return (!outcome.is_empty()).then_some(outcome);
        };
        let device = &context.device_tag;
        // «Время вышло» does not lengthen the conversation: only the person's words do.
        let (since, missed) = match time_up {
            Some(missed) => (self.going().get(device).map_or_else(Instant::now, |going| going.since), missed),
            None => (Instant::now(), 0),
        };
        let left = GOING_MS.saturating_sub(since.elapsed().as_millis() as u64);
        if left == 0 {
            self.going().remove(device);
            return (!outcome.is_empty()).then_some(outcome);
        }
        let mut going = Going {
            plugin: plugin.to_string(),
            version: manifest.as_ref().map(|manifest| manifest.version.clone()),
            handler: next,
            capability,
            since,
            missed,
            turn: None,
        };
        match timeout.map(turn_of) {
            Some(Ok(Some((seconds, then)))) => {
                let id = self.next_turn.fetch_add(1, Ordering::Relaxed);
                going.turn = Some(Turn { id, seconds, then, from_ms: None });
                // The count begins when the station stops speaking this answer.
                if let Some(timers) = self.timers() {
                    let device = device.clone();
                    outcome.on_spoken = Some(Box::new(move |ended_ms| timers.arm(device, id, ended_ms)));
                }
            }
            Some(Err(why)) => host.note_line(plugin, "warn", &format!("таймер хода не встал: {why}")),
            Some(Ok(None)) | None => {}
        }
        self.going().insert(device.clone(), going);
        outcome.listen = true;
        outcome.continuation = Some(ContinuationRequest { capability: CapabilityName::Plugins, ttl_ms: left });
        Some(outcome)
    }

    /// The built-in implementation of the words the plugin let go, when they were a built-in rule's.
    async fn fallback(&self, intent: &Intent, context: &ExecutionContext) -> Option<anyhow::Result<Outcome>> {
        let fallback: Intent = serde_json::from_value(intent.params.get(FALLBACK)?.clone()).ok()?;
        let skill = self.router.upgrade()?.skill(fallback.capability).filter(|skill| skill.available())?;
        Some(skill.execute(&fallback, context).await)
    }

    /// The words that follow a plugin's answer with `continue`: back to that plugin, with the words as `text`.
    async fn follow(&self, context: &ExecutionContext) -> anyhow::Result<Outcome> {
        let device = &context.device_tag;
        // The person answered: the turn timer goes, and the silences are counted from nothing again.
        let going = {
            let mut going = self.going();
            going.get_mut(device).filter(|going| going.since.elapsed() < Duration::from_millis(GOING_MS)).map(|going| {
                let turn = going.turn.take();
                going.missed = 0;
                (going.clone(), turn)
            })
        };
        let Some((going, turn)) = going else {
            self.going().remove(device);
            return Ok(Outcome::speech("Разговор закончился. Скажите, что сделать."));
        };
        let host = self.host.upgrade().ok_or_else(|| anyhow::anyhow!("plugins are gone"))?;
        let mut intent = Intent::new(going.capability, "continue");
        intent.params.insert("slots".into(), json!({ "text": context.text }));
        let mut input = PluginSkill::input(&intent, context);
        // How long the person thought: from the end of the station's speech to the words (0: they spoke over it).
        if let Some(turn) = turn {
            let spent = turn.from_ms.map_or(0.0, |from| (now_ms().saturating_sub(from) / 100) as f64 / 10.0);
            input["timer"] = json!({ "seconds": turn.seconds, "spent": spent });
        }
        let answer = host.call(&going.plugin, &going.handler, input, CallKind::Voice).await;
        match answer {
            Ok(answer) => Ok(self
                .outcome(&going.plugin, &going.handler, going.capability, answer, context, None)
                .unwrap_or_else(|| {
                    self.going().remove(device);
                    Outcome::speech("Хорошо.")
                })),
            Err(error) => {
                self.going().remove(device);
                Err(anyhow::anyhow!("plugin {}: {error}", going.plugin))
            }
        }
    }

    /// «Время вышло» on the device: the station, asked by a push (`TurnStation::ask`), wants its answer.
    async fn time_up(&self, intent: &Intent, context: &ExecutionContext) -> anyhow::Result<Outcome> {
        let device = &context.device_tag;
        let id = intent.params.get("turn").and_then(Value::as_u64);
        let taken = {
            let mut going = self.going();
            match going.get_mut(device).filter(|going| going.since.elapsed() < Duration::from_millis(GOING_MS)) {
                Some(going) if going.turn.as_ref().is_some_and(|turn| Some(turn.id) == id) => {
                    let turn = going.turn.take();
                    turn.map(|turn| (going.clone(), turn))
                }
                // Not this timer's any more (the person's words came first): the conversation stays as it is.
                Some(going) => {
                    let left = GOING_MS.saturating_sub(going.since.elapsed().as_millis() as u64);
                    let continuation = ContinuationRequest { capability: CapabilityName::Plugins, ttl_ms: left };
                    return Ok(Outcome { continuation: Some(continuation), ..Outcome::default() });
                }
                None => None,
            }
        };
        let Some((going, turn)) = taken else { return Ok(Outcome::default()) };
        if going.missed >= TIMES_UP_IN_A_ROW {
            self.going().remove(device);
            return Ok(Outcome::speech(NOBODY_THERE));
        }
        let missed = going.missed + 1;
        let handler = match turn.then {
            TimeUp::Say(text) => {
                self.going().remove(device);
                return Ok(Outcome::speech(text));
            }
            TimeUp::Handler(handler) => handler,
        };
        let host = self.host.upgrade().ok_or_else(|| anyhow::anyhow!("plugins are gone"))?;
        let mut input = Map::new();
        input.insert("timeout".into(), json!({ "seconds": turn.seconds, "missed": missed }));
        input.insert(REQUEST.into(), PluginSkill::request(context, going.capability));
        let answer = host.call(&going.plugin, &handler, Value::Object(input), CallKind::Voice).await;
        match answer {
            Ok(answer) => Ok(self
                .outcome(&going.plugin, &handler, going.capability, answer, context, Some(missed))
                .unwrap_or_else(|| {
                    self.going().remove(device);
                    Outcome::default()
                })),
            Err(error) => {
                self.going().remove(device);
                Err(anyhow::anyhow!("plugin {}: {error}", going.plugin))
            }
        }
    }

    async fn serve(&self, intent: &Intent, context: &ExecutionContext) -> anyhow::Result<Outcome> {
        if intent.action == "continue" {
            return self.follow(context).await;
        }
        if intent.action == "timeout" {
            return self.time_up(intent, context).await;
        }
        let text = |key: &str| intent.params.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
        let (plugin, handler) = (text("plugin"), text("handler"));
        let host = self.host.upgrade().ok_or_else(|| anyhow::anyhow!("plugins are gone"))?;
        let answer = host.call(&plugin, &handler, PluginSkill::input(intent, context), CallKind::Voice).await;
        let failed = match answer {
            Ok(answer) => match self.outcome(&plugin, &handler, intent.capability, answer, context, None) {
                Some(outcome) => return Ok(outcome),
                None => None,
            },
            Err(error) => Some(error),
        };
        if let Some(served) = self.fallback(intent, context).await {
            return served;
        }
        match failed {
            Some(error) => Err(anyhow::anyhow!("plugin {plugin}: {error}")),
            None => Ok(Outcome::speech("Не получилось.")),
        }
    }
}

impl OwnSkill for PluginSkill {
    fn capability(&self) -> CapabilityName {
        CapabilityName::Plugins
    }

    fn verified(&self, _intent: &Intent) -> bool {
        // Speech and our player: nothing the station has not confirmed.
        true
    }

    fn available(&self) -> bool {
        self.voice.get().is_some() || self.host.upgrade().is_some_and(|host| host.has_phrases())
    }

    fn execute<'a>(
        &'a self,
        intent: &'a Intent,
        context: &'a ExecutionContext,
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        if let Some(voice) = self.voice.get().filter(|voice| voice.handles(intent, &context.device_tag)) {
            return voice.execute(intent, context);
        }
        if let Some(voice) = self.history_voice.get().filter(|voice| voice.handles(intent, &context.device_tag)) {
            return voice.execute(intent, context);
        }
        Box::pin(self.serve(intent, context))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PluginsConfig;
    use crate::core::availability::Placement;
    use crate::core::events::EventBus;
    use crate::plugins::controller::PluginsDeps;
    use crate::plugins::quickjs::QuickJs;
    use crate::plugins::registry::tests::write_package;
    use crate::settings::SettingsService;
    use crate::skills::media::media_skills::RadioSkill;
    use crate::skills::media::sources::{MediaSources, StaticRadioCatalog};
    use crate::skills::router::{Decision, ModelState, Resolution, RouteInput};
    use crate::skills::routes::RouteTable;
    use crate::storage::Db;
    use crate::utils::time::now_ms;
    use chrono::Utc;

    const CODE: &str = r#"
        export function play({ station, request }) {
            if (station === "ошибка") throw new Error("каталог упал");
            if (station !== "джаз") return null;
            return { say: `Включаю: ${request.text}`, play: [{ id: "jazz", title: "Джаз", url: "https://radio.example/jazz.mp3" }] };
        }
    "#;

    /// A router with our radio (a list with «Студия 21») and a plugin that serves the radio's words.
    async fn router() -> (Arc<Router>, Arc<PluginHost>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let manifest = serde_json::json!({
            "schema": 1, "id": "radio", "version": "1.0.0", "title": "Радио", "api": "^1", "runs": ["pc"],
            "permissions": ["voice.intents"],
            "contributes": { "intents": [{ "phrases": ["включи радио {station}"], "handler": "play", "capability": "radio" }] }
        });
        write_package(&dir.path().join("packages"), &manifest, Some(CODE));
        std::fs::write(
            dir.path().join("radio.json"),
            r#"[{"id": "studio21", "title": "Студия 21", "url": "https://hls.example/studio21.m3u8"}]"#,
        )
        .unwrap();
        let db = Arc::new(Db::memory());
        let events = EventBus::new();
        let sources = MediaSources::new(vec![Arc::new(StaticRadioCatalog::new(dir.path().join("radio.json")))]);
        let player = Arc::new(MediaPlayer::new());
        let host = PluginHost::new(PluginsDeps {
            config: PluginsConfig {
                packages_dir: dir.path().join("packages"),
                disabled: false,
                idle_unload_s: 600,
                feed_url: None,
                catalog_dir: dir.path().join("packages-catalog"),
                dev_dir: dir.path().join("packages-dev"),
                catalog_key: None,
                reserve_mb: 40,
                storage_reserve_mb: 15,
                pressure_action: crate::plugins::budget::PressureAction::Disable,
                bundled: false,
                schedules: true,
            },
            db: db.clone(),
            settings: Arc::new(SettingsService::new(db, events.clone())),
            events,
            media: sources.clone(),
            placement: Arc::new(|| Placement::Pc),
            speaker: Arc::new(|| None),
            runtime: Arc::new(QuickJs),
            memory: crate::plugins::budget::fake::plenty(),
            gauge: Default::default(),
            bundled: Vec::new(),
            player: None,
            watch: None,
            home: None,
            gateway_url: "http://127.0.0.1:9".into(),
            services: None,
            secrets: Arc::new(
                crate::secrets::SecretsService::new(Arc::new(crate::storage::Db::memory()), None).unwrap(),
            ),
        });
        host.enable("radio", None).await.unwrap();
        let router = Arc::new(Router::new(
            RouteTable::parse("own,yandex", ""),
            vec![Arc::new(RadioSkill::new(sources, player.clone()))],
        ));
        router.register(Arc::new(PluginSkill::new(Arc::downgrade(&host), Arc::downgrade(&router), player)));
        router.add_rules(host.clone());
        (router, host, dir)
    }

    async fn heard(router: &Router, text: &str) -> anyhow::Result<Outcome> {
        let input = RouteInput {
            text: text.into(),
            device_tag: "kitchen".into(),
            model: ModelState { enabled: true, tools: true },
            now: now_ms(),
            ..RouteInput::default()
        };
        let Decision::Intent { intent, resolution: Resolution::Own(skill), rule, .. } = router.decide(&input) else {
            panic!("{text}: not ours");
        };
        assert_eq!(rule, Some("plugin:radio"), "{text}");
        let mut context = ExecutionContext::new("kitchen", Utc::now());
        context.text = text.into();
        skill.execute(&intent, &context).await
    }

    fn stream(outcome: &Outcome) -> String {
        outcome.directives[0].payload.as_ref().unwrap()["stream"]["url"].as_str().unwrap().to_string()
    }

    #[tokio::test]
    async fn plays_what_the_plugin_found_and_gives_the_built_in_radio_what_it_let_go() {
        let (router, _host, _dir) = router().await;
        let jazz = heard(&router, "Включи радио джаз").await.unwrap();
        assert_eq!(jazz.directives[0].name, "audio_play");
        assert_eq!(stream(&jazz), "https://radio.example/jazz.mp3");
        assert_eq!(jazz.speech.as_deref(), Some("Включаю: Включи радио джаз"));
        // Not the plugin's: our own list has it.
        let ours = heard(&router, "включи радио студия 21").await.unwrap();
        assert_eq!(stream(&ours), "https://hls.example/studio21.m3u8");
        // The plugin failed: our radio answers as it would without the plugin.
        let failed = heard(&router, "включи радио ошибка").await.unwrap();
        assert_eq!(failed.speech.as_deref(), Some("Не нашла такую станцию."));
    }

    /// The household's music as the gateway's services give it: two files of content/music.
    struct Content;

    impl crate::plugins::host_api::HostServices for Content {
        fn content(
            &self,
            folder: &str,
            _query: Option<String>,
            _limit: usize,
        ) -> futures::future::BoxFuture<'static, Vec<Value>> {
            let items = if folder == "music" {
                vec![
                    json!({ "id": "music/Битлз/Yesterday.mp3", "content": "music/Битлз/Yesterday.mp3", "title": "Yesterday" }),
                ]
            } else {
                Vec::new()
            };
            Box::pin(async move { items })
        }
        fn call_tool(
            &self,
            _: &str,
            _: &str,
            _: serde_json::Map<String, Value>,
        ) -> futures::future::BoxFuture<'static, Result<String, String>> {
            Box::pin(async { Err("нет".to_string()) })
        }
        fn smart_home_devices(&self) -> futures::future::BoxFuture<'static, Result<Value, String>> {
            Box::pin(async { Err("нет".to_string()) })
        }
        fn smart_home_act(
            &self,
            _: String,
            _: String,
            _: Option<f64>,
        ) -> futures::future::BoxFuture<'static, Result<String, String>> {
            Box::pin(async { Err("нет".to_string()) })
        }
        fn weather_place(&self) -> Option<String> {
            None
        }
        fn news_feeds(&self) -> Vec<Value> {
            Vec::new()
        }
    }

    /// The repository's built-in package, as the build lays it out.
    fn built_in(root: &std::path::Path, id: &str) {
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../packages").join(id);
        let manifest: Value =
            serde_json::from_str(&std::fs::read_to_string(source.join("manifest.json")).unwrap()).unwrap();
        let target = root.join(id).join(manifest["version"].as_str().unwrap());
        std::fs::create_dir_all(&target).unwrap();
        for file in ["manifest.json", "backend.js"] {
            std::fs::copy(source.join(file), target.join(file)).unwrap();
        }
    }

    /// A host with the repository's built-in packages `ids`, on, over these services.
    async fn built_in_host(
        dir: &std::path::Path,
        ids: &[&str],
        services: Arc<dyn crate::plugins::host_api::HostServices>,
    ) -> Arc<PluginHost> {
        let packages = dir.join("packages");
        for id in ids {
            built_in(&packages, id);
        }
        let db = Arc::new(Db::memory());
        let events = EventBus::new();
        let host = PluginHost::new(PluginsDeps {
            config: PluginsConfig {
                packages_dir: packages,
                disabled: false,
                idle_unload_s: 600,
                feed_url: None,
                catalog_dir: dir.join("packages-catalog"),
                dev_dir: dir.join("packages-dev"),
                catalog_key: None,
                reserve_mb: 40,
                storage_reserve_mb: 15,
                pressure_action: crate::plugins::budget::PressureAction::Disable,
                bundled: false,
                schedules: true,
            },
            db: db.clone(),
            settings: Arc::new(SettingsService::new(db, events.clone())),
            events,
            media: MediaSources::new(Vec::new()),
            placement: Arc::new(|| Placement::Pc),
            speaker: Arc::new(|| None),
            runtime: Arc::new(QuickJs),
            memory: crate::plugins::budget::fake::plenty(),
            gauge: Default::default(),
            bundled: Vec::new(),
            player: None,
            watch: None,
            home: None,
            gateway_url: "http://127.0.0.1:9".into(),
            services: Some(services),
            secrets: Arc::new(
                crate::secrets::SecretsService::new(Arc::new(crate::storage::Db::memory()), None).unwrap(),
            ),
        });
        for id in ids {
            host.enable(id, None).await.unwrap();
        }
        host
    }

    #[tokio::test]
    async fn the_built_in_games_and_music_plugins_speak_by_the_model_listen_on_and_play_the_households_files() {
        use futures::StreamExt;
        let dir = tempfile::tempdir().unwrap();
        let host = built_in_host(dir.path(), &["games", "music"], Arc::new(Content)).await;
        let player = Arc::new(MediaPlayer::new());
        let router = Arc::new(Router::new(RouteTable::parse("own,yandex", ""), Vec::new()));
        let skill = Arc::new(PluginSkill::new(Arc::downgrade(&host), Arc::downgrade(&router), player));
        // The model: says what it was given, so the test hears whose words went in.
        let asked: Arc<std::sync::Mutex<Vec<(String, String)>>> = Arc::default();
        let heard_by_model = asked.clone();
        skill.set_model(Arc::new(move |key: String, system: String, text: String, _cancel, _fresh| {
            heard_by_model.lock().unwrap().push((key, system));
            futures::stream::iter(vec![Ok(format!("Ход: {text}"))]).boxed()
        }));
        router.register(skill.clone());
        router.add_rules(host.clone());

        let input = |text: &str, continuation: Option<crate::skills::router::Continuation>| RouteInput {
            text: text.into(),
            device_tag: "kitchen".into(),
            continuation,
            wake_word: continuation.is_none(),
            model: ModelState { enabled: true, tools: true },
            now: now_ms(),
            ..RouteInput::default()
        };
        let run = |decision: Decision, text: &str| {
            let Decision::Intent { intent, resolution: Resolution::Own(skill), .. } = decision else {
                panic!("{text}: not ours")
            };
            let mut context = ExecutionContext::new("kitchen", Utc::now());
            context.text = text.into();
            (intent, skill, context)
        };
        // «Давай поиграем в города»: the model hosts, the station listens on without the wake word.
        let (intent, served, context) =
            run(router.decide(&input("давай поиграем в города", None)), "давай поиграем в города");
        let started = served.execute(&intent, &context).await.unwrap();
        assert!(started.listen && started.continuation.is_some());
        let said: Vec<String> = started.phrases.unwrap().map(|phrase| phrase.unwrap()).collect().await;
        assert!(said[0].starts_with("Ход: Начнём!"), "{said:?}");
        assert!(asked.lock().unwrap()[0].1.contains("«города»"));
        // The player's move comes back to the plugin; «хватит» ends it.
        let going = crate::skills::router::Continuation {
            capability: CapabilityName::Plugins,
            executor: crate::skills::routes::RouteStep::Own,
            until: now_ms() + 60_000,
        };
        let (intent, served, context) = run(router.decide(&input("москва", Some(going))), "москва");
        let moved = served.execute(&intent, &context).await.unwrap();
        let said: Vec<String> = moved.phrases.unwrap().map(|phrase| phrase.unwrap()).collect().await;
        assert_eq!(said, ["Ход: москва"]);
        let (intent, served, context) = run(router.decide(&input("хватит", Some(going))), "хватит");
        let ended = served.execute(&intent, &context).await.unwrap();
        assert_eq!(ended.speech.as_deref(), Some("Хорошо, поиграем в другой раз."));
        assert!(ended.continuation.is_none());

        // «Включи мою музыку»: the household's file, its link made by the gateway.
        let (intent, served, mut context) = run(router.decide(&input("включи мою музыку", None)), "включи мою музыку");
        context.media_base_url = Some("http://192.0.2.10/media".into());
        let music = served.execute(&intent, &context).await.unwrap();
        assert_eq!(
            stream(&music),
            format!("http://192.0.2.10/media/music/{}/Yesterday.mp3", crate::skills::js::encode_uri_component("Битлз"))
        );
    }

    /// The other plugins' tools for the morning show: the weather, the news, and the radio that answers what to play.
    struct Morning;

    impl crate::plugins::host_api::HostServices for Morning {
        fn content(&self, _: &str, _: Option<String>, _: usize) -> futures::future::BoxFuture<'static, Vec<Value>> {
            Box::pin(async { Vec::new() })
        }
        fn call_tool(
            &self,
            caller: &str,
            name: &str,
            _: serde_json::Map<String, Value>,
        ) -> futures::future::BoxFuture<'static, Result<String, String>> {
            assert_eq!(caller, "morning");
            let answer = match name {
                "weather_now" => Ok("Сейчас +5, облачно. Ветер слабый. Завтра теплее.".to_string()),
                "news_headlines" => Ok("Открыли новый мост. Завтра концерт. Ещё новость.".to_string()),
                "radio_play" => Ok(json!({ "say": "Включаю Хит FM", "play": [{ "id": "hitfm", "title": "Хит FM", "url": "https://hitfm.hostingradio.ru/hitfm128.mp3" }] }).to_string()),
                other => Err(format!("нет инструмента {other}")),
            };
            Box::pin(async move { answer })
        }
        fn smart_home_devices(&self) -> futures::future::BoxFuture<'static, Result<Value, String>> {
            Box::pin(async { Err("нет".to_string()) })
        }
        fn smart_home_act(
            &self,
            _: String,
            _: String,
            _: Option<f64>,
        ) -> futures::future::BoxFuture<'static, Result<String, String>> {
            Box::pin(async { Err("нет".to_string()) })
        }
        fn weather_place(&self) -> Option<String> {
            None
        }
        fn news_feeds(&self) -> Vec<Value> {
            Vec::new()
        }
    }

    #[tokio::test]
    async fn the_morning_show_tells_the_weather_and_the_news_from_other_plugins_and_plays_the_radio_after() {
        let dir = tempfile::tempdir().unwrap();
        let host = built_in_host(dir.path(), &["morning"], Arc::new(Morning)).await;
        let router = Arc::new(Router::new(RouteTable::parse("own,yandex", ""), Vec::new()));
        let skill =
            Arc::new(PluginSkill::new(Arc::downgrade(&host), Arc::downgrade(&router), Arc::new(MediaPlayer::new())));
        router.register(skill);
        router.add_rules(host.clone());
        let input = RouteInput {
            text: "утреннее шоу".into(),
            device_tag: "kitchen".into(),
            wake_word: true,
            now: now_ms(),
            ..RouteInput::default()
        };
        let Decision::Intent { intent, resolution: Resolution::Own(served), .. } = router.decide(&input) else {
            panic!("not ours")
        };
        let mut context = ExecutionContext::new("kitchen", Utc::now());
        context.text = "утреннее шоу".into();
        let show = served.execute(&intent, &context).await.unwrap();
        let said = show.speech.clone().unwrap();
        assert!(
            said.contains("Погода. Сейчас +5, облачно. Ветер слабый.") && !said.contains("Завтра теплее"),
            "{said}"
        );
        assert!(
            said.contains("Новости. Открыли новый мост. Завтра концерт.") && said.ends_with("А теперь — Хит FM."),
            "{said}"
        );
        assert_eq!(stream(&show), "https://hitfm.hostingradio.ru/hitfm128.mp3");
    }

    /// A quiz with the turn timer: 10 seconds a move, «время вышло» from its function; a ready phrase; a timer out
    /// of bounds.
    const QUIZ: &str = r#"
        const next = { handler: "answer", timeout: { seconds: 10, handler: "timeUp" } };
        export function start() {
            return { say: "Столица Австралии?", continue: next };
        }
        export function answer({ text, timer }) {
            return { say: `Ответ ${text}: ${timer ? `${timer.spent} из ${timer.seconds}` : "без таймера"}`, continue: next };
        }
        export function timeUp({ timeout }) {
            return { say: `Время вышло ${timeout.missed}`, continue: next };
        }
        export function ready() {
            return { say: "Два плюс два?", continue: { handler: "answer", timeout: { seconds: 5, say: "Время вышло. Четыре." } } };
        }
        export function wrong() {
            return { say: "Долго думать", continue: { handler: "answer", timeout: { seconds: 500, handler: "timeUp" } } };
        }
    "#;

    /// What the station said by the timer, and for how long the conversation went on after it.
    type Said = Arc<Mutex<Vec<(String, Option<u64>)>>>;

    /// The station as the session registry gives it to the turn timer: «время вышло» runs our skill's intent at once
    /// and keeps what was said (and for how long the conversation goes on); its speech «ended» `lag_ms` ago.
    #[derive(Default)]
    struct Station {
        skill: OnceLock<Weak<PluginSkill>>,
        said: Said,
        hearing: std::sync::atomic::AtomicBool,
        ringing: std::sync::atomic::AtomicBool,
        lag_ms: Arc<AtomicU64>,
    }

    impl TurnStation for Station {
        fn ask(
            &self,
            device: &str,
            intent: Intent,
            _label: String,
            still: Arc<dyn Fn() -> bool + Send + Sync>,
        ) -> BoxFuture<'static, bool> {
            let (skill, said, lag) = (self.skill.get().and_then(Weak::upgrade), self.said.clone(), self.lag_ms.clone());
            let device = device.to_string();
            Box::pin(async move {
                let Some(skill) = skill.filter(|_| still()) else { return false };
                let context = ExecutionContext::new(&device, Utc::now());
                let Ok(mut outcome) = skill.execute(&intent, &context).await else { return false };
                let ttl = outcome.continuation.map(|continuation| continuation.ttl_ms);
                said.lock().unwrap().push((outcome.speech.clone().unwrap_or_default(), ttl));
                if let Some(spoken) = outcome.on_spoken.take() {
                    spoken(now_ms().saturating_sub(lag.load(Ordering::Relaxed)));
                }
                true
            })
        }

        fn hearing(&self, _: &str) -> bool {
            self.hearing.load(Ordering::Relaxed)
        }

        fn ringing(&self, _: &str) -> bool {
            self.ringing.load(Ordering::Relaxed)
        }
    }

    struct Quiz {
        skill: Arc<PluginSkill>,
        station: Arc<Station>,
        host: Arc<PluginHost>,
        router: Arc<Router>,
        _dir: tempfile::TempDir,
    }

    impl Quiz {
        /// The person's phrase with the wake word.
        async fn say(&self, text: &str) -> Outcome {
            let input = RouteInput {
                text: text.into(),
                device_tag: "kitchen".into(),
                wake_word: true,
                model: ModelState { enabled: true, tools: true },
                now: now_ms(),
                ..RouteInput::default()
            };
            let Decision::Intent { intent, resolution: Resolution::Own(skill), .. } = self.router.decide(&input) else {
                panic!("{text}: not ours")
            };
            let mut context = ExecutionContext::new("kitchen", Utc::now());
            context.text = text.into();
            skill.execute(&intent, &context).await.unwrap()
        }

        /// The person's words in the conversation.
        async fn words(&self, text: &str) -> Outcome {
            let mut context = ExecutionContext::new("kitchen", Utc::now());
            context.text = text.into();
            self.skill.execute(&Intent::new(CapabilityName::Plugins, "continue"), &context).await.unwrap()
        }

        /// What the station said by the timer.
        fn said(&self) -> Vec<String> {
            self.station.said.lock().unwrap().iter().map(|(said, _)| said.clone()).collect()
        }
    }

    async fn quiz() -> Quiz {
        let dir = tempfile::tempdir().unwrap();
        let manifest = json!({
            "schema": 1, "id": "quiz", "version": "1.0.0", "title": "Викторина", "api": "^1", "runs": ["pc"],
            "permissions": ["voice.intents"],
            "contributes": { "intents": [
                { "phrases": ["викторина"], "handler": "start" },
                { "phrases": ["быстрый вопрос"], "handler": "ready" },
                { "phrases": ["долгий вопрос"], "handler": "wrong" }
            ] }
        });
        write_package(&dir.path().join("packages"), &manifest, Some(QUIZ));
        let db = Arc::new(Db::memory());
        let events = EventBus::new();
        let host = PluginHost::new(PluginsDeps {
            config: PluginsConfig {
                packages_dir: dir.path().join("packages"),
                disabled: false,
                idle_unload_s: 600,
                feed_url: None,
                catalog_dir: dir.path().join("packages-catalog"),
                dev_dir: dir.path().join("packages-dev"),
                catalog_key: None,
                reserve_mb: 40,
                storage_reserve_mb: 15,
                pressure_action: crate::plugins::budget::PressureAction::Disable,
                bundled: false,
                schedules: true,
            },
            db: db.clone(),
            settings: Arc::new(SettingsService::new(db, events.clone())),
            events,
            media: MediaSources::new(Vec::new()),
            placement: Arc::new(|| Placement::Pc),
            speaker: Arc::new(|| None),
            runtime: Arc::new(QuickJs),
            memory: crate::plugins::budget::fake::plenty(),
            gauge: Default::default(),
            bundled: Vec::new(),
            player: None,
            watch: None,
            home: None,
            gateway_url: "http://127.0.0.1:9".into(),
            services: None,
            secrets: Arc::new(
                crate::secrets::SecretsService::new(Arc::new(crate::storage::Db::memory()), None).unwrap(),
            ),
        });
        host.enable("quiz", None).await.unwrap();
        let router = Arc::new(Router::new(RouteTable::parse("own,yandex", ""), Vec::new()));
        let skill =
            Arc::new(PluginSkill::new(Arc::downgrade(&host), Arc::downgrade(&router), Arc::new(MediaPlayer::new())));
        let station = Arc::new(Station::default());
        let _ = station.skill.set(Arc::downgrade(&skill));
        skill.set_turns(station.clone());
        router.register(skill.clone());
        router.add_rules(host.clone());
        Quiz { skill, station, host, router, _dir: dir }
    }

    async fn until(check: impl Fn() -> bool) -> bool {
        for _ in 0..150 {
            if check() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        check()
    }

    #[tokio::test]
    async fn says_time_is_up_twice_and_the_third_silence_ends_the_conversation() {
        let quiz = quiz().await;
        // Each «время вышло» is said at once and its own count runs out 30 ms after it: nobody is there.
        quiz.station.lag_ms.store(10_000 - 30, Ordering::Relaxed);
        let mut asked = quiz.say("викторина").await;
        assert_eq!(asked.speech.as_deref(), Some("Столица Австралии?"));
        assert!(asked.listen && asked.continuation.is_some());
        (asked.on_spoken.take().unwrap())(now_ms() - (10_000 - 30));
        assert!(until(|| quiz.said().len() == 3).await, "{:?}", quiz.said());
        assert_eq!(quiz.said(), ["Время вышло 1", "Время вышло 2", NOBODY_THERE]);
        let after = quiz.words("канберра").await;
        assert_eq!(after.speech.as_deref(), Some("Разговор закончился. Скажите, что сделать."));
    }

    #[tokio::test]
    async fn the_persons_words_take_the_timer_and_tell_how_long_they_thought() {
        let quiz = quiz().await;
        let mut asked = quiz.say("викторина").await;
        // The station stopped speaking 4 s ago.
        (asked.on_spoken.take().unwrap())(now_ms() - 4_000);
        let said = quiz.words("канберра").await.speech.unwrap();
        assert!(said.starts_with("Ответ канберра: 4") && said.ends_with(" из 10"), "{said}");
        // Due in 200 ms, and the words come first: «время вышло» never comes.
        let mut asked = quiz.say("викторина").await;
        (asked.on_spoken.take().unwrap())(now_ms() - (10_000 - 200));
        quiz.words("канберра").await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(quiz.said().is_empty(), "{:?}", quiz.said());
    }

    #[tokio::test]
    async fn a_ready_phrase_ends_the_conversation_and_a_timer_out_of_bounds_is_not_set() {
        let quiz = quiz().await;
        let mut asked = quiz.say("быстрый вопрос").await;
        (asked.on_spoken.take().unwrap())(now_ms() - (5_000 - 30));
        assert!(until(|| quiz.said().len() == 1).await);
        assert_eq!(quiz.said(), ["Время вышло. Четыре."]);
        assert_eq!(quiz.words("четыре").await.speech.as_deref(), Some("Разговор закончился. Скажите, что сделать."));
        let long = quiz.say("долгий вопрос").await;
        assert!(long.on_spoken.is_none() && long.continuation.is_some());
        assert!(
            quiz.host.journal("quiz").iter().any(|line| line.message.contains("таймер хода не встал: seconds 500")),
            "{:?}",
            quiz.host.journal("quiz").iter().map(|line| line.message.clone()).collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn a_late_timer_goes_quietly_words_being_heard_hold_it_and_a_ringing_alarm_takes_it() {
        let quiz = quiz().await;
        // The Mac slept through it: 20 s late, more than its own 10.
        let mut asked = quiz.say("викторина").await;
        (asked.on_spoken.take().unwrap())(now_ms() - 30_000);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(quiz.said().is_empty());
        // Words are being heard when it comes due: it waits for them, and says nothing while they last.
        quiz.station.hearing.store(true, Ordering::Relaxed);
        let mut asked = quiz.say("викторина").await;
        (asked.on_spoken.take().unwrap())(now_ms() - (10_000 - 30));
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(quiz.said().is_empty());
        quiz.station.hearing.store(false, Ordering::Relaxed);
        assert!(until(|| quiz.said().len() == 1).await);
        // An alarm rings: the timer goes without a word.
        quiz.station.ringing.store(true, Ordering::Relaxed);
        let mut asked = quiz.say("викторина").await;
        (asked.on_spoken.take().unwrap())(now_ms() - (10_000 - 30));
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(quiz.said().len(), 1);
    }

    #[tokio::test]
    async fn time_up_does_not_lengthen_the_conversation_only_the_persons_words_do() {
        let quiz = quiz().await;
        let mut asked = quiz.say("викторина").await;
        // The person's last words were almost three minutes ago.
        let Some(long_ago) = Instant::now().checked_sub(Duration::from_millis(GOING_MS - 5_000)) else { return };
        quiz.skill.going().get_mut("kitchen").unwrap().since = long_ago;
        (asked.on_spoken.take().unwrap())(now_ms() - (10_000 - 30));
        assert!(until(|| quiz.said().len() == 1).await);
        let ttl = quiz.station.said.lock().unwrap()[0].1.unwrap();
        assert!(ttl <= 5_000, "{ttl}");
        // The person's words: three minutes again.
        let answered = quiz.words("канберра").await;
        assert!(answered.continuation.unwrap().ttl_ms > GOING_MS - 1_000);
    }
}
