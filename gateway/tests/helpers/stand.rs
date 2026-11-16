//! A stand-in helper for the block's end-to-end tests: a `/home/v1` on 127.0.0.1 — at home, as a computer of the
//! household is — with a switch for «away» (its status answers 500: a miss), its status of contract 1 or an old one
//! without a contract, its bot's contacts (200, or 503 without a bot), what its hearing hears, a voice, a model's
//! stream that counts its hits, and its copy of every app (`/home/v1/plugins/{id}/call`), which answers what the
//! hearing hears and counts its calls.

#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use serde_json::{json, Value};

/// The key every stand-in takes.
pub const KEY: &str = "vgw_stand_key_for_tests";

/// Every ability of contract 1, ready.
pub const ALL: &[(&str, bool)] = &[
    ("hearing", true),
    ("voice", true),
    ("model", true),
    ("messages", true),
    ("apps", true),
    ("history", true),
    ("catalog", true),
];

/// A status of contract 1: the helper's id, its name «Помощник <id>», what it declares and whether it is ready.
pub fn contract(id: &str, abilities: &[(&str, bool)]) -> Value {
    let abilities: Vec<Value> = abilities
        .iter()
        .map(|(ability, ready)| {
            let details = if *ability == "voice" { json!({ "voice": "silero/baya" }) } else { json!({}) };
            json!({ "id": ability, "ready": ready, "details": details })
        })
        .collect();
    json!({
        "version": "0.12.0", "peerApi": 2, "asr": true, "tts": true, "voice": "silero/baya", "llm": true,
        "contract": { "version": 1, "helper": { "id": id, "name": format!("Турбо {id}") }, "abilities": abilities }
    })
}

/// What a computer of 0.11.1 answers: no contract.
pub fn status_0_11() -> Value {
    json!({ "version": "0.11.1", "peerApi": 2, "asr": true, "tts": true, "voice": "silero/baya", "llm": true })
}

#[derive(Default)]
pub struct Stand {
    /// `http://127.0.0.1:<port>`.
    pub url: String,
    pub away: AtomicBool,
    pub status: Mutex<Value>,
    /// None: no bot set up.
    pub contacts: Mutex<Option<Vec<String>>>,
    pub heard: Mutex<String>,
    /// The looks at its status that reached it.
    pub looks: AtomicUsize,
    /// The model's streams it gave.
    pub llm_hits: AtomicUsize,
    /// The apps' calls it answered.
    pub app_calls: AtomicUsize,
}

impl Stand {
    pub fn set_away(&self, away: bool) {
        self.away.store(away, Ordering::SeqCst);
    }

    pub fn looks(&self) -> usize {
        self.looks.load(Ordering::SeqCst)
    }

    pub fn app_calls(&self) -> usize {
        self.app_calls.load(Ordering::SeqCst)
    }
}

fn keyed(headers: &HeaderMap) -> bool {
    headers.get("authorization").and_then(|value| value.to_str().ok()) == Some(&format!("Bearer {KEY}"))
}

pub async fn stand(status: Value) -> Arc<Stand> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let state = Arc::new(Stand {
        url: format!("http://{address}"),
        status: Mutex::new(status),
        heard: Mutex::new("слышу".into()),
        ..Stand::default()
    });
    let (looked, read, ear, model, apps) = (state.clone(), state.clone(), state.clone(), state.clone(), state.clone());
    let router = axum::Router::new()
        .route(
            "/home/v1/status",
            get(move |headers: HeaderMap| {
                let state = looked.clone();
                async move {
                    if state.away.load(Ordering::SeqCst) {
                        return Err(StatusCode::INTERNAL_SERVER_ERROR);
                    }
                    if !keyed(&headers) {
                        return Err(StatusCode::UNAUTHORIZED);
                    }
                    state.looks.fetch_add(1, Ordering::SeqCst);
                    Ok(axum::Json(state.status.lock().unwrap().clone()))
                }
            }),
        )
        .route(
            "/home/v1/messenger/contacts",
            get(move |headers: HeaderMap| {
                let state = read.clone();
                async move {
                    if !keyed(&headers) {
                        return Err(StatusCode::UNAUTHORIZED);
                    }
                    let contacts = state.contacts.lock().unwrap().clone().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
                    Ok(axum::Json(json!({ "contacts": contacts })))
                }
            }),
        )
        .route(
            "/home/v1/asr/audio/transcriptions",
            post(move |headers: HeaderMap| {
                let state = ear.clone();
                async move {
                    if !keyed(&headers) || state.away.load(Ordering::SeqCst) {
                        return Err(StatusCode::INTERNAL_SERVER_ERROR);
                    }
                    Ok(axum::Json(json!({ "text": state.heard.lock().unwrap().clone() })))
                }
            }),
        )
        .route("/home/v1/asr/models", get(|| async { axum::Json(json!({ "data": [{ "id": "home" }] })) }))
        .route("/home/v1/tts/audio/speech", post(|| async { b"OggS-voice".to_vec() }))
        .route(
            "/home/v1/llm/chat/completions",
            post(move || {
                let state = model.clone();
                async move {
                    state.llm_hits.fetch_add(1, Ordering::SeqCst);
                    let chunk = json!({ "choices": [{ "delta": { "content": "Ответ Турбо." } }] });
                    let body = format!("data: {chunk}\n\ndata: [DONE]\n\n");
                    ([(axum::http::header::CONTENT_TYPE, "text/event-stream")], body)
                }
            }),
        )
        .route(
            "/home/v1/plugins/{id}/call",
            post(move |axum::extract::Path(id): axum::extract::Path<String>, headers: HeaderMap| {
                let state = apps.clone();
                async move {
                    if !keyed(&headers) || state.away.load(Ordering::SeqCst) {
                        return Err(StatusCode::INTERNAL_SERVER_ERROR);
                    }
                    state.app_calls.fetch_add(1, Ordering::SeqCst);
                    Ok(axum::Json(json!({ "result": format!("{id}: {}", state.heard.lock().unwrap()) })))
                }
            }),
        );
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    state
}
