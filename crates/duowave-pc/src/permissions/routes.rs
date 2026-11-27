//! `GET /api/v1/permissions` — this computer's items now; `POST …/{item}/ask` — the system's own question (macOS);
//! `POST …/{item}/open` — its settings page, on a Mac with the guide window beside it; `POST …/{item}/guide` — the
//! guide window shown or closed (its own page closes it once the switch is on).
//!
//! The buttons act on this computer's screen, so they answer only a caller on this computer (loopback): from a phone
//! the system's windows would come up where nobody looks.

use axum::extract::Path;
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;

use super::{Item, State, View};
use crate::core::identity::permissions::{SETTINGS_EDIT, STATION_VIEW};
use crate::platform::http::{ok, ApiError, ApiResult, Body, Caller, RouteState};

pub fn routes<S: RouteState>() -> Router<S> {
    Router::new()
        .route("/api/v1/permissions", get(read))
        .route("/api/v1/permissions/{item}/ask", post(ask))
        .route("/api/v1/permissions/{item}/open", post(open))
        .route("/api/v1/permissions/{item}/guide", post(guide))
}

/// The desktop app runs this gateway (`DUOWAVE_MANAGED=desktop`, set by the app before the engine starts).
pub fn desktop() -> bool {
    std::env::var("DUOWAVE_MANAGED").is_ok_and(|managed| managed == "desktop")
}

fn here(caller: &Caller) -> bool {
    caller.peer.is_some_and(|peer| peer.ip().is_loopback())
}

async fn view(caller: &Caller) -> View {
    let items = super::rows(desktop()).await;
    // An item allowed meanwhile: its guide window has nothing more to say.
    if let Some(port) = super::port() {
        for row in items.iter().filter(|row| row.state == State::Granted) {
            port.guide(row.item, false);
        }
    }
    View { os: super::os(), app: desktop(), here: here(caller), items }
}

async fn read(caller: Caller) -> ApiResult<View> {
    caller.permitted(STATION_VIEW)?;
    Ok(ok(view(&caller).await))
}

fn item_of(name: &str) -> Result<Item, ApiError> {
    Item::parse(name).ok_or_else(|| ApiError::not_found(crate::tr!("Такого разрешения нет.")))
}

/// Only the person at this computer, in its desktop app.
fn at_this_computer(caller: &Caller) -> Result<(), ApiError> {
    if !desktop() {
        return Err(ApiError::conflict(crate::tr!("Разрешения нужны только приложению DuoWave на компьютере.")));
    }
    if !here(caller) {
        return Err(ApiError::conflict(crate::tr!(
            "Откройте DuoWave на этом компьютере: окна системы появятся на его экране."
        )));
    }
    Ok(())
}

/// The system's own question: the home network (one send brings macOS' window, and the waiting announcer goes on) or
/// the microphone (the app asks; the answer comes later, the panel sees it in the list).
async fn ask(caller: Caller, Path(name): Path<String>) -> ApiResult<View> {
    caller.permitted(SETTINGS_EDIT)?;
    at_this_computer(&caller)?;
    match item_of(&name)? {
        Item::LocalNetwork => {
            super::network_asked();
            let _ = super::local_network().await;
        }
        Item::Microphone => {
            let port = super::port()
                .ok_or_else(|| ApiError::conflict(crate::tr!("Приложение не умеет спросить микрофон.")))?;
            tokio::spawn(port.ask_microphone());
        }
        Item::Firewall | Item::NetworkProfile => {
            return Err(ApiError::bad_request(crate::tr!("Это разрешение не спрашивается: откройте его настройки.")));
        }
    }
    Ok(ok(view(&caller).await))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct OpenInput {
    /// On a Mac: the guide window beside System Settings.
    guide: bool,
}

async fn open(caller: Caller, Path(name): Path<String>, Body(input): Body<OpenInput>) -> ApiResult<View> {
    caller.permitted(SETTINGS_EDIT)?;
    at_this_computer(&caller)?;
    let item = item_of(&name)?;
    super::open_settings(item).map_err(ApiError::conflict)?;
    if input.guide && cfg!(target_os = "macos") {
        if let Some(port) = super::port() {
            port.guide(item, true);
        }
    }
    Ok(ok(view(&caller).await))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GuideInput {
    show: bool,
}

async fn guide(
    caller: Caller,
    Path(name): Path<String>,
    Body(input): Body<GuideInput>,
) -> ApiResult<serde_json::Value> {
    caller.permitted(SETTINGS_EDIT)?;
    at_this_computer(&caller)?;
    let item = item_of(&name)?;
    if let Some(port) = super::port() {
        port.guide(item, input.show);
    }
    Ok(ok(serde_json::json!({ "shown": input.show })))
}
