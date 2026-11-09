//! `duowave journal`: the live journal, one line per event, to watch the station while it is used
//! (src/tools/follow-journal.ts, `pnpm journal`).
//!
//!   duowave journal [--all]
//!
//! Follows the sessions under RECORDER_DIR (a reconnect starts a new one) and prints what matters: what the station
//! asked and what we heard, the route, the answers and their timings, Yandex's part, errors. --all prints every
//! event, the station's side connections included (one per wake word, to upload its audio).

use std::collections::{HashMap, HashSet};
use std::io::{IsTerminal as _, Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, SystemTime};

use regex::Regex;
use serde::Deserialize;

use super::flag;

/// How often the journals are looked at.
const POLL: Duration = Duration::from_millis(300);
/// Events of the latest session printed at the start, for context.
const CONTEXT: usize = 15;

/// One line of a session's events.jsonl (gateway::observability::session_recorder), the fields read here.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct JournalEvent {
    pub ts: String,
    pub dir: String,
    pub kind: String,
    pub summary: Option<String>,
}

/// What every request carries and nobody reads: acks, stats, stream bookkeeping, audio.
const ROUTINE: &[&str] = &[
    "ack",
    "audio",
    "inputStartAck",
    "interimAsr",
    "logAck",
    "matchedUser",
    "requestStat",
    "requestStatAck",
    "spotterValidation",
    "streamControl",
    "streamingResponse",
    "ttsGenerate",
    "ttsSpeak",
    "ttsTimings",
    "unknown",
];
static TROUBLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)error|unavailable|rejected|closed|disconnected|silent|dropped|ignored|orphan|stray")
        .expect("a valid pattern")
});
static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").expect("a valid pattern"));
/// How every connection starts; what comes next tells the station's session from a side connection.
const HANDSHAKE: &[&str] = &["connected", "synchronizeState", "synchronizeStateResponse", "yandexConnected"];
/// A side connection: the station opens it for each wake word, uploads the wake word's audio and leaves.
const SIDE: &str = "logSpotter";
static DAY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d{4}-\d{2}-\d{2}$").expect("a valid pattern"));

fn arrow(dir: &str) -> &str {
    match dir {
        "in" => "←",
        "out" => "→",
        "toYandex" => "⇢Y",
        "fromYandex" => "⇠Y",
        "note" => "•",
        other => other,
    }
}

pub fn main(args: &[String]) -> anyhow::Result<()> {
    let every = flag(args, "all");
    let config = super::config()?;
    let root = super::absolute(&config.gateway.recorder.dir);
    let mut follower = Follower::new(root, every, std::io::stdout().is_terminal());
    for line in follower.start() {
        println!("{line}");
    }
    loop {
        std::thread::sleep(POLL);
        for line in follower.poll() {
            println!("{line}");
        }
    }
}

/// Whether the event is worth a line: everything with --all; otherwise not the routine, not what Yandex sends (it
/// shows as what we pass on, →, and in our notes: yandexAnswer, yandexLookup, pushDropped) and not recognition as it
/// goes (the final text is enough).
pub fn shown(every: bool, event: &JournalEvent) -> bool {
    if every {
        return true;
    }
    if ROUTINE.contains(&event.kind.as_str()) || event.dir == "fromYandex" {
        return false;
    }
    !(event.kind == "asrResult"
        && event.summary.as_deref().is_some_and(|summary| summary.starts_with("endOfUtt=false")))
}

fn paint(colors: bool, code: u8, text: &str) -> String {
    if colors {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

/// The time of day (this computer's zone) of an ISO time; the text itself when it is not one.
fn local_time(ts: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(ts)
        .map(|at| at.with_timezone(&chrono::Local).format("%H:%M:%S").to_string())
        .unwrap_or_else(|_| ts.to_string())
}

/// The event's line: time, arrow, kind and summary; trouble in red, what was heard in bold, answers and connections
/// in green, our notes in cyan.
pub fn format(event: &JournalEvent, colors: bool) -> String {
    let summary = SPACES.replace_all(event.summary.as_deref().unwrap_or(""), " ");
    let line = format!("{} {:<2} {} {summary}", local_time(&event.ts), arrow(&event.dir), event.kind);
    let line = line.trim_end();
    let kind = event.kind.as_str();
    if TROUBLE.is_match(kind) {
        paint(colors, 31, line)
    } else if kind == "asrResult" || kind == "textInput" {
        paint(colors, 1, line)
    } else if kind == "aliceResponse" || kind == "connected" || kind == "yandexConnected" {
        paint(colors, 32, line)
    } else if event.dir == "note" {
        paint(colors, 36, line)
    } else {
        line.to_string()
    }
}

/// Follows the journals under the root: what was written since the last look, as lines to print.
pub struct Follower {
    root: PathBuf,
    every: bool,
    colors: bool,
    offsets: HashMap<PathBuf, u64>,
    /// A line still being written, kept for the next read.
    rests: HashMap<PathBuf, Vec<u8>>,
    /// New sessions' first events, held until it is clear whether the session is a side connection.
    held: HashMap<PathBuf, Vec<JournalEvent>>,
    sides: HashSet<PathBuf>,
    shown_session: String,
}

impl Follower {
    pub fn new(root: PathBuf, every: bool, colors: bool) -> Follower {
        Follower {
            root,
            every,
            colors,
            offsets: HashMap::new(),
            rests: HashMap::new(),
            held: HashMap::new(),
            sides: HashSet::new(),
            shown_session: String::new(),
        }
    }

    /// The journals of the two newest days, oldest first: a session goes on past midnight (UTC).
    pub fn journals(&self) -> Vec<PathBuf> {
        let names = |dir: &Path| -> Vec<String> {
            let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
            let mut names: Vec<String> =
                entries.filter_map(|entry| entry.ok()?.file_name().into_string().ok()).collect();
            names.sort();
            names
        };
        let days: Vec<String> = names(&self.root).into_iter().filter(|day| DAY.is_match(day)).collect();
        days[days.len().saturating_sub(2)..]
            .iter()
            .flat_map(|day| {
                let dir = self.root.join(day);
                names(&dir).into_iter().map(move |session| dir.join(session).join("events.jsonl"))
            })
            .filter(|file| file.is_file())
            .collect()
    }

    /// The header and, for context, the last events of the station's session written to last.
    pub fn start(&mut self) -> Vec<String> {
        let existing = self.journals();
        for file in &existing {
            self.offsets.insert(file.clone(), size(file));
        }
        let every = if self.every { ", every event" } else { "" };
        let mut lines = vec![paint(self.colors, 2, &format!("following {}{every}; Ctrl-C stops", self.root.display()))];
        let mut newest: Vec<(SystemTime, &PathBuf)> = existing
            .iter()
            .map(|file| {
                (std::fs::metadata(file).and_then(|meta| meta.modified()).unwrap_or(SystemTime::UNIX_EPOCH), file)
            })
            .collect();
        newest.sort_by_key(|entry| std::cmp::Reverse(entry.0));
        let side = format!("\"kind\":\"{SIDE}\"");
        let station = newest
            .into_iter()
            .map(|(_, file)| file.clone())
            .find(|file| std::fs::read_to_string(file).is_ok_and(|text| !text.contains(&side)));
        if let Some(file) = station {
            self.offsets.insert(file.clone(), 0);
            let events: Vec<JournalEvent> =
                self.read(&file).into_iter().filter(|event| shown(self.every, event)).collect();
            let context = events[events.len().saturating_sub(CONTEXT)..].to_vec();
            lines.extend(self.print(&file, context));
        }
        lines
    }

    /// What was written since the last look: new sessions too.
    pub fn poll(&mut self) -> Vec<String> {
        let mut lines = Vec::new();
        for file in self.journals() {
            if !self.offsets.contains_key(&file) {
                self.held.insert(file.clone(), Vec::new());
            }
            let events = self.read(&file);
            lines.extend(self.print(&file, events));
        }
        lines
    }

    /// The whole lines written since the last read; a line still being written waits for the next.
    fn read(&mut self, file: &Path) -> Vec<JournalEvent> {
        let size = size(file);
        let from = self.offsets.get(file).copied().unwrap_or(0);
        if size <= from {
            return Vec::new();
        }
        let mut chunk = Vec::new();
        let read = std::fs::File::open(file).and_then(|mut opened| {
            opened.seek(SeekFrom::Start(from))?;
            opened.take(size - from).read_to_end(&mut chunk)
        });
        if read.is_err() {
            return Vec::new();
        }
        self.offsets.insert(file.to_path_buf(), from + chunk.len() as u64);
        let mut pending = self.rests.remove(file).unwrap_or_default();
        pending.extend_from_slice(&chunk);
        let Some(end) = pending.iter().rposition(|&byte| byte == b'\n') else {
            self.rests.insert(file.to_path_buf(), pending);
            return Vec::new();
        };
        self.rests.insert(file.to_path_buf(), pending.split_off(end + 1));
        String::from_utf8_lossy(&pending)
            .split('\n')
            .filter(|line| !line.is_empty())
            .filter_map(|line| serde_json::from_str::<JournalEvent>(line).ok())
            .collect()
    }

    /// What of a session to print: nothing of a side connection, a new session's start once it is not one.
    fn sorted(&mut self, file: &Path, events: Vec<JournalEvent>) -> Vec<JournalEvent> {
        if self.every {
            return events;
        }
        if self.sides.contains(file) {
            return Vec::new();
        }
        let Some(start) = self.held.get_mut(file) else { return events };
        start.extend(events);
        let Some(telling) = start.iter().find(|event| !HANDSHAKE.contains(&event.kind.as_str())) else {
            return Vec::new();
        };
        let side = telling.kind == SIDE;
        let start = self.held.remove(file).unwrap_or_default();
        if !side {
            return start;
        }
        self.sides.insert(file.to_path_buf());
        Vec::new()
    }

    fn print(&mut self, file: &Path, events: Vec<JournalEvent>) -> Vec<String> {
        // The folder is <time>_<device>_<session>: the session's short id.
        let folder = file.parent().and_then(Path::file_name).map(|name| name.to_string_lossy().into_owned());
        let session = folder.as_deref().and_then(|name| name.rsplit('_').next()).unwrap_or("").to_string();
        let mut lines = Vec::new();
        for event in self.sorted(file, events).into_iter().filter(|event| shown(self.every, event)) {
            if session != self.shown_session {
                lines.push(paint(self.colors, 2, &format!("── session {session}")));
                self.shown_session = session.clone();
            }
            lines.push(format(&event, self.colors));
        }
        lines
    }
}

fn size(file: &Path) -> u64 {
    std::fs::metadata(file).map(|meta| meta.len()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use serde_json::json;

    use super::*;

    fn event(kind: &str, dir: &str, summary: &str) -> String {
        let mut line = json!({ "ts": "2026-11-06T02:11:00.331Z", "dir": dir, "kind": kind });
        if !summary.is_empty() {
            line["summary"] = json!(summary);
        }
        line.to_string() + "\n"
    }

    fn append(file: &Path, text: &str) {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::OpenOptions::new().create(true).append(true).open(file).unwrap().write_all(text.as_bytes()).unwrap();
    }

    fn aged(file: &Path, seconds_ago: u64) {
        let at = SystemTime::now() - Duration::from_secs(seconds_ago);
        std::fs::OpenOptions::new().append(true).open(file).unwrap().set_modified(at).unwrap();
    }

    /// The lines without the local time in front.
    fn bare(lines: Vec<String>) -> Vec<String> {
        lines
            .into_iter()
            .map(|line| match line.split_once(' ') {
                Some((time, rest)) if time.len() == 8 && time.as_bytes()[2] == b':' => rest.to_string(),
                _ => line,
            })
            .collect()
    }

    #[test]
    fn reads_a_journals_line_and_tells_what_is_worth_a_line() {
        let line = r#"{"ts":"2026-11-06T02:11:00.331Z","dir":"in","kind":"asrResult","summary":"endOfUtt=true  \"Включи\nсвет\"","data":{"x":1}}"#;
        let event: JournalEvent = serde_json::from_str(line).unwrap();
        assert_eq!(event.kind, "asrResult");
        assert_eq!(event.summary.as_deref(), Some("endOfUtt=true  \"Включи\nсвет\""));
        assert!(shown(false, &event));
        assert_eq!(format(&event, false).split_once(' ').unwrap().1, "←  asrResult endOfUtt=true \"Включи свет\"");
        assert_eq!(format(&event, false).split_once(' ').unwrap().0, local_time("2026-11-06T02:11:00.331Z"));
        assert!(format(&event, true).starts_with("\x1b[1m"), "what was heard in bold");

        let interim = JournalEvent { summary: Some("endOfUtt=false \"Вкл\"".into()), ..event.clone() };
        assert!(!shown(false, &interim) && shown(true, &interim));
        let routine = JournalEvent { kind: "requestStat".into(), ..event.clone() };
        assert!(!shown(false, &routine));
        let from_yandex = JournalEvent { dir: "fromYandex".into(), kind: "aliceResponse".into(), ..event.clone() };
        assert!(!shown(false, &from_yandex));

        let trouble = JournalEvent { kind: "yandexUnavailable".into(), dir: "note".into(), summary: None, ..event };
        assert_eq!(format(&trouble, true), format!("\x1b[31m{} •  yandexUnavailable\x1b[0m", local_time(&trouble.ts)));
        let to_yandex = JournalEvent { kind: "voiceInput".into(), dir: "toYandex".into(), ..trouble.clone() };
        assert!(format(&to_yandex, false).ends_with(" ⇢Y voiceInput"));
        let note = JournalEvent { kind: "route".into(), ..trouble };
        assert!(format(&note, true).starts_with("\x1b[36m"), "our notes in cyan");
        assert!(serde_json::from_str::<JournalEvent>("{oops").is_err());
    }

    #[test]
    fn starts_with_the_latest_station_session_and_follows_new_events_holding_back_side_connections() {
        let root = tempfile::tempdir().unwrap();
        let day = root.path().join("2026-11-06");
        let station = day.join("021100_dev_aaaa1111/events.jsonl");
        let side = day.join("021500_dev_bbbb2222/events.jsonl");
        // An older day is out of sight: only the two newest are followed.
        let old = root.path().join("2026-11-01/010000_dev_cccc3333/events.jsonl");
        let older = root.path().join("2026-11-04/010000_dev_dddd4444/events.jsonl");
        append(&old, &event("textInput", "in", "old"));
        append(&older, &event("textInput", "in", "older"));
        std::fs::write(root.path().join("notes.txt"), "not a day").unwrap();
        append(
            &station,
            &(event("connected", "note", "station")
                + &event("synchronizeState", "in", "")
                + &event("voiceInput", "in", "req-1")
                + &event("streamControl", "in", "close")
                + &event("asrResult", "out", "endOfUtt=false \"Как\"")
                + &event("asrResult", "out", "endOfUtt=true \"Как дела\"")
                + &event("aliceResponse", "out", "Хорошо")),
        );
        append(&side, &(event("connected", "note", "") + &event("logSpotter", "in", "spotter")));
        aged(&station, 60);
        aged(&older, 120);

        let mut follower = Follower::new(root.path().to_path_buf(), false, false);
        assert_eq!(follower.journals().len(), 3, "{:?}", follower.journals());
        let start = follower.start();
        assert_eq!(start[0], format!("following {}; Ctrl-C stops", root.path().display()));
        // The side connection is newer, the station's session is the one shown.
        assert_eq!(
            bare(start[1..].to_vec()),
            [
                "── session aaaa1111",
                "•  connected station",
                "←  synchronizeState",
                "←  voiceInput req-1",
                "→  asrResult endOfUtt=true \"Как дела\"",
                "→  aliceResponse Хорошо",
            ]
        );
        assert!(follower.poll().is_empty());

        // A new side connection prints nothing, ever.
        let spotter = day.join("022000_dev_eeee5555/events.jsonl");
        append(&spotter, &event("connected", "note", ""));
        assert!(follower.poll().is_empty());
        append(&spotter, &(event("logSpotter", "in", "") + &event("audio", "in", "") + &event("closed", "note", "")));
        assert!(follower.poll().is_empty());

        // A new station session: its handshake waits until it is clear, then comes whole.
        let next = day.join("022100_dev_ffff6666/events.jsonl");
        append(&next, &event("connected", "note", "reconnect"));
        assert!(follower.poll().is_empty());
        append(&next, &(event("voiceInput", "in", "req-2") + "{\"ts\":\"2026-11-06T02:21:01.000Z\",\"dir\":\"out\""));
        assert_eq!(bare(follower.poll()), ["── session ffff6666", "•  connected reconnect", "←  voiceInput req-2"]);
        // The line being written comes once it is whole; the old session goes on under its own heading.
        append(&next, ",\"kind\":\"aliceResponse\",\"summary\":\"Да\"}\n");
        append(&station, &event("yandexError", "note", "timeout"));
        assert_eq!(
            bare(follower.poll()),
            ["── session aaaa1111", "•  yandexError timeout", "── session ffff6666", "→  aliceResponse Да"]
        );
    }

    #[test]
    fn prints_every_event_with_all_side_connections_included() {
        let root = tempfile::tempdir().unwrap();
        let side = root.path().join("2026-11-06/021500_dev_bbbb2222/events.jsonl");
        append(&side, &event("connected", "note", ""));
        let mut follower = Follower::new(root.path().to_path_buf(), true, false);
        let start = follower.start();
        assert_eq!(start[0], format!("following {}, every event; Ctrl-C stops", root.path().display()));
        assert_eq!(bare(start[1..].to_vec()), ["── session bbbb2222", "•  connected"]);
        append(&side, &(event("logSpotter", "in", "") + &event("audio", "in", "") + &event("ack", "out", "")));
        assert_eq!(bare(follower.poll()), ["←  logSpotter", "←  audio", "→  ack"]);
        // No journals at all: the header only.
        let empty = tempfile::tempdir().unwrap();
        let mut follower = Follower::new(empty.path().join("sessions"), false, false);
        assert_eq!(follower.start().len(), 1);
        assert!(follower.poll().is_empty());
    }
}
