//! `duowave call-sim`: a station for our calls without the station (src/tools/station-call-sim.ts, `pnpm call-sim`).
//! Joins the hub the way a station client does (`/calls/ws`, role `sim`), answers a call, plays a recording as its
//! microphone in real time and keeps what it hears in a WAV. The call page (/calls) calls it like the real station.
//!
//!   duowave call-sim [--url ws://127.0.0.1:<PORT>/calls/ws] [--device default] [--play voice.wav]
//!                    [--record heard.wav] [--answer-after 1500] [--call "Мама"]
//!
//! --play takes a WAV (16-bit; mono, or stereo mixed down; resampled to 16 kHz) or raw 16 kHz PCM16LE; without it,
//! a tone. The hub lets the simulator in from this computer only.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context as _;
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use super::{option, Usage};
use crate::calls::call_hub::{pcm_from_bytes, pcm_to_bytes};
use crate::calls::{CALLS_WS_PATH, FRAME_SAMPLES};
use crate::speech::audio::pcm::{decode_wav, encode_wav, js_round, try_resample};

const RATE: u32 = 16000;
/// One frame of the microphone: 20 ms.
const FRAME: Duration = Duration::from_millis(20);
/// How the simulator is called in the hub.
pub const NAME: &str = "Колонка (имитатор)";

#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    pub url: String,
    pub device: String,
    pub play: Option<PathBuf>,
    pub record: PathBuf,
    pub answer_after: Duration,
    /// Call the party whose name has this in it, once in the hub.
    pub call: Option<String>,
}

/// The gateway's calls socket on this computer, at its PORT.
pub fn default_url(port: u16) -> String {
    format!("ws://127.0.0.1:{port}{CALLS_WS_PATH}")
}

impl Options {
    /// The options as given; `default_url` is asked only without `--url`.
    pub fn parse(args: &[String], default_url: impl FnOnce() -> anyhow::Result<String>) -> anyhow::Result<Options> {
        let url = match option(args, "url") {
            Some(url) => url.to_string(),
            None => default_url()?,
        };
        let answer_after = option(args, "answer-after").unwrap_or("1500");
        let answer_after = answer_after
            .trim()
            .parse::<u64>()
            .map_err(|_| Usage(format!("--answer-after takes milliseconds, not {answer_after:?}")))?;
        Ok(Options {
            url,
            device: option(args, "device").unwrap_or("default").to_string(),
            play: option(args, "play").map(PathBuf::from),
            record: PathBuf::from(option(args, "record").unwrap_or("heard.wav")),
            answer_after: Duration::from_millis(answer_after),
            call: option(args, "call").map(str::to_string),
        })
    }
}

pub fn main(args: &[String]) -> anyhow::Result<()> {
    let options = Options::parse(args, || Ok(default_url(super::config()?.app.port)))?;
    let microphone = load(options.play.as_deref())?;
    super::runtime()?.block_on(run(&options, &microphone, &mut |line| println!("{line}")))?;
    Ok(())
}

/// 16 kHz mono samples from a WAV (16-bit PCM, any rate, mono or stereo) or raw PCM16LE; a 440 Hz tone of 5 s
/// without a file.
pub fn load(file: Option<&Path>) -> anyhow::Result<Vec<i16>> {
    let Some(file) = file else {
        return Ok((0..RATE as usize * 5)
            .map(|i| {
                let phase = (2.0 * std::f64::consts::PI * 440.0 * i as f64) / f64::from(RATE);
                js_round(phase.sin() * 6000.0) as i16
            })
            .collect());
    };
    let bytes = std::fs::read(file).with_context(|| format!("cannot read {}", file.display()))?;
    let samples = if bytes.get(0..4) == Some(b"RIFF") {
        let wav = decode_wav(&bytes).with_context(|| format!("cannot play {}", file.display()))?;
        try_resample(&wav.pcm, wav.sample_rate, RATE)?
    } else {
        pcm_from_bytes(&bytes)
    };
    anyhow::ensure!(!samples.is_empty(), "nothing to play in {}", file.display());
    Ok(samples)
}

/// The party to call from the hub's welcome: the first whose name has `name` in it, the simulator itself aside.
fn target(welcome: &Value, name: &str) -> Option<String> {
    let name = name.to_lowercase();
    welcome["endpoints"].as_array()?.iter().find_map(|party| {
        let id = party["id"].as_str()?;
        let found = party["name"].as_str()?.to_lowercase().contains(&name) && Some(id) != welcome["id"].as_str();
        found.then(|| id.to_string())
    })
}

async fn tick(sending: &mut Option<tokio::time::Interval>) {
    match sending {
        Some(interval) => {
            interval.tick().await;
        }
        None => std::future::pending().await,
    }
}

/// One call: joins, answers (or calls `--call`), plays the microphone round and round while connected, and when the
/// call ends writes what it heard to `--record`. Says each event of the hub and the end; the samples heard, None
/// when the gateway closed the socket before a call ended.
pub async fn run(options: &Options, microphone: &[i16], say: &mut dyn FnMut(String)) -> anyhow::Result<Option<usize>> {
    anyhow::ensure!(!microphone.is_empty(), "nothing to play");
    let (socket, _) = tokio_tungstenite::connect_async(options.url.as_str())
        .await
        .map_err(|error| anyhow::anyhow!("cannot connect to {}: {error}", options.url))?;
    let (mut sink, mut incoming) = socket.split();
    let hello = json!({ "type": "hello", "role": "sim", "device": options.device, "name": NAME });
    sink.send(Message::Text(hello.to_string().into())).await?;
    let (accepts, mut accepted) = mpsc::unbounded_channel::<Value>();
    let mut heard: Vec<i16> = Vec::new();
    let mut sending: Option<tokio::time::Interval> = None;
    let mut offset = 0;
    loop {
        tokio::select! {
            message = incoming.next() => {
                let message = match message {
                    Some(Ok(message)) => message,
                    Some(Err(_)) | None => break,
                };
                let text = match message {
                    Message::Binary(bytes) => {
                        heard.extend(pcm_from_bytes(&bytes));
                        continue;
                    }
                    Message::Text(text) => text,
                    Message::Close(_) => break,
                    _ => continue,
                };
                let Ok(event) = serde_json::from_str::<Value>(text.as_str()) else { continue };
                say(event.to_string());
                match event["type"].as_str() {
                    Some("welcome") => {
                        if let Some(to) = options.call.as_deref().and_then(|name| target(&event, name)) {
                            sink.send(Message::Text(json!({ "type": "call", "to": to }).to_string().into())).await?;
                        }
                    }
                    Some("incoming") => {
                        let (call_id, accepts, after) = (event["callId"].clone(), accepts.clone(), options.answer_after);
                        tokio::spawn(async move {
                            tokio::time::sleep(after).await;
                            let _ = accepts.send(call_id);
                        });
                    }
                    Some("connected") => {
                        // 20 ms of the recording every 20 ms, round and round.
                        offset = 0;
                        sending = Some(tokio::time::interval(FRAME));
                    }
                    Some("ended") => {
                        std::fs::write(&options.record, encode_wav(&heard, RATE))
                            .with_context(|| format!("cannot write {}", options.record.display()))?;
                        say(format!("heard {:.1} s → {}", heard.len() as f64 / f64::from(RATE), options.record.display()));
                        let _ = sink.close().await;
                        return Ok(Some(heard.len()));
                    }
                    _ => {}
                }
            }
            Some(call_id) = accepted.recv() => {
                sink.send(Message::Text(json!({ "type": "accept", "callId": call_id }).to_string().into())).await?;
            }
            _ = tick(&mut sending) => {
                let end = (offset + FRAME_SAMPLES).min(microphone.len());
                let frame = &microphone[offset..end];
                offset = if offset + FRAME_SAMPLES >= microphone.len() { 0 } else { offset + FRAME_SAMPLES };
                sink.send(Message::Binary(pcm_to_bytes(frame).into())).await?;
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::speech::audio::pcm::resample;
    use crate::speech::test_audio::voice;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| arg.to_string()).collect()
    }

    #[test]
    fn takes_the_ts_options_with_their_defaults_and_the_gateways_port_for_the_url() {
        let options = Options::parse(&[], || Ok(default_url(3000))).unwrap();
        assert_eq!(
            options,
            Options {
                url: "ws://127.0.0.1:3000/calls/ws".into(),
                device: "default".into(),
                play: None,
                record: "heard.wav".into(),
                answer_after: Duration::from_millis(1500),
                call: None,
            }
        );
        let given = args(&[
            "--url",
            "ws://127.0.0.1:8080/calls/ws",
            "--device",
            "kitchen",
            "--play",
            "voice.wav",
            "--record",
            "out.wav",
            "--answer-after",
            "600000",
            "--call",
            "Мама",
        ]);
        let options = Options::parse(&given, || panic!("--url is given")).unwrap();
        assert_eq!(options.url, "ws://127.0.0.1:8080/calls/ws");
        assert_eq!((options.device.as_str(), options.call.as_deref()), ("kitchen", Some("Мама")));
        assert_eq!((options.play, options.record), (Some("voice.wav".into()), PathBuf::from("out.wav")));
        assert_eq!(options.answer_after, Duration::from_secs(600));
        let refused = Options::parse(&args(&["--answer-after", "soon"]), || Ok(default_url(3000))).unwrap_err();
        assert!(refused.is::<Usage>(), "{refused}");
    }

    #[test]
    fn plays_a_wav_at_16_khz_raw_pcm_or_a_tone() {
        let dir = tempfile::tempdir().unwrap();
        let speech = voice(500.0, -20.0);
        let wav = dir.path().join("voice.wav");
        std::fs::write(&wav, encode_wav(&speech, 16000)).unwrap();
        assert_eq!(load(Some(&wav)).unwrap(), speech);
        // 48 kHz goes down to 16 kHz.
        let high = dir.path().join("high.wav");
        std::fs::write(&high, encode_wav(&resample(&speech, 16000, 48000), 48000)).unwrap();
        assert_eq!(load(Some(&high)).unwrap().len(), speech.len());
        let raw = dir.path().join("voice.pcm");
        std::fs::write(&raw, pcm_to_bytes(&speech)).unwrap();
        assert_eq!(load(Some(&raw)).unwrap(), speech);
        let tone = load(None).unwrap();
        assert_eq!(tone.len(), 80_000);
        assert_eq!((tone[0], tone.iter().copied().max()), (0, Some(6000)));
        std::fs::write(dir.path().join("empty.pcm"), b"").unwrap();
        assert!(load(Some(&dir.path().join("empty.pcm"))).is_err());
        assert!(load(Some(&dir.path().join("missing.wav"))).is_err());
    }

    #[test]
    fn calls_the_first_party_with_the_name_in_it_but_never_itself() {
        let welcome = json!({
            "type": "welcome", "id": "station:default", "name": NAME,
            "endpoints": [
                { "id": "station:default", "name": NAME, "kind": "station" },
                { "id": "browser:1", "name": "Мама", "kind": "browser" },
                { "id": "station:kitchen", "name": "Колонка на кухне", "kind": "station" },
            ]
        });
        assert_eq!(target(&welcome, "мам").as_deref(), Some("browser:1"));
        assert_eq!(target(&welcome, "КОЛОНКА").as_deref(), Some("station:kitchen"));
        assert_eq!(target(&welcome, "Папа"), None);
    }
}
