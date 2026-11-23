//! Характер Алисы для пульта: карточки и говорящий текст видит всякий, кто видит колонку; меняет — тот, у кого право
//! persona.edit. Сказывается со следующего вопроса: промт читается на каждый ответ.

use std::sync::Arc;

use axum::routing::get;
use axum::Router;
use serde_json::{Map, Value};

use super::persona_service::PersonaView;
use crate::core::identity::permissions::{PERSONA_EDIT, STATION_VIEW};
use crate::dialog::PersonaService;
use crate::platform::http::{created, ok, ApiResult, Body, Caller, Uses};

pub fn routes<S: crate::dialog::RouteNeeds>() -> Router<S> {
    Router::new().route("/api/v1/persona", get(view).put(put).post(post))
}

fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

async fn view(Uses(personas): Uses<Arc<PersonaService>>, caller: Caller) -> ApiResult<PersonaView> {
    caller.permitted(STATION_VIEW)?;
    Ok(ok(personas.view()?))
}

async fn put(
    Uses(personas): Uses<Arc<PersonaService>>,
    caller: Caller,
    Body(body): Body<Value>,
) -> ApiResult<PersonaView> {
    caller.permitted(PERSONA_EDIT)?;
    personas.set(&object(body), caller.actor())?;
    Ok(ok(personas.view()?))
}

/// The same as PUT; a POST answered 201, as Nest's did.
async fn post(
    Uses(personas): Uses<Arc<PersonaService>>,
    caller: Caller,
    Body(body): Body<Value>,
) -> ApiResult<PersonaView> {
    caller.permitted(PERSONA_EDIT)?;
    personas.set(&object(body), caller.actor())?;
    Ok(created(personas.view()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
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
}
