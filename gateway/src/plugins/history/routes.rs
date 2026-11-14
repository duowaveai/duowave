//! The history's API (docs/plans/package-history.md, «API»): the admin's, as the workshop is.
//!
//! - `GET /api/v1/history` — the room it takes, where it is kept; `PUT …/settings`, `POST …/clean`.
//! - `GET /api/v1/packages/{id}/history` — the feed; `…/history/compare`, `…/history/{record}` and its files.
//! - `GET|POST /api/v1/packages/{id}/restore` — what a step back of the installed app would do, and the step back.
//! - `POST /api/v1/workshop/drafts/{id}/restore` — the draft back to a step.
//! - `/api/v1/workshop/drafts/{id}/merge` — a merge: start, its state, the answer to the agent's question, «Отменить».
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::routing::{get, post, put};
use axum::Router;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use super::gc::Place;
use super::merge::MergeReport;
use super::restore::{RestorePlan, RestoreReport};
use super::{Author, Comparison, Feed, History, What, Who};
use crate::app::App;
use crate::platform::http::{ok, ApiError, ApiResult, Body, Caller};
use crate::plugins::routes::admin;
use crate::plugins::workshop::agent::{self, ChatStep};
use crate::plugins::workshop::DraftView;
use crate::settings::definitions::PACKAGE_HISTORY;

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/v1/history", get(room))
        .route("/api/v1/history/settings", put(set_settings))
        .route("/api/v1/history/clean", post(clean))
        .route("/api/v1/packages/{id}/history", get(feed))
        .route("/api/v1/packages/{id}/history/compare", get(compare))
        .route("/api/v1/packages/{id}/history/{record}", get(record))
        .route("/api/v1/packages/{id}/history/{record}/files/{*path}", get(file))
        .route("/api/v1/packages/{id}/restore", get(restore_plan).post(restore))
        .route("/api/v1/workshop/drafts/{id}/restore", post(restore_draft))
        .route("/api/v1/workshop/drafts/{id}/merge", get(merge_state).post(merge).delete(cancel_merge))
        .route("/api/v1/workshop/drafts/{id}/merge/answer", post(answer))
}

fn history_of(app: &App) -> Result<Arc<History>, ApiError> {
    app.history
        .clone()
        .ok_or_else(|| ApiError::conflict(crate::tr!("Истории приложений здесь нет: её папка не создаётся.")))
}

fn by_of(actor: Option<&crate::core::events::Actor>) -> Author {
    Author::of(actor, Who::Hand)
}

async fn room(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<super::gc::Room> {
    admin(&caller)?;
    Ok(ok(history_of(&app)?.room()))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsBody {
    place: Option<String>,
    budget_mb: Option<u64>,
    days: Option<u64>,
    steps: Option<u64>,
}

async fn set_settings(
    State(app): State<Arc<App>>,
    caller: Caller,
    Body(body): Body<SettingsBody>,
) -> ApiResult<super::gc::Room> {
    let actor = admin(&caller)?;
    let history = history_of(&app)?;
    let mut stored = app.settings.get(&PACKAGE_HISTORY, None).unwrap_or_else(|| json!({}));
    if let Some(place) = body.place {
        let place = Place::parse(&place).ok_or_else(|| ApiError::field("place", crate::tr!("here, home или both")))?;
        if place != Place::Here && !history.on_station() {
            return Err(ApiError::field("place", crate::tr!("компьютер хранит историю у себя")));
        }
        stored["place"] = json!(place);
    }
    for (key, value) in [("budgetMb", body.budget_mb), ("days", body.days), ("steps", body.steps)] {
        if let Some(value) = value {
            if value == 0 {
                return Err(ApiError::field(key, crate::tr!("больше нуля")));
            }
            stored[key] = json!(value);
        }
    }
    app.settings.set(&PACKAGE_HISTORY, &stored, actor)?;
    Ok(ok(history.room()))
}

async fn clean(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<super::gc::Cleaned> {
    admin(&caller)?;
    let history = history_of(&app)?;
    let cleaned = tokio::task::spawn_blocking(move || history.clean()).await.map_err(ApiError::internal)?;
    Ok(ok(cleaned))
}

#[derive(Debug, Deserialize)]
pub struct FeedQuery {
    before: Option<String>,
    limit: Option<usize>,
}

async fn feed(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
    Query(query): Query<FeedQuery>,
) -> ApiResult<Feed> {
    admin(&caller)?;
    let history = history_of(&app)?;
    // On the station that sent its history to the computer: the records it cleaned come back for the feed.
    if history.on_station() {
        let missing: Vec<String> =
            history.store().log(&id).into_iter().filter(|record| !history.store().has(record)).collect();
        if !missing.is_empty() {
            history.fetch(&missing).await;
        }
    }
    Ok(ok(history.feed(&id, query.before.as_deref(), query.limit.unwrap_or(50).clamp(1, 200))))
}

#[derive(Debug, Deserialize)]
pub struct CompareQuery {
    from: Option<String>,
    to: Option<String>,
}

async fn compare(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
    Query(query): Query<CompareQuery>,
) -> ApiResult<Comparison> {
    admin(&caller)?;
    let history = history_of(&app)?;
    let resolve = |record: Option<String>| -> Result<Option<String>, ApiError> {
        match record.filter(|record| !record.is_empty()) {
            Some(record) => history
                .resolve(&id, &record)
                .map(Some)
                .ok_or_else(|| ApiError::not_found(crate::tr!("Такой записи нет."))),
            None => Ok(None),
        }
    };
    let (from, to) = (resolve(query.from)?, resolve(query.to)?);
    for record in from.iter().chain(to.iter()) {
        history.ensure_local(record).await;
    }
    let comparison = history.compare(&id, from.as_deref(), to.as_deref()).map_err(ApiError::bad_request)?;
    Ok(ok(comparison))
}

async fn record(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path((id, record)): Path<(String, String)>,
) -> ApiResult<Value> {
    admin(&caller)?;
    let history = history_of(&app)?;
    let record = history.resolve(&id, &record).ok_or_else(|| ApiError::not_found(crate::tr!("Такой записи нет.")))?;
    history.ensure_local(&record).await;
    let view = history.record_view(&id, &record).ok_or_else(|| ApiError::not_found(crate::tr!("Такой записи нет.")))?;
    let data = view
        .record
        .data
        .as_deref()
        .and_then(|data| super::data::DataParts::read(history.store(), data))
        .map(|snapshot| snapshot.summary());
    Ok(ok(json!({ "record": view, "files": history.files_of(&record), "data": data })))
}

async fn file(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path((id, record, path)): Path<(String, String, String)>,
) -> ApiResult<Value> {
    admin(&caller)?;
    let history = history_of(&app)?;
    let record = history.resolve(&id, &record).ok_or_else(|| ApiError::not_found(crate::tr!("Такой записи нет.")))?;
    history.ensure_local(&record).await;
    let tree = history
        .record(&record)
        .map(|record| record.tree)
        .ok_or_else(|| ApiError::not_found(crate::tr!("Записи нет.")))?;
    let bytes = history
        .file_at(&tree, &path)
        .ok_or_else(|| ApiError::not_found(crate::tr!("Нет файла {path}.", path = path)))?;
    let content = String::from_utf8(bytes).map_err(|_| ApiError::bad_request(crate::tr!("Файл не текстовый.")))?;
    Ok(ok(json!({ "path": path, "content": content })))
}

#[derive(Debug, Deserialize)]
pub struct RestoreQuery {
    record: String,
    what: Option<String>,
}

fn what_of(text: Option<&str>) -> Result<What, ApiError> {
    match text {
        None => Ok(What::Code),
        Some(text) => What::parse(text).ok_or_else(|| ApiError::field("what", crate::tr!("code, data или code+data"))),
    }
}

async fn restore_plan(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
    Query(query): Query<RestoreQuery>,
) -> ApiResult<RestorePlan> {
    admin(&caller)?;
    let history = history_of(&app)?;
    let record =
        history.resolve(&id, &query.record).ok_or_else(|| ApiError::not_found(crate::tr!("Такой записи нет.")))?;
    history.ensure_local(&record).await;
    Ok(ok(history.restore_plan(&id, &record, what_of(query.what.as_deref())?)?))
}

#[derive(Debug, Deserialize)]
pub struct RestoreBody {
    record: String,
    what: Option<String>,
    #[serde(default)]
    approve: bool,
}

async fn restore(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<RestoreBody>,
) -> ApiResult<RestoreReport> {
    let actor = admin(&caller)?;
    let history = history_of(&app)?;
    let record =
        history.resolve(&id, &body.record).ok_or_else(|| ApiError::not_found(crate::tr!("Такой записи нет.")))?;
    history.ensure_local(&record).await;
    let what = what_of(body.what.as_deref())?;
    let report =
        history.restore_installed(&id, &record, what, body.approve, by_of(actor.as_ref()), actor.clone()).await?;
    if report.restored {
        app.characters.sync();
    }
    Ok(ok(report))
}

#[derive(Debug, Deserialize)]
pub struct DraftRestoreBody {
    record: String,
}

async fn restore_draft(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<DraftRestoreBody>,
) -> ApiResult<DraftView> {
    let actor = admin(&caller)?;
    if let Some(history) = &app.history {
        if let Some(record) = history.resolve(&id, &body.record) {
            history.ensure_local(&record).await;
        }
    }
    Ok(ok(app.workshop.restore_step(&id, &body.record, by_of(actor.as_ref()))?))
}

/// The agent's work on a draft in the background, as the panel's chat starts it: the plaque and the log follow it.
pub fn start_agent(app: &App, caller: &Caller, id: &str, message: String) -> Result<(), ApiError> {
    if !app.dialog.enabled() {
        return Err(ApiError::conflict(crate::tr!("Модель не выбрана: добавьте провайдера в «Персонаж» → «Мозг».")));
    }
    let principal = caller.principal().map(|principal| principal.id.clone()).unwrap_or_default();
    let key = format!("workshop:{principal}:{id}");
    let told: Arc<dyn Fn(ChatStep) + Send + Sync> = Arc::new(|_| {});
    let mut phrases =
        agent::chat(&app.dialog, app.workshop.clone(), &key, id, &message, told, CancellationToken::new());
    tokio::spawn(async move { while phrases.next().await.is_some() {} });
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct MergeBody {
    /// A record; or `upstream` — the newer version of the app a copy was made from.
    record: String,
    #[serde(default)]
    agent: bool,
    /// What the owner said about it.
    asked: Option<String>,
}

async fn merge(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<MergeBody>,
) -> ApiResult<MergeReport> {
    let actor = admin(&caller)?;
    let history = history_of(&app)?;
    let record = if body.record == "upstream" {
        history
            .upstream_record(&id)
            .ok_or_else(|| ApiError::conflict(crate::tr!("Новой версии исходного приложения нет.")))?
    } else {
        history
            .resolve(&id, &body.record)
            .or_else(|| {
                // A record of another app (a copy's source): by its whole id.
                super::store::valid_id(&body.record)
                    .then(|| body.record.clone())
                    .filter(|r| history.record(r).is_some())
            })
            .ok_or_else(|| ApiError::not_found(crate::tr!("Такой записи нет.")))?
    };
    history.ensure_local(&record).await;
    let asked = body.asked.map(|asked| asked.trim().chars().take(600).collect::<String>()).filter(|a| !a.is_empty());
    let report = app.workshop.merge(&id, &record, body.agent, by_of(actor.as_ref()), asked).await?;
    if let Some(merge) = report.merge.as_ref() {
        start_agent(&app, &caller, &id, app.workshop.merge_brief(merge))?;
    }
    Ok(ok(report))
}

async fn merge_state(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<Value> {
    admin(&caller)?;
    let history = history_of(&app)?;
    Ok(ok(json!({ "merge": super::merge::state(&history, &id) })))
}

#[derive(Debug, Deserialize)]
pub struct AnswerBody {
    choice: String,
}

async fn answer(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<AnswerBody>,
) -> ApiResult<Value> {
    admin(&caller)?;
    if body.choice.trim().is_empty() {
        return Err(ApiError::field("choice", crate::tr!("ответ на вопрос")));
    }
    let message = app.workshop.answer_merge(&id, &body.choice)?;
    start_agent(&app, &caller, &id, message)?;
    Ok(ok(json!({ "answered": true })))
}

async fn cancel_merge(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<DraftView> {
    admin(&caller)?;
    app.workshop.cancel_merge(&id)?;
    Ok(ok(app.workshop.draft(&id)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Env};
    use crate::core::identity::principal::user_principal;
    use crate::core::identity::Role;
    use crate::plugins::history::RecordKind;
    use crate::plugins::host_api::{kv_replace, kv_rows};

    fn app_in(dir: &std::path::Path) -> Arc<App> {
        let text = |sub: &str| dir.join(sub).to_string_lossy().into_owned();
        let env = Env::from_pairs([
            ("STATE_DIR", String::new()),
            ("PACKAGES_DIR", text("packages")),
            ("DATA_DIR", text("data")),
            ("VOICE_LAB_DIR", text("voice-lab")),
            ("TTS_MODELS_DIR", text("voices")),
        ]);
        App::build(Config::for_tests(&env)).unwrap()
    }

    #[tokio::test]
    async fn keeps_a_drafts_steps_and_versions_and_brings_the_installed_app_back_and_forward_with_its_data() {
        let dir = tempfile::tempdir().unwrap();
        let app = app_in(dir.path());
        let history = app.history.clone().expect("the history stands");
        let admin = || Caller::local(app.clone(), Some(user_principal("u1", "Админ", Role::Admin, None)));
        app.workshop.create("notes", Some("plugin"), Some("Заметки"), None).unwrap();
        assert_eq!(history.store().log("notes").len(), 1, "the draft's first step: where it came from");
        app.workshop.write("notes", "README.md", "первая\n").unwrap();
        app.workshop.hand_edited("notes", Author::new(Who::Hand));
        let first = app.workshop.install("notes", true, &serde_json::Map::new(), None).await.unwrap();
        let v1 = first.version.clone().unwrap();
        kv_replace(&app.db, "notes", &[("list".into(), "[\"молоко\"]".into())]).unwrap();
        app.workshop.write("notes", "README.md", "вторая\n").unwrap();
        app.workshop.hand_edited("notes", Author::new(Who::Hand));
        let second = app.workshop.install("notes", false, &serde_json::Map::new(), None).await.unwrap();
        let v2 = second.version.clone().unwrap();
        assert_ne!(v1, v2);
        kv_replace(&app.db, "notes", &[("list".into(), "[\"хлеб\"]".into())]).unwrap();

        let feed =
            feed(State(app.clone()), admin(), Path("notes".into()), Query(FeedQuery { before: None, limit: None }))
                .await
                .unwrap()
                .1;
        let install = |version: &str| {
            feed.records
                .iter()
                .find(|view| view.record.kind == RecordKind::Install && view.record.version.as_deref() == Some(version))
                .map(|view| view.id.clone())
                .unwrap()
        };
        let (install1, install2) = (install(&v1), install(&v2));
        assert_eq!(feed.records[0].marks, vec!["draft".to_string(), "installed".to_string()]);
        // The second install kept the data the first version left: «молоко».
        let kept = history.record(&install2).unwrap().data.unwrap();
        assert_eq!(super::super::data::DataParts::read(history.store(), &kept).unwrap().kv[0].value, "[\"молоко\"]");

        // Back to the first version, the code alone: the data stays.
        let plan = history.restore_plan("notes", &install1, What::Code).unwrap();
        assert_eq!((plan.version.as_deref(), plan.needs_approval), (Some(v1.as_str()), false));
        let back = history.restore_installed("notes", &install1, What::Code, false, Author::new(Who::Hand), None).await;
        assert!(back.unwrap().restored);
        assert_eq!(app.plugins.installed("notes").unwrap().version(), v1);
        assert_eq!(kv_rows(&app.db, "notes").unwrap()[0].1, "[\"хлеб\"]");
        // And forward again: the second version comes back from the history.
        history.restore_installed("notes", &install2, What::Code, false, Author::new(Who::Hand), None).await.unwrap();
        assert_eq!(app.plugins.installed("notes").unwrap().version(), v2);
        // The data as it was when the first version gave way.
        let report = restore(
            State(app.clone()),
            admin(),
            Path("notes".into()),
            Body(RestoreBody { record: install1[..7].to_string(), what: Some("data".into()), approve: false }),
        )
        .await
        .unwrap()
        .1;
        assert!(report.restored, "{}", report.message);
        assert_eq!(kv_rows(&app.db, "notes").unwrap()[0].1, "[\"молоко\"]");
        // The draft back to its first step, and the step back is a step of its own.
        let steps = history.store().log("notes").len();
        let first_step = history.store().log("notes")[0].clone();
        let first_tree = history.record(&first_step).unwrap().tree;
        restore_draft(State(app.clone()), admin(), Path("notes".into()), Body(DraftRestoreBody { record: first_step }))
            .await
            .unwrap();
        let readme = std::fs::read(dir.path().join("data/packages-dev/notes/README.md")).ok();
        assert_eq!(readme, history.file_at(&first_tree, "README.md"), "the template's README as it was");
        assert_eq!(history.store().log("notes").len(), steps + 1);
        let room = room(State(app.clone()), admin()).await.unwrap().1;
        assert!(room.bytes > 0 && room.budget_bytes == 200 * 1024 * 1024);
    }

    #[tokio::test]
    async fn merges_another_line_by_itself_and_hands_a_clash_to_the_agent_who_asks_and_finishes() {
        let dir = tempfile::tempdir().unwrap();
        let app = app_in(dir.path());
        let history = app.history.clone().unwrap();
        app.workshop.create("tips", Some("plugin"), Some("Советы"), None).unwrap();
        let base = history.store().ref_get("tips", "draft").unwrap();
        // Another line from the same base (as the station would have it): its README changed.
        let other_side = |readme: &str| {
            let folder = tempfile::tempdir().unwrap();
            history.materialize(&history.record(&base).unwrap().tree, folder.path(), false).unwrap();
            std::fs::write(folder.path().join("README.md"), readme).unwrap();
            let tree = history.snapshot_dir(folder.path()).unwrap();
            let mut record = history.new_record("tips", RecordKind::Step, tree, Author::new(Who::Agent));
            record.parents = vec![base.clone()];
            history.write(&record).unwrap()
        };
        let theirs = other_side("с другой стороны\n");
        // Ours: another file.
        app.workshop.write("tips", "panel.css", ".a { color: red }\n").unwrap();
        app.workshop.hand_edited("tips", Author::new(Who::Hand));
        let report = app.workshop.merge("tips", &theirs, false, Author::new(Who::Hand), None).await.unwrap();
        assert_eq!(report.state, "merged", "{}", report.message);
        assert_eq!(app.workshop.read("tips", "README.md").unwrap(), "с другой стороны\n");
        assert_eq!(app.workshop.read("tips", "panel.css").unwrap(), ".a { color: red }\n");
        let merged = history.record(&report.record.unwrap()).unwrap();
        assert_eq!((merged.kind, merged.parents.len()), (RecordKind::Merge, 2));

        // The same file both ways: the agent's; it asks, gets the answer, finishes.
        app.workshop.write("tips", "README.md", "наша сторона\n").unwrap();
        app.workshop.hand_edited("tips", Author::new(Who::Hand));
        let clash = other_side("их сторона\n");
        let report = app.workshop.merge("tips", &clash, false, Author::new(Who::Hand), None).await.unwrap();
        assert_eq!(report.state, "agent");
        assert_eq!(report.merge.as_ref().unwrap().conflicts[0].path, "README.md");
        assert!(app.workshop.merge_brief(report.merge.as_ref().unwrap()).contains("README.md"));
        let files = app.workshop.merge_files("tips", None).unwrap();
        assert_eq!(files["files"][0]["theirs"], "их сторона\n");
        app.workshop.ask_owner("tips", "Какой текст оставить?", vec!["наш".into(), "их".into()]).unwrap();
        assert_eq!(super::super::merge::state(&history, "tips").unwrap().question.unwrap().options.len(), 2);
        assert!(app.workshop.answer_merge("tips", "их").unwrap().contains("«их»"));
        app.workshop.write("tips", "README.md", "их сторона\n").unwrap();
        app.workshop.finish_merge("tips").await.unwrap();
        assert!(super::super::merge::state(&history, "tips").is_none());
        let (_, head) = history.head("tips", "draft").unwrap();
        assert_eq!((head.kind, head.parents.len()), (RecordKind::Merge, 2));

        // «Отменить»: the draft back as it was before the merge.
        app.workshop.write("tips", "README.md", "снова наша\n").unwrap();
        app.workshop.hand_edited("tips", Author::new(Who::Hand));
        let again = other_side("снова их\n");
        assert_eq!(
            app.workshop.merge("tips", &again, true, Author::new(Who::Hand), None).await.unwrap().state,
            "agent"
        );
        app.workshop.cancel_merge("tips").unwrap();
        assert_eq!(app.workshop.read("tips", "README.md").unwrap(), "снова наша\n");
        assert!(super::super::merge::state(&history, "tips").is_none());
    }
}
