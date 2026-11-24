//! The tests of `plugins/history/routes.rs` that need the whole gateway (`App`): its crate does not have it, the
//! program does.

use std::sync::Arc;

use axum::extract::{Path, Query};

use crate::app::App;
use crate::config::{Config, Env};
use crate::core::identity::principal::user_principal;
use crate::core::identity::Role;
use crate::platform::http::{Body, Caller, Uses};
use crate::plugins::history::routes::*;
use crate::plugins::history::{Author, RecordKind, What, Who};
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

    let feed = feed(Uses::of(&app), admin(), Path("notes".into()), Query(FeedQuery { before: None, limit: None }))
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
    assert_eq!(
        crate::plugins::history::data::DataParts::read(history.store(), &kept).unwrap().kv[0].value,
        "[\"молоко\"]"
    );

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
        Uses::of(&app),
        Uses::of(&app),
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
    restore_draft(
        Uses::of(&app),
        Uses::of(&app),
        admin(),
        Path("notes".into()),
        Body(DraftRestoreBody { record: first_step }),
    )
    .await
    .unwrap();
    let readme = std::fs::read(dir.path().join("data/packages-dev/notes/README.md")).ok();
    assert_eq!(readme, history.file_at(&first_tree, "README.md"), "the template's README as it was");
    assert_eq!(history.store().log("notes").len(), steps + 1);
    let room = room(Uses::of(&app), admin()).await.unwrap().1;
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
    assert_eq!(crate::plugins::history::merge::state(&history, "tips").unwrap().question.unwrap().options.len(), 2);
    assert!(app.workshop.answer_merge("tips", "их").unwrap().contains("«их»"));
    app.workshop.write("tips", "README.md", "их сторона\n").unwrap();
    app.workshop.finish_merge("tips").await.unwrap();
    assert!(crate::plugins::history::merge::state(&history, "tips").is_none());
    let (_, head) = history.head("tips", "draft").unwrap();
    assert_eq!((head.kind, head.parents.len()), (RecordKind::Merge, 2));

    // «Отменить»: the draft back as it was before the merge.
    app.workshop.write("tips", "README.md", "снова наша\n").unwrap();
    app.workshop.hand_edited("tips", Author::new(Who::Hand));
    let again = other_side("снова их\n");
    assert_eq!(app.workshop.merge("tips", &again, true, Author::new(Who::Hand), None).await.unwrap().state, "agent");
    app.workshop.cancel_merge("tips").unwrap();
    assert_eq!(app.workshop.read("tips", "README.md").unwrap(), "снова наша\n");
    assert!(crate::plugins::history::merge::state(&history, "tips").is_none());
}
