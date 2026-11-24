//! The computer's part of whole_tests/home_character.rs: what only a computer has, tested with the whole gateway built.

use axum::http::StatusCode;
use serde_json::{json, Value};

use crate::whole_tests::home_character::*;

/// A computer as the home's centre: the voice waiting on it is its own, answered here and not passed on to its Турбо
/// (the rest of the scenario: whole_tests/home_character.rs, the centre and the voice waiting on its Турбо).
#[tokio::test]
async fn a_computer_that_is_the_centre_answers_its_own_waiting_voice_itself() {
    const PENDING: &str = "/api/v1/home/voice/pending";
    let dir = tempfile::tempdir().unwrap();
    let (app, turbo) = centre(dir.path(), None).await;
    let (viewer, owner) = (key(&app, &["station.view"]), key(&app, &["station.view", "models.manage"]));
    use crate::settings::definitions::{PendingWait, VoicePending, VOICE_PENDING};
    let own = VoicePending {
        model_id: "piper-denis".into(),
        voice_id: None,
        wait_for: PendingWait { kind: "file".into(), id: "piper-denis".into() },
        anyway: false,
        state: "failed".into(),
        reason: Some("файл голоса не скачался".into()),
        memory: None,
        recipe: None,
        since: "2026-11-19T10:00:00Z".into(),
    };
    app.settings.set(&VOICE_PENDING, &own, None).unwrap();
    let (_, view) = send(&app, "GET", PENDING, &viewer, Value::Null).await;
    assert_eq!((&view["turbo"], &view["pending"]["modelId"]), (&Value::Null, &json!("piper-denis")), "{view}");
    assert_eq!(view["pending"]["reason"], "файл голоса не скачался");
    let (status, view) = send(&app, "POST", PENDING, &owner, json!({ "answer": "cancel" })).await;
    assert_eq!((status, &view["pending"]), (StatusCode::OK, &Value::Null), "{view}");
    assert!(app.pc.models.pending().is_none(), "forgotten here");
    assert!(turbo.answers.lock().unwrap().is_empty(), "the centre's own voice is not the Турбо's");
}
