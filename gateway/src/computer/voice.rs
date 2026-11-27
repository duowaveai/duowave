//! The computer's own voice channel in the gateway (docs/plans/audio-interface-3.md, part К): when the app gave its
//! audio port (`ear::port::set_port`), the channel stands on it, talks to Thinking through the door — the talk layer
//! behind it (`talk::thinking::TalkThinking`) — and gives the word's program the ear's sound (К10). A gateway without
//! the app's port (Docker, a server) has no channel, and the panel's microphone works as before.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use futures::StreamExt;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::app::App;
use crate::channel::{share, Deps, HomeHearing, HomeVoice, VoiceChannel};
use crate::core::identity::principal::user_principal;
use crate::core::origin::{Why, COMPUTER};
use crate::core::requests::{RequestHandle, RequestKind, RequestStart};
use crate::door::{
    self, Delivery, Event, Events, Expects, Failure, Notice, NoticeWhy, Notices, Opened, Outcome, Outside, Phrase,
    Told, Unasked, Urgency, WaitWhy, What,
};
use crate::health_status::HealthSources;
use crate::placement::peer::PeerClient;
use crate::platform::http::Provides;
use crate::talk::thinking::{People, TalkThinking};

/// The trips of this computer's ear in its gateway's journal (`channel::Trips`), as a station's session writes its
/// own: Thinking here goes on in them; the hub of a Турбо thinks in a request of its own, which the trip names.
struct EarTrips(Weak<App>);

impl crate::channel::Trips for EarTrips {
    fn begin(&self) -> Option<Arc<RequestHandle>> {
        let app = self.0.upgrade()?;
        let start = RequestStart {
            device: COMPUTER.to_string(),
            session: None,
            kind: RequestKind::Voice,
            text: None,
            parent: None,
        };
        Some(app.requests.begin_in(start, "own"))
    }

    fn offer(&self, trip: Arc<RequestHandle>) {
        let Some(app) = self.0.upgrade() else { return };
        // The hub of a Турбо thinks on its own gateway: nothing here would take the trip.
        if hub(&app).is_none() {
            app.request_offers.offer(COMPUTER, trip);
        }
    }
}

/// The channel stands, when there is a port to stand on.
pub fn attach(app: &Arc<App>) {
    let Some(port) = crate::ear::port::port() else { return };
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    let parts: crate::talk::Parts = Provides::provide(app.as_ref());
    // The switch picks ours or the talk layer's stand-in per phrase (docs/plans/thinking.md, «implementation»).
    let local =
        crate::links::thinking::door(app, Arc::new(TalkThinking::new(parts, app.personas.clone(), people(app))));
    // The «Готово» of the hub's works this channel's phrases began: their changes come on the hub's live channel,
    // which this gateway relays to its panels anyway.
    let peers = Arc::downgrade(app);
    let hub_works =
        HubWorks::new(Box::new(move |hub| peers.upgrade()?.pc.station_gateway.find(hub)?.client()), people(app));
    let heard = Arc::downgrade(&hub_works);
    app.live.watch_stations(Box::new(move |hub, message| {
        if let Some(works) = heard.upgrade() {
            works.heard(hub, message);
        }
    }));
    // The talk is the home's centre's: this computer's own, or the station's whose Turbo it is (reconciliation
    // 26.11, point 3).
    let thinking = Arc::new(HomeThinking { local, app: Arc::downgrade(app), turns: Arc::default(), hub_works });
    let channel = VoiceChannel::start(Deps {
        port,
        thinking,
        hear: Arc::new(HomeHearing(app.speech.clone())),
        speak: Arc::new(HomeVoice(app.voice.clone())),
        settings: app.settings.clone(),
        events: app.events.clone(),
        others: others(app),
        trips: Arc::new(EarTrips(Arc::downgrade(app))),
    });
    if app.pc.voice.set(channel.clone()).is_err() {
        return;
    }
    let plugins = Arc::downgrade(&app.plugins);
    channel.set_word_apps(Arc::new(move || {
        plugins.upgrade().is_some_and(|plugins| {
            plugins.installed_all().iter().any(|package| !package.manifest.contributes.activators.is_empty())
        })
    }));
    app.health.add_sources(Arc::new(VoiceHealth(Arc::downgrade(&channel))));
    // The word's program reads the ear's sound here, the way it reads a station's (К10).
    let weak = Arc::downgrade(app);
    tokio::spawn(async move {
        match share::serve(Arc::downgrade(&channel)).await {
            Ok(share) => {
                if let Some(app) = weak.upgrade() {
                    let _ = app.pc.voice_share.set(share.port);
                }
            }
            Err(error) => tracing::warn!(target: "talk", "the ear's sound for the word did not open its port: {error}"),
        }
    });
}

/// Who speaks into the computer's microphone: the person signed in on its panel, by their id; nobody — the
/// household's voice (decision 3).
fn people(app: &Arc<App>) -> People {
    people_of(app)
}

/// The words of the home's stations, once one is on the line: «Алиса» and the words of the activator apps (К9). A
/// phrase the computer heard by itself that begins with one is the station's.
fn others(app: &Arc<App>) -> Arc<dyn Fn() -> Vec<String> + Send + Sync> {
    let weak: Weak<App> = Arc::downgrade(app);
    Arc::new(move || {
        let Some(app) = weak.upgrade() else { return Vec::new() };
        #[cfg(feature = "yandex-station")]
        let stations = crate::ear::StationSessions::open(&*app.sessions) > 0;
        #[cfg(not(feature = "yandex-station"))]
        let stations = false;
        if !stations {
            return Vec::new();
        }
        let mut words = vec![crate::core::origin::STATION_WORD.to_string()];
        for package in app.plugins.installed_all() {
            for activator in &package.manifest.contributes.activators {
                words.push(activator.title.clone());
                words.extend(activator.heard.iter().cloned());
            }
        }
        words
    })
}

/// /health's `voiceChannel`.
struct VoiceHealth(Weak<VoiceChannel>);

impl HealthSources for VoiceHealth {
    fn voice_channel(&self) -> Option<bool> {
        Some(self.0.upgrade().is_some())
    }
}

/// Thinking for the computer's channel, where the home's centre is (reconciliation 26.11, point 3):
///
/// - **The computer is the hub** — the talk layer of this gateway (`TalkThinking`), as written above.
/// - **The computer is a Турбо of a station hub** — the channel still runs here, where the sound is: it cuts the
///   pieces, recognizes them and judges the thought locally, then sends the phrase to the hub's `/api/v1/talk`
///   through the same pass the panel uses (the pair's key, the person and their role), and says the answer's phrases
///   it gets back. The talk layer, the works, the questions and the window's filter stay on the hub; the station gets
///   no audio work. The «Готово» of a work this channel's phrase began or added to is the channel's (`HubWorks`).
struct HomeThinking {
    local: Arc<dyn crate::door::Thinking>,
    app: Weak<App>,
    /// The turns sent to the hub: their stop.
    turns: Arc<Mutex<HashMap<String, CancellationToken>>>,
    hub_works: Arc<HubWorks>,
}

/// Whether the app's key «Сказать» goes to this computer's own ear: in the mode «Микрофон», and on a Турбо of a station
/// hub, where the ear runs here and its phrases go to the hub. 27.11, the trial of main: on a Турбо the key went the
/// panel's way — the window came forward with every press, and the panel's «Сказать» knew no release, so a held key
/// never worked.
pub fn key_to_ear(app: &App) -> bool {
    crate::ear::input::on_computer(&app.settings) || hub(app).is_some()
}

/// The station hub this computer is the Турбо of, when there is one: its key, its gateway's address and the pair's key.
fn hub(app: &App) -> Option<(String, PeerClient)> {
    app.pc.station_gateway.all().into_iter().find_map(|gateway| Some((gateway.key().to_string(), gateway.client()?)))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The changes of a work after which its line is said, and after which it is not said any more (the panel's
/// `SAID_ON`; the talk layer's own for the computer's works, talk/thinking.rs).
const SAID_ON: &[&str] = &["ready", "unfinished", "failed", "done"];
const WITHDRAWN_ON: &[&str] = &["said", "decided", "stopped", "answered", "declined", "removed"];
/// How many of the hub's works of this channel are kept: the newest.
const OURS_KEPT: usize = 64;
/// A notice of a hub's work: `hub-work:<hub>:<work>`.
const HUB_NOTICE: &str = "hub-work:";

/// A hub by its key: where its gateway is, with the pair's key.
type Peers = Box<dyn Fn(&str) -> Option<PeerClient> + Send + Sync>;

/// The hub's works this computer's channel began or added to (part Н, «Н1», on the Турбо): their «Готово» is the
/// channel's to say, as the computer's own works' are. The hub tells a work's change on its live channel, which this
/// gateway relays to its panels anyway (placement/relay.rs, `LiveHub::watch_stations`); the work is read from the hub
/// through the pass as the person whose phrase it was; a line said is told back (`/api/v1/works/{id}/said`, as the
/// panel did) and the hub opens its question, which the channel's window answers. Kept in memory: a work begun before
/// this gateway started is shown on the panel's strip only.
struct HubWorks {
    /// The hub by its key: its gateway's address and the pair's key.
    peer: Peers,
    people: People,
    /// (hub, work, the person whose phrase it was), the newest last.
    ours: Mutex<VecDeque<(String, String, Option<String>)>>,
    /// A line the person spoke over: said once more after the next turn.
    unsaid: Mutex<Option<(String, String)>>,
    listeners: Mutex<Vec<futures::channel::mpsc::UnboundedSender<Outside>>>,
}

impl HubWorks {
    fn new(peer: Peers, people: People) -> Arc<HubWorks> {
        Arc::new(HubWorks {
            peer,
            people,
            ours: Mutex::new(VecDeque::new()),
            unsaid: Mutex::new(None),
            listeners: Mutex::new(Vec::new()),
        })
    }

    /// A phrase of this channel began or added to the hub's work.
    fn began(&self, hub: &str, work: &str, person: Option<String>) {
        if !work.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') || work.is_empty() {
            return;
        }
        let mut ours = lock(&self.ours);
        ours.retain(|(at, id, _)| !(at == hub && id == work));
        ours.push_back((hub.to_string(), work.to_string(), person));
        while ours.len() > OURS_KEPT {
            ours.pop_front();
        }
    }

    /// Whose phrase the work was, when it is this channel's.
    fn person(&self, hub: &str, work: &str) -> Option<Option<String>> {
        lock(&self.ours).iter().find(|(at, id, _)| at == hub && id == work).map(|(_, _, person)| person.clone())
    }

    fn send(&self, outside: Outside) {
        lock(&self.listeners).retain(|listener| listener.unbounded_send(outside.clone()).is_ok());
    }

    fn notices(&self) -> Notices {
        let (sender, receiver) = futures::channel::mpsc::unbounded();
        lock(&self.listeners).push(sender);
        receiver.boxed()
    }

    /// A hub's message: the change of one of this channel's works.
    fn heard(self: &Arc<Self>, hub: &str, message: &Value) {
        let text = |name: &str| message.get(name).and_then(Value::as_str);
        if text("type") != Some("changed") || text("name") != Some("work.changed") {
            return;
        }
        let (Some(work), Some(change)) = (text("subject"), text("change")) else { return };
        if self.person(hub, work).is_none() {
            return;
        }
        if WITHDRAWN_ON.contains(&change) {
            self.send(Outside::new(Unasked::Withdraw { notice: format!("{HUB_NOTICE}{hub}:{work}") }));
            return;
        }
        if SAID_ON.contains(&change) {
            self.tell_line(hub, work);
        }
    }

    /// The work's line as a notice, read from the hub: unless it was said already or touched elsewhere since.
    fn tell_line(self: &Arc<Self>, hub: &str, work: &str) {
        let (Some(person), Some(peer)) = (self.person(hub, work), (self.peer)(hub)) else { return };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else { return };
        let principal = (self.people)(person.as_deref());
        let (works, hub, work) = (self.clone(), hub.to_string(), work.to_string());
        runtime.spawn(async move {
            let request = hub_client().get(format!("{}/api/v1/works/{work}", peer.base())).bearer_auth(peer.key());
            match hub_json(crate::platform::http::peer::as_person(&principal, request)).await {
                Ok(found) => {
                    if let Some(notice) = hub_notice(&hub, &found) {
                        works.send(Outside::new(Unasked::Notice(notice)));
                    }
                }
                Err(error) => tracing::info!(target: "talk", "the hub's work {work} was not read: {error}"),
            }
        });
    }

    /// How the channel said a work's line: said — the hub hears it and opens its question; cut or put off — once
    /// more after the next turn.
    fn delivered(self: &Arc<Self>, hub: &str, work: &str, outcome: Delivery) {
        match outcome {
            Delivery::Said => {
                let (Some(person), Some(peer)) = (self.person(hub, work), (self.peer)(hub)) else { return };
                let Ok(runtime) = tokio::runtime::Handle::try_current() else { return };
                let principal = (self.people)(person.as_deref());
                let work = work.to_string();
                runtime.spawn(async move {
                    let request =
                        hub_client().post(format!("{}/api/v1/works/{work}/said", peer.base())).bearer_auth(peer.key());
                    if let Err(error) = hub_json(crate::platform::http::peer::as_person(&principal, request)).await {
                        tracing::info!(target: "talk", "the hub did not take work {work} as said: {error}");
                    }
                });
            }
            Delivery::Cut | Delivery::Later | Delivery::Shown => {
                *lock(&self.unsaid) = Some((hub.to_string(), work.to_string()));
            }
            Delivery::Dropped | Delivery::Other => {}
        }
    }

    /// A turn through the hub ended: the line not said comes once more.
    fn turn_ended(self: &Arc<Self>) {
        if let Some((hub, work)) = lock(&self.unsaid).take() {
            self.tell_line(&hub, &work);
        }
    }
}

/// The hub's answer out of the API's envelope (`{message, code}`), or why there is none.
async fn hub_json(request: reqwest::RequestBuilder) -> Result<Value, String> {
    let response = request.send().await.map_err(|error| format!("the hub did not answer: {error}"))?;
    let status = response.status();
    let value: Value = response.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        let message = value.get("message").and_then(Value::as_str).unwrap_or_default();
        return Err(format!("the hub answered {status}: {message}"));
    }
    Ok(value.get("message").cloned().unwrap_or(value))
}

/// A hub's work as the channel's notice: its line, when it is not said yet and its last phrase was this computer's
/// (the work's own or a note's, the newest; the panel's `touchedOn`).
fn hub_notice(hub: &str, work: &Value) -> Option<Notice> {
    if work.get("saidAt").is_some_and(|at| !at.is_null()) {
        return None;
    }
    let id = work.get("id").and_then(Value::as_str)?;
    let line = work.pointer("/result/line").and_then(Value::as_str).filter(|line| !line.is_empty())?;
    let mut device = work.pointer("/origin/device").and_then(Value::as_str);
    let mut at = work.get("startedAt").and_then(Value::as_u64).unwrap_or(0);
    for note in work.get("notes").and_then(Value::as_array).into_iter().flatten() {
        let noted = note.get("at").and_then(Value::as_u64).unwrap_or(0);
        if noted >= at {
            at = noted;
            device = note.pointer("/origin/device").and_then(Value::as_str);
        }
    }
    if device != Some(COMPUTER) {
        return None;
    }
    let asks = work.pointer("/question/kind").is_some_and(|kind| !kind.is_null());
    Some(Notice {
        notice: format!("{HUB_NOTICE}{hub}:{id}"),
        text: line.to_string(),
        speaker: None,
        why: if asks { NoticeWhy::WorkQuestion } else { NoticeWhy::WorkReady },
        work: Some(id.to_string()),
        question: asks.then(|| Why::Ready.as_str().to_string()),
        urgency: Urgency::Now,
        to: Some(door::Hint { device: Some(COMPUTER.to_string()), ..door::Hint::default() }),
        until: None,
        expects: Expects::Free,
    })
}

/// The hub and the work of a hub's notice.
fn hub_work_of(notice: &str) -> Option<(&str, &str)> {
    notice.strip_prefix(HUB_NOTICE)?.rsplit_once(':')
}

/// The window's reason as the panel names it (`why`), from the phrase's context.
fn why_of(phrase: &Phrase) -> Option<&'static str> {
    if phrase.opened != Opened::Window {
        return None;
    }
    if phrase.context.over.is_some() {
        return Some("barge");
    }
    match (phrase.context.answer_to.as_deref(), phrase.context.after) {
        (Some("ready"), _) | (_, Some(door::After::Notice)) => Some("ready"),
        (Some("work"), _) | (_, Some(door::After::Work)) => Some("work"),
        _ => Some("question"),
    }
}

/// One event of the hub's talk (the panel's stream) as the door's.
fn door_of_json(event: &Value, failed_line: &str) -> Vec<What> {
    let text = |name: &str| event.get(name).and_then(Value::as_str).unwrap_or_default().to_string();
    match event.get("type").and_then(Value::as_str) {
        Some("start") => vec![What::Begun { request: text("request"), by: text("by") }],
        Some("phrase") => vec![What::Say { text: text("text") }],
        Some("tool") => vec![What::Wait { why: WaitWhy::Tool, expect_ms: None, name: Some(text("name")) }],
        Some("done") => {
            let (said, ms) = (text("said"), event.get("ms").and_then(Value::as_u64).unwrap_or(0));
            let mut whats = Vec::new();
            if let Some(work) = event.get("work").and_then(Value::as_str) {
                whats.push(What::Work { work: work.to_string(), title: None, state: None });
            }
            if event.get("dropped").and_then(Value::as_bool) == Some(true) {
                whats.push(What::End { outcome: Outcome::Dropped, reason: None, said, ms, error: None });
                return whats;
            }
            let remind = event.get("remind").and_then(Value::as_str).map(str::to_string);
            let asks = event.get("asks").and_then(Value::as_bool) == Some(true);
            let question = if remind.is_some() { Some("ready") } else { asks.then_some("question") };
            if let Some(remind) = remind {
                whats.push(What::Say { text: remind });
            }
            match question {
                Some(question) => {
                    whats.push(What::Ask { question: question.to_string(), text: None, expects: Expects::Free });
                    whats.push(What::End { outcome: Outcome::Asked, reason: None, said, ms, error: None });
                }
                None => whats.push(What::End { outcome: Outcome::Answered, reason: None, said, ms, error: None }),
            }
            whats
        }
        Some("error") => vec![
            What::Say { text: failed_line.to_string() },
            What::End {
                outcome: Outcome::Failed,
                reason: Some(Failure::Error),
                said: String::new(),
                ms: 0,
                error: Some(text("message")),
            },
        ],
        _ => Vec::new(),
    }
}

impl HomeThinking {
    /// The phrase goes to the hub's talk; its stream comes back as the door's events.
    fn through(&self, app: &App, hub: String, peer: PeerClient, phrase: Phrase) -> Events {
        let person = phrase.who.person.clone();
        let principal = (people_of(app))(person.as_deref());
        let failed_line = crate::talk::pack::of(app.talk.lang(crate::core::origin::COMPUTER)).failed.to_string();
        let mut body = json!({ "text": phrase.text, "voice": true });
        match why_of(&phrase) {
            Some(why) => {
                body["via"] = json!("answer");
                body["why"] = json!(why);
            }
            None => body["via"] = json!("button"),
        }
        if let Some(heard) = &phrase.heard {
            body["heard"] = json!({ "by": heard.by, "ms": heard.ms });
        }
        let cancel = CancellationToken::new();
        lock(&self.turns).insert(phrase.turn.clone(), cancel.clone());
        let (turns, turn_id) = (self.turns.clone(), phrase.turn.clone());
        let (sender, receiver) = futures::channel::mpsc::unbounded();
        let turn = phrase.turn.clone();
        let event = move |what: What| Event::new(&turn, None, what);
        let request = crate::platform::http::peer::as_person(
            &principal,
            hub_client()
                .post(format!("{}/api/v1/talk", peer.base()))
                .header(reqwest::header::ACCEPT, "text/event-stream")
                .json(&body)
                .bearer_auth(peer.key()),
        );
        let works = self.hub_works.clone();
        tokio::spawn(async move {
            // Whatever ends the hub's answer (its end, a stop, the audio side gone): the turn is over here.
            let answer = async {
                let failed = |error: String| {
                    vec![
                        What::Say { text: failed_line.clone() },
                        What::End {
                            outcome: Outcome::Failed,
                            reason: Some(Failure::Error),
                            said: String::new(),
                            ms: 0,
                            error: Some(error),
                        },
                    ]
                };
                let response = tokio::select! {
                    _ = cancel.cancelled() => return,
                    response = request.send() => response,
                };
                let whats: Vec<What> = match response {
                    Ok(response) if response.status().is_success() => {
                        let mut bytes = response.bytes_stream();
                        let mut pending = String::new();
                        loop {
                            let chunk = tokio::select! {
                                _ = cancel.cancelled() => return,
                                chunk = bytes.next() => chunk,
                            };
                            let Some(Ok(chunk)) = chunk else { break };
                            pending.push_str(&String::from_utf8_lossy(&chunk));
                            while let Some(at) = pending.find("\n\n") {
                                let frame: String = pending.drain(..at + 2).collect();
                                let data: String = frame
                                    .lines()
                                    .filter_map(|line| line.strip_prefix("data:"))
                                    .map(str::trim_start)
                                    .collect::<Vec<_>>()
                                    .join("\n");
                                let Ok(value) = serde_json::from_str::<Value>(&data) else { continue };
                                for what in door_of_json(&value, &failed_line) {
                                    if let What::Work { work, .. } = &what {
                                        works.began(&hub, work, person.clone());
                                    }
                                    if sender.unbounded_send(event(what)).is_err() {
                                        return;
                                    }
                                }
                            }
                        }
                        Vec::new()
                    }
                    Ok(response) => failed(format!("the hub answered {}", response.status())),
                    Err(error) => failed(format!("the hub did not answer: {error}")),
                };
                for what in whats {
                    let _ = sender.unbounded_send(event(what));
                }
            };
            answer.await;
            lock(&turns).remove(&turn_id);
            works.turn_ended();
        });
        receiver.boxed()
    }
}

/// The HTTP client of the pass to the hub.
fn hub_client() -> reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| crate::utils::http::client_builder().build().expect("the HTTP client builds")).clone()
}

fn people_of(app: &App) -> People {
    let (users, household) = (app.access.users.clone(), app.access.household.clone());
    Arc::new(move |id| {
        id.and_then(|id| users.get(id).ok())
            .filter(|person| person.status == crate::access::users_repository::UserStatus::Active)
            .map(|person| user_principal(&person.id, &person.name, person.role, Some(&person.restrictions)))
            .unwrap_or_else(|| household.speaker(None))
    })
}

impl door::Thinking for HomeThinking {
    fn phrase(&self, phrase: Phrase) -> Events {
        if let Some(app) = self.app.upgrade() {
            if let Some((hub, peer)) = hub(&app) {
                return self.through(&app, hub, peer, phrase);
            }
        }
        door::Thinking::phrase(&*self.local, phrase)
    }

    fn tell(&self, news: door::News) {
        match &news.what {
            Told::Stop { turn, .. } => {
                let passed = lock(&self.turns).remove(turn);
                if let Some(cancel) = passed {
                    // Dropping the hub's stream stops its answer there, as the panel's does.
                    cancel.cancel();
                    return;
                }
            }
            Told::Delivered { notice: Some(notice), outcome, .. } => {
                if let Some((hub, work)) = hub_work_of(notice) {
                    self.hub_works.delivered(hub, work, *outcome);
                    return;
                }
            }
            _ => {}
        }
        door::Thinking::tell(&*self.local, news);
    }

    /// The computer's own works' and the hub's works' of this channel.
    fn notices(&self) -> Notices {
        futures::stream::select(door::Thinking::notices(&*self.local), self.hub_works.notices()).boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hub's work as its `/api/v1/works/{id}` gives it.
    fn hub_work(device: &str, noted_on: Option<&str>) -> Value {
        let mut work = json!({
            "id": "7", "kind": "widget", "title": "Курс валют", "state": "ready",
            "origin": { "device": device, "activator": "button" },
            "notes": [], "startedAt": 1000,
            "result": { "line": "Курс валют: на стол?" },
        });
        if let Some(noted_on) = noted_on {
            work["notes"] = json!([{ "text": "и ещё юань", "at": 2000, "origin": { "device": noted_on } }]);
        }
        work
    }

    #[test]
    fn a_hub_s_work_last_touched_here_is_the_channel_s_line_to_say() {
        let notice = hub_notice("h:kitchen", &hub_work(COMPUTER, None)).expect("a notice");
        assert_eq!((notice.notice.as_str(), notice.text.as_str()), ("hub-work:h:kitchen:7", "Курс валют: на стол?"));
        assert_eq!((notice.why, notice.question.as_deref()), (NoticeWhy::WorkReady, None));
        assert_eq!(hub_work_of(&notice.notice), Some(("h:kitchen", "7")));
        // Its last phrase was the station's: the station says it.
        assert!(hub_notice("h:kitchen", &hub_work(COMPUTER, Some("kitchen"))).is_none());
        assert!(hub_notice("h:kitchen", &hub_work("kitchen", Some(COMPUTER))).is_some());
        let mut said = hub_work(COMPUTER, None);
        said["saidAt"] = json!(3000);
        assert!(hub_notice("h:kitchen", &said).is_none(), "said already");
        let mut asks = hub_work(COMPUTER, None);
        asks["question"] = json!({ "text": "На стол?", "until": 9000, "kind": "draft" });
        let notice = hub_notice("h:kitchen", &asks).expect("a notice");
        assert_eq!((notice.why, notice.question.as_deref()), (NoticeWhy::WorkQuestion, Some("ready")));
    }

    /// A hub that gives work 7 and hears what was said.
    async fn pretend_hub() -> (PeerClient, Arc<Mutex<Vec<String>>>) {
        use axum::routing::{get, post};
        let asked: Arc<Mutex<Vec<String>>> = Arc::default();
        let (read, said) = (asked.clone(), asked.clone());
        let router = axum::Router::new()
            .route(
                "/api/v1/works/{id}",
                get(move |headers: axum::http::HeaderMap, axum::extract::Path(id): axum::extract::Path<String>| {
                    let read = read.clone();
                    async move {
                        let key = headers.get("authorization").and_then(|key| key.to_str().ok()).unwrap_or_default();
                        lock(&read).push(format!("read {id} {key}"));
                        axum::Json(json!({ "message": hub_work(COMPUTER, None), "code": 200 }))
                    }
                }),
            )
            .route(
                "/api/v1/works/{id}/said",
                post(move |axum::extract::Path(id): axum::extract::Path<String>| {
                    let said = said.clone();
                    async move {
                        lock(&said).push(format!("said {id}"));
                        axum::Json(json!({ "message": { "id": id }, "code": 200 }))
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, router).await });
        (PeerClient::new("127.0.0.1", port, "pair-key"), asked)
    }

    async fn next(notices: &mut Notices) -> Unasked {
        tokio::time::timeout(std::time::Duration::from_secs(5), notices.next()).await.expect("in time").unwrap().what
    }

    #[tokio::test]
    async fn the_turbo_s_channel_says_the_hub_s_ready_line_of_its_own_work_and_tells_the_hub_it_was_said() {
        let (peer, asked) = pretend_hub().await;
        let people: People = Arc::new(|_| crate::core::identity::principal::system());
        let works = HubWorks::new(Box::new(move |hub| (hub == "h:kitchen").then(|| peer.clone())), people);
        let mut notices = works.notices();
        let changed = |work: &str, change: &str| json!({ "type": "changed", "name": "work.changed", "subject": work, "change": change });

        // A work of somebody else's phrase: not this channel's to say.
        works.heard("h:kitchen", &changed("7", "ready"));
        works.began("h:kitchen", "7", None);
        works.heard("h:kitchen", &json!({ "type": "request", "request": {} }));
        works.heard("h:kitchen", &changed("7", "started"));
        works.heard("h:kitchen", &changed("7", "ready"));
        let Unasked::Notice(notice) = next(&mut notices).await else { panic!("a notice") };
        assert_eq!((notice.notice.as_str(), notice.text.as_str()), ("hub-work:h:kitchen:7", "Курс валют: на стол?"));
        assert_eq!(*lock(&asked), ["read 7 Bearer pair-key"], "read once, with the pair's key");

        // Spoken over: once more after the next turn; said: the hub hears it.
        works.delivered("h:kitchen", "7", Delivery::Cut);
        works.turn_ended();
        let Unasked::Notice(again) = next(&mut notices).await else { panic!("the notice again") };
        assert_eq!(again.notice, notice.notice);
        works.delivered("h:kitchen", "7", Delivery::Said);
        for _ in 0..50 {
            if lock(&asked).len() == 3 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(lock(&asked)[2], "said 7");
        works.heard("h:kitchen", &changed("7", "said"));
        assert!(matches!(next(&mut notices).await, Unasked::Withdraw { notice } if notice == "hub-work:h:kitchen:7"));
    }

    #[test]
    fn the_hub_s_stream_reads_as_the_door_s_events() {
        let said = door_of_json(&json!({"type": "phrase", "text": "Пять часов."}), "—");
        assert_eq!(said, [What::Say { text: "Пять часов.".into() }]);
        let asked = door_of_json(&json!({"type": "done", "said": "Добавить?", "ms": 10, "asks": true}), "—");
        assert!(
            matches!(&asked[..], [What::Ask { question, .. }, What::End { outcome: Outcome::Asked, .. }] if question == "question")
        );
        let reminded = door_of_json(&json!({"type": "done", "said": "", "remind": "Курс валют: на стол?"}), "—");
        assert!(matches!(&reminded[0], What::Say { text } if text.starts_with("Курс валют")));
        let broke = door_of_json(&json!({"type": "error", "message": "boom"}), "Не получилось.");
        assert!(matches!(&broke[1], What::End { outcome: Outcome::Failed, error: Some(error), .. } if error == "boom"));
    }
}
