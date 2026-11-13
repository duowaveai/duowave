//! `/api/v1/voice-lab`: the owner's own voices — the engines, the takes, the imprints. Anyone who sees the station
//! sees what is there; changing it is `models.manage`. Takes are people's voices: they arrive only from the home
//! network.
//!
//! The `.ts` files named here are the Node gateway's, removed from the tree: git tag `node-gateway-last`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body as RawBody;
use axum::extract::{Path, RawQuery, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post};
use axum::Router;
use futures::StreamExt;
use serde_json::{json, Value};

use super::jobs::JobState;
use super::js;
use super::service::VoiceLabView;
use super::voice_store::{OwnVoice, TakeInfo, MAX_TAKE_BYTES};
use crate::app::App;
use crate::core::identity::permissions::{MODELS_MANAGE, STATION_VIEW};
use crate::platform::http::{created, ok, ApiError, ApiResult, Body, Caller, Zone};

pub fn router() -> Router<Arc<App>> {
    Router::new()
        .route("/api/v1/voice-lab", get(view))
        .route("/api/v1/voice-lab/platform/recheck", post(recheck))
        .route("/api/v1/voice-lab/engines/{id}/install", post(install))
        .route("/api/v1/voice-lab/draft/takes", post(add_take))
        .route("/api/v1/voice-lab/draft/takes/{n}", get(take).delete(remove_take))
        .route("/api/v1/voice-lab/draft", delete(clear_draft))
        .route("/api/v1/voice-lab/imprint", post(imprint))
        .route("/api/v1/voice-lab/job/stop", post(stop))
        .route("/api/v1/voice-lab/voices/{id}/samples/{n}", get(sample))
        .route("/api/v1/voice-lab/voices/{id}", patch(rename).delete(remove))
        .route("/api/v1/voice-lab/voices/{id}/reimprint", post(reimprint))
}

/// How long an upload may stand still: a phone that left the Wi-Fi mid-take without a word frees its request, its
/// buffered take and the gateway's shutdown (Node ended such a request after its 300 s requestTimeout).
const UPLOAD_IDLE: Duration = Duration::from_secs(60);

/// The raw body of an upload, refused past the limit while it still arrives.
pub(crate) async fn read_body(body: RawBody, limit: usize) -> Result<Vec<u8>, ApiError> {
    read_body_within(body, limit, UPLOAD_IDLE).await
}

async fn read_body_within(body: RawBody, limit: usize, idle: Duration) -> Result<Vec<u8>, ApiError> {
    let mut stream = body.into_data_stream();
    let mut bytes = Vec::new();
    loop {
        let Ok(next) = tokio::time::timeout(idle, stream.next()).await else {
            return Err(ApiError::new(StatusCode::REQUEST_TIMEOUT, "Запись не дошла: связь прервалась."));
        };
        let Some(chunk) = next else { break };
        let chunk = chunk.map_err(|error| ApiError::bad_request(error.to_string()))?;
        if bytes.len() + chunk.len() > limit {
            return Err(ApiError::bad_request(format!(
                "Запись больше {} МБ.",
                js::round_i64(limit as f64 / 1_048_576.0)
            )));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// `?passage=N`: which passage was read, when it is one whole number.
pub(crate) fn passage(query: Option<&str>) -> Option<i64> {
    let values: Vec<String> = url::form_urlencoded::parse(query.unwrap_or("").as_bytes())
        .filter(|(name, _)| name == "passage")
        .map(|(_, value)| value.into_owned())
        .collect();
    match values.as_slice() {
        [value] => js::integer(value),
        _ => None,
    }
}

/// A WAV from the store, never cached: a take re-recorded under the same number is a new one.
async fn wav(path: PathBuf, missing: &str) -> Result<Response, ApiError> {
    let bytes = tokio::fs::read(&path).await.map_err(|_| ApiError::not_found(missing))?;
    let mut response = RawBody::from(bytes).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("audio/wav"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

async fn view(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<VoiceLabView> {
    caller.permitted(STATION_VIEW)?;
    Ok(ok(app.voice_lab.view().await))
}

async fn recheck(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<VoiceLabView> {
    caller.permitted(MODELS_MANAGE)?;
    Ok(created(app.voice_lab.recheck().await))
}

async fn install(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<JobState> {
    caller.permitted(MODELS_MANAGE)?;
    Ok(created(app.voice_lab.install(&id, caller.actor()).await?))
}

/// A take: the body is a 16-bit PCM WAV (the panel records or converts it), ?passage=N — which passage was read.
async fn add_take(
    State(app): State<Arc<App>>,
    caller: Caller,
    RawQuery(query): RawQuery,
    body: RawBody,
) -> ApiResult<TakeInfo> {
    caller.zone_at_least(Zone::Home)?;
    caller.permitted(MODELS_MANAGE)?;
    let passage = passage(query.as_deref());
    let body = read_body(body, MAX_TAKE_BYTES).await?;
    Ok(created(app.voice_lab.add_take(body, passage).await?))
}

async fn take(State(app): State<Arc<App>>, caller: Caller, Path(n): Path<String>) -> Result<Response, ApiError> {
    caller.permitted(MODELS_MANAGE)?;
    wav(app.voice_lab.take_path(js::integer(&n))?, "нет такого дубля").await
}

async fn remove_take(State(app): State<Arc<App>>, caller: Caller, Path(n): Path<String>) -> ApiResult<Value> {
    caller.permitted(MODELS_MANAGE)?;
    app.voice_lab.remove_take(js::integer(&n))?;
    Ok(ok(json!({ "deleted": true })))
}

async fn clear_draft(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Value> {
    caller.permitted(MODELS_MANAGE)?;
    app.voice_lab.clear_draft()?;
    Ok(ok(json!({ "cleared": true })))
}

/// «Снять отпечаток»: {engine, name, anyway?}; the progress rides in GET /voice-lab. Tight on memory: 428 with the
/// warning, and the same request with `"anyway": true` starts it.
async fn imprint(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<Value>) -> ApiResult<JobState> {
    caller.permitted(MODELS_MANAGE)?;
    let Some(engine) = body.get("engine").and_then(Value::as_str) else {
        return Err(ApiError::bad_request(r#"body: {"engine": "omnivoice", "name": "Мой голос"}"#));
    };
    let anyway = body.get("anyway") == Some(&Value::Bool(true));
    Ok(created(app.voice_lab.imprint(engine, body.get("name"), caller.actor(), anyway).await?))
}

/// «Переснять для другой модели»: {engine, anyway?} — the voice imprinted anew under that engine from its reference
/// and words (or its takes), a new voice beside it; the progress rides in GET /voice-lab, 428 as for an imprint.
async fn reimprint(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<Value>,
) -> ApiResult<JobState> {
    caller.permitted(MODELS_MANAGE)?;
    let Some(engine) = body.get("engine").and_then(Value::as_str) else {
        return Err(ApiError::bad_request(r#"body: {"engine": "cosyvoice3"}"#));
    };
    let anyway = body.get("anyway") == Some(&Value::Bool(true));
    Ok(created(app.voice_lab.reimprint(&id, engine, anyway, caller.actor()).await?))
}

async fn stop(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Value> {
    caller.permitted(MODELS_MANAGE)?;
    Ok(created(json!({ "stopped": app.voice_lab.stop_job() })))
}

async fn sample(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path((id, n)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    caller.permitted(MODELS_MANAGE)?;
    wav(app.voice_lab.sample_path(&id, js::integer(&n))?, "нет такой пробы").await
}

async fn rename(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<Value>,
) -> ApiResult<OwnVoice> {
    caller.permitted(MODELS_MANAGE)?;
    Ok(ok(app.voice_lab.rename(&id, body.get("name"), caller.actor())?))
}

async fn remove(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<Value> {
    caller.permitted(MODELS_MANAGE)?;
    app.voice_lab.remove(&id, caller.actor())?;
    Ok(ok(json!({ "deleted": true })))
}

/// test/voice-lab.e2e-spec.ts through the handlers, with the caller a signed-in admin of this computer (sign-in itself
/// is the access module's): engines, takes, an imprint, choosing it. tests/voice_lab.rs asks the same over HTTP.
#[cfg(all(test, unix))]
mod tests {
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    use axum::http::StatusCode;
    use serde::Serialize;

    use super::*;
    use crate::core::identity::principal::user_principal;
    use crate::core::identity::Role;
    use crate::platform::http::Api;
    use crate::voice_lab::service::testing::{lab, spoken_take, Lab};

    fn admin(lab: &Lab) -> Caller {
        Caller::local(lab.app.clone(), Some(user_principal("admin", "Админ", Role::Admin, None)))
    }

    fn state(lab: &Lab) -> State<Arc<App>> {
        State(lab.app.clone())
    }

    /// An answer as the envelope carries it: the status and the message's JSON, or the error's.
    fn answer<T: Serialize>(result: ApiResult<T>) -> (StatusCode, Value) {
        match result {
            Ok(Api(status, value)) => (status, serde_json::to_value(value).unwrap()),
            Err(error) => (error.status, Value::String(error.message)),
        }
    }

    async fn view_of(lab: &Lab) -> Value {
        let (status, view) = answer(view(state(lab), admin(lab)).await);
        assert_eq!(status, StatusCode::OK);
        view
    }

    async fn add(lab: &Lab, query: Option<&str>, body: Vec<u8>) -> (StatusCode, Value) {
        answer(add_take(state(lab), admin(lab), RawQuery(query.map(str::to_string)), RawBody::from(body)).await)
    }

    async fn imprint_as(lab: &Lab, body: Value) -> (StatusCode, Value) {
        answer(imprint(state(lab), admin(lab), Body(body)).await)
    }

    async fn finished_job(lab: &Lab) -> Value {
        for _ in 0..100 {
            let job = view_of(lab).await["job"].clone();
            if job["state"] != "running" {
                return job;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the imprint did not finish");
    }

    async fn imprinted(lab: &Lab, name: &str) -> String {
        assert_eq!(add(lab, Some("passage=1"), spoken_take()).await.0, StatusCode::CREATED);
        let (status, job) = imprint_as(lab, json!({ "engine": "omnivoice", "name": name })).await;
        assert_eq!(status, StatusCode::CREATED, "{job}");
        assert_eq!(
            (&job["kind"], &job["engine"], &job["state"]),
            (&json!("imprint"), &json!("omnivoice"), &json!("running"))
        );
        let job = finished_job(lab).await;
        assert_eq!((&job["state"], &job["peakMb"]), (&json!("done"), &json!(2100)), "{job}");
        assert_eq!((&job["result"]["peakMb"], &job["result"]["rtf"]), (&json!(2100), &json!(1.2)), "{job}");
        job["result"]["voiceId"].as_str().unwrap().to_string()
    }

    #[tokio::test]
    async fn takes_a_voice_again_for_another_engine_and_a_characters_voice_from_its_reference() {
        use std::os::unix::fs::PermissionsExt;
        let lab = lab().await;
        let id = imprinted(&lab, "Мой голос").await;
        let reimprint_as = |body: Value| reimprint(state(&lab), admin(&lab), Path(id.clone()), Body(body));
        let (status, message) = answer(reimprint_as(json!({ "engine": "cosyvoice3" })).await);
        assert_eq!((status, message), (StatusCode::BAD_REQUEST, json!("Сначала установите CosyVoice 3.")));
        let (status, _) = answer(reimprint_as(json!({ "engine": "omnivoice" })).await);
        assert_eq!(status, StatusCode::BAD_REQUEST, "the engine it has already");
        // CosyVoice 3 installed (a Python that only pretends, as OmniVoice's).
        let runtime = lab.app.config.voice_lab.runtime_dir.join("cosyvoice3");
        std::fs::create_dir_all(runtime.join("env/bin")).unwrap();
        let python = runtime.join("env/bin/python");
        std::fs::write(&python, crate::voice_lab::service::testing::FAKE_PYTHON).unwrap();
        std::fs::set_permissions(&python, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(runtime.join("ready.json"), r#"{"engine":"cosyvoice3","device":"cpu","torch":"2.3.1","source":"bench","installedAt":"2026-11-07T00:00:00Z"}"#).unwrap();
        let (status, job) = answer(reimprint_as(json!({ "engine": "cosyvoice3" })).await);
        assert_eq!(status, StatusCode::CREATED, "{job}");
        let job = finished_job(&lab).await;
        assert_eq!(job["state"], "done", "{job}");
        let again = job["result"]["voiceId"].as_str().unwrap();
        let store = &lab.app.voice_lab.store;
        let voice = store.voice(again).unwrap();
        assert_eq!((voice.engine.as_str(), voice.name.as_str(), voice.takes), ("cosyvoice3", "Мой голос", 1));
        assert!(store.voice(&id).is_some(), "the old one stays");

        // A character's voice from its package's reference and words: the id it is given, its origin noted.
        let reference = store.voices_dir.join(&id).join("ref.wav");
        let mut meta = serde_json::Map::new();
        meta.insert("origin".into(), json!("package"));
        meta.insert("package".into(), json!("arthur"));
        let started = lab
            .app
            .voice_lab
            .imprint_reference(
                "c-arthur-arthur-omnivoice",
                "omnivoice",
                &reference,
                "Чай подан, сэр.",
                "Артур",
                meta,
                false,
                None,
            )
            .await;
        assert!(started.is_ok(), "{:?}", started.err());
        assert_eq!(finished_job(&lab).await["state"], "done");
        let character = store.voice("c-arthur-arthur-omnivoice").unwrap();
        assert_eq!((character.origin.as_deref(), character.package.as_deref()), (Some("package"), Some("arthur")));
        let twice = lab
            .app
            .voice_lab
            .imprint_reference(
                "c-arthur-arthur-omnivoice",
                "omnivoice",
                &reference,
                "Чай подан.",
                "Артур",
                serde_json::Map::new(),
                false,
                None,
            )
            .await;
        assert_eq!(twice.unwrap_err().status, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn sees_this_computer_and_its_engines() {
        let lab = lab().await;
        let view = view_of(&lab).await;
        assert_eq!(view["available"], true);
        assert_eq!(view["platform"]["summary"], "Мак, Apple M4 Max, 36 ГБ памяти");
        assert_eq!((&view["platform"]["os"], &view["platform"]["gpu"]), (&json!("darwin"), &json!("apple")));
        let engines: Vec<(Value, Value, Value)> = view["engines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|engine| (engine["id"].clone(), engine["status"].clone(), engine["device"].clone()))
            .collect();
        assert_eq!(
            engines,
            vec![
                (json!("omnivoice"), json!("ready"), json!("mps")),
                (json!("cosyvoice3"), json!("missing"), json!("cpu")),
                (json!("voxcpm"), json!("missing"), json!("mps"))
            ]
        );
        assert_eq!(view["passages"].as_array().unwrap().len(), 5);
        assert_eq!(
            view["memory"],
            json!({ "totalMb": 36_864, "poolMb": 16_384, "reserveMb": 2_212, "pressure": "normal" })
        );
        // The voice's server says what it holds: its next start is judged by its peak, not by the engine's guess.
        let server = &view["servers"]["omnivoice"];
        assert_eq!(
            (&server["up"], &server["memoryMb"], &server["peakMb"]),
            (&json!(true), &json!(2_600), &json!(3_000))
        );
        assert_eq!(view["engines"][0]["needMb"], json!({ "imprint": 3_500, "serve": 3_450 }));
    }

    #[tokio::test]
    async fn records_a_take_plays_it_back_refuses_what_is_not_a_wav() {
        let lab = lab().await;
        let (status, added) = add(&lab, Some("passage=2"), spoken_take()).await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!((&added["n"], &added["passage"], &added["verdict"]), (&json!(1), &json!(2), &json!("good")));
        let audio = take_wav(&lab, "1").await.unwrap();
        assert_eq!(audio.status(), StatusCode::OK);
        assert_eq!(audio.headers()[header::CONTENT_TYPE], "audio/wav");
        assert_eq!(audio.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(add(&lab, None, b"ID3...".to_vec()).await.0, StatusCode::BAD_REQUEST);
        assert_eq!(answer(clear_draft(state(&lab), admin(&lab)).await), (StatusCode::OK, json!({ "cleared": true })));
        assert_eq!(view_of(&lab).await["draft"], json!({ "takes": [], "seconds": 0 }));
    }

    async fn take_wav(lab: &Lab, n: &str) -> Result<Response, ApiError> {
        take(state(lab), admin(lab), Path(n.to_string())).await
    }

    #[tokio::test]
    async fn takes_an_imprint_the_takes_go_with_the_voice_its_sample_plays_it_can_be_renamed_and_deleted() {
        let lab = lab().await;
        let id = imprinted(&lab, "Тестовый").await;
        let view = view_of(&lab).await;
        assert_eq!(view["draft"]["takes"], json!([]));
        let voices = view["voices"].as_array().unwrap();
        assert_eq!(voices.len(), 1);
        for (field, value) in [
            ("id", json!(id)),
            ("name", json!("Тестовый")),
            ("engine", json!("omnivoice")),
            ("takes", json!(1)),
            ("chosen", json!(false)),
            ("refSeconds", json!(6)),
        ] {
            assert_eq!(voices[0][field], value, "{field}");
        }
        let played = sample(state(&lab), admin(&lab), Path((id.clone(), "1".into()))).await.unwrap();
        assert_eq!(played.status(), StatusCode::OK);
        let missing = sample(state(&lab), admin(&lab), Path((id.clone(), "7".into()))).await.unwrap_err();
        assert_eq!(missing.status, StatusCode::NOT_FOUND);
        let (status, renamed) =
            answer(rename(state(&lab), admin(&lab), Path(id.clone()), Body(json!({ "name": "Второй голос" }))).await);
        assert_eq!((status, &renamed["name"]), (StatusCode::OK, &json!("Второй голос")));
        assert_eq!(
            answer(remove(state(&lab), admin(&lab), Path(id)).await),
            (StatusCode::OK, json!({ "deleted": true }))
        );
        assert_eq!(view_of(&lab).await["voices"], json!([]));
    }

    /// What the models ask of the voice lab (the cards, a chosen voice's route), and what it answers once the
    /// models chose `own:<id>` through the voice (src/links). The whole path over HTTP is tests/voice_lab.rs.
    #[tokio::test]
    async fn chooses_an_own_voice_through_the_models_the_gateway_speaks_through_its_server_and_keeps_it_from_deletion()
    {
        let lab = lab().await;
        let id = imprinted(&lab, "Мой голос").await;
        let cards = serde_json::to_value(lab.app.voice_lab.voices_for_cards().await).unwrap();
        for (field, value) in [
            ("id", json!(id)),
            ("engine", json!("omnivoice")),
            ("name", json!("Мой голос")),
            ("serving", json!(true)),
            ("ready", json!(true)),
        ] {
            assert_eq!(cards[0][field], value, "{field}");
        }
        assert_eq!(cards[0]["samples"].as_array().unwrap().len(), 1);
        let prepared = lab.app.voice_lab.prepare(&id, false).await.unwrap();
        assert_eq!(prepared.route.base_url(), lab.clone_url, "it speaks through its own server");
        let activated = lab.app.models.activate_tts(&json!({ "modelId": format!("own:{id}") }), None).await.unwrap();
        assert!(!activated.needs_restart);
        assert_eq!(
            lab.app.voice.own_choice().map(|pair| (pair.model, pair.voice)),
            Some(("omnivoice".into(), id.clone()))
        );
        assert_eq!(lab.app.voice.own_speech().unwrap().base_url(), lab.clone_url);
        assert_eq!(view_of(&lab).await["voices"][0]["chosen"], true);
        let (status, _) = answer(remove(state(&lab), admin(&lab), Path(id)).await);
        assert_eq!(status, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn only_warns_when_memory_is_tight_or_the_mac_is_short_and_starts_on_the_owners_vse_ravno() {
        let lab = lab().await;
        // Measured once (2.1 GB), the imprint needs 2.4 GB with its margin; its ceiling is half again above.
        imprinted(&lab, "Мой голос").await;
        assert_eq!(add(&lab, None, spoken_take()).await.0, StatusCode::CREATED);
        lab.memory.free_mb.store(4_000, Ordering::SeqCst);
        let warned = imprint_as(&lab, json!({ "engine": "omnivoice" })).await;
        assert_eq!(
            warned,
            (
                StatusCode::PRECONDITION_REQUIRED,
                json!("Памяти впритык: свободно 3,9 ГБ, нужно около 2,4 ГБ, а 2,2 ГБ держу для колонки — она может подтормаживать.")
            )
        );
        lab.memory.pressure_level.store(2, Ordering::SeqCst);
        let pressed = imprint_as(&lab, json!({ "engine": "omnivoice" })).await;
        assert_eq!(pressed.0, StatusCode::PRECONDITION_REQUIRED, "a pressed computer is a warning, not a refusal");
        assert!(pressed.1.as_str().unwrap().contains("не хватает памяти"), "{}", pressed.1);
        lab.memory.pressure_level.store(1, Ordering::SeqCst);
        assert_eq!(imprint_as(&lab, json!({ "engine": "omnivoice", "anyway": true })).await.0, StatusCode::CREATED);
        let job = finished_job(&lab).await;
        assert_eq!((&job["state"], &job["limitMb"]), (&json!("done"), &json!(3_623)));
        lab.memory.free_mb.store(16_384, Ordering::SeqCst);
        assert_eq!(answer(stop(state(&lab), admin(&lab)).await), (StatusCode::CREATED, json!({ "stopped": false })));
    }

    #[tokio::test]
    async fn lets_everyone_who_sees_the_station_look_and_only_managers_change() {
        let lab = lab().await;
        let nobody = || Caller::local(lab.app.clone(), None);
        assert_eq!(answer(view(state(&lab), nobody()).await).0, StatusCode::UNAUTHORIZED);
        let anonymous = answer(install(state(&lab), nobody(), Path("cosyvoice3".into())).await);
        assert_eq!(anonymous.0, StatusCode::UNAUTHORIZED);
        let member = || Caller::local(lab.app.clone(), Some(user_principal("m", "Участник", Role::Member, None)));
        assert_eq!(answer(view(state(&lab), member()).await).0, StatusCode::OK);
        let (status, message) = answer(install(state(&lab), member(), Path("cosyvoice3".into())).await);
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(message.as_str().unwrap().starts_with("Нет права:"), "{message}");
    }

    #[tokio::test]
    async fn takes_arrive_only_from_the_home_network_and_no_larger_than_16_mb() {
        let lab = lab().await;
        let mut from_outside = admin(&lab);
        from_outside.zone = Zone::Remote;
        let refused = add_take(state(&lab), from_outside, RawQuery(None), RawBody::from(spoken_take())).await;
        assert_eq!(answer(refused), (StatusCode::FORBIDDEN, json!("Только из домашней сети.")));
        let large = read_body(RawBody::from(vec![0u8; MAX_TAKE_BYTES + 1]), MAX_TAKE_BYTES).await.unwrap_err();
        assert_eq!((large.status, large.message.as_str()), (StatusCode::BAD_REQUEST, "Запись больше 16 МБ."));
        // A phone gone mid-take without a word: the upload stands still, and the request lets it go.
        let first = Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"RIFF"));
        let stalled = RawBody::from_stream(futures::stream::iter([first]).chain(futures::stream::pending()));
        let gone = read_body_within(stalled, MAX_TAKE_BYTES, Duration::from_millis(50)).await.unwrap_err();
        assert_eq!(
            (gone.status, gone.message.as_str()),
            (StatusCode::REQUEST_TIMEOUT, "Запись не дошла: связь прервалась.")
        );
        assert_eq!(passage(Some("passage=3")), Some(3));
        assert_eq!(passage(Some("passage=2.5")), None);
        assert_eq!(passage(Some("passage=1&passage=2")), None);
        assert_eq!(passage(None), None);
        let (status, message) = imprint_as(&lab, json!({ "name": "Мой голос" })).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(message, json!(r#"body: {"engine": "omnivoice", "name": "Мой голос"}"#));
    }
}
