//! The tests of `talk/listen.rs` that need the whole gateway (`App`): its crate does not have it, the program does.

use axum::body::Bytes;
use axum::extract::Query;
use axum::http::StatusCode;

use crate::platform::http::{ApiResult, Caller, Uses};
use crate::talk::listen::*;

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
    assert_eq!(
        status(listen(Uses::of(&app), Uses::of(&app), guest(), Query(Piece::default()), phrase.clone()).await),
        Some(StatusCode::FORBIDDEN)
    );
    assert_eq!(
        status(
            listen(Uses::of(&app), Uses::of(&app), admin(), Query(Piece::default()), Bytes::from_static(b"RIFF")).await
        ),
        Some(StatusCode::BAD_REQUEST)
    );
    let long = Bytes::from(wav(16_000, 1, &vec![0; 16_000 * 16]));
    assert_eq!(
        status(listen(Uses::of(&app), Uses::of(&app), admin(), Query(Piece::default()), long).await),
        Some(StatusCode::BAD_REQUEST)
    );
    assert_eq!(
        status(listen(Uses::of(&app), Uses::of(&app), admin(), Query(Piece::default()), phrase).await),
        Some(StatusCode::CONFLICT),
        "«Слух» off"
    );
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
