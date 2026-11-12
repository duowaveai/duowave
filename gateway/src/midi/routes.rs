//! `/api/v1/midi` (docs/plans/midi.md, «API»): the keyboards and who listens, the channel of notes' ticket and socket,
//! a `.mid` read into tracks. Whoever may use the apps (`use:plugins`), as the apps' own calls from the panel.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Query, State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use serde_json::json;

use super::file::{SongView, FILE_MAX};
use super::socket::TICKET_LIFETIME;
use super::MidiView;
use crate::app::App;
use crate::core::identity::permissions::use_of;
use crate::platform::http::{created, ok, ApiError, ApiResult, Body, Caller};
use crate::skills::capabilities::CapabilityName;

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/v1/midi", get(view))
        .route("/api/v1/midi/ticket", post(ticket))
        .route("/api/v1/midi/ws", get(socket))
        .route("/api/v1/midi/read", post(read).layer(DefaultBodyLimit::max(FILE_MAX + 1024)))
}

fn may_use(caller: &Caller) -> Result<(), ApiError> {
    caller.permitted(&use_of(CapabilityName::Plugins)).map(|_| ())
}

/// The keyboards plugged in, read now, and who listens; why there is no MIDI here, if there is none.
async fn view(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<MidiView> {
    may_use(&caller)?;
    let midi = app.midi.clone();
    let view =
        tokio::task::spawn_blocking(move || midi.view()).await.map_err(|_| ApiError::internal("MIDI не прочитался"))?;
    Ok(ok(view))
}

#[derive(Debug, Deserialize)]
struct TicketBody {
    package: String,
}

/// A ticket for the channel of notes of one app's page.
async fn ticket(
    State(app): State<Arc<App>>,
    caller: Caller,
    Body(body): Body<TicketBody>,
) -> ApiResult<serde_json::Value> {
    may_use(&caller)?;
    app.midi.may_hear(&body.package, caller.zone).map_err(ApiError::forbidden)?;
    let ticket = app.midi.tickets.issue(&body.package, caller.zone);
    Ok(created(json!({ "ticket": ticket, "expiresIn": TICKET_LIFETIME.as_secs() })))
}

#[derive(Debug, Deserialize)]
struct SocketQuery {
    #[serde(default)]
    ticket: Option<String>,
}

/// The channel of notes, by a ticket; the page is still at home and the app may still hear.
async fn socket(
    State(app): State<Arc<App>>,
    caller: Caller,
    Query(query): Query<SocketQuery>,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let (package, _) = query
        .ticket
        .as_deref()
        .and_then(|ticket| app.midi.tickets.take(ticket))
        .ok_or_else(|| ApiError::unauthorized("Билет канала нот не подошёл: возьмите новый."))?;
    app.midi.may_hear(&package, caller.zone).map_err(ApiError::forbidden)?;
    Ok(super::socket::upgrade(upgrade, app.midi.clone(), package, caller.zone))
}

/// A `.mid` read into tracks with their melodies; the file is kept nowhere.
async fn read(State(_app): State<Arc<App>>, caller: Caller, bytes: Bytes) -> ApiResult<SongView> {
    may_use(&caller)?;
    let reading = tokio::task::spawn_blocking(move || super::file::read(&bytes));
    match tokio::time::timeout(Duration::from_secs(1), reading).await {
        Ok(Ok(Ok(song))) => Ok(ok(song)),
        Ok(Ok(Err(why))) => Err(ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, why)),
        _ => Err(ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "Файл не прочитался за секунду.")),
    }
}
