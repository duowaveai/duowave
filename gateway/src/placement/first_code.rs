//! The panel's first-run code at the end of an install, in the terminal the person already has (owner 22.11: «в конце
//! команды установки в терминале … вывести этот код, ну, или, по крайней мере, … логи последние. Чтобы не нужно было
//! искать, заходить отдельно»). While the panel has no password the gateway makes a new code at every start and writes
//! it to its log (access/auth_service.rs); on the station that log is gateway.sh's `gateway.log`, moved aside to
//! `gateway.log.old` before a start when it grew past 256 KB. No code there — the log's last lines, so what went wrong
//! is in sight too.

use std::path::Path;
use std::time::{Duration, Instant};

/// What the gateway's log line says before the code.
pub const LOG_MARK: &str = "первая настройка пульта";
/// How many of the log's last lines are shown when no code is in it.
pub const TAIL_LINES: usize = 15;
/// How long a fresh start has to write its code, at most.
pub const WAIT: Duration = Duration::from_secs(10);

/// The code `1234-5678` in a log line, if there is one.
pub fn code_in(text: &str) -> Option<String> {
    text.split(|c: char| !(c.is_ascii_digit() || c == '-'))
        .find(|word| {
            word.len() == 9 && word.as_bytes()[4] == b'-' && word.chars().filter(char::is_ascii_digit).count() == 8
        })
        .map(str::to_string)
}

/// The latest first-run code in a log's text: the last start's.
pub fn last_code(log: &str) -> Option<String> {
    log.lines().rev().filter(|line| line.contains(LOG_MARK)).find_map(code_in)
}

/// The last `count` lines of a log's text, the empty ones left out.
pub fn tail(log: &str, count: usize) -> Vec<String> {
    let lines: Vec<&str> = log.lines().filter(|line| !line.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(count)..].iter().map(|line| line.to_string()).collect()
}

/// What the station says of its panel's way in, at the end of `duowave station install` and in `duowave station code`.
/// `configured` is the gateway's own word (None: it did not answer): a panel with its password needs no code. Else
/// this start's code from `log`, read from `since` (its length before the start: a code of an earlier start is not
/// this one's; a log moved aside since is read whole), waited for up to `wait`. No code by then — the log's last lines.
pub fn station_lines(log: &Path, since: u64, configured: Option<bool>, wait: Duration) -> Vec<String> {
    if configured == Some(true) {
        return vec![crate::tr!("У пульта уже есть пароль: входите с ним.")];
    }
    let deadline = Instant::now() + wait;
    loop {
        let bytes = std::fs::read(log).unwrap_or_default();
        let from = if (bytes.len() as u64) < since { 0 } else { since as usize };
        if let Some(code) = last_code(&String::from_utf8_lossy(&bytes[from..])) {
            return vec![crate::tr!(
                "Код первой настройки пульта: {code} — пульт спросит его вместе с паролем.",
                code = code
            )];
        }
        if Instant::now() >= deadline {
            let mut lines = vec![crate::tr!(
                "Кода первой настройки в журнале пока нет. Последние строки журнала {log}:",
                log = log.display()
            )];
            lines
                .extend(tail(&String::from_utf8_lossy(&bytes), TAIL_LINES).into_iter().map(|line| format!("  {line}")));
            return lines;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE: &str =
        "2026-11-22T10:00:00Z WARN первая настройка пульта: с этого компьютера — без кода, с другого устройства — код";

    #[test]
    fn the_code_is_the_last_starts() {
        let log = format!("{LINE} 1111-2222\nstarted\n{LINE} 4821-0937\nlistening on 8790\n");
        assert_eq!(last_code(&log).as_deref(), Some("4821-0937"));
        assert_eq!(last_code("listening on 8790\nport 1234-5678 elsewhere\n"), None, "only the first-run line counts");
        assert_eq!(code_in("код 4821-0937"), Some("4821-0937".into()));
        assert_eq!(code_in("2026-11-22 10:00"), None);
    }

    #[test]
    fn the_install_says_this_starts_code_or_the_logs_end() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("gateway.log");
        // An earlier start's code before the install: not this one's — the log's end, where the code is not.
        std::fs::write(&log, format!("{LINE} 1111-2222\n")).unwrap();
        let since = std::fs::metadata(&log).unwrap().len();
        let said = station_lines(&log, since, Some(false), Duration::ZERO);
        assert!(said[0].starts_with("Кода первой настройки в журнале пока нет."), "{said:?}");
        assert_eq!(said.len(), 2, "{said:?}");
        assert!(said[1].ends_with("1111-2222") && said[1].starts_with("  "), "{said:?}");
        // This start's line after it.
        std::fs::write(&log, format!("{LINE} 1111-2222\nstarting\n{LINE} 4821-0937\n")).unwrap();
        let said = station_lines(&log, since, Some(false), Duration::ZERO);
        assert_eq!(
            said,
            vec!["Код первой настройки пульта: 4821-0937 — пульт спросит его вместе с паролем.".to_string()]
        );
        // The log moved aside before the start: the new one is read whole, and an unknown state reads it too.
        std::fs::write(&log, format!("{LINE} 5555-6666\n")).unwrap();
        assert!(station_lines(&log, since + 1000, None, Duration::ZERO)[0].contains("5555-6666"));
        // A panel with its password: no code, no log.
        assert_eq!(
            station_lines(&log, 0, Some(true), Duration::ZERO),
            vec!["У пульта уже есть пароль: входите с ним."]
        );
        // No log at all: said, nothing more.
        assert_eq!(station_lines(&dir.path().join("none.log"), 0, Some(false), Duration::ZERO).len(), 1);
    }

    #[test]
    fn the_logs_end_is_its_last_lines_without_the_empty_ones() {
        let log = (1..=20).map(|n| format!("line {n}\n\n")).collect::<String>();
        let lines = tail(&log, TAIL_LINES);
        assert_eq!(lines.len(), TAIL_LINES);
        assert_eq!(lines.first().map(String::as_str), Some("line 6"));
        assert_eq!(lines.last().map(String::as_str), Some("line 20"));
    }
}
