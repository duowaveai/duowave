//! Yandex mode (relay_session.rs): the station's requests go to Yandex unchanged, and their trip is read off the
//! frames as they pass — the station's voice or text, Yandex's recognition (its ear), its response (its brain), its
//! speech stream (its voice), and the station's statistics at the end. Nothing here touches a frame.

use std::sync::Arc;

use serde_json::Value;

use super::yandex_relay::{card_text, plain_speech};
use crate::core::requests::{
    Place, RequestHandle, RequestKind, RequestPhase, RequestStart, RequestStarter, RequestStatus,
};
use crate::gateway::protocol::proto_object::is_js_space;
use crate::gateway::protocol::protocol_types::{AliceEventType, ClientEvent, ClientMessage};

const ALICE: &str = "Алиса";

/// A model enum as the TS model spelled it (its serde name): `interruption`, `userStop`, …
fn name_of(value: impl serde::Serialize) -> String {
    match serde_json::to_value(value) {
        Ok(Value::String(name)) => name,
        _ => String::new(),
    }
}

/// Yandex's recognition in an AsrResult: the normalized text, or its words.
pub fn recognition_text(asr_result: Option<&Value>) -> String {
    recognition_text_raw(asr_result).trim_matches(is_js_space).to_string()
}

/// The same, untrimmed: as the relay's journal line shows it.
pub fn recognition_text_raw(asr_result: Option<&Value>) -> String {
    let recognition = asr_result.and_then(|asr| asr.get("recognition")).and_then(|list| list.get(0));
    let text = match recognition.and_then(|recognition| recognition.get("normalized")) {
        Some(Value::String(normalized)) => normalized.clone(),
        Some(Value::Null) | None => recognition
            .and_then(|recognition| recognition.get("words"))
            .and_then(Value::as_array)
            .map(|words| {
                words.iter().map(|word| word.get("value").and_then(Value::as_str).unwrap_or("")).collect::<Vec<_>>()
            })
            .map(|words| words.join(" "))
            .unwrap_or_default(),
        Some(other) => other.to_string(),
    };
    text
}

/// The requests' trips of one relayed session, by the station's message id.
pub struct RelayTrips {
    starter: Option<Arc<dyn RequestStarter>>,
    device: String,
    session: String,
    /// In the order they began, as a JS Map keeps them.
    trips: Vec<(String, Arc<RequestHandle>)>,
    /// Yandex's speech streams, to the request each answers: the end of the stream is the end of the answer.
    speech: Vec<(u32, String)>,
}

impl RelayTrips {
    pub fn new(starter: Option<Arc<dyn RequestStarter>>, device: &str, session: &str) -> RelayTrips {
        RelayTrips {
            starter,
            device: device.to_string(),
            session: session.to_string(),
            trips: Vec::new(),
            speech: Vec::new(),
        }
    }

    fn trip(&self, r#ref: &str) -> Option<Arc<RequestHandle>> {
        self.trips.iter().find(|(known, _)| known == r#ref).map(|(_, trip)| trip.clone())
    }

    pub fn from_station(&mut self, message: &ClientMessage) {
        let ClientMessage::Event(event) = message else { return };
        match event {
            ClientEvent::VoiceInput { header, .. } => {
                self.supersede();
                if let Some(trip) = self.begin(&header.message_id, RequestKind::Voice, None) {
                    trip.step(RequestPhase::Listen, Some(Place::Yandex), Some(ALICE));
                }
            }
            ClientEvent::TextInput { header, request } => {
                // The station's callbacks (server_action) are not requests: only typed text is.
                let text = match request.event.r#type {
                    AliceEventType::TextInput => {
                        request.event.text.as_deref().map(|text| text.trim_matches(is_js_space))
                    }
                    _ => None,
                };
                let Some(text) = text.filter(|text| !text.is_empty()) else { return };
                self.supersede();
                if let Some(trip) = self.begin(&header.message_id, RequestKind::Text, Some(text)) {
                    trip.step(RequestPhase::Think, Some(Place::Yandex), Some(ALICE));
                }
            }
            ClientEvent::RequestStat { request_message_id, error, .. } => {
                let Some(r#ref) = request_message_id.as_deref().filter(|r#ref| !r#ref.is_empty()) else { return };
                if self.trip(r#ref).is_none() {
                    return;
                }
                if let Some(error) = error.as_deref().filter(|error| !error.is_empty()) {
                    let status = if error.to_lowercase().contains("nospeech") {
                        RequestStatus::Silent
                    } else {
                        RequestStatus::Failed
                    };
                    return self.end(r#ref, status, Some(error));
                }
                // The statistics may come while Alice still speaks: then her stream's end ends the request.
                if !self.speech.iter().any(|(_, answered)| answered == r#ref) {
                    self.end(r#ref, RequestStatus::Answered, None);
                }
            }
            ClientEvent::CancelRequest { request_message_id, reason, .. } => {
                let refs: Vec<String> = match request_message_id.as_deref().filter(|r#ref| !r#ref.is_empty()) {
                    Some(r#ref) => vec![r#ref.to_string()],
                    None => self.trips.iter().map(|(r#ref, _)| r#ref.clone()).collect(),
                };
                let error = format!("cancel:{}", name_of(reason));
                for r#ref in refs {
                    self.end(&r#ref, RequestStatus::Cancelled, Some(&error));
                }
            }
            _ => {}
        }
    }

    pub fn from_yandex(&mut self, kind: &str, decoded: &Value) {
        if kind == "streamControl" {
            let control = decoded.get("streamControl");
            let stream_id = control.and_then(|control| control.get("streamId")).and_then(Value::as_u64);
            let r#ref = stream_id.and_then(|id| self.speech.iter().find(|(stream, _)| u64::from(*stream) == id));
            if let (Some((_, r#ref)), Some(_)) = (r#ref.cloned(), control.and_then(|control| control.get("close"))) {
                self.end(&r#ref, RequestStatus::Answered, None);
            }
            return;
        }
        let empty = Value::Object(Default::default());
        let event = decoded.get("event").unwrap_or(&empty);
        let r#ref = event
            .get("header")
            .and_then(|header| header.get("refMessageId"))
            .and_then(Value::as_str)
            .filter(|r#ref| !r#ref.is_empty())
            .map(str::to_string);
        let Some(r#ref) = r#ref else { return };
        let Some(trip) = self.trip(&r#ref) else { return };
        match kind {
            "asrResult" => {
                let asr = event.get("asrResult");
                let text = recognition_text(asr);
                let end = asr.and_then(|asr| asr.get("endOfUtt")).and_then(Value::as_bool).unwrap_or(false);
                if !end {
                    if !text.is_empty() {
                        trip.partial(&text);
                    }
                    return;
                }
                // Nothing heard: the station's statistics end it.
                if text.is_empty() {
                    return;
                }
                trip.heard(&text);
                trip.step(RequestPhase::Think, Some(Place::Yandex), Some(ALICE));
            }
            "aliceResponse" => {
                let response = event.get("aliceResponse").unwrap_or(&empty);
                let voice = response.get("voiceResponse").unwrap_or(&empty);
                let card = card_text(response);
                let speech = plain_speech(
                    voice
                        .get("outputSpeech")
                        .and_then(|speech| speech.get("text"))
                        .and_then(Value::as_str)
                        .unwrap_or(""),
                );
                trip.routed(None, "yandex");
                if !card.is_empty() || !speech.is_empty() {
                    trip.answered(if card.is_empty() { &speech } else { &card });
                }
                // No speech comes: the response is the whole answer (music, a command).
                let has_voice = voice.get("hasVoiceResponse").and_then(Value::as_bool).unwrap_or(false);
                if !has_voice && speech.is_empty() {
                    self.end(&r#ref, RequestStatus::Answered, None);
                }
            }
            "ttsSpeak" => {
                let stream_id = event.get("header").and_then(|header| header.get("streamId")).and_then(Value::as_u64);
                if let Some(stream_id) = stream_id.and_then(|id| u32::try_from(id).ok()) {
                    self.speech.retain(|(stream, _)| *stream != stream_id);
                    self.speech.push((stream_id, r#ref.clone()));
                }
                trip.step(RequestPhase::Speak, Some(Place::Yandex), Some(ALICE));
            }
            _ => {}
        }
    }

    /// The station left: what was going ends with it.
    pub fn close(&mut self) {
        let refs: Vec<String> = self.trips.iter().map(|(r#ref, _)| r#ref.clone()).collect();
        for r#ref in refs {
            self.end(&r#ref, RequestStatus::Cancelled, Some("disconnected"));
        }
    }

    fn begin(&mut self, r#ref: &str, kind: RequestKind, text: Option<&str>) -> Option<Arc<RequestHandle>> {
        let starter = self.starter.as_ref()?;
        let trip = starter.begin(RequestStart {
            device: self.device.clone(),
            session: Some(self.session.clone()),
            kind,
            text: text.map(str::to_string),
            parent: None,
        });
        self.trips.retain(|(known, _)| known != r#ref);
        self.trips.push((r#ref.to_string(), trip.clone()));
        Some(trip)
    }

    /// A new wake word or text: Alice drops what she was doing.
    fn supersede(&mut self) {
        let refs: Vec<String> = self.trips.iter().map(|(r#ref, _)| r#ref.clone()).collect();
        for r#ref in refs {
            self.end(&r#ref, RequestStatus::Cancelled, Some("superseded"));
        }
    }

    fn end(&mut self, r#ref: &str, status: RequestStatus, error: Option<&str>) {
        let Some(index) = self.trips.iter().position(|(known, _)| known == r#ref) else { return };
        let (_, trip) = self.trips.remove(index);
        self.speech.retain(|(_, answered)| answered != r#ref);
        trip.finish(status, error, None);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use serde_json::json;

    use super::*;
    use crate::core::requests::{monotonic_clock, RequestHooks, RequestView};
    use crate::gateway::protocol::protocol_types::{AliceEvent, AliceRequest, CancelReason, EventHeader};

    /// Starts real handles and keeps their last state.
    #[derive(Default)]
    struct Starter {
        views: Arc<Mutex<Vec<RequestView>>>,
        starts: Mutex<Vec<RequestStart>>,
    }

    struct Keep {
        views: Arc<Mutex<Vec<RequestView>>>,
        index: usize,
    }

    impl RequestHooks for Keep {
        fn changed(&self, handle: &RequestHandle) {
            self.views.lock().unwrap()[self.index] = handle.snapshot();
        }
        fn ended(&self, _handle: &RequestHandle) {}
    }

    impl RequestStarter for Starter {
        fn begin(&self, start: RequestStart) -> Arc<RequestHandle> {
            self.starts.lock().unwrap().push(start.clone());
            let mut views = self.views.lock().unwrap();
            let index = views.len();
            let view = RequestView {
                id: format!("r{index}"),
                device: start.device,
                session: start.session,
                kind: start.kind,
                mode: "relay".into(),
                status: RequestStatus::Active,
                phase: None,
                started_at: crate::utils::time::now_iso(),
                ms: None,
                text: start.text,
                partial: None,
                answer: None,
                capability: None,
                executor: None,
                steps: Vec::new(),
                error: None,
                parent: None,
            };
            views.push(view.clone());
            Arc::new(RequestHandle::new(view, Arc::new(Keep { views: self.views.clone(), index }), monotonic_clock()))
        }
    }

    impl Starter {
        fn view(&self, index: usize) -> RequestView {
            self.views.lock().unwrap()[index].clone()
        }
    }

    fn header(message_id: &str) -> EventHeader {
        EventHeader { message_id: message_id.into(), ..EventHeader::default() }
    }

    fn request(kind: AliceEventType, text: Option<&str>, name: Option<&str>) -> AliceRequest {
        AliceRequest {
            request_id: None,
            sequence_number: None,
            dialog_id: None,
            application: None,
            event: AliceEvent {
                r#type: kind,
                text: text.map(str::to_string),
                name: name.map(str::to_string),
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

    fn voice_input(message_id: &str) -> ClientMessage {
        ClientMessage::Event(ClientEvent::VoiceInput {
            header: header(message_id),
            request: request(AliceEventType::VoiceInput, None, None),
        })
    }

    fn stat(message_id: &str, r#ref: &str, error: Option<&str>) -> ClientMessage {
        ClientMessage::Event(ClientEvent::RequestStat {
            header: header(message_id),
            request_message_id: Some(r#ref.into()),
            error: error.map(str::to_string),
        })
    }

    fn asr(r#ref: &str, text: &str, end_of_utt: bool) -> Value {
        json!({ "event": { "header": { "refMessageId": r#ref }, "asrResult": { "endOfUtt": end_of_utt, "recognition": [{ "normalized": text }] } } })
    }

    fn trips(starter: &Arc<Starter>) -> RelayTrips {
        RelayTrips::new(Some(starter.clone() as Arc<dyn RequestStarter>), "kitchen", "session-1")
    }

    #[test]
    fn reads_alices_trip_off_the_frames_her_ear_her_brain_her_voice_the_end_of_her_speech() {
        let requests = Arc::new(Starter::default());
        let mut trips = trips(&requests);
        trips.from_station(&voice_input("m1"));
        let view = requests.view(0);
        assert_eq!(
            (view.kind, view.status, view.phase),
            (RequestKind::Voice, RequestStatus::Active, Some(RequestPhase::Listen))
        );
        assert_eq!((view.steps[0].phase, view.steps[0].by), (RequestPhase::Listen, Some(Place::Yandex)));

        trips.from_yandex("asrResult", &asr("m1", "какая", false));
        assert_eq!(requests.view(0).partial.as_deref(), Some("какая"));
        trips.from_yandex("asrResult", &asr("m1", "какая погода", true));
        let view = requests.view(0);
        assert_eq!((view.text.as_deref(), view.phase), (Some("какая погода"), Some(RequestPhase::Think)));

        trips.from_yandex(
            "aliceResponse",
            &json!({ "event": { "header": { "refMessageId": "m1" }, "aliceResponse": { "voiceResponse": {
                "hasVoiceResponse": true, "outputSpeech": { "text": "Сейчас <[accented]> плюс десять." } } } } }),
        );
        let view = requests.view(0);
        assert_eq!(view.answer.as_deref(), Some("Сейчас плюс десять."));
        assert_eq!((view.executor.as_deref(), view.status), (Some("yandex"), RequestStatus::Active));
        trips.from_yandex(
            "ttsSpeak",
            &json!({ "event": { "header": { "refMessageId": "m1", "streamId": 6 }, "ttsSpeak": {} } }),
        );
        assert_eq!(requests.view(0).phase, Some(RequestPhase::Speak));

        // The station's statistics come while Alice speaks: the end of her stream ends the request.
        trips.from_station(&stat("s1", "m1", None));
        assert_eq!(requests.view(0).status, RequestStatus::Active);
        trips.from_yandex("streamControl", &json!({ "streamControl": { "streamId": 6, "close": {} } }));
        let view = requests.view(0);
        assert_eq!(view.status, RequestStatus::Answered);
        let phases: Vec<RequestPhase> = view.steps.iter().map(|step| step.phase).collect();
        assert_eq!(phases, vec![RequestPhase::Listen, RequestPhase::Think, RequestPhase::Speak]);
    }

    #[test]
    fn ends_a_request_with_nothing_heard_as_silent_by_the_station_statistics() {
        let requests = Arc::new(Starter::default());
        let mut trips = trips(&requests);
        trips.from_station(&voice_input("m2"));
        trips.from_yandex("asrResult", &asr("m2", "", true));
        trips.from_station(&stat("s2", "m2", Some("asr:NoSpeechDetectedError")));
        let view = requests.view(0);
        assert_eq!((view.status, view.error.as_deref()), (RequestStatus::Silent, Some("asr:NoSpeechDetectedError")));
    }

    #[test]
    fn takes_typed_text_as_a_request_a_response_without_speech_as_its_whole_answer_and_callbacks_as_nothing() {
        let requests = Arc::new(Starter::default());
        let mut trips = trips(&requests);
        trips.from_station(&ClientMessage::Event(ClientEvent::TextInput {
            header: header("cb"),
            request: request(AliceEventType::ServerAction, None, Some("@@mm_typed_callback")),
        }));
        assert!(requests.starts.lock().unwrap().is_empty());
        trips.from_station(&ClientMessage::Event(ClientEvent::TextInput {
            header: header("t1"),
            request: request(AliceEventType::TextInput, Some("Включи музыку"), None),
        }));
        trips.from_yandex(
            "aliceResponse",
            &json!({ "event": { "header": { "refMessageId": "t1" }, "aliceResponse": {
                "response": { "cards": [{ "text": "Включаю." }] }, "voiceResponse": {} } } }),
        );
        let view = requests.view(0);
        assert_eq!(view.kind, RequestKind::Text);
        assert_eq!((view.text.as_deref(), view.answer.as_deref()), (Some("Включи музыку"), Some("Включаю.")));
        assert_eq!(view.status, RequestStatus::Answered);
    }

    #[test]
    fn cancels_what_was_going_when_a_new_wake_word_comes_and_everything_when_the_station_leaves() {
        let requests = Arc::new(Starter::default());
        let mut trips = trips(&requests);
        trips.from_station(&voice_input("m3"));
        trips.from_station(&voice_input("m4"));
        let view = requests.view(0);
        assert_eq!((view.status, view.error.as_deref()), (RequestStatus::Cancelled, Some("superseded")));
        trips.close();
        let view = requests.view(1);
        assert_eq!((view.status, view.error.as_deref()), (RequestStatus::Cancelled, Some("disconnected")));
        // A cancel names its reason.
        trips.from_station(&voice_input("m5"));
        trips.from_station(&ClientMessage::Event(ClientEvent::CancelRequest {
            header: header("c1"),
            request_message_id: None,
            reason: CancelReason::Interruption,
        }));
        assert_eq!(requests.view(2).error.as_deref(), Some("cancel:interruption"));
    }
}
