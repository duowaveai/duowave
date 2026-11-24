//! Per-session journal on disk: events.jsonl with every protocol event and the raw station audio of each voice
//! request. Files are opened lazily, so a session without traffic leaves no folder.

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, MutexGuard};

use regex::Regex;
use serde::Serialize;
use serde_json::Value;

use super::redact::to_log_value;
use crate::config::RecorderConfig;
use crate::utils::time::now_iso;

/// in/out: with the station; toYandex/fromYandex: our speech session with Yandex; note: our own remarks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum EventDirection {
    #[serde(rename = "in")]
    In,
    #[serde(rename = "out")]
    Out,
    #[serde(rename = "note")]
    Note,
    #[serde(rename = "toYandex")]
    ToYandex,
    #[serde(rename = "fromYandex")]
    FromYandex,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionEventRecord {
    pub ts: String,
    pub dir: EventDirection,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// Where one stream's audio goes: a file, or nowhere when the recorder or its audio is off.
pub struct AudioSink {
    pub file: Option<PathBuf>,
    stream: Option<File>,
}

impl AudioSink {
    /// The sink that keeps nothing.
    pub fn null() -> AudioSink {
        AudioSink { file: None, stream: None }
    }

    pub fn write(&mut self, chunk: &[u8]) {
        // A journal is a convenience: losing a chunk of it loses nothing else.
        if let Some(stream) = self.stream.as_mut() {
            let _ = stream.write_all(chunk);
        }
    }

    pub fn close(&mut self) {
        if let Some(stream) = self.stream.take() {
            let _ = stream.sync_data();
        }
    }
}

struct Stamp {
    day: String,
    time: String,
}

/// The day and HHMMSS (UTC) of an ISO time.
fn stamp(iso: &str) -> Stamp {
    Stamp { day: iso.get(0..10).unwrap_or("").to_string(), time: iso.get(11..19).unwrap_or("").replace(':', "") }
}

static NOT_DEVICE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^\p{L}\p{N}_:-]").expect("a valid pattern"));
static NOT_FILE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^\p{L}\p{N}._-]").expect("a valid pattern"));

/// Letters, digits and `_:-` of a device tag, `:` made `-`: its part of a folder name.
fn safe_device(device_tag: &str) -> String {
    let safe = NOT_DEVICE.replace_all(device_tag, "").replace(':', "-");
    if safe.is_empty() {
        "unknown".into()
    } else {
        safe
    }
}

/// A file name of letters, digits and `._-`, the rest `_`.
fn safe_name(name: &str) -> String {
    NOT_FILE.replace_all(name, "_").into_owned()
}

/// The session's journal. Writes are plain file writes, one per event: the lines are on disk when the debug API
/// or a test reads them.
pub struct SessionRecorder {
    config: RecorderConfig,
    pub dir: Option<PathBuf>,
    events: Option<File>,
    closed: bool,
}

impl SessionRecorder {
    pub fn new(config: &RecorderConfig, session_id: &str, device_tag: &str, started_at: &str) -> SessionRecorder {
        let dir = config.enabled.then(|| {
            let Stamp { day, time } = stamp(started_at);
            let short: String = session_id.chars().take(8).collect();
            let base = std::path::absolute(&config.dir).unwrap_or_else(|_| config.dir.clone());
            base.join(day).join(format!("{time}_{}_{short}", safe_device(device_tag)))
        });
        SessionRecorder { config: config.clone(), dir, events: None, closed: false }
    }

    /// The event as journaled: the time added, the data made safe (secrets and identifiers hashed, bytes shown).
    pub fn record(
        &mut self,
        dir: EventDirection,
        kind: &str,
        summary: Option<String>,
        data: Option<&Value>,
    ) -> SessionEventRecord {
        let record = SessionEventRecord {
            ts: now_iso(),
            dir,
            kind: kind.to_string(),
            summary: summary.filter(|summary| !summary.is_empty()),
            data: data.map(|data| to_log_value(data, self.config.redact)),
        };
        if let Some(folder) = self.dir.clone() {
            if let Ok(mut line) = serde_json::to_string(&record) {
                line.push('\n');
                let file = folder.join("events.jsonl");
                if self.closed {
                    // An answer that ends after the station left still reports, without reopening the stream.
                    if let Ok(mut late) = OpenOptions::new().create(true).append(true).open(&file) {
                        let _ = late.write_all(line.as_bytes());
                    }
                    return record;
                }
                if self.events.is_none() {
                    let _ = std::fs::create_dir_all(&folder);
                    self.events = OpenOptions::new().create(true).append(true).open(&file).ok();
                }
                if let Some(events) = self.events.as_mut() {
                    let _ = events.write_all(line.as_bytes());
                }
            }
        }
        record
    }

    pub fn open_audio(&self, name: &str) -> AudioSink {
        let Some(file) = self.audio_path(name) else { return AudioSink::null() };
        let stream = File::create(&file).ok();
        AudioSink { file: Some(file), stream }
    }

    /// Writes a finished audio file (the WAV given to recognition, spotter logs) in the background; its path.
    pub fn save_audio(&self, name: &str, data: Vec<u8>) -> Option<PathBuf> {
        let file = self.audio_path(name)?;
        let target = file.clone();
        let write = move || {
            let _ = std::fs::write(&target, data);
        };
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => drop(runtime.spawn_blocking(write)),
            Err(_) => write(),
        }
        Some(file)
    }

    fn audio_path(&self, name: &str) -> Option<PathBuf> {
        let dir = self.dir.as_ref().filter(|_| self.config.audio)?;
        let audio_dir = dir.join("audio");
        let _ = std::fs::create_dir_all(&audio_dir);
        Some(audio_dir.join(safe_name(name)))
    }

    pub fn close(&mut self) {
        self.closed = true;
        if let Some(events) = self.events.take() {
            let _ = events.sync_data();
        }
    }

    /// The journal's folder as the debug API shows it.
    pub fn dir_display(&self) -> Option<String> {
        self.dir.as_deref().map(Path::to_string_lossy).map(|dir| dir.into_owned())
    }
}

/// The journal as a session keeps it: the recorder on disk, the latest events for the debug API, and their count.
///
/// Its lock is a leaf of every lock order in the gateway. Each method takes it and lets it go before it returns, the
/// guard never leaves this type, and nothing done under it takes another lock or calls out: the recorder writes its
/// own files (a finished audio file goes to a blocking thread) and makes the data safe with plain functions. So code
/// that holds any other lock (the session's state, our Yandex session's, an answer's) may journal, and no thread ever
/// waits for another lock while it holds the journal's: the journal cannot close a lock cycle.
pub struct SessionJournal(Mutex<Journal>);

struct Journal {
    recorder: SessionRecorder,
    recent: VecDeque<SessionEventRecord>,
    events: usize,
    limit: usize,
}

impl SessionJournal {
    pub fn new(recorder: SessionRecorder, limit: usize) -> SessionJournal {
        SessionJournal(Mutex::new(Journal { recorder, recent: VecDeque::new(), events: 0, limit }))
    }

    fn lock(&self) -> MutexGuard<'_, Journal> {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// One event: on disk, among the latest, counted.
    pub fn record(
        &self,
        dir: EventDirection,
        kind: &str,
        summary: Option<String>,
        data: Option<&Value>,
    ) -> SessionEventRecord {
        let mut journal = self.lock();
        let record = journal.recorder.record(dir, kind, summary, data);
        journal.events += 1;
        journal.recent.push_back(record.clone());
        while journal.recent.len() > journal.limit {
            journal.recent.pop_front();
        }
        record
    }

    pub fn events(&self) -> usize {
        self.lock().events
    }

    /// The latest `limit` events (at least one), oldest first.
    pub fn recent(&self, limit: usize) -> Vec<SessionEventRecord> {
        let journal = self.lock();
        let limit = limit.max(1);
        journal.recent.iter().skip(journal.recent.len().saturating_sub(limit)).cloned().collect()
    }

    /// The journal's folder as the debug API shows it.
    pub fn dir_display(&self) -> Option<String> {
        self.lock().recorder.dir_display()
    }

    pub fn open_audio(&self, name: &str) -> AudioSink {
        self.lock().recorder.open_audio(name)
    }

    /// Writes a finished audio file in the background; its path.
    pub fn save_audio(&self, name: &str, data: Vec<u8>) -> Option<PathBuf> {
        self.lock().recorder.save_audio(name, data)
    }

    pub fn close(&self) {
        self.lock().recorder.close();
    }
}

/// HHMMSS (UTC) now, for file names.
pub fn clock_tag() -> String {
    now_iso().get(11..19).unwrap_or("").replace(':', "")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config(dir: &Path, audio: bool) -> RecorderConfig {
        RecorderConfig { enabled: true, dir: dir.to_path_buf(), audio, redact: true }
    }

    #[test]
    fn opens_its_folder_lazily_and_journals_safe_data() {
        let root = tempfile::tempdir().unwrap();
        let mut recorder = SessionRecorder::new(
            &config(root.path(), true),
            "0123456789abcdef",
            "dev:ice/1",
            "2026-11-06T02:11:00.331Z",
        );
        let dir = recorder.dir.clone().unwrap();
        assert!(dir.ends_with("2026-11-06/021100_dev-ice1_01234567"), "{}", dir.display());
        assert!(!dir.exists(), "nothing written yet");
        let record = recorder.record(
            EventDirection::In,
            "synchronizeState",
            Some(String::new()),
            Some(&json!({"oauthToken": "y0_secret", "uuid": "u-1"})),
        );
        assert_eq!(record.summary, None);
        assert_eq!(record.data.as_ref().unwrap()["uuid"], crate::gateway::observability::redact::hash_id("u-1"));
        let mut sink = recorder.open_audio("021100_req 1.in.ogg");
        sink.write(b"OggS");
        sink.close();
        assert!(sink.file.as_ref().unwrap().ends_with("audio/021100_req_1.in.ogg"));
        recorder.close();
        recorder.record(EventDirection::Note, "late", Some("after the end".into()), None);
        let journal = std::fs::read_to_string(dir.join("events.jsonl")).unwrap();
        assert_eq!(journal.lines().count(), 2);
        assert!(!journal.contains("y0_secret"));
        assert_eq!(std::fs::read(sink.file.unwrap()).unwrap(), b"OggS");
    }

    #[test]
    fn keeps_no_audio_when_audio_is_off() {
        let root = tempfile::tempdir().unwrap();
        let recorder = SessionRecorder::new(&config(root.path(), false), "s", "", "2026-11-06T02:11:00.331Z");
        assert!(recorder.open_audio("x.ogg").file.is_none());
        assert!(recorder.save_audio("x.wav", vec![1]).is_none());
        assert!(recorder.dir.as_ref().unwrap().to_string_lossy().contains("_unknown_"));
    }
}
