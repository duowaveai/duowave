//! One station connection on /uni.ws. Owns the protocol state of that connection: which wire format it speaks, its
//! voice requests by stream id, and the order of everything we send back.
//!
//! The state is `Inner` behind one lock: each synchronous stretch of the TS (the code between two awaits) takes it
//! once and holds it to the end of the stretch, never across an await — so each stretch runs whole, as JavaScript
//! ran it, and the logic that depends on it holds. What the session sends goes through its outbox and its journal,
//! which take no lock of the state's: an answer, a relay or our session with Yandex may send while the state is
//! held, and none of them is called back with its own lock held.
//!
//! The locks, in the one order they are taken (only ever under locks above, never under one below):
//!
//! 1. the session's state (`Inner`);
//! 2. the state of what the session drives: an answer (SpokenAnswer), a relay (YandexRelay), a request's trip, our
//!    session with Yandex (YandexSession), which an answer or a relay may call;
//! 3. a phrase or an exchange of our session with Yandex;
//! 4. the journal (SessionJournal): a leaf, whose methods take nothing else and never hand its guard out.
//!
//! None of 2–4 takes the session's state: their way to the station (`Link`) is the outbox and the journal, and what
//! does need the state (a push from Yandex, a relayed answer's response) is called after they let their own lock
//! go. So no two threads can wait for each other: a sessions poll (`snapshot`) reads the state, our Yandex session's
//! with it, and then the journal on its own, while our Yandex session journals under its own state.
//!
//! A slow answer keeps the station waiting with silence on its stream (spoken_answer). One the station stopped
//! waiting for — past ANSWER_WAIT_MAX_MS, or the station said it gave up (requestStat) — is deferred: it leaves the
//! current-answer slot, so a new request neither stops it nor is spoken over by it. Its words, once complete within
//! DEFERRED_MAX_MS of its start, are said through our push as a reminder's are (`say`), when the station is free:
//! no answer speaking or still playing, nobody talking, no push of ours waiting. «стоп» with nothing else to stop, a
//! cancel of its request, or the station's session closing drop it, journaled as deferredAnswer.

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use axum::extract::ws::WebSocket;
use bytes::Bytes;
use futures::future::{BoxFuture, FutureExt};
use futures::stream::StreamExt;
use serde::Serialize;
use serde_json::{json, Map, Value};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use super::device_tag::device_tag_of;
use super::relay_session::recorded_headers;
use super::relay_trips::yandex_heard;
use super::spoken_answer::{
    describe, ends_with_question, without_markup, AnswerObserver, AnswerOutcome, AnswerOutput, AnswerPart, AnswerParts,
    ResponseHeader, SpokenAnswer, SpokenAnswerOptions, Synthesize,
};
use super::summaries::{summarize_client, summarize_server};
use super::uniproxy_server::{pump, Handshake, Liveness, Outbox, Outgoing, SocketEnd};
use super::voice_request::{AllAudio, RawVoice, UtteranceView, VoiceRequest};
use super::yandex_relay::{
    RelayOutcome, RelayOutput, YandexAnswer, YandexRelay, YandexRelayOptions, NO_EVENTS, RECOGNITION_EVENTS,
};
use crate::access::{HouseholdService, SpeakerHint};
use crate::config::{GatewayConfig, ListenMode, ModelProfile, TtsProvider};
use crate::core::connectors::{ContributedOffer, ContributedTools};
use crate::core::events::{EventBus, GatewayEvent};
use crate::core::identity::Principal;
use crate::core::requests::{
    Owner, Place, RequestHandle, RequestKind, RequestPhase, RequestStart, RequestStarter, RequestStatus,
};
use crate::dialog::generator::generate;
use crate::dialog::llm::{ToolCall, ToolResult};
use crate::dialog::{
    with_error_phrase, AnsweredBy, DialogContext, DialogService, PhraseStream, ToolBox, ToolOutcome, LOOKUP_FILLER,
};
use crate::gateway::mode::ModeService;
use crate::gateway::observability::session_recorder::{
    clock_tag, EventDirection, SessionEventRecord, SessionJournal, SessionRecorder,
};
use crate::gateway::protocol::aapi_codec::AapiCodec;
use crate::gateway::protocol::ids::{new_message_id, new_session_id};
use crate::gateway::protocol::proto_edit::{
    edit_frame, get_bytes, get_string, restream, retext_text_input, voice_to_text_input, FrameEdit,
};
use crate::gateway::protocol::proto_object::{bytes_value, is_js_space};
use crate::gateway::protocol::protocol_types::{
    AliceEvent, AliceEventType, AliceRequest, ClientEvent, ClientInfo, ClientMessage, CloseReason, Directive,
    DirectiveType, EventHeader, PlayerState, ServerMessage, StreamAction, StreamControlEvent, WireFormat,
};
use crate::gateway::yandex::yandex_session::{
    upstream_headers, Exchange, StationCredentials, Synthesis, UpstreamJournal, YandexEvents, YandexSession,
    YandexSessionOptions,
};
use crate::mcp::{McpOffer, McpService, OfferOptions};
use crate::skills::capabilities::{is_capability, CapabilityName, Intent};
use crate::skills::media::recording::FileRecording;
use crate::skills::own::reminders_skill::AudioClip;
use crate::skills::quick_commands::is_stop;
use crate::skills::router::{Continuation, Decision, ModelState, Resolution, RouteInput, RESTRICTED};
use crate::skills::routes::RouteStep;
use crate::skills::skill::{ExecutionContext, Outcome, OwnSkill};
use crate::skills::skill_stats::{Executor, Unsupported};
use crate::skills::tools::{lookup_question, tools_for, ToolsKind, SAFE_AFTER_OUTSIDE};
use crate::skills::{SkillsService, OUR_CALLBACK_PREFIX};
use crate::speech::audio::ogg_opus_joiner::OggOpusJoiner;
use crate::speech::audio::opus_pcm_decoder::SPEECH_SAMPLE_RATE;
use crate::speech::audio::pcm::encode_wav;
use crate::speech::interim_recognition::{InterimOptions, InterimRecognition};
use crate::speech::speech_pipeline::{SpeechEvent, SpeechReport, UtteranceAudio};
use crate::speech::{Recognition, SpeechService, VoiceService};
use crate::utils::time::{now_iso, now_ms};

/// What every station session is built from: the gateway's services.
pub struct SessionDeps {
    pub codec: Arc<AapiCodec>,
    pub config: Arc<GatewayConfig>,
    pub speech: Arc<SpeechService>,
    pub dialog: Arc<DialogService>,
    pub skills: Arc<SkillsService>,
    /// Whose voice answers, read per answer (the panel changes it at runtime), and our engine.
    pub voice: Arc<VoiceService>,
    /// Whether anything may go to Yandex now: «Свой» says no — no session with it, no route step.
    pub modes: Arc<ModeService>,
    /// Who spoke: the household until people are recognised; the router checks their rights.
    pub household: Arc<HouseholdService>,
    /// The station said who it is (synchronizeState): its model and colour, for its record and look.
    pub events: EventBus,
    /// Where requests are followed as they go: the panel's live trip and the journal (src/requests).
    pub requests: Option<Arc<dyn RequestStarter>>,
    /// MCP servers' tools for the model (src/mcp); without it the model has only the station's own.
    pub mcp: Option<Arc<McpService>>,
    /// The tools the connectors give the model (the plugins', src/plugins), by the speaker's rights.
    pub tools: Option<Arc<dyn ContributedTools>>,
    /// What gives the station back to Yandex for the emergency words (rescue.rs): the station's own gateway changes
    /// its files, a computer's goes over SSH. None: the words are answered with where to do it instead.
    pub rescue: Option<Arc<dyn super::rescue::StationRescue>>,
}

/// The station's model and firmware, as the debug API shows them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub firmware_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
}

impl DeviceView {
    pub fn of(info: &ClientInfo) -> DeviceView {
        DeviceView {
            device_model: info.device_model.clone(),
            firmware_version: info.firmware_version.clone(),
            timezone: info.timezone.clone(),
            lang: info.lang.clone(),
            platform: info.platform.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSnapshot {
    pub id: String,
    /// relay: the station talks to Yandex through us and we only journal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<&'static str>,
    pub device_tag: String,
    pub remote_address: String,
    pub connected_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wire: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceView>,
    pub active_voice_requests: usize,
    /// The station's credentials arrived, so answers can be voiced by Yandex.
    pub can_speak: bool,
    /// Our session with Yandex is up: speech, pushes, the Yandex route.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub yandex_connected: Option<bool>,
    pub events: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub journal: Option<String>,
}

/// What `say` came to: an answer's outcome, or nothing spoken at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SayOutcome {
    Answer(AnswerOutcome),
    Failed,
}

impl SayOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            SayOutcome::Answer(outcome) => outcome.as_str(),
            SayOutcome::Failed => "failed",
        }
    }
}

/// The silent check of the speech session (the debug API's /tts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SynthesisCheck {
    pub format: String,
    pub bytes: usize,
    pub audio_ms: u64,
    pub first_audio_ms: u64,
    pub total_ms: u64,
}

/// Audio the station attaches to LogSpotter: the wake word it heard, raw PCM. Kept as a WAV for analysis.
struct SpotterLog {
    stream_id: u32,
    format: Option<String>,
    transcript: Option<String>,
    time: String,
    chunks: Vec<Bytes>,
    bytes: usize,
}

/// 30 s of 16-bit 16 kHz PCM.
const MAX_SPOTTER_LOG_BYTES: usize = 960_000;
/// Recent requests whose frames the Yandex route may still need.
const KEPT_REQUESTS: usize = 8;
/// Message ids Yandex owns (its pushes, the callbacks it answers): the station's acks of them go back.
const KEPT_YANDEX_IDS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameKind {
    VoiceInput,
    TextInput,
}

/// A station request as it came: the Yandex route builds its own request from it.
struct RequestFrame {
    message_id: String,
    kind: FrameKind,
    frame: Bytes,
    /// Voice for Yandex (YANDEX_VOICE): the station's audio as it came, the wake word's end among it.
    audio: Option<Vec<Bytes>>,
    spotter_end_chunk: Option<usize>,
    stream_id: Option<u32>,
}

/// The capability a callback of a Yandex command belongs to, for its follow-ups.
fn callback_capability(name: &str) -> CapabilityName {
    let lower = name.to_lowercase();
    if lower.contains("call") {
        return CapabilityName::Calls;
    }
    if ["music", "audio", "radio", "player"].iter().any(|word| lower.contains(word)) {
        return CapabilityName::Music;
    }
    CapabilityName::Skills
}

fn seconds(ms: f64) -> String {
    format!("{:.2}", ms / 1000.0)
}

/// A number as JavaScript prints it.
fn js_text(value: f64) -> String {
    crate::gateway::protocol::proto_object::js_number(value).to_string()
}

/// A model enum as the TS model spelled it (its serde name).
fn name_of(value: impl Serialize) -> String {
    match serde_json::to_value(value) {
        Ok(Value::String(name)) => name,
        _ => String::new(),
    }
}

/// The first eight characters of an id, as the log writes them.
fn tag8(id: &str) -> String {
    id.chars().take(8).collect()
}

fn single(text: impl Into<String>) -> AnswerParts {
    futures::stream::iter([AnswerPart::Text(text.into())]).boxed()
}

/// Below this a text is synthesized whole: one intonation, one request.
const ONE_PHRASE_CHARS: usize = 140;
/// How long our skill may take to its first words before the response goes out early, with the jingle.
const OWN_EARLY_MS: u64 = 300;
/// The same for a skill that shows its result on the display (the weather): the display command can only go with
/// the response, and the forecast takes ~0,8 s (a city seen before) to ~1,7 s.
const OWN_DISPLAY_EARLY_MS: u64 = 2000;
/// How long the answer to the model's question about a command («На сколько поставить таймер?») stays a command.
const COMMAND_QUESTION_MS: u64 = 30_000;
/// How long a callback for Yandex waits for our session with it while the session opens (the station just
/// connected).
const YANDEX_OPENING_WAIT_MS: u64 = 5000;
/// After the emergency words are said and played, a moment more before the station is given back.
const RESCUE_AFTER_MS: u64 = 3000;
/// After connecting, the station's request to continue its player is its own, from before the break (a button's
/// comes later).
const RECONNECT_CONTINUE_MS: u64 = 15_000;
/// Callbacks of Yandex's player: they report on the music, the dialog goes on beside them.
fn player_callback(name: &str) -> bool {
    name.starts_with("music_thin_client_") || name.starts_with("@@mm_stack_engine_")
}
/// Quiet speech with this many windows of pitch in a row is distant speech, not a whisper (voicing.rs). Live under
/// the radio, a whisper had 2 windows 2 s apart; on 02.11 a whispered «включи кино» had 2 in a row, distant speech
/// 4–8.
const VOICED_SPEECH_RUN: usize = 3;
/// A wake word this quiet was whispered too: the whole request is. Recordings of 01–02.11: whispers −59…−79 dBFS,
/// speech −41…−60. VAD may still hear a whisper after the gain (a whispered «какая погода» at −72 dBFS, answered
/// aloud on 02.11), so it is checked for a voice as quiet speech is.
const WHISPERED_WAKE_DBFS: f64 = -60.0;
/// Our server_action for speech nobody asked for: pushed to the station, which calls back and gets the speech.
static SAY_CALLBACK: LazyLock<String> = LazyLock::new(|| format!("{OUR_CALLBACK_PREFIX}say"));
/// How long a push waits for the station's callback before the speech counts as failed.
const SAY_CALLBACK_MS: u64 = 5000;
/// Said when the admin has not allowed what was asked (the router's RESTRICTED): not «I can't», but «not for you».
const RESTRICTED_PHRASE: &str = "Это мне не разрешено. Попросите взрослого или администратора.";

/// What LogSpotter's format says of raw 16-bit 16 kHz mono PCM, which keeps as a WAV.
const PCM_FORMAT: [&str; 4] = ["audio/x-pcm", "bit=16", "rate=16000", "channels=1"];

/// The station's request to continue its player: a semantic frame whose analytics (field 2) name `player_continue`.
fn is_player_continue(request: &AliceRequest) -> bool {
    let (Some("@@mm_semantic_frame"), Some(raw)) = (request.event.name.as_deref(), &request.event.payload_raw) else {
        return false;
    };
    let analytics = match get_bytes(raw, 2) {
        Ok(analytics) => analytics.unwrap_or_default(),
        Err(_) => return false,
    };
    matches!(get_string(analytics, 3), Ok(Some(purpose)) if purpose == "player_continue")
}

/// A whole text as phrases, so long ones are synthesized in parallel.
fn split_phrases(text: &str) -> Vec<String> {
    if text.encode_utf16().count() <= ONE_PHRASE_CHARS {
        let trimmed = text.trim_matches(is_js_space);
        return if trimmed.is_empty() { Vec::new() } else { vec![trimmed.to_string()] };
    }
    let mut splitter = crate::dialog::PhraseSplitter::new();
    let mut phrases = splitter.push(text);
    phrases.extend(splitter.flush());
    phrases
}

fn phrase_stream(phrases: Vec<String>) -> PhraseStream {
    futures::stream::iter(phrases.into_iter().map(Ok)).boxed()
}

/// `"command" (heard "raw")`: what the recognizer returned and what is left after cleaning.
fn heard(recognition: &Recognition) -> String {
    let raw =
        if recognition.raw != recognition.text { format!(" (heard \"{}\")", recognition.raw) } else { String::new() };
    format!("\"{}\"{raw}", recognition.text)
}

fn describe_recognition(voice: &VoiceRequest) -> String {
    let mut parts = Vec::new();
    if let Some(recognition) = &voice.recognition {
        parts.push(heard(recognition));
    }
    if let Some(all) = &voice.all_audio {
        parts.push(format!("all audio: {}", heard(&all.recognition)));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" {}", parts.join(", "))
    }
}

fn describe_speech(speech: &SpeechReport, voice: &VoiceRequest) -> String {
    let mut parts = vec![
        format!("audio {:.1}s", speech.decoded_ms as f64 / 1000.0),
        format!("gain +{}dB", js_text(speech.gain_db)),
    ];
    let vad = &speech.vad;
    match vad.speech_start_ms {
        Some(start) => {
            let end = vad.speech_end_ms.map_or_else(|| "…".to_string(), seconds);
            parts.push(format!("speech {}–{end}s", seconds(start)));
        }
        None => parts.push(format!("no speech after the wake word (vad max {})", js_text(vad.max_probability))),
    }
    if voice.interim.quiet() {
        parts.push("quiet speech".into());
    }
    if let Some(voicing) = &voice.voicing {
        parts.push(if voicing.longest_run >= VOICED_SPEECH_RUN {
            format!("voiced ({} windows in a row), not a whisper", voicing.longest_run)
        } else {
            format!("a whisper, no voice ({} windows of pitch, {} in a row)", voicing.windows, voicing.longest_run)
        });
    }
    if let Some(recognition) = &voice.recognition {
        parts.push(format!("asr {}ms", recognition.latency_ms));
    }
    if let Some(all) = &voice.all_audio {
        parts.push(format!("all-audio asr {}ms", all.recognition.latency_ms));
    }
    if voice.interim.count() > 0 {
        parts.push(format!("interims {}", voice.interim.count()));
    }
    if voice.partials > 0 {
        parts.push(format!("partials {}", voice.partials));
    }
    if speech.decode_errors > 0 {
        parts.push(format!(
            "decode errors {} ({})",
            speech.decode_errors,
            speech.last_decode_error.as_deref().unwrap_or("undefined")
        ));
    }
    if speech.skipped_bytes > 0 {
        parts.push(format!("skipped {}B", speech.skipped_bytes));
    }
    if let Some(error) = &voice.error {
        parts.push(error.clone());
    }
    parts.join(", ")
}

/// A request's end from its answer's: spoken (or text alone, when nothing failed) is answered; so is a command
/// without words (the radio's audio_play, «громче»: live 05.11 two radio requests were journaled as silence).
/// The station closes the stream of an answer it gave up on, then says why: wait this long for its word.
const STATION_REASON_WAIT_MS: u64 = 1000;
/// A deferred answer is said once the station has been free this long (a follow-up's listening starts within it),
/// looked at this often.
const IDLE_SETTLE_MS: u64 = 1000;
const IDLE_CHECK_MS: u64 = 250;

/// The station's requestStat errors that say it no longer waits for an answer's response or speech (the session
/// journals, 01–06.11): its 15 s for the response, its 10 s for the first audio and between audio, its player gone.
fn station_gave_up_on(error: &str) -> bool {
    ["Vins request timeout", "Synthesis start timeout", "Synthesis chunk timeout", "Audioclient disconnected"]
        .iter()
        .any(|what| error.contains(what))
}

/// «5 min», «90 s».
fn minutes(ms: u64) -> String {
    if ms >= 60_000 && ms.is_multiple_of(60_000) {
        format!("{} min", ms / 60_000)
    } else {
        format!("{} s", ms / 1000)
    }
}

fn status_of(outcome: SayOutcome, failed: bool, commanded: bool) -> RequestStatus {
    match outcome {
        SayOutcome::Answer(AnswerOutcome::Spoken) => RequestStatus::Answered,
        SayOutcome::Answer(AnswerOutcome::Silent) => {
            if failed {
                RequestStatus::Failed
            } else {
                RequestStatus::Answered
            }
        }
        SayOutcome::Answer(AnswerOutcome::Empty) => {
            if commanded {
                RequestStatus::Answered
            } else {
                RequestStatus::Silent
            }
        }
        SayOutcome::Answer(AnswerOutcome::Cancelled) => RequestStatus::Cancelled,
        // Still going: its words follow, and its request ends with them.
        SayOutcome::Answer(AnswerOutcome::Deferred) => RequestStatus::Active,
        SayOutcome::Failed => RequestStatus::Failed,
    }
}

/// The journal's name of what we send.
fn kind_of(message: &ServerMessage) -> &'static str {
    match message {
        ServerMessage::SynchronizeStateResponse { .. } => "synchronizeStateResponse",
        ServerMessage::InputStartAck { .. } => "inputStartAck",
        ServerMessage::AsrResult { .. } => "asrResult",
        ServerMessage::SpotterValidation { .. } => "spotterValidation",
        ServerMessage::LogAck { .. } => "logAck",
        ServerMessage::RequestStatAck { .. } => "requestStatAck",
        ServerMessage::AliceResponse { .. } => "aliceResponse",
        ServerMessage::TtsSpeak { .. } => "ttsSpeak",
        ServerMessage::StreamClose { .. } => "streamClose",
        ServerMessage::Push { .. } => "push",
    }
}

fn arrow(dir: EventDirection) -> &'static str {
    match dir {
        EventDirection::In => "←",
        EventDirection::Out => "→",
        EventDirection::Note => "·",
        EventDirection::ToYandex => "⇢ yandex",
        EventDirection::FromYandex => "⇠ yandex",
    }
}

/// An empty request, for a response when the request's own frame is gone.
fn unknown_request() -> AliceRequest {
    AliceRequest {
        request_id: None,
        sequence_number: None,
        dialog_id: None,
        application: None,
        event: AliceEvent {
            r#type: AliceEventType::Unknown,
            text: None,
            name: None,
            payload: None,
            payload_raw: None,
            asr_whisper: None,
        },
        enable_spotter_validation: None,
        spotter_phrase: None,
        activation_type: None,
        lang: None,
        topic: None,
        device_state: None,
    }
}

/// A request's id, when the station gave one.
fn request_id_of(request: &AliceRequest) -> Option<&str> {
    request.request_id.as_deref().filter(|id| !id.is_empty())
}

/// Whether the station listens again after the last answer: decided by that answer.
enum FollowUp {
    /// Our answer: it knows once its text is complete.
    Answer(Arc<SpokenAnswer>),
    /// Yandex's: its should_listen.
    Listens(bool),
}

/// A request whose answer the station still plays: it ends with the speech, or when something new begins.
struct Lingering {
    r#ref: String,
    id: u64,
    status: RequestStatus,
    error: Option<String>,
    owner: Owner,
    timer: CancellationToken,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VoiceKey {
    stream_id: u32,
    serial: u64,
}

/// What a callback of our push brings: the request it answers.
type SayCallback = (String, AliceRequest);

/// A voice request for Yandex that came before our session with it was open: its frames, the VoiceInput first, wait
/// here in the order they came until it is.
struct HeldVoice {
    message_id: String,
    stream_id: Option<u32>,
    request: AliceRequest,
    frames: Vec<Bytes>,
}

/// The station's streams given to Yandex as they are, the newest kept: what Yandex says on them is the station's.
const KEPT_STATION_STREAMS: usize = 8;

pub struct Inner {
    closed_at: Option<String>,
    wire: Option<WireFormat>,
    client_info: Option<ClientInfo>,
    /// In the order they started, as a JS Map keeps them; one per stream.
    voice_requests: Vec<VoiceRequest>,
    spotter_logs: Vec<SpotterLog>,
    orphan_streams: HashSet<u32>,
    json_warned: bool,
    /// The station's SynchronizeState frame: it opens our speech session with Yandex. Memory only.
    synchronize_state: Option<Bytes>,
    /// Our session with Yandex: speech, pushes, the Yandex route. Open while the station is connected.
    yandex: Option<Arc<YandexSession>>,
    /// A request Yandex answers right now, relayed to the station.
    yandex_relay: Option<Arc<YandexRelay>>,
    /// Callbacks of the player Yandex answers beside the dialog, by the station's message id.
    player_relays: Vec<(String, Arc<YandexRelay>)>,
    request_frames: Vec<RequestFrame>,
    yandex_ids: Vec<String>,
    /// Streams Yandex opened with a push (it speaks): ours by its id.
    push_streams: Vec<(u32, u32)>,
    /// The station's streams that go to Yandex as they are (music recognition, a voice nothing here recognises).
    forwarded_streams: HashSet<u32>,
    /// The same, still known after the station closed them: Yandex may answer on them later.
    station_streams_at_yandex: VecDeque<u32>,
    /// A voice request waiting for our session with Yandex to open.
    held_voice: Option<HeldVoice>,
    stray_stream_noted: bool,
    /// Whether the station's next listening without the wake word is wanted: set by the last answer.
    follow_up: Option<FollowUp>,
    /// The gain the last wake word gave: a request without one (listening on after a question) starts with it.
    /// Before the session's first, the station's typical one (SpeechService::wake_word_gain).
    last_gain_db: Option<f64>,
    /// The answer being spoken; a new request or a cancel stops it.
    answer: Option<Arc<SpokenAnswer>>,
    /// Follow-ups go to a Yandex dialog or our game while it lasts.
    continuation: Option<Continuation>,
    /// Until when the next request answers the model's question about a command, and so is a command too.
    command_question_until: u64,
    /// The request that was whispered: its answer is whispered too.
    whispered: Option<String>,
    /// The requests being followed (their trip: steps, words, end), by the station's message id.
    trips: Vec<(String, Arc<RequestHandle>)>,
    lingering: Vec<Lingering>,
    /// Speech pushed to the station, waiting for its callback: by our id, oldest first.
    pending_says: Vec<(String, oneshot::Sender<SayCallback>)>,
    /// Answers the station stopped waiting for, going on in the background: their words follow through our push.
    /// Not the current answer: a new request does not stop them.
    deferred: Vec<Arc<SpokenAnswer>>,
    /// A deferred answer is being said now: the next waits.
    delivering: bool,
}

impl Inner {
    fn voice(&mut self, key: VoiceKey) -> Option<&mut VoiceRequest> {
        self.voice_requests.iter_mut().find(|voice| voice.stream_id == key.stream_id && voice.serial == key.serial)
    }

    fn voice_by_stream(&mut self, stream_id: u32) -> Option<&mut VoiceRequest> {
        self.voice_requests.iter_mut().find(|voice| voice.stream_id == stream_id)
    }

    fn request_frame(&self, message_id: &str) -> Option<&RequestFrame> {
        self.request_frames.iter().find(|kept| kept.message_id == message_id)
    }

    /// A voice request and its kept frame's audio, both borrowed.
    fn voice_and_raw(&mut self, key: VoiceKey) -> Option<(&mut VoiceRequest, Option<RawVoice<'_>>)> {
        let Inner { voice_requests, request_frames, .. } = self;
        let voice =
            voice_requests.iter_mut().find(|voice| voice.stream_id == key.stream_id && voice.serial == key.serial)?;
        let raw = if voice.raw {
            request_frames
                .iter_mut()
                .find(|kept| kept.message_id == voice.message_id)
                .map(|kept| RawVoice { audio: &mut kept.audio, spotter_end_chunk: &mut kept.spotter_end_chunk })
        } else {
            None
        };
        Some((voice, raw))
    }

    fn trip(&self, r#ref: &str) -> Option<Arc<RequestHandle>> {
        self.trips.iter().find(|(known, _)| known == r#ref).map(|(_, trip)| trip.clone())
    }

    fn push_stream(&self, yandex_stream: u32) -> Option<u32> {
        self.push_streams.iter().find(|(theirs, _)| *theirs == yandex_stream).map(|(_, ours)| *ours)
    }
}

pub struct StationSession {
    pub id: String,
    pub connected_at: String,
    connected: Instant,
    pub device_tag: String,
    pub remote_address: String,
    /// The host the station reached us at (its Host header): our /media lives there too.
    host: Option<String>,
    /// The station's handshake headers: they open our speech session with Yandex. Memory only.
    handshake_headers: Vec<(String, String)>,
    deps: Arc<SessionDeps>,
    outbox: Arc<Outbox>,
    /// A leaf lock (see SessionJournal): anything may journal under its own lock.
    journal: SessionJournal,
    next_stream_id: AtomicU32,
    next_serial: AtomicU64,
    inner: Mutex<Inner>,
    me: Weak<StationSession>,
}

/// The session as our answers, relays and Yandex session see it; nothing once the session is gone.
struct Link(Weak<StationSession>);

impl AnswerOutput for Link {
    fn send(&self, message: ServerMessage) {
        if let Some(session) = self.0.upgrade() {
            session.send(&message, false);
        }
    }

    fn send_audio(&self, stream_id: u32, bytes: &[u8]) {
        if let Some(session) = self.0.upgrade() {
            session.send_audio(stream_id, bytes);
        }
    }

    fn new_stream_id(&self) -> u32 {
        self.0.upgrade().map_or(0, |session| session.new_stream_id())
    }
}

impl RelayOutput for Link {
    fn send_frame(&self, frame: Bytes, kind: &str, summary: String, decoded: &Value) {
        if let Some(session) = self.0.upgrade() {
            session.send_raw(frame, kind, summary, decoded);
        }
    }

    fn send_audio(&self, frame: Bytes) {
        if let Some(session) = self.0.upgrade() {
            session.outbox.send_binary(frame);
        }
    }

    fn close_stream(&self, stream_id: u32, reason: CloseReason) {
        if let Some(session) = self.0.upgrade() {
            session.send(&ServerMessage::StreamClose { stream_id, reason }, false);
        }
    }

    fn new_stream_id(&self) -> u32 {
        self.0.upgrade().map_or(0, |session| session.new_stream_id())
    }
}

impl UpstreamJournal for Link {
    fn record(&self, dir: EventDirection, kind: &str, summary: Option<String>, data: Option<Value>) {
        if let Some(session) = self.0.upgrade() {
            session.record(dir, kind, summary, data.as_ref(), false);
        }
    }

    fn note(&self, kind: &str, summary: &str) {
        if let Some(session) = self.0.upgrade() {
            session.note(kind, summary, None);
        }
    }
}

impl YandexEvents for Link {
    fn on_push(&self, frame: Bytes, kind: &str, decoded: &Value) {
        if let Some(session) = self.0.upgrade() {
            session.on_yandex_push(frame, kind, decoded);
        }
    }

    fn on_stray_stream(&self, frame: Bytes, stream_id: u32, is_control: bool) {
        if let Some(session) = self.0.upgrade() {
            session.on_yandex_stray(frame, stream_id, is_control);
        }
    }
}

/// The request's trip follows the answer: its text as it grows, its speech's start.
struct TripObserver {
    trip: Arc<RequestHandle>,
    by: Place,
    what: Option<String>,
}

impl AnswerObserver for TripObserver {
    fn text(&self, text: &str) {
        self.trip.answered(text);
    }

    fn speaking(&self) {
        self.trip.step(RequestPhase::Speak, Some(self.by), self.what.as_deref());
    }
}

/// Which model thinks and where.
struct Brain {
    by: Place,
    what: String,
}

impl StationSession {
    /// The session for a station that just connected, and its socket's queue.
    pub fn new(
        handshake: &Handshake,
        deps: Arc<SessionDeps>,
    ) -> (Arc<StationSession>, mpsc::UnboundedReceiver<Outgoing>) {
        let id = new_session_id();
        let connected_at = now_iso();
        let device_tag = device_tag_of(&handshake.headers, deps.config.recorder.redact);
        let recorder = SessionRecorder::new(&deps.config.recorder, &id, &device_tag, &connected_at);
        let journal = SessionJournal::new(recorder, deps.config.session_events_limit);
        let host = handshake
            .headers
            .get("host")
            .and_then(|host| host.to_str().ok())
            .filter(|host| !host.is_empty())
            .map(str::to_string);
        let (outbox, outgoing) = Outbox::new();
        let session = Arc::new_cyclic(|me| StationSession {
            id,
            connected_at,
            connected: Instant::now(),
            device_tag,
            remote_address: if handshake.remote_address.is_empty() {
                "?".into()
            } else {
                handshake.remote_address.clone()
            },
            host,
            handshake_headers: upstream_headers(&handshake.headers),
            deps,
            outbox,
            journal,
            next_stream_id: AtomicU32::new(2),
            next_serial: AtomicU64::new(1),
            inner: Mutex::new(Inner {
                closed_at: None,
                wire: None,
                client_info: None,
                voice_requests: Vec::new(),
                spotter_logs: Vec::new(),
                orphan_streams: HashSet::new(),
                json_warned: false,
                synchronize_state: None,
                yandex: None,
                yandex_relay: None,
                player_relays: Vec::new(),
                request_frames: Vec::new(),
                yandex_ids: Vec::new(),
                push_streams: Vec::new(),
                forwarded_streams: HashSet::new(),
                station_streams_at_yandex: VecDeque::new(),
                held_voice: None,
                stray_stream_noted: false,
                follow_up: None,
                last_gain_db: None,
                answer: None,
                continuation: None,
                command_question_until: 0,
                whispered: None,
                trips: Vec::new(),
                lingering: Vec::new(),
                pending_says: Vec::new(),
                deferred: Vec::new(),
                delivering: false,
            }),
            me: me.clone(),
        });
        (session, outgoing)
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn link(&self) -> Arc<Link> {
        Arc::new(Link(self.me.clone()))
    }

    fn new_stream_id(&self) -> u32 {
        self.next_stream_id.fetch_add(2, Ordering::SeqCst)
    }

    /// The station's socket until it closes (or is cut, silent: STATION_PING_MS); then the session's end, and how
    /// the socket ended.
    pub async fn run(
        self: &Arc<Self>,
        socket: WebSocket,
        outgoing: mpsc::UnboundedReceiver<Outgoing>,
        handshake: Handshake,
        stopping: CancellationToken,
    ) -> SocketEnd {
        self.note(
            "connected",
            &format!("from {} device={}", self.remote_address, self.device_tag),
            Some(json!({ "headers": recorded_headers(&handshake.headers) })),
        );
        let me = self.clone();
        let liveness = Liveness::every(self.deps.config.station_ping_ms);
        let end =
            pump(socket, outgoing, stopping, liveness, move |frame, is_binary| me.on_message(frame, is_binary)).await;
        if let Some(error) = &end.error {
            self.note("socketError", error, None);
        }
        self.outbox.closed();
        self.on_close(end.code, &end.reason);
        end
    }

    /// Closes the station's socket (a mode switch): it reconnects into the new mode.
    pub fn close_socket(&self, code: u16, reason: &str) {
        self.outbox.close(code, reason);
    }

    pub fn closed_at(&self) -> Option<String> {
        self.lock().closed_at.clone()
    }

    /// The session's state is read first and let go; the journal's own lock is taken after, on its own.
    pub fn snapshot(&self) -> SessionSnapshot {
        let mut snapshot = {
            let inner = self.lock();
            SessionSnapshot {
                id: self.id.clone(),
                mode: Some("gateway"),
                device_tag: self.device_tag.clone(),
                remote_address: self.remote_address.clone(),
                connected_at: self.connected_at.clone(),
                closed_at: inner.closed_at.clone(),
                wire: inner.wire.map(|wire| match wire {
                    WireFormat::Aapi => "aapi",
                    WireFormat::Json => "json",
                }),
                device: inner.client_info.as_ref().map(DeviceView::of),
                active_voice_requests: inner.voice_requests.len(),
                can_speak: inner.synchronize_state.is_some(),
                // Our Yandex session's own lock, under the session's (the order everywhere: session, then Yandex).
                yandex_connected: Some(inner.yandex.as_ref().is_some_and(|yandex| yandex.connected())),
                events: 0,
                journal: None,
            }
        };
        snapshot.events = self.journal.events();
        snapshot.journal = self.journal.dir_display();
        snapshot
    }

    pub fn can_speak(&self) -> bool {
        self.lock().synchronize_state.is_some()
    }

    pub fn yandex_connected(&self) -> bool {
        self.lock().yandex.as_ref().is_some_and(|yandex| yandex.connected())
    }

    pub fn recent_events(&self, limit: usize) -> Vec<SessionEventRecord> {
        self.journal.recent(limit)
    }

    /// Speaks on the station without a question (the debug API, our reminders, a call announced), with recorded
    /// audio after the text if given (a voice message).
    ///
    /// The speech runs on its own task, whatever its caller does: a Node promise ran to its end when the HTTP client
    /// left, so the station hears the whole speech, its stream closes and its trip ends. A caller that stops waiting
    /// (a closed panel tab, a client's timeout) only misses the outcome.
    pub async fn say(self: &Arc<Self>, text: String, clip: Option<AudioClip>) -> SayOutcome {
        let session = self.clone();
        match tokio::spawn(async move { session.speak(text, clip).await }).await {
            Ok(outcome) => outcome,
            Err(error) => {
                tracing::error!(target: "session", "Session {}: the speech failed: {error}", tag8(&self.id));
                SayOutcome::Failed
            }
        }
    }

    async fn speak(self: Arc<Self>, text: String, clip: Option<AudioClip>) -> SayOutcome {
        let label = match &clip {
            Some(clip) => format!("say \"{text}\" + {}", clip.label),
            None => format!("say \"{text}\""),
        };
        let asked = match &clip {
            Some(clip) => format!("{text} [{}]", clip.label),
            None => text.clone(),
        };
        let answer = self
            .ask_by_push(&label, asked, |inner, r#ref, request| {
                let mut options = self.answer_options(inner, r#ref, request);
                options.listen = ListenMode::Never;
                options.hold_response_ms = 0;
                // Said through our push already (a reminder, a deferred answer): past the wait it stops.
                options.deferrable = false;
                self.start_answer(inner, options)
            })
            .await;
        let Some(answer) = answer else { return SayOutcome::Failed };
        let mut parts = vec![AnswerPart::Text(text)];
        if let Some(clip) = clip {
            parts.push(AnswerPart::Clip(clip));
        }
        self.play_answer(answer, futures::stream::iter(parts).boxed(), label).await
    }

    /// Our own skill's answer to a push: the turn timer's «время вышло» (plugins/skill.rs). The station is asked as
    /// for our speech, and its request is answered as a phrase is — said, played, listened after, the conversation
    /// going on. `still` is asked once the station calls back: no longer wanted (the person's words came first), the
    /// callback gets an empty response and the answer being spoken plays on. False when nothing was spoken to its
    /// end.
    pub async fn answer_pushed(
        self: &Arc<Self>,
        intent: Intent,
        label: String,
        still: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> bool {
        let session = self.clone();
        // On its own task, as `say`: a caller that stops waiting does not cut the answer short.
        tokio::spawn(async move { session.push_answer(intent, label, still).await }).await.unwrap_or(false)
    }

    async fn push_answer(
        self: Arc<Self>,
        intent: Intent,
        label: String,
        still: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> bool {
        let Some(skill) = self.deps.skills.router.implementation(&intent) else { return false };
        let started = self
            .ask_by_push(&label, label.clone(), |inner, r#ref, request| {
                if !still() {
                    self.send_empty_response(r#ref, request, Vec::new());
                    self.end_trip(inner, r#ref, RequestStatus::Silent, Some("no longer wanted"), None);
                    return None;
                }
                let mut options = self.answer_options(inner, r#ref, request);
                // The outcome asks the station to listen when the conversation goes on (apply_outcome).
                options.listen = ListenMode::Never;
                options.hold_response_ms = 0;
                options.deferrable = false;
                let answer = self.start_answer(inner, options);
                let context = self.execution_context(inner, "", request, answer.token());
                Some((answer, context, request.clone()))
            })
            .await
            .flatten();
        let Some((answer, context, request)) = started else { return false };
        let session = self.clone();
        let phrases_answer = answer.clone();
        let phrases: PhraseStream = generate(move |y| async move {
            let outcome = match skill.execute(&intent, &context).await {
                Ok(outcome) => outcome,
                Err(error) => return y.send(Err(error)).await,
            };
            let mut phrases = session.apply_outcome(&phrases_answer, &request, "", outcome, None);
            while let Some(phrase) = phrases.next().await {
                y.send(phrase).await;
            }
        });
        let parts = self.with_errors(&answer, phrases);
        self.play_answer(answer, parts, label).await == SayOutcome::Answer(AnswerOutcome::Spoken)
    }

    /// Words run in their time as if said now (a delayed command, skills/own/later_skill.rs): the station is asked by
    /// a push, and its request is served as these words — the same routes, rules and answers. False when it did not
    /// call back.
    pub async fn run_words(self: &Arc<Self>, text: String) -> bool {
        let session = self.clone();
        tokio::spawn(async move {
            let label = format!("отложенное «{text}»");
            session
                .ask_by_push(&label, text.clone(), |inner, r#ref, request| {
                    session.handle_request(inner, r#ref, request, &text, false)
                })
                .await
                .is_some()
        })
        .await
        .unwrap_or(false)
    }

    /// The person's words are being heard now: a voice request still listening with words in it (without interim
    /// recognition, any still listening), or one being recognized — the turn timer waits for them.
    pub fn hearing(&self) -> bool {
        use super::voice_request::VoiceState;
        let interim = self.deps.config.partial_interval_ms > 0;
        let inner = self.lock();
        inner.voice_requests.iter().any(|voice| match voice.state {
            VoiceState::Recognizing => true,
            VoiceState::Listening => !interim || !voice.interim.text().is_empty(),
            VoiceState::Done => false,
        })
    }

    /// The station's player plays, as it last said (its device state in a request).
    pub fn playing(&self) -> bool {
        self.deps.skills.player_memory.get(&self.device_tag).is_some_and(|player| player.playing)
    }

    /// An alarm or a timer of ours rings on the station now.
    pub fn ringing(&self) -> bool {
        self.deps.skills.ringing_now(&self.device_tag, None, now_ms())
    }

    /// Someone is with the station: it rings, words are being heard, or a conversation goes on (a game, a question) —
    /// speech by a plugin's schedule waits (plugins/schedule.rs).
    pub fn busy(&self) -> bool {
        let talking = self.lock().continuation.is_some_and(|continuation| continuation.until > now_ms());
        talking || self.ringing() || self.hearing()
    }

    /// The station's time zone as it said it.
    pub fn zone(&self) -> Option<String> {
        self.lock().client_info.as_ref().and_then(|info| info.timezone.clone())
    }

    /// Asks the station by a push to call back and answers its request with `answer`, under the session's state. The
    /// station ignores speech for a request it never sent (live: /say "spoken", the station silent); as Yandex rings
    /// a reminder, a push makes the station ask, and our words are the answer to that request. None when no
    /// callback came: the trip `asked` ends failed.
    async fn ask_by_push<R>(
        &self,
        label: &str,
        asked: String,
        answer: impl FnOnce(&mut Inner, &str, &AliceRequest) -> R,
    ) -> Option<R> {
        let id = new_message_id();
        let trip = self.deps.requests.as_ref().map(|requests| {
            requests.begin(RequestStart {
                device: self.device_tag.clone(),
                session: Some(self.id.clone()),
                kind: RequestKind::Say,
                text: Some(asked),
                parent: None,
            })
        });
        let (resolve, called_back) = oneshot::channel::<SayCallback>();
        {
            let mut inner = self.lock();
            inner.pending_says.push((id.clone(), resolve));
            let mut payload = Map::new();
            payload.insert("id".into(), json!(id));
            let directive = Directive {
                r#type: DirectiveType::ServerAction,
                name: SAY_CALLBACK.clone(),
                analytics_type: Some(SAY_CALLBACK.clone()),
                payload: Some(payload),
                typed_payload: None,
                is_led_silent: Some(true),
                ignore_answer: None,
            };
            self.send(&ServerMessage::Push { id: id.clone(), directive }, false);
        }
        let callback = match tokio::time::timeout(Duration::from_millis(SAY_CALLBACK_MS), called_back).await {
            Ok(Ok(callback)) => Some(callback),
            _ => None,
        };
        let mut inner = self.lock();
        inner.pending_says.retain(|(pending, _)| *pending != id);
        let Some((r#ref, request)) = callback else {
            self.note(
                "sayFailed",
                &format!("{label}: no {} from the station in {SAY_CALLBACK_MS} ms", *SAY_CALLBACK),
                Some(json!({ "id": id })),
            );
            if let Some(trip) = trip {
                trip.finish(
                    RequestStatus::Failed,
                    Some(&format!("no callback from the station in {SAY_CALLBACK_MS} ms")),
                    None,
                );
            }
            return None;
        };
        if let Some(trip) = trip {
            Self::set_trip(&mut inner, &r#ref, trip);
        }
        Some(answer(&mut inner, &r#ref, &request))
    }

    /// A command of ours nobody asked for (the player's retry through the gateway, stage 5а): pushed, and the station
    /// calls back for it as for our speech.
    pub fn push(&self, directive: Directive) {
        self.note("push", &directive.name, None);
        self.send(&ServerMessage::Push { id: new_message_id(), directive }, false);
    }

    /// The station's callback to our push: the speech waiting under its id (the oldest, when it brings none).
    fn on_say_callback(&self, inner: &mut Inner, r#ref: &str, request: &AliceRequest) {
        let id = match request.event.payload.as_ref().and_then(|payload| payload.get("id")) {
            Some(Value::String(id)) => id.clone(),
            Some(Value::Null) | None => String::new(),
            Some(other) => other.to_string(),
        };
        let index = inner
            .pending_says
            .iter()
            .position(|(pending, _)| *pending == id)
            .or_else(|| (!inner.pending_says.is_empty()).then_some(0));
        let Some(index) = index else {
            self.note(
                "callback",
                &format!("{}: no speech waits for it (late or repeated), an empty response", *SAY_CALLBACK),
                None,
            );
            return self.send_empty_response(r#ref, request, Vec::new());
        };
        let (_, resolve) = inner.pending_says.remove(index);
        // The speech stopped waiting a moment ago (its time ran out and it has not yet taken its entry back): the
        // late callback gets its empty response, as in the TS, where the speech's give-up and its entry's removal
        // came before any frame.
        if resolve.send((r#ref.to_string(), request.clone())).is_err() {
            self.note(
                "callback",
                &format!("{}: its speech no longer waits for it (late), an empty response", *SAY_CALLBACK),
                None,
            );
            self.send_empty_response(r#ref, request, Vec::new());
        }
    }

    /// Debug API: synthesizes a text through Yandex without playing it, to check the speech session silently. On
    /// its own task, as `say`: a caller that leaves does not cut the phrase short.
    pub async fn synthesize_only(self: &Arc<Self>, text: &str) -> anyhow::Result<SynthesisCheck> {
        let session = self.clone();
        let text = text.to_string();
        tokio::spawn(async move { session.check_synthesis(&text).await })
            .await
            .unwrap_or_else(|error| Err(anyhow::anyhow!("the check failed: {error}")))
    }

    async fn check_synthesis(&self, text: &str) -> anyhow::Result<SynthesisCheck> {
        let started = Instant::now();
        let synthesize = {
            let inner = self.lock();
            self.synthesizer(&inner)
        };
        let cancel = CancellationToken::new();
        let timeout = cancel.clone();
        let timer = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(30)).await;
            timeout.cancel();
        });
        let synthesis = synthesize(text.to_string(), cancel).await;
        let first_audio_ms = started.elapsed().as_millis() as u64;
        let synthesis = match synthesis {
            Ok(synthesis) => synthesis,
            Err(error) => {
                timer.abort();
                return Err(error);
            }
        };
        let mut joiner = OggOpusJoiner::new();
        let _ = joiner.begin_segment();
        let mut bytes = 0;
        let mut audio = synthesis.audio;
        while let Some(chunk) = audio.next().await {
            let chunk = chunk?;
            bytes += chunk.len();
            joiner.push(&chunk)?;
        }
        timer.abort();
        let check = SynthesisCheck {
            format: synthesis.format,
            bytes,
            audio_ms: joiner.duration_ms(),
            first_audio_ms,
            total_ms: started.elapsed().as_millis() as u64,
        };
        self.note(
            "ttsCheck",
            &format!(
                "\"{text}\": {} bytes, {:.1}s of audio, first audio +{}ms",
                check.bytes,
                check.audio_ms as f64 / 1000.0,
                check.first_audio_ms
            ),
            None,
        );
        Ok(check)
    }

    // ---------------------------------------------------------------- inbound

    fn on_message(self: &Arc<Self>, frame: Bytes, is_binary: bool) {
        let mut inner = self.lock();
        let handled = if !is_binary {
            self.on_text_frame(&mut inner, &String::from_utf8_lossy(&frame));
            Ok(())
        } else if AapiCodec::is_aapi_frame(&frame) {
            self.on_proto_frame(&mut inner, frame.clone())
        } else {
            self.on_stream_frame(&mut inner, &frame)
        };
        if let Err(error) = handled {
            let message = format!("{error:#}");
            tracing::error!(target: "session", "Session {} cannot handle frame ({} bytes): {message}", tag8(&self.id), frame.len());
            let head = &frame[..frame.len().min(256)];
            self.note("frameError", &message, Some(json!({ "frame": bytes_value(head) })));
        }
    }

    fn on_text_frame(&self, inner: &mut Inner, text: &str) {
        if inner.wire.is_none() {
            inner.wire = Some(WireFormat::Json);
        }
        if !inner.json_warned {
            inner.json_warned = true;
            tracing::warn!(target: "session", "Session {} station speaks the legacy JSON protocol, which is not supported yet", tag8(&self.id));
        }
        let parsed = serde_json::from_str::<Value>(text).unwrap_or_else(|_| Value::String(text.to_string()));
        self.record(EventDirection::In, "json", None, Some(&parsed), false);
    }

    fn on_proto_frame(self: &Arc<Self>, inner: &mut Inner, frame: Bytes) -> anyhow::Result<()> {
        if inner.wire.is_none() {
            inner.wire = Some(WireFormat::Aapi);
        }
        let decoded = self.deps.codec.decode_client(&frame).map_err(|error| anyhow::anyhow!("{error}"))?;
        let message = decoded.message;
        if let ClientMessage::Event(event) = &message {
            match event {
                // Kept whole: the tokens in it are not in our schema, and our session with Yandex repeats it.
                ClientEvent::SynchronizeState { .. } => inner.synchronize_state = Some(frame.clone()),
                ClientEvent::VoiceInput { header, .. } => {
                    Self::keep_request_frame(inner, &header.message_id, FrameKind::VoiceInput, &frame)
                }
                ClientEvent::TextInput { header, .. } => {
                    Self::keep_request_frame(inner, &header.message_id, FrameKind::TextInput, &frame)
                }
                _ => {}
            }
        }
        let kind = match &message {
            ClientMessage::StreamControl(_) => "streamControl",
            ClientMessage::Event(event) => event.kind(),
        };
        self.record(EventDirection::In, kind, Some(summarize_client(&message)), Some(&decoded.decoded), false);
        if self.to_yandex(inner, &message, &frame) {
            return Ok(());
        }
        self.dispatch(inner, message, frame);
        Ok(())
    }

    fn keep_request_frame(inner: &mut Inner, message_id: &str, kind: FrameKind, frame: &Bytes) {
        let kept = RequestFrame {
            message_id: message_id.to_string(),
            kind,
            frame: frame.clone(),
            audio: None,
            spotter_end_chunk: None,
            stream_id: None,
        };
        match inner.request_frames.iter_mut().find(|known| known.message_id == message_id) {
            Some(known) => *known = kept,
            None => inner.request_frames.push(kept),
        }
        while inner.request_frames.len() > KEPT_REQUESTS {
            inner.request_frames.remove(0);
        }
    }

    /// The station's frames about what Yandex started go back to Yandex as they are: sync push responses, acks and
    /// playback states of its pushes and answers, the streams of a music recognition. True when sent there.
    fn to_yandex(self: &Arc<Self>, inner: &mut Inner, message: &ClientMessage, frame: &Bytes) -> bool {
        let Some(yandex) = inner.yandex.clone() else { return false };
        let event = match message {
            ClientMessage::StreamControl(control) => {
                if !inner.forwarded_streams.contains(&control.stream_id) {
                    return false;
                }
                if control.action == StreamAction::Close {
                    inner.forwarded_streams.remove(&control.stream_id);
                }
                if let Some(held) = inner.held_voice.as_mut().filter(|held| held.stream_id == Some(control.stream_id)) {
                    held.frames.push(frame.clone());
                    return true;
                }
                return yandex.send(frame.clone(), "streamControl");
            }
            ClientMessage::Event(event) => event,
        };
        match event {
            ClientEvent::Unknown { name, .. } if name == "syncPushResponse" => {
                yandex.send(frame.clone(), "syncPushResponse")
            }
            ClientEvent::Ack { received_message_id: Some(received), .. } if inner.yandex_ids.contains(received) => {
                yandex.send(frame.clone(), "ack")
            }
            ClientEvent::TtsUpdateState { request_message_id: Some(r#ref), .. } if inner.yandex_ids.contains(r#ref) => {
                yandex.send(frame.clone(), "ttsUpdateState")
            }
            ClientEvent::Unknown { name, header, .. } if name == "musicInput" => {
                // The music recognizer Yandex started: the station's recording and its stream go to Yandex.
                if let Some(stream_id) = header.stream_id {
                    Self::forward_stream(inner, stream_id);
                }
                self.relay_yandex(
                    inner,
                    None,
                    &header.message_id,
                    vec![frame.clone()],
                    CapabilityName::MusicRecognition,
                    "music recognition",
                    NO_EVENTS,
                    "musicInput",
                    false,
                );
                true
            }
            _ => false,
        }
    }

    fn on_stream_frame(self: &Arc<Self>, inner: &mut Inner, frame: &Bytes) -> anyhow::Result<()> {
        if frame.len() < 4 {
            anyhow::bail!("stream frame shorter than its 4-byte stream id");
        }
        let stream_id = u32::from_be_bytes([frame[0], frame[1], frame[2], frame[3]]);
        let payload = &frame[4..];
        if inner.forwarded_streams.contains(&stream_id) {
            if let Some(held) = inner.held_voice.as_mut().filter(|held| held.stream_id == Some(stream_id)) {
                held.frames.push(frame.clone());
            } else if let Some(yandex) = &inner.yandex {
                yandex.send(frame.clone(), "audio");
            }
            return Ok(());
        }
        if let Some(key) = inner.voice_by_stream(stream_id).map(|voice| VoiceKey { stream_id, serial: voice.serial }) {
            if let Some((voice, raw)) = inner.voice_and_raw(key) {
                voice.append(payload, raw);
            }
            self.flush_voice(inner, key);
            return Ok(());
        }
        if let Some(log) = inner.spotter_logs.iter_mut().find(|log| log.stream_id == stream_id) {
            if log.bytes + payload.len() <= MAX_SPOTTER_LOG_BYTES {
                log.chunks.push(Bytes::copy_from_slice(payload));
            }
            log.bytes += payload.len();
            return Ok(());
        }
        if inner.orphan_streams.insert(stream_id) {
            self.note("orphanStream", &format!("audio for stream {stream_id} without an open voice request"), None);
        }
        Ok(())
    }

    /// What the speech pipeline found in the audio queued so far: the speech's start, the end, a failure.
    fn flush_voice(self: &Arc<Self>, inner: &mut Inner, key: VoiceKey) {
        let Some(voice) = inner.voice(key) else { return };
        let events = voice.speech.flush();
        for event in events {
            match event {
                SpeechEvent::SpeechStart { at_ms } => {
                    if let Some(voice) = inner.voice(key) {
                        voice.timeline.mark("speechStart");
                        tracing::debug!(target: "session", "voice {}: speech starts at {}s of audio", voice.tag(), seconds(at_ms));
                    }
                }
                SpeechEvent::End { reason, .. } => self.complete_voice(inner, key, reason.as_str()),
                SpeechEvent::Error(error) => {
                    if let Some(voice) = inner.voice(key) {
                        voice.error = Some(format!("speech: {error}"));
                    }
                    self.complete_voice(inner, key, "speechError");
                }
            }
        }
    }

    fn dispatch(self: &Arc<Self>, inner: &mut Inner, message: ClientMessage, frame: Bytes) {
        let event = match message {
            ClientMessage::StreamControl(control) => return self.on_stream_control(inner, &control),
            ClientMessage::Event(event) => event,
        };
        match event {
            ClientEvent::SynchronizeState { header, application, .. } => {
                inner.client_info = application;
                self.send(
                    &ServerMessage::SynchronizeStateResponse { r#ref: header.message_id, session_id: self.id.clone() },
                    false,
                );
                let info = inner.client_info.clone();
                self.deps.skills.prepare(info.as_ref().and_then(|info| info.timezone.as_deref()));
                self.deps.events.emit(GatewayEvent::StationSynced {
                    device_tag: self.device_tag.clone(),
                    device_model: info.as_ref().and_then(|info| info.device_model.clone()),
                    device_color: info.as_ref().and_then(|info| info.device_color.clone()),
                    firmware: info.as_ref().and_then(|info| info.firmware_version.clone()),
                });
                self.open_yandex(inner);
            }
            ClientEvent::VoiceInput { header, request } => {
                self.remember_player(&request);
                // Nothing here recognises speech (a station's gateway with no computer and no cloud): the request
                // goes to Yandex whole, voice and all, as in «Яндекс» — closed with an empty answer, the station fell
                // silent (live, 07.11).
                if !self.deps.speech.recognition_enabled() && inner.yandex.is_some() {
                    return self.voice_to_yandex(inner, header, request, frame);
                }
                self.start_voice(inner, header, request);
            }
            ClientEvent::TextInput { header, request } => {
                if !is_player_continue(&request) {
                    self.remember_player(&request);
                }
                self.on_text_input(inner, header, request, frame);
            }
            ClientEvent::LogSpotter { header, format, transcript, firmware, .. } => {
                // Its firmware is said here alone: kept with its record, a change of it is an update.
                if firmware.is_some() {
                    self.deps.events.emit(GatewayEvent::StationSynced {
                        device_tag: self.device_tag.clone(),
                        device_model: None,
                        device_color: None,
                        firmware: firmware.clone(),
                    });
                }
                if let Some(stream_id) = header.stream_id {
                    inner.spotter_logs.retain(|log| log.stream_id != stream_id);
                    inner.spotter_logs.push(SpotterLog {
                        stream_id,
                        format,
                        transcript,
                        time: clock_tag(),
                        chunks: Vec::new(),
                        bytes: 0,
                    });
                }
                self.send(&ServerMessage::LogAck { r#ref: header.message_id }, false);
            }
            // Without the ack the station keeps resending the statistics.
            ClientEvent::RequestStat { header, request_message_id, error } => {
                self.send(&ServerMessage::RequestStatAck { r#ref: header.message_id }, false);
                if let (Some(r#ref), Some(error)) = (request_message_id.as_deref(), error.as_deref()) {
                    self.on_station_error(inner, r#ref, error);
                }
            }
            ClientEvent::CancelRequest { request_message_id, reason, .. } => {
                let reason = format!("cancel:{}", name_of(reason));
                let player = request_message_id.as_deref().and_then(|r#ref| {
                    inner.player_relays.iter().find(|(id, _)| id == r#ref).map(|(_, relay)| relay.clone())
                });
                if let Some(player) = player {
                    return player.cancel(&reason);
                }
                let r#ref = request_message_id.as_deref().filter(|r#ref| !r#ref.is_empty());
                if let Some(answer) = &inner.answer {
                    if r#ref.is_none_or(|r#ref| answer.options.r#ref == r#ref) {
                        answer.cancel(&reason);
                    }
                }
                for deferred in inner.deferred.iter().filter(|answer| r#ref == Some(answer.options.r#ref.as_str())) {
                    deferred.cancel(&reason);
                }
                if let Some(relay) = &inner.yandex_relay {
                    relay.cancel(&reason);
                }
                if inner.held_voice.as_ref().is_some_and(|held| r#ref.is_none_or(|r#ref| held.message_id == r#ref)) {
                    self.drop_held_voice(inner, RequestStatus::Cancelled, &reason);
                }
                self.cancel_voice(inner, r#ref, &reason);
            }
            ClientEvent::TtsGenerate { header, text, .. } => {
                // The station asks to speak a text itself (vocalize, alice-say.py): TtsSpeak and audio, no
                // AliceResponse.
                self.begin_trip(inner, &header.message_id, RequestKind::Say, Some(&text));
                let answer = self.start_answer(inner, SpokenAnswerOptions::speech_only(&header.message_id));
                let label = format!("TtsGenerate \"{text}\"");
                self.spawn_answer(answer, single(text), label);
            }
            // ack, ping, matchedUser, ttsUpdateState, unknown: journal only for now
            _ => {}
        }
    }

    fn on_stream_control(self: &Arc<Self>, inner: &mut Inner, control: &StreamControlEvent) {
        let close = control.action == StreamAction::Close;
        let reason = control.reason.map_or_else(|| "ok".to_string(), name_of);
        if close && inner.spotter_logs.iter().any(|log| log.stream_id == control.stream_id) {
            return self.save_spotter_log(inner, control.stream_id);
        }
        if close {
            if let Some(answer) = inner.answer.clone().filter(|answer| answer.stream_id() == Some(control.stream_id)) {
                let reason = format!("station:{reason}");
                if control.reason == Some(CloseReason::Cancel) || !answer.options.deferrable {
                    return answer.cancel(&reason);
                }
                // The station closes the stream of an answer it gave up on, and says why just after (requestStat:
                // 5 ms later on 06.11). Until then nothing goes to the stream; without a reason the answer stops.
                answer.station_closed_stream();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(STATION_REASON_WAIT_MS)).await;
                    if !answer.is_deferred() {
                        answer.cancel(&reason);
                    }
                });
                return;
            }
        }
        let Some(key) = inner
            .voice_by_stream(control.stream_id)
            .map(|voice| VoiceKey { stream_id: voice.stream_id, serial: voice.serial })
        else {
            return;
        };
        if control.action == StreamAction::SpotterEnd {
            if let Some((voice, raw)) = inner.voice_and_raw(key) {
                voice.mark_spotter_end(raw);
            }
            return self.flush_voice(inner, key);
        }
        if !close {
            return;
        }
        if let Some(voice) = inner.voice(key) {
            voice.station_closed = true;
        }
        if control.reason == Some(CloseReason::Cancel) {
            return self.abort_voice(inner, key, "station:cancel");
        }
        self.complete_voice(inner, key, &format!("station:{reason}"));
    }

    // ---------------------------------------------------------------- voice requests

    fn start_voice(self: &Arc<Self>, inner: &mut Inner, header: EventHeader, request: AliceRequest) {
        // A new request supersedes whatever the station asked before (a new wake word interrupts us).
        let previous: Vec<VoiceKey> = inner
            .voice_requests
            .iter()
            .map(|voice| VoiceKey { stream_id: voice.stream_id, serial: voice.serial })
            .collect();
        for key in previous {
            self.abort_voice(inner, key, "superseded");
        }
        if let Some(answer) = &inner.answer {
            answer.cancel("superseded");
        }
        if let Some(relay) = &inner.yandex_relay {
            relay.cancel("superseded");
        }

        let stream_id = header.stream_id.unwrap_or(1);
        let message_id = header.message_id.clone();
        self.send(
            &ServerMessage::InputStartAck {
                r#ref: message_id.clone(),
                request_start_time_us: (now_ms() * 1000) as i64,
            },
            false,
        );
        if request.activation_type.as_deref() == Some("auto_listening") && !self.follow_up_wanted(inner) {
            return self.dismiss_follow_up(inner, &message_id, &request, stream_id);
        }
        // The speech session opens while the user speaks, so the answer does not wait for it.
        if let Some(yandex) = &inner.yandex {
            yandex.warm_up();
        }
        if let Some(trip) = self.begin_trip(inner, &message_id, RequestKind::Voice, None) {
            trip.step(RequestPhase::Listen, Some(Place::Own), self.ear_name().as_deref());
        }

        let name = format!("{}_{}.in.ogg", clock_tag(), request_id_of(&request).unwrap_or(&message_id));
        let audio = self.journal.open_audio(&name);
        // The session's last wake word, else the station's typical one: after a restart or a reconnect, or with a
        // wake word of our own only, the session has heard none.
        let assumed = inner.last_gain_db.or_else(|| self.deps.speech.wake_word_gain(&self.device_tag));
        let speech = self.deps.speech.create_pipeline(request.activation_type.as_deref(), assumed);
        let interim = InterimRecognition::new(self.interim_options());
        let serial = self.next_serial.fetch_add(1, Ordering::SeqCst);
        let activation = request.activation_type.clone();
        let mut voice = VoiceRequest::new(serial, message_id.clone(), stream_id, request, audio, speech, interim);
        // Voice for Yandex (YANDEX_VOICE, a whisper): the station's audio as it came, until the route is decided.
        let keeps = !self.deps.skills.config.yandex_voice.is_empty() || self.deps.dialog.config.whisper_reply;
        if let Some(kept) = inner.request_frames.iter_mut().find(|kept| kept.message_id == message_id).filter(|_| keeps)
        {
            kept.audio = Some(Vec::new());
            kept.stream_id = Some(stream_id);
            voice.raw = true;
        }
        let key = VoiceKey { stream_id, serial };
        let timer = voice.timer.clone();
        let me = Arc::downgrade(self);
        let max_ms = self.deps.config.max_utterance_ms;
        tokio::spawn(async move {
            tokio::select! {
                _ = timer.cancelled() => {}
                _ = tokio::time::sleep(Duration::from_millis(max_ms)) => {
                    if let Some(session) = me.upgrade() {
                        let mut inner = session.lock();
                        session.complete_voice(&mut inner, key, "timeout");
                    }
                }
            }
        });
        let interval_ms = self.deps.config.partial_interval_ms;
        if interval_ms > 0 {
            let timer = voice.partial_timer.clone();
            let me = Arc::downgrade(self);
            tokio::spawn(async move {
                let period = Duration::from_millis(interval_ms);
                let mut ticks = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
                loop {
                    tokio::select! {
                        _ = timer.cancelled() => return,
                        _ = ticks.tick() => {}
                    }
                    let Some(session) = me.upgrade() else { return };
                    let mut inner = session.lock();
                    if let Some(voice) = inner.voice(key) {
                        Self::send_partial(&session, voice);
                    }
                    session.recognize_interim(&mut inner, key);
                }
            });
        }
        let tag = voice.tag();
        inner.voice_requests.retain(|known| known.stream_id != stream_id);
        inner.voice_requests.push(voice);
        tracing::info!(
            target: "session",
            "Session {} voice {tag}: started on stream {stream_id}, activation {}",
            tag8(&self.id),
            activation.as_deref().unwrap_or("?")
        );
    }

    /// Interim result with the text recognised so far. Without them the station closes its stream with NO_SPEECH
    /// ~5 s after VoiceInput; empty ones do not stop that even mid-sentence, the text does.
    fn send_partial(&self, voice: &mut VoiceRequest) {
        voice.partials += 1;
        let message = ServerMessage::AsrResult {
            r#ref: voice.message_id.clone(),
            text: voice.interim.text().to_string(),
            end_of_utt: false,
        };
        self.send(&message, true);
    }

    /// Interim recognition is a decision of the speech side; the session runs it, one request at a time.
    fn interim_options(&self) -> InterimOptions {
        let config = &self.deps.config;
        InterimOptions {
            interval_ms: if self.deps.speech.recognition_enabled() { config.interim_interval_ms } else { 0 },
            quiet_speech: config.quiet_speech,
            quiet_end_ms: config.quiet_end_ms,
            unchanged_end_ms: config.unchanged_end_ms,
            stop_end_ms: config.stop_end_ms,
            guard_ms: self.deps.speech.config().endpoint.guard_ms,
        }
    }

    /// While the command is spoken, recognise it so far (or all audio after the wake word while VAD hears nothing).
    /// New text goes to the station at once; quiet speech that stopped changing ends the command.
    fn recognize_interim(self: &Arc<Self>, inner: &mut Inner, key: VoiceKey) {
        let Some(voice) = inner.voice(key) else { return };
        if voice.state != super::voice_request::VoiceState::Listening || voice.interim_abort.is_some() {
            return;
        }
        let Some(job) = voice.interim.next(&voice.speech) else { return };
        let number = self.next_serial.fetch_add(1, Ordering::SeqCst);
        let abort = voice.abort.child_token();
        voice.interim_abort = Some((number, abort.clone()));
        let extra = extra_wake_words(voice);
        let session = self.clone();
        tokio::spawn(async move {
            let recognized = session.deps.speech.recognize(&job.audio, &extra, &abort).await;
            let mut inner = session.lock();
            let Some(voice) = inner.voice(key) else { return };
            let listening = voice.state == super::voice_request::VoiceState::Listening;
            let mut end = None;
            match recognized {
                Ok(recognition) if listening => {
                    let stop = is_stop(&recognition.text);
                    let outcome = voice.interim.accept(&mut voice.speech, &job, &recognition.text, stop);
                    let source = if job.all_audio { "all audio " } else { "" };
                    let ended = outcome.end.map(|end| format!(", {}: end", end.describe())).unwrap_or_default();
                    let summary = format!(
                        "{} {source}{} at {}s of audio, {}ms{ended}",
                        voice.tag(),
                        heard(&recognition),
                        seconds(job.at_ms),
                        recognition.latency_ms
                    );
                    tracing::debug!(target: "session", "interim {summary}");
                    if outcome.changed {
                        session.send_partial(voice);
                        let text = voice.interim.text().to_string();
                        let message_id = voice.message_id.clone();
                        session.record(EventDirection::Note, "interimAsr", Some(summary), None, false);
                        if let Some(trip) = inner.trip(&message_id) {
                            trip.partial(&text);
                        }
                    } else {
                        session.record(EventDirection::Note, "interimAsr", Some(summary), None, false);
                    }
                    end = outcome.end;
                }
                Ok(_) => {}
                Err(error) => {
                    // Aborted by the final recognition or the end of the request: nothing to say.
                    if listening {
                        tracing::warn!(target: "session", "voice {}: interim recognition failed: {}", voice.tag(), describe(&error));
                    }
                }
            }
            if let Some(end) = end {
                session.complete_voice(&mut inner, key, end.outcome());
            }
            if let Some(voice) = inner.voice(key) {
                if voice.interim_abort.as_ref().is_some_and(|(running, _)| *running == number) {
                    voice.interim_abort = None;
                }
            }
        });
    }

    fn cancel_voice(self: &Arc<Self>, inner: &mut Inner, request_message_id: Option<&str>, reason: &str) {
        let keys: Vec<VoiceKey> = inner
            .voice_requests
            .iter()
            .filter(|voice| request_message_id.is_none_or(|r#ref| voice.message_id == r#ref))
            .map(|voice| VoiceKey { stream_id: voice.stream_id, serial: voice.serial })
            .collect();
        for key in keys {
            self.abort_voice(inner, key, reason);
        }
    }

    /// Listening is over (end of speech, no speech, the station closed its stream, safety timeout): recognise the
    /// command, then close the request in the order the station expects. Interim results keep going until the
    /// final AsrResult, so the station waits for recognition. The first stretch runs here, the rest in a task.
    fn complete_voice(self: &Arc<Self>, inner: &mut Inner, key: VoiceKey, outcome: &str) {
        let Some(voice) = inner.voice(key) else { return };
        if !voice.begin_recognition(outcome) {
            return;
        }
        // A station close can overtake audio that is still queued for VAD.
        self.flush_voice(inner, key);
        let enabled = self.deps.speech.recognition_enabled();
        if let Some(picked) = inner.voice(key).and_then(|voice| voice.speech.picked_gain_db()) {
            inner.last_gain_db = Some(picked);
            self.deps.speech.note_wake_word_gain(&self.device_tag, picked);
        }
        let Some(voice) = inner.voice(key) else { return };
        // Also after the station's own NO_SPEECH: it gives up on a timer, possibly mid-sentence. Quiet speech goes
        // to recognition whole: the VAD-cut audio would drop the quiet words.
        let audio = if voice.interim.quiet() { None } else { voice.speech.utterance_audio() };
        let message_id = voice.message_id.clone();
        if let Some(audio) = &audio {
            let file = self.save_recognition_audio(voice, "asr", audio);
            voice.utterance =
                Some(UtteranceView { from_ms: audio.from_ms, to_ms: audio.to_ms, gain_db: audio.gain_db, file });
            if enabled {
                voice.timeline.mark("asrStart");
            }
        }
        let cancel = voice.abort.clone();
        let extra = extra_wake_words(voice);
        if audio.is_some() && enabled {
            if let Some(trip) = inner.trip(&message_id) {
                trip.step(RequestPhase::Recognize, Some(Place::Own), self.ear_name().as_deref());
            }
        }
        let session = self.clone();
        let outcome = outcome.to_string();
        tokio::spawn(async move { session.recognize_voice(key, audio, extra, cancel, outcome).await });
    }

    /// The rest of `complete_voice`: recognition, then quiet speech's look at all the audio, then the answer.
    async fn recognize_voice(
        self: Arc<Self>,
        key: VoiceKey,
        audio: Option<UtteranceAudio>,
        extra: Vec<String>,
        cancel: CancellationToken,
        outcome: String,
    ) {
        let enabled = self.deps.speech.recognition_enabled();
        let mut failure: Option<anyhow::Error> = None;
        if let (Some(audio), true) = (&audio, enabled) {
            match self.deps.speech.recognize(audio, &extra, &cancel).await {
                Ok(recognition) => {
                    let mut inner = self.lock();
                    let Some(voice) = inner.voice(key) else { return };
                    voice.recognition = Some(recognition);
                    voice.timeline.mark("asrDone");
                }
                Err(error) => failure = Some(error),
            }
        }
        if failure.is_none() {
            let all = {
                let mut inner = self.lock();
                let Some(voice) = inner.voice(key) else { return };
                let heard_nothing = voice.recognition.as_ref().is_none_or(|recognition| recognition.text.is_empty());
                if heard_nothing && self.deps.config.quiet_speech && enabled {
                    self.prepare_all_audio(&mut inner, key, audio.as_ref())
                } else {
                    None
                }
            };
            if let Some((all, file)) = all {
                match self.deps.speech.recognize(&all, &extra, &cancel).await {
                    Ok(recognition) => {
                        let mut inner = self.lock();
                        let Some(voice) = inner.voice(key) else { return };
                        voice.all_audio = Some(AllAudio { recognition, from_ms: all.from_ms, to_ms: all.to_ms, file });
                        voice.timeline.mark("allAudioAsrDone");
                    }
                    Err(error) => failure = Some(error),
                }
            }
        }
        let mut inner = self.lock();
        let Some(voice) = inner.voice(key) else { return };
        if let Some(error) = failure {
            voice.error = Some(format!("recognition failed: {}", describe(&error)));
            tracing::warn!(target: "session", "voice {}: {}", voice.tag(), voice.error.as_deref().unwrap_or_default());
        }
        let recognition = match &voice.recognition {
            Some(recognition) if !recognition.text.is_empty() => Some(recognition.clone()),
            _ => voice.all_audio.as_ref().map(|all| all.recognition.clone()),
        };
        let restarted = recognition.as_ref().is_some_and(|recognition| recognition.restarted);
        let message_id = voice.message_id.clone();
        let raw = voice.raw;
        if restarted && raw {
            let kept = inner.request_frames.iter_mut().find(|kept| kept.message_id == message_id);
            if let Some(kept) = kept.filter(|kept| kept.audio.is_some()) {
                // The voice holds what came before the repeated wake word too: Yandex gets our text (live, 02.11:
                // «Алиса, стоп. Алиса, включи кино» as voice, Yandex heard «стоп»).
                kept.audio = None;
                let text = recognition.as_ref().map(|recognition| recognition.text.clone()).unwrap_or_default();
                self.note(
                    "voiceKept",
                    &format!("\"{text}\" follows a repeated wake word: Yandex gets the text, not the voice"),
                    None,
                );
            }
        }
        let text = recognition.map(|recognition| recognition.text).unwrap_or_default();
        self.answer_voice(&mut inner, key, &text);
        self.end_voice(&mut inner, key, &outcome);
    }

    /// Quiet speech, or no command in the VAD-cut audio: whispered or distant speech can sit below VAD's threshold
    /// even after the gain. All audio after the wake word goes to recognition; room noise comes back empty.
    fn prepare_all_audio(
        &self,
        inner: &mut Inner,
        key: VoiceKey,
        cut: Option<&UtteranceAudio>,
    ) -> Option<(UtteranceAudio, Option<String>)> {
        let voice = inner.voice(key)?;
        let audio = voice.speech.command_audio()?;
        if cut.is_some_and(|cut| audio.pcm.len() <= cut.pcm.len()) {
            return None;
        }
        let file = self.save_recognition_audio(voice, "asr-all", &audio);
        voice.timeline.mark("allAudioAsrStart");
        let message_id = voice.message_id.clone();
        if let Some(trip) = inner.trip(&message_id) {
            trip.step(RequestPhase::Recognize, Some(Place::Own), self.ear_name().as_deref());
        }
        Some((audio, file))
    }

    fn save_recognition_audio(&self, voice: &VoiceRequest, kind: &str, audio: &UtteranceAudio) -> Option<String> {
        let id = request_id_of(&voice.request).unwrap_or(&voice.message_id);
        let name = format!("{}_{id}.{kind}.wav", clock_tag());
        let file = self.journal.save_audio(&name, encode_wav(&audio.pcm, SPEECH_SAMPLE_RATE));
        file.map(|file| file.to_string_lossy().into_owned())
    }

    /// Final AsrResult, the stream close, spotter validation, then the spoken answer (its own timeline). With no
    /// command or no dialog the response is empty and the station goes idle.
    fn answer_voice(self: &Arc<Self>, inner: &mut Inner, key: VoiceKey, text: &str) {
        let Some(voice) = inner.voice(key) else { return };
        voice.stop_partials();
        let message_id = voice.message_id.clone();
        self.send(
            &ServerMessage::AsrResult { r#ref: message_id.clone(), text: text.to_string(), end_of_utt: true },
            false,
        );
        // The station keeps streaming until the server closes its input stream.
        if !voice.station_closed {
            self.send(&ServerMessage::StreamClose { stream_id: voice.stream_id, reason: CloseReason::Ok }, false);
        }
        if voice.request.enable_spotter_validation == Some(true) {
            self.send(&ServerMessage::SpotterValidation { r#ref: message_id.clone(), valid: true }, false);
        }
        let request = voice.request.clone();
        if !text.is_empty() {
            voice.timeline.mark("answerStart");
            // Heard only as quiet speech, or after a whispered wake word: a whisper gets a whisper back. Speech from
            // across the room is quiet too but keeps its pitch (live, 01.11: a distant «привет» was answered in a
            // whisper).
            let heard_whole = voice.recognition.as_ref().is_some_and(|recognition| !recognition.text.is_empty());
            let quiet = voice.interim.quiet()
                || (!heard_whole && voice.all_audio.as_ref().is_some_and(|all| !all.recognition.text.is_empty()));
            let whispered_wake = voice.speech.report().wake_word_dbfs.unwrap_or(0.0) <= WHISPERED_WAKE_DBFS;
            let mut whispered = false;
            if quiet || whispered_wake {
                let voicing = voice.speech.command_voicing();
                whispered = voicing.longest_run < VOICED_SPEECH_RUN;
                voice.voicing = Some(voicing);
            }
            if whispered {
                inner.whispered = Some(message_id.clone());
            }
            if let Some(trip) = inner.trip(&message_id) {
                trip.heard(text);
            }
            return self.handle_request(inner, &message_id, &request, text, false);
        }
        let error = voice.error.clone();
        if request.activation_type.as_deref() == Some("spotter")
            && self.deps.skills.ringing_now(&self.device_tag, request.device_state.as_ref(), now_ms())
        {
            // While its timer rings the station sends next to nothing (live, 02.11: «Алиса, стоп» at −70 dBFS, no
            // words): the wake word alone then means stop.
            voice.timeline.mark("answerStart");
            self.note("ringingStop", "no words while a timer or an alarm rings: taken for «стоп»", None);
            if let Some(trip) = inner.trip(&message_id) {
                trip.heard("Стоп.");
            }
            return self.handle_request(inner, &message_id, &request, "Стоп.", false);
        }
        self.send_empty_response(&message_id, &request, Vec::new());
        voice.timeline.mark("responseSent");
        // Nothing after the wake word (or nothing recognised): the request ends without a word.
        let status = if error.is_some() { RequestStatus::Failed } else { RequestStatus::Silent };
        self.end_trip(inner, &message_id, status, error.as_deref(), None);
    }

    /// The station listens after an early response whenever should_listen was set before the text was known. If the
    /// finished answer did not ask for a follow-up (question mode, no question at the end), that listening ends at
    /// once: an empty result, the stream closed, an empty response.
    fn follow_up_wanted(&self, inner: &Inner) -> bool {
        match &inner.follow_up {
            Some(FollowUp::Answer(answer)) => answer.expects_follow_up(),
            Some(FollowUp::Listens(listens)) => *listens,
            None => false,
        }
    }

    fn dismiss_follow_up(&self, inner: &mut Inner, r#ref: &str, request: &AliceRequest, stream_id: u32) {
        inner.follow_up = None;
        // Its audio may still come: dropped without an orphan-stream note.
        inner.orphan_streams.insert(stream_id);
        self.send(&ServerMessage::AsrResult { r#ref: r#ref.to_string(), text: String::new(), end_of_utt: true }, false);
        self.send(&ServerMessage::StreamClose { stream_id, reason: CloseReason::Ok }, false);
        self.send_empty_response(r#ref, request, Vec::new());
        self.note(
            "followUpDismissed",
            &format!("listening after an answer that did not ask for it: request {} closed", tag8(r#ref)),
            None,
        );
    }

    /// A response without speech: nothing (the station goes idle) or commands alone.
    fn send_empty_response(&self, r#ref: &str, request: &AliceRequest, directives: Vec<Directive>) {
        let response = crate::gateway::protocol::protocol_types::AliceResponseModel {
            request_id: request_id_of(request).map_or_else(new_message_id, str::to_string),
            response_id: new_message_id(),
            sequence_number: request.sequence_number,
            dialog_id: request.dialog_id.clone(),
            directives,
            ..Default::default()
        };
        self.send(&ServerMessage::AliceResponse { r#ref: r#ref.to_string(), response }, false);
    }

    // ---------------------------------------------------------------- requests: our skills, the model, Yandex or "not yet"

    /// A recognised or typed request goes where the router sends it; each decision is journaled. The emergency words
    /// go first, past everything (rescue.rs).
    fn handle_request(
        self: &Arc<Self>,
        inner: &mut Inner,
        r#ref: &str,
        request: &AliceRequest,
        text: &str,
        waited: bool,
    ) {
        if let Some(rescue) = super::rescue::rescue_of(text) {
            return self.rescue(inner, r#ref, request, text, rescue);
        }
        let router = self.deps.skills.router.clone();
        let input = self.route_input(inner, text, request);
        // Right after connecting a request can come before our session with Yandex is up (live 02.11: «Кто сейчас
        // играет?» 80 ms early got «Этого я пока не умею»): what Yandex would take waits for the opening session.
        if let (false, Some(yandex), false) = (waited, inner.yandex.clone(), input.yandex_available) {
            let with_yandex = router.decide(&RouteInput { yandex_available: true, ..input.clone() });
            let to_yandex = match &with_yandex {
                Decision::YandexDialog { .. } => true,
                Decision::Intent { resolution, .. } => *resolution == Resolution::Yandex,
                _ => false,
            };
            if to_yandex {
                let session = self.clone();
                let (r#ref, request, text) = (r#ref.to_string(), request.clone(), text.to_string());
                tokio::spawn(async move {
                    yandex.ready(YANDEX_OPENING_WAIT_MS).await;
                    let mut inner = session.lock();
                    session.handle_request(&mut inner, &r#ref, &request, &text, true);
                });
                return;
            }
        }
        let decision = router.decide(&input);
        // A new request let go of Yandex's dialog; an answer of Yandex's opens it again if it asks.
        if !matches!(decision, Decision::YandexDialog { .. })
            && inner.continuation.is_some_and(|continuation| continuation.executor == RouteStep::Yandex)
        {
            inner.continuation = None;
        }
        match decision {
            Decision::Intent { intent, source, rule, resolution } => {
                self.record_route(text, intent.capability, &resolution, source.as_str(), rule);
                if let Some(trip) = inner.trip(r#ref) {
                    trip.routed(Some(intent.capability.as_str()), resolution.kind());
                }
                self.serve_intent(inner, r#ref, request, text, intent, resolution);
            }
            Decision::YandexDialog { capability } => {
                self.record_route(text, capability, &Resolution::Yandex, "continuation", None);
                if let Some(trip) = inner.trip(r#ref) {
                    trip.routed(Some(capability.as_str()), "yandex");
                }
                self.ask_yandex(inner, r#ref, request, text, capability, false);
            }
            Decision::Model { command: chosen, profile } => {
                // «Поставь таймер» → «На сколько?» → «На две минуты»: the answer needs the command's tools.
                let command = chosen || inner.command_question_until > now_ms();
                inner.command_question_until = 0;
                let kind = if command { "a command: its choice first" } else { "a conversation" };
                let answering = if command && !chosen { ", answering its question" } else { "" };
                let profile_name = name_of(profile);
                self.record(
                    EventDirection::Note,
                    "route",
                    Some(format!("the model ({kind}{answering}, {profile_name}) \"{text}\"")),
                    Some(&json!({ "executor": "model", "command": command, "profile": profile_name, "text": text })),
                    false,
                );
                if let Some(trip) = inner.trip(r#ref) {
                    trip.routed(if command { None } else { Some("chat") }, "model");
                }
                if command {
                    return self.command_by_model(inner, r#ref, request, text, profile);
                }
                // A conversation with the model is a served chat request of ours; commands and tool calls count
                // where they land (record_route).
                self.deps.skills.stats.count(CapabilityName::Chat, Executor::Own);
                self.converse(inner, r#ref, request, text, profile);
            }
            Decision::Unsupported { capability, reason } => {
                let resolution = Resolution::Unsupported { reason: reason.clone() };
                self.record_route(text, capability, &resolution, "model", None);
                if let Some(trip) = inner.trip(r#ref) {
                    trip.routed(Some(capability.as_str()), "unsupported");
                }
                self.say_unsupported(inner, r#ref, request, text, capability, &reason);
            }
        }
    }

    /// The emergency words: said what happens, then — once the station has said it — done: the station given back to
    /// Yandex (its voice, a reboot) or the mode «Яндекс».
    fn rescue(
        self: &Arc<Self>,
        inner: &mut Inner,
        r#ref: &str,
        request: &AliceRequest,
        text: &str,
        rescue: super::rescue::Rescue,
    ) {
        use super::rescue::Rescue;
        let can = match rescue {
            Rescue::GiveBack => self.deps.rescue.is_some(),
            Rescue::YandexMode => true,
        };
        let speech = match (rescue, can) {
            (Rescue::GiveBack, true) => {
                "Аварийный возврат: отдаю колонку Яндексу и перезагружаю её. Через минуту она заговорит как обычная Алиса."
            }
            (Rescue::GiveBack, false) => "Отсюда вернуть колонку не получится: это делают в пульте — «Вернуть к Яндексу».",
            (Rescue::YandexMode, _) => "Перевожу шлюз в режим Яндекса: дальше отвечает Яндекс, как до шлюза. Вернуть — в пульте.",
        };
        self.record(
            EventDirection::Note,
            "rescue",
            Some(format!("{}: \"{text}\"{}", rescue.as_str(), if can { "" } else { " — no way to do it here" })),
            Some(&json!({ "rescue": rescue.as_str(), "text": text, "can": can })),
            false,
        );
        if let Some(trip) = inner.trip(r#ref) {
            trip.routed(Some("rescue"), "own");
        }
        let mut options = self.answer_options(inner, r#ref, request);
        options.listen = ListenMode::Never;
        let answer = self.start_answer(inner, options);
        let parts = self.with_errors(&answer, phrase_stream(split_phrases(speech)));
        let actor = Some(crate::core::events::Actor::from(&self.speaker(text)));
        let session = self.clone();
        let question = text.to_string();
        tokio::spawn(async move {
            let said = answer.clone();
            session.play_answer(answer, parts, question).await;
            if !can {
                return;
            }
            // The words are out before anything changes: the station plays them first, then a moment more.
            let left = said.playback_left_ms(now_ms());
            tokio::time::sleep(Duration::from_millis(left + RESCUE_AFTER_MS)).await;
            let done = match rescue {
                Rescue::GiveBack => {
                    let Some(giver) = session.deps.rescue.clone() else { return };
                    let address = session.remote_address.clone();
                    tokio::task::spawn_blocking(move || giver.give_back(&address, actor))
                        .await
                        .map_err(|error| error.to_string())
                        .and_then(|result| result)
                }
                Rescue::YandexMode => {
                    session.deps.modes.switch(Some(&json!("relay")), actor).map(|_| ()).map_err(|error| error.message)
                }
            };
            match done {
                Ok(()) => tracing::warn!(target: "session", "emergency words: {} done", rescue.as_str()),
                Err(error) => {
                    tracing::error!(target: "session", "emergency words: {} failed: {error}", rescue.as_str())
                }
            }
        });
    }

    fn speaker(&self, text: &str) -> Principal {
        self.deps.household.speaker(Some(&SpeakerHint { device_tag: self.device_tag.clone(), text: text.to_string() }))
    }

    fn route_input(&self, inner: &Inner, text: &str, request: &AliceRequest) -> RouteInput {
        let now = now_ms();
        // What the station holds now — its player, its timers — for the panel between requests.
        self.deps.skills.note_device_state(&self.device_tag, request.device_state.as_ref(), now);
        RouteInput {
            text: text.to_string(),
            device_tag: self.device_tag.clone(),
            device_state: request.device_state.clone(),
            continuation: inner.continuation,
            yandex_available: self.yandex_available(inner),
            ringing: self.deps.skills.ringing_now(&self.device_tag, request.device_state.as_ref(), now),
            wake_word: request.activation_type.as_deref() == Some("spotter"),
            principal: Some(self.speaker(text)),
            model: ModelState { enabled: self.deps.dialog.enabled(), tools: self.deps.dialog.tools_enabled() },
            now,
        }
    }

    fn serve_intent(
        self: &Arc<Self>,
        inner: &mut Inner,
        r#ref: &str,
        request: &AliceRequest,
        text: &str,
        intent: Intent,
        resolution: Resolution,
    ) {
        // «стоп» with nothing else to stop: the answers still to be said later are not wanted either.
        if intent.capability == CapabilityName::Playback && intent.action == "silence" {
            for answer in &inner.deferred {
                answer.cancel("«стоп»");
            }
        }
        match resolution {
            Resolution::Own(skill) => self.run_own(inner, r#ref, request, text, intent, skill),
            Resolution::Yandex => {
                // A phrase read as another command goes to Yandex as that command (live: «Что дальше?» → «Не
                // поняла», «Мьют» → a song called so), as text: the voice says what Yandex misreads.
                let said = intent.params.get("yandexText").and_then(Value::as_str).unwrap_or(text).to_string();
                let text_only = said != text;
                self.ask_yandex(inner, r#ref, request, &said, intent.capability, text_only);
            }
            Resolution::Unsupported { reason } => {
                self.say_unsupported(inner, r#ref, request, text, intent.capability, &reason)
            }
        }
    }

    /// Our implementation: its commands and its speech in one answer. One that sends the station no commands
    /// (weather, news) and is slow to speak answers early with the jingle, as a conversation does.
    fn run_own(
        self: &Arc<Self>,
        inner: &mut Inner,
        r#ref: &str,
        request: &AliceRequest,
        text: &str,
        intent: Intent,
        skill: Arc<dyn OwnSkill>,
    ) {
        let capability = intent.capability.info();
        if let Some(trip) = inner.trip(r#ref) {
            trip.step(RequestPhase::Think, Some(Place::Own), Some(capability.title));
        }
        let may_answer_early = !capability.station && self.deps.dialog.config.early_response;
        // tts_play_placeholder makes no sound on our station (live, with listen=false and true alike): the early
        // response only tells the station an answer is coming, so it does not ask to listen.
        let mut options = self.answer_options(inner, r#ref, request);
        options.listen = ListenMode::Never;
        let answer = self.start_answer(inner, options);
        let early = CancellationToken::new();
        if may_answer_early {
            let (early, answer) = (early.clone(), answer.clone());
            let wait = if capability.display { OWN_DISPLAY_EARLY_MS } else { OWN_EARLY_MS };
            tokio::spawn(async move {
                tokio::select! {
                    _ = early.cancelled() => {}
                    _ = tokio::time::sleep(Duration::from_millis(wait)) => answer.respond_early(),
                }
            });
        }
        let context = self.execution_context(inner, text, request, answer.token());
        let session = self.clone();
        let phrases_answer = answer.clone();
        let phrases_early = early.clone();
        let (request, text) = (request.clone(), text.to_string());
        let question = text.clone();
        let phrases: PhraseStream = generate(move |y| async move {
            let answer = phrases_answer;
            let mut outcome = match skill.execute(&intent, &context).await {
                Ok(outcome) => outcome,
                Err(error) => return y.send(Err(error)).await,
            };
            if may_answer_early && !outcome.directives.is_empty() {
                // The display's command joins the response, which goes at once: the model still has to word the
                // result. A response already out (a slow forecast) goes without it.
                let directives = std::mem::take(&mut outcome.directives);
                let names: Vec<String> = directives.iter().map(|directive| directive.name.clone()).collect();
                if answer.add_directives(directives).is_err() {
                    session.note(
                        "displayLate",
                        &format!("{}: answered early, {} not sent", intent.capability, names.join(", ")),
                        None,
                    );
                } else {
                    phrases_early.cancel();
                    answer.respond_early();
                }
            }
            let mut phrases = session.apply_outcome(&answer, &request, &text, outcome, None);
            while let Some(phrase) = phrases.next().await {
                // The first words are ready: the response goes with them (a clock's answer needs no jingle).
                phrases_early.cancel();
                y.send(phrase).await;
            }
        });
        let parts = self.with_errors(&answer, phrases);
        let session = self.clone();
        tokio::spawn(async move {
            session.play_answer(answer, parts, question).await;
            early.cancel();
        });
    }

    /// An outcome's commands join the response; then its speech, or, for a lookup's data, the model's words for it
    /// (`on_data`: the model that called the tool goes on; otherwise a new answer with the data as facts).
    fn apply_outcome(
        self: &Arc<Self>,
        answer: &Arc<SpokenAnswer>,
        request: &AliceRequest,
        text: &str,
        outcome: Outcome,
        on_data: Option<Box<dyn FnOnce(String) -> PhraseStream + Send>>,
    ) -> PhraseStream {
        let Outcome { speech, phrases, directives, listen, data, continuation, record_audio, done, on_spoken } =
            outcome;
        if let Some(on_spoken) = on_spoken {
            answer.after_spoken(on_spoken);
        }
        if !directives.is_empty() {
            if let Err(error) = answer.add_directives(directives) {
                return futures::stream::iter([Err(anyhow::anyhow!(error))]).boxed();
            }
        }
        if listen {
            if let Err(error) = answer.set_listen(ListenMode::Always) {
                return futures::stream::iter([Err(anyhow::anyhow!(error))]).boxed();
            }
        }
        if let Some(file) = record_audio {
            answer.record(Arc::new(FileRecording::new(file)));
        }
        let timezone = {
            let mut inner = self.lock();
            inner.continuation = continuation.map(|continuation| Continuation {
                capability: continuation.capability,
                executor: RouteStep::Own,
                until: now_ms() + continuation.ttl_ms,
            });
            self.timezone(&inner, request)
        };
        if let Some(phrases) = phrases {
            return phrases;
        }
        if let (Some(data), true) = (&data, self.deps.dialog.enabled()) {
            // The model puts the skill's data into words.
            let brain = self.brain_of(None);
            let trip = self.lock().trip(&answer.options.r#ref);
            if let Some(trip) = trip {
                trip.step(RequestPhase::Think, Some(brain.by), Some(&brain.what));
            }
            if let Some(on_data) = on_data {
                return on_data(data.clone());
            }
            let context = DialogContext {
                timezone,
                profile: Some(ModelProfile::Fast),
                facts: Some(data.clone()),
                system: None,
                model: None,
            };
            return self.deps.dialog.answer(&self.device_tag, text, context, answer.token(), None);
        }
        if let Some(speech) = speech {
            self.deps.dialog.remember(&self.device_tag, text, &speech, AnsweredBy::Skill, done.as_deref());
            return phrase_stream(split_phrases(&speech));
        }
        futures::stream::empty().boxed()
    }

    /// A command the quick rules do not know: the model picks a tool before anything is said.
    fn command_by_model(
        self: &Arc<Self>,
        inner: &mut Inner,
        r#ref: &str,
        request: &AliceRequest,
        text: &str,
        profile: ModelProfile,
    ) {
        let brain = self.brain_of(Some(profile));
        if let Some(trip) = inner.trip(r#ref) {
            trip.step(RequestPhase::Think, Some(brain.by), Some(&brain.what));
        }
        let options = self.answer_options(inner, r#ref, request);
        let answer = self.start_answer(inner, options);
        let phrases = self.model_command(inner, &answer, r#ref, request, text, profile);
        let parts = self.with_errors(&answer, phrases);
        self.spawn_answer(answer, parts, text.to_string());
    }

    fn model_command(
        self: &Arc<Self>,
        inner: &Inner,
        answer: &Arc<SpokenAnswer>,
        r#ref: &str,
        request: &AliceRequest,
        text: &str,
        profile: ModelProfile,
    ) -> PhraseStream {
        let context = DialogContext {
            timezone: self.timezone(inner, request),
            profile: Some(profile),
            facts: None,
            system: None,
            model: None,
        };
        let session = self.clone();
        let answer = answer.clone();
        let (r#ref, request, text) = (r#ref.to_string(), request.clone(), text.to_string());
        generate(move |y| async move {
            let principal = session.speaker(&text);
            let mcp = session.mcp_offer(&principal);
            let mut tools = tools_for(ToolsKind::Command, Some(&principal));
            if let Some(mcp) = &mcp {
                tools.extend(mcp.tools.iter().cloned());
            }
            let contributed = session.contributed_offer(&principal, &tools);
            if let Some(contributed) = &contributed {
                tools.extend(contributed.tools.iter().cloned());
            }
            let instructions = mcp.as_ref().map(|mcp| mcp.instructions.clone());
            let dialog = session.deps.dialog.clone();
            let choice =
                match dialog.decide(&session.device_tag, &text, context, answer.token(), tools, instructions).await {
                    Ok(choice) => choice,
                    Err(error) => return y.send(Err(error)).await,
                };
            let aside = choice.calls.iter().any(|made| {
                mcp.as_ref().is_some_and(|mcp| mcp.owns(&made.name))
                    || contributed.as_ref().is_some_and(|contributed| contributed.owns(&made.name))
            });
            if aside {
                // «Найди», «добавь в Todoist», a plugin's search: a connected server or a plugin answers, so this
                // goes on as a conversation — the jingle now, «Сейчас посмотрю» while it works, then the model's
                // words.
                if session.deps.dialog.config.early_response {
                    answer.respond_early();
                }
                let toolbox = session.lookup_toolbox(&r#ref, &request, &text, &answer, profile, Some(mcp.clone()));
                let Some(toolbox) = toolbox else { return };
                let mut phrases = choice.proceed(toolbox, answer.token());
                while let Some(phrase) = phrases.next().await {
                    y.send(phrase).await;
                }
                return;
            }
            let mut calls = choice.calls.clone().into_iter();
            let Some(call) = calls.next() else {
                if !choice.text.is_empty() {
                    session.deps.dialog.remember(&session.device_tag, &text, &choice.text, AnsweredBy::Model, None);
                }
                // A question about the command: its answer comes back here, with the tools.
                if ends_with_question(&choice.text) {
                    session.lock().command_question_until = now_ms() + COMMAND_QUESTION_MS;
                }
                for phrase in split_phrases(&choice.text) {
                    y.send(Ok(phrase)).await;
                }
                return;
            };
            let ignored: Vec<String> = calls.map(|call| call.name).collect();
            if !ignored.is_empty() {
                session.note(
                    "toolCallsIgnored",
                    &format!("one command per request: {} not run", ignored.join(", ")),
                    None,
                );
            }
            let found = match session.deps.skills.intent_of(&call) {
                Ok(found) => found,
                Err(error) => {
                    let result = ToolResult { id: call.id.clone(), content: format!("error: {error}"), is_error: true };
                    let mut phrases = choice.continue_with(vec![result], answer.token());
                    while let Some(phrase) = phrases.next().await {
                        y.send(phrase).await;
                    }
                    return;
                }
            };
            let (resolution, intent, context) = {
                let mut inner = session.lock();
                let router = session.deps.skills.router.clone();
                let intent = router.rewrite(&found, &session.route_input(&inner, &text, &request));
                let resolution = router.resolve(&intent, session.yandex_available(&inner), Some(&principal));
                session.record_route(&text, intent.capability, &resolution, "model", Some(&call.name));
                if let Some(trip) = inner.trip(&r#ref) {
                    trip.routed(Some(intent.capability.as_str()), resolution.kind());
                    if matches!(resolution, Resolution::Own(_)) {
                        trip.step(RequestPhase::Tool, Some(Place::Own), Some(intent.capability.info().title));
                    }
                }
                if resolution == Resolution::Yandex {
                    // Nothing was sent for this answer yet: it ends silently and Yandex answers the request.
                    answer.cancel("handed to Yandex");
                    session.ask_yandex(&mut inner, &r#ref, &request, &text, intent.capability, false);
                    return;
                }
                let context = session.execution_context(&inner, &text, &request, answer.token());
                (resolution, intent, context)
            };
            let skill = match resolution {
                Resolution::Own(skill) => skill,
                Resolution::Unsupported { reason } => {
                    session.note_unsupported(&text, intent.capability, &reason);
                    return y.send(Ok(session.unsupported_phrase(&reason))).await;
                }
                Resolution::Yandex => return,
            };
            let outcome = match skill.execute(&intent, &context).await {
                Ok(outcome) => outcome,
                Err(error) => return y.send(Err(error)).await,
            };
            let choice = Arc::new(choice);
            let continued = choice.clone();
            let token = answer.token();
            let id = call.id.clone();
            let on_data: Box<dyn FnOnce(String) -> PhraseStream + Send> = Box::new(move |data| {
                continued.continue_with(vec![ToolResult { id, content: data, is_error: false }], token)
            });
            let mut phrases = session.apply_outcome(&answer, &request, &text, outcome, Some(on_data));
            while let Some(phrase) = phrases.next().await {
                y.send(phrase).await;
            }
        })
    }

    /// A conversation: the response and the jingle at once, the model's words as they come, lookups as its tools.
    fn converse(
        self: &Arc<Self>,
        inner: &mut Inner,
        r#ref: &str,
        request: &AliceRequest,
        text: &str,
        profile: ModelProfile,
    ) {
        let brain = self.brain_of(Some(profile));
        if let Some(trip) = inner.trip(r#ref) {
            trip.step(RequestPhase::Think, Some(brain.by), Some(&brain.what));
        }
        let mut options = self.answer_options(inner, r#ref, request);
        options.early = self.deps.dialog.config.early_response;
        let answer = self.start_answer(inner, options);
        let context = DialogContext {
            timezone: self.timezone(inner, request),
            profile: Some(profile),
            facts: None,
            system: None,
            model: None,
        };
        let toolbox = self.lookup_toolbox(r#ref, request, text, &answer, profile, None);
        let phrases = self.deps.dialog.answer(&self.device_tag, text, context, answer.token(), toolbox);
        let parts = self.with_errors(&answer, phrases);
        self.spawn_answer(answer, parts, text.to_string());
    }

    /// The model's tools in a conversation: lookups (commands for the station are not among them) and the MCP
    /// servers' tools; a command that went to a server keeps its command tools (`from`: the command's offer). After
    /// a text from outside the house (a web page, the news) only the tools that change nothing stay offered.
    fn lookup_toolbox(
        self: &Arc<Self>,
        r#ref: &str,
        request: &AliceRequest,
        text: &str,
        answer: &Arc<SpokenAnswer>,
        profile: ModelProfile,
        from: Option<Option<Arc<McpOffer>>>,
    ) -> Option<ToolBox> {
        if !self.deps.dialog.tools_enabled() {
            return None;
        }
        let principal = self.speaker(text);
        let command = from.is_some();
        let mcp = match from {
            Some(mcp) => mcp,
            None => self.mcp_offer(&principal),
        };
        let mut tools = tools_for(if command { ToolsKind::Command } else { ToolsKind::Conversation }, Some(&principal));
        if let Some(mcp) = &mcp {
            tools.extend(mcp.tools.iter().cloned());
        }
        let contributed = self.contributed_offer(&principal, &tools);
        if let Some(contributed) = &contributed {
            tools.extend(contributed.tools.iter().cloned());
        }
        let session = self.clone();
        let (r#ref, request, text) = (r#ref.to_string(), request.clone(), text.to_string());
        let (tool_answer, tool_mcp, tool_contributed) = (answer.clone(), mcp.clone(), contributed.clone());
        let mut toolbox = ToolBox::new(tools, move |call: ToolCall, cancel: CancellationToken| {
            let session = session.clone();
            let (r#ref, request, text) = (r#ref.clone(), request.clone(), text.clone());
            let (answer, mcp, principal) = (tool_answer.clone(), tool_mcp.clone(), principal.clone());
            let contributed = tool_contributed.clone();
            async move {
                if let Some(offer) = contributed.filter(|offer| offer.owns(&call.name)) {
                    return Ok(session
                        .run_contributed(&r#ref, &answer, profile, &offer, &principal, &call, cancel)
                        .await);
                }
                session.run_tool(&r#ref, &request, &text, &answer, profile, mcp, &principal, call, cancel).await
            }
        });
        toolbox.filler = Some(LOOKUP_FILLER);
        if let Some(mcp) = &mcp {
            toolbox.rounds = Some(3);
            toolbox.instructions = Some(mcp.instructions.clone());
        } else if contributed.is_some() {
            toolbox.rounds = Some(3);
        }
        let safe_mcp = mcp.clone();
        let safe_contributed = contributed.clone();
        toolbox.safe_after_taint = Some(Arc::new(move |offered: &[crate::dialog::llm::LlmTool]| {
            offered
                .iter()
                .filter(|tool| {
                    SAFE_AFTER_OUTSIDE.contains(&tool.name.as_str())
                        || safe_mcp.as_ref().is_some_and(|mcp| mcp.owns(&tool.name) && mcp.safe(&tool.name))
                        || safe_contributed
                            .as_ref()
                            .is_some_and(|offer| offer.owns(&tool.name) && offer.safe(&tool.name))
                })
                .cloned()
                .collect()
        }));
        Some(toolbox)
    }

    /// One call of the model's in a conversation.
    #[allow(clippy::too_many_arguments)]
    async fn run_tool(
        self: &Arc<Self>,
        r#ref: &str,
        request: &AliceRequest,
        text: &str,
        answer: &Arc<SpokenAnswer>,
        profile: ModelProfile,
        mcp: Option<Arc<McpOffer>>,
        principal: &Principal,
        call: ToolCall,
        cancel: CancellationToken,
    ) -> anyhow::Result<ToolOutcome> {
        if let Some(mcp) = mcp.filter(|mcp| mcp.owns(&call.name)) {
            return self.run_mcp(r#ref, profile, &mcp, &call, &cancel).await;
        }
        let intent = match self.deps.skills.intent_of(&call) {
            Ok(intent) => intent,
            Err(error) => return Ok(ToolOutcome::error(format!("error: {error}"))),
        };
        let info = intent.capability.info();
        if info.station && answer.has_responded() {
            // The station's command goes only with the response, and this one is out already.
            return Ok(ToolOutcome::error(
                "error: this station command cannot run inside this answer; ask the user to say it again",
            ));
        }
        let brain = self.brain_of(Some(profile));
        let (resolution, trip, context) = {
            let inner = self.lock();
            let resolution = self.deps.skills.router.resolve(&intent, self.yandex_available(&inner), Some(principal));
            self.record_route(text, intent.capability, &resolution, "model", Some(&call.name));
            let trip = inner.trip(r#ref);
            let context = self.execution_context(&inner, text, request, cancel.clone());
            (resolution, trip, context)
        };
        match &resolution {
            Resolution::Unsupported { .. } => {}
            Resolution::Yandex => {
                if let Some(trip) = &trip {
                    trip.step(RequestPhase::Tool, Some(Place::Yandex), Some(info.title));
                }
            }
            Resolution::Own(_) => {
                if let Some(trip) = &trip {
                    trip.step(RequestPhase::Tool, Some(Place::Own), Some(info.title));
                }
            }
        }
        let skill = match resolution {
            Resolution::Unsupported { reason } => {
                self.note_unsupported(text, intent.capability, &reason);
                return Ok(ToolOutcome::from(if reason == RESTRICTED {
                    "Not allowed for this person: tell the user in one short sentence that this is not permitted."
                } else {
                    "Not available yet: tell the user in one short sentence that you cannot do this yet."
                }));
            }
            Resolution::Yandex => {
                let question = lookup_question(&call.name, &call.input, text);
                let said = self.ask_yandex_quietly(r#ref, request, &question, text, &cancel).await;
                if let Some(trip) = &trip {
                    trip.step(RequestPhase::Think, Some(brain.by), Some(&brain.what));
                }
                return Ok(ToolOutcome::from(said));
            }
            Resolution::Own(skill) => skill,
        };
        let outcome = skill.execute(&intent, &context).await?;
        if let Some(trip) = &trip {
            trip.step(RequestPhase::Think, Some(brain.by), Some(&brain.what));
        }
        if !outcome.directives.is_empty() {
            let names: Vec<String> = outcome.directives.iter().map(|directive| directive.name.clone()).collect();
            if answer.add_directives(outcome.directives.clone()).is_err() {
                // A conversation answers at once: a lookup's display command comes too late to join it.
                self.note(
                    "displayLate",
                    &format!("{}: the conversation answered at once, {} not sent", intent.capability, names.join(", ")),
                    None,
                );
            }
        }
        let content = outcome.data.or(outcome.speech).unwrap_or_else(|| "done".into());
        // Headlines are other people's text: after them the model is offered nothing that acts.
        Ok(if intent.capability == CapabilityName::News {
            ToolOutcome::tainted(content)
        } else {
            ToolOutcome::from(content)
        })
    }

    /// MCP servers' tools for this speaker: by voice only the servers open to the voice, by the routes of their
    /// capability.
    fn mcp_offer(&self, principal: &Principal) -> Option<Arc<McpOffer>> {
        let mcp = self.deps.mcp.as_ref().filter(|_| self.deps.dialog.tools_enabled())?;
        let routes = self.deps.skills.router.routes().describe();
        let allowed = |capability: CapabilityName| {
            routes.get(&capability).is_some_and(|route| route.split(',').any(|step| step == "own"))
        };
        mcp.offer(principal, OfferOptions { voice: true, capability_allowed: Some(&allowed) }).map(Arc::new)
    }

    /// The tools the connectors give (the plugins'), for this speaker: by the routes of their capability, as the
    /// MCP servers' are; a name offered already keeps its owner.
    fn contributed_offer(
        &self,
        principal: &Principal,
        taken: &[crate::dialog::llm::LlmTool],
    ) -> Option<Arc<ContributedOffer>> {
        let source = self.deps.tools.as_ref().filter(|_| self.deps.dialog.tools_enabled())?;
        let routes = self.deps.skills.router.routes().describe();
        let contributions = source
            .tools(principal)
            .into_iter()
            .filter(|contribution| {
                let capability = contribution.permission.strip_prefix("use:").and_then(is_capability);
                capability.is_none_or(|capability| {
                    routes.get(&capability).is_some_and(|route| route.split(',').any(|step| step == "own"))
                })
            })
            .collect();
        ContributedOffer::new(contributions, taken).map(Arc::new)
    }

    /// One call of a connector's tool (a plugin's): the trip shows it, never its arguments.
    #[allow(clippy::too_many_arguments)]
    async fn run_contributed(
        &self,
        r#ref: &str,
        answer: &Arc<SpokenAnswer>,
        profile: ModelProfile,
        offer: &ContributedOffer,
        principal: &Principal,
        call: &ToolCall,
        cancel: CancellationToken,
    ) -> ToolOutcome {
        let trip = self.lock().trip(r#ref);
        if let Some(trip) = &trip {
            trip.step(RequestPhase::Tool, Some(Place::Own), Some(call.name.as_str()));
        }
        // A plugin's tool that acts (plays, speaks) adds its commands to this answer.
        let station = {
            let answer = answer.clone();
            crate::core::connectors::StationAct {
                device: self.device_tag.clone(),
                media_base_url: self.deps.skills.media_base_url(self.host.as_deref()),
                add: Arc::new(move |directives| answer.add_directives(directives).is_ok()),
            }
        };
        let outcome = offer.run_for(call, principal, cancel, Some(station)).await;
        let failed = if outcome.is_error { " failed" } else { "" };
        self.note("tool", &format!("{}{failed}", call.name), None);
        let brain = self.brain_of(Some(profile));
        if let Some(trip) = &trip {
            trip.step(RequestPhase::Think, Some(brain.by), Some(&brain.what));
        }
        outcome
    }

    /// One MCP call: the trip shows which server works and whether it is outside the house; never the arguments.
    async fn run_mcp(
        &self,
        r#ref: &str,
        profile: ModelProfile,
        mcp: &McpOffer,
        call: &ToolCall,
        cancel: &CancellationToken,
    ) -> anyhow::Result<ToolOutcome> {
        let server = mcp.server_of(&call.name);
        let trip = self.lock().trip(r#ref);
        if let Some(trip) = &trip {
            let by = if server.as_ref().is_some_and(|server| server.cloud) { Place::Cloud } else { Place::Own };
            trip.step(RequestPhase::Tool, Some(by), server.as_ref().map(|server| server.title.as_str()));
        }
        let outcome = mcp.run(call, cancel).await.map_err(|cancelled| anyhow::anyhow!("{cancelled}"))?;
        let title = server.as_ref().map_or(call.name.as_str(), |server| server.title.as_str());
        let failed = if outcome.is_error { " failed" } else { "" };
        self.note("mcp", &format!("{title}: {}{failed}", call.name), None);
        let brain = self.brain_of(Some(profile));
        if let Some(trip) = &trip {
            trip.step(RequestPhase::Think, Some(brain.by), Some(&brain.what));
        }
        Ok(outcome)
    }

    fn say_unsupported(
        self: &Arc<Self>,
        inner: &mut Inner,
        r#ref: &str,
        request: &AliceRequest,
        text: &str,
        capability: CapabilityName,
        reason: &str,
    ) {
        self.note_unsupported(text, capability, reason);
        if let Some(trip) = inner.trip(r#ref) {
            trip.routed(Some(capability.as_str()), "unsupported");
            trip.step(RequestPhase::Think, Some(Place::Own), None);
        }
        let mut options = self.answer_options(inner, r#ref, request);
        options.listen = ListenMode::Never;
        let answer = self.start_answer(inner, options);
        self.spawn_answer(answer, single(self.unsupported_phrase(reason)), text.to_string());
    }

    /// What the station says when nobody serves a request: not yet, or not for this person.
    fn unsupported_phrase(&self, reason: &str) -> String {
        if reason == RESTRICTED {
            RESTRICTED_PHRASE.to_string()
        } else {
            self.deps.skills.config.unsupported_phrase.clone()
        }
    }

    /// The provider in force for this answer: the panel's choice, with «Свой» keeping Alice's voice out.
    fn tts_provider(&self) -> TtsProvider {
        self.deps.voice.provider(self.deps.modes.yandex_allowed())
    }

    fn note_unsupported(&self, text: &str, capability: CapabilityName, reason: &str) {
        let route = self.deps.skills.router.routes().describe_one(capability);
        let entry = self.deps.skills.stats.unsupported(Unsupported {
            device: self.device_tag.clone(),
            capability,
            text: text.to_string(),
            route: route.clone(),
            reason: reason.to_string(),
        });
        self.note(
            "unsupported",
            &format!("{capability} \"{text}\": {reason} (route {route})"),
            serde_json::to_value(entry).ok(),
        );
    }

    fn record_route(
        &self,
        text: &str,
        capability: CapabilityName,
        resolution: &Resolution,
        source: &str,
        detail: Option<&str>,
    ) {
        match resolution {
            Resolution::Own(_) => self.deps.skills.stats.count(capability, Executor::Own),
            Resolution::Yandex => self.deps.skills.stats.count(capability, Executor::Yandex),
            Resolution::Unsupported { .. } => {}
        }
        let target = match resolution.reason() {
            Some(reason) => format!("not yet ({reason})"),
            None => resolution.kind().to_string(),
        };
        let detail_part =
            detail.filter(|detail| !detail.is_empty()).map(|detail| format!(" {detail}")).unwrap_or_default();
        let summary = format!("{capability} → {target}, {source}{detail_part}: \"{text}\"");
        tracing::info!(target: "session", "Session {} route {summary}", tag8(&self.id));
        let mut data =
            json!({ "capability": capability, "executor": resolution.kind(), "source": source, "text": text });
        if let Some(detail) = detail {
            data["detail"] = json!(detail);
        }
        if let Some(reason) = resolution.reason() {
            data["reason"] = json!(reason);
        }
        self.record(EventDirection::Note, "route", Some(summary), Some(&data), false);
    }

    fn with_errors(&self, answer: &Arc<SpokenAnswer>, phrases: PhraseStream) -> AnswerParts {
        let failed = answer.clone();
        with_error_phrase(phrases, answer.token(), move |error| {
            failed.push_error(format!("dialog: {}", describe(&error)))
        })
        .map(AnswerPart::Text)
        .boxed()
    }

    fn answer_options(&self, inner: &Inner, r#ref: &str, request: &AliceRequest) -> SpokenAnswerOptions {
        let config = &self.deps.dialog.config;
        SpokenAnswerOptions {
            r#ref: r#ref.to_string(),
            response: Some(ResponseHeader {
                request_id: request_id_of(request).map_or_else(new_message_id, str::to_string),
                sequence_number: request.sequence_number,
                dialog_id: request.dialog_id.clone(),
            }),
            hold_response_ms: config.hold_response_ms,
            listen: config.listen,
            early: false,
            jingle: config.jingle,
            directives: Vec::new(),
            whisper: config.whisper_reply && inner.whispered.as_deref() == Some(r#ref),
            // Yandex's markup means nothing to other engines.
            markup: self.deps.voice.markup(self.deps.modes.yandex_allowed()),
            wait_max_ms: self.deps.config.answer_wait_max_ms,
            deferrable: true,
        }
    }

    fn execution_context(
        &self,
        inner: &Inner,
        text: &str,
        request: &AliceRequest,
        cancel: CancellationToken,
    ) -> ExecutionContext {
        ExecutionContext {
            device_tag: self.device_tag.clone(),
            text: text.to_string(),
            timezone: self.timezone(inner, request),
            device_state: request.device_state.clone(),
            media_base_url: self.deps.skills.media_base_url(self.host.as_deref()),
            now: chrono::Utc::now(),
            cancel,
        }
    }

    fn timezone(&self, inner: &Inner, request: &AliceRequest) -> Option<String> {
        request
            .application
            .as_ref()
            .and_then(|application| application.timezone.clone())
            .or_else(|| inner.client_info.as_ref().and_then(|info| info.timezone.clone()))
    }

    // ---------------------------------------------------------------- the Yandex route

    /// Whether requests can go to Yandex now: our session with it is up.
    fn yandex_available(&self, inner: &Inner) -> bool {
        self.deps.modes.yandex_allowed() && inner.yandex.as_ref().is_some_and(|yandex| yandex.connected())
    }

    /// Pushes, speech and the Yandex route need our session; it opens with the station's SynchronizeState.
    fn open_yandex(&self, inner: &mut Inner) {
        let Some(synchronize_state) = inner.synchronize_state.clone() else { return };
        if inner.yandex.is_some() || !self.deps.modes.yandex_allowed() {
            return;
        }
        let config = &self.deps.config;
        let routes = self.deps.skills.router.routes().describe();
        let needed = self.tts_provider() == TtsProvider::Yandex
            || config.yandex_pushes_pass
            || routes.values().any(|route| route.contains("yandex"));
        if !needed {
            return;
        }
        let link = self.link();
        let yandex = YandexSession::new(
            YandexSessionOptions {
                url: config.yandex_url.clone(),
                tts: config.tts.clone(),
                ping_ms: config.yandex_ping_ms,
                reconnect_ms: vec![1000, 2000, 5000, 10_000, 30_000],
            },
            StationCredentials { headers: self.handshake_headers.clone(), synchronize_state },
            self.deps.codec.clone(),
            link.clone(),
            link,
        );
        inner.yandex = Some(yandex.clone());
        yandex.start();
    }

    /// A push from Yandex (a reminder, a call, a notification): to the station as it is (YANDEX_PUSHES=pass).
    fn on_yandex_push(&self, frame: Bytes, kind: &str, decoded: &Value) {
        if !self.deps.config.yandex_pushes_pass {
            let summary = format!("{kind} from Yandex (YANDEX_PUSHES=drop)");
            return self.record(EventDirection::Note, "pushDropped", Some(summary), None, false);
        }
        let mut inner = self.lock();
        let header = decoded.get("event").and_then(|event| event.get("header"));
        if let Some(message_id) = header.and_then(|header| header.get("messageId")).and_then(Value::as_str) {
            if !message_id.is_empty() {
                Self::owned_by_yandex(&mut inner, message_id);
            }
        }
        let yandex_stream = header
            .and_then(|header| header.get("streamId"))
            .and_then(Value::as_u64)
            .filter(|&id| id != 0)
            .and_then(|id| u32::try_from(id).ok());
        if let Some(stream) = yandex_stream {
            if inner.push_stream(stream).is_none() {
                let ours = self.new_stream_id();
                inner.push_streams.push((stream, ours));
            }
        }
        let edited = match yandex_stream {
            None => Ok(frame.clone()),
            Some(_) => {
                let map = |id: u32| inner.push_stream(id);
                edit_frame(&frame, &FrameEdit { stream_id: Some(&map), ..Default::default() }).map(Bytes::from)
            }
        };
        match edited {
            Ok(edited) => self.send_raw(edited, kind, "push from Yandex".into(), decoded),
            Err(error) => self.note("yandexFrameError", &error.to_string(), None),
        }
    }

    fn on_yandex_stray(&self, frame: Bytes, stream_id: u32, is_control: bool) {
        let mut inner = self.lock();
        let Some(ours) = inner.push_stream(stream_id) else {
            // The station's own stream, given to Yandex as it was: what Yandex says on it is the station's, as in
            // «Яндекс». Its numbering is the station's (odd, from 1 on a new socket); Yandex's own streams are even.
            if inner.station_streams_at_yandex.contains(&stream_id) {
                if !is_control {
                    self.outbox.send_binary(frame);
                    return;
                }
                let summary = format!("stream={stream_id}: the station's own, from Yandex");
                return self.send_raw(frame, "streamControl", summary, &json!({}));
            }
            if !inner.stray_stream_noted {
                let what = if is_control { "a stream control" } else { "audio" };
                self.note(
                    "strayYandexStream",
                    &format!("Yandex stream {stream_id} ({what}) belongs to nothing of ours: dropped (noted once)"),
                    None,
                );
            }
            inner.stray_stream_noted = true;
            return;
        };
        if !is_control {
            if let Ok(audio) = restream(&frame, ours) {
                self.outbox.send_binary(Bytes::from(audio));
            }
            return;
        }
        let map = |id: u32| inner.push_stream(id);
        if let Ok(edited) = edit_frame(&frame, &FrameEdit { stream_id: Some(&map), ..Default::default() }) {
            let summary = format!("stream={ours} (Yandex {stream_id})");
            self.send_raw(Bytes::from(edited), "streamControl", summary, &json!({}));
        }
        inner.push_streams.retain(|(theirs, _)| *theirs != stream_id);
    }

    fn owned_by_yandex(inner: &mut Inner, message_id: &str) {
        if !inner.yandex_ids.iter().any(|known| known == message_id) {
            inner.yandex_ids.push(message_id.to_string());
        }
        while inner.yandex_ids.len() > KEPT_YANDEX_IDS {
            inner.yandex_ids.remove(0);
        }
    }

    /// A station's stream goes to Yandex as it is from here on.
    fn forward_stream(inner: &mut Inner, stream_id: u32) {
        inner.forwarded_streams.insert(stream_id);
        inner.station_streams_at_yandex.retain(|&known| known != stream_id);
        inner.station_streams_at_yandex.push_back(stream_id);
        while inner.station_streams_at_yandex.len() > KEPT_STATION_STREAMS {
            inner.station_streams_at_yandex.pop_front();
        }
    }

    /// The station's voice, nothing here to recognise it: to Yandex whole, voice and all, as in «Яндекс», and
    /// followed as any request — Yandex hears it (its AsrResult is the request's words) and answers it. Before, the
    /// request was not followed and went nowhere in the journal or on the panel (live, 08.11).
    ///
    /// A station that reconnects and speaks at once is ahead of our session with Yandex, which its SynchronizeState
    /// only began to open (live, 08.11: the voice 77 ms after the socket). Its voice waits for the session, as a
    /// callback does; before, the request was cancelled at once and the station fell silent, while its audio still
    /// went to Yandex once the session was up, on a stream Yandex never heard opened.
    fn voice_to_yandex(self: &Arc<Self>, inner: &mut Inner, header: EventHeader, request: AliceRequest, frame: Bytes) {
        if inner.held_voice.is_some() {
            self.drop_held_voice(inner, RequestStatus::Cancelled, "superseded");
        }
        let message_id = header.message_id.clone();
        if let Some(stream_id) = header.stream_id {
            Self::forward_stream(inner, stream_id);
        }
        if let Some(trip) = self.begin_trip(inner, &message_id, RequestKind::Voice, None) {
            trip.step(RequestPhase::Listen, Some(Place::Yandex), Some("Алиса"));
            trip.routed(None, "yandex");
        }
        let Some(yandex) = inner.yandex.clone() else { return };
        if yandex.connected() {
            return self.voice_relayed(inner, &message_id, vec![frame]);
        }
        // Between its reconnects, it opens now.
        yandex.warm_up();
        inner.held_voice = Some(HeldVoice {
            message_id: message_id.clone(),
            stream_id: header.stream_id,
            request,
            frames: vec![frame],
        });
        self.note("voiceHeld", &format!("request {}: waits for our session with Yandex", tag8(&message_id)), None);
        let session = self.clone();
        tokio::spawn(async move {
            let ready = yandex.ready(YANDEX_OPENING_WAIT_MS).await;
            let mut inner = session.lock();
            let Some(held) = inner.held_voice.take() else { return };
            if held.message_id != message_id {
                inner.held_voice = Some(held);
                return;
            }
            if !ready {
                inner.held_voice = Some(held);
                return session.drop_held_voice(&mut inner, RequestStatus::Failed, "Yandex is not reachable");
            }
            session.voice_relayed(&mut inner, &held.message_id, held.frames);
        });
    }

    fn voice_relayed(self: &Arc<Self>, inner: &mut Inner, message_id: &str, frames: Vec<Bytes>) {
        self.relay_yandex(
            inner,
            None,
            message_id,
            frames,
            CapabilityName::Chat,
            "no recognition here",
            NO_EVENTS,
            "voiceInput",
            false,
        );
    }

    /// A held voice request that will not go to Yandex: its stream stops going there, its trip ends, and the station,
    /// unless it cancelled it itself, stops listening and waiting (an empty result, its stream closed, an empty
    /// response).
    fn drop_held_voice(&self, inner: &mut Inner, status: RequestStatus, reason: &str) {
        let Some(held) = inner.held_voice.take() else { return };
        if let Some(stream_id) = held.stream_id {
            inner.forwarded_streams.remove(&stream_id);
            inner.orphan_streams.insert(stream_id);
        }
        if status == RequestStatus::Failed {
            let r#ref = held.message_id.clone();
            self.send(&ServerMessage::AsrResult { r#ref: r#ref.clone(), text: String::new(), end_of_utt: true }, false);
            if let Some(stream_id) = held.stream_id {
                self.send(&ServerMessage::StreamClose { stream_id, reason: CloseReason::Ok }, false);
            }
            self.send_empty_response(&r#ref, &held.request, Vec::new());
        }
        self.note("voiceDropped", &format!("request {}: {reason}", tag8(&held.message_id)), None);
        self.end_trip(inner, &held.message_id, status, Some(reason), None);
    }

    /// The request goes to Yandex: as text (the station's own request with our recognised text, the voice stays
    /// here) or as the station's voice: for capabilities in YANDEX_VOICE, and for a whisper, which Yandex answers in
    /// a whisper only when it hears one (live, 02.11: «Включаю Miyagi» whispered back to a whispered request).
    /// Yandex's answer is relayed.
    fn ask_yandex(
        self: &Arc<Self>,
        inner: &mut Inner,
        r#ref: &str,
        request: &AliceRequest,
        text: &str,
        capability: CapabilityName,
        text_only: bool,
    ) {
        if !self.yandex_available(inner) {
            return self.say_unsupported(inner, r#ref, request, text, capability, "Yandex is not reachable");
        }
        if let Some(trip) = inner.trip(r#ref) {
            trip.step(RequestPhase::Think, Some(Place::Yandex), Some(capability.info().title));
        }
        let message_id = new_message_id();
        let whisper = self.deps.dialog.config.whisper_reply && inner.whispered.as_deref() == Some(r#ref);
        let as_voice = !text_only && (self.deps.skills.config.yandex_voice.contains(&capability) || whisper);
        let voice = match inner.request_frame(r#ref) {
            Some(kept) if kept.audio.is_some() && as_voice => self.voice_frames(kept, &message_id).ok(),
            _ => None,
        };
        if voice.is_some() && whisper {
            self.note(
                "whisperToYandex",
                &format!("\"{text}\" was whispered: Yandex gets the voice, to whisper back"),
                None,
            );
        }
        let label = if voice.is_some() { "voiceInput" } else { "textInput" };
        let frames = match voice {
            Some(frames) => frames,
            None => match self.text_frame(inner, r#ref, &message_id, request, text, None) {
                Ok(frame) => vec![frame],
                Err(error) => return self.note("frameError", &format!("{error:#}"), None),
            },
        };
        self.relay_yandex(inner, Some(r#ref), &message_id, frames, capability, text, RECOGNITION_EVENTS, label, false);
    }

    /// Our recognised text in the station's own request, or, without one, in a request of our own.
    fn text_frame(
        &self,
        inner: &Inner,
        r#ref: &str,
        message_id: &str,
        request: &AliceRequest,
        text: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<Bytes> {
        let frame = match inner.request_frame(r#ref) {
            Some(kept) if kept.kind == FrameKind::VoiceInput => {
                voice_to_text_input(&kept.frame, message_id, text, request_id)?
            }
            Some(kept) => retext_text_input(&kept.frame, message_id, text, request_id)?,
            None => {
                let application = inner.client_info.as_ref().map_or_else(|| json!({}), |info| json!(info));
                let request_id = request_id
                    .map(str::to_string)
                    .or_else(|| request.request_id.clone())
                    .unwrap_or_else(new_message_id);
                let lang = request
                    .lang
                    .clone()
                    .or_else(|| inner.client_info.as_ref().and_then(|info| info.lang.clone()))
                    .unwrap_or_else(|| "ru-RU".into());
                let object = json!({ "event": {
                    "header": { "messageId": message_id },
                    "textInput": {
                        "header": { "requestId": request_id, "sequenceNumber": request.sequence_number.unwrap_or(0) },
                        "application": application,
                        "request": { "event": { "type": "text_input", "text": text }, "voiceSession": true },
                        "lang": lang,
                    },
                } });
                return self.deps.codec.encode_client_object(&object).map_err(|error| anyhow::anyhow!("{error}"));
            }
        };
        Ok(Bytes::from(frame))
    }

    /// The station's VoiceInput (another message id) and its audio, the wake word's end marked where it was.
    fn voice_frames(&self, kept: &RequestFrame, message_id: &str) -> anyhow::Result<Vec<Bytes>> {
        let stream_id = kept.stream_id.unwrap_or(1);
        let prefix = stream_id.to_be_bytes();
        let control = |body: Value| -> anyhow::Result<Bytes> {
            let mut control = json!({ "streamId": stream_id, "messageId": new_message_id() });
            if let (Value::Object(control), Value::Object(body)) = (&mut control, body) {
                control.extend(body);
            }
            self.deps
                .codec
                .encode_client_object(&json!({ "streamControl": control }))
                .map_err(|error| anyhow::anyhow!("{error}"))
        };
        let chunk = |chunk: &Bytes| Bytes::from([&prefix[..], chunk].concat());
        let chunks = kept.audio.as_deref().unwrap_or_default();
        let spotter_end = kept.spotter_end_chunk.unwrap_or(0).min(chunks.len());
        let mut frames = vec![Bytes::from(edit_frame(
            &kept.frame,
            &FrameEdit { message_id: Some(message_id), ..Default::default() },
        )?)];
        frames.extend(chunks[..spotter_end].iter().map(chunk));
        if kept.spotter_end_chunk.is_some() {
            frames.push(control(json!({ "spotterEnd": {} }))?);
        }
        frames.extend(chunks[spotter_end..].iter().map(chunk));
        frames.push(control(json!({ "close": { "reason": "CLOSE_OK" } }))?);
        Ok(frames)
    }

    /// Relays Yandex's answer to a request of ours (`station_ref`: the station's message id it answers) or to the
    /// station's own frame (`station_ref` None: the ids already match). The station hears Yandex's answer;
    /// should_listen and the dialog decide where follow-ups go.
    #[allow(clippy::too_many_arguments)]
    fn relay_yandex(
        self: &Arc<Self>,
        inner: &mut Inner,
        station_ref: Option<&str>,
        message_id: &str,
        frames: Vec<Bytes>,
        capability: CapabilityName,
        text: &str,
        drop: &'static [&'static str],
        label: &str,
        player: bool,
    ) {
        let yandex = inner.yandex.clone();
        // The player's own callbacks (a track started, the next one wanted) go beside the dialog: live, the next
        // track's request cancelled our answer while it was being synthesized.
        if !player {
            if let Some(answer) = &inner.answer {
                answer.cancel("superseded");
            }
            if let Some(relay) = &inner.yandex_relay {
                relay.cancel("superseded");
            }
        }
        let r#ref = station_ref.unwrap_or(message_id).to_string();
        // The request's trip, when one is followed: the player's own callbacks are not requests.
        let trip = if player { None } else { inner.trip(&r#ref) };
        let revoicing = self.tts_provider() == TtsProvider::Own && self.deps.voice.speaker().is_some();
        let mut options = YandexRelayOptions::new(drop, self.deps.config.yandex_timeout_ms);
        options.station_ref = station_ref.map(str::to_string);
        if revoicing {
            let me = self.me.clone();
            let r#ref = r#ref.clone();
            options.revoice = Some(Box::new(move |speech: String| {
                if let Some(session) = me.upgrade() {
                    let mut inner = session.lock();
                    session.revoice(&mut inner, &r#ref, &speech);
                }
            }));
        }
        if !player {
            let me = self.me.clone();
            let trip = trip.clone();
            options.on_response = Some(Box::new(move |answer: &YandexAnswer| {
                if let Some(session) = me.upgrade() {
                    let mut inner = session.lock();
                    session.follow_yandex(&mut inner, answer, capability);
                }
                if let Some(trip) = &trip {
                    if !answer.text.is_empty() {
                        trip.answered(&answer.text);
                    }
                    // Alice speaks it; with our voice instead, our answer says when it speaks (revoice).
                    if answer.has_voice && !revoicing {
                        trip.step(RequestPhase::Speak, Some(Place::Yandex), Some("Алиса"));
                    }
                }
            }));
        }
        // Yandex hears what nothing here did: its recognition is the request's words.
        if let Some(trip) = trip.clone().filter(|_| !drop.contains(&"asrResult")) {
            options.on_asr = Some(Box::new(move |asr_result: &Value| yandex_heard(&trip, Some(asr_result))));
        }
        let relay = YandexRelay::new(options, self.link());
        if let Some(trip) = &trip {
            trip.claim(relay.owner);
        }
        if player {
            inner.player_relays.retain(|(id, _)| id != message_id);
            inner.player_relays.push((message_id.to_string(), relay.clone()));
        } else {
            inner.yandex_relay = Some(relay.clone());
        }
        if station_ref.is_none() {
            Self::owned_by_yandex(inner, message_id);
        }
        let started = Instant::now();
        let exchange: Option<Arc<Exchange>> = match &yandex {
            None => {
                relay.cancel("no session with Yandex");
                None
            }
            Some(yandex) => match yandex.exchange(message_id, frames, relay.clone(), label) {
                Ok(exchange) => Some(exchange),
                Err(error) => {
                    relay.cancel(&describe(&error));
                    None
                }
            },
        };
        let session = self.clone();
        let (message_id, text) = (message_id.to_string(), text.to_string());
        tokio::spawn(async move {
            let answer = relay.done().await;
            let mut inner = session.lock();
            if let (Some(exchange), Some(yandex)) = (exchange, &yandex) {
                yandex.end_exchange(exchange, 3000);
            }
            if inner.yandex_relay.as_ref().is_some_and(|current| Arc::ptr_eq(current, &relay)) {
                inner.yandex_relay = None;
            }
            inner.player_relays.retain(|(id, kept)| !(id == &message_id && Arc::ptr_eq(kept, &relay)));
            if let Some(trip) = &trip {
                if !answer.text.is_empty() {
                    trip.answered(&answer.text);
                }
                let status = match answer.outcome {
                    RelayOutcome::Cancelled => RequestStatus::Cancelled,
                    RelayOutcome::Answered if answer.responded => RequestStatus::Answered,
                    _ => RequestStatus::Failed,
                };
                let error = if status == RequestStatus::Answered { None } else { answer.error.as_deref() };
                session.end_trip(&mut inner, &r#ref, status, error, Some(relay.owner));
            }
            let ms = started.elapsed().as_millis() as u64;
            session.after_yandex_answer(&mut inner, &answer, &r#ref, capability, &text, ms);
        });
    }

    fn after_yandex_answer(
        self: &Arc<Self>,
        inner: &mut Inner,
        answer: &YandexAnswer,
        r#ref: &str,
        capability: CapabilityName,
        text: &str,
        ms: u64,
    ) {
        let directives = if answer.directives.is_empty() { "-".to_string() } else { answer.directives.join(",") };
        let mut parts = vec![
            format!("listen={}", answer.should_listen),
            format!("directives={directives}"),
            format!("streams {}", answer.streams),
            format!("{ms}ms"),
        ];
        if let Some(heard) = &answer.heard {
            parts.push(format!("heard \"{heard}\""));
        }
        if let Some(error) = &answer.error {
            parts.push(error.clone());
        }
        let outcome = answer.outcome.as_str();
        let summary = format!("{outcome} \"{}\" for \"{text}\" | {}", answer.text, parts.join(", "));
        if matches!(answer.outcome, RelayOutcome::Answered | RelayOutcome::Cancelled) {
            tracing::info!(target: "session", "Session {} yandex answer {}: {summary}", tag8(&self.id), tag8(r#ref));
        } else {
            tracing::warn!(target: "session", "Session {} yandex answer {}: {summary}", tag8(&self.id), tag8(r#ref));
        }
        let mut data = json!({ "ref": r#ref, "capability": capability, "text": text });
        if let (Value::Object(data), Ok(Value::Object(fields))) = (&mut data, serde_json::to_value(answer)) {
            data.extend(fields);
        }
        data["ms"] = json!(ms);
        self.record(
            EventDirection::Note,
            "yandexAnswer",
            Some(format!("{} {summary}", tag8(r#ref))),
            Some(&data),
            false,
        );
        if answer.outcome == RelayOutcome::Cancelled {
            return;
        }
        if !answer.responded && self.outbox.is_open() {
            // The station still waits for its response: ours, so it does not wait in vain.
            let request = inner
                .request_frame(r#ref)
                .and_then(|kept| self.deps.codec.decode_client(&kept.frame).ok())
                .and_then(|decoded| match decoded.message {
                    ClientMessage::Event(
                        ClientEvent::VoiceInput { request, .. } | ClientEvent::TextInput { request, .. },
                    ) => Some(request),
                    _ => None,
                })
                .unwrap_or_else(unknown_request);
            let mut options = self.answer_options(inner, r#ref, &request);
            options.listen = ListenMode::Never;
            let reply = self.start_answer(inner, options);
            self.spawn_answer(reply, single("Яндекс не ответил."), text.to_string());
        }
    }

    /// A skill or a clarifying question keeps the dialog with Yandex until it lets go or time runs out.
    fn follow_yandex(&self, inner: &mut Inner, answer: &YandexAnswer, capability: CapabilityName) {
        if answer.ends_dialog || !answer.should_listen {
            if inner.continuation.is_some_and(|continuation| continuation.executor == RouteStep::Yandex) {
                inner.continuation = None;
            }
        } else {
            inner.continuation = Some(Continuation {
                capability,
                executor: RouteStep::Yandex,
                until: now_ms() + self.deps.skills.config.continuation_ms,
            });
        }
        inner.follow_up = Some(FollowUp::Listens(answer.should_listen));
    }

    /// A lookup for the model in a conversation: Yandex's answer as text; nothing of it reaches the station. The
    /// question is the model's call put into words, the phrase what the user said.
    async fn ask_yandex_quietly(
        &self,
        r#ref: &str,
        request: &AliceRequest,
        question: &str,
        phrase: &str,
        cancel: &CancellationToken,
    ) -> String {
        let message_id = new_message_id();
        let (yandex, frame) = {
            let inner = self.lock();
            let Some(yandex) = inner.yandex.clone().filter(|yandex| yandex.connected()) else {
                return "Yandex is not reachable: tell the user in one short sentence that you cannot do it now."
                    .into();
            };
            // A request id of its own: this is not the station's request, which our model answers.
            let request_id = new_message_id();
            (yandex, self.text_frame(&inner, r#ref, &message_id, request, question, Some(&request_id)))
        };
        let frame = match frame {
            Ok(frame) => frame,
            Err(error) => return format!("error: {}", describe(&error)),
        };
        let mut options = YandexRelayOptions::new(NO_EVENTS, self.deps.config.yandex_timeout_ms);
        options.quiet = true;
        let relay = YandexRelay::new(options, self.link());
        let watched = relay.clone();
        let stop = cancel.clone();
        let watcher = tokio::spawn(async move {
            tokio::select! {
                _ = stop.cancelled() => watched.cancel("the answer was cancelled"),
                _ = watched.done() => {}
            }
        });
        let exchange = match yandex.exchange(&message_id, vec![frame], relay.clone(), "textInput (lookup)") {
            Ok(exchange) => exchange,
            Err(error) => {
                watcher.abort();
                return format!("error: {}", describe(&error));
            }
        };
        let answer = relay.done().await;
        watcher.abort();
        yandex.end_exchange(exchange, 10_000);
        let asked =
            if question == phrase { format!("\"{question}\"") } else { format!("\"{question}\" (for \"{phrase}\")") };
        let mut data = json!({ "question": question, "phrase": phrase });
        if let (Value::Object(data), Ok(Value::Object(fields))) = (&mut data, serde_json::to_value(&answer)) {
            data.extend(fields);
        }
        self.record(
            EventDirection::Note,
            "yandexLookup",
            Some(format!("{asked} → \"{}\" ({})", answer.text, answer.outcome.as_str())),
            Some(&data),
            false,
        );
        if answer.text.is_empty() {
            "Yandex had no answer: tell the user you cannot find out now.".into()
        } else {
            format!("Yandex answered: {}", answer.text)
        }
    }

    // ---------------------------------------------------------------- answers

    /// One voice at a time: a new answer stops the one being spoken.
    fn start_answer(&self, inner: &mut Inner, mut options: SpokenAnswerOptions) -> Arc<SpokenAnswer> {
        if let Some(answer) = &inner.answer {
            answer.cancel("superseded");
        }
        options.wait_max_ms = self.deps.config.answer_wait_max_ms;
        let follows = options.response.is_some();
        let answer = SpokenAnswer::new(options, self.link(), self.synthesizer(inner));
        inner.answer = Some(answer.clone());
        if follows {
            inner.follow_up = Some(FollowUp::Answer(answer.clone()));
        }
        answer
    }

    /// The answer plays in its own task, as `void this.playAnswer(…)` did.
    fn spawn_answer(self: &Arc<Self>, answer: Arc<SpokenAnswer>, phrases: AnswerParts, question: String) {
        let session = self.clone();
        tokio::spawn(async move {
            session.play_answer(answer, phrases, question).await;
        });
    }

    async fn play_answer(
        self: &Arc<Self>,
        answer: Arc<SpokenAnswer>,
        phrases: AnswerParts,
        question: String,
    ) -> SayOutcome {
        let trip = self.lock().trip(&answer.options.r#ref);
        if let Some(trip) = &trip {
            trip.claim(answer.owner);
            let (by, what) = self.voice_of();
            answer.observe(Arc::new(TripObserver { trip: trip.clone(), by, what }));
        }
        let outcome = SayOutcome::Answer(answer.play(phrases).await);
        // Whoever counts from the end of the speech (the turn timer): the audio still to be played is known.
        if let Some(spoken) =
            answer.take_after_spoken().filter(|_| outcome == SayOutcome::Answer(AnswerOutcome::Spoken))
        {
            let now = now_ms();
            spoken(now + answer.playback_left_ms(now));
        }
        let deferred = outcome == SayOutcome::Answer(AnswerOutcome::Deferred);
        let errors = answer.errors();
        let text = answer.text();
        {
            let mut inner = self.lock();
            if inner.answer.as_ref().is_some_and(|current| Arc::ptr_eq(current, &answer)) {
                inner.answer = None;
            }
            // A deferred answer's request ends when its words are said (deliver).
            if let Some(trip) = trip.as_ref().filter(|_| !deferred) {
                trip.answered(&without_markup(&text));
                let status = status_of(outcome, !errors.is_empty(), answer.has_directives());
                let error = if errors.is_empty() { None } else { Some(errors.join("; ")) };
                // The audio went out faster than it is played: the request ends when the station stops speaking.
                let playing = if outcome == SayOutcome::Answer(AnswerOutcome::Spoken) {
                    answer.playback_left_ms(now_ms())
                } else {
                    0
                };
                if playing > 0 {
                    self.end_trip_later(&mut inner, &answer.options.r#ref, playing, status, error, answer.owner);
                } else {
                    self.end_trip(&mut inner, &answer.options.r#ref, status, error.as_deref(), Some(answer.owner));
                }
            }
        }
        let tag = tag8(&answer.options.r#ref);
        let failed =
            if answer.failed_phrases() > 0 { format!(" ({} failed)", answer.failed_phrases()) } else { String::new() };
        let audio_ms = answer.audio_ms();
        let mut counts =
            vec![format!("phrases {}{failed}", answer.phrases()), format!("audio {:.1}s", audio_ms as f64 / 1000.0)];
        if let Some(stream_id) = answer.stream_id() {
            counts.push(format!("stream {stream_id}"));
        }
        let silence_ms = answer.silence_ms();
        if silence_ms > 0 {
            counts.push(format!("kept waiting with {:.1}s of silence", silence_ms as f64 / 1000.0));
        }
        let expects = answer.expects_follow_up();
        if answer.should_listen() {
            counts.push(if expects { "listens after" } else { "listening closed after" }.into());
        }
        let errors_part = if errors.is_empty() { String::new() } else { format!(" | {}", errors.join("; ")) };
        let summary = format!(
            "{} \"{text}\" | {} | {}{errors_part}",
            outcome.as_str(),
            answer.timeline_summary(),
            counts.join(", ")
        );
        if errors.is_empty() {
            tracing::info!(target: "session", "Session {} answer {tag}: {summary}", tag8(&self.id));
        } else {
            tracing::warn!(target: "session", "Session {} answer {tag}: {summary}", tag8(&self.id));
        }
        let mut data = json!({
            "ref": answer.options.r#ref,
            "question": question,
            "outcome": outcome.as_str(),
            "text": text,
            "timeline": answer.timeline_json(),
            "phrases": answer.phrases(),
            "failedPhrases": answer.failed_phrases(),
            "audioMs": audio_ms,
            "shouldListen": answer.should_listen(),
            "expectsFollowUp": expects,
            "dialog": self.deps.dialog.description(),
        });
        if let Some(stream_id) = answer.stream_id() {
            data["streamId"] = json!(stream_id);
        }
        if !errors.is_empty() {
            data["errors"] = json!(errors);
        }
        if silence_ms > 0 {
            data["silenceMs"] = json!(silence_ms);
        }
        if let Some(reason) = answer.deferred_reason() {
            data["deferred"] = json!(reason);
        }
        self.record(EventDirection::Note, "answerTimeline", Some(format!("{tag} {summary}")), Some(&data), false);
        if deferred {
            self.deliver_later(answer);
        }
        outcome
    }

    // ---------------------------------------------------------------- deferred answers

    /// The station says it gave up on a request (requestStat): an answer of ours still going for it no longer
    /// reaches it, and goes on to be said later; one that cannot be stops.
    fn on_station_error(&self, inner: &mut Inner, r#ref: &str, error: &str) {
        if !station_gave_up_on(error) {
            return;
        }
        let Some(answer) = inner.answer.clone().filter(|answer| answer.options.r#ref == r#ref) else { return };
        if !answer.station_gave_up(error) {
            answer.cancel(&format!("station: {error}"));
        }
    }

    /// An answer the station stopped waiting for goes on detached from the current answer: a new request does not
    /// stop it; «стоп» with nothing else to stop, a cancel of its request or the station's session closing do. Its
    /// words, once complete within DEFERRED_MAX_MS of its start, are said through our push as a reminder is (`say`),
    /// when the station is free (see `free`): it never speaks over a reply.
    fn deliver_later(self: &Arc<Self>, answer: Arc<SpokenAnswer>) {
        let reason = answer.deferred_reason().unwrap_or_default();
        let within = minutes(self.deps.config.deferred_max_ms);
        self.lock().deferred.push(answer.clone());
        self.note_deferred(
            &answer,
            "deferred",
            &format!("{reason}; its words follow through our push within {within}"),
        );
        let session = self.clone();
        tokio::spawn(async move { session.deliver(answer).await });
    }

    async fn deliver(self: Arc<Self>, answer: Arc<SpokenAnswer>) {
        let max_ms = self.deps.config.deferred_max_ms;
        let deadline = answer.started_at().unwrap_or_else(tokio::time::Instant::now) + Duration::from_millis(max_ms);
        let words = match tokio::time::timeout_at(deadline, answer.deferred_text()).await {
            Err(_) => {
                let why = format!("its words were not complete within {}", minutes(max_ms));
                answer.cancel(&why);
                Err(why)
            }
            Ok(None) => Err(answer.cancel_reason().unwrap_or_else(|| "stopped".into())),
            Ok(Some(text)) if without_markup(&text).is_empty() => Err("no words left to say".into()),
            Ok(Some(text)) => self.wait_until_free(&answer, deadline).await.map(|()| text),
        };
        let (status, error) = match words {
            Err(why) => {
                self.note_deferred(&answer, "dropped", &why);
                (RequestStatus::Cancelled, Some(why))
            }
            Ok(text) => {
                self.note_deferred(&answer, "saying", &format!("\"{}\"", without_markup(&text)));
                let said = self.say(text.clone(), None).await;
                self.lock().delivering = false;
                self.note_deferred(&answer, said.as_str(), &format!("\"{}\"", without_markup(&text)));
                let failed =
                    (said != SayOutcome::Answer(AnswerOutcome::Spoken)).then(|| format!("deferred: {}", said.as_str()));
                (status_of(said, failed.is_some(), false), failed)
            }
        };
        let mut inner = self.lock();
        inner.deferred.retain(|deferred| !Arc::ptr_eq(deferred, &answer));
        self.end_trip(&mut inner, &answer.options.r#ref, status, error.as_deref(), Some(answer.owner));
    }

    /// Waits until the station is free for a moment (`IDLE_SETTLE_MS`), and takes the turn to speak.
    async fn wait_until_free(&self, answer: &SpokenAnswer, deadline: tokio::time::Instant) -> Result<(), String> {
        let mut free_since: Option<tokio::time::Instant> = None;
        loop {
            {
                let mut inner = self.lock();
                if inner.closed_at.is_some() {
                    return Err("the station's session closed".into());
                }
                if answer.cancelled() {
                    return Err(answer.cancel_reason().unwrap_or_else(|| "stopped".into()));
                }
                if Self::free(&inner) {
                    let since = *free_since.get_or_insert_with(tokio::time::Instant::now);
                    if since.elapsed() >= Duration::from_millis(IDLE_SETTLE_MS) {
                        inner.delivering = true;
                        return Ok(());
                    }
                } else {
                    free_since = None;
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(format!("the station was busy for {}", minutes(self.deps.config.deferred_max_ms)));
            }
            tokio::time::sleep(Duration::from_millis(IDLE_CHECK_MS)).await;
        }
    }

    /// Nothing for a deferred answer to speak over: no answer going or still playing, nobody talking, Yandex not
    /// answering, no push of ours waiting, no other deferred answer being said.
    fn free(inner: &Inner) -> bool {
        inner.answer.is_none()
            && inner.voice_requests.is_empty()
            && inner.lingering.is_empty()
            && inner.yandex_relay.is_none()
            && inner.pending_says.is_empty()
            && !inner.delivering
    }

    fn note_deferred(&self, answer: &SpokenAnswer, event: &str, summary: &str) {
        let r#ref = &answer.options.r#ref;
        self.note(
            "deferredAnswer",
            &format!("{} {event}: {summary}", tag8(r#ref)),
            Some(json!({ "ref": r#ref, "event": event })),
        );
    }

    /// Speech for our answers, as TTS_PROVIDER chooses and nothing else: Yandex through our session with the
    /// station's credentials, or our engine (mixed, own). No fallback from one to the other (the owner's choice,
    /// 02.11).
    fn synthesizer(&self, inner: &Inner) -> Synthesize {
        let provider = self.tts_provider();
        if matches!(provider, TtsProvider::Mixed | TtsProvider::Own) {
            return match self.deps.voice.speaker() {
                Some(own) => Arc::new(move |text: String, cancel: CancellationToken| {
                    let own = own.clone();
                    async move {
                        let spoken = own.synthesize(&text, &cancel).await?;
                        let audio = futures::stream::once(async move { Ok::<Bytes, anyhow::Error>(spoken.audio) });
                        Ok(Synthesis { format: spoken.format, audio: audio.boxed() })
                    }
                    .boxed()
                }),
                None => {
                    let reason = format!("TTS_PROVIDER={} without TTS_OWN_URL", provider.as_str());
                    failing(reason)
                }
            };
        }
        if let (TtsProvider::Yandex, Some(yandex)) = (provider, inner.yandex.clone()) {
            // The phrase is asked for at the call, in the answer's order.
            return Arc::new(move |text: String, cancel: CancellationToken| yandex.synthesize(&text, &cancel));
        }
        failing(if provider == TtsProvider::None {
            "speech is off (TTS_PROVIDER=none)".into()
        } else {
            "no SynchronizeState from the station yet: no credentials for Yandex speech".into()
        })
    }

    /// Yandex's speech in our voice (TTS_PROVIDER=own): its response went to the station as it came, its stream
    /// stays here, and we say its text.
    fn revoice(self: &Arc<Self>, inner: &mut Inner, r#ref: &str, text: &str) {
        self.note("revoiced", &format!("Yandex's \"{text}\" in our voice"), None);
        let answer = self.start_answer(inner, SpokenAnswerOptions::speech_only(r#ref));
        self.spawn_answer(answer, single(text), format!("Yandex's speech \"{text}\""));
    }

    /// Silent end: cancelled, superseded by a new wake word, or the connection is gone.
    fn abort_voice(&self, inner: &mut Inner, key: VoiceKey, reason: &str) {
        // Still listening or recognising: the request ends here, unanswered.
        if let Some(message_id) = inner.voice(key).map(|voice| voice.message_id.clone()) {
            self.end_trip(inner, &message_id, RequestStatus::Cancelled, Some(reason), None);
        }
        self.end_voice(inner, key, reason);
    }

    /// Releases the request and writes its one-line summary plus the full report to the journal.
    fn end_voice(&self, inner: &mut Inner, key: VoiceKey, outcome: &str) {
        let Some(index) = inner
            .voice_requests
            .iter()
            .position(|voice| voice.stream_id == key.stream_id && voice.serial == key.serial)
        else {
            return;
        };
        let previous = inner.voice_requests[index].outcome.clone();
        if !inner.voice_requests[index].close(outcome) {
            return;
        }
        let voice = inner.voice_requests.remove(index);
        let label = match previous.filter(|previous| previous != outcome) {
            Some(previous) => format!("{outcome} (after {previous})"),
            None => outcome.to_string(),
        };
        let speech = voice.speech.report();
        let summary = format!(
            "{label}{} | {} | {}",
            describe_recognition(&voice),
            voice.timeline.summary(),
            describe_speech(&speech, &voice)
        );
        tracing::info!(target: "session", "Session {} voice {}: {summary}", tag8(&self.id), voice.tag());
        let mut data = json!({
            "outcome": label,
            "timeline": voice.timeline,
            "audio": { "bytes": voice.audio_bytes, "frames": voice.audio_frames },
            "speech": speech,
            "interims": voice.interim.count(),
            "partials": voice.partials,
        });
        if let Some(request_id) = &voice.request.request_id {
            data["requestId"] = json!(request_id);
        }
        if let Some(at) = voice.spotter_end_at_bytes {
            data["audio"]["spotterEndAtBytes"] = json!(at);
        }
        if let Some(file) = voice.audio_file() {
            data["audio"]["file"] = json!(file);
        }
        for (key, value) in [
            ("utterance", serde_json::to_value(&voice.utterance).ok()),
            ("recognition", serde_json::to_value(&voice.recognition).ok()),
            ("allAudio", serde_json::to_value(&voice.all_audio).ok()),
            ("voicing", serde_json::to_value(voice.voicing).ok()),
        ] {
            if let Some(value) = value.filter(|value| !value.is_null()) {
                data[key] = value;
            }
        }
        if !voice.interim.text().is_empty() {
            data["lastInterimText"] = json!(voice.interim.text());
        }
        if voice.interim.quiet() {
            data["quietSpeech"] = json!(true);
        }
        if let Some(error) = &voice.error {
            data["error"] = json!(error);
        }
        self.record(
            EventDirection::Note,
            "voiceTimeline",
            Some(format!("{} {summary}", voice.tag())),
            Some(&data),
            false,
        );
    }

    fn save_spotter_log(&self, inner: &mut Inner, stream_id: u32) {
        let Some(index) = inner.spotter_logs.iter().position(|log| log.stream_id == stream_id) else { return };
        let log = inner.spotter_logs.remove(index);
        if log.bytes == 0 {
            return;
        }
        let raw: Vec<u8> = log.chunks.concat();
        let format = log.format.clone().unwrap_or_default();
        let wav = PCM_FORMAT.iter().all(|part| format.contains(part));
        let data = if wav {
            let pcm: Vec<i16> = raw.as_chunks::<2>().0.iter().map(|pair| i16::from_le_bytes(*pair)).collect();
            encode_wav(&pcm, SPEECH_SAMPLE_RATE)
        } else {
            raw
        };
        let name = format!("{}_spotter_{stream_id}.{}", log.time, if wav { "wav" } else { "bin" });
        let file = self.journal.save_audio(&name, data).map(|file| file.to_string_lossy().into_owned());
        let mut details = Map::new();
        if let Some(file) = file {
            details.insert("file".into(), json!(file));
        }
        if let Some(transcript) = &log.transcript {
            details.insert("transcript".into(), json!(transcript));
        }
        self.record(
            EventDirection::Note,
            "spotterLog",
            Some(format!("stream {stream_id}: {} bytes {format}", log.bytes)),
            Some(&Value::Object(details)),
            false,
        );
    }

    fn on_text_input(self: &Arc<Self>, inner: &mut Inner, header: EventHeader, request: AliceRequest, frame: Bytes) {
        if request.event.r#type == AliceEventType::ServerAction {
            return self.on_callback(inner, header, request, frame, false);
        }
        let text = match request.event.r#type {
            AliceEventType::TextInput => {
                request.event.text.as_deref().map(|text| text.trim_matches(is_js_space).to_string())
            }
            _ => None,
        };
        if let Some(text) = text.filter(|text| !text.is_empty()) {
            self.begin_trip(inner, &header.message_id, RequestKind::Text, Some(&text));
            return self.handle_request(inner, &header.message_id, &request, &text, false);
        }
        self.send_empty_response(&header.message_id, &request, Vec::new());
    }

    /// A callback of a command the station ran (server_action): ours by our prefix, the rest are Yandex's commands.
    /// Either way the station gets an empty response, as Yandex sends it; a callback nobody takes is journaled.
    fn on_callback(
        self: &Arc<Self>,
        inner: &mut Inner,
        header: EventHeader,
        request: AliceRequest,
        frame: Bytes,
        waited: bool,
    ) {
        let name = request.event.name.clone().unwrap_or_default();
        let message_id = header.message_id.clone();
        if !waited && self.drops_player_continue(&request) {
            return self.send_empty_response(&message_id, &request, Vec::new());
        }
        if !waited {
            self.deps.skills.player_memory.note_callback(&self.device_tag, &name, chrono::Utc::now());
        }
        if name == *SAY_CALLBACK {
            return self.on_say_callback(inner, &message_id, &request);
        }
        if let Some(handler) = self.deps.skills.callback_handler(&name, request.event.payload.as_ref()) {
            // Ours: an empty response, or one with the commands it brings (the next track).
            let session = self.clone();
            let payload = request.event.payload.clone().unwrap_or_default();
            let device = self.device_tag.clone();
            tokio::spawn(async move {
                let outcome = handler(device, name, payload).await;
                let directives = outcome.map(|outcome| outcome.directives).unwrap_or_default();
                session.send_empty_response(&message_id, &request, directives);
            });
            return;
        }
        let ours = name.starts_with(OUR_CALLBACK_PREFIX);
        if !ours && !waited && !self.yandex_available(inner) {
            if let Some(yandex) = inner.yandex.clone() {
                // Right after connecting the station sends what it kept while away, a second before our session is
                // up.
                let session = self.clone();
                tokio::spawn(async move {
                    yandex.ready(YANDEX_OPENING_WAIT_MS).await;
                    let mut inner = session.lock();
                    session.on_callback(&mut inner, header, request, frame, true);
                });
                return;
            }
        }
        if !ours && self.yandex_available(inner) {
            // Yandex's command calls back: Yandex answers it (the next block of a show, a call's next step).
            let label = format!("callback {name}");
            return self.relay_yandex(
                inner,
                None,
                &message_id,
                vec![frame],
                callback_capability(&name),
                &name,
                NO_EVENTS,
                &label,
                player_callback(&name),
            );
        }
        let shown = if name.is_empty() { "(no name)" } else { name.as_str() };
        let why = if ours { "no handler of ours" } else { "Yandex is not reachable" };
        self.note("callback", &format!("{shown}: {why}, journaled only"), None);
        self.send_empty_response(&message_id, &request, Vec::new());
    }

    /// What the station says of its player in a request: playing, or idle or stopped.
    fn remember_player(&self, request: &AliceRequest) {
        let state = request.device_state.as_ref().and_then(|state| state.player.as_ref()).map(|player| player.state);
        if let Some(state) = state.filter(|state| *state != PlayerState::Unknown) {
            self.deps.skills.player_memory.note(
                &self.device_tag,
                state == PlayerState::Playing,
                "device state",
                chrono::Utc::now(),
                None,
            );
        }
    }

    /// Reconnecting, the station asks to continue its player, and Yandex starts music even an hour after it was
    /// stopped. Right after connecting it goes on only if the player played before the break; later it is a
    /// button's.
    fn drops_player_continue(&self, request: &AliceRequest) -> bool {
        if !is_player_continue(request) || self.connected.elapsed() > Duration::from_millis(RECONNECT_CONTINUE_MS) {
            return false;
        }
        let before = self.deps.skills.player_memory.get(&self.device_tag);
        let now = request
            .device_state
            .as_ref()
            .and_then(|state| state.player.as_ref())
            .map_or_else(|| "not reported".to_string(), |player| name_of(player.state));
        let drop = !before.as_ref().is_some_and(|before| before.playing);
        let action = if drop { "dropped" } else { "passed to Yandex" };
        let was = match &before {
            Some(before) => {
                format!("{} ({}, {})", if before.playing { "playing" } else { "not playing" }, before.by, before.at)
            }
            None => "unknown".into(),
        };
        self.note(
            "playerContinue",
            &format!("{action}: before the break {was}; the station now: {now}"),
            Some(json!({ "before": before, "now": now, "drop": drop })),
        );
        drop
    }

    // ---------------------------------------------------------------- outbound

    /// `quiet` keeps frequent events (interim results) in the journal but out of the console.
    fn send(&self, message: &ServerMessage, quiet: bool) {
        if !self.outbox.is_open() {
            return;
        }
        match self.deps.codec.encode_server(message) {
            Ok(encoded) => {
                self.outbox.send_binary(encoded.frame);
                self.record(
                    EventDirection::Out,
                    kind_of(message),
                    Some(summarize_server(message)),
                    Some(&encoded.encoded),
                    quiet,
                );
            }
            Err(error) => tracing::error!(target: "session", "cannot encode {}: {error}", kind_of(message)),
        }
    }

    /// A frame from Yandex for the station, already edited (refs, stream ids); journaled under its kind.
    fn send_raw(&self, frame: Bytes, kind: &str, summary: String, decoded: &Value) {
        if !self.outbox.is_open() {
            return;
        }
        self.outbox.send_binary(frame);
        self.record(EventDirection::Out, kind, Some(summary), Some(decoded), false);
    }

    /// Audio of an answer: [u32 BE stream id][Ogg pages]. Counted in the answer's timeline, not journaled frame by
    /// frame.
    fn send_audio(&self, stream_id: u32, bytes: &[u8]) {
        if !self.outbox.is_open() || bytes.is_empty() {
            return;
        }
        let mut frame = Vec::with_capacity(4 + bytes.len());
        frame.extend_from_slice(&stream_id.to_be_bytes());
        frame.extend_from_slice(bytes);
        self.outbox.send_binary(Bytes::from(frame));
    }

    // ---------------------------------------------------------------- lifecycle and journal

    fn on_close(&self, code: u16, reason: &str) {
        {
            let mut inner = self.lock();
            if let Some(answer) = &inner.answer {
                answer.cancel("disconnected");
            }
            for answer in &inner.deferred {
                answer.cancel("the station's session closed");
            }
            if let Some(relay) = &inner.yandex_relay {
                relay.cancel("disconnected");
            }
            for (_, relay) in &inner.player_relays {
                relay.cancel("disconnected");
            }
            if let Some(yandex) = &inner.yandex {
                yandex.close();
            }
            let voices: Vec<VoiceKey> = inner
                .voice_requests
                .iter()
                .map(|voice| VoiceKey { stream_id: voice.stream_id, serial: voice.serial })
                .collect();
            for key in voices {
                self.abort_voice(&mut inner, key, "disconnected");
            }
            let logs: Vec<u32> = inner.spotter_logs.iter().map(|log| log.stream_id).collect();
            for stream_id in logs {
                self.save_spotter_log(&mut inner, stream_id);
            }
            self.end_lingering(&mut inner);
            let refs: Vec<String> = inner.trips.iter().map(|(r#ref, _)| r#ref.clone()).collect();
            for r#ref in refs {
                self.end_trip(&mut inner, &r#ref, RequestStatus::Cancelled, Some("disconnected"), None);
            }
            inner.closed_at = Some(now_iso());
        }
        let reason = if reason.is_empty() { String::new() } else { format!(" reason={reason}") };
        self.note("disconnected", &format!("code={code}{reason}"), None);
        self.journal.close();
    }

    // ---------------------------------------------------------------- the request's trip (src/core/requests)

    fn set_trip(inner: &mut Inner, r#ref: &str, trip: Arc<RequestHandle>) {
        match inner.trips.iter_mut().find(|(known, _)| known == r#ref) {
            Some(entry) => entry.1 = trip,
            None => inner.trips.push((r#ref.to_string(), trip)),
        }
    }

    /// Starts following a request under the station's message id; an answer still being played has ended by now.
    fn begin_trip(
        &self,
        inner: &mut Inner,
        r#ref: &str,
        kind: RequestKind,
        text: Option<&str>,
    ) -> Option<Arc<RequestHandle>> {
        self.end_lingering(inner);
        let handle = self.deps.requests.as_ref()?.begin(RequestStart {
            device: self.device_tag.clone(),
            session: Some(self.id.clone()),
            kind,
            text: text.map(str::to_string),
            parent: None,
        });
        Self::set_trip(inner, r#ref, handle.clone());
        // What our player starts from here on is this request's: the station's answers go into its journal.
        let media_base = self.deps.skills.media_base_url(self.host.as_deref());
        self.deps.skills.player.note_request(&self.device_tag, &handle.id(), media_base);
        Some(handle)
    }

    /// Ends a request once the station has said its answer (at most a minute on): its speech step lasts as long.
    fn end_trip_later(
        &self,
        inner: &mut Inner,
        r#ref: &str,
        after_ms: u64,
        status: RequestStatus,
        error: Option<String>,
        owner: Owner,
    ) {
        if let Some(index) = inner.lingering.iter().position(|lingering| lingering.r#ref == r#ref) {
            let earlier = inner.lingering.remove(index);
            earlier.timer.cancel();
            self.end_trip(inner, &earlier.r#ref, earlier.status, earlier.error.as_deref(), Some(earlier.owner));
        }
        let id = self.next_serial.fetch_add(1, Ordering::SeqCst);
        let timer = CancellationToken::new();
        let stop = timer.clone();
        let me = self.me.clone();
        let watched = r#ref.to_string();
        tokio::spawn(async move {
            tokio::select! {
                _ = stop.cancelled() => {}
                _ = tokio::time::sleep(Duration::from_millis(after_ms.min(60_000))) => {
                    let Some(session) = me.upgrade() else { return };
                    let mut inner = session.lock();
                    let Some(index) = inner.lingering.iter().position(|lingering| lingering.r#ref == watched && lingering.id == id) else {
                        return;
                    };
                    let lingering = inner.lingering.remove(index);
                    session.end_trip(&mut inner, &lingering.r#ref, lingering.status, lingering.error.as_deref(), Some(lingering.owner));
                }
            }
        });
        inner.lingering.push(Lingering { r#ref: r#ref.to_string(), id, status, error, owner, timer });
    }

    /// Whatever is still being played ends now: a new request, the station gone.
    fn end_lingering(&self, inner: &mut Inner) {
        for lingering in std::mem::take(&mut inner.lingering) {
            lingering.timer.cancel();
            self.end_trip(inner, &lingering.r#ref, lingering.status, lingering.error.as_deref(), Some(lingering.owner));
        }
    }

    /// Ends a request (only by its claimant, when one is named) and forgets it once it has ended.
    fn end_trip(
        &self,
        inner: &mut Inner,
        r#ref: &str,
        status: RequestStatus,
        error: Option<&str>,
        owner: Option<Owner>,
    ) {
        let Some(handle) = inner.trip(r#ref) else { return };
        handle.finish(status, error, owner);
        if !handle.active() {
            inner.trips.retain(|(known, _)| known != r#ref);
        }
    }

    /// Our recognition's model, as its server says: the one that really listens, not the one ASR_MODEL names.
    fn ear_name(&self) -> Option<String> {
        self.deps.speech.ear()
    }

    /// Which model thinks and where: in the cloud or at home, by the router's profile.
    fn brain_of(&self, profile: Option<ModelProfile>) -> Brain {
        let brain = self.deps.dialog.brain();
        let profile = profile.unwrap_or(ModelProfile::Fast);
        let slot = match profile {
            ModelProfile::Fast => brain.profiles.fast.as_ref(),
            ModelProfile::Smart => brain.profiles.smart.as_ref().or(brain.profiles.fast.as_ref()),
        };
        let cloud = slot.map_or(brain.cloud, |slot| slot.cloud);
        let what = slot.map(|slot| slot.model.clone()).unwrap_or_else(|| match profile {
            ModelProfile::Fast => brain.models.fast.clone(),
            ModelProfile::Smart => brain.models.smart.clone(),
        });
        Brain { by: if cloud { Place::Cloud } else { Place::Own }, what }
    }

    /// Whose voice says our answers now: Alice's through Yandex, or ours.
    fn voice_of(&self) -> (Place, Option<String>) {
        if self.tts_provider() == TtsProvider::Yandex {
            return (Place::Yandex, Some("Алиса".into()));
        }
        (Place::Own, self.deps.voice.own_choice().map(|choice| choice.voice))
    }

    fn note(&self, kind: &str, summary: &str, data: Option<Value>) {
        tracing::info!(target: "session", "Session {} {kind}: {summary}", tag8(&self.id));
        self.record(EventDirection::Note, kind, Some(summary.to_string()), data.as_ref(), false);
    }

    fn record(&self, dir: EventDirection, kind: &str, summary: Option<String>, data: Option<&Value>, quiet: bool) {
        if dir != EventDirection::Note && !quiet {
            let line =
                summary.as_deref().filter(|summary| !summary.is_empty()).map(|s| format!(" {s}")).unwrap_or_default();
            tracing::debug!(target: "session", "Session {} {} {kind}{line}", tag8(&self.id), arrow(dir));
        }
        self.journal.record(dir, kind, summary, data);
    }
}

/// The spotter phrase the station reports is removed from the text along with WAKE_WORDS.
fn extra_wake_words(voice: &VoiceRequest) -> Vec<String> {
    voice.request.spotter_phrase.iter().cloned().collect()
}

/// A synthesis that cannot be: every phrase fails with the reason.
fn failing(reason: String) -> Synthesize {
    Arc::new(move |_text: String, _cancel: CancellationToken| -> BoxFuture<'static, anyhow::Result<Synthesis>> {
        let reason = reason.clone();
        async move { Err(anyhow::anyhow!(reason)) }.boxed()
    })
}

#[cfg(test)]
mod tests {
    use axum::extract::ws::Message;
    use axum::http::HeaderMap;

    use super::*;
    use crate::app::App;
    use crate::config::{Config, Env};

    /// The review's deadlock: a sessions poll (`snapshot`) asked our Yandex session whether it was up while it held
    /// the journal, and our Yandex session journals under its own state (`open`, `dispatch`, the heartbeat). Here
    /// the two meet in the order that deadlocked: our Yandex session holds its state, the poll comes in and waits
    /// for that state, then our Yandex session journals. The journal is a leaf lock now (SessionJournal): the poll
    /// waits holding the session's state only, which journaling never takes, so both finish.
    /// A session of an in-memory gateway, with no socket under it: what it sends stays in the queue.
    fn session_of(app: &App) -> (Arc<StationSession>, mpsc::UnboundedReceiver<Outgoing>) {
        let deps = Arc::new(SessionDeps {
            codec: Arc::new(AapiCodec::new()),
            config: Arc::new(app.config.gateway.clone()),
            speech: app.speech.clone(),
            dialog: app.dialog.clone(),
            skills: app.skills.clone(),
            voice: app.voice.clone(),
            modes: app.modes.clone(),
            household: app.access.household.clone(),
            events: app.events.clone(),
            requests: None,
            mcp: None,
            tools: None,
            rescue: None,
        });
        let handshake = Handshake { headers: HeaderMap::new(), remote_address: "192.168.1.50".into() };
        StationSession::new(&handshake, deps)
    }

    /// Nothing on disk: no database, no session journal folder (the latest events stay in memory).
    fn memory_gateway() -> Arc<App> {
        App::build(Config::for_tests(&Env::from_pairs([("STATE_DIR", ""), ("RECORDER_ENABLED", "false")]))).unwrap()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_sessions_poll_never_deadlocks_with_our_yandex_session_journaling_under_its_state() {
        let app = memory_gateway();
        let (session, _outgoing) = session_of(&app);
        let link = session.link();
        // Never opened: nothing connects anywhere.
        let yandex = YandexSession::new(
            YandexSessionOptions {
                url: "ws://127.0.0.1:9/uni.ws".into(),
                tts: app.config.gateway.tts.clone(),
                ping_ms: 0,
                reconnect_ms: vec![1000],
            },
            StationCredentials { headers: Vec::new(), synchronize_state: Bytes::new() },
            session.deps.codec.clone(),
            link.clone(),
            link,
        );
        session.lock().yandex = Some(yandex.clone());

        let (done, finished) = std::sync::mpsc::channel();
        let (held, holding) = std::sync::mpsc::channel();
        let upstream = done.clone();
        std::thread::spawn(move || {
            let meanwhile = || {
                let _ = held.send(());
                // Time for the poll to come in and wait for our state.
                std::thread::sleep(Duration::from_millis(300));
            };
            yandex.note_under_state(meanwhile, "yandexConnected", "journaled under its state");
            let _ = upstream.send("our Yandex session journaled");
        });
        holding.recv().unwrap();
        let poll = session.clone();
        std::thread::spawn(move || {
            let snapshot = poll.snapshot();
            let _ = done.send(if snapshot.yandex_connected == Some(false) { "the poll answered" } else { "?" });
        });
        let mut ended = Vec::new();
        for _ in 0..2 {
            ended.push(
                finished.recv_timeout(Duration::from_secs(5)).expect("the poll and our Yandex session deadlocked"),
            );
        }
        ended.sort();
        assert_eq!(ended, ["our Yandex session journaled", "the poll answered"]);
        assert!(session.recent_events(10).iter().any(|event| event.kind == "yandexConnected"));
    }

    /// A vgw_say callback that comes as its speech stops waiting (the 5 s ran out, and the speech has not yet taken
    /// its entry back) still gets an empty response, as in the TS.
    #[tokio::test]
    async fn answers_a_callback_its_speech_no_longer_waits_for_with_an_empty_response() {
        let app = memory_gateway();
        let (session, mut outgoing) = session_of(&app);
        let (resolve, called_back) = oneshot::channel::<SayCallback>();
        drop(called_back);
        let mut request = unknown_request();
        request.event.payload = json!({ "id": "say-1" }).as_object().cloned();
        {
            let mut inner = session.lock();
            inner.pending_says.push(("say-1".into(), resolve));
            session.on_say_callback(&mut inner, "callback-1", &request);
            assert!(inner.pending_says.is_empty());
        }
        let Ok(Outgoing::Message(Message::Binary(frame))) = outgoing.try_recv() else {
            panic!("a response goes to the station")
        };
        let decoded = session.deps.codec.decode_server_frame(&frame).unwrap();
        let event = &decoded["event"];
        assert_eq!(event["header"]["refMessageId"], "callback-1");
        assert!(event["aliceResponse"].is_object(), "{decoded}");
        assert!(session.recent_events(10).iter().any(|event| event.kind == "callback"
            && event.summary.as_deref().is_some_and(|summary| summary.contains("no longer waits"))));
    }

    #[test]
    fn reads_the_stations_request_to_continue_its_player() {
        // As the station sends it: field 1 the typed frame {53: {3: {1: 1}}}, field 2 the analytics with its purpose.
        let purpose = b"player_continue";
        let analytics = [&[0x10, 0x03, 0x1a, purpose.len() as u8][..], purpose].concat();
        let frame = [0xaa, 0x03, 0x04, 0x1a, 0x02, 0x08, 0x01];
        let raw = [&[0x0a, frame.len() as u8][..], &frame, &[0x12, analytics.len() as u8], &analytics].concat();
        let mut request = unknown_request();
        request.event.name = Some("@@mm_semantic_frame".into());
        request.event.payload_raw = Some(Bytes::from(raw));
        assert!(is_player_continue(&request));
        request.event.name = Some("@@mm_stack_engine_get_next".into());
        assert!(!is_player_continue(&request));
    }

    #[test]
    fn names_the_capability_of_a_yandex_callback() {
        assert_eq!(callback_capability("calls_next_step"), CapabilityName::Calls);
        assert_eq!(callback_capability("music_thin_client_on_started"), CapabilityName::Music);
        assert_eq!(callback_capability("@@mm_stack_engine_get_next"), CapabilityName::Skills);
        assert!(player_callback("@@mm_stack_engine_get_next") && !player_callback("vgw_media_on_finished"));
    }

    #[test]
    fn ends_a_request_by_its_answer() {
        let spoken = SayOutcome::Answer(AnswerOutcome::Spoken);
        assert_eq!(status_of(spoken, true, false), RequestStatus::Answered);
        assert_eq!(status_of(SayOutcome::Answer(AnswerOutcome::Silent), true, false), RequestStatus::Failed);
        assert_eq!(status_of(SayOutcome::Answer(AnswerOutcome::Empty), false, true), RequestStatus::Answered);
        assert_eq!(status_of(SayOutcome::Answer(AnswerOutcome::Empty), false, false), RequestStatus::Silent);
        assert_eq!(status_of(SayOutcome::Failed, false, false), RequestStatus::Failed);
    }

    #[test]
    fn splits_long_texts_into_phrases() {
        assert_eq!(split_phrases("  Хорошо.  "), vec!["Хорошо."]);
        assert!(split_phrases("   ").is_empty());
        let long = "Первое длинное предложение для проверки разбиения на фразы. ".repeat(4);
        assert!(split_phrases(&long).len() > 1);
    }
}
