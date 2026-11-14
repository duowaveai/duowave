//! `duowave-echo-app`: the smallest app for the gateway (docs/plans/plugins.md, stage 9) — the reference for writing
//! one and the program the apps' tests run. Only the standard library: an app may be written in anything that serves
//! HTTP on 127.0.0.1.
//!
//! What the supervisor gives it in its environment: `PORT` (where to listen), `DUOWAVE_URL` and `DUOWAVE_APP_TOKEN`
//! (the Host API: `/api/v1/apps/host/…` with `Authorization: Bearer <token>`), `DUOWAVE_APP_ID`, `DUOWAVE_APP_DATA` (its
//! own folder). What it serves:
//!
//! - `GET /health` — 200 while it is well;
//! - `POST /call/echo` — its input back, and remembered through the Host API's `kv` (`last`);
//! - `POST /call/crash` — falls on purpose (the supervisor's restart is tried with it);
//! - `POST /call/count` `{seconds?, fail?}` — a long task (src/tasks; docs/plugins/AUTHORING.md, «Долгие задачи»): it
//!   «counts for a long time» — answers at once with the task's id (`{task}`), then counts in a thread of its own,
//!   telling the gateway how far it got twice a second; a person's «Отменить» reaches it with the answer to its next
//!   report, and it stops. `fail: true` — it loses count halfway, to see a task fail. The gateway's refusal (a task
//!   already runs) comes back as `{refused}`;
//! - `GET /stream/<name>` — its own sound (a few bytes of MP3 here), which the gateway gives on to the station at
//!   `/media/apps/<id>/<name>`: an item `{ id, title, stream: "<name>" }` plays it.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};

/// An MP3 frame header and silence: enough to tell the sound came through.
const SOUND: &[u8] = &[0xFF, 0xFB, 0x90, 0x64, 0, 0, 0, 0];

fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

/// A request to the gateway's Host API: its status line's code and its body.
fn host_answer(method: &str, path: &str, body: &str) -> Option<(u16, String)> {
    let url = std::env::var("DUOWAVE_URL").ok()?;
    let token = std::env::var("DUOWAVE_APP_TOKEN").ok()?;
    let address = url.strip_prefix("http://")?.trim_end_matches('/').to_string();
    let mut stream = TcpStream::connect(&address).ok()?;
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .ok()?;
    let mut answer = String::new();
    stream.read_to_string(&mut answer).ok()?;
    let code = answer.split_whitespace().nth(1)?.parse().ok()?;
    let body = answer.split_once("\r\n\r\n").map(|(_, body)| body.to_string()).unwrap_or_default();
    Some((code, body))
}

/// A request to the gateway's Host API: its status line's code.
fn host(method: &str, path: &str, body: &str) -> Option<u16> {
    host_answer(method, path, body).map(|(code, _)| code)
}

/// A whole number of a flat JSON object, by its key: enough for what this app is sent.
fn number(body: &str, key: &str) -> Option<u64> {
    let rest = body.split_once(&format!("\"{key}\""))?.1.trim_start().strip_prefix(':')?.trim_start();
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// A string of a flat JSON object, by its key — one without escapes (an id).
fn string(body: &str, key: &str) -> Option<String> {
    let rest = body.split_once(&format!("\"{key}\""))?.1.trim_start().strip_prefix(':')?.trim_start();
    Some(rest.strip_prefix('"')?.split_once('"')?.0.to_string())
}

fn truth(body: &str, key: &str) -> bool {
    body.split_once(&format!("\"{key}\""))
        .and_then(|(_, rest)| rest.trim_start().strip_prefix(':'))
        .is_some_and(|rest| rest.trim_start().starts_with("true"))
}

/// How often the count tells the gateway how far it got.
const COUNT_STEP: std::time::Duration = std::time::Duration::from_millis(500);

/// The long task: `seconds` of counting, a report every half a second. It ends the task itself — done with what it
/// counted, failed halfway when asked to, cancelled when the gateway's answer says a person asked for that.
fn count(task: String, seconds: u64, fail: bool) {
    let steps = (seconds * 2).max(1);
    let path = format!("/api/v1/apps/host/tasks/{task}");
    for step in 1..=steps {
        std::thread::sleep(COUNT_STEP);
        if fail && step * 2 >= steps {
            host("POST", &format!("{path}/fail"), "{\"error\":\"Сбился со счёта на половине.\"}");
            return;
        }
        let percent = step * 100 / steps;
        let text = match percent {
            0..=19 => "Разминаюсь",
            20..=79 => "Считаю",
            _ => "Проверяю счёт",
        };
        let report = format!("{{\"percent\":{percent},\"text\":\"{text}\"}}");
        match host_answer("POST", &format!("{path}/progress"), &report) {
            Some((200, answer)) if truth(&answer, "cancelRequested") => {
                host("POST", &format!("{path}/cancelled"), "");
                eprintln!("count: cancelled at {percent}%");
                return;
            }
            Some((200, _)) => {}
            // The task is over without us (409: the gateway closed it), or the gateway is gone: nothing to count for.
            other => {
                eprintln!("count: the gateway answered {:?}, stopped", other.map(|(code, _)| code));
                return;
            }
        }
    }
    let result = format!("{{\"result\":{{\"counted\":{steps},\"seconds\":{seconds}}}}}");
    host("POST", &format!("{path}/done"), &result);
}

fn serve(mut stream: TcpStream) {
    let mut reader = BufReader::new(stream.try_clone().expect("the stream clones"));
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let mut parts = line.split_whitespace();
    let (method, path) = (parts.next().unwrap_or_default().to_string(), parts.next().unwrap_or_default().to_string());
    let mut length = 0usize;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).is_err() || header.trim().is_empty() {
            break;
        }
        if let Some(value) = header.to_ascii_lowercase().strip_prefix("content-length:") {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; length.min(1024 * 1024)];
    let _ = reader.read_exact(&mut body);
    let body = String::from_utf8_lossy(&body).into_owned();
    match (method.as_str(), path.as_str()) {
        ("GET", "/health") => respond(&mut stream, "200 OK", "{\"ok\":true}"),
        ("POST", "/call/echo") => {
            let stored = host("PUT", "/api/v1/apps/host/kv/last", if body.is_empty() { "null" } else { &body });
            eprintln!("echo: kv answered {stored:?}");
            let answer = format!(
                "{{\"echo\":{},\"pid\":{},\"stored\":{}}}",
                if body.is_empty() { "null" } else { &body },
                std::process::id(),
                stored == Some(204)
            );
            respond(&mut stream, "200 OK", &answer);
        }
        ("POST", "/call/count") => {
            let seconds = number(&body, "seconds").unwrap_or(20).clamp(1, 3600);
            let start = format!("{{\"title\":\"Долго считаю: {seconds} с\",\"text\":\"Разминаюсь\",\"percent\":0}}");
            match host_answer("POST", "/api/v1/apps/host/tasks", &start) {
                Some((201, task)) => match string(&task, "id") {
                    Some(id) => {
                        let fail = truth(&body, "fail");
                        respond(&mut stream, "200 OK", &format!("{{\"task\":\"{id}\"}}"));
                        std::thread::spawn(move || count(id, seconds, fail));
                    }
                    None => {
                        respond(&mut stream, "200 OK", "{\"refused\":{\"error\":\"no id in the gateway's answer\"}}")
                    }
                },
                // 429: it counts already — the gateway's words go back as they are.
                Some((_, refusal)) if refusal.trim_start().starts_with('{') => {
                    respond(&mut stream, "200 OK", &format!("{{\"refused\":{}}}", refusal.trim()));
                }
                _ => respond(&mut stream, "200 OK", "{\"refused\":{\"error\":\"the gateway did not answer\"}}"),
            }
        }
        ("POST", "/call/crash") => {
            respond(&mut stream, "200 OK", "null");
            std::process::exit(3);
        }
        ("GET", sound) if sound.starts_with("/stream/") => {
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: audio/mpeg\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                SOUND.len()
            );
            let _ = stream.write_all(SOUND);
        }
        _ => respond(&mut stream, "404 Not Found", "{\"error\":\"no such function\"}"),
    }
}

fn main() {
    let port = std::env::var("PORT").unwrap_or_else(|_| "8799".into());
    let listener = TcpListener::bind(format!("127.0.0.1:{port}")).expect("the port is free");
    println!("duowave-echo-app listens on {port}");
    for stream in listener.incoming().flatten() {
        std::thread::spawn(move || serve(stream));
    }
}
