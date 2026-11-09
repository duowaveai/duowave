//! One readable line per protocol event for the console; full details go to the journal.

use serde::Serialize;

use crate::gateway::protocol::protocol_types::{ClientEvent, ClientMessage, DirectiveType, ServerMessage};

/// An id's first eight characters, `-` for none.
fn short(value: Option<&str>) -> String {
    match value {
        Some(value) if !value.is_empty() => js_slice(value, 8).to_string(),
        _ => "-".to_string(),
    }
}

/// A model enum as the TS model spelled it (its serde name): `internalError`, `spotterEnd`, …
fn name_of(value: impl Serialize) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(name)) => name,
        _ => String::new(),
    }
}

/// The first `units` UTF-16 code units of a text, as JavaScript's `slice` counts them.
fn js_slice(text: &str, units: usize) -> &str {
    let mut used = 0;
    for (index, c) in text.char_indices() {
        used += c.len_utf16();
        if used > units {
            return &text[..index];
        }
    }
    text
}

fn or_dash(value: Option<&str>) -> &str {
    value.unwrap_or("-")
}

pub fn summarize_client(message: &ClientMessage) -> String {
    let event = match message {
        ClientMessage::StreamControl(control) => {
            let reason = control.reason.map(|reason| format!(":{}", name_of(reason))).unwrap_or_default();
            return format!("stream={} {}{reason}", control.stream_id, name_of(control.action));
        }
        ClientMessage::Event(event) => event,
    };
    match event {
        ClientEvent::SynchronizeState { application, .. } => {
            let app = application.as_ref();
            format!(
                "model={} fw={} tz={} lang={}",
                or_dash(app.and_then(|app| app.device_model.as_deref())),
                or_dash(app.and_then(|app| app.firmware_version.as_deref())),
                or_dash(app.and_then(|app| app.timezone.as_deref())),
                or_dash(app.and_then(|app| app.lang.as_deref())),
            )
        }
        ClientEvent::VoiceInput { header, request } => format!(
            "req={} stream={} validation={}",
            short(request.request_id.as_deref()),
            header.stream_id.map_or("-".to_string(), |id| id.to_string()),
            request.enable_spotter_validation.unwrap_or(false),
        ),
        ClientEvent::TextInput { request, .. } | ClientEvent::MatchedUser { request, .. } => {
            let event = &request.event;
            let name = event.name.as_deref().filter(|name| !name.is_empty()).map(|name| format!(" {name}"));
            let text = event.text.as_deref().filter(|text| !text.is_empty()).map(|text| format!(" \"{text}\""));
            format!(
                "req={} {}{}{}",
                short(request.request_id.as_deref()),
                event.r#type.as_str(),
                name.unwrap_or_default(),
                text.unwrap_or_default(),
            )
        }
        ClientEvent::CancelRequest { request_message_id, reason, .. } => {
            format!("ref={} reason={}", short(request_message_id.as_deref()), name_of(reason))
        }
        ClientEvent::TtsUpdateState { request_message_id, played_ms, reason, .. } => format!(
            "ref={} {} played={}ms",
            short(request_message_id.as_deref()),
            name_of(reason),
            played_ms.map_or("-".to_string(), |ms| ms.to_string()),
        ),
        ClientEvent::TtsGenerate { text, voice, .. } => {
            format!("\"{}\" voice={}", js_slice(text, 60), or_dash(voice.as_deref()))
        }
        ClientEvent::RequestStat { request_message_id, error, .. } => {
            let error = error.as_deref().filter(|error| !error.is_empty()).map(|error| format!(" error={error}"));
            format!("ref={}{}", short(request_message_id.as_deref()), error.unwrap_or_default())
        }
        ClientEvent::Unknown { name, .. } => format!("field={name}"),
        ClientEvent::LogSpotter { .. } | ClientEvent::Ack { .. } | ClientEvent::Ping { .. } => String::new(),
    }
}

pub fn summarize_server(message: &ServerMessage) -> String {
    match message {
        ServerMessage::AsrResult { text, end_of_utt, .. } => format!("endOfUtt={end_of_utt} \"{text}\""),
        ServerMessage::AliceResponse { response, .. } => {
            let names: Vec<&str> = response.directives.iter().map(|directive| directive.name.as_str()).collect();
            let directives = if names.is_empty() { "-".to_string() } else { names.join(",") };
            format!(
                "speech=\"{}\" listen={} directives={directives}",
                response.speech.as_deref().unwrap_or(""),
                response.should_listen.unwrap_or(false),
            )
        }
        ServerMessage::TtsSpeak { stream_id, format, .. } => format!("stream={stream_id} {format}"),
        ServerMessage::StreamClose { stream_id, reason } => format!("stream={stream_id} {}", name_of(reason)),
        ServerMessage::SpotterValidation { valid, .. } => format!("valid={valid}"),
        ServerMessage::Push { id, directive } => {
            let kind = match directive.r#type {
                DirectiveType::ClientAction => "client_action",
                DirectiveType::ServerAction => "server_action",
            };
            format!("{kind} {} id={id}", directive.name)
        }
        ServerMessage::SynchronizeStateResponse { .. }
        | ServerMessage::InputStartAck { .. }
        | ServerMessage::LogAck { .. }
        | ServerMessage::RequestStatAck { .. } => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::protocol::protocol_types::{
        AliceResponseModel, CloseReason, Directive, EventHeader, StreamAction, StreamControlEvent, TtsStateReason,
    };

    #[test]
    fn sums_up_an_event_in_a_line() {
        let control = ClientMessage::StreamControl(StreamControlEvent {
            stream_id: 2,
            message_id: None,
            action: StreamAction::Close,
            reason: Some(CloseReason::NoSpeech),
        });
        assert_eq!(summarize_client(&control), "stream=2 close:noSpeech");
        let update = ClientMessage::Event(ClientEvent::TtsUpdateState {
            header: EventHeader::default(),
            request_message_id: Some("voice-123456789".into()),
            played_ms: None,
            is_playing: None,
            reason: TtsStateReason::Unknown,
        });
        assert_eq!(summarize_client(&update), "ref=voice-12 unknown played=-ms");
        let text = "Привет, это длинный текст для проверки обрезки до шестидесяти знаков в сводке.";
        let generate = ClientMessage::Event(ClientEvent::TtsGenerate {
            header: EventHeader::default(),
            text: text.into(),
            voice: Some("shitova".into()),
            lang: None,
            format: None,
        });
        assert_eq!(
            summarize_client(&generate),
            "\"Привет, это длинный текст для проверки обрезки до шестидесят\" voice=shitova"
        );
        let response = ServerMessage::AliceResponse {
            r#ref: "voice-1".into(),
            response: AliceResponseModel {
                speech: Some("Включаю.".into()),
                should_listen: Some(true),
                directives: vec![Directive::client("audio_play", Default::default())],
                ..Default::default()
            },
        };
        assert_eq!(summarize_server(&response), "speech=\"Включаю.\" listen=true directives=audio_play");
        let close = ServerMessage::StreamClose { stream_id: 9, reason: CloseReason::Cancel };
        assert_eq!(summarize_server(&close), "stream=9 cancel");
    }
}
