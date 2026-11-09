//! `/api/v1/characters`: the characters for the panel — whoever sees the station sees them; choosing one is
//! `persona.edit`, and its voice besides `models.manage`. Installing and removing a character is its package's
//! (`/api/v1/packages`).

use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, Path, State};
use axum::routing::{get, post};
use axum::Router;

use super::{CharactersView, ChooseAnswer, ChooseBody};
use crate::app::App;
use crate::core::identity::permissions::{MODELS_MANAGE, PERSONA_EDIT, STATION_VIEW};
use crate::platform::http::{ok, ApiResult, Body, Caller};

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/v1/characters", get(view))
        .route("/api/v1/characters/{id}/choose", post(choose))
        .layer(DefaultBodyLimit::max(16 * 1024))
}

async fn view(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<CharactersView> {
    caller.permitted(STATION_VIEW)?;
    Ok(ok(app.characters.view().await?))
}

/// `{tones, install, anyway, voice}`: the character at once; its voice unless `voice` is false — 200 with what became
/// of it (`needs` when its engine is not here: the panel asks and repeats with `install`), 428 on tight memory as for
/// any voice (`anyway` starts it).
async fn choose(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<ChooseBody>,
) -> ApiResult<ChooseAnswer> {
    caller.permitted(PERSONA_EDIT)?;
    let voice_allowed = caller.permitted(MODELS_MANAGE).is_ok();
    Ok(ok(app.characters.choose(&id, body, voice_allowed, caller.actor()).await?))
}
