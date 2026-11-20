//! «Отчёт для разработчика»: what a person sends us when something goes wrong on their computer — we debug strangers'
//! machines from pasted logs (the Windows test of 21.11). This build, the system, on Windows what keeps others from
//! reaching the computer, and the end of the gateway's log — with every stored secret, the home folder, the user's
//! name and e-mail addresses taken out. `GET /api/v1/system/report` → `{text}`; the panel copies it.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::{Arc, LazyLock};

use axum::extract::State;
use axum::routing::get;
use axum::Router;
use regex::Regex;
use serde::Serialize;

use crate::app::App;
use crate::core::identity::permissions::SETTINGS_EDIT;
use crate::platform::http::{ok, ApiResult, Caller};

/// How much of the log's end goes in: enough for an evening's trouble, small enough to paste.
const LOG_BYTES: u64 = 192 * 1024;
const LOG_LINES: usize = 600;

#[derive(Debug, Serialize)]
pub struct Report {
    pub text: String,
}

pub fn routes() -> Router<Arc<App>> {
    Router::new().route("/api/v1/system/report", get(report))
}

async fn report(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Report> {
    caller.permitted(SETTINGS_EDIT)?;
    let mut text = format!(
        "DuoWave {} · {} {} · {}\n",
        crate::VERSION,
        std::env::consts::OS,
        std::env::consts::ARCH,
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    );
    if let Some(network) = crate::platform::windows_net::refresh().await {
        text.push_str(&format!("windows network: {}\n", network.summary()));
    }
    match crate::log_file() {
        Some(path) => {
            text.push_str(&format!("--- gateway.log, the last {LOG_LINES} lines ---\n"));
            text.push_str(&tail(path, LOG_BYTES, LOG_LINES));
        }
        None => text.push_str("--- the gateway logs to its terminal or service journal, not to a file ---\n"),
    }
    let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).ok();
    let user = std::env::var("USER").or_else(|_| std::env::var("USERNAME")).ok();
    Ok(ok(Report { text: clean(&app.secrets.redact(&text), home.as_deref(), user.as_deref()) }))
}

/// The last `lines` of a file, read from at most its last `bytes`.
fn tail(path: &Path, bytes: u64, lines: usize) -> String {
    let Ok(mut file) = std::fs::File::open(path) else { return format!("({} is not readable)\n", path.display()) };
    let length = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    let _ = file.seek(SeekFrom::Start(length.saturating_sub(bytes)));
    let mut raw = Vec::new();
    let _ = file.read_to_end(&mut raw);
    let text = String::from_utf8_lossy(&raw);
    let all: Vec<&str> = text.lines().collect();
    let mut out = all[all.len().saturating_sub(lines)..].join("\n");
    out.push('\n');
    out
}

static EMAIL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}").expect("a valid pattern"));

/// What is personal taken out: the home folder (as it is and with the other slashes) → `~`, the user's name →
/// `<user>`, e-mail addresses → `<email>`.
fn clean(text: &str, home: Option<&str>, user: Option<&str>) -> String {
    let mut text = text.to_string();
    if let Some(home) = home.filter(|home| home.len() > 3) {
        for form in [home.to_string(), home.replace('\\', "/"), home.replace('\\', "\\\\")] {
            text = text.replace(&form, "~");
        }
    }
    if let Some(user) = user.filter(|user| user.chars().count() >= 3) {
        text = text.replace(user, "<user>");
    }
    EMAIL.replace_all(&text, "<email>").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takes_out_the_home_folder_the_user_and_addresses() {
        let said =
            r"C:\Users\Anna\AppData\Roaming\app.duowave.desktop\voices · C:/Users/Anna/x · anna.k@example.org · Anna";
        let clean = clean(said, Some(r"C:\Users\Anna"), Some("Anna"));
        assert_eq!(clean, r"~\AppData\Roaming\app.duowave.desktop\voices · ~/x · <email> · <user>");
        assert_eq!(super::clean("nothing", Some("/"), Some("ab")), "nothing", "too short to be sure");
    }

    #[test]
    fn reads_the_end_of_the_log() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("gateway.log");
        std::fs::write(&log, (1..=10).map(|n| format!("line {n}\n")).collect::<String>()).unwrap();
        assert_eq!(tail(&log, 1024, 3), "line 8\nline 9\nline 10\n");
        assert!(tail(&dir.path().join("none.log"), 1024, 3).contains("not readable"));
    }
}
