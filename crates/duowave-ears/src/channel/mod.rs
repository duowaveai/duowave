//! The computer's voice channel (docs/plans/audio-interface-3.md, part К): the computer is spoken to as the station
//! is — called from any program by the key, its answer heard from its own speakers, cut by a word, and its «Готово»
//! said by itself, the panel's window open or not. The ears and the voice are the app's (its audio port,
//! `ear::port`); the panel and the strip only show and let one press.
//!
//! - **Press** (`press`): speaking — fall silent and listen; listening — end the thought now; otherwise listen (the
//!   «слушаю» tone), going on with a thought left hanging. **Let go** after holding longer than 0.6 s — the thought
//!   ends now. **Stop** — everything ends.
//! - **The ear** (endpoint.rs) cuts the sound into pieces; each is recognized once («Слух», `SpeechService`) and judged
//!   by «Чутьё» (`finished`). The thought is over — the «услышала» tone, and the phrase goes through the door to
//!   Thinking (`door::Thinking`), as `opened: button` (`window` after our question, `word` after an app's word).
//! - **The voice** (mouth.rs): the answer's `say` in order, in the computer's voice.
//! - **The window**: the answer asks (`ask`), or touched a work — the computer listens six seconds by itself; the
//!   phrase goes back with `context`, and the talk layer's closed window takes it or not.
//! - **«Готово»** (К6): Thinking's `notice` is said when the computer is free — not listening, not speaking, the
//!   microphone not held by a call —; at night, in a call or without a voice of its own, a notification instead;
//!   `delivered` tells Thinking what became of it.
//! - **Speaking over** (К7): after the echo check passed, the microphone listens while the computer speaks; half a
//!   second of speech pauses the sound, the piece is recognized: words that are not ours cut the answer and go to the
//!   window with why `barge`; our own echo or nothing — the sound goes on.
//! - **Two ears in a room** (К9): a phrase heard by itself that begins with a station's word is the station's.
//! - **The word** (К10, share.rs): while «Слушать слово» is on, the capture runs all the time and its sound goes only
//!   to the word's program; its `wake` opens the thought with the sound before the word.
//!
//! The channel is one task: what it is asked, the sound, the answer's events, the mouth's news and the notices come to
//! it in turn, so its state has one owner. What it does is seen in `view()` and on the live channel (`voice`).

pub mod endpoint;
pub mod mouth;
pub mod routes;
pub mod share;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use serde::Serialize;
use tokio::sync::{broadcast, mpsc, oneshot, watch};
// The test's clock: the channel's timers follow tokio's time, so a test runs them as fast as it likes.
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use self::endpoint::{Ending, Endpoint, Keyed, Step, RATE};
use self::mouth::{tones, Ask as Say, Mouth, Told as Spoken};
use crate::core::events::{EventBus, GatewayEvent};
use crate::core::requests::{self as journal, Owner, RequestHandle, RequestPhase, RequestStatus};
use crate::door::{
    self, After, Budget, By, Context, Delivery, Events, Kind, News, Notice, Notices, Opened, Outcome, Phrase, Place,
    StopWhy, Thinking, To, Told, Unasked, What, Who,
};
use crate::ear::port::{Access, AudioPort, Frames};
use crate::sense::{self, End, ForUs, Window};
use crate::settings::definitions::{EchoCheck, KeyActivation, QuietHours, TalkVoice, QUIET_HOURS, TALK_VOICE};
use crate::settings::SettingsService;
use crate::speech::speech_pipeline::UtteranceAudio;
use crate::speech::{SpeechService, VoiceService};
use crate::utils::time::now_ms;

/// The device this channel is: the talk layer's computer.
pub const DEVICE: &str = crate::core::origin::COMPUTER;
/// What the phrases of this ear are called in their origin (`Origin.name`): the app's ear, not the panel.
pub const APP: &str = "app";
/// How long the window listens by itself after our question or a work's line.
pub const WINDOW_MS: u64 = 6_000;
/// Held at least this long, the key is a walkie-talkie: letting it go ends the thought; shorter, a tap (Handy's
/// `hold_threshold_ms`, its default).
pub const HOLD_MS: u64 = 300;
/// A thought left hanging waits this long for the next press.
const PARK_MS: u64 = crate::sense::thought::PARK_MS;
/// «Готово» waits this long for the computer to be free; then the next talk reminds of it.
const NOTICE_WAITS: Duration = Duration::from_secs(10 * 60);
/// The level goes to the strip at most this often.
const LEVEL_EVERY: Duration = Duration::from_millis(125);
/// The first second of our own speech is not listened to: the echo cancelling settles.
const BARGE_DEAF_MS: u64 = 1_000;
/// Speech this long over ours pauses it.
const BARGE_SPEECH_MS: u64 = 500;
/// A paused answer goes on at most this late when what was heard is not for us.
const BARGE_RESUME: Duration = Duration::from_secs(2);
/// The echo check's phrase is heard for this long after it was said.
const CHECK_TAIL_MS: u64 = 600;
/// Less sound than this while the check's phrase was said: the microphone gave nothing, and that is no check.
const CHECK_SOUND_LEAST_MS: u64 = 500;
/// How long Thinking may say nothing at all before the channel gives up on the turn (the contract's rule 7).
const TURN_SILENT: Duration = Duration::from_secs(60);
/// The microphone gives no sound at all this long: the ear stops listening (27.11, the trial of main: with macOS's
/// question about the microphone open, no frames came and «Слушаю» stayed on for good).
const NO_SOUND: Duration = Duration::from_secs(20);
/// The same before the thought's first sound: macOS's question about the microphone is read, the hand goes to
/// «Разрешить» (27.11, the owner: 20 s closed the listening just before the answer).
const NO_SOUND_YET: Duration = Duration::from_secs(60);
/// The first sound after this long a wait (macOS asked): the «слушаю» tone again — now it hears.
const LATE_SOUND: Duration = Duration::from_millis(1500);

/// What the channel does now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Quiet,
    Listening,
    Recognizing,
    Thinking,
    Speaking,
    /// Listening by itself after our question, a work's line, a notice.
    Window,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Quiet => "quiet",
            State::Listening => "listening",
            State::Recognizing => "recognizing",
            State::Thinking => "thinking",
            State::Speaking => "speaking",
            State::Window => "window",
        }
    }
}

/// What is seen of the channel: its state and the strip's details.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    pub state: State,
    /// Why the window is open: `question`, `work`, `ready`, `barge`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// Seconds left of the window, or of the silence a hanging thought waits for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wait: Option<u64>,
    /// The journal's request of the turn going on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request: Option<String>,
    /// The thought left hanging, which the next press goes on with.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parked: Option<String>,
    /// A line the channel said of itself last («Жду.», «Слушать некому…»), for the strip.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Default for View {
    fn default() -> View {
        View { state: State::Quiet, why: None, wait: None, request: None, parked: None, note: None }
    }
}

/// What the channel needs: the device's sound, the door, recognition and the computer's voice, the settings, the bus.
#[derive(Clone)]
pub struct Deps {
    pub port: Arc<dyn AudioPort>,
    pub thinking: Arc<dyn Thinking>,
    /// «Слух»: the home's recognition (SpeechService, through «Дом»'s chain).
    pub hear: Arc<dyn Hear>,
    /// The computer's voice (VoiceService::for_computer).
    pub speak: Arc<dyn Speak>,
    pub settings: Arc<SettingsService>,
    pub events: EventBus,
    /// The words of the home's stations — their «Алиса» and their activator apps' words (К9).
    pub others: Arc<dyn Fn() -> Vec<String> + Send + Sync>,
    /// Where the question's trip is written, as every ear writes it (`Trips`).
    pub trips: Arc<dyn Trips>,
}

/// Where an ear writes the question's trip — the same for every ear, a station's session writes it so too
/// (`core::requests`, the journal): the ear opens the trip when it starts listening, writes its own steps (listen,
/// recognize, speak) and ends it once the answer is said; Thinking on this gateway goes on in the same trip, one
/// request per phrase. The panel's orbit follows any ear's trip, not only a station's (the owner, 27.11: «не
/// привязано к колонке… любое ухо тоже сообщает»).
pub trait Trips: Send + Sync {
    /// A question begins on this ear; None — no journal now (the gateway stops).
    fn begin(&self) -> Option<Arc<RequestHandle>>;
    /// The question goes to Thinking: Thinking here goes on in this trip; Thinking on another gateway (the hub of a
    /// Турбо) opens its own, which the trip names when it begins (`RequestHandle::thought_in`).
    fn offer(&self, trip: Arc<RequestHandle>);
}

/// No journal: the trips go nowhere (a channel without one, in tests).
pub struct NoTrips;

impl Trips for NoTrips {
    fn begin(&self) -> Option<Arc<RequestHandle>> {
        None
    }

    fn offer(&self, _trip: Arc<RequestHandle>) {}
}

/// «Слух», as the channel uses it: a piece of 16 kHz mono in, its text out.
pub trait Hear: Send + Sync {
    /// Somebody is there to recognize.
    fn ready(&self) -> bool;
    /// It hears at home, not in a cloud.
    fn at_home(&self) -> bool;
    fn recognize(&self, pcm: Vec<i16>) -> futures::future::BoxFuture<'static, Result<String, String>>;
}

/// The computer's voice, as the channel uses it: a phrase in, its sound and rate out.
pub trait Speak: Send + Sync {
    /// The home has a voice of its own for the computer.
    fn ready(&self) -> bool;
    fn synthesize(
        &self,
        text: &str,
        cancel: &CancellationToken,
    ) -> futures::future::BoxFuture<'static, Option<(Vec<i16>, u32)>>;
}

/// The home's «Слух» for the channel: `SpeechService::recognize`, with «Дом»'s chain behind it (a Турбо, the own
/// server, the cloud) under `away`.
pub struct HomeHearing(pub Arc<SpeechService>);

impl Hear for HomeHearing {
    fn ready(&self) -> bool {
        self.0.recognizes()
    }

    fn at_home(&self) -> bool {
        self.0.at_home()
    }

    fn recognize(&self, pcm: Vec<i16>) -> futures::future::BoxFuture<'static, Result<String, String>> {
        let speech = self.0.clone();
        Box::pin(async move {
            let to_ms = pcm.len() as f64 * 1000.0 / f64::from(RATE);
            let audio = UtteranceAudio { pcm, from_ms: 0.0, to_ms, gain_db: 0.0 };
            speech
                .recognize(&audio, &[], &CancellationToken::new())
                .await
                .map(|heard| heard.text)
                .map_err(|error| format!("{error:#}"))
        })
    }
}

/// The computer's voice for the channel: `VoiceService::for_computer` — the Турбо's voice, the own server, the cloud,
/// its ready phrases first.
pub struct HomeVoice(pub Arc<VoiceService>);

impl Speak for HomeVoice {
    fn ready(&self) -> bool {
        self.0.for_computer().is_some()
    }

    fn synthesize(
        &self,
        text: &str,
        cancel: &CancellationToken,
    ) -> futures::future::BoxFuture<'static, Option<(Vec<i16>, u32)>> {
        let speech = self.0.for_computer();
        let (text, cancel) = (text.to_string(), cancel.clone());
        Box::pin(async move {
            let speech = speech?;
            match speech.synthesize_wav(&text, &cancel).await {
                Ok(wav) => match crate::speech::audio::pcm::decode_wav(&wav) {
                    Ok(wav) => Some((wav.pcm, wav.sample_rate)),
                    Err(error) => {
                        tracing::warn!(target: "talk", "the computer's voice gave a sound it cannot play: {error:#}");
                        None
                    }
                },
                Err(error) => {
                    if !cancel.is_cancelled() {
                        tracing::warn!(target: "talk", "the computer's voice did not answer: {error:#}");
                    }
                    None
                }
            }
        })
    }
}

/// An app's word heard on the computer (К10): which word, the agent it names, the sound before it.
#[derive(Debug, Clone, PartialEq)]
pub struct Woken {
    pub word: String,
    /// The forms recognition may write it in: the text loses them at its start.
    pub heard: Vec<String>,
    pub agent: Option<String>,
    pub before: Vec<i16>,
}

/// What the echo check found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Checked {
    pub ok: bool,
    pub heard: String,
}

enum Command {
    /// The gateway stops: the microphone and the sound end, and the channel with them.
    Shutdown,
    /// «Сказать» or the key; `held` — the key is down now and its release ends the thought.
    Press {
        held: bool,
    },
    Release {
        held_ms: u64,
    },
    Stop,
    Word(Woken),
    Check(oneshot::Sender<Result<Checked, String>>),
}

/// The channel's handle: what the routes, the app's key and the word ask it.
pub struct VoiceChannel {
    commands: mpsc::UnboundedSender<Command>,
    view: watch::Receiver<View>,
    /// The person signed in on this computer's panel who pressed «Сказать» last: their talk and memory.
    person: Arc<Mutex<Option<(String, String)>>>,
    settings: Arc<SettingsService>,
    port: Arc<dyn AudioPort>,
    sound: broadcast::Sender<Arc<Vec<i16>>>,
    open_panel: watch::Sender<u64>,
    /// The strip's × (and the dock's): stopped by the person, the strip over the windows goes at once.
    closed: watch::Sender<u64>,
    /// The key «Сказать» as chosen (К4): the app registers it, and again when it changes.
    key: watch::Sender<String>,
    /// Another program holds the key: the app could not register it.
    key_taken: std::sync::atomic::AtomicBool,
    /// Whether an activator app stands on this computer: «Слушать слово» shows only then (К10).
    word_apps: std::sync::OnceLock<Arc<dyn Fn() -> bool + Send + Sync>>,
}

impl VoiceChannel {
    /// The channel starts its task.
    pub fn start(deps: Deps) -> Arc<VoiceChannel> {
        let (commands, receiver) = mpsc::unbounded_channel();
        let (view_by, view) = watch::channel(View::default());
        let person = Arc::new(Mutex::new(None));
        let (sound, _) = broadcast::channel(64);
        let channel = Arc::new(VoiceChannel {
            commands,
            view,
            person: person.clone(),
            settings: deps.settings.clone(),
            port: deps.port.clone(),
            sound: sound.clone(),
            open_panel: watch::channel(0).0,
            closed: watch::channel(0).0,
            key: watch::channel(
                deps.settings
                    .get(&crate::settings::definitions::TALK_KEY, None)
                    .unwrap_or_else(|| crate::settings::definitions::DEFAULT_TALK_KEY.to_string()),
            )
            .0,
            key_taken: std::sync::atomic::AtomicBool::new(false),
            word_apps: std::sync::OnceLock::new(),
        });
        let (told_by, told) = mpsc::unbounded_channel();
        let mouth = Mouth::start(deps.port.clone(), deps.speak.clone(), told_by);
        let notices = deps.thinking.notices();
        let actor = Actor {
            mouth,
            told,
            notices,
            view: view_by,
            person,
            sound,
            state: State::Quiet,
            capture: Capture::Off,
            frames: None,
            ear: None,
            turn: None,
            parked: None,
            waiting: VecDeque::new(),
            telling: None,
            barge: None,
            check: None,
            next: 0,
            level_at: Instant::now(),
            window_until: None,
            word_on: false,
            owner: journal::new_owner(),
            deps,
        };
        tokio::spawn(actor.run(receiver));
        channel
    }

    pub fn press(&self) {
        let _ = self.commands.send(Command::Press { held: false });
    }

    /// The key went down: as `press`, and while it is held no pause ends the thought — its release does.
    pub fn press_held(&self) {
        let _ = self.commands.send(Command::Press { held: true });
    }

    /// The gateway stops: the microphone closes, the sound stops, the channel ends.
    pub fn shutdown(&self) {
        let _ = self.commands.send(Command::Shutdown);
    }

    pub fn release(&self, held_ms: u64) {
        let _ = self.commands.send(Command::Release { held_ms });
    }

    /// The person stopped the talk (the strip's ×): silence, and the strip goes at once (27.11: the × of the strip over
    /// the windows did nothing a person could see; the strip went 4 seconds later by itself).
    pub fn stop(&self) {
        let _ = self.commands.send(Command::Stop);
        self.closed.send_modify(|closed| *closed += 1);
    }

    /// The strip was closed by the person: the app hides it now.
    pub fn strip_closed(&self) -> watch::Receiver<u64> {
        self.closed.subscribe()
    }

    /// An app's word woke the computer (К10).
    pub fn woken(&self, woken: Woken) {
        let _ = self.commands.send(Command::Word(woken));
    }

    /// The echo check (К7): the computer says a phrase and listens to itself.
    pub async fn check(&self) -> Result<Checked, String> {
        let (reply, answer) = oneshot::channel();
        self.commands.send(Command::Check(reply)).map_err(|_| crate::tr!("Голосовой канал остановлен."))?;
        answer.await.map_err(|_| crate::tr!("Голосовой канал остановлен."))?
    }

    /// The person on this computer's panel: who speaks into its microphone from now on.
    pub fn set_person(&self, id: &str, name: &str) {
        *self.person.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((id.to_string(), name.to_string()));
    }

    pub fn view(&self) -> View {
        self.view.borrow().clone()
    }

    /// The view as it changes: the app's strip follows it (К8).
    pub fn watch(&self) -> watch::Receiver<View> {
        self.view.clone()
    }

    pub fn settings(&self) -> TalkVoice {
        self.settings.get(&TALK_VOICE, None).unwrap_or_default()
    }

    pub fn port(&self) -> &Arc<dyn AudioPort> {
        &self.port
    }

    /// The ear's sound as it is captured, for the word's program (share.rs): only while «Слушать слово» is on.
    pub fn sound(&self) -> broadcast::Receiver<Arc<Vec<i16>>> {
        self.sound.subscribe()
    }

    /// The strip asked for the panel's window: the app follows this (К8).
    pub fn open_panel(&self) {
        self.open_panel.send_modify(|asked| *asked += 1);
    }

    pub fn panel_asked(&self) -> watch::Receiver<u64> {
        self.open_panel.subscribe()
    }

    /// The key «Сказать» as it changes: the app follows it.
    pub fn key(&self) -> watch::Receiver<String> {
        self.key.subscribe()
    }

    /// A new key was chosen (the routes).
    pub fn key_changed(&self, key: &str) {
        self.key.send_replace(key.to_string());
    }

    /// Whether the app could register the key: another program may hold it.
    pub fn set_key_taken(&self, taken: bool) {
        self.key_taken.store(taken, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn key_taken(&self) -> bool {
        self.key_taken.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Who tells whether an activator app stands here (the gateway, once its apps are known).
    pub fn set_word_apps(&self, word_apps: Arc<dyn Fn() -> bool + Send + Sync>) {
        let _ = self.word_apps.set(word_apps);
    }

    /// An activator app stands on this computer.
    pub fn word_app(&self) -> bool {
        self.word_apps.get().is_some_and(|word_apps| word_apps())
    }

    /// The settings changed (the routes): the capture follows «Слушать слово» at once.
    pub fn settings_changed(&self) {
        // The task reads the settings on every step; a press-free change of the word's switch is picked up here.
        let _ = self.commands.send(Command::Release { held_ms: 0 });
    }
}

/// Who ends a thought the key does not hold (a tap, «два нажатия», «Сказать»): «Чутьё»'s pause and the next press
/// (`auto_end`, the default), or the next press only — Handy's way (the owner, 27.11).
fn untouched(voice: &TalkVoice) -> Keyed {
    if voice.auto_end {
        Keyed::Free
    } else {
        Keyed::Locked
    }
}

/// How the port captures now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Capture {
    Off,
    Plain,
    Voice,
}

/// A thought being heard.
struct Ear {
    endpoint: Endpoint,
    opened: Opened,
    after: Option<After>,
    answer_to: Option<String>,
    over: Option<String>,
    word: Option<Woken>,
    texts: Vec<String>,
    pieces: mpsc::UnboundedSender<(u32, Vec<i16>)>,
    heard: mpsc::UnboundedReceiver<Heard>,
    ending: Option<Ending>,
    by_cloud: bool,
    ms: u64,
    /// The parked thought it goes on with.
    goes_on: Option<String>,
    /// When the microphone last gave a frame.
    sound_at: Instant,
    /// The microphone gave a frame for this thought at all.
    heard_any: bool,
    /// The question's trip in the journal: from the press, or from the first speech in a window.
    trip: Option<Arc<RequestHandle>>,
}

/// A piece recognized.
struct Heard {
    text: String,
    thought: Option<sense::Thought>,
    cloud: bool,
    ms: u64,
}

/// A turn going on behind the door.
struct Turn {
    id: String,
    events: Events,
    ended: Option<Outcome>,
    asked: Option<String>,
    work: bool,
    request: Option<String>,
    /// The phrases sent to the mouth and how much of each was heard.
    said: Vec<(u64, String, Option<bool>)>,
    last_event: Instant,
    /// The question's trip, the ear's: it ends once the answer is said.
    trip: Option<Arc<RequestHandle>>,
}

/// A thought left hanging.
struct Parked {
    text: String,
    until: Instant,
}

/// The answer listened to while it plays (К7).
struct Barge {
    endpoint: Endpoint,
    heard_from: Option<Instant>,
    paused: Option<Instant>,
    pieces: mpsc::UnboundedSender<(u32, Vec<i16>)>,
    heard: mpsc::UnboundedReceiver<Heard>,
}

/// The echo check going on.
struct Check {
    id: u64,
    sound: Vec<i16>,
    phrase: String,
    said_at: Option<Instant>,
    reply: oneshot::Sender<Result<Checked, String>>,
}

struct Actor {
    deps: Deps,
    mouth: Mouth,
    told: mpsc::UnboundedReceiver<Spoken>,
    notices: Notices,
    view: watch::Sender<View>,
    person: Arc<Mutex<Option<(String, String)>>>,
    sound: broadcast::Sender<Arc<Vec<i16>>>,
    state: State,
    capture: Capture,
    frames: Option<Frames>,
    ear: Option<Ear>,
    turn: Option<Turn>,
    parked: Option<Parked>,
    waiting: VecDeque<(Notice, Instant)>,
    telling: Option<(Notice, u64)>,
    barge: Option<Barge>,
    check: Option<Check>,
    next: u64,
    level_at: Instant,
    window_until: Option<Instant>,
    /// «Слушать слово» as the capture last followed it: the frames go to the word's program then.
    word_on: bool,
    /// The channel's claim on its trips: Thinking going on in one does not end it, the channel does once it is said.
    owner: Owner,
}

/// The next of an optional receiver, or never.
async fn next_of<T>(receiver: &mut Option<mpsc::UnboundedReceiver<T>>) -> Option<T> {
    match receiver.as_mut() {
        Some(receiver) => receiver.recv().await,
        None => std::future::pending().await,
    }
}

impl Actor {
    async fn run(mut self, mut commands: mpsc::UnboundedReceiver<Command>) {
        let mut tick = tokio::time::interval(Duration::from_millis(500));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        self.follow_capture();
        loop {
            tokio::select! {
                command = commands.recv() => match command {
                    Some(Command::Shutdown) | None => {
                        self.mouth.ask(Say::Stop);
                        self.stop_turn(StopWhy::Gone);
                        if self.capture != Capture::Off {
                            self.deps.port.stop_capture();
                        }
                        return;
                    }
                    Some(command) => self.command(command),
                },
                frame = next_of(&mut self.frames) => match frame {
                    Some(frame) => self.frame(frame),
                    None => {
                        // The port stopped the capture by itself (a device changed): heard so far is the thought.
                        self.frames = None;
                        self.capture = Capture::Off;
                        if self.ear.is_some() {
                            self.end_thought(Ending::Silence);
                        }
                    }
                },
                heard = async {
                    match self.ear.as_mut() {
                        Some(ear) => ear.heard.recv().await,
                        None => std::future::pending().await,
                    }
                } => {
                    if let Some(heard) = heard {
                        self.piece_heard(heard);
                    }
                },
                heard = async {
                    match self.barge.as_mut() {
                        Some(barge) => barge.heard.recv().await,
                        None => std::future::pending().await,
                    }
                } => {
                    if let Some(heard) = heard {
                        self.barge_heard(heard);
                    }
                },
                event = async {
                    match self.turn.as_mut() {
                        Some(turn) if turn.ended.is_none() => turn.events.next().await,
                        _ => std::future::pending().await,
                    }
                } => self.event(event),
                spoken = self.told.recv() => {
                    if let Some(spoken) = spoken {
                        self.spoken(spoken);
                    }
                },
                outside = self.notices.next() => {
                    if let Some(outside) = outside {
                        self.outside(outside.what);
                    }
                },
                _ = tick.tick() => self.tick(),
            }
        }
    }

    fn voice_settings(&self) -> TalkVoice {
        self.deps.settings.get(&TALK_VOICE, None).unwrap_or_default()
    }

    fn quiet_hours(&self) -> QuietHours {
        self.deps.settings.get(&QUIET_HOURS, None).unwrap_or_default()
    }

    /// Whether the computer may speak over itself being heard: the check passed, the system cancels the echo.
    fn barges(&self) -> bool {
        let settings = self.voice_settings();
        settings.barge_in && settings.echo.is_some_and(|echo| echo.ok) && self.deps.port.cancels_echo()
    }

    fn id(&mut self) -> u64 {
        self.next += 1;
        self.next
    }

    // ── what is seen ──────────────────────────────────────────────────────────────────────────────────────────────

    fn set(&mut self, state: State) {
        self.state = state;
        self.follow_capture();
        self.show(None);
    }

    fn note(&mut self, note: String) {
        self.view.send_modify(|view| view.note = Some(note));
    }

    /// The view and the live channel's `voice` (the level only while listening).
    fn show(&mut self, level: Option<f32>) {
        let why = self.ear.as_ref().and_then(|ear| match ear.opened {
            Opened::Window => Some(match (ear.over.is_some(), ear.after) {
                (true, _) => "barge",
                (_, Some(After::Work)) => "work",
                (_, Some(After::Notice)) => "ready",
                _ => "question",
            }),
            _ => None,
        });
        let wait = self.window_until.map(|until| until.saturating_duration_since(Instant::now()).as_secs());
        let request = self.turn.as_ref().and_then(|turn| turn.request.clone());
        let parked = self.parked.as_ref().map(|parked| parked.text.clone());
        let state = self.state;
        self.view.send_modify(|view| {
            view.state = state;
            view.why = why.map(str::to_string);
            view.wait = wait;
            view.request = request.clone();
            view.parked = parked.clone();
            if state != State::Quiet && state != State::Speaking {
                view.note = None;
            }
        });
        self.deps.events.emit(GatewayEvent::VoiceChannel {
            device: DEVICE.to_string(),
            state: state.as_str().to_string(),
            level,
            why: why.map(str::to_string),
            wait,
            request,
            parked,
        });
    }

    // ── the capture ───────────────────────────────────────────────────────────────────────────────────────────────

    /// The capture as the state wants it: with voice processing while we listen, speak (to be cut) or check; plain
    /// all the time while «Слушать слово» is on; off otherwise.
    fn follow_capture(&mut self) {
        let word = self.voice_settings().word && !self.deps.port.mic_busy();
        self.word_on = word;
        let wanted = match self.state {
            State::Listening | State::Window | State::Recognizing if self.ear.is_some() => Capture::Voice,
            State::Speaking if self.barge.is_some() || self.check.is_some() => Capture::Voice,
            _ if word => Capture::Plain,
            _ => Capture::Off,
        };
        if wanted == self.capture {
            return;
        }
        if self.capture != Capture::Off {
            self.deps.port.stop_capture();
            self.frames = None;
        }
        self.capture = Capture::Off;
        if wanted == Capture::Off {
            return;
        }
        match self.deps.port.start_capture(wanted == Capture::Voice) {
            Ok(frames) => {
                self.frames = Some(frames);
                self.capture = wanted;
            }
            Err(error) => tracing::warn!(target: "talk", "the computer's microphone did not open: {error}"),
        }
    }

    fn frame(&mut self, frame: Vec<i16>) {
        if self.word_on {
            let _ = self.sound.send(Arc::new(frame.clone()));
        }
        if let Some(check) = self.check.as_mut() {
            check.sound.extend_from_slice(&frame);
            return;
        }
        if self.ear.is_some() {
            return self.ear_frame(&frame);
        }
        if self.barge.is_some() {
            self.barge_frame(&frame);
        }
    }

    // ── listening ─────────────────────────────────────────────────────────────────────────────────────────────────

    /// The microphone opens for a thought.
    fn listen(&mut self, opened: Opened, after: Option<After>, answer_to: Option<String>, over: Option<String>) {
        if let Some(refused) = self.cannot_listen() {
            self.say_own(refused);
            return;
        }
        let window = opened == Opened::Window;
        let sounds = self.voice_settings().sounds;
        let tone = if sounds { tones::listen() } else { Vec::new() };
        let tone_ms = tones::ms(&tone);
        if !tone.is_empty() {
            mouth::sound(&self.deps.port, tone);
        }
        let goes_on = if window { None } else { self.parked.take().map(|parked| parked.text) };
        let (pieces, heard) = recognizer(self.deps.hear.clone());
        // A press, a word: a question begins now. A window: only once someone speaks in it.
        let trip = if window { None } else { self.trip_begins() };
        self.ear = Some(Ear {
            endpoint: Endpoint::new(tone_ms, if window { WINDOW_MS } else { endpoint::START_WAIT_MS }),
            opened,
            after,
            answer_to,
            over,
            word: None,
            texts: Vec::new(),
            pieces,
            heard,
            ending: None,
            by_cloud: false,
            ms: 0,
            goes_on,
            sound_at: Instant::now(),
            heard_any: false,
            trip,
        });
        self.window_until = window.then(|| Instant::now() + Duration::from_millis(WINDOW_MS));
        self.set(if window { State::Window } else { State::Listening });
    }

    /// Why the microphone cannot open now, in words; None — it can.
    fn cannot_listen(&self) -> Option<String> {
        if self.deps.port.access() == Access::Denied {
            return Some(crate::tr!(
                "Нет доступа к микрофону. Разрешите его DuoWave: «Системные настройки» → «Конфиденциальность и безопасность» → «Микрофон»."
            ));
        }
        if !self.deps.hear.ready() {
            return Some(crate::tr!("Слушать некому: «Слух» не настроен. Его выбирают в «Настройки» → «Микрофон»."));
        }
        None
    }

    fn ear_frame(&mut self, frame: &[i16]) {
        let Some(ear) = self.ear.as_mut() else { return };
        let late = !ear.heard_any && ear.sound_at.elapsed() >= LATE_SOUND;
        ear.sound_at = Instant::now();
        ear.heard_any = true;
        if late {
            // The sound came only now (macOS's question answered): as if the key were pressed now.
            tracing::info!(target: "talk", "the computer's microphone gives sound now: «слушаю» again");
            if self.voice_settings().sounds {
                mouth::sound(&self.deps.port, tones::listen());
            }
        }
        let Some(ear) = self.ear.as_mut() else { return };
        let steps = ear.endpoint.push(frame);
        for step in steps {
            match step {
                Step::Level(level) => {
                    if self.level_at.elapsed() >= LEVEL_EVERY {
                        self.level_at = Instant::now();
                        self.show(Some(level));
                    }
                }
                Step::Speech => {
                    self.window_until = None;
                    if self.ear.as_ref().is_some_and(|ear| ear.trip.is_none()) {
                        let trip = self.trip_begins();
                        if let Some(ear) = self.ear.as_mut() {
                            ear.trip = trip;
                        }
                    }
                    if self.state == State::Window {
                        self.state = State::Listening;
                        self.show(None);
                    }
                }
                Step::Piece { n, pcm } => {
                    if let Some(ear) = self.ear.as_ref() {
                        let _ = ear.pieces.send((n, pcm));
                    }
                }
                Step::Over(ending) => self.end_thought(ending),
            }
        }
    }

    fn piece_heard(&mut self, heard: Heard) {
        let Some(ear) = self.ear.as_mut() else { return };
        ear.ms += heard.ms;
        ear.by_cloud |= heard.cloud;
        if let Some(thought) = &heard.thought {
            if !thought.text.trim().is_empty() {
                ear.texts.push(thought.text.trim().to_string());
            }
        } else if !heard.text.trim().is_empty() {
            ear.texts.push(heard.text.trim().to_string());
        }
        let steps = ear.endpoint.judged(heard.thought.as_ref());
        let finishing = ear.ending;
        let settled = ear.endpoint.settled();
        for step in steps {
            if let Step::Over(ending) = step {
                self.end_thought(ending);
            }
        }
        if let (Some(ending), true) = (finishing, settled) {
            self.complete(ending);
        }
    }

    /// The ear heard the thought's end: once every piece is recognized, it goes.
    fn end_thought(&mut self, ending: Ending) {
        let Some(ear) = self.ear.as_mut() else { return };
        if ear.ending.is_some() {
            return;
        }
        if let Some(Step::Piece { n, pcm }) = ear.endpoint.take_rest() {
            let _ = ear.pieces.send((n, pcm));
        }
        ear.ending = Some(ending);
        let settled = ear.endpoint.settled();
        if ending != Ending::Nobody {
            if let Some(trip) = ear.trip.as_ref() {
                let by = if self.deps.hear.at_home() { journal::Place::Own } else { journal::Place::Cloud };
                trip.step(RequestPhase::Recognize, Some(by), None);
            }
        }
        self.window_until = None;
        if ending != Ending::Nobody {
            self.state = State::Recognizing;
        }
        if settled {
            self.complete(ending);
        } else {
            self.set(State::Recognizing);
        }
    }

    /// The thought is heard whole: parked, dropped, or sent through the door.
    fn complete(&mut self, ending: Ending) {
        let Some(ear) = self.ear.take() else { return };
        self.window_until = None;
        let window = ear.opened == Opened::Window;
        let said = ear.texts.join(" ");
        let said = strip_word(&said, ear.word.as_ref());
        if ending == Ending::Nobody || said.trim().is_empty() {
            self.trip_ends(ear.trip.as_ref(), RequestStatus::Silent);
            if let (true, Some(question)) = (window, ear.answer_to.clone()) {
                self.deps.thinking.tell(News::new(Told::NoAnswer { question }));
            }
            if let Some(goes_on) = ear.goes_on {
                self.parked = Some(Parked { text: goes_on, until: Instant::now() + Duration::from_millis(PARK_MS) });
            }
            self.set(State::Quiet);
            return self.tell_next();
        }
        // «Стоп», «отмена» take a hanging thought away.
        if ear.goes_on.is_some() && sense::home().window(&said, None) == Window::Close {
            self.trip_heard(ear.trip.as_ref(), &said);
            self.trip_ends(ear.trip.as_ref(), RequestStatus::Cancelled);
            self.set(State::Quiet);
            return self.tell_next();
        }
        let text = match &ear.goes_on {
            Some(before) => format!("{before} {said}"),
            None => said.clone(),
        };
        // Left hanging after «и…» or «подожди»: it waits for the next press.
        if ending == Ending::Silence && matches!(ear.endpoint.end(), End::Hanging | End::Wait) && !window {
            // The thought waits for the next press, which begins a trip of its own with the whole of it.
            self.trip_heard(ear.trip.as_ref(), &text);
            self.trip_ends(ear.trip.as_ref(), RequestStatus::Silent);
            self.parked = Some(Parked { text, until: Instant::now() + Duration::from_millis(PARK_MS) });
            self.say_own(crate::tr!("Жду."));
            return;
        }
        // Heard by itself, it begins with a station's word: the station's (К9).
        if window && sense::home().for_us(&text, None, &(self.deps.others)()) == ForUs::Station {
            tracing::info!(target: "talk", "the computer's window let a phrase to a station go");
            self.trip_heard(ear.trip.as_ref(), &text);
            self.trip_ends(ear.trip.as_ref(), RequestStatus::Cancelled);
            if let Some(question) = ear.answer_to.clone() {
                self.deps.thinking.tell(News::new(Told::NoAnswer { question }));
            }
            self.set(State::Quiet);
            return self.tell_next();
        }
        if self.voice_settings().sounds {
            mouth::sound(&self.deps.port, tones::heard());
        }
        self.send(text, ear);
    }

    /// The phrase goes through the door.
    fn send(&mut self, text: String, ear: Ear) {
        let id = self.id();
        let turn = format!("t-{}-{id}", now_ms());
        let person = self.person.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        let (to, opened) = match &ear.word {
            Some(woken) => {
                (To { agent: woken.agent.clone(), by: By::Word, name: Some(woken.word.clone()) }, Opened::Word)
            }
            None => (
                To {
                    agent: None,
                    by: if ear.opened == Opened::Window { By::Window } else { By::Button },
                    name: Some(APP.to_string()),
                },
                ear.opened,
            ),
        };
        let phrase = Phrase {
            v: door::V,
            turn: turn.clone(),
            text,
            lang: None,
            alts: Vec::new(),
            who: Who {
                person: person.as_ref().map(|(id, _)| id.clone()),
                name: person.as_ref().map(|(_, name)| name.clone()),
                sure: if person.is_some() { door::Sure::Guess } else { door::Sure::Unknown },
            },
            to,
            place: Place { device: DEVICE.to_string(), room: None, kind: Kind::Computer, screen: true, speaker: true },
            hands: Vec::new(),
            opened,
            context: Context { answer_to: ear.answer_to, after: ear.after, over: ear.over, fresh: false },
            budget: Budget { voice: true, first_word_ms: Some(1_500), limit_ms: Some(TURN_SILENT.as_millis() as u64) },
            heard: Some(door::Heard {
                by: if ear.by_cloud { "cloud" } else { "home" }.to_string(),
                ms: Some(ear.ms),
                model: None,
            }),
            at: Some(now_ms()),
        };
        tracing::info!(target: "talk", "the computer's thought goes to Thinking: turn {turn}, opened {opened:?}");
        // What was asked, and Thinking here goes on in the ear's trip (one request per phrase).
        let trip = ear.trip.clone();
        if let Some(trip) = trip.as_ref() {
            trip.heard(&phrase.text);
            self.deps.trips.offer(trip.clone());
        }
        let events = self.deps.thinking.phrase(phrase);
        self.turn = Some(Turn {
            id: turn,
            events,
            ended: None,
            asked: None,
            work: false,
            request: None,
            said: Vec::new(),
            last_event: Instant::now(),
            trip,
        });
        self.set(State::Thinking);
    }

    /// A line of the channel's own («Жду.», «Слушать некому…»): said, and the strip shows it.
    fn say_own(&mut self, line: String) {
        self.note(line.clone());
        if self.deps.speak.ready() {
            let id = self.id();
            self.mouth.ask(Say::Say { id, text: line });
            self.set(State::Speaking);
        } else {
            self.set(State::Quiet);
        }
    }

    // ── the trip (`Trips`) ────────────────────────────────────────────────────────────────────────────────────────

    /// A question begins on the ear: its trip, the channel's, listening now.
    fn trip_begins(&self) -> Option<Arc<RequestHandle>> {
        let trip = self.deps.trips.begin()?;
        trip.claim(self.owner);
        trip.step(RequestPhase::Listen, Some(journal::Place::Own), None);
        Some(trip)
    }

    /// What was heard, on a trip that ends without Thinking.
    fn trip_heard(&self, trip: Option<&Arc<RequestHandle>>, text: &str) {
        if let Some(trip) = trip {
            trip.heard(text);
        }
    }

    fn trip_ends(&self, trip: Option<&Arc<RequestHandle>>, status: RequestStatus) {
        if let Some(trip) = trip {
            trip.finish(status, None, Some(self.owner));
        }
    }

    /// The ear goes without its thought (a stop, a press over a window, a word): its trip ends cancelled.
    fn drop_ear(&mut self) {
        if let Some(ear) = self.ear.take() {
            self.trip_ends(ear.trip.as_ref(), RequestStatus::Cancelled);
        }
    }

    // ── the answer ────────────────────────────────────────────────────────────────────────────────────────────────

    fn event(&mut self, event: Option<door::Event>) {
        let Some(turn) = self.turn.as_mut() else { return };
        turn.last_event = Instant::now();
        let Some(event) = event else {
            // The answer ended without its end: as if it ended.
            turn.ended = Some(Outcome::Answered);
            return self.maybe_after_turn();
        };
        match event.what {
            What::Begun { request, by } => {
                if let Some(trip) = turn.trip.as_ref().filter(|trip| trip.id() != request) {
                    // Thinking thinks on another gateway (the hub of a Турбо): the trip names its request there and
                    // keeps the brain's step itself.
                    trip.thought_in(&request);
                    let by = match by.as_str() {
                        "cloud" => journal::Place::Cloud,
                        "yandex" => journal::Place::Yandex,
                        _ => journal::Place::Own,
                    };
                    trip.step(RequestPhase::Think, Some(by), None);
                }
                turn.request = Some(request);
                self.show(None);
            }
            What::Say { text } => {
                let id = self.next + 1;
                self.next = id;
                if let Some(turn) = self.turn.as_mut() {
                    turn.said.push((id, text.clone(), None));
                }
                self.mouth.ask(Say::Say { id, text: speakable(&text, &(self.deps.others)()) });
            }
            What::Ask { question, .. } => turn.asked = Some(question),
            What::Work { .. } => turn.work = true,
            What::End { outcome, .. } => {
                turn.ended = Some(outcome);
                self.maybe_after_turn();
            }
            // A skill going on is the station's (its rules read it); the computer's talk keeps its own window.
            What::Wait { .. } | What::Hands { .. } | What::Handoff { .. } | What::Follow { .. } | What::Unknown => {}
        }
    }

    fn spoken(&mut self, spoken: Spoken) {
        match spoken {
            Spoken::Started { .. } => {
                if let Some(trip) = self.turn.as_ref().and_then(|turn| turn.trip.as_ref()) {
                    trip.step(RequestPhase::Speak, Some(journal::Place::Own), None);
                }
                if self.state != State::Speaking {
                    if self.barges() && self.check.is_none() {
                        self.barge = Some(barge(self.deps.hear.clone()));
                    }
                    self.set(State::Speaking);
                }
            }
            Spoken::Ended { id, full } => {
                if let Some(turn) = self.turn.as_mut() {
                    if let Some(said) = turn.said.iter_mut().find(|(said, _, _)| *said == id) {
                        said.2 = Some(full);
                    }
                }
                if let Some((notice, telling)) = self.telling.take() {
                    if telling == id {
                        return self.told_notice(notice, full);
                    }
                    self.telling = Some((notice, telling));
                }
                if self.check.as_ref().is_some_and(|check| check.id == id) {
                    if let Some(check) = self.check.as_mut() {
                        check.said_at = Some(Instant::now());
                    }
                }
            }
            Spoken::Idle => {
                self.barge = None;
                if self.check.is_some() {
                    return;
                }
                if self.turn.is_some() {
                    return self.maybe_after_turn();
                }
                if self.state == State::Speaking {
                    self.set(State::Quiet);
                    self.tell_next();
                }
            }
        }
    }

    /// Thinking's part is over and everything said: the window opens, or the channel goes quiet.
    fn maybe_after_turn(&mut self) {
        let Some(turn) = self.turn.as_ref() else { return };
        let Some(outcome) = turn.ended else { return };
        // Still saying it: the mouth's `idle` comes back here.
        if turn.said.iter().any(|(_, _, done)| done.is_none()) {
            return;
        }
        let Some(turn) = self.turn.take() else { return };
        self.barge = None;
        let full = turn.said.iter().filter(|(_, _, done)| *done == Some(true)).count();
        tracing::info!(target: "talk", "the computer's turn {} is over: {outcome:?}, {full} of {} phrases said to the end",
            turn.id, turn.said.len());
        if let Some(trip) = turn.trip.as_ref() {
            let said: Vec<&str> = turn.said.iter().map(|(_, text, _)| text.as_str()).collect();
            trip.answered(&said.join(" "));
            let status = match outcome {
                Outcome::Failed => RequestStatus::Failed,
                Outcome::Dropped if turn.said.is_empty() => RequestStatus::Silent,
                _ => RequestStatus::Answered,
            };
            self.trip_ends(Some(trip), status);
        }
        // An answer heard to its end: Thinking's «after the speech» starts now (`said`).
        if !turn.said.is_empty() && turn.said.iter().all(|(_, _, done)| *done == Some(true)) {
            self.deps.thinking.tell(News::new(Told::Said { turn: turn.id.clone(), at: Some(now_ms()) }));
        }
        match outcome {
            Outcome::Asked => self.listen(Opened::Window, Some(After::Ask), turn.asked, None),
            Outcome::Answered if turn.work => self.listen(Opened::Window, Some(After::Work), None, None),
            _ => {
                self.set(State::Quiet);
                self.tell_next();
            }
        }
    }

    /// The turn going on is stopped: what was heard of it goes to Thinking.
    fn stop_turn(&mut self, why: StopWhy) {
        let Some(turn) = self.turn.take() else { return };
        tracing::info!(target: "talk", "the computer's turn {} is stopped: {why:?}", turn.id);
        let said: Vec<&str> =
            turn.said.iter().filter(|(_, _, done)| done.is_some()).map(|(_, text, _)| text.as_str()).collect();
        if let Some(trip) = turn.trip.as_ref() {
            trip.answered(&said.join(" "));
        }
        self.trip_ends(turn.trip.as_ref(), RequestStatus::Cancelled);
        let chars: usize =
            turn.said.iter().filter(|(_, _, done)| *done == Some(true)).map(|(_, text, _)| text.chars().count()).sum();
        self.deps.thinking.tell(News::new(Told::Stop {
            turn: turn.id,
            why,
            heard: Some(door::Played { chars: Some(chars as u64), ms: None }),
        }));
    }

    // ── what is asked ─────────────────────────────────────────────────────────────────────────────────────────────

    fn command(&mut self, command: Command) {
        match command {
            Command::Press { held } => self.key_press(held),
            Command::Release { held_ms } => self.key_release(held_ms),
            Command::Stop => {
                self.mouth.ask(Say::Stop);
                self.stop_turn(StopWhy::Word);
                self.drop_ear();
                self.barge = None;
                self.parked = None;
                self.window_until = None;
                if let Some((notice, _)) = self.telling.take() {
                    self.told_notice(notice, false);
                }
                self.set(State::Quiet);
            }
            Command::Word(woken) => self.woken(woken),
            Command::Check(reply) => self.start_check(reply),
            Command::Shutdown => {}
        }
    }

    /// The key went down (`held`) or «Сказать» was pressed, as Handy's coordinator takes it (the owner, 27.11): a
    /// thought the key began is ended by the key, never by a pause.
    fn key_press(&mut self, held: bool) {
        let keyed = self.ear.as_ref().map(|ear| ear.endpoint.keyed_by());
        tracing::info!(target: "talk", "the computer's {} down, in state {}, the thought {keyed:?}",
            if held { "key" } else { "«Сказать»" }, self.state.as_str());
        if self.state == State::Listening {
            match self.ear.as_ref().map(|ear| ear.endpoint.keyed_by()) {
                // A thought locked on by a tap (or in «два нажатия»): this press ends it.
                Some(Keyed::Locked) => return self.end_by_key(),
                // The key is down already: a repeat of the same press.
                Some(Keyed::Held) => return,
                // A tapped thought that ends by itself too (`auto_end`): the press ends it now.
                Some(Keyed::Free) if self.ear.as_ref().is_some_and(|ear| ear.opened == Opened::Button) => {
                    return self.end_by_key();
                }
                Some(Keyed::Free) | None => {}
            }
        }
        self.press();
        let voice = self.voice_settings();
        let keyed = if held && voice.activation != KeyActivation::Toggle { Keyed::Held } else { untouched(&voice) };
        if self.state == State::Listening {
            if let Some(ear) = self.ear.as_mut().filter(|ear| ear.opened == Opened::Button) {
                if ear.endpoint.keyed_by() == Keyed::Free && ear.ending.is_none() {
                    ear.endpoint.keyed(keyed);
                }
            }
        }
    }

    /// The key came up after `held_ms`.
    fn key_release(&mut self, held_ms: u64) {
        let mode = self.voice_settings().activation;
        let keyed = self.ear.as_ref().map(|ear| ear.endpoint.keyed_by());
        tracing::info!(target: "talk", "the computer's key up after {held_ms} ms ({mode:?}), the thought {keyed:?}");
        if self.ear.as_ref().is_some_and(|ear| ear.endpoint.keyed_by() == Keyed::Held && !ear.heard_any) {
            // No sound yet: macOS asks about the microphone, and the hand left the key for «Разрешить» (27.11, the
            // owner). The thought waits as after a tap: once the sound comes, it goes on as if the key were pressed.
            let keyed = untouched(&self.voice_settings());
            if let Some(ear) = self.ear.as_mut() {
                ear.endpoint.keyed(keyed);
            }
            tracing::info!(target: "talk", "the key let go before any sound came (macOS asking?): the thought waits");
            return;
        }
        if self.ear.as_ref().is_some_and(|ear| ear.endpoint.keyed_by() == Keyed::Held) {
            match mode {
                KeyActivation::PushToTalk => return self.end_by_key(),
                KeyActivation::HoldOrToggle if held_ms >= HOLD_MS => return self.end_by_key(),
                // A tap, or «два нажатия»: the thought goes on until the next press (or its end, `auto_end`).
                KeyActivation::HoldOrToggle | KeyActivation::Toggle => {
                    let keyed = untouched(&self.voice_settings());
                    if let Some(ear) = self.ear.as_mut() {
                        ear.endpoint.keyed(keyed);
                    }
                    return;
                }
            }
        }
        self.follow_capture();
    }

    /// The key ends the thought: what was said goes; nothing said — the microphone closes.
    /// (The thought the key does not hold: see `untouched`.)
    fn end_by_key(&mut self) {
        let Some(ear) = self.ear.as_mut() else { return };
        let spoke = ear.endpoint.spoke();
        ear.endpoint.keyed(Keyed::Free);
        self.end_thought(if spoke { Ending::Silence } else { Ending::Nobody });
    }

    fn press(&mut self) {
        tracing::info!(target: "talk", "the computer's key, in state {}", self.state.as_str());
        match self.state {
            State::Listening if self.ear.as_ref().is_some_and(|ear| ear.endpoint.spoke()) => {
                self.end_thought(Ending::Silence);
            }
            State::Listening | State::Recognizing => {}
            State::Speaking | State::Thinking => {
                self.mouth.ask(Say::Stop);
                self.stop_turn(StopWhy::Word);
                if let Some((notice, _)) = self.telling.take() {
                    self.told_notice(notice, false);
                }
                self.barge = None;
                self.listen(Opened::Button, None, None, None);
            }
            State::Window => {
                self.drop_ear();
                self.listen(Opened::Button, None, None, None);
            }
            State::Quiet => self.listen(Opened::Button, None, None, None),
        }
    }

    /// An app's word (К10): as the key, the sound before the word the thought's beginning.
    fn woken(&mut self, woken: Woken) {
        if matches!(self.state, State::Listening | State::Recognizing) {
            return;
        }
        if matches!(self.state, State::Speaking | State::Thinking) {
            self.mouth.ask(Say::Stop);
            self.stop_turn(StopWhy::New);
            self.barge = None;
        }
        self.drop_ear();
        let before = woken.before.clone();
        self.listen(Opened::Word, None, None, None);
        if let Some(ear) = self.ear.as_mut() {
            ear.word = Some(woken);
        }
        if !before.is_empty() {
            self.ear_frame(&before);
        }
    }

    // ── «Готово» and the other notices ────────────────────────────────────────────────────────────────────────────

    fn outside(&mut self, what: Unasked) {
        match what {
            Unasked::Notice(notice) => {
                let elsewhere =
                    notice.to.as_ref().and_then(|to| to.device.as_deref()).is_some_and(|device| device != DEVICE);
                if elsewhere {
                    return;
                }
                self.waiting.retain(|(waits, _)| waits.notice != notice.notice);
                self.waiting.push_back((notice, Instant::now()));
                self.tell_next();
            }
            Unasked::Withdraw { notice } => self.waiting.retain(|(waits, _)| waits.notice != notice),
            Unasked::Hands { .. } | Unasked::Unknown => {}
        }
    }

    /// The first notice waiting is said now, if the computer is free; at night or in a call, a notification.
    fn tell_next(&mut self) {
        if self.state != State::Quiet || self.turn.is_some() || self.ear.is_some() || self.check.is_some() {
            return;
        }
        let Some((notice, since)) = self.waiting.pop_front() else { return };
        if notice.until.is_some_and(|until| until < now_ms()) {
            self.delivered(&notice, Delivery::Dropped);
            return self.tell_next();
        }
        let minute = {
            use chrono::Timelike;
            let now = chrono::Local::now();
            (now.hour() * 60 + now.minute()) as u16
        };
        let night = self.quiet_hours().holds(minute);
        let call = self.deps.port.mic_busy();
        let voiceless = !self.deps.speak.ready();
        if night || call || voiceless {
            self.deps.port.notify("DuoWave", &notice.text);
            self.delivered(&notice, Delivery::Shown);
            return self.tell_next();
        }
        let _ = since;
        let id = self.id();
        self.mouth.ask(Say::Say { id, text: speakable(&notice.text, &(self.deps.others)()) });
        self.telling = Some((notice, id));
        self.set(State::Speaking);
    }

    fn told_notice(&mut self, notice: Notice, full: bool) {
        self.delivered(&notice, if full { Delivery::Said } else { Delivery::Cut });
        match (full, notice.question.clone()) {
            (true, Some(question)) => self.listen(Opened::Window, Some(After::Notice), Some(question), None),
            _ => {
                if self.state == State::Speaking && self.turn.is_none() {
                    self.set(State::Quiet);
                }
                self.tell_next();
            }
        }
    }

    fn delivered(&self, notice: &Notice, outcome: Delivery) {
        self.deps.thinking.tell(News::new(Told::Delivered {
            notice: Some(notice.notice.clone()),
            outcome,
            device: DEVICE.to_string(),
            heard: None,
        }));
    }

    // ── speaking over (К7) ────────────────────────────────────────────────────────────────────────────────────────

    fn barge_frame(&mut self, frame: &[i16]) {
        let Some(barge) = self.barge.as_mut() else { return };
        for step in barge.endpoint.push(frame) {
            match step {
                Step::Speech => barge.heard_from = Some(Instant::now()),
                Step::Piece { n, pcm } => {
                    let _ = barge.pieces.send((n, pcm));
                }
                Step::Level(_) | Step::Over(_) => {}
            }
        }
        let long = barge.heard_from.is_some_and(|from| from.elapsed() >= Duration::from_millis(BARGE_SPEECH_MS));
        if long && barge.paused.is_none() {
            barge.paused = Some(Instant::now());
            self.mouth.ask(Say::Pause);
            if let Some(Step::Piece { n, pcm }) = barge.endpoint.take_rest() {
                let _ = barge.pieces.send((n, pcm));
            }
        }
    }

    fn barge_heard(&mut self, heard: Heard) {
        let text = heard.thought.as_ref().map_or(heard.text.clone(), |thought| thought.text.clone());
        let ours: String = self
            .turn
            .as_ref()
            .map(|turn| turn.said.iter().map(|(_, text, _)| text.as_str()).collect::<Vec<_>>().join(" "))
            .unwrap_or_default();
        let echo = text.trim().is_empty() || is_echo(&text, &ours);
        if echo {
            if self.barge.as_ref().is_some_and(|barge| barge.paused.is_some()) {
                self.mouth.ask(Say::Resume);
            }
            self.barge = Some(barge(self.deps.hear.clone()));
            return;
        }
        // Spoken over: the answer stops, the phrase goes to the window.
        let over = self.turn.as_ref().map(|turn| turn.id.clone());
        self.mouth.ask(Say::Stop);
        self.stop_turn(StopWhy::Over);
        self.barge = None;
        let (pieces, receiver) = recognizer(self.deps.hear.clone());
        let mut ear = Ear {
            endpoint: Endpoint::new(0, WINDOW_MS),
            opened: Opened::Window,
            after: None,
            answer_to: None,
            over,
            word: None,
            texts: vec![text.trim().to_string()],
            pieces,
            heard: receiver,
            ending: Some(Ending::Silence),
            by_cloud: heard.cloud,
            ms: heard.ms,
            goes_on: None,
            sound_at: Instant::now(),
            heard_any: true,
            // Said over the answer: a question of its own.
            trip: self.trip_begins(),
        };
        let _ = ear.endpoint.take_rest();
        self.ear = Some(ear);
        self.complete(Ending::Silence);
    }

    // ── the echo check (К7) ───────────────────────────────────────────────────────────────────────────────────────

    fn start_check(&mut self, reply: oneshot::Sender<Result<Checked, String>>) {
        if self.state != State::Quiet || self.turn.is_some() {
            let _ = reply.send(Err(crate::tr!("Компьютер сейчас занят разговором: проверьте чуть позже.")));
            return;
        }
        if !self.deps.speak.ready() {
            let _ = reply.send(Err(crate::tr!("У компьютера нет своего голоса: выберите его в «Голос».")));
            return;
        }
        let phrase = crate::tr!("Проверяю, слышу ли я себя. Раз, два, три.");
        let id = self.id();
        self.check = Some(Check { id, sound: Vec::new(), phrase: phrase.clone(), said_at: None, reply });
        self.mouth.ask(Say::Say { id, text: phrase });
        self.set(State::Speaking);
    }

    /// The check's phrase was said and its tail heard: what the microphone caught is recognized.
    fn finish_check(&mut self) {
        let Some(check) = self.check.take() else { return };
        self.set(State::Quiet);
        let caught_ms = check.sound.len() as u64 * 1000 / u64::from(RATE);
        if caught_ms < CHECK_SOUND_LEAST_MS {
            tracing::warn!(target: "talk", "the echo check caught {caught_ms} ms of sound: no check");
            let _ = check.reply.send(Err(crate::tr!(
                "Микрофон не дал звука во время проверки: проверьте доступ к микрофону и повторите."
            )));
            return;
        }
        let (hear, settings) = (self.deps.hear.clone(), self.deps.settings.clone());
        tokio::spawn(async move {
            let heard = match hear.recognize(check.sound).await {
                Ok(text) => text,
                Err(error) => {
                    let _ = check.reply.send(Err(crate::tr!("«Слух» не ответил: {error}", error = error)));
                    return;
                }
            };
            // 27.11, before the first check by hand: nothing recognized was taken as our echo, so a microphone that
            // cancels the echo fully failed the check. Nothing heard is the echo cancelled; its own words — it hears
            // itself. The log counts the words only: the room may have been heard too.
            let ok = !says_ours(&heard, &check.phrase);
            tracing::info!(
                target: "talk",
                "the echo check: {caught_ms} ms caught, {} words heard: {}",
                sense::words::household().split(&heard).len(),
                if ok { "the echo is cancelled" } else { "the computer hears itself" }
            );
            let mut voice = settings.get(&TALK_VOICE, None).unwrap_or_default();
            voice.echo = Some(EchoCheck { ok, at: now_ms() });
            let _ = settings.set(&TALK_VOICE, &voice, None);
            let _ = check.reply.send(Ok(Checked { ok, heard }));
        });
    }

    // ── the clock ─────────────────────────────────────────────────────────────────────────────────────────────────

    fn tick(&mut self) {
        if self.ear.as_ref().is_some_and(|ear| {
            ear.ending.is_none() && ear.sound_at.elapsed() > if ear.heard_any { NO_SOUND } else { NO_SOUND_YET }
        }) {
            tracing::warn!(target: "talk", "the computer's microphone gave no sound for {NO_SOUND:?}: it stops listening");
            self.end_thought(Ending::Nobody);
        }
        if self.parked.as_ref().is_some_and(|parked| parked.until <= Instant::now()) {
            self.parked = None;
            self.show(None);
        }
        if self.window_until.is_some() {
            self.show(None);
        }
        if let Some(check) = self.check.as_ref() {
            if check.said_at.is_some_and(|at| at.elapsed() >= Duration::from_millis(CHECK_TAIL_MS)) {
                self.finish_check();
            }
        }
        if let Some(barge) = self.barge.as_ref() {
            if barge.paused.is_some_and(|at| at.elapsed() >= BARGE_RESUME) {
                // Nothing came of it in time: the answer goes on.
                self.mouth.ask(Say::Resume);
                self.barge = Some(barge_after(self.deps.hear.clone()));
            }
        }
        if let Some(turn) = self.turn.as_ref() {
            if turn.ended.is_none() && turn.last_event.elapsed() > TURN_SILENT {
                tracing::warn!(target: "talk", "Thinking said nothing for a minute on {DEVICE}: the turn is given up");
                self.mouth.ask(Say::Stop);
                self.stop_turn(StopWhy::Timeout);
                self.say_own(crate::tr!("Не получилось ответить. Подробности в пульте."));
            }
        }
        // A notice that waited too long: the next talk reminds of it.
        while self.waiting.front().is_some_and(|(_, since)| since.elapsed() > NOTICE_WAITS) {
            if let Some((notice, _)) = self.waiting.pop_front() {
                self.delivered(&notice, Delivery::Later);
            }
        }
        if self.state == State::Quiet {
            self.follow_capture();
            self.tell_next();
        }
    }
}

/// A thought's recognizer: pieces in, in order, each recognized once and judged by «Чутьё».
fn recognizer(hear: Arc<dyn Hear>) -> (mpsc::UnboundedSender<(u32, Vec<i16>)>, mpsc::UnboundedReceiver<Heard>) {
    let (pieces, mut incoming) = mpsc::unbounded_channel::<(u32, Vec<i16>)>();
    let (heard_by, heard) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Some((n, pcm)) = incoming.recv().await {
            let started = Instant::now();
            let text = match hear.recognize(pcm).await {
                Ok(text) => text,
                Err(error) => {
                    tracing::warn!(target: "talk", "«Слух» did not hear a piece of the computer's thought: {error}");
                    String::new()
                }
            };
            let thought = (!text.trim().is_empty()).then(|| sense::home().finished(&text, None, n == 1));
            let heard = Heard { text, thought, cloud: !hear.at_home(), ms: started.elapsed().as_millis() as u64 };
            if heard_by.send(heard).is_err() {
                return;
            }
        }
    });
    (pieces, heard)
}

/// The answer listened to while it plays: the first second is deaf.
fn barge(hear: Arc<dyn Hear>) -> Barge {
    barge_from(hear, BARGE_DEAF_MS)
}

/// Listened to again after a pause that came to nothing: no deaf second.
fn barge_after(hear: Arc<dyn Hear>) -> Barge {
    barge_from(hear, 0)
}

fn barge_from(hear: Arc<dyn Hear>, deaf_ms: u64) -> Barge {
    let (pieces, heard) = recognizer(hear);
    Barge { endpoint: Endpoint::new(deaf_ms, u64::MAX / 4), heard_from: None, paused: None, pieces, heard }
}

/// What was heard over our speech is our own echo: no words at all, or mostly our phrase's.
fn is_echo(heard: &str, ours: &str) -> bool {
    sense::words::household().split(heard).is_empty() || says_ours(heard, ours)
}

/// Whether the words heard are mostly our own phrase's. Nothing heard is not our phrase.
fn says_ours(heard: &str, ours: &str) -> bool {
    let words = sense::words::household();
    let heard = words.split(heard);
    if heard.is_empty() {
        return false;
    }
    let ours = words.split(ours);
    let shared = heard.iter().filter(|word| ours.contains(word)).count();
    shared * 2 >= heard.len()
}

/// The thought's text without the word that woke it (К10).
fn strip_word(text: &str, woken: Option<&Woken>) -> String {
    let Some(woken) = woken else { return text.trim().to_string() };
    let words = sense::words::household();
    let tokens: Vec<&str> = text.split_whitespace().collect();
    for form in std::iter::once(&woken.word).chain(woken.heard.iter()) {
        let form = words.split(form);
        if form.is_empty() {
            continue;
        }
        let head = words.split(&tokens.iter().take(form.len()).copied().collect::<Vec<_>>().join(" "));
        if head == form {
            return tokens[form.len()..].join(" ").trim_start_matches([',', '.', '!', ' ']).to_string();
        }
    }
    text.trim().to_string()
}

/// What the computer says aloud: without a station's word, so that it does not wake the station beside it (Н4). The
/// word becomes «колонка», its punctuation kept around it; the strip shows the text as it was.
fn speakable(text: &str, others: &[String]) -> String {
    let words = sense::words::household();
    let theirs: Vec<Vec<String>> = others.iter().map(|word| words.split(word)).filter(|word| word.len() == 1).collect();
    let mut said: Vec<String> = Vec::new();
    for token in text.split_whitespace() {
        let normal = words.split(token);
        let station = normal.len() == 1 && theirs.iter().any(|word| stem_of(&word[0]) == stem_of(&normal[0]));
        if !station {
            said.push(token.to_string());
            continue;
        }
        let first = token.find(char::is_alphanumeric).unwrap_or(0);
        let last = token
            .rfind(char::is_alphanumeric)
            .map_or(token.len(), |at| at + token[at..].chars().next().map_or(1, char::len_utf8));
        said.push(format!("{}{}{}", &token[..first], crate::tr!("колонка"), &token[last..]));
    }
    said.join(" ")
}

/// A word without its ending, so that «Алисе» and «Алису» are «Алиса» too.
fn stem_of(word: &str) -> String {
    let count = word.chars().count();
    if count < 4 {
        return word.to_string();
    }
    word.chars().take(count - 1).collect()
}

#[cfg(test)]
mod tests;
