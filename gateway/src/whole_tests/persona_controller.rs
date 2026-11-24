//! The tests of `dialog/persona_controller.rs` that need the whole gateway (`App`): its crate does not have it, the
//! program does.

use axum::http::StatusCode;
use serde_json::json;

use crate::app::App;
use crate::config::{Config, Env};
use crate::core::identity::principal::user_principal;
use crate::core::identity::Role;
use crate::dialog::persona_controller::*;
use crate::platform::http::{Body, Caller, Uses};

#[tokio::test]
async fn shows_the_character_to_the_household_and_lets_only_who_may_change_it() {
    let app = App::build(Config::for_tests(&Env::from_pairs([("STATE_DIR", "")]))).unwrap();
    let member = || Caller::local(app.clone(), Some(user_principal("u2", "Аня", Role::Member, None)));
    let admin = || Caller::local(app.clone(), Some(user_principal("u1", "Админ", Role::Admin, None)));
    let shown = view(Uses::of(&app), member()).await.unwrap();
    assert_eq!(shown.1.active_preset.as_deref(), Some("alice"));
    let refused = put(Uses::of(&app), member(), Body(json!({"presetId": "alice"}))).await.err().unwrap();
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    let changed = put(Uses::of(&app), admin(), Body(json!({"presetId": "alice"}))).await.unwrap();
    assert_eq!((changed.0, changed.1.active_preset.as_deref()), (StatusCode::OK, Some("alice")));
    // An older panel's card is Alisa in its positions.
    let posted = post(Uses::of(&app), admin(), Body(json!({"presetId": "irony"}))).await.unwrap();
    assert_eq!((posted.0, posted.1.active_preset.as_deref()), (StatusCode::CREATED, Some("alice")));
    assert!(posted.1.persona.contains("лёгкая ирония"));
    let wrong = put(Uses::of(&app), admin(), Body(json!({"text": "Мало"}))).await.err().unwrap();
    assert_eq!(wrong.status, StatusCode::BAD_REQUEST);
}
