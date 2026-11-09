//! The owner's takes and voices on disk (data/voice-lab, outside git — people's voices never go there). Disk is the
//! truth: a voice is its folder, renamed in its meta.json, deleted with it. The takes being recorded wait in draft/.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::clone_engines::EngineId;
use super::js;
use crate::models::model_files::safe_name;
use crate::speech::audio::pcm::{decode_wav, encode_wav, resample};
use crate::utils::files::owner_only;
use crate::utils::ids::random_hex;
use crate::utils::time::{now_iso, now_ms};

pub const RATE: u32 = 24_000;
pub const MAX_TAKES: usize = 12;
pub const MAX_TAKE_SECONDS: f64 = 120.0;
pub const MAX_TAKE_BYTES: usize = 16 * 1_048_576;
/// Shorter than this, a take is kept but called short: the reference wants whole phrases to choose from.
pub const SHORT_TAKE_SECONDS: f64 = 15.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TakeVerdict {
    Good,
    Short,
    Quiet,
    Noisy,
    Clipped,
}

/// A take's levels.
#[derive(Debug, Clone, PartialEq)]
pub struct TakeLevels {
    pub seconds: f64,
    /// Speech level, dBFS.
    pub level_db: f64,
    pub noise_db: f64,
    pub snr_db: f64,
    pub peak_db: f64,
    /// Share of samples at full scale.
    pub clipping: f64,
    pub verdict: TakeVerdict,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TakeInfo {
    pub n: u32,
    #[serde(serialize_with = "js::number")]
    pub seconds: f64,
    #[serde(serialize_with = "js::number")]
    pub level_db: f64,
    #[serde(serialize_with = "js::number")]
    pub noise_db: f64,
    #[serde(serialize_with = "js::number")]
    pub snr_db: f64,
    #[serde(serialize_with = "js::number")]
    pub peak_db: f64,
    #[serde(serialize_with = "js::number")]
    pub clipping: f64,
    pub verdict: TakeVerdict,
    /// Which reading passage it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passage: Option<u32>,
    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sample {
    #[serde(serialize_with = "js::number")]
    pub n: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "js::opt_number")]
    pub seconds: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnVoice {
    pub id: String,
    pub name: String,
    pub engine: EngineId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "js::opt_number")]
    pub ref_seconds: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "js::opt_number")]
    pub snr_db: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "js::opt_number")]
    pub rtf: Option<f64>,
    pub samples: Vec<Sample>,
    pub takes: usize,
    /// `package`: a character's voice its package brought (src/characters), not one of the owner's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub character: Option<String>,
    /// Its reference's words (the imprint's own record): a reimprint under another engine needs no recognition.
    #[serde(skip)]
    pub words: Option<String>,
    /// Its reference recording is beside it (ref.wav).
    #[serde(skip)]
    pub has_ref: bool,
}

fn db(value: f64) -> f64 {
    20.0 * value.max(1e-9).log10()
}

fn percentile(sorted: &[f64], share: f64) -> f64 {
    if sorted.is_empty() {
        return -90.0;
    }
    let index = (share * (sorted.len() - 1) as f64).floor().max(0.0) as usize;
    sorted[index.min(sorted.len() - 1)]
}

/// Halving (48 kHz → 24 kHz) averages pairs first, so nothing above the new Nyquist folds back; other ratios are
/// linear.
pub fn to_rate(pcm: Vec<i16>, from: u32) -> Vec<i16> {
    if from == RATE {
        return pcm;
    }
    if from == RATE * 2 {
        return pcm
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| js::round((pair[0] as f64 + pair[1] as f64) / 2.0) as i16)
            .collect();
    }
    resample(&pcm, from, RATE)
}

/// A take's levels: 20 ms frames, the noise floor (10th percentile), speech (frames 10 dB over it), peaks.
pub fn analyse_take(pcm: &[i16], rate: u32) -> TakeLevels {
    let size = js::round(rate as f64 * 0.02).max(1.0) as usize;
    let mut frames = Vec::with_capacity(pcm.len() / size);
    let mut peak = 0u32;
    let mut clipped = 0usize;
    for frame in pcm.chunks_exact(size) {
        let mut energy = 0.0;
        for &sample in frame {
            let value = sample as f64 / 32768.0;
            energy += value * value;
            let magnitude = (sample as i32).unsigned_abs();
            peak = peak.max(magnitude);
            if magnitude >= 32_700 {
                clipped += 1;
            }
        }
        frames.push(db((energy / size as f64).sqrt()));
    }
    let mut sorted = frames;
    sorted.sort_by(f64::total_cmp);
    let noise_db = percentile(&sorted, 0.1);
    let speech: Vec<f64> = sorted.iter().copied().filter(|value| *value > noise_db + 10.0).collect();
    let level_db = percentile(if speech.is_empty() { &sorted } else { &speech }, 0.75);
    let seconds = pcm.len() as f64 / rate as f64;
    let clipping = if pcm.is_empty() { 0.0 } else { clipped as f64 / pcm.len() as f64 };
    let snr_db = level_db - noise_db;
    let verdict = if seconds < SHORT_TAKE_SECONDS {
        TakeVerdict::Short
    } else if clipping > 0.001 {
        TakeVerdict::Clipped
    } else if level_db < -38.0 {
        TakeVerdict::Quiet
    } else if snr_db < 20.0 {
        TakeVerdict::Noisy
    } else {
        TakeVerdict::Good
    };
    let tenth = |value: f64| js::round(value * 10.0) / 10.0;
    TakeLevels {
        seconds: tenth(seconds),
        level_db: tenth(level_db),
        noise_db: tenth(noise_db),
        snr_db: tenth(snr_db),
        peak_db: tenth(db(peak as f64 / 32768.0)),
        clipping: js::round(clipping * 100_000.0) / 100_000.0,
        verdict,
    }
}

fn take_name(n: u32) -> String {
    format!("{n:02}")
}

/// Writes a file only its owner reads: made so (TS `mode: 0o600`), never readable by others for a moment; one that
/// was there already is made so too.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)?.write_all(bytes)?;
    owner_only(path);
    Ok(())
}

/// A file gone already is no error (TS `rmSync(path, {force: true})`); any other failure is.
fn remove_if_there(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

pub struct VoiceStore {
    pub root: PathBuf,
    pub voices_dir: PathBuf,
    pub draft_dir: PathBuf,
    pub run_dir: PathBuf,
    /// One change at a time: the Node gateway's store ran on its one event loop, so two uploads never took the same
    /// number and two renames never wrote the same temporary file.
    turn: Mutex<()>,
}

impl VoiceStore {
    pub fn new(root: &Path) -> VoiceStore {
        let root = js::resolve(root);
        VoiceStore {
            voices_dir: root.join("voices"),
            draft_dir: root.join("draft").join("takes"),
            run_dir: root.join("run"),
            root,
            turn: Mutex::new(()),
        }
    }

    fn turn(&self) -> MutexGuard<'_, ()> {
        self.turn.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The folders, the root owner-only — before anything is written under it, so a root deleted while the gateway
    /// runs comes back as private as it was made at the start, and the voices the imprint writes into it with the
    /// default modes stay the owner's.
    pub fn folders(&self) -> std::io::Result<()> {
        for dir in [&self.root, &self.voices_dir, &self.draft_dir, &self.run_dir] {
            std::fs::create_dir_all(dir)?;
        }
        // A filesystem without modes (Windows) keeps its own.
        owner_only(&self.root);
        Ok(())
    }

    /// Folders, owner-only; half-written voices of a run that died are removed.
    pub fn prepare(&self) -> std::io::Result<Vec<String>> {
        self.folders()?;
        let mut removed = Vec::new();
        for entry in std::fs::read_dir(&self.voices_dir)?.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with(".partial-") {
                continue;
            }
            let _ = std::fs::remove_dir_all(entry.path());
            removed.push(name);
        }
        removed.sort();
        Ok(removed)
    }

    pub fn takes(&self) -> Vec<TakeInfo> {
        let Ok(entries) = std::fs::read_dir(&self.draft_dir) else { return Vec::new() };
        let mut takes: Vec<TakeInfo> = entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                let stem = name.strip_suffix(".json")?;
                if stem.is_empty() || !stem.bytes().all(|b| b.is_ascii_digit()) {
                    return None;
                }
                let text = std::fs::read_to_string(entry.path()).ok()?;
                serde_json::from_str::<TakeInfo>(&text).ok()
            })
            .filter(|take| self.take_file(take.n).exists())
            .collect();
        takes.sort_by_key(|take| take.n);
        takes
    }

    pub fn take_file(&self, n: u32) -> PathBuf {
        self.draft_dir.join(format!("{}.wav", take_name(n)))
    }

    /// A take from the panel: 16-bit PCM WAV, any rate, mono or stereo — kept as 24 kHz mono with its levels.
    pub fn add_take(&self, wav: &[u8], passage: Option<i64>) -> Result<TakeInfo, String> {
        let enough = || format!("Хватит дублей: не больше {MAX_TAKES}. Удалите лишние.");
        if self.takes().len() >= MAX_TAKES {
            return Err(enough());
        }
        let decoded = decode_wav(wav).map_err(|error| format!("Запись не принята: нужен WAV 16 бит ({error})."))?;
        if decoded.sample_rate < 16_000 {
            return Err("Запись не принята: частота ниже 16 кГц.".into());
        }
        let pcm = to_rate(decoded.pcm, decoded.sample_rate);
        let seconds = pcm.len() as f64 / RATE as f64;
        if seconds < 3.0 {
            return Err("Запись слишком короткая — нужно хотя бы несколько фраз.".into());
        }
        if seconds > MAX_TAKE_SECONDS {
            return Err(format!("Запись длиннее {MAX_TAKE_SECONDS} секунд — разбейте её на дубли."));
        }
        let levels = analyse_take(&pcm, RATE);
        let encoded = encode_wav(&pcm, RATE);
        // The number is taken and its files written in one turn: an upload finishing beside this one takes the next.
        let _turn = self.turn();
        let existing = self.takes();
        if existing.len() >= MAX_TAKES {
            return Err(enough());
        }
        let n = existing.last().map(|take| take.n).unwrap_or(0) + 1;
        let info = TakeInfo {
            n,
            seconds: levels.seconds,
            level_db: levels.level_db,
            noise_db: levels.noise_db,
            snr_db: levels.snr_db,
            peak_db: levels.peak_db,
            clipping: levels.clipping,
            verdict: levels.verdict,
            passage: passage
                .filter(|passage| *passage > 0 && *passage <= u32::MAX as i64)
                .map(|passage| passage as u32),
            at: now_iso(),
        };
        let failed = |error: std::io::Error| error.to_string();
        self.folders().map_err(failed)?;
        write_private(&self.take_file(n), &encoded).map_err(failed)?;
        let json = serde_json::to_vec(&info).map_err(|error| error.to_string())?;
        write_private(&self.draft_dir.join(format!("{}.json", take_name(n))), &json).map_err(failed)?;
        Ok(info)
    }

    /// Whether the take was there. A recording that could not be removed is an error, the WAV before its JSON: the
    /// take stays listed instead of its voice staying on disk unseen.
    pub fn remove_take(&self, n: u32) -> std::io::Result<bool> {
        let _turn = self.turn();
        let name = take_name(n);
        let wav = self.draft_dir.join(format!("{name}.wav"));
        let had = wav.exists();
        remove_if_there(&wav)?;
        remove_if_there(&self.draft_dir.join(format!("{name}.json")))?;
        Ok(had)
    }

    pub fn clear_draft(&self) -> std::io::Result<()> {
        let _turn = self.turn();
        match std::fs::remove_dir_all(&self.draft_dir) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        self.folders()
    }

    /// `v-<time in base 36><4 hex>`.
    pub fn new_voice_id(&self) -> String {
        format!("v-{}{}", base36(now_ms()), random_hex(2))
    }

    pub fn voice_dir(&self, id: &str) -> Result<PathBuf, String> {
        if !safe_name(id) || id.starts_with('.') {
            return Err(format!("не голос: {id}"));
        }
        Ok(self.voices_dir.join(id))
    }

    pub fn voices(&self) -> Vec<OwnVoice> {
        let Ok(entries) = std::fs::read_dir(&self.voices_dir) else { return Vec::new() };
        let mut voices: Vec<OwnVoice> = entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                (!name.starts_with('.') && safe_name(&name)).then(|| self.voice(&name)).flatten()
            })
            .collect();
        voices.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        voices
    }

    pub fn voice(&self, id: &str) -> Option<OwnVoice> {
        if !safe_name(id) || id.starts_with('.') {
            return None;
        }
        let dir = self.voices_dir.join(id);
        if !dir.join("voice.pt").exists() {
            return None;
        }
        let meta: Map<String, Value> =
            serde_json::from_str(&std::fs::read_to_string(dir.join("meta.json")).ok()?).ok()?;
        let engine = meta.get("engine").and_then(Value::as_str).and_then(EngineId::parse)?;
        let empty = Map::new();
        let reference = meta.get("reference").and_then(Value::as_object).unwrap_or(&empty);
        let samples = meta
            .get("samples")
            .and_then(Value::as_array)
            .map(|samples| {
                samples
                    .iter()
                    .filter_map(|sample| {
                        Some(Sample {
                            n: sample.get("n")?.as_f64()?,
                            text: sample.get("text").and_then(Value::as_str).map(str::to_string),
                            seconds: sample.get("seconds").and_then(Value::as_f64),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let takes = std::fs::read_dir(dir.join("takes"))
            .map(|entries| {
                entries.flatten().filter(|entry| entry.file_name().to_string_lossy().ends_with(".wav")).count()
            })
            .unwrap_or(0);
        let name = meta.get("name").and_then(Value::as_str).filter(|name| !js::trim(name).is_empty());
        let text = |key: &str| meta.get(key).and_then(Value::as_str).map(str::to_string);
        Some(OwnVoice {
            id: id.to_string(),
            name: name.unwrap_or("Мой голос").to_string(),
            engine,
            device: meta.get("device").and_then(Value::as_str).map(str::to_string),
            created_at: meta
                .get("createdAt")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| "1970-01-01T00:00:00.000Z".into()),
            ref_seconds: reference.get("seconds").and_then(Value::as_f64),
            snr_db: reference.get("snrDb").and_then(Value::as_f64),
            rtf: meta.get("rtf").and_then(Value::as_f64),
            samples,
            takes,
            origin: text("origin"),
            package: text("package"),
            character: text("character"),
            words: reference
                .get("words")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|w| !w.is_empty())
                .map(str::to_string),
            has_ref: dir.join("ref.wav").is_file(),
        })
    }

    /// A voice's own recordings (`takes/NN.wav`), in order.
    pub fn takes_of(&self, id: &str) -> Vec<PathBuf> {
        let Ok(dir) = self.voice_dir(id) else { return Vec::new() };
        let mut takes: Vec<PathBuf> = std::fs::read_dir(dir.join("takes"))
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| path.extension().is_some_and(|extension| extension == "wav"))
                    .collect()
            })
            .unwrap_or_default();
        takes.sort();
        takes
    }

    /// A voice put in place whole: its files copied into `.partial-<id>` with its meta.json, then renamed over what
    /// was there — never a voice half there.
    pub fn place_voice(&self, id: &str, files: &[(&str, &Path)], meta: &Value) -> Result<(), String> {
        let dir = self.voice_dir(id)?;
        let failed = |error: std::io::Error| error.to_string();
        let _turn = self.turn();
        self.folders().map_err(failed)?;
        let partial = self.voices_dir.join(format!(".partial-{id}"));
        let _ = std::fs::remove_dir_all(&partial);
        let placed = (|| {
            std::fs::create_dir_all(partial.join("samples")).map_err(failed)?;
            for (name, from) in files {
                std::fs::copy(from, partial.join(name)).map_err(|error| format!("{}: {error}", from.display()))?;
            }
            let text = serde_json::to_string_pretty(meta).map_err(|error| error.to_string())?;
            write_private(&partial.join("meta.json"), text.as_bytes()).map_err(failed)?;
            match std::fs::remove_dir_all(&dir) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.to_string()),
                _ => {}
            }
            std::fs::rename(&partial, &dir).map_err(failed)
        })();
        if placed.is_err() {
            let _ = std::fs::remove_dir_all(&partial);
        }
        placed
    }

    /// The voices packages brought (meta.json `origin: "package"`), with their meta: whole or not.
    pub fn package_voices(&self) -> Vec<(String, Map<String, Value>)> {
        let Ok(entries) = std::fs::read_dir(&self.voices_dir) else { return Vec::new() };
        let mut found: Vec<(String, Map<String, Value>)> = entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') || !safe_name(&name) {
                    return None;
                }
                let text = std::fs::read_to_string(entry.path().join("meta.json")).ok()?;
                let meta: Map<String, Value> = serde_json::from_str(&text).ok()?;
                (meta.get("origin").and_then(Value::as_str) == Some("package")).then_some((name, meta))
            })
            .collect();
        found.sort_by(|a, b| a.0.cmp(&b.0));
        found
    }

    /// Fields merged into a voice's meta.json (written beside it, then renamed over it).
    pub fn patch_meta(&self, id: &str, fields: Map<String, Value>) -> Result<(), String> {
        let dir = self.voice_dir(id)?;
        let _turn = self.turn();
        let meta_path = dir.join("meta.json");
        let text = std::fs::read_to_string(&meta_path).map_err(|error| error.to_string())?;
        let mut meta: Map<String, Value> = serde_json::from_str(&text).map_err(|error| error.to_string())?;
        meta.extend(fields);
        let pretty = serde_json::to_string_pretty(&meta).map_err(|error| error.to_string())?;
        let temporary = dir.join("meta.json.tmp");
        write_private(&temporary, pretty.as_bytes()).map_err(|error| error.to_string())?;
        std::fs::rename(&temporary, &meta_path).map_err(|error| error.to_string())
    }

    /// A voice's recordings copied to another: a voice imprinted anew under another engine keeps them.
    pub fn copy_takes(&self, from: &str, to: &str) -> Result<(), String> {
        let source = self.voice_dir(from)?.join("takes");
        let target = self.voice_dir(to)?.join("takes");
        let Ok(entries) = std::fs::read_dir(&source) else { return Ok(()) };
        let _turn = self.turn();
        std::fs::create_dir_all(&target).map_err(|error| error.to_string())?;
        for entry in entries.flatten() {
            std::fs::copy(entry.path(), target.join(entry.file_name())).map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    pub fn sample_path(&self, id: &str, n: i64) -> Option<PathBuf> {
        if !(1..=9).contains(&n) {
            return None;
        }
        let path = self.voice_dir(id).ok()?.join("samples").join(format!("{n}.wav"));
        path.exists().then_some(path)
    }

    pub fn rename(&self, id: &str, name: &str) -> Result<OwnVoice, String> {
        let dir = self.voice_dir(id)?;
        let _turn = self.turn();
        let meta_path = dir.join("meta.json");
        let text = std::fs::read_to_string(&meta_path).map_err(|error| error.to_string())?;
        let mut meta: Value = serde_json::from_str(&text).map_err(|error| error.to_string())?;
        let Some(object) = meta.as_object_mut() else { return Err(format!("{}: not an object", meta_path.display())) };
        object.insert("name".into(), Value::String(name.to_string()));
        let pretty = serde_json::to_string_pretty(&meta).map_err(|error| error.to_string())?;
        let temporary = dir.join("meta.json.tmp");
        write_private(&temporary, pretty.as_bytes()).map_err(|error| error.to_string())?;
        std::fs::rename(&temporary, &meta_path).map_err(|error| error.to_string())?;
        self.voice(id).ok_or_else(|| format!("не голос: {id}"))
    }

    pub fn remove(&self, id: &str) -> Result<(), String> {
        let dir = self.voice_dir(id)?;
        let _turn = self.turn();
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }

    /// After an imprint, its takes go with the voice: a dataset for training it some day.
    pub fn adopt_takes(&self, id: &str, takes: &[TakeInfo]) -> Result<(), String> {
        let target = self.voice_dir(id)?.join("takes");
        let _turn = self.turn();
        self.folders().map_err(|error| error.to_string())?;
        std::fs::create_dir_all(&target).map_err(|error| error.to_string())?;
        for take in takes {
            let name = take_name(take.n);
            for ext in ["wav", "json"] {
                let from = self.draft_dir.join(format!("{name}.{ext}"));
                if from.exists() {
                    std::fs::rename(&from, target.join(format!("{name}.{ext}"))).map_err(|error| error.to_string())?;
                }
            }
        }
        Ok(())
    }

    /// Measured peaks, MB, by `<engine>/<kind>/<device>`: the next need is the last peak with a margin.
    pub fn learned(&self, key: &str) -> Option<f64> {
        self.read_learned().get(key).and_then(Value::as_f64)
    }

    pub fn learn(&self, key: &str, peak_mb: f64) {
        if !peak_mb.is_finite() || peak_mb <= 0.0 {
            return;
        }
        let _turn = self.turn();
        let mut all = self.read_learned();
        all.insert(key.to_string(), Value::from(js::round_i64(peak_mb)));
        if let Ok(text) = serde_json::to_string_pretty(&Value::Object(all)) {
            let written = self.folders().and_then(|()| write_private(&self.root.join("engines.json"), text.as_bytes()));
            if let Err(error) = written {
                tracing::warn!("voice lab: the measured peaks were not kept ({error})");
            }
        }
    }

    fn read_learned(&self) -> Map<String, Value> {
        std::fs::read_to_string(self.root.join("engines.json"))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }
}

/// Number.prototype.toString(36).
fn base36(mut value: u64) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if value == 0 {
        return "0".into();
    }
    let mut out = Vec::new();
    while value > 0 {
        out.push(DIGITS[(value % 36) as usize]);
        value /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const TEST_RATE: u32 = 16_000;

    /// A voiced, speech-like tone (fundamental plus harmonics, syllable-rate wobble) at the given RMS level.
    pub(crate) fn voice(ms: u32, rms_dbfs: f64) -> Vec<i16> {
        let count = js::round(ms as f64 * TEST_RATE as f64 / 1000.0) as usize;
        let pitch = 160.0;
        let raw: Vec<f64> = (0..count)
            .map(|i| {
                let t = i as f64 / TEST_RATE as f64;
                let wobble = 0.75 + 0.25 * (2.0 * std::f64::consts::PI * 4.0 * t).sin();
                wobble
                    * ((2.0 * std::f64::consts::PI * pitch * t).sin()
                        + 0.5 * (4.0 * std::f64::consts::PI * pitch * t).sin()
                        + 0.25 * (6.0 * std::f64::consts::PI * pitch * t).sin())
            })
            .collect();
        let rms = (raw.iter().map(|v| v * v).sum::<f64>() / count as f64).sqrt();
        let scale = 10f64.powf(rms_dbfs / 20.0) / rms * 32767.0;
        raw.iter().map(|v| js::round(v * scale) as i16).collect()
    }

    /// Room noise at the given RMS level (deterministic).
    pub(crate) fn silence(ms: u32, rms_dbfs: f64) -> Vec<i16> {
        let count = js::round(ms as f64 * TEST_RATE as f64 / 1000.0) as usize;
        let amplitude = 10f64.powf(rms_dbfs / 20.0) * 32767.0 * 3f64.sqrt();
        let mut seed: u64 = 12345;
        (0..count)
            .map(|_| {
                seed = (seed.wrapping_mul(1_103_515_245).wrapping_add(12345)) & 0x7fff_ffff;
                js::round(((seed as f64 / 0x7fff_ffff as f64) * 2.0 - 1.0) * amplitude) as i16
            })
            .collect()
    }

    /// Phrases with pauses, as a person reads: 16 kHz, like the test voices.
    pub(crate) fn reading(seconds: f64, voice_db: f64, noise_db: f64) -> Vec<u8> {
        let mut pcm = Vec::new();
        let mut t = 0.0;
        while t < seconds {
            pcm.extend(voice(2000, voice_db));
            pcm.extend(silence(500, noise_db));
            t += 2.5;
        }
        encode_wav(&pcm, TEST_RATE)
    }

    fn store() -> (tempfile::TempDir, VoiceStore) {
        let root = tempfile::tempdir().unwrap();
        let store = VoiceStore::new(root.path());
        store.prepare().unwrap();
        (root, store)
    }

    #[test]
    fn keeps_a_take_as_24_khz_mono_with_its_levels_and_calls_a_clean_reading_good() {
        let (_root, store) = store();
        let take = store.add_take(&reading(20.0, -20.0, -60.0), Some(2)).unwrap();
        assert_eq!((take.n, take.passage, take.verdict), (1, Some(2), TakeVerdict::Good));
        assert!((take.seconds - 20.0).abs() < 0.5, "{}", take.seconds);
        assert!(take.snr_db > 30.0, "{}", take.snr_db);
        let saved = decode_wav(&std::fs::read(store.take_file(1)).unwrap()).unwrap();
        assert_eq!(saved.sample_rate, RATE);
        assert!((saved.pcm.len() as f64 / RATE as f64 - take.seconds).abs() < 0.5);
        assert_eq!(store.takes(), vec![take]);
    }

    #[test]
    fn says_what_is_wrong_with_a_take_too_short_too_quiet_too_noisy_clipped() {
        let (_root, store) = store();
        assert_eq!(store.add_take(&reading(10.0, -20.0, -60.0), None).unwrap().verdict, TakeVerdict::Short);
        assert_eq!(store.add_take(&reading(20.0, -46.0, -80.0), None).unwrap().verdict, TakeVerdict::Quiet);
        assert_eq!(store.add_take(&reading(20.0, -20.0, -32.0), None).unwrap().verdict, TakeVerdict::Noisy);
        let square: Vec<i16> = (0..20 * TEST_RATE).map(|i| if (i / 40) % 2 == 1 { 32_767 } else { -32_768 }).collect();
        assert_eq!(store.add_take(&encode_wav(&square, TEST_RATE), None).unwrap().verdict, TakeVerdict::Clipped);
    }

    #[test]
    fn refuses_what_is_not_a_16_bit_wav_and_a_recording_too_long_or_too_short() {
        let (_root, store) = store();
        assert!(store.add_take(b"ID3 not a wav", None).unwrap_err().contains("нужен WAV 16 бит"));
        assert!(store.add_take(&reading(125.0, -20.0, -60.0), None).unwrap_err().contains("длиннее 120 секунд"));
        let short = encode_wav(&voice(1000, -20.0), TEST_RATE);
        assert!(store.add_take(&short, None).unwrap_err().contains("слишком короткая"));
    }

    #[test]
    fn numbers_takes_on_and_forgets_a_deleted_one() {
        let (_root, store) = store();
        store.add_take(&reading(16.0, -20.0, -60.0), None).unwrap();
        store.add_take(&reading(16.0, -20.0, -60.0), None).unwrap();
        assert!(store.remove_take(1).unwrap());
        assert!(!store.remove_take(1).unwrap());
        assert_eq!(store.add_take(&reading(16.0, -20.0, -60.0), None).unwrap().n, 3);
        assert_eq!(store.takes().iter().map(|take| take.n).collect::<Vec<_>>(), vec![2, 3]);
        store.clear_draft().unwrap();
        assert_eq!(store.takes(), vec![]);
    }

    #[test]
    fn numbers_takes_uploaded_at_once_apart_none_overwrites_another() {
        let (_root, store) = store();
        let take = reading(16.0, -20.0, -60.0);
        let together = std::sync::Barrier::new(4);
        let mut numbers: Vec<u32> = std::thread::scope(|scope| {
            let uploads: Vec<_> = (0..4)
                .map(|_| {
                    scope.spawn(|| {
                        together.wait();
                        store.add_take(&take, None).unwrap().n
                    })
                })
                .collect();
            uploads.into_iter().map(|upload| upload.join().unwrap()).collect()
        });
        numbers.sort();
        assert_eq!(numbers, vec![1, 2, 3, 4]);
        assert_eq!(store.takes().iter().map(|take| take.n).collect::<Vec<_>>(), vec![1, 2, 3, 4]);
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[cfg(unix)]
    #[test]
    fn says_when_a_take_could_not_be_removed_and_keeps_it_listed() {
        use std::os::unix::fs::PermissionsExt;
        // SAFETY: geteuid only reads the process's user id.
        if unsafe { libc::geteuid() } == 0 {
            return; // root removes from a read-only folder all the same
        }
        let (_root, store) = store();
        store.add_take(&reading(16.0, -20.0, -60.0), None).unwrap();
        let writable = |mode: u32| std::fs::set_permissions(&store.draft_dir, std::fs::Permissions::from_mode(mode));
        writable(0o500).unwrap();
        let refused = store.remove_take(1);
        writable(0o700).unwrap();
        assert!(refused.is_err(), "{refused:?}");
        assert_eq!(store.takes().iter().map(|take| take.n).collect::<Vec<_>>(), vec![1]);
        assert!(store.take_file(1).exists());
        assert!(store.remove_take(1).unwrap());
        assert_eq!(store.takes(), vec![]);
    }

    #[cfg(unix)]
    #[test]
    fn makes_a_root_deleted_while_running_owner_only_again_before_writing_under_it() {
        let dir = tempfile::tempdir().unwrap();
        let store = VoiceStore::new(&dir.path().join("voice-lab"));
        store.prepare().unwrap();
        assert_eq!(mode(&store.root), 0o700);
        std::fs::remove_dir_all(&store.root).unwrap();
        store.add_take(&reading(16.0, -20.0, -60.0), None).unwrap();
        assert_eq!(mode(&store.root), 0o700, "a take");
        assert_eq!((mode(&store.take_file(1)), mode(&store.draft_dir.join("01.json"))), (0o600, 0o600));
        std::fs::remove_dir_all(&store.root).unwrap();
        store.learn("omnivoice/serve/mps", 3000.0);
        assert_eq!(mode(&store.root), 0o700, "a measured peak");
        assert_eq!(mode(&store.root.join("engines.json")), 0o600);
        std::fs::remove_dir_all(&store.root).unwrap();
        store.clear_draft().unwrap();
        assert_eq!(mode(&store.root), 0o700, "a cleared draft");
    }

    #[test]
    fn halves_48_khz_by_averaging_pairs_nothing_folds_back_other_rates_linearly() {
        let pcm = vec![100, 300, 1000, 2000, -500, -700];
        assert_eq!(to_rate(pcm.clone(), 48_000), vec![200, 1500, -600]);
        assert_eq!(to_rate(pcm.clone(), RATE), pcm);
        assert_eq!(to_rate(vec![0; 16_000], 16_000).len(), 24_000);
    }

    fn make_voice(root: &Path, id: &str, meta: &str) {
        let dir = root.join("voices").join(id);
        std::fs::create_dir_all(dir.join("samples")).unwrap();
        std::fs::write(dir.join("voice.pt"), "imprint").unwrap();
        std::fs::write(dir.join("meta.json"), meta).unwrap();
        std::fs::write(dir.join("samples").join("1.wav"), encode_wav(&voice(500, -20.0), RATE)).unwrap();
    }

    #[test]
    fn are_the_folders_with_an_imprint_and_a_readable_meta_json_half_taken_ones_are_swept_at_start() {
        let (root, store) = store();
        make_voice(
            root.path(),
            "v-one",
            r#"{"name":"Мой голос","engine":"omnivoice","createdAt":"2026-11-06T00:06:00+00:00",
                "reference":{"seconds":6.4,"snrDb":34.2,"words":"не показываем"},
                "samples":[{"n":1,"text":"Привет!","seconds":3.2}],"rtf":1.22}"#,
        );
        make_voice(root.path(), "v-broken", "{not json");
        make_voice(root.path(), "v-alien", r#"{"engine":"chatterbox"}"#);
        make_voice(root.path(), ".partial-v-two", r#"{"engine":"omnivoice"}"#);
        assert_eq!(
            store.voices(),
            vec![OwnVoice {
                id: "v-one".into(),
                name: "Мой голос".into(),
                engine: EngineId::Omnivoice,
                device: None,
                created_at: "2026-11-06T00:06:00+00:00".into(),
                ref_seconds: Some(6.4),
                snr_db: Some(34.2),
                rtf: Some(1.22),
                samples: vec![Sample { n: 1.0, text: Some("Привет!".into()), seconds: Some(3.2) }],
                takes: 0,
                origin: None,
                package: None,
                character: None,
                words: Some("не показываем".into()),
                has_ref: false,
            }]
        );
        assert!(!serde_json::to_string(&store.voices()).unwrap().contains("не показываем"), "the words stay here");
        assert_eq!(store.prepare().unwrap(), vec![".partial-v-two".to_string()]);
        assert!(!root.path().join("voices").join(".partial-v-two").exists());
    }

    #[test]
    fn rename_only_the_name_give_their_samples_never_a_path_outside_and_go_away_whole() {
        let (root, store) = store();
        make_voice(root.path(), "v-one", r#"{"name":"Мой голос","engine":"cosyvoice3","reference":{"words":"слова"}}"#);
        assert_eq!(store.rename("v-one", "Второй голос").unwrap().name, "Второй голос");
        let meta: Value =
            serde_json::from_str(&std::fs::read_to_string(root.path().join("voices/v-one/meta.json")).unwrap())
                .unwrap();
        assert_eq!(meta["reference"]["words"], "слова");
        assert_eq!(store.sample_path("v-one", 1), Some(store.voices_dir.join("v-one").join("samples").join("1.wav")));
        assert_eq!(store.sample_path("v-one", 2), None);
        assert!(store.voice_dir("../state").is_err());
        assert_eq!(store.voice(".."), None);
        store.remove("v-one").unwrap();
        assert_eq!(store.voices(), vec![]);
    }

    #[test]
    fn take_the_drafts_takes_with_them_a_dataset_for_later() {
        let (root, store) = store();
        make_voice(root.path(), "v-one", r#"{"engine":"omnivoice"}"#);
        let takes = vec![
            store.add_take(&reading(16.0, -20.0, -60.0), None).unwrap(),
            store.add_take(&reading(16.0, -20.0, -60.0), None).unwrap(),
        ];
        store.adopt_takes("v-one", &takes).unwrap();
        assert_eq!(store.takes(), vec![]);
        assert_eq!(store.voice("v-one").map(|voice| voice.takes), Some(2));
    }

    #[test]
    fn remembers_measured_peaks_by_engine_kind_and_device() {
        let (root, store) = store();
        assert_eq!(store.learned("omnivoice/imprint/cpu"), None);
        store.learn("omnivoice/imprint/cpu", 2955.4);
        store.learn("omnivoice/serve/mps", 0.0);
        assert_eq!(VoiceStore::new(root.path()).learned("omnivoice/imprint/cpu"), Some(2955.0));
        assert_eq!(store.learned("omnivoice/serve/mps"), None);
    }

    #[test]
    fn names_voices_by_time_and_chance() {
        let (_root, store) = store();
        let id = store.new_voice_id();
        assert!(id.starts_with("v-") && safe_name(&id), "{id}");
        assert_eq!(base36(35), "z");
        assert_eq!(base36(36), "10");
    }
}
