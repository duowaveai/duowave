//! The panel's desk per person (docs/plans/desk.md): which widgets are on it, where and how wide, the zoom and the
//! view, whether it is pinned — kept on the gateway, so another browser or computer shows the same desk. Nothing kept
//! — the panel lays out its starting desk. A desk per role of the panel: the gateway's white one, the helper's dark
//! one. The panel owns what is inside (shell/widgets.ts); the gateway keeps it whole, up to 64 KB.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::Router;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};

use crate::app::App;
use crate::core::identity::principal::PrincipalKind;
use crate::platform::http::{ok, ApiError, ApiResult, Body, Caller};
use crate::utils::time::now_iso;

/// Its rows in the `state` table: `desk`, `<role>:<person>`.
const KIND: &str = "desk";
/// A desk bigger than this is not a layout.
const MAX_BYTES: usize = 64 * 1024;

pub fn routes() -> Router<Arc<App>> {
    Router::new().route("/api/v1/desk/{role}", get(read).put(write).delete(reset))
}

/// The row of the caller's desk for the role: a person's (or the household's, without people), never a key's.
fn row_of(caller: &Caller, role: &str) -> Result<String, ApiError> {
    if role != "gateway" && role != "helper" {
        return Err(ApiError::not_found("Стол бывает у шлюза (gateway) и у помощника (helper)."));
    }
    let principal = caller.signed_in()?;
    match principal.kind {
        PrincipalKind::User | PrincipalKind::Household => Ok(format!("{role}:{}", principal.id)),
        PrincipalKind::ApiKey | PrincipalKind::System => {
            Err(ApiError::forbidden("Стол бывает у людей дома, у ключа его нет."))
        }
    }
}

/// The desk as it was kept, and when; `layout: null` — the panel lays out its starting desk.
async fn read(State(app): State<Arc<App>>, caller: Caller, Path(role): Path<String>) -> ApiResult<Value> {
    let row = row_of(&caller, &role)?;
    let kept: Option<(String, String)> = app.db.with(|c| {
        c.query_row(
            "SELECT items, updated_at FROM state WHERE kind = ?1 AND device = ?2",
            params![KIND, row],
            |found| Ok((found.get(0)?, found.get(1)?)),
        )
        .optional()
    })?;
    let (layout, at) = match kept {
        Some((items, at)) => {
            (serde_json::from_str::<Vec<Value>>(&items).ok().and_then(|mut items| items.pop()), Some(at))
        }
        None => (None, None),
    };
    Ok(ok(json!({ "layout": layout, "at": at })))
}

/// The desk, whole: what the panel has now.
async fn write(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(role): Path<String>,
    Body(layout): Body<Value>,
) -> ApiResult<Value> {
    let row = row_of(&caller, &role)?;
    if !layout.is_object() {
        return Err(ApiError::bad_request("Стол — объект JSON."));
    }
    // A list of one, as the table keeps its items.
    let items = serde_json::to_string(&[layout])?;
    if items.len() > MAX_BYTES {
        return Err(ApiError::bad_request("Стол больше 64 КБ — так много виджетов не бывает."));
    }
    let at = now_iso();
    app.db.with(|c| {
        c.execute(
            "INSERT INTO state (kind, device, items, updated_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (kind, device) DO UPDATE SET items = excluded.items, updated_at = excluded.updated_at",
            params![KIND, row, items, at],
        )
    })?;
    Ok(ok(json!({ "at": at })))
}

/// «Расставить заново» from the start: the kept desk is gone, the panel lays out its starting one.
async fn reset(State(app): State<Arc<App>>, caller: Caller, Path(role): Path<String>) -> Result<StatusCode, ApiError> {
    let row = row_of(&caller, &role)?;
    app.db.with(|c| c.execute("DELETE FROM state WHERE kind = ?1 AND device = ?2", params![KIND, row]))?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Env};
    use crate::core::identity::principal::{api_key_principal, household_principal, user_principal};
    use crate::core::identity::Role;

    #[tokio::test]
    async fn keeps_each_persons_desk_per_role_whole_and_forgets_it_on_reset() {
        let dir = tempfile::tempdir().unwrap();
        let text = |sub: &str| dir.path().join(sub).to_string_lossy().into_owned();
        let env = Env::from_pairs([
            ("STATE_DIR", String::new()),
            ("PACKAGES_DIR", text("packages")),
            ("DATA_DIR", text("data")),
            ("VOICE_LAB_DIR", text("voice-lab")),
            ("TTS_MODELS_DIR", text("voices")),
        ]);
        let app = App::build(Config::for_tests(&env)).unwrap();
        let anya = || Caller::local(app.clone(), Some(user_principal("u1", "Аня", Role::Member, None)));
        let borya = || Caller::local(app.clone(), Some(user_principal("u2", "Боря", Role::Admin, None)));
        let read_of = |caller: Caller, role: &str| read(State(app.clone()), caller, Path(role.into()));

        // Nothing kept: the panel lays out its starting desk.
        assert_eq!(read_of(anya(), "gateway").await.unwrap().1, json!({ "layout": null, "at": null }));

        let desk = json!({ "v": 1, "hidden": ["core.recent"], "places": { "core.mode": { "x": 10, "y": 20 } }, "locked": true });
        let saved = write(State(app.clone()), anya(), Path("gateway".into()), Body(desk.clone())).await.unwrap().1;
        assert!(saved["at"].is_string());
        let kept = read_of(anya(), "gateway").await.unwrap().1;
        assert_eq!((kept["layout"].clone(), kept["at"].clone()), (desk.clone(), saved["at"].clone()));
        // Another person, the other role: their own desks.
        assert_eq!(read_of(borya(), "gateway").await.unwrap().1["layout"], Value::Null);
        assert_eq!(read_of(anya(), "helper").await.unwrap().1["layout"], Value::Null);
        // The household, without people, keeps one too.
        let household = Caller::local(app.clone(), Some(household_principal(Role::Admin, None)));
        write(State(app.clone()), household, Path("helper".into()), Body(json!({ "v": 1 }))).await.unwrap();

        // A key has no desk; an unknown role, a list or a huge desk are refused.
        let key = Caller::local(app.clone(), Some(api_key_principal("k1", "Скрипт", &["station.view".into()])));
        assert_eq!(read_of(key, "gateway").await.err().unwrap().status, StatusCode::FORBIDDEN);
        assert_eq!(read_of(anya(), "kitchen").await.err().unwrap().status, StatusCode::NOT_FOUND);
        let list = write(State(app.clone()), anya(), Path("gateway".into()), Body(json!([1, 2]))).await;
        assert_eq!(list.err().unwrap().status, StatusCode::BAD_REQUEST);
        let huge = json!({ "v": 1, "note": "щ".repeat(40_000) });
        let huge = write(State(app.clone()), anya(), Path("gateway".into()), Body(huge)).await;
        assert_eq!(huge.err().unwrap().status, StatusCode::BAD_REQUEST);
        assert_eq!(read_of(anya(), "gateway").await.unwrap().1["layout"], desk, "a refused write keeps the desk");

        // Reset: back to the starting desk.
        let reset = reset(State(app.clone()), anya(), Path("gateway".into())).await.unwrap();
        assert_eq!(reset, StatusCode::NO_CONTENT);
        assert_eq!(read_of(anya(), "gateway").await.unwrap().1["layout"], Value::Null);
        // Without signing in — nothing.
        let nobody = Caller::local(app.clone(), None);
        assert_eq!(read_of(nobody, "gateway").await.err().unwrap().status, StatusCode::UNAUTHORIZED);
    }
}
