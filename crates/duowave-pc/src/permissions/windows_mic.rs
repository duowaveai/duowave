//! Windows' microphone switches as they concern a desktop program (not one from the Store): «Доступ к микрофону» for the
//! whole computer (HKLM) and for this person (HKCU), and «Разрешить классическим приложениям доступ к микрофону»
//! (`NonPackaged`). Windows asks no desktop program by a window of its own today: when one of them is off, the capture
//! gives silence or fails, and nothing says why.
//!
//! The keys are not documented by Microsoft; Handy reads the same ones. `NonPackaged` is read first: tools that strip
//! Windows down may deny the Store apps' key and leave the desktop programs' one open. A key that is missing or cannot
//! be read counts as allowed — a wrong read must never stop anyone. Read with `reg query` (no rights needed), at most
//! once in two seconds.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::State;

const CONSENT: &str = r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone";
const AGAIN: Duration = Duration::from_secs(2);

static LAST: Mutex<Option<(Instant, State)>> = Mutex::new(None);

/// The microphone for desktop programs now; off Windows, unknown.
pub async fn state() -> State {
    if !cfg!(windows) {
        return State::Unknown;
    }
    if let Some((at, state)) = *LAST.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) {
        if at.elapsed() < AGAIN {
            return state;
        }
    }
    let state = tokio::task::spawn_blocking(read).await.unwrap_or(State::Unknown);
    *LAST.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((Instant::now(), state));
    state
}

fn read() -> State {
    let machine = value_at(&format!(r"HKLM\{CONSENT}"));
    let person = value_at(&format!(r"HKCU\{CONSENT}"));
    let desktop = value_at(&format!(r"HKCU\{CONSENT}\NonPackaged"));
    judge(machine.as_deref(), person.as_deref(), desktop.as_deref())
}

/// The `Value` of one key, as `reg query` prints it; None when missing or unread.
fn value_at(key: &str) -> Option<String> {
    let out = std::process::Command::new("reg").args(["query", key, "/v", "Value"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    value_of(&String::from_utf8_lossy(&out.stdout))
}

/// `    Value    REG_SZ    Allow` → `Allow`.
fn value_of(printed: &str) -> Option<String> {
    printed.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        if parts.next()? != "Value" || !parts.next()?.starts_with("REG_") {
            return None;
        }
        parts.next().map(str::to_string)
    })
}

/// The three switches together: any `Deny` on the way — denied; an `Allow` for desktop programs, or nothing denied —
/// allowed; nothing read at all — unknown.
fn judge(machine: Option<&str>, person: Option<&str>, desktop: Option<&str>) -> State {
    let denied = |value: Option<&str>| value.is_some_and(|value| value.eq_ignore_ascii_case("deny"));
    if denied(machine) || denied(desktop) {
        return State::Denied;
    }
    // The person's own switch counts only when the desktop programs' one says nothing (Handy's order).
    if desktop.is_none() && denied(person) {
        return State::Denied;
    }
    if machine.is_none() && person.is_none() && desktop.is_none() {
        return State::Unknown;
    }
    State::Granted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_value_is_read_from_reg_query() {
        let printed =
            "\r\nHKEY_CURRENT_USER\\Software\\...\\microphone\\NonPackaged\r\n    Value    REG_SZ    Deny\r\n\r\n";
        assert_eq!(value_of(printed).as_deref(), Some("Deny"));
        assert_eq!(value_of("ERROR: The system was unable to find the specified registry key or value."), None);
        assert_eq!(value_of("    LastUsedTimeStart    REG_QWORD    0x1d9"), None);
    }

    #[test]
    fn any_deny_on_the_way_denies() {
        assert_eq!(judge(Some("Allow"), Some("Allow"), Some("Deny")), State::Denied);
        assert_eq!(judge(Some("Deny"), Some("Allow"), Some("Allow")), State::Denied);
        assert_eq!(judge(Some("Allow"), Some("Deny"), None), State::Denied);
    }

    #[test]
    fn the_desktop_switch_wins_over_the_store_apps_one() {
        // A debloater denied the Store apps' key; desktop programs are still allowed.
        assert_eq!(judge(Some("Allow"), Some("Deny"), Some("Allow")), State::Granted);
    }

    #[test]
    fn nothing_read_is_unknown_and_never_blocks() {
        assert_eq!(judge(None, None, None), State::Unknown);
        assert_eq!(judge(Some("Allow"), None, None), State::Granted);
    }
}
