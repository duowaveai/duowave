//! One request answered by Yandex, relayed to the station: every event under the station's message id, every stream
//! Yandex opens (its speech) under a stream id of ours, the rest of each frame byte for byte. Done when the response
//! is out and every stream it opened has closed; the answer's gist (should_listen, the dialog, the commands, the
//! text) decides where follow-ups go.

use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::Duration;

use anyhow::Result;
use bytes::Bytes;
use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::watch;
use tokio::time::Instant;

use super::relay_trips::recognition_text;
use crate::core::requests::{new_owner, Owner};
use crate::gateway::protocol::proto_edit::{edit_frame, get_bytes, get_string, restream, FrameEdit};
use crate::gateway::protocol::proto_object::{is_js_space, value_bytes};
use crate::gateway::protocol::protocol_types::CloseReason;
use crate::gateway::yandex::yandex_session::ExchangeHandler;

/// What the relay gives the station.
pub trait RelayOutput: Send + Sync {
    /// An event or a stream control frame for the station, journaled under its kind.
    fn send_frame(&self, frame: Bytes, kind: &str, summary: String, decoded: &Value);
    fn send_audio(&self, frame: Bytes);
    fn close_stream(&self, stream_id: u32, reason: CloseReason);
    /// A new even stream id: the station's streams are odd, Yandex's numbering is not ours.
    fn new_stream_id(&self) -> u32;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RelayOutcome {
    Answered,
    Cancelled,
    Timeout,
    Failed,
}

impl RelayOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            RelayOutcome::Answered => "answered",
            RelayOutcome::Cancelled => "cancelled",
            RelayOutcome::Timeout => "timeout",
            RelayOutcome::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct YandexAnswer {
    pub outcome: RelayOutcome,
    /// Yandex's AliceResponse arrived.
    pub responded: bool,
    pub should_listen: bool,
    pub has_voice: bool,
    /// A skill's dialog: its follow-ups go to Yandex until it ends.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dialog_id: Option<String>,
    /// end_dialog_session or close_dialog: the skill let go.
    pub ends_dialog: bool,
    pub directives: Vec<String>,
    /// The answer's text: the card's, the streamed text, or the speech without markup.
    pub text: String,
    /// What Yandex heard: its final recognition of the voice it was given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heard: Option<String>,
    pub streams: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Default for YandexAnswer {
    fn default() -> YandexAnswer {
        YandexAnswer {
            outcome: RelayOutcome::Answered,
            responded: false,
            should_listen: false,
            has_voice: false,
            dialog_id: None,
            ends_dialog: false,
            directives: Vec::new(),
            text: String::new(),
            heard: None,
            streams: 0,
            error: None,
        }
    }
}

pub type OnResponse = Box<dyn Fn(&YandexAnswer) + Send + Sync>;
pub type Revoice = Box<dyn Fn(String) + Send + Sync>;
pub type OnAsr = Box<dyn Fn(&Value) + Send + Sync>;

pub struct YandexRelayOptions {
    /// The station's message id Yandex's replies go to; unchanged when the request was the station's own frame.
    pub station_ref: Option<String>,
    /// Event kinds the station must not get: our own recognition already answered a voice request.
    pub drop: &'static [&'static str],
    /// Collect the answer and send nothing to the station: a lookup for the model.
    pub quiet: bool,
    /// For Yandex's response, then between the frames of each stream.
    pub timeout_ms: u64,
    /// Yandex's response is in: who takes the follow-ups is known before its speech ends.
    pub on_response: Option<OnResponse>,
    /// Yandex's speech said in our voice instead (TTS_PROVIDER=own): its speech stream stays here, and its text, the
    /// response's speech or the streamed text, goes to this when the speech begins.
    pub revoice: Option<Revoice>,
    /// Yandex's recognition of the voice it was given (each AsrResult, partial and final): nothing here heard it, so
    /// the request's words are Yandex's.
    pub on_asr: Option<OnAsr>,
}

impl YandexRelayOptions {
    pub fn new(drop: &'static [&'static str], timeout_ms: u64) -> YandexRelayOptions {
        YandexRelayOptions {
            station_ref: None,
            drop,
            quiet: false,
            timeout_ms,
            on_response: None,
            revoice: None,
            on_asr: None,
        }
    }
}

/// Recognition events of a voice request Yandex hears again: the station already has ours.
pub const RECOGNITION_EVENTS: &[&str] = &["asrResult", "inputStartAck", "spotterValidation", "logAck"];
pub const NO_EVENTS: &[&str] = &[];

/// Markup of Yandex's speech text, left out of the plain text.
static SPEECH_MARKUP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<\[[^\]]*\]>|<speaker[^>]*>|sil<\[[0-9]+\]>|#\p{L}+").expect("a valid pattern"));

/// Yandex's speech text as plain words: its markup out, spaces collapsed.
pub fn plain_speech(text: &str) -> String {
    let spaced = SPEECH_MARKUP.replace_all(text, " ");
    spaced.split(is_js_space).filter(|word| !word.is_empty()).collect::<Vec<_>>().join(" ")
}

/// The texts of a response's cards, joined.
pub fn card_text(response: &Value) -> String {
    let cards = response.get("response").and_then(|body| body.get("cards")).and_then(Value::as_array);
    let texts: Vec<&str> = cards
        .map(|cards| cards.iter().map(|card| card.get("text").and_then(Value::as_str).unwrap_or("")).collect())
        .unwrap_or_default();
    texts.join(" ").trim_matches(is_js_space).to_string()
}

struct State {
    /// Yandex's stream ids to ours; -1 for a stream kept here (a lookup, our voice instead of Alice's).
    streams: Vec<(u32, i64)>,
    /// Yandex's streams still open, in the order they opened.
    open: Vec<u32>,
    answer: YandexAnswer,
    streamed_text: String,
    /// The response's speech without markup.
    speech: String,
    revoiced: bool,
    finished: bool,
    cancelled: bool,
    /// When the armed timeout fires, and what it says then.
    deadline: Instant,
    waiting_for: &'static str,
}

impl State {
    fn ours(&self, yandex_stream: u32) -> Option<i64> {
        self.streams.iter().find(|(stream, _)| *stream == yandex_stream).map(|(_, ours)| *ours)
    }
}

pub struct YandexRelay {
    options: YandexRelayOptions,
    output: Arc<dyn RelayOutput>,
    /// The request's claimant: only the newest answer ends it.
    pub owner: Owner,
    state: Mutex<State>,
    done: watch::Sender<Option<YandexAnswer>>,
}

impl YandexRelay {
    /// The relay, its timeout armed for Yandex's response. Inside a tokio runtime.
    pub fn new(options: YandexRelayOptions, output: Arc<dyn RelayOutput>) -> Arc<YandexRelay> {
        let timeout = Duration::from_millis(options.timeout_ms);
        let relay = Arc::new(YandexRelay {
            options,
            output,
            owner: new_owner(),
            state: Mutex::new(State {
                streams: Vec::new(),
                open: Vec::new(),
                answer: YandexAnswer::default(),
                streamed_text: String::new(),
                speech: String::new(),
                revoiced: false,
                finished: false,
                cancelled: false,
                deadline: Instant::now() + timeout,
                waiting_for: "no response from Yandex",
            }),
            done: watch::channel(None).0,
        });
        let timer = relay.clone();
        tokio::spawn(async move { timer.watch_timeout().await });
        relay
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The answer, once the relay is done.
    pub async fn done(&self) -> YandexAnswer {
        let mut receiver = self.done.subscribe();
        loop {
            if let Some(answer) = receiver.borrow_and_update().clone() {
                return answer;
            }
            if receiver.changed().await.is_err() {
                return self.lock().answer.clone();
            }
        }
    }

    /// The armed timeout, re-armed by each frame; it ends with the relay.
    async fn watch_timeout(&self) {
        loop {
            let deadline = {
                let state = self.lock();
                if state.finished {
                    return;
                }
                state.deadline
            };
            tokio::time::sleep_until(deadline).await;
            let mut state = self.lock();
            if state.finished {
                return;
            }
            if Instant::now() < state.deadline {
                continue;
            }
            state.answer.error = Some(format!("{} in {} ms", state.waiting_for, self.options.timeout_ms));
            self.close_open_streams(&mut state, CloseReason::Cancel);
            self.finish(&mut state, RelayOutcome::Timeout);
            return;
        }
    }

    /// Superseded by a new request or cancelled by the station: its streams close as cancelled.
    pub fn cancel(&self, reason: &str) {
        let mut state = self.lock();
        if state.finished {
            return;
        }
        state.answer.error = Some(reason.to_string());
        self.close_open_streams(&mut state, CloseReason::Cancel);
        state.cancelled = true;
        self.finish(&mut state, RelayOutcome::Cancelled);
    }

    fn read_response(state: &mut State, response: &Value) {
        let empty = Value::Object(Default::default());
        state.answer.responded = true;
        let voice = response.get("voiceResponse").unwrap_or(&empty);
        let flag = |key: &str| voice.get(key).and_then(Value::as_bool).unwrap_or(false);
        state.answer.should_listen = flag("shouldListen");
        state.answer.has_voice = flag("hasVoiceResponse");
        state.answer.dialog_id = response
            .get("header")
            .and_then(|header| header.get("dialogId"))
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_string);
        state.answer.directives = response
            .get("response")
            .and_then(|body| body.get("directives"))
            .and_then(Value::as_array)
            .map(|directives| {
                directives
                    .iter()
                    .map(|directive| directive.get("name").and_then(Value::as_str).unwrap_or("").to_string())
                    .collect()
            })
            .unwrap_or_default();
        state.answer.ends_dialog =
            state.answer.directives.iter().any(|name| name == "end_dialog_session" || name == "close_dialog");
        let card = card_text(response);
        let speech = plain_speech(
            voice.get("outputSpeech").and_then(|speech| speech.get("text")).and_then(Value::as_str).unwrap_or(""),
        );
        state.speech = speech.clone();
        state.answer.text = if card.is_empty() { speech } else { card };
    }

    /// Yandex's recognition: its final text kept with the answer. False once the relay is cancelled.
    fn hear(&self, asr_result: &Value) -> bool {
        let mut state = self.lock();
        if state.cancelled {
            return false;
        }
        let text = recognition_text(Some(asr_result));
        if asr_result.get("endOfUtt").and_then(Value::as_bool).unwrap_or(false) && !text.is_empty() {
            state.answer.heard = Some(text);
        }
        true
    }

    fn settle_if_done(&self, state: &mut State) {
        if !state.answer.responded || !state.open.is_empty() {
            return;
        }
        // A response that promises speech waits for its stream.
        if state.answer.has_voice && state.answer.streams == 0 {
            return;
        }
        self.finish(state, RelayOutcome::Answered);
    }

    fn finish(&self, state: &mut State, outcome: RelayOutcome) {
        if state.finished {
            return;
        }
        state.finished = true;
        state.answer.outcome = outcome;
        if state.answer.text.is_empty() && !state.streamed_text.is_empty() {
            state.answer.text = state.streamed_text.clone();
        }
        self.done.send_replace(Some(state.answer.clone()));
    }

    fn arm(&self, state: &mut State, what: &'static str) {
        if state.finished {
            return;
        }
        state.deadline = Instant::now() + Duration::from_millis(self.options.timeout_ms);
        state.waiting_for = what;
    }

    fn close_open_streams(&self, state: &mut State, reason: CloseReason) {
        if self.options.quiet {
            return;
        }
        for yandex_stream in std::mem::take(&mut state.open) {
            if let Some(ours) = state.ours(yandex_stream).and_then(|ours| u32::try_from(ours).ok()) {
                self.output.close_stream(ours, reason);
            }
        }
    }

    fn summary(&self, state: &State, kind: &str, decoded: &Value) -> String {
        let event = decoded.get("event");
        match kind {
            "aliceResponse" => {
                let voice = event.and_then(|event| event.get("aliceResponse")).and_then(|r| r.get("voiceResponse"));
                let speech = voice
                    .and_then(|voice| voice.get("outputSpeech"))
                    .and_then(|speech| speech.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let listen =
                    voice.and_then(|voice| voice.get("shouldListen")).and_then(Value::as_bool).unwrap_or(false);
                let directives = if state.answer.directives.is_empty() {
                    "-".to_string()
                } else {
                    state.answer.directives.join(",")
                };
                format!("from Yandex: speech=\"{speech}\" listen={listen} directives={directives}")
            }
            "ttsSpeak" => {
                let stream = event.and_then(|event| event.get("header")).and_then(|h| h.get("streamId"));
                let stream_id = stream.and_then(Value::as_u64).and_then(|id| u32::try_from(id).ok());
                let ours = stream_id
                    .and_then(|id| state.ours(id))
                    .map_or_else(|| "undefined".to_string(), |ours| ours.to_string());
                let theirs = stream.map_or_else(|| "undefined".to_string(), Value::to_string);
                format!("from Yandex: stream={ours} (Yandex {theirs})")
            }
            _ => "from Yandex".into(),
        }
    }

    /// A frame of Yandex's with its stream ids ours.
    fn edited(&self, state: &State, frame: &[u8], station_ref: Option<&str>) -> Result<Bytes> {
        let map = |id: u32| state.ours(id).and_then(|ours| u32::try_from(ours).ok());
        let edit = FrameEdit { r#ref: station_ref, message_id: None, stream_id: Some(&map) };
        Ok(Bytes::from(edit_frame(frame, &edit)?))
    }
}

/// The header's stream id when it is set (a zero or none is no stream).
fn header_stream(event: &Value) -> Option<u32> {
    event
        .get("header")
        .and_then(|header| header.get("streamId"))
        .and_then(Value::as_u64)
        .filter(|&id| id != 0)
        .and_then(|id| u32::try_from(id).ok())
}

impl ExchangeHandler for YandexRelay {
    fn on_event(&self, frame: &Bytes, kind: &str, decoded: &Value) -> Result<()> {
        let empty = Value::Object(Default::default());
        let event = decoded.get("event").unwrap_or(&empty);
        if kind == "asrResult" {
            let asr_result = event.get("asrResult").unwrap_or(&empty);
            // Out of our lock, as the response's: the session follows the request's trip.
            if let (true, Some(on_asr)) = (self.hear(asr_result), &self.options.on_asr) {
                on_asr(asr_result);
            }
        }
        if kind == "aliceResponse" {
            let answer = {
                let mut state = self.lock();
                if state.cancelled || self.options.drop.contains(&kind) {
                    return Ok(());
                }
                YandexRelay::read_response(&mut state, event.get("aliceResponse").unwrap_or(&empty));
                state.answer.clone()
            };
            // Out of our lock: the session takes its own to follow the answer.
            if let Some(on_response) = &self.options.on_response {
                on_response(&answer);
            }
        }
        let mut state = self.lock();
        if state.cancelled || self.options.drop.contains(&kind) {
            return Ok(());
        }
        if kind == "streamingResponse" {
            if let Some(response) = event.get("streamingResponse").and_then(value_bytes) {
                // The text so far is field 1 of field 1; the last event repeats it whole.
                let text = get_string(get_bytes(&response, 1)?.unwrap_or_default(), 1)?;
                if let Some(text) = text.filter(|text| !text.is_empty()) {
                    state.streamed_text = text;
                }
            }
        }
        let yandex_stream = header_stream(event);
        if let Some(stream) = yandex_stream.filter(|&stream| state.ours(stream).is_none()) {
            let kept = self.options.quiet || (self.options.revoice.is_some() && kind == "ttsSpeak");
            let ours = if kept { -1 } else { i64::from(self.output.new_stream_id()) };
            state.streams.push((stream, ours));
            state.open.push(stream);
            state.answer.streams += 1;
        }
        if self.options.quiet {
            // A lookup needs the text, not the speech: done once Yandex starts speaking or answers in text.
            if kind == "ttsSpeak" || (kind == "aliceResponse" && !state.answer.text.is_empty()) {
                self.finish(&mut state, RelayOutcome::Answered);
            }
            return Ok(());
        }
        if let (Some(revoice), Some(stream)) = (&self.options.revoice, yandex_stream) {
            if state.ours(stream) == Some(-1) {
                // Its speech, said in our voice: the text, once, when the speech begins; the stream's frames stay here.
                let text = if state.speech.is_empty() { state.streamed_text.clone() } else { state.speech.clone() };
                if kind == "ttsSpeak" && !state.revoiced && !text.is_empty() {
                    state.revoiced = true;
                    drop(state);
                    revoice(text);
                    state = self.lock();
                    if state.cancelled {
                        return Ok(());
                    }
                }
                self.arm(&mut state, "Yandex stopped mid-answer");
                self.settle_if_done(&mut state);
                return Ok(());
            }
        }
        let edited = self.edited(&state, frame, self.options.station_ref.as_deref())?;
        let summary = self.summary(&state, kind, decoded);
        self.output.send_frame(edited, kind, summary, decoded);
        let what = if state.answer.responded { "Yandex stopped mid-answer" } else { "no response from Yandex" };
        self.arm(&mut state, what);
        self.settle_if_done(&mut state);
        Ok(())
    }

    fn on_audio(&self, frame: &Bytes, stream_id: u32) -> Result<()> {
        let mut state = self.lock();
        let ours = state.ours(stream_id);
        let Some(ours) = ours.filter(|&ours| ours >= 0 && !state.cancelled && !self.options.quiet) else {
            return Ok(());
        };
        self.output.send_audio(Bytes::from(restream(frame, ours as u32)?));
        self.arm(&mut state, "Yandex audio stalled");
        Ok(())
    }

    fn on_stream_control(&self, frame: &Bytes, stream_id: u32, decoded: &Value) -> Result<()> {
        let mut state = self.lock();
        let Some(ours) = state.ours(stream_id).filter(|_| !state.cancelled) else { return Ok(()) };
        if decoded.get("streamControl").and_then(|control| control.get("close")).is_some() {
            state.open.retain(|&open| open != stream_id);
        }
        if !self.options.quiet && ours >= 0 {
            let edited = self.edited(&state, frame, None)?;
            self.output.send_frame(
                edited,
                "streamControl",
                format!("stream={ours} (Yandex {stream_id}) close"),
                decoded,
            );
        }
        self.settle_if_done(&mut state);
        Ok(())
    }

    fn on_closed(&self, error: &str) {
        let mut state = self.lock();
        state.answer.error = Some(error.to_string());
        self.close_open_streams(&mut state, CloseReason::Cancel);
        self.finish(&mut state, RelayOutcome::Failed);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;
    use crate::gateway::protocol::aapi_codec::AapiCodec;
    use crate::gateway::protocol::protocol_types::{AliceResponseModel, ServerMessage};

    /// What the relay gives the station.
    #[derive(Default)]
    struct Output {
        kinds: Mutex<Vec<String>>,
        audio: AtomicU32,
        closed: Mutex<Vec<u32>>,
        next: AtomicU32,
    }

    impl RelayOutput for Output {
        fn send_frame(&self, _frame: Bytes, kind: &str, _summary: String, _decoded: &Value) {
            self.kinds.lock().unwrap().push(kind.to_string());
        }
        fn send_audio(&self, _frame: Bytes) {
            self.audio.fetch_add(1, Ordering::SeqCst);
        }
        fn close_stream(&self, stream_id: u32, _reason: CloseReason) {
            self.closed.lock().unwrap().push(stream_id);
        }
        fn new_stream_id(&self) -> u32 {
            100 + self.next.fetch_add(2, Ordering::SeqCst) + 2
        }
    }

    /// Yandex's answer with speech, frame by frame as the session hands it to the relay.
    fn answer(relay: &YandexRelay, speech: &str) {
        let codec = AapiCodec::new();
        let response = AliceResponseModel {
            request_id: "r".into(),
            response_id: "y".into(),
            text: Some(speech.into()),
            speech: Some(format!("<[domain music]>{speech}<[/domain]>")),
            should_listen: Some(false),
            ..Default::default()
        };
        for message in [
            ServerMessage::AliceResponse { r#ref: "station-1".into(), response },
            ServerMessage::TtsSpeak { r#ref: "station-1".into(), stream_id: 21, format: "audio/opus".into() },
        ] {
            let frame = codec.encode_server(&message).unwrap().frame;
            let decoded = codec.decode_server_frame(&frame).unwrap();
            let kind = codec.server_event_kind(&decoded).unwrap().to_string();
            relay.on_event(&frame, &kind, &decoded).unwrap();
        }
        let audio = Bytes::from([&[0u8, 0, 0, 21][..], b"OggS"].concat());
        relay.on_audio(&audio, 21).unwrap();
        let close =
            codec.encode_server(&ServerMessage::StreamClose { stream_id: 21, reason: CloseReason::Ok }).unwrap();
        let decoded = codec.decode_server_frame(&close.frame).unwrap();
        relay.on_stream_control(&close.frame, 21, &decoded).unwrap();
    }

    #[tokio::test]
    async fn relays_yandex_speech_to_the_station() {
        let sent = Arc::new(Output::default());
        let relay = YandexRelay::new(YandexRelayOptions::new(NO_EVENTS, 1000), sent.clone());
        answer(&relay, "Включаю Miyagi.");
        assert_eq!(*sent.kinds.lock().unwrap(), vec!["aliceResponse", "ttsSpeak", "streamControl"]);
        assert_eq!(sent.audio.load(Ordering::SeqCst), 1);
        assert_eq!(relay.done().await.outcome, RelayOutcome::Answered);
    }

    #[tokio::test]
    async fn keeps_yandex_speech_back_and_gives_its_text_to_our_voice_tts_provider_own() {
        let sent = Arc::new(Output::default());
        let said = Arc::new(Mutex::new(Vec::new()));
        let mut options = YandexRelayOptions::new(NO_EVENTS, 1000);
        let into = said.clone();
        options.revoice = Some(Box::new(move |text| into.lock().unwrap().push(text)));
        let relay = YandexRelay::new(options, sent.clone());
        answer(&relay, "Включаю Miyagi.");
        // The response with its commands goes on; the speech is ours, once, without Yandex's markup.
        assert_eq!(*sent.kinds.lock().unwrap(), vec!["aliceResponse"]);
        assert_eq!(sent.audio.load(Ordering::SeqCst), 0);
        assert_eq!(*said.lock().unwrap(), vec!["Включаю Miyagi."]);
        assert_eq!(relay.done().await.outcome, RelayOutcome::Answered);
    }

    #[tokio::test]
    async fn times_out_without_a_response_and_ends_once_when_cancelled() {
        let sent = Arc::new(Output::default());
        let relay = YandexRelay::new(YandexRelayOptions::new(NO_EVENTS, 30), sent.clone());
        let answer = relay.done().await;
        assert_eq!(answer.outcome, RelayOutcome::Timeout);
        assert_eq!(answer.error.as_deref(), Some("no response from Yandex in 30 ms"));
        relay.cancel("late");
        assert_eq!(relay.done().await.outcome, RelayOutcome::Timeout);
        assert_eq!(plain_speech(" Сейчас <[accented]>  плюс #десять sil<[300]> градусов. "), "Сейчас плюс градусов.");
    }
}
