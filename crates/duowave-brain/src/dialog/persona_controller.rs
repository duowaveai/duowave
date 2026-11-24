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

pub async fn view(Uses(personas): Uses<Arc<PersonaService>>, caller: Caller) -> ApiResult<PersonaView> {
    caller.permitted(STATION_VIEW)?;
    Ok(ok(personas.view()?))
}

pub async fn put(
    Uses(personas): Uses<Arc<PersonaService>>,
    caller: Caller,
    Body(body): Body<Value>,
) -> ApiResult<PersonaView> {
    caller.permitted(PERSONA_EDIT)?;
    personas.set(&object(body), caller.actor())?;
    Ok(ok(personas.view()?))
}

/// The same as PUT; a POST answered 201, as Nest's did.
pub async fn post(
    Uses(personas): Uses<Arc<PersonaService>>,
    caller: Caller,
    Body(body): Body<Value>,
) -> ApiResult<PersonaView> {
    caller.permitted(PERSONA_EDIT)?;
    personas.set(&object(body), caller.actor())?;
    Ok(created(personas.view()?))
}
