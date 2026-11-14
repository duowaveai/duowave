//! The talk's routes.
//!
//! - `GET /api/v1/talk/input` → `{input, before?}`, `PUT … {input}` — «Как говорить с DuoWave». Anyone who sees the
//!   station reads the choice; whoever may switch the mode makes it — the choice switches the mode.
//! - `POST /api/v1/talk {text, via?, heard?, voice?, fresh?}` — a phrase into the talk. With
//!   `Accept: text/event-stream` the answer is a stream of events (`start`, `phrase`, `tool`, `done`, `error`), and
//!   closing it cancels the answer; without it — `{said, by}` once the answer is whole. `via` is what opened the
//!   phrase (`button`, `answer`, `text`); `heard` says where it was recognized (`home`, `cloud`, or
//!   `{by, ms?, what?}`); `voice` — the computer will say the answer aloud; `fresh` — the talk starts over. The right
//!   is the one a conversation by voice needs (`use:chat`).
//! - `POST /api/v1/talk/speech {text}` → `audio/wav`, 16-bit mono: one phrase of the answer in the household's own
//!   voice, for the computer's speakers. Alice's voice cannot speak here — 409 in words; so does a voice server
//!   that is not up yet. A phrase of up to 600 characters; the same right.

use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use futures::StreamExt;
use serde::Serialize;
use serde_json::Value;
use tokio_stream::wrappers::UnboundedReceiverStream;

use super::{input, Activator, Heard, Origin, TalkEvent, Turn, MAX_TEXT};
use crate::app::App;
use crate::core::identity::permissions::{use_of, STATION_MODE, STATION_VIEW};
use crate::core::requests::Place;
use crate::placement::service::Role;
use crate::platform::http::{ok, ApiError, ApiResult, Body, Caller};
use crate::settings::definitions::VoiceInput;
use crate::skills::capabilities::CapabilityName;

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/v1/talk/input", get(voice_input).put(set_voice_input))
        .route("/api/v1/talk", post(talk))
        .route("/api/v1/talk/speech", post(speech))
}

/// One phrase at most this long: the panel asks for an answer phrase by phrase.
pub const MAX_PHRASE: usize = 600;

pub const NO_VOICE: &str = "Голос не выбран: выберите свой — «Персонаж» → «Голос».";
pub const VOICE_STARTING: &str = "Голос запускается: попробуйте через несколько секунд.";

/// A phrase said by the household's own voice, as a WAV the panel plays.
async fn speech(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<Value>) -> Result<Response, ApiError> {
    caller.permitted(&use_of(CapabilityName::Chat))?;
    let text = body.get("text").and_then(Value::as_str).map(str::trim).unwrap_or_default();
    if text.is_empty() {
        return Err(ApiError::bad_request(crate::tr!("Нет текста.")));
    }
    if text.chars().count() > MAX_PHRASE {
        return Err(ApiError::bad_request(crate::tr!("Фраза длиннее {max} знаков.", max = MAX_PHRASE)));
    }
    let voice = app.voice.for_computer().ok_or_else(|| ApiError::conflict(NO_VOICE))?;
    // The panel going away (a new «Сказать», the × of the strip) drops the request, and that stops the synthesis.
    let cancel = tokio_util::sync::CancellationToken::new();
    let _guard = cancel.clone().drop_guard();
    let wav = voice.synthesize_wav(text, &cancel).await.map_err(|error| {
        if crate::speech::tts::own_speech::is_unreachable(&error) {
            return ApiError::conflict(VOICE_STARTING);
        }
        let why: String = app.secrets.redact(&format!("{error:#}")).chars().take(200).collect();
        ApiError::bad_gateway(crate::tr!("Голос не ответил: {why}", why = why))
    })?;
    let mut response = (StatusCode::OK, wav).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("audio/wav"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

async fn voice_input(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<VoiceInput> {
    caller.permitted(STATION_VIEW)?;
    Ok(ok(input::current(&app.settings)))
}

async fn set_voice_input(
    State(app): State<Arc<App>>,
    caller: Caller,
    Body(body): Body<Value>,
) -> ApiResult<VoiceInput> {
    caller.permitted(STATION_MODE)?;
    let station = app.placement.role() == Role::Station;
    Ok(ok(input::choose(&app.modes, &app.settings, station, body.get("input"), caller.actor())?))
}

/// The whole answer, for a caller that does not read a stream.
#[derive(Debug, Serialize)]
struct Said {
    said: String,
    by: &'static str,
}

/// The phrase as the body gave it: what cannot be read is left out, never guessed.
fn turn_of(body: &Value) -> Result<Turn, ApiError> {
    let text: String = body
        .get("text")
        .and_then(Value::as_str)
        .map(|text| text.trim().chars().take(MAX_TEXT).collect())
        .unwrap_or_default();
    if text.is_empty() {
        return Err(ApiError::bad_request(crate::tr!("Скажите или напишите что-нибудь.")));
    }
    let place = |value: &str| match value {
        "home" => Some(Place::Own),
        "cloud" => Some(Place::Cloud),
        _ => None,
    };
    let heard = body.get("heard").and_then(|heard| match heard {
        Value::String(by) => place(by).map(|by| Heard { by, ms: None, what: None }),
        Value::Object(heard) => Some(Heard {
            by: heard.get("by").and_then(Value::as_str).and_then(place)?,
            ms: heard.get("ms").and_then(Value::as_u64),
            what: heard.get("what").and_then(Value::as_str).map(str::to_string),
        }),
        _ => None,
    });
    // A phrase somebody heard was said: after the button unless the body says it answers a question.
    let fallback = if heard.is_some() { Activator::Button } else { Activator::Text };
    let activator = body.get("via").and_then(Value::as_str).and_then(Activator::parse).unwrap_or(fallback);
    Ok(Turn {
        text,
        origin: Origin::computer(activator),
        heard,
        voice: body.get("voice").and_then(Value::as_bool) == Some(true),
        fresh: body.get("fresh").and_then(Value::as_bool) == Some(true),
    })
}

async fn talk(
    State(app): State<Arc<App>>,
    caller: Caller,
    headers: HeaderMap,
    Body(body): Body<Value>,
) -> Result<Response, ApiError> {
    let principal = caller.permitted(&use_of(CapabilityName::Chat))?.clone();
    let turn = turn_of(&body)?;
    let mut talk = super::begin(&app, &principal, turn)?;
    let stream = headers
        .get(header::ACCEPT)
        .and_then(|accept| accept.to_str().ok())
        .is_some_and(|accept| accept.contains("text/event-stream"));
    if !stream {
        // The caller going away drops the guard, and that stops the answer.
        let _guard = talk.cancel.clone().drop_guard();
        while let Some(event) = talk.events.recv().await {
            match event {
                TalkEvent::Done { said, by, .. } => return Ok(ok(Said { said, by }).into_response()),
                TalkEvent::Error { message } => return Err(ApiError::bad_gateway(message)),
                _ => {}
            }
        }
        return Err(ApiError::bad_gateway(crate::tr!("Ответ оборвался.")));
    }
    let guard = talk.cancel.drop_guard();
    let body = UnboundedReceiverStream::new(talk.events).map(move |event| {
        let _ = &guard;
        let data = serde_json::to_string(&event).unwrap_or_else(|_| "{}".into());
        Ok::<_, Infallible>(format!("data: {data}\n\n"))
    });
    let mut response = (StatusCode::OK, axum::body::Body::from_stream(body)).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/event-stream; charset=utf-8"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache, no-transform"));
    headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
    Ok(response)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::talk::Via;

    #[test]
    fn reads_the_phrase_and_where_it_came_from() {
        let typed = turn_of(&json!({ "text": "  Который час?  " })).unwrap();
        assert_eq!(
            (typed.text.as_str(), typed.origin.activator, typed.origin.via),
            ("Который час?", Activator::Text, Via::Text)
        );
        assert_eq!((typed.heard, typed.voice, typed.fresh), (None, false, false));

        let said = turn_of(&json!({ "text": "Привет", "heard": "cloud", "voice": true, "fresh": true })).unwrap();
        assert_eq!(said.origin.activator, Activator::Button, "a phrase somebody heard was said after the button");
        assert_eq!(said.heard, Some(Heard { by: Place::Cloud, ms: None, what: None }));
        assert!(said.voice && said.fresh);

        let answer = turn_of(&json!({
            "text": "Да", "via": "answer", "heard": { "by": "home", "ms": 420, "what": "gigaam-v3" }
        }))
        .unwrap();
        assert_eq!((answer.origin.activator, answer.origin.device.as_str()), (Activator::Answer, "computer"));
        assert_eq!(answer.heard, Some(Heard { by: Place::Own, ms: Some(420), what: Some("gigaam-v3".into()) }));

        assert_eq!(turn_of(&json!({ "text": "Да", "heard": "the moon", "via": "word" })).unwrap().heard, None);
        assert_eq!(turn_of(&json!({ "text": "я".repeat(1500) })).unwrap().text.chars().count(), MAX_TEXT);
        for empty in [json!({}), json!({ "text": "  " }), json!({ "text": 5 })] {
            let refused = turn_of(&empty).unwrap_err();
            assert_eq!((refused.status.as_u16(), refused.message.as_str()), (400, "Скажите или напишите что-нибудь."));
        }
    }
}
