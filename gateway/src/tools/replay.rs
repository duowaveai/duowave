//! `duowave replay`: a recorded station request through the speech pipeline (src/tools/replay-voice.ts, `pnpm
//! replay`). The same Ogg/Opus decoding, gain, VAD and end-of-utterance decisions as live (they run on audio time),
//! plus recognition when ASR is configured: interim results while the command is spoken (with quiet speech VAD does
//! not hear and its end), the final one, and the look at all audio after the wake word. For tuning VAD and checking
//! recognition on real recordings without the station.
//!
//!   duowave replay data/sessions/<day>/<session>/audio/<time>_<request>.in.ogg [--no-asr] [--wav out.wav]
//!
//! Settings come from the environment and .env, exactly as for the server (VAD_*, AUDIO_GAIN*, ASR_*):
//! `VAD_END_SILENCE_MS=900 duowave replay …`.

use std::path::Path;
use std::sync::Arc;

use anyhow::Context as _;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::{flag, option, Usage};
use crate::config::{GatewayConfig, SpeechConfig};
use crate::gateway::protocol::proto_object::js_number;
use crate::skills::quick_commands::is_stop;
use crate::speech::asr::speech_recognizer::{create_recognizer, SpeechRecognizer};
use crate::speech::audio::opus_pcm_decoder::SPEECH_SAMPLE_RATE;
use crate::speech::audio::pcm::encode_wav;
use crate::speech::interim_recognition::{InterimOptions, InterimRecognition};
use crate::speech::speech_pipeline::{SpeechEvent, UtteranceAudio};
use crate::speech::SpeechService;

const USAGE: &str = "usage: duowave replay <file.in.ogg> [--no-asr] [--wav out.wav]";
const CHUNK: usize = 4096;
/// Command audio goes in small pieces when interim recognition is replayed: ~one Ogg page each.
const INTERIM_CHUNK: usize = 128;
/// VAD characters per row of the trace.
const PER_ROW: usize = 32;

/// A recording to replay: its name as given, its bytes and where SpotterEnd came, when the journal knows.
pub struct Recording<'a> {
    pub name: &'a str,
    pub bytes: &'a [u8],
    pub spotter_end: Option<usize>,
}

pub fn main(args: &[String]) -> anyhow::Result<()> {
    let Some(file) = file_argument(args) else { return Err(Usage(USAGE.into()).into()) };
    let wav_out = option(args, "wav");
    let config = super::config()?;
    let bytes = std::fs::read(file).with_context(|| format!("cannot read {file}"))?;
    let spotter_end = spotter_end_from_journal(Path::new(file));
    let recognizer = if flag(args, "no-asr") { None } else { create_recognizer(&config.speech.asr) };
    let recording = Recording { name: file, bytes: &bytes, spotter_end };
    let written = super::runtime()?.block_on(replay(
        &config.gateway,
        config.speech.clone(),
        recognizer,
        &recording,
        &mut |line| println!("{line}"),
    ));
    if let (Some(out), Some(audio)) = (wav_out, written) {
        std::fs::write(out, encode_wav(&audio.pcm, SPEECH_SAMPLE_RATE))
            .with_context(|| format!("cannot write {out}"))?;
        println!("written {out}");
    }
    Ok(())
}

/// The recording: the first argument that is neither a flag nor the value of `--wav`.
fn file_argument(args: &[String]) -> Option<&str> {
    args.iter()
        .enumerate()
        .find(|(index, arg)| !arg.starts_with("--") && (*index == 0 || args[index - 1] != "--wav"))
        .map(|(_, arg)| arg.as_str())
}

/// SpotterEnd byte offset of this recording from the session journal next to the audio folder.
pub fn spotter_end_from_journal(file: &Path) -> Option<usize> {
    let file = super::absolute(file);
    let journal = file.parent()?.parent()?.join("events.jsonl");
    let text = std::fs::read_to_string(journal).ok()?;
    let name = file.file_name()?.to_str()?;
    let line = text.lines().find(|line| line.contains("\"voiceTimeline\"") && line.contains(name))?;
    let record: Value = serde_json::from_str(line).ok()?;
    let data = &record["data"];
    let at = data["audio"]["spotterEndAtBytes"].as_u64().or_else(|| data["spotterEndAtBytes"].as_u64())?;
    usize::try_from(at).ok()
}

fn seconds(ms: f64) -> String {
    format!("{:.2}s", ms / 1000.0)
}

fn seconds_or_dash(ms: Option<f64>) -> String {
    ms.map_or_else(|| "-".into(), seconds)
}

/// A number as JavaScript prints it: 12.5, 0, -23.4.
fn js(value: f64) -> String {
    js_number(value).to_string()
}

fn describe(event: &SpeechEvent) -> String {
    match event {
        SpeechEvent::SpeechStart { at_ms } => format!("speech starts at {}", seconds(*at_ms)),
        SpeechEvent::End { reason, at_ms } => format!("{} at {}", reason.as_str(), seconds(*at_ms)),
        SpeechEvent::Error(message) => format!("error: {message}"),
    }
}

/// One recognition and its line; never fails, the replay prints what it can. The text is None when it failed.
struct Heard {
    text: Option<String>,
    line: String,
}

async fn recognize(speech: &SpeechService, audio: &UtteranceAudio) -> Heard {
    match speech.recognize(audio, &[], &CancellationToken::new()).await {
        Ok(recognition) => Heard {
            line: format!("\"{}\" → \"{}\" in {} ms", recognition.raw, recognition.text, recognition.latency_ms),
            text: Some(recognition.text),
        },
        Err(error) => Heard { text: None, line: format!("failed: {error:#}") },
    }
}

/// Replays the recording, saying each line of the report; the audio that recognition got (or all audio after the
/// wake word, when that was looked at), for `--wav`. `recognizer` None: no recognition (`--no-asr`, ASR_PROVIDER=none).
pub async fn replay(
    gateway: &GatewayConfig,
    speech: SpeechConfig,
    recognizer: Option<Arc<dyn SpeechRecognizer>>,
    recording: &Recording<'_>,
    say: &mut dyn FnMut(String),
) -> Option<UtteranceAudio> {
    let description = recognizer.as_ref().map(|recognizer| recognizer.description());
    let guard_ms = speech.endpoint.guard_ms;
    let speech = SpeechService::with_recognizer(speech, recognizer);
    let bytes = recording.bytes;
    let mut pipeline = speech.create_pipeline(Some("spotter"), None);
    let mut events: Vec<String> = Vec::new();

    // Feed it as the station did: the wake word buffer, SpotterEnd, then the command. Interim recognition runs on
    // audio time here (every ASR_INTERIM_INTERVAL_MS of audio); live it is checked with each interim result, so live
    // interims come a little less often.
    let split = recording.spotter_end.unwrap_or(bytes.len()).min(bytes.len());
    for chunk in bytes[..split].chunks(CHUNK) {
        pipeline.push(chunk);
    }
    if recording.spotter_end.is_some() {
        pipeline.mark_spotter_end();
    }
    let interim_ms = if description.is_some() { gateway.interim_interval_ms } else { 0 };
    let mut interim = InterimRecognition::new(InterimOptions {
        interval_ms: interim_ms,
        quiet_speech: gateway.quiet_speech,
        quiet_end_ms: gateway.quiet_end_ms,
        unchanged_end_ms: gateway.unchanged_end_ms,
        stop_end_ms: gateway.stop_end_ms,
        guard_ms,
    });
    let chunk = if interim_ms > 0 { INTERIM_CHUNK } else { CHUNK };
    let mut interims: Vec<String> = Vec::new();
    for piece in bytes[split..].chunks(chunk) {
        pipeline.push(piece);
        if interim_ms == 0 {
            continue;
        }
        events.extend(pipeline.flush().iter().map(describe));
        let Some(job) = interim.next(&pipeline) else { continue };
        let heard = recognize(&speech, &job.audio).await;
        // As live: a failed interim recognition changes nothing.
        let end = heard.text.and_then(|text| interim.accept(&mut pipeline, &job, &text, is_stop(&text)).end);
        let source = if job.all_audio { " (all audio)" } else { "" };
        interims.push(format!("  at {}{source}: {}", seconds(job.at_ms), heard.line));
        if let Some(end) = end {
            // The gateway ends the command here and stops taking audio.
            events.push(format!("{}: end at {}", end.describe(), seconds(job.at_ms)));
            pipeline.stop();
            break;
        }
    }
    events.extend(pipeline.flush().iter().map(describe));

    let report = pipeline.report();
    let spotter = match recording.spotter_end {
        Some(at) => format!("at {at} B (journal)"),
        None => "not in the journal".into(),
    };
    say(format!("{} ({} B), SpotterEnd {spotter}", recording.name, bytes.len()));
    let chains: Vec<String> = report.chains_at_ms.iter().map(|&at| seconds(at as f64)).collect();
    let source = report
        .command_start_source
        .and_then(|source| serde_json::to_value(source).ok())
        .and_then(|source| source.as_str().map(str::to_string))
        .unwrap_or_else(|| "-".into());
    let errors =
        if report.decode_errors > 0 { format!(", decode errors {}", report.decode_errors) } else { String::new() };
    say(format!(
        "audio {}, chains at {}, command from {} ({source}), gain +{} dB (wake word loudest window {} dBFS){errors}",
        seconds(report.decoded_ms as f64),
        chains.join(", "),
        seconds_or_dash(report.command_start_ms.map(|at| at as f64)),
        js(report.gain_db),
        report.wake_word_dbfs.map_or_else(|| "-".into(), js),
    ));
    say(format!(
        "VAD {}, {} ms per character: # speech, + unsure, . low, _ silence",
        speech.vad_status(),
        js(report.vad.window_ms)
    ));
    let trace: Vec<char> = report.vad.trace.chars().collect();
    for (row, characters) in trace.chunks(PER_ROW).enumerate() {
        let start_ms = (row * PER_ROW) as f64 * report.vad.window_ms;
        say(format!("  {:>6} {}", seconds(start_ms), characters.iter().collect::<String>()));
    }
    say(if events.is_empty() {
        "no end of utterance inside the recording (the station closed it first)".into()
    } else {
        events.join("; ")
    });
    if !interims.is_empty() {
        say(format!("interim recognition (ASR_INTERIM_INTERVAL_MS={interim_ms}):\n{}", interims.join("\n")));
    }

    // Quiet speech goes to recognition whole, as in the gateway.
    let audio = if interim.quiet() { None } else { pipeline.utterance_audio() };
    let mut text = String::new();
    match &audio {
        Some(audio) => {
            say(format!(
                "recognition audio {}–{} → {} after cutting pauses, gain +{} dB",
                seconds(audio.from_ms),
                seconds(audio.to_ms),
                seconds((audio.pcm.len() * 1000) as f64 / f64::from(SPEECH_SAMPLE_RATE)),
                js(audio.gain_db)
            ));
            if let Some(description) = &description {
                let heard = recognize(&speech, audio).await;
                text = heard.text.unwrap_or_default();
                say(format!("ASR {description}: {}", heard.line));
            }
        }
        None => say(if interim.quiet() {
            "quiet speech: recognition takes all audio after the wake word".into()
        } else {
            "no speech after the wake word".into()
        }),
    }

    // All audio after the wake word, when the final text is empty or the speech was quiet (ASR_QUIET_SPEECH).
    let all = if text.is_empty() && gateway.quiet_speech { pipeline.command_audio() } else { None };
    let fallback = all.filter(|all| audio.as_ref().is_none_or(|audio| all.pcm.len() > audio.pcm.len()));
    if let Some(fallback) = &fallback {
        say(format!(
            "all audio after the wake word {}–{}, gain +{} dB",
            seconds(fallback.from_ms),
            seconds(fallback.to_ms),
            js(fallback.gain_db)
        ));
        if let Some(description) = &description {
            say(format!("ASR {description}: {}", recognize(&speech, fallback).await.line));
        }
    }
    // Quiet speech is a whisper only without a voice (the station session, VOICED_SPEECH_RUN).
    let voicing = pipeline.command_voicing();
    say(format!("windows with a pitch after the wake word: {}, {} in a row", voicing.windows, voicing.longest_run));
    pipeline.close();
    audio.or(fallback)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use futures::future::{BoxFuture, FutureExt};

    use super::*;
    use crate::config::{Config, Env};
    use crate::speech::test_audio::{concat, silence, station_audio, voice};

    /// A recognizer that hears what it is told to, and counts what it is asked.
    struct Heard {
        text: String,
        asked: Mutex<Vec<usize>>,
    }

    impl SpeechRecognizer for Heard {
        fn description(&self) -> String {
            "heard via test".into()
        }

        fn model(&self) -> String {
            "test".into()
        }

        fn recognize<'a>(
            &'a self,
            pcm: &'a [i16],
            _sample_rate: u32,
            _cancel: &'a CancellationToken,
        ) -> BoxFuture<'a, anyhow::Result<String>> {
            self.asked.lock().unwrap().push(pcm.len());
            futures::future::ready(Ok(self.text.clone())).boxed()
        }

        fn probe(&self) -> BoxFuture<'_, String> {
            futures::future::ready("reachable".to_string()).boxed()
        }

        fn refresh(&self) -> BoxFuture<'static, ()> {
            futures::future::ready(()).boxed()
        }

        fn forget(&self) {}
    }

    fn config(pairs: &[(&str, &str)]) -> Config {
        Config::load(&Env::from_pairs([("VAD_ENGINE", "energy")].into_iter().chain(pairs.iter().copied()))).unwrap()
    }

    /// «Алиса» at the end of the wake word buffer, then a command and the silence after it.
    fn request() -> crate::speech::test_audio::StationAudio {
        station_audio(
            &concat(&[&silence(1200.0), &voice(800.0, -50.0)]),
            &concat(&[&voice(800.0, -50.0), &silence(1500.0)]),
        )
    }

    async fn run(
        config: &Config,
        recognizer: Option<Arc<dyn SpeechRecognizer>>,
        recording: &Recording<'_>,
    ) -> (Vec<String>, Option<UtteranceAudio>) {
        let mut lines = Vec::new();
        let audio =
            replay(&config.gateway, config.speech.clone(), recognizer, recording, &mut |line| lines.push(line)).await;
        (lines, audio)
    }

    #[tokio::test]
    async fn replays_a_request_through_the_pipeline_with_interim_and_final_recognition() {
        let audio = request();
        let heard = Arc::new(Heard { text: "Алиса, включи свет.".into(), asked: Mutex::default() });
        let recording = Recording { name: "x.in.ogg", bytes: &audio.bytes, spotter_end: Some(audio.spotter_end_at) };
        let (lines, written) = run(&config(&[]), Some(heard.clone()), &recording).await;
        let text = lines.join("\n");

        assert_eq!(
            lines[0],
            format!("x.in.ogg ({} B), SpotterEnd at {} B (journal)", audio.bytes.len(), audio.spotter_end_at)
        );
        assert!(
            lines[1]
                .contains("s, chains at 0.00s, 2.00s, command from 2.00s (station), gain +28 dB (wake word loudest"),
            "{}",
            lines[1]
        );
        assert_eq!(lines[2], "VAD energy, 32 ms per character: # speech, + unsure, . low, _ silence");
        assert!(lines[3].starts_with("   0.00s "), "{}", lines[3]);
        assert!(text.contains("speech starts at "), "{text}");
        assert!(text.contains("endOfUtterance at "), "{text}");
        assert!(text.contains("interim recognition (ASR_INTERIM_INTERVAL_MS=500):\n  at "), "{text}");
        assert!(text.contains(": \"Алиса, включи свет.\" → \"Включи свет.\" in "), "{text}");
        assert!(text.contains("ASR heard via test: \"Алиса, включи свет.\" → \"Включи свет.\" in "), "{text}");
        let recognition = lines.iter().find(|line| line.starts_with("recognition audio ")).unwrap();
        assert!(recognition.contains(" after cutting pauses, gain +"), "{recognition}");
        // The final text is not empty: no second look at all audio.
        assert!(!text.contains("all audio after the wake word"), "{text}");
        assert!(lines.last().unwrap().starts_with("windows with a pitch after the wake word: "), "{text}");

        // What went to the final recognition is what --wav writes.
        let written = written.unwrap();
        assert_eq!(heard.asked.lock().unwrap().last(), Some(&written.pcm.len()));
        assert!(written.from_ms < 2000.0 && written.to_ms > 2800.0, "{} – {}", written.from_ms, written.to_ms);
    }

    #[tokio::test]
    async fn replays_without_recognition_and_takes_the_end_of_audio_without_spotter_end() {
        let audio = request();
        let recording = Recording { name: "x.in.ogg", bytes: &audio.bytes, spotter_end: None };
        let (lines, written) = run(&config(&[]), None, &recording).await;
        let text = lines.join("\n");
        assert!(lines[0].ends_with("SpotterEnd not in the journal"), "{}", lines[0]);
        assert!(!text.contains("ASR "), "{text}");
        assert!(!text.contains("interim recognition"), "{text}");
        // Without SpotterEnd the command starts where the station restarted its stream; all audio is decoded.
        assert!(
            lines[1].starts_with("audio 4.30s, chains at 0.00s, 2.00s, command from 2.00s (stream)"),
            "{}",
            lines[1]
        );
        assert!(written.is_some(), "{text}");
    }

    #[tokio::test]
    async fn takes_the_vad_settings_from_the_environment() {
        let audio = request();
        let recording = Recording { name: "x.in.ogg", bytes: &audio.bytes, spotter_end: Some(audio.spotter_end_at) };
        let ends = |lines: &[String]| -> String {
            lines.iter().find(|line| line.contains("endOfUtterance at ")).cloned().unwrap_or_default()
        };
        let (quick, _) = run(&config(&[("VAD_END_SILENCE_MS", "300")]), None, &recording).await;
        let (slow, _) = run(&config(&[("VAD_END_SILENCE_MS", "1200")]), None, &recording).await;
        assert!(!ends(&quick).is_empty() && !ends(&slow).is_empty(), "{quick:?} {slow:?}");
        assert_ne!(ends(&quick), ends(&slow), "a longer end silence ends later");
    }

    #[tokio::test]
    async fn looks_at_all_audio_after_the_wake_word_when_vad_hears_no_command() {
        // A whisper below the energy VAD's threshold after the wake word.
        let audio = station_audio(
            &concat(&[&silence(1200.0), &voice(800.0, -30.0)]),
            &concat(&[&silence(400.0), &voice(1200.0, -75.0), &silence(3000.0)]),
        );
        let heard = Arc::new(Heard { text: "Привет.".into(), asked: Mutex::default() });
        let recording = Recording { name: "q.in.ogg", bytes: &audio.bytes, spotter_end: Some(audio.spotter_end_at) };
        let (lines, written) = run(&config(&[]), Some(heard), &recording).await;
        let text = lines.join("\n");
        assert!(text.contains(" (all audio): \"Привет.\""), "{text}");
        assert!(text.contains("quiet speech unchanged: end at "), "{text}");
        assert!(text.contains("quiet speech: recognition takes all audio after the wake word"), "{text}");
        assert!(text.contains("all audio after the wake word "), "{text}");
        assert!(written.is_some());
    }

    #[test]
    fn finds_the_recording_among_the_flags_and_the_wav_out() {
        let args = |list: &[&str]| list.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
        assert_eq!(file_argument(&args(&["--wav", "out.wav", "a.in.ogg", "--no-asr"])), Some("a.in.ogg"));
        assert_eq!(file_argument(&args(&["a.in.ogg", "--wav", "out.wav"])), Some("a.in.ogg"));
        assert_eq!(file_argument(&args(&["--no-asr"])), None);
        assert_eq!(file_argument(&args(&["--wav", "out.wav"])), None);
    }

    #[test]
    fn reads_spotter_end_from_the_sessions_journal_beside_the_audio_folder() {
        let dir = tempfile::tempdir().unwrap();
        let audio = dir.path().join("audio");
        std::fs::create_dir_all(&audio).unwrap();
        let file = audio.join("021100_req-1.in.ogg");
        let lines = [
            r#"{"ts":"2026-11-06T02:11:00.000Z","dir":"in","kind":"voiceInput","summary":"req-1"}"#,
            r#"{"ts":"2026-11-06T02:11:03.000Z","dir":"note","kind":"voiceTimeline","data":{"audio":{"bytes":9000,"spotterEndAtBytes":4321,"file":"/x/audio/021100_req-1.in.ogg"}}}"#,
            r#"{"ts":"2026-11-06T02:12:03.000Z","dir":"note","kind":"voiceTimeline","data":{"spotterEndAtBytes":77,"file":"021200_req-2.in.ogg"}}"#,
        ];
        std::fs::write(dir.path().join("events.jsonl"), lines.join("\n") + "\n").unwrap();
        assert_eq!(spotter_end_from_journal(&file), Some(4321));
        // The older journals kept it beside the audio.
        assert_eq!(spotter_end_from_journal(&audio.join("021200_req-2.in.ogg")), Some(77));
        assert_eq!(spotter_end_from_journal(&audio.join("missing.in.ogg")), None);
        assert_eq!(spotter_end_from_journal(&dir.path().join("elsewhere/x/y.in.ogg")), None);
    }
}
