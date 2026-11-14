//! Hearing for apps (`system/hearing` → `recognize`): a file of the app's own folder → its text, the language and each
//! word's time, by the same recognition that hears the household — the computer's own engine, the configured server
//! or the station's chain, whichever the owner set up. An app never calls a recognizer itself. A WAV is read as it is;
//! another kind (MP3, M4A, MP4, OGG, WebM) through the computer's ffmpeg, when there is one.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::speech::asr::speech_recognizer::SpeechRecognizer;
use crate::speech::asr::Heard;

/// The longest audio one recognition takes.
const MAX_SECONDS: usize = 600;

/// An audio file → 16-bit PCM and its rate.
pub async fn pcm_of(path: &Path) -> Result<(Vec<i16>, u32), String> {
    let bytes = tokio::fs::read(path).await.map_err(|error| format!("{}: {error}", file_name(path)))?;
    if let Ok(wav) = crate::speech::audio::pcm::decode_wav(&bytes) {
        return Ok((wav.pcm, wav.sample_rate));
    }
    let ffmpeg = crate::gateway::media::stream::find_ffmpeg().ok_or_else(|| {
        crate::tr!("{file}: не WAV, а ffmpeg на этом компьютере нет — слух берёт только WAV", file = file_name(path))
    })?;
    let output = tokio::process::Command::new(ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-vn", "-ac", "1", "-ar", "16000", "-f", "wav", "-"])
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|error| format!("ffmpeg: {error}"))?;
    if !output.status.success() {
        let why = String::from_utf8_lossy(&output.stderr);
        return Err(crate::tr!(
            "{file}: звук не прочитался — {why}",
            file = file_name(path),
            why = why.trim().chars().take(200).collect::<String>()
        ));
    }
    let wav = crate::speech::audio::pcm::decode_wav(&output.stdout)
        .map_err(|error| format!("{}: {error}", file_name(path)))?;
    Ok((wav.pcm, wav.sample_rate))
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Recognizes the whole file: up to ten minutes, four seconds of waiting per second of audio, not less than half a
/// minute.
pub async fn recognize_file(
    recognizer: &Arc<dyn SpeechRecognizer>,
    path: &Path,
    language: Option<&str>,
    cancel: &CancellationToken,
) -> Result<Heard, String> {
    let (pcm, rate) = pcm_of(path).await?;
    let seconds = pcm.len() / rate.max(1) as usize;
    if seconds > MAX_SECONDS {
        return Err(crate::tr!(
            "{file}: длиннее {minutes} мин — слух берёт до {max} мин",
            file = file_name(path),
            minutes = seconds / 60,
            max = MAX_SECONDS / 60
        ));
    }
    if !recognizer.available() {
        return Err(crate::tr!("слух сейчас недоступен: распознавание не настроено или далеко"));
    }
    let deadline = Duration::from_secs((seconds as u64 * 4).max(30));
    tokio::time::timeout(deadline, recognizer.recognize_heard(&pcm, rate, language, cancel))
        .await
        .map_err(|_| crate::tr!("слух не ответил за {seconds} с", seconds = deadline.as_secs()))?
        .map_err(|error| crate::tr!("слух не распознал: {error}", error = error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::future::BoxFuture;

    /// A recognizer that hears «привет мир» with times.
    struct Words;

    impl SpeechRecognizer for Words {
        fn description(&self) -> String {
            "words".into()
        }
        fn model(&self) -> String {
            "words".into()
        }
        fn recognize<'a>(
            &'a self,
            _pcm: &'a [i16],
            _rate: u32,
            _cancel: &'a CancellationToken,
        ) -> BoxFuture<'a, anyhow::Result<String>> {
            Box::pin(async { Ok("привет мир".into()) })
        }
        fn recognize_heard<'a>(
            &'a self,
            pcm: &'a [i16],
            rate: u32,
            language: Option<&'a str>,
            _cancel: &'a CancellationToken,
        ) -> BoxFuture<'a, anyhow::Result<Heard>> {
            let length = pcm.len() as f64 / f64::from(rate);
            Box::pin(async move {
                Ok(Heard {
                    text: "привет мир".into(),
                    language: language.map(str::to_string),
                    words: vec![
                        crate::speech::asr::HeardWord { word: "привет".into(), start: 0.1, end: length / 2.0 },
                        crate::speech::asr::HeardWord {
                            word: "мир".into(),
                            start: length / 2.0 + 0.05,
                            end: length - 0.1,
                        },
                    ],
                })
            })
        }
        fn probe(&self) -> BoxFuture<'_, String> {
            Box::pin(async { "ok".into() })
        }
        fn refresh(&self) -> BoxFuture<'static, ()> {
            Box::pin(async {})
        }
        fn forget(&self) {}
    }

    #[tokio::test]
    async fn a_wav_of_the_folder_is_heard_with_the_words_times_and_a_broken_file_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("01.wav");
        std::fs::write(&wav, crate::speech::audio::pcm::encode_wav(&vec![0i16; 32_000], 16_000)).unwrap();
        let recognizer: Arc<dyn SpeechRecognizer> = Arc::new(Words);
        let heard = recognize_file(&recognizer, &wav, Some("ru"), &CancellationToken::new()).await.unwrap();
        assert_eq!(heard.text, "привет мир");
        assert_eq!(heard.language.as_deref(), Some("ru"));
        assert_eq!(heard.words.len(), 2);
        assert!((heard.words[1].end - 1.9).abs() < 1e-9, "times from the audio's start, seconds");
        let broken = dir.path().join("broken.wav");
        std::fs::write(&broken, b"not a sound").unwrap();
        assert!(recognize_file(&recognizer, &broken, None, &CancellationToken::new()).await.is_err());
    }
}
