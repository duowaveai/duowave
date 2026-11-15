//! What the panel remembers about a person (docs/plans/menu-scheme.md): where «Папки» were left open, whether the
//! developer layer is on — kept on the gateway, so the phone and the computer of the same person show the same.
//! Kept like the desk (desk.rs): a row of the `state` table, `panel` / `<person>`, one JSON object up to 16 KB whose
//! keys the panel owns (shell/memory.ts). A change names only its keys: `PATCH {"folders": {...}}` keeps the rest,
//! `null` drops a key. A key has no memory of its own, as it has no desk. What belongs to a device — the theme, the
//! language, the microphone — stays in the browser.

use std::sync::Arc;

use axum::extract::State;
use axum::routing::get;
use axum::Router;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Map, Value};

use crate::app::App;
use crate::core::identity::principal::PrincipalKind;
use crate::platform::http::{ok, ApiError, ApiResult, Body, Caller};
use crate::utils::time::now_iso;

/// Its rows in the `state` table: `panel`, `<person>`.
const KIND: &str = "panel";
/// More than this is not a few remembered places.
const MAX_BYTES: usize = 16 * 1024;

pub fn routes() -> Router<Arc<App>> {
    Router::new().route("/api/v1/panel/state", get(read).patch(change))
}

/// The row of the caller: a person's (or the household's, without people), never a key's.
fn row_of(caller: &Caller) -> Result<String, ApiError> {
    let principal = caller.signed_in()?;
    match principal.kind {
        PrincipalKind::User | PrincipalKind::Household => Ok(principal.id.clone()),
        PrincipalKind::ApiKey | PrincipalKind::System => {
            Err(ApiError::forbidden(crate::tr!("Пульт помнит людей дома, у ключа памяти пульта нет.")))
        }
    }
}

/// What is kept for the row and when; nothing kept — an empty object.
fn kept(app: &App, row: &str) -> Result<(Map<String, Value>, Option<String>), ApiError> {
    let found: Option<(String, String)> = app.db.with(|c| {
        c.query_row(
            "SELECT items, updated_at FROM state WHERE kind = ?1 AND device = ?2",
            params![KIND, row],
            |found| Ok((found.get(0)?, found.get(1)?)),
        )
        .optional()
    })?;
    Ok(match found {
        Some((items, at)) => match serde_json::from_str::<Value>(&items) {
            Ok(Value::Object(map)) => (map, Some(at)),
            _ => (Map::new(), Some(at)),
        },
        None => (Map::new(), None),
    })
}

/// Everything the panel remembers about the caller.
async fn read(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Value> {
    let row = row_of(&caller)?;
    let (state, at) = kept(&app, &row)?;
    Ok(ok(json!({ "state": state, "at": at })))
}

/// Remember the keys named, drop those set to `null`; the other keys stay as they were.
async fn change(State(app): State<Arc<App>>, caller: Caller, Body(patch): Body<Value>) -> ApiResult<Value> {
    let row = row_of(&caller)?;
    let Value::Object(patch) = patch else {
        return Err(ApiError::bad_request(crate::tr!("Память пульта — объект JSON.")));
    };
    let (mut state, _) = kept(&app, &row)?;
    for (key, value) in patch {
        if value.is_null() {
            state.remove(&key);
        } else {
            state.insert(key, value);
        }
    }
    let items = serde_json::to_string(&state)?;
    if items.len() > MAX_BYTES {
        return Err(ApiError::bad_request(crate::tr!("Память пульта больше 16 КБ — так много пульт не помнит.")));
    }
    let at = now_iso();
    app.db.with(|c| {
        c.execute(
            "INSERT INTO state (kind, device, items, updated_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (kind, device) DO UPDATE SET items = excluded.items, updated_at = excluded.updated_at",
            params![KIND, row, items, at],
        )
    })?;
    Ok(ok(json!({ "state": state, "at": at })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Env};
    use crate::core::identity::principal::{api_key_principal, household_principal, user_principal};
    use crate::core::identity::Role;
    use axum::http::StatusCode;

    #[tokio::test]
    async fn remembers_each_persons_keys_merges_changes_and_drops_nulls() {
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
        let change_by = |caller: Caller, patch: Value| change(State(app.clone()), caller, Body(patch));

        // Nothing kept yet: an empty object.
        assert_eq!(read(State(app.clone()), anya()).await.unwrap().1, json!({ "state": {}, "at": null }));

        // Two keys, then a change of one: the other stays.
        let folders = json!({ "at": "Фото/Отпуск" });
        change_by(anya(), json!({ "folders": folders, "dev": true })).await.unwrap();
        let saved = change_by(anya(), json!({ "dev": false })).await.unwrap().1;
        assert_eq!(saved["state"], json!({ "folders": folders, "dev": false }));
        assert!(saved["at"].is_string());
        let kept = read(State(app.clone()), anya()).await.unwrap().1;
        assert_eq!((kept["state"].clone(), kept["at"].clone()), (saved["state"].clone(), saved["at"].clone()));

        // `null` drops a key.
        let dropped = change_by(anya(), json!({ "dev": null })).await.unwrap().1;
        assert_eq!(dropped["state"], json!({ "folders": folders }));

        // Another person has their own; the household without people too.
        assert_eq!(read(State(app.clone()), borya()).await.unwrap().1["state"], json!({}));
        let household = Caller::local(app.clone(), Some(household_principal(Role::Admin, None)));
        change_by(household, json!({ "dev": true })).await.unwrap();
        assert_eq!(read(State(app.clone()), anya()).await.unwrap().1["state"], json!({ "folders": folders }));

        // A key has none; a list or a huge change are refused and keep what was there.
        let key = Caller::local(app.clone(), Some(api_key_principal("k1", "Скрипт", &["station.view".into()])));
        assert_eq!(read(State(app.clone()), key).await.err().unwrap().status, StatusCode::FORBIDDEN);
        let list = change_by(anya(), json!([1, 2])).await;
        assert_eq!(list.err().unwrap().status, StatusCode::BAD_REQUEST);
        let huge = change_by(anya(), json!({ "note": "щ".repeat(10_000) })).await;
        assert_eq!(huge.err().unwrap().status, StatusCode::BAD_REQUEST);
        assert_eq!(read(State(app.clone()), anya()).await.unwrap().1["state"], json!({ "folders": folders }));

        // Without signing in — nothing.
        let nobody = Caller::local(app.clone(), None);
        assert_eq!(read(State(app.clone()), nobody).await.err().unwrap().status, StatusCode::UNAUTHORIZED);
    }
}
