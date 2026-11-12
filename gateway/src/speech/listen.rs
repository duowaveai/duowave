//! The panel's voice (docs/plans/voice-creation.md, «Голос пульта»; decisions 2 and 3): the phrase the panel's
//! microphone heard — a WAV, 16 kHz mono, up to 15 s and 1 MB — goes to the gateway's own «Слух», the recognition the
//! stations use, and never to the browser's (Chrome sends it to Google). The answer says where it was heard: at home,
//! or in a cloud (the panel's strip writes that in amber). The right is the one «Сказать» needed (`station.say`).
//!
//! - `POST /api/v1/listen` — body `audio/wav` → `{text, heard: "home" | "cloud"}`.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::routing::post;
use axum::Router;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::app::App;
use crate::core::identity::permissions::STATION_SAY;
use crate::platform::http::{ok, ApiError, ApiResult, Caller};
use crate::speech::speech_pipeline::UtteranceAudio;

/// The phrase at most this big and this long: a sentence, not a recording.
pub const MAX_BYTES: usize = 1024 * 1024;
pub const MAX_SECONDS: f64 = 15.0;
/// What «Слух» takes: 16 kHz mono, 16-bit.
pub const RATE: u32 = 16_000;

pub fn routes() -> Router<Arc<App>> {
    Router::new().route("/api/v1/listen", post(listen)).layer(DefaultBodyLimit::max(MAX_BYTES + 4096))
}

/// What was heard and where.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Heard {
    pub text: String,
    /// `home` — the gateway's or the computer's own server; `cloud` — a recognizer in the network.
    pub heard: &'static str,
}

/// The samples of a WAV: PCM, 16-bit, mono, 16 kHz — what the panel's recorder writes (panel/src/voice/mic.ts).
pub fn wav_pcm(bytes: &[u8]) -> Result<Vec<i16>, String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("нужен WAV".into());
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
                    return Err("в WAV нет описания звука".into());
                };
                if kind != 1 || bits != 16 {
                    return Err("нужен WAV PCM 16 бит".into());
                }
                if channels != 1 || rate != RATE {
                    return Err(format!("нужен WAV 16 кГц моно, а пришёл {rate} Гц, каналов: {channels}"));
                }
                return Ok(bytes[body..end].as_chunks::<2>().0.iter().map(|pair| i16::from_le_bytes(*pair)).collect());
            }
            _ => {}
        }
        at = body + size + (size & 1);
    }
    Err("в WAV нет звука".into())
}

/// The phrase recognized by «Слух», as the station's would be.
async fn listen(State(app): State<Arc<App>>, caller: Caller, body: Bytes) -> ApiResult<Heard> {
    caller.permitted(STATION_SAY)?;
    if body.len() > MAX_BYTES {
        return Err(ApiError::bad_request("Фраза больше 1 МБ."));
    }
    let pcm = wav_pcm(&body).map_err(ApiError::bad_request)?;
    let seconds = pcm.len() as f64 / RATE as f64;
    if seconds > MAX_SECONDS + 0.5 {
        return Err(ApiError::bad_request("Фраза длиннее 15 с."));
    }
    if !app.speech.recognizes() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "«Слух» выключен: распознавание не настроено (Настройки → Колонка → «Слух»).",
        ));
    }
    let audio = UtteranceAudio { pcm, from_ms: 0.0, to_ms: seconds * 1000.0, gain_db: 0.0 };
    let recognition = app
        .speech
        .recognize(&audio, &[], &CancellationToken::new())
        .await
        .map_err(|error| ApiError::bad_gateway(format!("«Слух» не ответил: {error:#}")))?;
    Ok(ok(Heard { text: recognition.text, heard: if app.speech.at_home() { "home" } else { "cloud" } }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(rate: u32, channels: u16, samples: &[i16]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend(b"RIFF");
        out.extend((36 + samples.len() as u32 * 2).to_le_bytes());
        out.extend(b"WAVEfmt ");
        out.extend(16u32.to_le_bytes());
        out.extend(1u16.to_le_bytes());
        out.extend(channels.to_le_bytes());
        out.extend(rate.to_le_bytes());
        out.extend((rate * 2).to_le_bytes());
        out.extend(2u16.to_le_bytes());
        out.extend(16u16.to_le_bytes());
        out.extend(b"data");
        out.extend((samples.len() as u32 * 2).to_le_bytes());
        for sample in samples {
            out.extend(sample.to_le_bytes());
        }
        out
    }

    /// The route: the right of «Сказать», a WAV of the panel, a phrase not longer than 15 s, «Слух» on.
    #[tokio::test]
    async fn hears_the_panels_phrase_only_with_the_right_and_hear_on() {
        use crate::config::{Config, Env};
        use crate::core::identity::principal::user_principal;
        use crate::core::identity::Role;

        let dir = tempfile::tempdir().unwrap();
        let text = |sub: &str| dir.path().join(sub).to_string_lossy().into_owned();
        let env = Env::from_pairs([
            ("STATE_DIR", String::new()),
            ("ASR_PROVIDER", "none".into()),
            ("PACKAGES_DIR", text("packages")),
            ("DATA_DIR", text("data")),
            ("VOICE_LAB_DIR", text("voice-lab")),
            ("TTS_MODELS_DIR", text("voices")),
        ]);
        let app = crate::app::App::build(Config::for_tests(&env)).unwrap();
        let admin = || Caller::local(app.clone(), Some(user_principal("u1", "Админ", Role::Admin, None)));
        let guest = || Caller::local(app.clone(), Some(user_principal("u3", "Гость", Role::Guest, None)));
        let phrase = Bytes::from(wav(16_000, 1, &vec![0; 16_000]));
        let status = |result: ApiResult<Heard>| result.err().map(|error| error.status);
        assert_eq!(status(listen(State(app.clone()), guest(), phrase.clone()).await), Some(StatusCode::FORBIDDEN));
        assert_eq!(
            status(listen(State(app.clone()), admin(), Bytes::from_static(b"RIFF")).await),
            Some(StatusCode::BAD_REQUEST)
        );
        let long = Bytes::from(wav(16_000, 1, &vec![0; 16_000 * 16]));
        assert_eq!(status(listen(State(app.clone()), admin(), long).await), Some(StatusCode::BAD_REQUEST));
        assert_eq!(status(listen(State(app.clone()), admin(), phrase).await), Some(StatusCode::CONFLICT), "«Слух» off");
    }

    #[test]
    fn reads_the_panels_wav_and_refuses_what_hear_cannot_take() {
        assert_eq!(wav_pcm(&wav(16_000, 1, &[1, -2, 3])).unwrap(), vec![1, -2, 3]);
        assert!(wav_pcm(&wav(24_000, 1, &[1])).unwrap_err().contains("16 кГц моно"));
        assert!(wav_pcm(&wav(16_000, 2, &[1, 2])).unwrap_err().contains("каналов: 2"));
        assert!(wav_pcm(b"not a wav at all").is_err());
        // Another chunk before the data is passed by.
        let mut with_list = wav(16_000, 1, &[7, 8]);
        let data = with_list.split_off(36);
        with_list.extend(b"LIST");
        with_list.extend(4u32.to_le_bytes());
        with_list.extend(b"INFO");
        with_list.extend(data);
        assert_eq!(wav_pcm(&with_list).unwrap(), vec![7, 8]);
    }
}
