//! One voice request: the station's audio stream from VoiceInput until we answer or it is cancelled. It lives in its
//! session's state; the session's tasks find it again by its stream and serial.

use bytes::Bytes;
use serde::{Serialize, Serializer};
use tokio_util::sync::CancellationToken;

use crate::gateway::observability::session_recorder::AudioSink;
use crate::gateway::observability::timeline::Timeline;
use crate::gateway::protocol::proto_object::js_number;
use crate::gateway::protocol::protocol_types::AliceRequest;
use crate::speech::audio::voicing::Voicing;
use crate::speech::interim_recognition::InterimRecognition;
use crate::speech::speech_pipeline::SpeechPipeline;
use crate::speech::Recognition;

/// listening: the station streams the command, VAD watches it; recognizing: the end is decided, recognition runs,
/// interim results still go out; done: answered or aborted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceState {
    Listening,
    Recognizing,
    Done,
}

/// A number as JSON.stringify writes it: an integral one without a fraction.
fn js<S: Serializer>(value: &f64, serializer: S) -> Result<S::Ok, S::Error> {
    js_number(*value).serialize(serializer)
}

/// The audio given to recognition, without its samples: where it was cut and how loud it was made.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UtteranceView {
    #[serde(serialize_with = "js")]
    pub from_ms: f64,
    #[serde(serialize_with = "js")]
    pub to_ms: f64,
    #[serde(serialize_with = "js")]
    pub gain_db: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

/// All audio after the wake word, recognised: quiet speech, or nothing in the VAD-cut audio.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AllAudio {
    #[serde(flatten)]
    pub recognition: Recognition,
    #[serde(serialize_with = "js")]
    pub from_ms: f64,
    #[serde(serialize_with = "js")]
    pub to_ms: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

pub struct VoiceRequest {
    /// Which request this is among the session's: a stream id may come again.
    pub serial: u64,
    pub message_id: String,
    pub stream_id: u32,
    pub request: AliceRequest,
    pub timeline: Timeline,
    /// Aborts recognition when the request is cancelled or superseded.
    pub abort: CancellationToken,
    pub state: VoiceState,
    /// Why listening ended (endOfUtterance, noSpeech, station:noSpeech, timeout, …) or why it was aborted.
    pub outcome: Option<String>,
    pub audio_bytes: usize,
    pub audio_frames: usize,
    /// Audio bytes received when the station marked the end of the wake word (SpotterEnd).
    pub spotter_end_at_bytes: Option<usize>,
    /// The station closed its audio stream itself, so we must not close it again.
    pub station_closed: bool,
    pub partials: u32,
    /// Set while an interim recognition runs (one at a time): its number and its cancel.
    pub interim_abort: Option<(u64, CancellationToken)>,
    pub utterance: Option<UtteranceView>,
    pub recognition: Option<Recognition>,
    pub all_audio: Option<AllAudio>,
    /// Windows with a pitch, counted for quiet speech only: none in a row in a whisper.
    pub voicing: Option<Voicing>,
    pub error: Option<String>,
    /// The safety timeout and the interim results' timer.
    pub timer: CancellationToken,
    pub partial_timer: CancellationToken,
    /// The station's audio as it came is kept for Yandex (YANDEX_VOICE, a whisper): in the session's request frame.
    pub raw: bool,
    audio: AudioSink,
    pub speech: SpeechPipeline,
    /// Recognition while the command is spoken; its text goes into every interim AsrResult.
    pub interim: InterimRecognition,
}

/// The station's audio kept for Yandex, as `append` and `mark_spotter_end` fill it.
pub struct RawVoice<'a> {
    pub audio: &'a mut Option<Vec<Bytes>>,
    pub spotter_end_chunk: &'a mut Option<usize>,
}

impl VoiceRequest {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        serial: u64,
        message_id: String,
        stream_id: u32,
        request: AliceRequest,
        audio: AudioSink,
        speech: SpeechPipeline,
        interim: InterimRecognition,
    ) -> VoiceRequest {
        VoiceRequest {
            serial,
            message_id,
            stream_id,
            request,
            timeline: Timeline::new(),
            abort: CancellationToken::new(),
            state: VoiceState::Listening,
            outcome: None,
            audio_bytes: 0,
            audio_frames: 0,
            spotter_end_at_bytes: None,
            station_closed: false,
            partials: 0,
            interim_abort: None,
            utterance: None,
            recognition: None,
            all_audio: None,
            voicing: None,
            error: None,
            timer: CancellationToken::new(),
            partial_timer: CancellationToken::new(),
            raw: false,
            audio,
            speech,
            interim,
        }
    }

    pub fn tag(&self) -> String {
        let id = self.request.request_id.as_deref().filter(|id| !id.is_empty()).unwrap_or(&self.message_id);
        id.chars().take(8).collect()
    }

    pub fn audio_file(&self) -> Option<String> {
        self.audio.file.as_ref().map(|file| file.to_string_lossy().into_owned())
    }

    /// Answered or aborted.
    pub fn done(&self) -> bool {
        self.state == VoiceState::Done
    }

    /// Audio after the end of the command is still recorded (it shows what the station sent), but VAD ignores it.
    pub fn append(&mut self, chunk: &[u8], raw: Option<RawVoice<'_>>) {
        if self.state == VoiceState::Done {
            return;
        }
        if self.audio_frames == 0 {
            self.timeline.mark("firstAudio");
        }
        self.audio_frames += 1;
        self.audio_bytes += chunk.len();
        self.audio.write(chunk);
        if let Some(RawVoice { audio: Some(audio), .. }) = raw {
            audio.push(Bytes::copy_from_slice(chunk));
        }
        self.speech.push(chunk);
    }

    pub fn mark_spotter_end(&mut self, raw: Option<RawVoice<'_>>) {
        self.timeline.mark("spotterEnd");
        self.spotter_end_at_bytes = Some(self.audio_bytes);
        if let Some(RawVoice { audio: Some(audio), spotter_end_chunk }) = raw {
            *spotter_end_chunk = Some(audio.len());
        }
        self.speech.mark_spotter_end();
    }

    /// listening → recognizing, once. False when the request already ended or was aborted.
    pub fn begin_recognition(&mut self, outcome: &str) -> bool {
        if self.state != VoiceState::Listening {
            return false;
        }
        self.state = VoiceState::Recognizing;
        self.outcome = Some(outcome.to_string());
        self.timer.cancel();
        self.timeline.mark("end");
        self.speech.stop();
        // The final recognition supersedes the interim one; the recognizer serves one request at a time.
        if let Some((_, interim)) = &self.interim_abort {
            interim.cancel();
        }
        true
    }

    pub fn stop_partials(&mut self) {
        self.partial_timer.cancel();
    }

    /// Final state: timers stopped, recognition aborted, files and decoder released. False when already done.
    pub fn close(&mut self, outcome: &str) -> bool {
        if self.state == VoiceState::Done {
            return false;
        }
        self.state = VoiceState::Done;
        if self.outcome.is_none() {
            self.outcome = Some(outcome.to_string());
        }
        self.timer.cancel();
        self.stop_partials();
        self.abort.cancel();
        self.audio.close();
        self.speech.close();
        true
    }
}
