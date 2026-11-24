//! `duowave asr-check [--model v3_e2e_rnnt] [--compare] <file.wav>…` — phrases through this computer's own
//! recognition (src/speech/asr/local), the model fetched first if it is not here. With `--compare` the same audio
//! goes to the recognition server at ASR_BASE_URL too (asr-server, the PyTorch GigaAM): both texts and times side by
//! side. The files are 16-bit PCM WAV (16 kHz best; other rates are resampled).
//!
//! The models go to ASR_LOCAL_DIR, else DATA_DIR/speech, else ./data/speech.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Context;
use tokio_util::sync::CancellationToken;

use super::{flag, option, Usage};
use crate::speech::asr::local::catalog;
use crate::speech::asr::local::supervisor::{LocalAsr, LocalAsrOptions, Phase};
use crate::speech::asr::openai_transcriber::OpenAiTranscriber;
use crate::speech::audio::pcm::decode_wav;

const USAGE: &str = "usage: duowave asr-check [--model v3_e2e_rnnt] [--compare] [--cpu] <file.wav>…";

pub fn main(args: &[String]) -> anyhow::Result<()> {
    if !crate::speech::asr::local::AVAILABLE {
        return Err(Usage("duowave asr-check: this build has no local recognition (feature local-asr)".into()).into());
    }
    let model = option(args, "model").unwrap_or(catalog::DEFAULT_MODEL).to_string();
    let values: Vec<&str> = ["--model"].iter().filter_map(|name| option(args, &name[2..])).collect();
    let files: Vec<&String> =
        args.iter().filter(|arg| !arg.starts_with("--") && !values.contains(&arg.as_str())).collect();
    if files.is_empty() {
        return Err(Usage(USAGE.into()).into());
    }
    let config = super::config()?;
    let dir = std::env::var_os("ASR_LOCAL_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("DATA_DIR").map(|data| PathBuf::from(data).join("speech")))
        .unwrap_or_else(|| PathBuf::from("data/speech"));
    let options = LocalAsrOptions::new(
        super::absolute(&dir),
        std::env::var("ASR_MODELS_MIRROR").ok(),
        config.speech.asr.language.clone(),
        !flag(args, "cpu"),
    );
    let server = flag(args, "compare").then(|| OpenAiTranscriber::new(config.speech.asr.clone()));
    // The worker is this program: a multi-thread runtime keeps the download's progress and the worker's pipes going.
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    runtime.block_on(async move {
        let asr = LocalAsr::new(options)?;
        asr.choose(&model);
        asr.start();
        let started = Instant::now();
        let mut said = String::new();
        loop {
            let status = asr.status();
            let line = match status.phase {
                Phase::Ready => break,
                Phase::Failed => anyhow::bail!(status.error.unwrap_or_else(|| "не загрузилась".into())),
                Phase::Downloading => {
                    format!("скачиваю {model}: {} из {} МБ", status.bytes / 1_000_000, status.total / 1_000_000)
                }
                _ => format!("загружаю {model}…"),
            };
            if line != said {
                eprintln!("{line}");
                said = line;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let status = asr.status();
        eprintln!(
            "{model} готова за {:.1} с на {}{}",
            started.elapsed().as_secs_f64(),
            status.backend.unwrap_or_default(),
            status.device.map(|device| format!(" ({device})")).unwrap_or_default()
        );
        for file in files {
            let bytes = std::fs::read(file).with_context(|| format!("cannot read {file}"))?;
            let wav = decode_wav(&bytes).with_context(|| format!("{file}: not a 16-bit WAV"))?;
            let seconds = wav.pcm.len() as f64 / f64::from(wav.sample_rate.max(1));
            let cancel = CancellationToken::new();
            let at = Instant::now();
            let local = asr.recognize(&wav.pcm, wav.sample_rate, &cancel).await;
            let local_ms = at.elapsed().as_millis();
            println!("{file} ({seconds:.1} с)");
            match local {
                Ok(text) => println!("  здесь  {local_ms:>5} мс  {text}"),
                Err(error) => println!("  здесь  {local_ms:>5} мс  ошибка: {error:#}"),
            }
            if let Some(server) = &server {
                let at = Instant::now();
                let heard = server.recognize(&wav.pcm, wav.sample_rate, &cancel).await;
                let ms = at.elapsed().as_millis();
                match heard {
                    Ok(text) => println!("  сервер {ms:>5} мс  {text}"),
                    Err(error) => println!("  сервер {ms:>5} мс  ошибка: {error:#}"),
                }
            }
        }
        Ok(())
    })
}
