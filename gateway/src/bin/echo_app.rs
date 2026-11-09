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

/// A request to the gateway's Host API: its status line's code.
fn host(method: &str, path: &str, body: &str) -> Option<u16> {
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
    answer.split_whitespace().nth(1)?.parse().ok()
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
