//! One spoken answer. Phrases go to synthesis as soon as the dialog has them, all in flight at once, and their audio
//! is joined in order into one stream. AliceResponse goes out when the first phrase can be heard, after waiting up to
//! holdResponseMs for the rest of the text, so the response carries it and a right should_listen; then TtsSpeak, the
//! audio, the last page and the stream close.
//!
//! The station gives up on speech that keeps it waiting: 10 s after the response with no audio yet, or 10 s after the
//! stream's last audio, it drops the answer (see `STATION_SPEECH_TIMEOUT_MS`). A slow answer (a web search, an MCP
//! tool, a slow voice) keeps it waiting with silence on the stream, real Ogg/Opus pages joined into it as the speech is
//! (`keep_alive`), for at most `wait_max_ms` from the answer's start (ANSWER_WAIT_MAX_MS). Past that, or when the
//! station says it gave up all the same, the stream is closed and the answer goes on without the station: deferred,
//! its words are said later by the session, through our push, once the station is free (`deferred_text`).
//!
//! Its state is behind one lock taken for each step, as JavaScript ran them whole; what it sends goes straight to the
//! session's writer, which takes no lock of the session's.

use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::Duration;

use anyhow::Result;
use bytes::Bytes;
use futures::future::BoxFuture;
use futures::stream::{BoxStream, StreamExt};
use regex::Regex;
use serde_json::{json, Map, Value};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::config::ListenMode;
use crate::core::requests::{new_owner, Owner};
use crate::gateway::observability::timeline::Timeline;
use crate::gateway::protocol::ids::new_message_id;
use crate::gateway::protocol::proto_object::is_js_space;
use crate::gateway::protocol::protocol_types::{
    AliceResponseModel, CloseReason, Directive, DirectiveType, ServerMessage,
};
use crate::gateway::yandex::yandex_session::Synthesis;
use crate::skills::media::recording::AudioRecording;
use crate::skills::own::reminders_skill::AudioClip;
use crate::skills::skill::SpokenHook;
use crate::speech::audio::ogg_opus_encoder::{encode_ogg_opus, EncodeRate};
use crate::speech::audio::ogg_opus_joiner::OggOpusJoiner;
use crate::utils::time::now_ms;

pub trait AnswerOutput: Send + Sync {
    fn send(&self, message: ServerMessage);
    fn send_audio(&self, stream_id: u32, bytes: &[u8]);
    /// A new even stream id: the station's streams are odd.
    fn new_stream_id(&self) -> u32;
}

/// Speech for one phrase; `cancel` is the answer's.
pub type Synthesize = Arc<dyn Fn(String, CancellationToken) -> BoxFuture<'static, Result<Synthesis>> + Send + Sync>;

/// AliceResponse header fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseHeader {
    pub request_id: String,
    pub sequence_number: Option<i64>,
    pub dialog_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SpokenAnswerOptions {
    /// The station's request this answers; TtsSpeak and AliceResponse refer to it.
    pub r#ref: String,
    /// None for speech the station asked for itself (TtsGenerate): it gets no AliceResponse.
    pub response: Option<ResponseHeader>,
    pub hold_response_ms: u64,
    /// When the station keeps listening after this answer.
    pub listen: ListenMode,
    /// AliceResponse at once, before any text: the jingle plays while the model thinks, as with the generative
    /// Alice. should_listen is set then, so any mode but never asks the station to listen; the session closes a
    /// follow-up that the finished answer did not ask for (see `expects_follow_up`).
    pub early: bool,
    /// tts_play_placeholder in the response, last after the commands (first, on the Content channel, before
    /// audio_play).
    pub jingle: bool,
    /// Station commands the response carries.
    pub directives: Vec<Directive>,
    /// A whispered question gets a whispered answer: the station lowers its volume for it (sound_set_level with
    /// is_whisper, as Yandex does) and, with `markup`, the speech is synthesized as a whisper.
    pub whisper: bool,
    /// Synthesis markup (`<speaker …>`, sil<[ms]>) goes to Yandex's synthesis; without it, it is taken out.
    pub markup: bool,
    /// How long, from its start, the answer may keep the station waiting for its next speech (ANSWER_WAIT_MAX_MS):
    /// silence keeps the stream alive until then, and the stream is closed past it.
    pub wait_max_ms: u64,
    /// Past `wait_max_ms`, or when the station gives up on it, the answer goes on without the station and its words
    /// are said later (deferred). Off for speech that is said so itself (our push, a reminder) and for speech the
    /// station asked for (TtsGenerate): past the wait such an answer stops.
    pub deferrable: bool,
}

/// ANSWER_WAIT_MAX_MS by default.
pub const WAIT_MAX_MS: u64 = 45_000;

impl SpokenAnswerOptions {
    /// Speech for the station without an AliceResponse (TtsGenerate, our voice for Yandex's).
    pub fn speech_only(r#ref: &str) -> SpokenAnswerOptions {
        SpokenAnswerOptions {
            r#ref: r#ref.to_string(),
            response: None,
            hold_response_ms: 0,
            listen: ListenMode::Never,
            early: false,
            jingle: false,
            directives: Vec::new(),
            whisper: false,
            markup: false,
            wait_max_ms: WAIT_MAX_MS,
            deferrable: false,
        }
    }
}

/// Synthesis markup, the way Yandex writes it in output_speech (docs/research/original-alice.md).
static MARKUP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<speaker[^>]*>|sil<\[[0-9]+\]>|<\[[^\]]*\]>").expect("a valid pattern"));

/// A last sentence that asks for an answer though it ends with a full stop: «Скажите, что именно хотите…».
static INVITATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(скажите|назовите|уточните|выберите|подскажите|расскажите|ответьте)(?:[\s,!.]|$)")
        .expect("a valid pattern")
});

/// The text without synthesis markup: for the response's card, and for synthesis when markup is off.
pub fn without_markup(text: &str) -> String {
    let spaced = MARKUP.replace_all(text, " ");
    spaced.split(is_js_space).filter(|word| !word.is_empty()).collect::<Vec<_>>().join(" ")
}

/// How long an answer may keep the station without its response: the station gives up at 15 s (see
/// `keep_the_station_waiting`).
const RESPONSE_DEADLINE_MS: u64 = 10_000;

/// The station's own timers for an answer's speech, read from its requestStat errors in the session journals
/// (01.11–06.11). «Synthesis start timeout elapsed»: 10.0 s from our AliceResponse with no audio on the stream yet
/// (9 cases, 9.97–9.99 s by the station's clock). «Synthesis chunk timeout elapsed»: 10.0 s after the stream's audio
/// (2 cases, 06.11 01:12: 1.1 s and 1.8 s of «Сейчас посмотрю» came at once with the TtsSpeak, and the station gave
/// up 10.0 s after it, while its playback had ended 8.9 s and 8.2 s before: it counts from the audio it gets, not
/// from what it plays). Either way it then drops the answer.
pub const STATION_SPEECH_TIMEOUT_MS: u64 = 10_000;

/// Silence goes once the station has had nothing left to play for this long (before any audio: since the
/// response), 7 s inside its 10 s…
const KEEP_ALIVE_QUIET_MS: u64 = 3000;
/// …then again this long after the last silence has played, while the answer still waits: one every 1.5 s.
const KEEP_ALIVE_AGAIN_MS: u64 = 1000;
/// Nor does audio stop for longer than this, however much the station still has to play (its timer counts from the
/// audio it got): 4 s inside its 10 s.
const KEEP_ALIVE_SEND_MAX_MS: u64 = 6000;
/// One silence: 25 Opus packets of 20 ms.
const SILENCE_MS: usize = 500;
/// Looks at a stream that needs nothing yet (no response, a phrase streaming) this often.
const KEEP_ALIVE_POLL_MS: u64 = 250;

/// Half a second of digital silence as an Ogg/Opus segment: 48 kHz mono, as our own synthesis is encoded; made once.
static SILENCE: LazyLock<Option<Vec<u8>>> =
    LazyLock::new(|| encode_ogg_opus(&vec![0; SILENCE_MS * 48], EncodeRate::FullBand).ok());

/// The station plays its stream on its own clock, from the stream's first audio on, and skips audio that comes after
/// its time. Live 13.11: the stream opened with keep-alive silence at 32.5 s, half a second more went at 34.0, the
/// speech (2.7 s) came at 35.3 and was placed right after the 1 s of silence, so the station had passed most of it:
/// only «…как» of «Отлично, всё в порядке. А у вас как?» was heard, and the station stopped at 36.1, the stream's
/// start plus its length. Audio that would land behind the clock goes after silence that brings the stream up to
/// the clock and this much ahead of it (the way to the station and its own buffering).
const CATCH_UP_LEAD_MS: u64 = 200;
/// The silence that catches up, in steps of this length.
const CATCH_UP_STEP_MS: usize = 100;
static CATCH_UP_SILENCE: LazyLock<Option<Vec<u8>>> =
    LazyLock::new(|| encode_ogg_opus(&vec![0; CATCH_UP_STEP_MS * 48], EncodeRate::FullBand).ok());

/// The jingle directive as Yandex sends it in every response.
pub fn placeholder_directive(channel: &str) -> Directive {
    let mut payload = Map::new();
    payload.insert("channel".into(), json!(channel));
    Directive {
        r#type: DirectiveType::ClientAction,
        name: "tts_play_placeholder".into(),
        analytics_type: None,
        payload: Some(payload),
        typed_payload: None,
        is_led_silent: Some(true),
        ignore_answer: None,
    }
}

/// The sentence a text ends with: what follows the last space after a sentence's end (`.!?…` and any closing
/// quotes or brackets). The TS split on a look-behind, which the regex crate does not have.
fn last_sentence(text: &str) -> &str {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut start = 0;
    let mut i = 0;
    while i < chars.len() {
        if !is_js_space(chars[i].1) {
            i += 1;
            continue;
        }
        let run_start = i;
        while i < chars.len() && is_js_space(chars[i].1) {
            i += 1;
        }
        // Before the run: closing marks, then a sentence's end.
        let mut j = run_start;
        while j > 0 && matches!(chars[j - 1].1, '"' | '»' | ')') {
            j -= 1;
        }
        if j > 0 && matches!(chars[j - 1].1, '.' | '!' | '?' | '…') {
            start = chars.get(i).map_or(text.len(), |(at, _)| *at);
        }
    }
    &text[start..]
}

/// A question at the end of the text, closing quotes and brackets aside, or a last sentence that invites an answer
/// («Скажите, что именно…»); not a conditional one («Если захотите…, просто скажите.»).
pub fn ends_with_question(text: &str) -> bool {
    let trimmed = text.trim_matches(is_js_space);
    if trimmed.trim_end_matches(['"', '»', ')']).ends_with('?') {
        return true;
    }
    let last = last_sentence(trimmed);
    let bare = last.trim_start_matches(|c: char| is_js_space(c) || matches!(c, '«' | '"' | '—' | '–' | '-'));
    INVITATION.is_match(bare)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnswerOutcome {
    Spoken,
    Silent,
    Empty,
    Cancelled,
    /// The station stopped waiting for it (ANSWER_WAIT_MAX_MS, or it gave up): its stream is closed, and its words
    /// come later (`deferred_text`).
    Deferred,
}

impl AnswerOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            AnswerOutcome::Spoken => "spoken",
            AnswerOutcome::Silent => "silent",
            AnswerOutcome::Empty => "empty",
            AnswerOutcome::Cancelled => "cancelled",
            AnswerOutcome::Deferred => "deferred",
        }
    }
}

/// Who follows an answer as it goes (the request's trip): its text as it grows, the moment its speech begins.
pub trait AnswerObserver: Send + Sync {
    fn text(&self, text: &str);
    fn speaking(&self);
}

/// A phrase, or recorded audio among the phrases (a voice message, a sound): Ogg/Opus played as it is, in the same
/// stream.
pub enum AnswerPart {
    Text(String),
    Clip(AudioClip),
}

/// The phrases of an answer as they come.
pub type AnswerParts = BoxStream<'static, AnswerPart>;

struct QueuedPhrase {
    text: String,
    synthesis: JoinHandle<Result<Synthesis>>,
}

/// An error as the journal says it: its message, and its cause in brackets.
pub fn describe(error: &anyhow::Error) -> String {
    match error.source() {
        Some(cause) => format!("{error} ({cause})"),
        None => error.to_string(),
    }
}

/// The first `count` characters, as JavaScript's slice of a short text.
fn head(text: &str, count: usize) -> String {
    text.chars().take(count).collect()
}

fn ms(value: u64) -> Duration {
    Duration::from_millis(value)
}

/// A told tale being kept, with its own joiner: it gets the speech alone, not the silence the station waited in.
struct Recorder {
    recording: Arc<dyn AudioRecording>,
    joiner: OggOpusJoiner,
}

struct State {
    text: String,
    phrases: u32,
    failed_phrases: u32,
    stream_id: Option<u32>,
    /// should_listen as sent in the response.
    should_listen: bool,
    errors: Vec<String>,
    joiner: OggOpusJoiner,
    text_complete: bool,
    responded: bool,
    responded_early: bool,
    recorder: Option<Recorder>,
    observer: Option<Arc<dyn AnswerObserver>>,
    /// When its stream began (ms since the epoch): the station plays it from then on.
    speech_started_at: Option<u64>,
    /// The same on tokio's clock: where the station is in the stream (`catch_up`).
    stream_opened: Option<Instant>,
    /// Silence put in so audio does not land behind the station's clock, ms (part of `silence_ms`).
    caught_up_ms: u64,
    timeline: Timeline,
    listen: ListenMode,
    directives: Vec<Directive>,
    // What keeps the station waiting is timed on tokio's clock (paused in tests).
    /// When the answer began to play, and when its response went out (speech the station asked for: its start).
    started: Option<Instant>,
    responded_at: Option<Instant>,
    /// When audio last went to the stream, and until when the station has it to play.
    last_sent: Option<Instant>,
    heard_until: Option<Instant>,
    /// A phrase's audio is going out: silence waits for its end (its pages belong to its segment).
    in_phrase: bool,
    /// Real speech reached the stream, not only silence.
    speaking: bool,
    /// The last audio sent was silence: the next comes sooner.
    keeping_alive: bool,
    /// Silence sent, ms: not speech (`audio_ms` leaves it out).
    silence_ms: u64,
    /// Each queued phrase's text in order (None for a clip), and how many of them the station heard or never will.
    parts: Vec<Option<String>>,
    played: usize,
    /// Why the answer goes on without the station; None while the station waits for it.
    deferred: Option<String>,
    /// The station closed the stream or gave up: nothing more goes to it.
    station_gone: bool,
    /// The answer has ended: nothing more is sent or deferred.
    ended: bool,
    cancel_reason: Option<String>,
    /// Told when the speech ends on the station, once the answer was spoken to its end (the turn timer).
    after_spoken: Option<SpokenHook>,
}

pub struct SpokenAnswer {
    pub options: SpokenAnswerOptions,
    /// Whoever answers a request claims it: only the newest answer ends it.
    pub owner: Owner,
    cancel: CancellationToken,
    /// Fires when the answer is deferred: playing stops waiting for speech.
    deferral: CancellationToken,
    output: Arc<dyn AnswerOutput>,
    synthesize: Synthesize,
    state: Mutex<State>,
    text_done: watch::Sender<bool>,
}

impl SpokenAnswer {
    pub fn new(
        options: SpokenAnswerOptions,
        output: Arc<dyn AnswerOutput>,
        synthesize: Synthesize,
    ) -> Arc<SpokenAnswer> {
        let state = State {
            text: String::new(),
            phrases: 0,
            failed_phrases: 0,
            stream_id: None,
            should_listen: false,
            errors: Vec::new(),
            joiner: OggOpusJoiner::new(),
            text_complete: false,
            responded: false,
            responded_early: false,
            recorder: None,
            observer: None,
            speech_started_at: None,
            stream_opened: None,
            caught_up_ms: 0,
            timeline: Timeline::new(),
            listen: options.listen,
            directives: options.directives.clone(),
            started: None,
            responded_at: None,
            last_sent: None,
            heard_until: None,
            in_phrase: false,
            speaking: false,
            keeping_alive: false,
            silence_ms: 0,
            parts: Vec::new(),
            played: 0,
            deferred: None,
            station_gone: false,
            ended: false,
            cancel_reason: None,
            after_spoken: None,
        };
        Arc::new(SpokenAnswer {
            options,
            owner: new_owner(),
            cancel: CancellationToken::new(),
            deferral: CancellationToken::new(),
            output,
            synthesize,
            state: Mutex::new(state),
            text_done: watch::channel(false).0,
        })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The answer's cancel: the dialog and the synthesis stop with it.
    pub fn token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// Stops it; the first reason given is kept (a deferred answer dropped says why).
    pub fn cancel(&self, reason: &str) {
        {
            let mut state = self.lock();
            if state.cancel_reason.is_none() && !self.cancelled() {
                state.cancel_reason = Some(reason.to_string());
            }
        }
        self.cancel.cancel();
    }

    pub fn cancel_reason(&self) -> Option<String> {
        self.lock().cancel_reason.clone()
    }

    pub fn text(&self) -> String {
        self.lock().text.clone()
    }

    pub fn phrases(&self) -> u32 {
        self.lock().phrases
    }

    pub fn failed_phrases(&self) -> u32 {
        self.lock().failed_phrases
    }

    pub fn errors(&self) -> Vec<String> {
        self.lock().errors.clone()
    }

    pub fn push_error(&self, error: String) {
        self.lock().errors.push(error);
    }

    pub fn stream_id(&self) -> Option<u32> {
        self.lock().stream_id
    }

    pub fn should_listen(&self) -> bool {
        self.lock().should_listen
    }

    /// The commands it carries, added ones included.
    pub fn has_directives(&self) -> bool {
        !self.lock().directives.is_empty()
    }

    /// The speech on its stream, the silence it kept the station waiting in left out.
    pub fn audio_ms(&self) -> u64 {
        let state = self.lock();
        state.joiner.duration_ms().saturating_sub(state.silence_ms)
    }

    /// The silence that kept the station waiting for the speech.
    pub fn silence_ms(&self) -> u64 {
        self.lock().silence_ms
    }

    /// The silence that brought the stream up to the station's clock before late audio (`catch_up`).
    pub fn caught_up_ms(&self) -> u64 {
        self.lock().caught_up_ms
    }

    pub fn timeline_summary(&self) -> String {
        self.lock().timeline.summary()
    }

    pub fn timeline_json(&self) -> Value {
        serde_json::to_value(&self.lock().timeline).unwrap_or(Value::Null)
    }

    /// How long the station still speaks this answer: we send its audio faster than it is played.
    pub fn playback_left_ms(&self, now: u64) -> u64 {
        let state = self.lock();
        let Some(started) = state.speech_started_at else { return 0 };
        (started + state.joiner.duration_ms()).saturating_sub(now)
    }

    /// When it began to play (tokio's clock): a deferred answer's words are wanted only so long after.
    pub fn started_at(&self) -> Option<Instant> {
        self.lock().started
    }

    /// Why it went on without the station, once it has.
    pub fn deferred_reason(&self) -> Option<String> {
        self.lock().deferred.clone()
    }

    pub fn is_deferred(&self) -> bool {
        self.lock().deferred.is_some()
    }

    /// Commands found while the answer was being prepared (a tool the model called); only before the response.
    pub fn add_directives(&self, directives: Vec<Directive>) -> Result<(), &'static str> {
        let mut state = self.lock();
        if state.responded {
            return Err("the response has already been sent");
        }
        state.directives.extend(directives);
        Ok(())
    }

    /// A different listening for this answer (a game asks for follow-ups): before the response, or after an early
    /// one, which decided the station's listening already; the finished answer still decides the follow-up.
    pub fn set_listen(&self, listen: ListenMode) -> Result<(), &'static str> {
        let mut state = self.lock();
        if state.responded && !state.responded_early {
            return Err("the response has already been sent");
        }
        state.listen = listen;
        Ok(())
    }

    /// The response with the jingle now, before any text, unless it is out: our skill is slow to answer.
    pub fn respond_early(&self) {
        let mut state = self.lock();
        if state.responded || self.options.response.is_none() || self.cancelled() || state.deferred.is_some() {
            return;
        }
        self.respond(&mut state, true, true);
    }

    /// Follow the answer: its text as it grows, the start of its speech. One observer; a later one replaces it.
    pub fn observe(&self, observer: Arc<dyn AnswerObserver>) {
        self.lock().observer = Some(observer);
    }

    /// Keeps the joined speech as it is played (a told tale); kept only when the answer is spoken to the end.
    pub fn record(&self, recording: Arc<dyn AudioRecording>) {
        self.lock().recorder = Some(Recorder { recording, joiner: OggOpusJoiner::new() });
    }

    /// Who wants to know when this answer's speech ends on the station (the turn timer, plugins/skill.rs).
    pub fn after_spoken(&self, hook: SpokenHook) {
        self.lock().after_spoken = Some(hook);
    }

    /// That hook, taken once: the session calls it when the answer was spoken to its end.
    pub fn take_after_spoken(&self) -> Option<SpokenHook> {
        self.lock().after_spoken.take()
    }

    /// Whether the AliceResponse is out, so commands can no longer join it.
    pub fn has_responded(&self) -> bool {
        self.lock().responded
    }

    /// Whether a follow-up without the wake word is part of this answer: always — after any spoken answer;
    /// question — when the finished text ends with a question. Known once the text is complete.
    pub fn expects_follow_up(&self) -> bool {
        let state = self.lock();
        if !state.should_listen || !state.text_complete || state.deferred.is_some() {
            return false;
        }
        match state.listen {
            ListenMode::Always => true,
            ListenMode::Question => ends_with_question(&state.text),
            ListenMode::Never => false,
        }
    }

    /// The station closed the answer's stream: nothing more goes to it. Whether it gave up (and the answer is
    /// deferred) or the answer stops is the session's to decide.
    pub fn station_closed_stream(&self) {
        self.lock().station_gone = true;
    }

    /// The station gave up on this answer (its requestStat: a timeout of its own, its audio client gone): nothing
    /// more goes to its stream, and the answer goes on to be said later. False when it cannot be deferred (it has
    /// ended, or is not deferrable): the session stops it then.
    pub fn station_gave_up(&self, why: &str) -> bool {
        let mut state = self.lock();
        if state.ended {
            return false;
        }
        state.station_gone = true;
        self.defer(&mut state, format!("the station gave up: {why}"))
    }

    /// The words the station has not heard, once the whole text is there: None when the answer was stopped first.
    /// A phrase cut off by the deferral is said again whole.
    pub async fn deferred_text(&self) -> Option<String> {
        let mut done = self.text_done.subscribe();
        tokio::select! {
            _ = self.cancel.cancelled() => return None,
            result = done.wait_for(|done| *done) => {
                if result.is_err() {
                    return None;
                }
            }
        }
        if self.cancelled() {
            return None;
        }
        let state = self.lock();
        let words: Vec<&str> = state.parts.iter().skip(state.played).flatten().map(String::as_str).collect();
        Some(words.join(" "))
    }

    pub async fn play(self: &Arc<Self>, phrases: AnswerParts) -> AnswerOutcome {
        {
            let mut state = self.lock();
            let now = Instant::now();
            state.started = Some(now);
            // Speech the station asked for itself waits for its audio from now: it gets no response.
            if self.options.response.is_none() {
                state.responded_at = Some(now);
            }
        }
        if self.options.early {
            self.respond_early();
        }
        let deadline = self.options.response.is_some().then(|| tokio::spawn(self.clone().keep_the_station_waiting()));
        let keep_alive = tokio::spawn(self.clone().keep_alive());
        let (queue, mut queued) = mpsc::unbounded_channel::<QueuedPhrase>();
        let collecting = tokio::spawn(self.clone().collect(phrases, queue));
        loop {
            let phrase = tokio::select! {
                biased;
                _ = self.deferral.cancelled() => None,
                _ = self.cancel.cancelled() => None,
                phrase = queued.recv() => phrase,
            };
            let Some(mut phrase) = phrase else { break };
            if self.cancelled() || self.deferral.is_cancelled() {
                phrase.synthesis.abort();
                break;
            }
            let synthesis = tokio::select! {
                biased;
                _ = self.deferral.cancelled() => {
                    phrase.synthesis.abort();
                    break;
                }
                _ = self.cancel.cancelled() => {
                    phrase.synthesis.abort();
                    break;
                }
                synthesis = &mut phrase.synthesis => synthesis,
            };
            let synthesis = match synthesis {
                Ok(Ok(synthesis)) => Ok(synthesis),
                Ok(Err(error)) => Err(describe(&error)),
                Err(error) => Err(error.to_string()),
            };
            let synthesis = match synthesis {
                Ok(synthesis) => synthesis,
                Err(error) => {
                    if self.cancelled() {
                        break;
                    }
                    let mut state = self.lock();
                    state.failed_phrases += 1;
                    state.errors.push(format!("speech for \"{}\": {error}", head(&phrase.text, 40)));
                    // Never to be heard: not said later either.
                    state.played += 1;
                    continue;
                }
            };
            if !self.lock().speaking {
                let responded = {
                    let mut state = self.lock();
                    state.timeline.mark("firstAudio");
                    state.responded
                };
                if !responded {
                    let mut done = self.text_done.subscribe();
                    let hold = ms(self.options.hold_response_ms);
                    let _ = tokio::time::timeout(hold, done.wait_for(|done| *done)).await;
                }
                if self.cancelled() || self.deferral.is_cancelled() {
                    break;
                }
                let observer = {
                    let mut state = self.lock();
                    // Silence may have opened the stream already.
                    if state.stream_id.is_none() {
                        self.open_stream(&mut state);
                    }
                    state.speaking = true;
                    state.observer.clone()
                };
                if let Some(observer) = observer {
                    observer.speaking();
                }
            }
            self.play_phrase(synthesis, &phrase.text).await;
            let mut state = self.lock();
            if state.deferred.is_none() && !state.station_gone {
                state.played += 1;
            }
        }
        keep_alive.abort();
        let deferred = {
            let mut state = self.lock();
            state.ended = true;
            state.deferred.is_some()
        };
        if deferred {
            // The rest of the text comes in the background; `deferred_text` hands it on.
            if let Some(deadline) = deadline {
                deadline.abort();
            }
            return AnswerOutcome::Deferred;
        }
        let _ = collecting.await;
        if let Some(deadline) = deadline {
            deadline.abort();
        }
        let mut state = self.lock();
        if self.cancelled() {
            if let Some(stream_id) = state.stream_id {
                self.output.send(ServerMessage::StreamClose { stream_id, reason: CloseReason::Cancel });
            }
            if let Some(recorder) = &state.recorder {
                recorder.recording.finish(false);
            }
            return AnswerOutcome::Cancelled;
        }
        let said = if state.text.is_empty() { AnswerOutcome::Empty } else { AnswerOutcome::Silent };
        let Some(stream_id) = state.stream_id else {
            if state.responded {
                // The early response promised speech: an empty stream, so the station does not wait for it.
                self.start_stream(&mut state);
                if let Some(stream_id) = state.stream_id {
                    self.output.send(ServerMessage::StreamClose { stream_id, reason: CloseReason::Ok });
                }
                return said;
            }
            // Nothing could be voiced: the text alone, so the station is not left waiting.
            self.respond(&mut state, false, false);
            return said;
        };
        let last = state.joiner.finish().unwrap_or_default();
        if !state.station_gone {
            self.output.send_audio(stream_id, &last);
        }
        let complete = state.failed_phrases == 0 && !state.station_gone;
        if let Some(recorder) = state.recorder.as_mut() {
            let last = recorder.joiner.finish().unwrap_or_default();
            recorder.recording.write(&last);
            recorder.recording.finish(complete);
        }
        if !state.station_gone {
            self.output.send(ServerMessage::StreamClose { stream_id, reason: CloseReason::Ok });
        }
        state.timeline.mark("audioEnd");
        if state.speaking {
            AnswerOutcome::Spoken
        } else {
            said
        }
    }

    /// The station waits RESPONSE_DEADLINE_MS for the response, then says an error of its own in Alice's voice
    /// (requestStat «vins:Timeout: Vins request timeout elapsed: 15000 ms»). Live 06.11: our own voice took 17 s to
    /// voice «Будильник на 17:47.», whose text was ready in 1.4 s. A response not out well before that goes early:
    /// the station hears the jingle and waits for the speech that follows.
    async fn keep_the_station_waiting(self: Arc<Self>) {
        tokio::time::sleep(ms(RESPONSE_DEADLINE_MS)).await;
        let mut state = self.lock();
        if state.responded || self.cancelled() || state.deferred.is_some() || state.ended {
            return;
        }
        state.timeline.mark("responseDeadline");
        self.respond(&mut state, true, true);
    }

    /// While the station waits for speech (the response is out, no phrase is streaming), silence on the stream keeps
    /// it from giving up (STATION_SPEECH_TIMEOUT_MS): the first once it has had nothing to play for 3 s, then one
    /// every 1.5 s, and never 6 s without audio. The spotter hears the wake word during it, as during any speech.
    async fn keep_alive(self: Arc<Self>) {
        let silence = SILENCE.as_deref();
        loop {
            let next = {
                let mut state = self.lock();
                match self.keep_alive_step(&mut state, silence) {
                    Some(next) => next,
                    None => return,
                }
            };
            tokio::select! {
                _ = self.cancel.cancelled() => return,
                _ = tokio::time::sleep_until(next) => {}
            }
        }
    }

    /// One look at the stream: silence when it is due, the end of the wait past `wait_max_ms`. When to look again,
    /// or None when there is nothing more to keep.
    fn keep_alive_step(&self, state: &mut State, silence: Option<&[u8]>) -> Option<Instant> {
        if state.ended || state.deferred.is_some() || state.station_gone || self.cancelled() {
            return None;
        }
        let now = Instant::now();
        let poll = now + ms(KEEP_ALIVE_POLL_MS);
        // Before the response the station waits for it, not for speech (keep_the_station_waiting); a phrase that
        // is streaming is audio enough.
        let Some(responded_at) = state.responded_at else { return Some(poll) };
        if state.in_phrase {
            return Some(poll);
        }
        let last_sent = state.last_sent.unwrap_or(responded_at);
        let heard_until = state.heard_until.unwrap_or(responded_at).max(last_sent);
        let quiet = if state.keeping_alive { KEEP_ALIVE_AGAIN_MS } else { KEEP_ALIVE_QUIET_MS };
        let due = (heard_until + ms(quiet)).min(last_sent + ms(KEEP_ALIVE_SEND_MAX_MS));
        if now < due {
            return Some(due);
        }
        // The station waits for the next speech: so long and no longer.
        let started = state.started.unwrap_or(responded_at);
        if now >= started + ms(self.options.wait_max_ms) {
            state.timeline.mark("waitedMax");
            let why = format!("no speech in {:.0} s", self.options.wait_max_ms as f64 / 1000.0);
            if !self.defer(state, why.clone()) {
                // Speech that cannot come later stops here (its stream closes as cancelled).
                state.errors.push(why.clone());
                if state.cancel_reason.is_none() {
                    state.cancel_reason = Some(why);
                }
                self.cancel.cancel();
            }
            return None;
        }
        // No silence to send (the encoder failed): the wait still ends at its limit.
        let Some(silence) = silence else { return Some(now + ms(KEEP_ALIVE_AGAIN_MS)) };
        if let Err(error) = self.send_silence(state, silence, now) {
            state.errors.push(format!("keep-alive: {}", describe(&error)));
            return None;
        }
        Some(now)
    }

    /// Silence on the stream, opening it if it is not open yet (the TtsSpeak goes first).
    fn send_silence(&self, state: &mut State, silence: &[u8], now: Instant) -> Result<()> {
        if state.stream_id.is_none() {
            self.open_stream(state);
        }
        state.timeline.mark("keepAlive");
        let stream_id = state.stream_id.unwrap_or(0);
        let before = state.joiner.duration_ms();
        state.joiner.begin_segment()?;
        let pages = state.joiner.push(silence)?;
        if !pages.is_empty() {
            self.output.send_audio(stream_id, &pages);
        }
        state.silence_ms += state.joiner.duration_ms().saturating_sub(before);
        self.sent(state, before, now);
        state.keeping_alive = true;
        Ok(())
    }

    /// Audio went to the stream: the station has it to play after what it had.
    fn sent(&self, state: &mut State, before_ms: u64, now: Instant) {
        let added = state.joiner.duration_ms().saturating_sub(before_ms);
        state.heard_until = Some(state.heard_until.unwrap_or(now).max(now) + ms(added));
        state.last_sent = Some(now);
        state.keeping_alive = false;
    }

    /// Before a phrase goes to the stream: when the stream is behind the station's clock (it has played everything and
    /// more time has passed: keep-alive silence, a slow phrase), silence fills it up to the clock plus
    /// `CATCH_UP_LEAD_MS`, so the phrase is not skipped. That silence comes late itself and is skipped by the station;
    /// it only moves the phrase's place ahead. Keep-alive silence needs none: it is there to be received, not heard.
    fn catch_up(&self, state: &mut State, now: Instant) -> Result<()> {
        let (Some(opened), Some(stream_id)) = (state.stream_opened, state.stream_id) else { return Ok(()) };
        let played = now.saturating_duration_since(opened).as_millis() as u64;
        let before = state.joiner.duration_ms();
        if before >= played {
            return Ok(());
        }
        let Some(silence) = CATCH_UP_SILENCE.as_deref() else { return Ok(()) };
        let target = played + CATCH_UP_LEAD_MS;
        while state.joiner.duration_ms() < target {
            state.joiner.begin_segment()?;
            let pages = state.joiner.push(silence)?;
            if !pages.is_empty() && !state.station_gone {
                self.output.send_audio(stream_id, &pages);
            }
        }
        let added = state.joiner.duration_ms().saturating_sub(before);
        state.silence_ms += added;
        state.caught_up_ms += added;
        state.timeline.mark("caughtUp");
        Ok(())
    }

    /// Goes on without the station: its stream ends with what it has heard (a clean close, unless the station
    /// left it), and the rest of the words wait for `deferred_text`. False when it cannot.
    fn defer(&self, state: &mut State, why: String) -> bool {
        if !self.options.deferrable || state.ended || state.deferred.is_some() || self.cancelled() {
            return false;
        }
        state.deferred = Some(why);
        state.timeline.mark("deferred");
        if let Some(stream_id) = state.stream_id.filter(|_| !state.station_gone) {
            let last = state.joiner.finish().unwrap_or_default();
            if !last.is_empty() {
                self.output.send_audio(stream_id, &last);
            }
            self.output.send(ServerMessage::StreamClose { stream_id, reason: CloseReason::Ok });
        }
        if let Some(recorder) = &state.recorder {
            recorder.recording.finish(false);
        }
        self.deferral.cancel();
        true
    }

    async fn collect(self: Arc<Self>, mut phrases: AnswerParts, queue: mpsc::UnboundedSender<QueuedPhrase>) {
        while let Some(part) = phrases.next().await {
            if self.cancelled() {
                break;
            }
            {
                let mut state = self.lock();
                state.phrases += 1;
                state.timeline.mark("firstPhrase");
            }
            match part {
                AnswerPart::Clip(clip) => {
                    let deferred = {
                        let mut state = self.lock();
                        state.parts.push(None);
                        state.deferred.is_some()
                    };
                    if deferred {
                        continue;
                    }
                    let audio = clip.audio.clone();
                    let synthesis = tokio::spawn(async move {
                        let audio = futures::stream::once(async move { Ok::<Bytes, anyhow::Error>(audio) });
                        Ok(Synthesis { format: "audio/opus".into(), audio: audio.boxed() })
                    });
                    let _ = queue.send(QueuedPhrase { text: format!("[{}]", clip.label), synthesis });
                }
                AnswerPart::Text(text) => {
                    let (observer, so_far, deferred) = {
                        let mut state = self.lock();
                        state.text =
                            if state.text.is_empty() { text.clone() } else { format!("{} {text}", state.text) };
                        state.parts.push(Some(text.clone()));
                        (state.observer.clone(), without_markup(&state.text), state.deferred.is_some())
                    };
                    if let Some(observer) = observer {
                        observer.text(&so_far);
                    }
                    // Deferred: the words wait to be said later, not voiced now.
                    if deferred {
                        continue;
                    }
                    let speech = self.speech_of(&text);
                    let synthesis = tokio::spawn((self.synthesize)(speech, self.cancel.clone()));
                    let _ = queue.send(QueuedPhrase { text, synthesis });
                }
            }
        }
        let mut state = self.lock();
        state.text_complete = true;
        state.timeline.mark("textDone");
        drop(state);
        drop(queue);
        self.text_done.send_replace(true);
    }

    /// What goes to synthesis for a phrase: markup kept or taken out, a whisper when asked.
    fn speech_of(&self, text: &str) -> String {
        let speech = if self.options.markup { text.to_string() } else { without_markup(text) };
        if self.options.whisper && self.options.markup {
            format!("<speaker is_whisper=\"true\"> {speech}")
        } else {
            speech
        }
    }

    /// The stream begins (TtsSpeak): the station plays from now.
    fn open_stream(&self, state: &mut State) {
        self.start_stream(state);
        state.speech_started_at = Some(now_ms());
        state.stream_opened = Some(Instant::now());
    }

    fn start_stream(&self, state: &mut State) {
        if !state.responded {
            self.respond(state, true, false);
        }
        let stream_id = self.output.new_stream_id();
        state.stream_id = Some(stream_id);
        self.output.send(ServerMessage::TtsSpeak {
            r#ref: self.options.r#ref.clone(),
            stream_id,
            format: "audio/opus".into(),
        });
    }

    /// AliceResponse: once per answer, early (before any text) or when the first phrase can be heard.
    fn respond(&self, state: &mut State, voiced: bool, early: bool) {
        state.responded = true;
        state.responded_early = early;
        if state.responded_at.is_none() {
            state.responded_at = Some(Instant::now());
        }
        let Some(response) = &self.options.response else { return };
        let listen = state.listen;
        state.should_listen = voiced
            && if early {
                listen != ListenMode::Never
            } else {
                listen == ListenMode::Always
                    || (listen == ListenMode::Question && state.text_complete && ends_with_question(&state.text))
            };
        let text = without_markup(&state.text);
        let speech = (voiced && !state.text.is_empty()).then(|| self.speech_of(&state.text));
        let directives = self.directives(state, voiced);
        self.output.send(ServerMessage::AliceResponse {
            r#ref: self.options.r#ref.clone(),
            response: AliceResponseModel {
                request_id: response.request_id.clone(),
                response_id: new_message_id(),
                sequence_number: response.sequence_number,
                dialog_id: response.dialog_id.clone(),
                text: (!text.is_empty()).then_some(text),
                speech,
                has_voice: Some(voiced),
                should_listen: Some(state.should_listen),
                directives,
            },
        });
        state.timeline.mark("responseSent");
    }

    fn directives(&self, state: &State, voiced: bool) -> Vec<Directive> {
        let mut commands = Vec::new();
        if self.options.whisper {
            let mut payload = Map::new();
            payload.insert("new_level".into(), json!(3));
            payload.insert("is_whisper".into(), json!(true));
            let mut level = Directive::client("sound_set_level", payload);
            level.is_led_silent = Some(true);
            commands.push(level);
        }
        commands.extend(state.directives.iter().cloned());
        // A response with nothing to say or do (a stop that only interrupts speech) stays silent.
        if !self.options.jingle || (!voiced && commands.is_empty()) {
            return commands;
        }
        // Before audio_play Yandex puts the jingle first, on the Content channel; otherwise last.
        if commands.iter().any(|directive| directive.name == "audio_play") {
            let mut first = vec![placeholder_directive("Content")];
            first.extend(commands);
            return first;
        }
        commands.push(placeholder_directive("Dialog"));
        commands
    }

    /// A phrase's audio into the stream (and a recording's own joiner), until it ends, the answer stops or the
    /// station no longer listens.
    async fn play_phrase(&self, synthesis: Synthesis, text: &str) {
        let stream_id = {
            let mut state = self.lock();
            if state.deferred.is_some() || state.station_gone {
                return;
            }
            if let Err(error) = self.catch_up(&mut state, Instant::now()) {
                state.errors.push(format!("catching up before \"{}\": {}", head(text, 40), describe(&error)));
            }
            let _ = state.joiner.begin_segment();
            if let Some(recorder) = state.recorder.as_mut() {
                let _ = recorder.joiner.begin_segment();
            }
            state.in_phrase = true;
            state.stream_id.unwrap_or(0)
        };
        let mut audio = synthesis.audio;
        while let Some(chunk) = audio.next().await {
            if self.cancelled() {
                break;
            }
            let mut state = self.lock();
            if state.deferred.is_some() || state.station_gone {
                break;
            }
            let before = state.joiner.duration_ms();
            let pages = match chunk {
                Ok(chunk) => state.joiner.push(&chunk).map(|pages| (pages, chunk)),
                Err(error) => Err(error),
            };
            match pages {
                Ok((pages, chunk)) => {
                    if !pages.is_empty() {
                        self.output.send_audio(stream_id, &pages);
                        self.sent(&mut state, before, Instant::now());
                    }
                    if let Some(recorder) = state.recorder.as_mut() {
                        if let Ok(pages) = recorder.joiner.push(&chunk) {
                            if !pages.is_empty() {
                                recorder.recording.write(&pages);
                            }
                        }
                    }
                }
                Err(error) => {
                    state.failed_phrases += 1;
                    state.errors.push(format!("audio for \"{}\": {}", head(text, 40), describe(&error)));
                    break;
                }
            }
        }
        self.lock().in_phrase = false;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use futures::FutureExt;

    use super::*;
    use crate::speech::audio::ogg_opus_demuxer::{OggOpusDemuxer, OggOpusEvent};
    use crate::speech::test_audio::{ogg_opus_chain, ogg_pages, voice};

    #[derive(Debug, Clone, PartialEq)]
    enum Sent {
        Message(ServerMessage),
        Audio(u32, Vec<u8>),
    }

    struct Output {
        sent: Mutex<Vec<Sent>>,
        /// When each was sent, on tokio's clock.
        at: Mutex<Vec<Instant>>,
        next: AtomicU32,
    }

    impl Output {
        fn new() -> Arc<Output> {
            Arc::new(Output { sent: Mutex::new(Vec::new()), at: Mutex::new(Vec::new()), next: AtomicU32::new(2) })
        }

        /// When each audio frame went out.
        fn audio_times(&self) -> Vec<Instant> {
            let sent = self.sent.lock().unwrap();
            let at = self.at.lock().unwrap();
            sent.iter().zip(at.iter()).filter(|(sent, _)| matches!(sent, Sent::Audio(..))).map(|(_, at)| *at).collect()
        }

        fn time_of(&self, matches: impl Fn(&ServerMessage) -> bool) -> Instant {
            let sent = self.sent.lock().unwrap();
            let at = self.at.lock().unwrap();
            sent.iter()
                .zip(at.iter())
                .find_map(|(sent, at)| match sent {
                    Sent::Message(message) if matches(message) => Some(*at),
                    _ => None,
                })
                .expect("the message")
        }

        /// Message kinds in order, a run of audio frames as one entry.
        fn kinds(&self) -> Vec<&'static str> {
            let mut kinds: Vec<&'static str> = Vec::new();
            for sent in self.sent.lock().unwrap().iter() {
                let kind = match sent {
                    Sent::Audio(..) => "audio",
                    Sent::Message(ServerMessage::AliceResponse { .. }) => "aliceResponse",
                    Sent::Message(ServerMessage::TtsSpeak { .. }) => "ttsSpeak",
                    Sent::Message(ServerMessage::StreamClose { .. }) => "streamClose",
                    Sent::Message(_) => "other",
                };
                if kind != "audio" || kinds.last() != Some(&"audio") {
                    kinds.push(kind);
                }
            }
            kinds
        }

        fn response(&self) -> AliceResponseModel {
            self.sent
                .lock()
                .unwrap()
                .iter()
                .find_map(|sent| match sent {
                    Sent::Message(ServerMessage::AliceResponse { response, .. }) => Some(response.clone()),
                    _ => None,
                })
                .expect("a response")
        }

        fn find(&self, matches: impl Fn(&ServerMessage) -> bool) -> ServerMessage {
            self.sent
                .lock()
                .unwrap()
                .iter()
                .find_map(|sent| match sent {
                    Sent::Message(message) if matches(message) => Some(message.clone()),
                    _ => None,
                })
                .expect("the message")
        }

        fn audio(&self) -> Vec<u8> {
            self.sent
                .lock()
                .unwrap()
                .iter()
                .filter_map(|sent| match sent {
                    Sent::Audio(_, bytes) => Some(bytes.clone()),
                    _ => None,
                })
                .flatten()
                .collect()
        }

        fn len(&self) -> usize {
            self.sent.lock().unwrap().len()
        }

        fn last(&self) -> Sent {
            self.sent.lock().unwrap().last().cloned().unwrap()
        }
    }

    impl AnswerOutput for Output {
        fn send(&self, message: ServerMessage) {
            self.sent.lock().unwrap().push(Sent::Message(message));
            self.at.lock().unwrap().push(Instant::now());
        }
        fn send_audio(&self, stream_id: u32, bytes: &[u8]) {
            self.sent.lock().unwrap().push(Sent::Audio(stream_id, bytes.to_vec()));
            self.at.lock().unwrap().push(Instant::now());
        }
        fn new_stream_id(&self) -> u32 {
            self.next.fetch_add(2, Ordering::SeqCst)
        }
    }

    async fn sleep(ms: u64) {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }

    /// A phrase's Ogg/Opus as Yandex streams it: pages in a few network chunks. 20 ms packets, 10 per 200 ms.
    fn phrase_audio(ms: f64, pause_ms: u64) -> BoxStream<'static, Result<Bytes>> {
        let chain = ogg_opus_chain(&voice(ms, -20.0), 0x77);
        let chunks: Vec<Bytes> = ogg_pages(&chain).chunks(4).map(|pages| Bytes::from(pages.concat())).collect();
        futures::stream::iter(chunks)
            .then(move |chunk| async move {
                sleep(pause_ms).await;
                Ok(chunk)
            })
            .boxed()
    }

    fn speak_all() -> Synthesize {
        Arc::new(|_text, _cancel| {
            async { Ok(Synthesis { format: "audio/opus".into(), audio: phrase_audio(200.0, 2) }) }.boxed()
        })
    }

    /// Records what goes to synthesis.
    fn spoken_into(spoken: Arc<Mutex<Vec<String>>>) -> Synthesize {
        Arc::new(move |text, _cancel| {
            spoken.lock().unwrap().push(text);
            async { Ok(Synthesis { format: "audio/opus".into(), audio: phrase_audio(200.0, 2) }) }.boxed()
        })
    }

    fn failing(error: &'static str) -> Synthesize {
        Arc::new(move |_text, _cancel| async move { Err(anyhow::anyhow!(error)) }.boxed())
    }

    fn phrases(texts: &[&str]) -> AnswerParts {
        let texts: Vec<String> = texts.iter().map(|text| text.to_string()).collect();
        futures::stream::iter(texts)
            .then(|text| async move {
                sleep(5).await;
                AnswerPart::Text(text)
            })
            .boxed()
    }

    fn options() -> SpokenAnswerOptions {
        SpokenAnswerOptions {
            r#ref: "voice-1".into(),
            response: Some(ResponseHeader { request_id: "req-1".into(), sequence_number: Some(3), dialog_id: None }),
            hold_response_ms: 300,
            listen: ListenMode::Question,
            early: false,
            jingle: false,
            directives: Vec::new(),
            whisper: false,
            markup: false,
            wait_max_ms: WAIT_MAX_MS,
            deferrable: true,
        }
    }

    fn names(directives: &[Directive]) -> Vec<&str> {
        directives.iter().map(|directive| directive.name.as_str()).collect()
    }

    #[tokio::test]
    async fn sends_the_response_then_all_phrases_as_one_audio_stream_then_closes_it() {
        let output = Output::new();
        let answer = SpokenAnswer::new(options(), output.clone(), speak_all());
        assert_eq!(answer.play(phrases(&["Сейчас половина восьмого.", "Хорошего дня!"])).await, AnswerOutcome::Spoken);

        assert_eq!(output.kinds(), vec!["aliceResponse", "ttsSpeak", "audio", "streamClose"]);
        // The text was complete within the hold, so the response carries all of it.
        let response = output.response();
        assert_eq!((response.request_id.as_str(), response.sequence_number), ("req-1", Some(3)));
        assert_eq!(response.speech.as_deref(), Some("Сейчас половина восьмого. Хорошего дня!"));
        assert_eq!(response.should_listen, Some(false));
        let speak = output.find(|message| matches!(message, ServerMessage::TtsSpeak { .. }));
        assert_eq!(
            speak,
            ServerMessage::TtsSpeak { r#ref: "voice-1".into(), stream_id: 2, format: "audio/opus".into() }
        );
        let close = output.find(|message| matches!(message, ServerMessage::StreamClose { .. }));
        assert_eq!(close, ServerMessage::StreamClose { stream_id: 2, reason: CloseReason::Ok });

        let events = OggOpusDemuxer::new().push(&output.audio());
        assert_eq!(events.iter().filter(|event| matches!(event, OggOpusEvent::Head { .. })).count(), 1);
        assert_eq!(events.iter().filter(|event| matches!(event, OggOpusEvent::Packet { .. })).count(), 20);
        assert_eq!(answer.audio_ms(), 400);
    }

    #[tokio::test]
    async fn keeps_the_station_listening_after_an_answer_that_asks_something() {
        let output = Output::new();
        let answer = SpokenAnswer::new(options(), output.clone(), speak_all());
        answer.play(phrases(&["Какой город вас интересует?"])).await;
        assert_eq!(output.response().should_listen, Some(true));
        assert!(answer.expects_follow_up());
    }

    #[tokio::test]
    async fn in_always_mode_keeps_listening_after_any_answer_as_the_generative_alice() {
        let output = Output::new();
        let answer = SpokenAnswer::new(
            SpokenAnswerOptions { listen: ListenMode::Always, ..options() },
            output.clone(),
            speak_all(),
        );
        answer.play(phrases(&["Сейчас половина восьмого."])).await;
        assert_eq!(output.response().should_listen, Some(true));
        assert!(answer.expects_follow_up());
    }

    #[tokio::test]
    async fn answers_early_with_the_jingle_before_any_text_and_the_speech_follows() {
        let output = Output::new();
        let answer = SpokenAnswer::new(
            SpokenAnswerOptions { early: true, jingle: true, ..options() },
            output.clone(),
            speak_all(),
        );
        let playing = tokio::spawn({
            let answer = answer.clone();
            async move { answer.play(phrases(&["Сейчас половина восьмого."])).await }
        });
        // The response is out before the dialog has said anything.
        for _ in 0..100 {
            if !output.kinds().is_empty() {
                break;
            }
            sleep(1).await;
        }
        assert_eq!(output.kinds(), vec!["aliceResponse"]);
        let response = output.response();
        assert_eq!(
            (response.has_voice, response.text.as_deref(), response.speech.as_deref()),
            (Some(true), None, None)
        );
        // question mode: listening is asked for before the text is known…
        assert_eq!(response.should_listen, Some(true));
        assert_eq!(names(&response.directives), vec!["tts_play_placeholder"]);
        assert_eq!(response.directives[0].payload.as_ref().unwrap()["channel"], "Dialog");
        assert_eq!(playing.await.unwrap(), AnswerOutcome::Spoken);
        assert_eq!(output.kinds(), vec!["aliceResponse", "ttsSpeak", "audio", "streamClose"]);
        // …and the session closes it, as the answer did not ask anything.
        assert!(!answer.expects_follow_up());
    }

    #[tokio::test]
    async fn answers_early_on_demand_when_our_skill_is_slow_with_the_jingle_and_without_listening() {
        let output = Output::new();
        let answer = SpokenAnswer::new(
            SpokenAnswerOptions { listen: ListenMode::Never, jingle: true, ..options() },
            output.clone(),
            speak_all(),
        );
        let slow = futures::stream::once(async {
            sleep(50).await;
            AnswerPart::Text("Москва: сейчас плюс десять, ясно.".into())
        })
        .boxed();
        let playing = tokio::spawn({
            let answer = answer.clone();
            async move { answer.play(slow).await }
        });
        answer.respond_early();
        assert_eq!(output.kinds(), vec!["aliceResponse"]);
        let response = output.response();
        assert_eq!((response.has_voice, response.should_listen), (Some(true), Some(false)));
        assert_eq!(names(&response.directives), vec!["tts_play_placeholder"]);
        // A game's listening may still be set after an early response.
        assert!(answer.set_listen(ListenMode::Always).is_ok());
        assert_eq!(playing.await.unwrap(), AnswerOutcome::Spoken);
        assert_eq!(output.kinds(), vec!["aliceResponse", "ttsSpeak", "audio", "streamClose"]);
    }

    /// Live 06.11: our own voice took 17 s for a phrase whose text was ready at once; the station gave up at 15 s
    /// and said an error in Alice's voice. The response goes at the deadline, the speech when it is ready.
    #[tokio::test(start_paused = true)]
    async fn sends_the_response_before_the_station_gives_up_on_a_slow_voice() {
        let output = Output::new();
        let slow: Synthesize = Arc::new(|_text, _cancel| {
            async {
                sleep(17_000).await;
                Ok(Synthesis { format: "audio/opus".into(), audio: phrase_audio(200.0, 2) })
            }
            .boxed()
        });
        let answer =
            SpokenAnswer::new(SpokenAnswerOptions { listen: ListenMode::Never, ..options() }, output.clone(), slow);
        let playing = tokio::spawn({
            let answer = answer.clone();
            async move { answer.play(phrases(&["Будильник на 17:47."])).await }
        });
        sleep(RESPONSE_DEADLINE_MS - 100).await;
        assert!(output.kinds().is_empty(), "nothing before the deadline");
        sleep(200).await;
        assert_eq!(output.kinds(), vec!["aliceResponse"]);
        let response = output.response();
        assert_eq!((response.has_voice, response.text.as_deref()), (Some(true), Some("Будильник на 17:47.")));
        assert_eq!(playing.await.unwrap(), AnswerOutcome::Spoken);
        assert_eq!(output.kinds(), vec!["aliceResponse", "ttsSpeak", "audio", "streamClose"]);
    }

    #[tokio::test]
    async fn after_an_early_response_plays_an_empty_stream_when_nothing_can_be_voiced() {
        let output = Output::new();
        let answer =
            SpokenAnswer::new(SpokenAnswerOptions { early: true, ..options() }, output.clone(), failing("no TtsSpeak"));
        assert_eq!(answer.play(phrases(&["Ответ без голоса."])).await, AnswerOutcome::Silent);
        assert_eq!(output.kinds(), vec!["aliceResponse", "ttsSpeak", "streamClose"]);
    }

    #[tokio::test]
    async fn carries_station_commands_in_the_response_the_jingle_after_them() {
        let output = Output::new();
        let mut payload = Map::new();
        payload.insert("new_level".into(), json!(4));
        let directives = vec![Directive::client("sound_set_level", payload)];
        let answer = SpokenAnswer::new(
            SpokenAnswerOptions { jingle: true, directives, ..options() },
            output.clone(),
            speak_all(),
        );
        answer.play(phrases(&["Хорошо."])).await;
        assert_eq!(names(&output.response().directives), vec!["sound_set_level", "tts_play_placeholder"]);
    }

    #[tokio::test]
    async fn answers_a_command_without_speech_with_its_directives_alone() {
        let output = Output::new();
        let directives = vec![Directive::client("sound_louder", Map::new())];
        let answer = SpokenAnswer::new(
            SpokenAnswerOptions { directives: directives.clone(), ..options() },
            output.clone(),
            speak_all(),
        );
        assert_eq!(answer.play(phrases(&[])).await, AnswerOutcome::Empty);
        assert_eq!(output.kinds(), vec!["aliceResponse"]);
        let response = output.response();
        assert_eq!((response.has_voice, response.should_listen), (Some(false), Some(false)));
        assert_eq!(response.directives, directives);
    }

    #[tokio::test]
    async fn skips_a_phrase_that_cannot_be_voiced_and_speaks_the_rest() {
        let output = Output::new();
        let synthesize: Synthesize = Arc::new(|text, _cancel| {
            async move {
                if text.starts_with("Первая") {
                    anyhow::bail!("no TtsSpeak");
                }
                Ok(Synthesis { format: "audio/opus".into(), audio: phrase_audio(200.0, 2) })
            }
            .boxed()
        });
        let answer = SpokenAnswer::new(options(), output.clone(), synthesize);
        assert_eq!(
            answer.play(phrases(&["Первая фраза ответа.", "Вторая фраза ответа."])).await,
            AnswerOutcome::Spoken
        );
        assert_eq!(answer.failed_phrases(), 1);
        let errors = answer.errors();
        assert!(errors[0].contains("Первая") && errors[0].contains("no TtsSpeak"), "{errors:?}");
        assert_eq!(output.kinds(), vec!["aliceResponse", "ttsSpeak", "audio", "streamClose"]);
    }

    #[tokio::test]
    async fn answers_with_the_text_alone_when_nothing_can_be_voiced() {
        let output = Output::new();
        let answer = SpokenAnswer::new(options(), output.clone(), failing("Yandex is unreachable"));
        assert_eq!(answer.play(phrases(&["Ответ без голоса."])).await, AnswerOutcome::Silent);
        assert_eq!(output.kinds(), vec!["aliceResponse"]);
        let response = output.response();
        assert_eq!((response.text.as_deref(), response.speech.as_deref()), (Some("Ответ без голоса."), None));
    }

    #[tokio::test]
    async fn stops_at_once_when_cancelled_and_closes_the_stream_as_cancelled() {
        let output = Output::new();
        let slow: Synthesize = Arc::new(|_text, _cancel| {
            async { Ok(Synthesis { format: "audio/opus".into(), audio: phrase_audio(2000.0, 22) }) }.boxed()
        });
        let answer = SpokenAnswer::new(options(), output.clone(), slow);
        let playing = tokio::spawn({
            let answer = answer.clone();
            async move { answer.play(phrases(&["Длинная история про колонку."])).await }
        });
        sleep(150).await;
        answer.cancel("superseded");
        assert_eq!(playing.await.unwrap(), AnswerOutcome::Cancelled);
        assert_eq!(
            output.last(),
            Sent::Message(ServerMessage::StreamClose { stream_id: 2, reason: CloseReason::Cancel })
        );
        let sent_before = output.len();
        sleep(100).await;
        assert_eq!(output.len(), sent_before);
    }

    #[tokio::test]
    async fn speaks_for_the_station_without_an_alice_response() {
        let output = Output::new();
        let answer = SpokenAnswer::new(SpokenAnswerOptions::speech_only("tts-1"), output.clone(), speak_all());
        answer.play(phrases(&["Привет."])).await;
        assert_eq!(output.kinds(), vec!["ttsSpeak", "audio", "streamClose"]);
        let speak = output.find(|message| matches!(message, ServerMessage::TtsSpeak { .. }));
        assert!(matches!(speak, ServerMessage::TtsSpeak { r#ref, .. } if r#ref == "tts-1"));
    }

    #[tokio::test]
    async fn answers_a_whisper_at_a_lowered_volume_and_as_a_whisper_when_markup_is_on() {
        let output = Output::new();
        let spoken = Arc::new(Mutex::new(Vec::new()));
        let answer = SpokenAnswer::new(
            SpokenAnswerOptions { whisper: true, markup: true, ..options() },
            output.clone(),
            spoken_into(spoken.clone()),
        );
        answer.play(phrases(&["Шестнадцать тридцать пять."])).await;
        let level = &output.response().directives[0];
        assert_eq!(level.name, "sound_set_level");
        assert_eq!(level.payload.as_ref().unwrap()["new_level"], 3);
        assert_eq!(level.payload.as_ref().unwrap()["is_whisper"], true);
        assert_eq!(*spoken.lock().unwrap(), vec!["<speaker is_whisper=\"true\"> Шестнадцать тридцать пять."]);
    }

    #[tokio::test]
    async fn takes_markup_out_of_the_speech_when_it_is_off_and_always_out_of_the_card() {
        let output = Output::new();
        let spoken = Arc::new(Mutex::new(Vec::new()));
        let answer = SpokenAnswer::new(options(), output.clone(), spoken_into(spoken.clone()));
        answer.play(phrases(&["Верно! <speaker audio=\"alice-sounds-game-win-1.opus\"> Следующий вопрос."])).await;
        assert!(!spoken.lock().unwrap().join(" ").contains("<speaker"));
        assert_eq!(output.response().text.as_deref(), Some("Верно! Следующий вопрос."));
    }

    /// Parts that come after the given pauses (ms from the previous one).
    fn paced(parts: &[(u64, &str)]) -> AnswerParts {
        let parts: Vec<(u64, String)> = parts.iter().map(|(wait, text)| (*wait, text.to_string())).collect();
        futures::stream::iter(parts)
            .then(|(wait, text)| async move {
                sleep(wait).await;
                AnswerPart::Text(text)
            })
            .boxed()
    }

    /// OpusHead count and packet count of a stream.
    fn heads_and_packets(audio: &[u8]) -> (usize, usize) {
        let events = OggOpusDemuxer::new().push(audio);
        let heads = events.iter().filter(|event| matches!(event, OggOpusEvent::Head { .. })).count();
        (heads, events.iter().filter(|event| matches!(event, OggOpusEvent::Packet { .. })).count())
    }

    fn longest_gap(times: &[Instant]) -> Duration {
        times.windows(2).map(|pair| pair[1] - pair[0]).max().unwrap_or_default()
    }

    /// The station drops speech that keeps it waiting 10 s (STATION_SPEECH_TIMEOUT_MS). A model that takes 8 s to say
    /// anything keeps the stream alive with silence from 3 s after the early response: one stream, one head, the
    /// silence counted apart from the speech.
    #[tokio::test(start_paused = true)]
    async fn keeps_the_station_waiting_with_silence_well_inside_its_10_s_and_counts_it_apart_from_the_speech() {
        let output = Output::new();
        let answer = SpokenAnswer::new(
            SpokenAnswerOptions { early: true, jingle: true, ..options() },
            output.clone(),
            speak_all(),
        );
        assert_eq!(answer.play(paced(&[(8000, "Нашлось вот что.")])).await, AnswerOutcome::Spoken);
        assert_eq!(output.kinds(), vec!["aliceResponse", "ttsSpeak", "audio", "streamClose"]);
        let response = output.time_of(|message| matches!(message, ServerMessage::AliceResponse { .. }));
        let audio = output.audio_times();
        assert_eq!(audio[0] - response, ms(KEEP_ALIVE_QUIET_MS));
        assert!(longest_gap(&audio) <= ms(1500), "{:?}", longest_gap(&audio));
        // Silence at 3, 4.5, 6 and 7.5 s, then the phrase at 8 s: 5 s into the stream, which has 2 s of audio, so
        // 3.2 s more silence puts it 0.2 s ahead of the station's clock.
        assert_eq!((answer.audio_ms(), answer.silence_ms(), answer.caught_up_ms()), (200, 5200, 3200));
        assert_eq!(heads_and_packets(&output.audio()), (1, 10 + 4 * 25 + 160));
        assert!(answer.timeline_summary().contains("keepAlive"));
    }

    #[tokio::test(start_paused = true)]
    async fn puts_a_phrase_that_comes_late_ahead_of_the_stations_clock_not_behind_it() {
        // Live 13.11: silence opened the stream, the speech came 2.8 s later and was placed after 1 s of it, so the
        // station had passed all but its last word.
        let output = Output::new();
        let answer = SpokenAnswer::new(options(), output.clone(), speak_all());
        let parts = paced(&[(0, "Сейчас посмотрю."), (5000, "Нашлось вот что.")]);
        assert_eq!(answer.play(parts).await, AnswerOutcome::Spoken);
        // The first phrase opens the stream at 0.3 s (the response waits that long for the rest of the text), then
        // keep-alive silence; the second phrase at 5 s, 4.7 s into the stream, goes at 4.9 s of it and lasts 0.2 s.
        let length = answer.audio_ms() + answer.silence_ms();
        assert_eq!(length, 5100);
        assert!(answer.caught_up_ms() > 0);
        assert_eq!(answer.audio_ms(), 400);
        assert_eq!(heads_and_packets(&output.audio()).0, 1);
        assert!(answer.timeline_summary().contains("caughtUp"));
    }

    #[tokio::test(start_paused = true)]
    async fn plays_phrases_ready_in_time_without_catching_up() {
        let output = Output::new();
        let answer = SpokenAnswer::new(options(), output.clone(), speak_all());
        assert_eq!(answer.play(phrases(&["Первая фраза ответа.", "Вторая."])).await, AnswerOutcome::Spoken);
        assert_eq!((answer.caught_up_ms(), answer.silence_ms()), (0, 0));
    }

    #[tokio::test(start_paused = true)]
    async fn fills_a_long_gap_between_two_phrases_with_silence() {
        let output = Output::new();
        let answer = SpokenAnswer::new(options(), output.clone(), speak_all());
        let parts = paced(&[(0, "Сейчас посмотрю."), (12_000, "Нашлось вот что.")]);
        assert_eq!(answer.play(parts).await, AnswerOutcome::Spoken);
        let audio = output.audio_times();
        // 3 s after the first phrase has played, then every 1.5 s: never near the station's 10 s.
        assert!(longest_gap(&audio) <= ms(3300), "{:?}", longest_gap(&audio));
        assert_eq!(answer.audio_ms(), 400);
        assert!(answer.silence_ms() >= 3000, "{}", answer.silence_ms());
        assert_eq!(heads_and_packets(&output.audio()).0, 1);
        assert_eq!(output.kinds(), vec!["aliceResponse", "ttsSpeak", "audio", "streamClose"]);
    }

    #[tokio::test(start_paused = true)]
    async fn closes_its_stream_at_the_wait_limit_and_hands_on_the_words_not_heard() {
        let output = Output::new();
        let answer = SpokenAnswer::new(SpokenAnswerOptions { early: true, ..options() }, output.clone(), speak_all());
        let parts = paced(&[(0, "Сейчас посмотрю."), (60_000, "Нашлось вот что.")]);
        let started = Instant::now();
        assert_eq!(answer.play(parts).await, AnswerOutcome::Deferred);
        let waited = Instant::now() - started;
        assert!(waited >= ms(WAIT_MAX_MS) && waited <= ms(WAIT_MAX_MS + 1500), "{waited:?}");
        assert_eq!(output.last(), Sent::Message(ServerMessage::StreamClose { stream_id: 2, reason: CloseReason::Ok }));
        // What the station got is one whole stream.
        assert_eq!(heads_and_packets(&output.audio()).0, 1);
        assert!(answer.deferred_reason().unwrap().contains("no speech in 45 s"));
        let sent = output.len();
        assert_eq!(answer.deferred_text().await.as_deref(), Some("Нашлось вот что."));
        assert_eq!(output.len(), sent, "nothing more goes to the station");
        assert!(!answer.expects_follow_up());
    }

    #[tokio::test(start_paused = true)]
    async fn goes_on_without_the_station_once_it_gives_up_and_sends_it_nothing_more() {
        let output = Output::new();
        let answer = SpokenAnswer::new(SpokenAnswerOptions { early: true, ..options() }, output.clone(), speak_all());
        let playing = tokio::spawn({
            let answer = answer.clone();
            async move { answer.play(paced(&[(5000, "Готово.")])).await }
        });
        sleep(1000).await;
        assert!(answer.station_gave_up("vins:TtsPlayingError: Synthesis start timeout elapsed"));
        assert_eq!(playing.await.unwrap(), AnswerOutcome::Deferred);
        assert_eq!(answer.deferred_text().await.as_deref(), Some("Готово."));
        assert_eq!(output.kinds(), vec!["aliceResponse"]);
    }

    #[tokio::test(start_paused = true)]
    async fn stops_speech_that_cannot_come_later_at_the_wait_limit() {
        let output = Output::new();
        let options = SpokenAnswerOptions { early: true, deferrable: false, wait_max_ms: 20_000, ..options() };
        let answer = SpokenAnswer::new(options, output.clone(), speak_all());
        assert_eq!(answer.play(paced(&[(30_000, "Поздно.")])).await, AnswerOutcome::Cancelled);
        assert_eq!(
            output.last(),
            Sent::Message(ServerMessage::StreamClose { stream_id: 2, reason: CloseReason::Cancel })
        );
        assert_eq!(answer.cancel_reason().as_deref(), Some("no speech in 20 s"));
        assert!(!answer.station_gave_up("too late"), "an ended answer is not deferred");
    }

    /// A told tale being kept.
    #[derive(Default)]
    struct Kept {
        bytes: Mutex<Vec<u8>>,
        complete: Mutex<Option<bool>>,
    }

    impl AudioRecording for Kept {
        fn write(&self, bytes: &[u8]) {
            self.bytes.lock().unwrap().extend_from_slice(bytes);
        }
        fn finish(&self, complete: bool) {
            *self.complete.lock().unwrap() = Some(complete);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn keeps_the_speech_of_a_told_tale_without_the_silence_the_station_waited_in() {
        let output = Output::new();
        let answer = SpokenAnswer::new(SpokenAnswerOptions { early: true, ..options() }, output.clone(), speak_all());
        let kept = Arc::new(Kept::default());
        answer.record(kept.clone());
        assert_eq!(answer.play(paced(&[(5000, "Жили-были."), (0, "Конец.")])).await, AnswerOutcome::Spoken);
        assert!(answer.silence_ms() > 0);
        assert_eq!(heads_and_packets(&kept.bytes.lock().unwrap()), (1, 20));
        assert_eq!(*kept.complete.lock().unwrap(), Some(true));
    }

    #[test]
    fn hears_a_question_and_an_invitation_to_answer() {
        assert!(ends_with_question("Какой город вас интересует?"));
        assert!(ends_with_question("Что вам ближе?» "));
        assert!(ends_with_question(
            "Могу управлять устройствами умного дома. Скажите, что именно хотите включить или проверить."
        ));
        assert!(ends_with_question("Назовите город."));
    }

    #[test]
    fn does_not_hear_one_in_a_statement_or_a_conditional_invitation() {
        assert!(!ends_with_question("Сейчас половина восьмого."));
        assert!(!ends_with_question("Хорошо, остановился. Если захотите вернуться к истории Рима, просто скажите."));
        assert!(!ends_with_question("Скажите? Нет, это просто пример. Отличного дня!"));
        assert!(!ends_with_question("Расскажите-ка лучше вы."));
    }
}
