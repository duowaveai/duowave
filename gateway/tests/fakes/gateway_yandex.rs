//! Yandex's uniproxy as our own session sees it (test/app.e2e-spec.ts's fake): SynchronizeStateResponse on the
//! station's frame, then per TtsGenerate a TtsSpeak, 300 ms of Ogg/Opus in a few frames on a stream of its own and
//! the stream close; TextInput answered as the test says; a VoiceInput recognised itself and answered «Включаю.».
//!
//! test/app.e2e-spec.ts, the Node original, is removed from the tree: git tag `node-gateway-last`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use duowave::gateway::protocol::aapi_codec::AapiCodec;
use duowave::gateway::protocol::protocol_types::{
    AliceResponseModel, ClientEvent, ClientMessage, CloseReason, Directive, ServerMessage, StreamAction,
};

use super::{codec, encode_server, lock, sleep, speech_frames, FakeConnection, WsServer};

/// A TextInput Yandex got: our text on the Yandex route, the station's callbacks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextInputSeen {
    pub message_id: String,
    pub r#type: String,
    pub text: Option<String>,
    pub name: Option<String>,
    pub request_id: Option<String>,
}

/// A VoiceInput Yandex got, with its audio and stream controls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceInputSeen {
    pub message_id: String,
    pub stream_id: u32,
    pub request_id: Option<String>,
    pub audio_bytes: usize,
    pub spotter_end: bool,
    pub closed: bool,
}

/// What the fake Yandex answers to a TextInput: a response, and speech when `speech` is set.
#[derive(Debug, Clone, Default)]
pub struct FakeAnswer {
    pub speech: Option<String>,
    pub should_listen: Option<bool>,
    pub directives: Vec<Directive>,
}

pub type Answerer = Box<dyn Fn(&TextInputSeen) -> Option<FakeAnswer> + Send + Sync>;

#[derive(Default)]
pub struct YandexState {
    /// Every TtsGenerate: text, lang, format.
    pub tts: Vec<(String, Option<String>, Option<String>)>,
    pub text_inputs: Vec<TextInputSeen>,
    pub voice_inputs: Vec<VoiceInputSeen>,
    /// How Yandex answers a TextInput; None: it does not answer.
    pub answer: Option<Answerer>,
    /// Pause between audio frames; tests that interrupt an answer slow it down.
    pub chunk_delay_ms: u64,
    /// Pause before TtsSpeak: a phrase can be dropped before Yandex answers it.
    pub tts_delay_ms: u64,
}

pub struct GatewayYandex {
    pub server: WsServer,
    pub state: Arc<Mutex<YandexState>>,
}

/// Speech on a stream of Yandex's own numbering: TtsSpeak and the first frame now, the rest at the fake's pace.
fn speak(connection: &Arc<FakeConnection>, state: &Arc<Mutex<YandexState>>, r#ref: &str, stream_id: u32) {
    connection.send(encode_server(&ServerMessage::TtsSpeak {
        r#ref: r#ref.to_string(),
        stream_id,
        format: "audio/opus".into(),
    }));
    let mut frames = speech_frames(stream_id).into_iter();
    if let Some(first) = frames.next() {
        connection.send(first);
    }
    let (connection, state) = (connection.clone(), state.clone());
    let rest: Vec<Bytes> = frames.collect();
    tokio::spawn(async move {
        let delay = || lock(&state).chunk_delay_ms;
        let first = delay();
        sleep(first).await;
        for frame in rest {
            if !connection.open() {
                return;
            }
            connection.send(frame);
            let pause = delay();
            sleep(pause).await;
        }
        if connection.open() {
            connection.send(encode_server(&ServerMessage::StreamClose { stream_id, reason: CloseReason::Ok }));
        }
    });
}

impl GatewayYandex {
    pub async fn start() -> GatewayYandex {
        let state: Arc<Mutex<YandexState>> = Arc::default();
        let shared = state.clone();
        let streams: Arc<Mutex<HashMap<usize, u32>>> = Arc::default();
        let server = WsServer::start(Arc::new(move |connection, frame| {
            let state = &shared;
            // Each connection numbers its streams 1, 3, 5, …
            let key = Arc::as_ptr(connection) as usize;
            let next_stream = || {
                let mut streams = lock(&streams);
                let next = streams.entry(key).or_insert(1);
                let stream = *next;
                *next += 2;
                stream
            };
            if !AapiCodec::is_aapi_frame(frame) {
                if frame.len() > 4 {
                    let stream_id = u32::from_be_bytes([frame[0], frame[1], frame[2], frame[3]]);
                    let mut state = lock(state);
                    if let Some(voice) = state.voice_inputs.iter_mut().find(|voice| voice.stream_id == stream_id) {
                        voice.audio_bytes += frame.len() - 4;
                    }
                }
                return;
            }
            let Ok(decoded) = codec().decode_client(frame) else { return };
            match decoded.message {
                ClientMessage::StreamControl(control) => {
                    let voice = {
                        let mut state = lock(state);
                        let Some(voice) =
                            state.voice_inputs.iter_mut().find(|voice| voice.stream_id == control.stream_id)
                        else {
                            return;
                        };
                        if control.action == StreamAction::SpotterEnd {
                            voice.spotter_end = true;
                        }
                        if control.action != StreamAction::Close {
                            return;
                        }
                        voice.closed = true;
                        voice.clone()
                    };
                    // Yandex recognises the voice itself: its own results (the station has ours, or nothing here
                    // recognises), the words so far and then all of them, then the answer.
                    connection.send(encode_server(&ServerMessage::AsrResult {
                        r#ref: voice.message_id.clone(),
                        text: "включи".into(),
                        end_of_utt: false,
                    }));
                    connection.send(encode_server(&ServerMessage::AsrResult {
                        r#ref: voice.message_id.clone(),
                        text: "включи кино".into(),
                        end_of_utt: true,
                    }));
                    let response = AliceResponseModel {
                        request_id: voice.request_id.clone().unwrap_or_default(),
                        response_id: "yandex-voice-response".into(),
                        speech: Some("Включаю.".into()),
                        text: Some("Включаю.".into()),
                        ..Default::default()
                    };
                    connection.send(encode_server(&ServerMessage::AliceResponse {
                        r#ref: voice.message_id.clone(),
                        response,
                    }));
                    speak(connection, state, &voice.message_id, next_stream());
                }
                ClientMessage::Event(ClientEvent::VoiceInput { header, request }) => {
                    lock(state).voice_inputs.push(VoiceInputSeen {
                        message_id: header.message_id,
                        stream_id: header.stream_id.unwrap_or(0),
                        request_id: request.request_id,
                        audio_bytes: 0,
                        spotter_end: false,
                        closed: false,
                    });
                }
                ClientMessage::Event(ClientEvent::SynchronizeState { header, .. }) => {
                    connection.send(encode_server(&ServerMessage::SynchronizeStateResponse {
                        r#ref: header.message_id,
                        session_id: "yandex-session".into(),
                    }));
                }
                ClientMessage::Event(ClientEvent::TextInput { header, request }) => {
                    let input = TextInputSeen {
                        message_id: header.message_id.clone(),
                        r#type: request.event.r#type.as_str().to_string(),
                        text: request.event.text.clone(),
                        name: request.event.name.clone(),
                        request_id: request.request_id.clone(),
                    };
                    let answer = {
                        let mut state = lock(state);
                        state.text_inputs.push(input.clone());
                        state.answer.as_ref().and_then(|answer| answer(&input))
                    };
                    let Some(answer) = answer else { return };
                    let response = AliceResponseModel {
                        request_id: input
                            .request_id
                            .clone()
                            .filter(|id| !id.is_empty())
                            .unwrap_or_else(|| "yandex-request".into()),
                        response_id: "yandex-response".into(),
                        text: answer.speech.clone(),
                        speech: answer.speech.clone(),
                        should_listen: answer.should_listen,
                        directives: answer.directives.clone(),
                        ..Default::default()
                    };
                    connection.send(encode_server(&ServerMessage::AliceResponse {
                        r#ref: input.message_id.clone(),
                        response,
                    }));
                    if answer.speech.as_deref().is_some_and(|speech| !speech.is_empty()) {
                        speak(connection, state, &input.message_id, next_stream());
                    }
                }
                ClientMessage::Event(ClientEvent::TtsGenerate { header, text, lang, format, .. }) => {
                    let delay = {
                        let mut state = lock(state);
                        state.tts.push((text, lang, format));
                        state.tts_delay_ms
                    };
                    let stream_id = next_stream();
                    if delay == 0 {
                        return speak(connection, state, &header.message_id, stream_id);
                    }
                    let (connection, state) = (connection.clone(), state.clone());
                    tokio::spawn(async move {
                        sleep(delay).await;
                        if connection.open() {
                            speak(&connection, &state, &header.message_id, stream_id);
                        }
                    });
                }
                _ => {}
            }
        }))
        .await;
        GatewayYandex { server, state }
    }

    pub fn url(&self) -> String {
        self.server.url.clone()
    }

    /// The texts of every TtsGenerate.
    pub fn tts(&self) -> Vec<String> {
        lock(&self.state).tts.iter().map(|(text, _, _)| text.clone()).collect()
    }

    pub fn tts_full(&self) -> Vec<(String, Option<String>, Option<String>)> {
        lock(&self.state).tts.clone()
    }

    pub fn text_inputs(&self) -> Vec<TextInputSeen> {
        lock(&self.state).text_inputs.clone()
    }

    pub fn voice_inputs(&self) -> Vec<VoiceInputSeen> {
        lock(&self.state).voice_inputs.clone()
    }

    pub fn answer(&self, answer: Option<Answerer>) {
        lock(&self.state).answer = answer;
    }

    pub fn set_chunk_delay(&self, ms: u64) {
        lock(&self.state).chunk_delay_ms = ms;
    }

    pub fn set_tts_delay(&self, ms: u64) {
        lock(&self.state).tts_delay_ms = ms;
    }
}
