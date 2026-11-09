//! Характер Алисы для пульта: карточки и говорящий текст видит всякий, кто видит колонку; меняет — тот, у кого право
//! persona.edit. Сказывается со следующего вопроса: промт читается на каждый ответ.

use std::sync::Arc;

use axum::extract::State;
use axum::routing::get;
use axum::Router;
use serde_json::{Map, Value};

use super::persona_service::PersonaView;
use crate::app::App;
use crate::core::identity::permissions::{PERSONA_EDIT, STATION_VIEW};
use crate::platform::http::{created, ok, ApiResult, Body, Caller};

pub fn routes() -> Router<Arc<App>> {
    Router::new().route("/api/v1/persona", get(view).put(put).post(post))
}

fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

async fn view(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<PersonaView> {
    caller.permitted(STATION_VIEW)?;
    Ok(ok(app.personas.view()?))
}

async fn put(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<Value>) -> ApiResult<PersonaView> {
    caller.permitted(PERSONA_EDIT)?;
    app.personas.set(&object(body), caller.actor())?;
    Ok(ok(app.personas.view()?))
}

/// The same as PUT; a POST answered 201, as Nest's did.
async fn post(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<Value>) -> ApiResult<PersonaView> {
    caller.permitted(PERSONA_EDIT)?;
    app.personas.set(&object(body), caller.actor())?;
    Ok(created(app.personas.view()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Env};
    use crate::core::identity::principal::user_principal;
    use crate::core::identity::Role;
    use axum::http::StatusCode;
    use serde_json::json;

    #[tokio::test]
    async fn shows_the_character_to_the_household_and_lets_only_who_may_change_it() {
        let app = App::build(Config::for_tests(&Env::from_pairs([("STATE_DIR", "")]))).unwrap();
        let member = || Caller::local(app.clone(), Some(user_principal("u2", "Аня", Role::Member, None)));
        let admin = || Caller::local(app.clone(), Some(user_principal("u1", "Админ", Role::Admin, None)));
        let shown = view(State(app.clone()), member()).await.unwrap();
        assert_eq!(shown.1.active_preset.as_deref(), Some("alice"));
        let refused = put(State(app.clone()), member(), Body(json!({"presetId": "alice"}))).await.err().unwrap();
        assert_eq!(refused.status, StatusCode::FORBIDDEN);
        let changed = put(State(app.clone()), admin(), Body(json!({"presetId": "alice"}))).await.unwrap();
        assert_eq!((changed.0, changed.1.active_preset.as_deref()), (StatusCode::OK, Some("alice")));
        // An older panel's card is Alisa in its positions.
        let posted = post(State(app.clone()), admin(), Body(json!({"presetId": "irony"}))).await.unwrap();
        assert_eq!((posted.0, posted.1.active_preset.as_deref()), (StatusCode::CREATED, Some("alice")));
        assert!(posted.1.persona.contains("лёгкая ирония"));
        let wrong = put(State(app.clone()), admin(), Body(json!({"text": "Мало"}))).await.err().unwrap();
        assert_eq!(wrong.status, StatusCode::BAD_REQUEST);
    }
}
