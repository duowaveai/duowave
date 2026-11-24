//! How the centre tells a join code (docs/plans/home-setup-flow.md, stage 2):
//!
//! - aloud, in Alice's voice, through a station of the centre whose session can speak now — the session mode
//!   (`StationSession::say_in`). In the relay mode, the default «Алиса», a centre on the station asks Yandex for the
//!   phrase over the relayed connection itself (`RelaySession::say_aloud`) and plays it on the station's own speaker
//!   with `aplay`; if any of that fails, the code is on the screens only (the plan's «пробный шаг»: never tried on a
//!   real station yet);
//! - in a window of the desktop app on a computer centre: the app gives its hook at its start (`set_shower`), as it
//!   gives its folder picker;
//! - on stdout of a computer without the app — a server without a screen;
//! - and in the log, on every centre.
//!
//! The admins' panels show it with «Разрешить» besides (join.rs, `home.join`).

use std::sync::{Arc, OnceLock};

use serde::Serialize;

use super::join::{JoinKind, Told};
use super::Parts;
#[cfg(feature = "yandex-station")]
use crate::gateway::uniproxy::station_session::{SayOutcome, SayVoice};

/// A code to show, for the desktop app's window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShownCode {
    pub id: String,
    /// Who asks in: «Ноутбук», «Колонка в спальне».
    pub name: String,
    pub kind: JoinKind,
    pub code: String,
    pub expires_ms: u64,
}

pub type Shower = Arc<dyn Fn(ShownCode) + Send + Sync>;

static SHOWER: OnceLock<Shower> = OnceLock::new();

/// The desktop app's window for a code (desktop/src-tauri/src/main.rs).
pub fn set_shower(shower: Shower) {
    let _ = SHOWER.set(shower);
}

/// The code as Alice says it: digit by digit, in pairs — «четыре семь, один девять, восемь два» — and once more
/// right away: six digits heard once were long to catch (the owner at his station, 22.11: the first time three of
/// them, the second time all).
pub fn words(code: &str, name: &str) -> String {
    let digits: Vec<String> = code.chars().map(|digit| digit.to_string()).collect();
    let pairs: Vec<String> = digits.chunks(2).map(|pair| pair.join(" ")).collect();
    crate::tr!("Код подключения для «{name}»: {code}. Ещё раз: {code}.", name = name, code = pairs.join(", "))
}

/// The code in pairs for the eye: «47 19 82».
pub fn shown(code: &str) -> String {
    let chars: Vec<char> = code.chars().collect();
    chars.chunks(2).map(|pair| pair.iter().collect::<String>()).collect::<Vec<_>>().join(" ")
}

/// Tells the code each way this centre has; what it told — the speech a moment later (`Told::spoken` says it was
/// started; a failure comes into the request's record).
pub fn tell(parts: Parts, id: &str, name: &str, kind: JoinKind, code: &str, expires_ms: u64) -> Told {
    let mut told = Told::default();
    tracing::warn!(target: "home", "join: the code for «{name}» ({}) is {}", kind.as_str(), shown(code));
    // A pretend station says it in its terminal (docs/plans/pretend-stations.md): it has no speaker of its own.
    if let Some(pretend) = crate::stations::pretend::get().filter(|pretend| pretend.is_station()) {
        crate::stations::pretend::say(pretend, &words(code, name));
        told.spoken = true;
        return told;
    }
    #[cfg(feature = "yandex-station")]
    say_on_station(&parts, id, name, code, &mut told);
    if let Some(shower) = SHOWER.get() {
        shower(ShownCode { id: id.to_string(), name: name.to_string(), kind, code: code.to_string(), expires_ms });
        told.window = true;
    } else if parts.placement.role() == crate::placement::service::Role::Pc
        && parts.config.placement.managed_by.as_deref() != Some("desktop")
    {
        // A computer without the app's window: a server — its terminal shows it.
        println!("{}", crate::tr!("Код подключения для «{name}»: {code}.", name = name, code = shown(code)));
        told.printed = true;
    }
    told
}

/// The code aloud on the Station of this centre, when one is on the line (the Station's adapter, src/gateway): in the
/// mode «Алиса» Yandex says it over the relayed connection, else our own session does.
#[cfg(feature = "yandex-station")]
fn say_on_station(parts: &Parts, id: &str, name: &str, code: &str, told: &mut Told) {
    let relay = if parts.placement.role() == crate::placement::service::Role::Station {
        parts.sessions.local_relay()
    } else {
        None
    };
    if let (None, Some(relay)) = (parts.sessions.speaking_on("*"), relay) {
        // The mode «Алиса»: Yandex says it over the relayed connection, the station's own speaker plays it.
        told.spoken = true;
        let text = words(code, name);
        let id = id.to_string();
        let parts = parts.clone();
        let language = crate::locale::of_request();
        tokio::spawn(crate::locale::with_language(language, async move {
            let played = match relay.say_aloud(&text).await {
                Ok(ogg) => play(&ogg).await,
                Err(error) => Err(error),
            };
            if let Err(error) = played {
                tracing::warn!(target: "home", "join: the station did not say the code (relay mode): {error}");
                parts.joins.told_failed(&id);
            }
        }));
    } else if let Some(session) = parts.sessions.speaking_on("*") {
        told.spoken = true;
        let text = words(code, name);
        let id = id.to_string();
        let language = crate::locale::of_request();
        let parts = parts.clone();
        tokio::spawn(crate::locale::with_language(language, async move {
            let outcome = session.say_in(text, None, Some(SayVoice::Alice)).await;
            if matches!(outcome, SayOutcome::Failed) {
                tracing::warn!(target: "home", "join: the station did not say the code");
                parts.joins.told_failed(&id);
            }
        }));
    }
}

/// Ogg/Opus as Yandex sends it → a WAV of 16 kHz mono PCM, as `aplay` takes it.
pub fn ogg_to_wav(ogg: &[u8]) -> Option<Vec<u8>> {
    use crate::speech::audio::ogg_opus_demuxer::{OggOpusDemuxer, OggOpusEvent};
    use crate::speech::audio::opus_pcm_decoder::{OpusPcmDecoder, SPEECH_SAMPLE_RATE};
    let mut demuxer = OggOpusDemuxer::new();
    let mut decoder = OpusPcmDecoder::new();
    let mut pcm: Vec<i16> = Vec::new();
    for event in demuxer.push(ogg) {
        match event {
            OggOpusEvent::Head { head, .. } => decoder.reset(&head),
            OggOpusEvent::Packet { data, .. } => pcm.extend(decoder.decode(&data)),
        }
    }
    (!pcm.is_empty()).then(|| crate::speech::audio::pcm::encode_wav(&pcm, SPEECH_SAMPLE_RATE))
}

#[cfg(feature = "yandex-station")]
/// The phrase on the station's own speaker: `aplay` reads the WAV from its stdin (ALSA's dmix shares the device with
/// the firmware). Never killed half way — a player cut mid-stream once left a station mute until its reboot
/// (station-client/call-client.sh); a player still going after its time is left to finish.
async fn play(ogg: &[u8]) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    let wav = ogg_to_wav(ogg).ok_or_else(|| "the phrase's audio did not decode".to_string())?;
    let mut child = tokio::process::Command::new("aplay")
        .args(["-q", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|error| format!("aplay: {error}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(&wav).await.map_err(|error| format!("aplay: {error}"))?;
    }
    match tokio::time::timeout(std::time::Duration::from_secs(30), child.wait()).await {
        Ok(Ok(status)) if status.success() => Ok(()),
        Ok(Ok(status)) => Err(format!("aplay ended with {status}")),
        Ok(Err(error)) => Err(format!("aplay: {error}")),
        Err(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The phrase Yandex sends (Ogg/Opus) turns into a WAV `aplay` plays: our own encoder stands in for Yandex's.
    #[test]
    fn yandex_s_ogg_opus_becomes_a_wav_for_aplay() {
        use crate::speech::audio::ogg_opus_encoder::{encode_ogg_opus, EncodeRate};
        let tone: Vec<i16> = (0..16_000).map(|n| ((n as f32 * 0.05).sin() * 8000.0) as i16).collect();
        let ogg = encode_ogg_opus(&tone, EncodeRate::Speech).unwrap();
        let wav = ogg_to_wav(&ogg).expect("a WAV");
        assert_eq!(&wav[..4], b"RIFF");
        assert!(wav.len() > 44 + 16_000, "about a second of 16-bit audio: {}", wav.len());
        assert!(ogg_to_wav(b"not ogg").is_none());
    }

    #[test]
    fn the_code_is_said_digit_by_digit_in_pairs_and_shown_in_pairs() {
        assert_eq!(shown("471982"), "47 19 82");
        assert_eq!(words("471982", "Ноутбук"), "Код подключения для «Ноутбук»: 4 7, 1 9, 8 2. Ещё раз: 4 7, 1 9, 8 2.");
    }
}
