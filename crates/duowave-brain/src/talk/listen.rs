//! The panel's voice (docs/plans/voice-creation.md, «Голос пульта»; decisions 2 and 3): the phrase the panel's
//! microphone heard — a WAV, 16 kHz mono, up to 15 s and 1 MB — goes to the gateway's own «Слух», the recognition the
//! stations use, and never to the browser's (Chrome sends it to Google). The answer says where it was heard: at home,
//! or in a cloud (the panel's strip writes that in amber). The right is the one «Сказать» needed (`station.say`).
//!
//! - `POST /api/v1/listen?piece=N` — body `audio/wav` → `{text, heard: "home" | "cloud", thought}`. The panel records a
//!   thought in pieces cut at pauses (docs/plans/audio-interface-2.md, «К1. Мысль»): `piece` is the piece's number in
//!   its thought, from 1; `thought` says how the piece ends it and how long to wait for more (`talk::thought`), by the
//!   words of the household's language.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Query};
use axum::http::StatusCode;
use axum::routing::post;
use axum::Router;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::core::identity::permissions::STATION_SAY;
use crate::platform::http::{ok, ApiError, ApiResult, Caller, Uses};
use crate::settings::SettingsService;
use crate::speech::speech_pipeline::UtteranceAudio;
use crate::speech::SpeechService;

/// The phrase at most this big and this long: a sentence, not a recording.
pub const MAX_BYTES: usize = 1024 * 1024;
pub const MAX_SECONDS: f64 = 15.0;
/// What «Слух» takes: 16 kHz mono, 16-bit.
pub const RATE: u32 = 16_000;

pub fn routes<S: crate::speech::RouteNeeds>() -> Router<S> {
    Router::new().route("/api/v1/listen", post(listen)).layer(DefaultBodyLimit::max(MAX_BYTES + 4096))
}

/// What was heard and where.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Heard {
    pub text: String,
    /// `home` — the gateway's or the computer's own server; `cloud` — a recognizer in the network.
    pub heard: &'static str,
    /// How the piece ends its thought.
    pub thought: super::thought::Thought,
}

/// Which piece of its thought the audio is.
#[derive(Debug, Default, serde::Deserialize)]
pub struct Piece {
    piece: Option<u32>,
}

/// The samples of a WAV: PCM, 16-bit, mono, 16 kHz — what the panel's recorder writes (panel/src/voice/mic.ts).
pub fn wav_pcm(bytes: &[u8]) -> Result<Vec<i16>, String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(crate::tr!("нужен WAV"));
    }
    let mut at = 12;
    let mut format: Option<(u16, u16, u32, u16)> = None;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32::from_le_bytes([bytes[at + 4], bytes[at + 5], bytes[at + 6], bytes[at + 7]]) as usize;
        let body = at + 8;
        let end = body.saturating_add(size).min(bytes.len());
        match id {
            b"fmt " if end - body >= 16 => {
                let word = |i: usize| u16::from_le_bytes([bytes[body + i], bytes[body + i + 1]]);
                let rate = u32::from_le_bytes([bytes[body + 4], bytes[body + 5], bytes[body + 6], bytes[body + 7]]);
                format = Some((word(0), word(2), rate, word(14)));
            }
            b"data" => {
                let Some((kind, channels, rate, bits)) = format else {
                    return Err(crate::tr!("в WAV нет описания звука"));
                };
                if kind != 1 || bits != 16 {
                    return Err(crate::tr!("нужен WAV PCM 16 бит"));
                }
                if channels != 1 || rate != RATE {
                    return Err(crate::tr!(
                        "нужен WAV 16 кГц моно, а пришёл {rate} Гц, каналов: {channels}",
                        rate = rate,
                        channels = channels
                    ));
                }
                return Ok(bytes[body..end].as_chunks::<2>().0.iter().map(|pair| i16::from_le_bytes(*pair)).collect());
            }
            _ => {}
        }
        at = body + size + (size & 1);
    }
    Err(crate::tr!("в WAV нет звука"))
}

/// The phrase recognized by «Слух», as the station's would be.
pub async fn listen(
    Uses(speech): Uses<Arc<SpeechService>>,
    Uses(settings): Uses<Arc<SettingsService>>,
    caller: Caller,
    Query(piece): Query<Piece>,
    body: Bytes,
) -> ApiResult<Heard> {
    caller.permitted(STATION_SAY)?;
    if body.len() > MAX_BYTES {
        return Err(ApiError::bad_request(crate::tr!("Фраза больше 1 МБ.")));
    }
    let pcm = wav_pcm(&body).map_err(ApiError::bad_request)?;
    let seconds = pcm.len() as f64 / RATE as f64;
    if seconds > MAX_SECONDS + 0.5 {
        return Err(ApiError::bad_request(crate::tr!("Фраза длиннее 15 с.")));
    }
    if !speech.recognizes() {
        // The page of «Слух» goes by the way the household talks (src/talk/input.rs).
        let page = if crate::ear::input::on_computer(&settings) {
            crate::tr!("Микрофон")
        } else {
            crate::tr!("Колонка")
        };
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            crate::tr!("«Слух» выключен: распознавание не настроено (Настройки → {page} → «Слух»).", page = page),
        ));
    }
    let audio = UtteranceAudio { pcm, from_ms: 0.0, to_ms: seconds * 1000.0, gain_db: 0.0 };
    let recognition = speech.recognize(&audio, &[], &CancellationToken::new()).await.map_err(|error| {
        ApiError::bad_gateway(crate::tr!("«Слух» не ответил: {error}", error = format!("{error:#}")))
    })?;
    let first = piece.piece.unwrap_or(1) <= 1;
    let thought = crate::sense::home().finished(&recognition.text, None, first);
    Ok(ok(Heard { text: recognition.text, heard: if speech.at_home() { "home" } else { "cloud" }, thought }))
}
