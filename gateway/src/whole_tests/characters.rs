//! The tests of `characters/mod.rs` that need the whole gateway (`App`): its crate does not have it, the program does.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::json;

use crate::app::App;
use crate::characters::*;
use crate::config::{Config, Env};
use crate::dialog::system_prompt::{wake_word_line, ALICE};
use crate::whole_tests::characters_contributes::{arthur_manifest, write_arthur};

/// A gateway with its packages in a folder of the test's own.
pub fn app(dir: &std::path::Path) -> Arc<App> {
    let env = Env::from_pairs([
        ("STATE_DIR", String::new()),
        ("PACKAGES_DIR", dir.join("packages").to_string_lossy().into_owned()),
        ("VOICE_LAB_DIR", dir.join("voice-lab").to_string_lossy().into_owned()),
        ("VOICE_LAB_RUNTIME_DIR", dir.join("runtime").to_string_lossy().into_owned()),
        ("TTS_MODELS_DIR", dir.join("voices").to_string_lossy().into_owned()),
        ("PACKAGES_FEED_URL", "off".into()),
    ]);
    App::build(Config::for_tests(&env)).unwrap()
}

fn rows(app: &App) -> Vec<(String, Option<String>)> {
    app.db
        .with(|c| {
            let mut statement = c.prepare("SELECT id, package FROM characters ORDER BY rowid")?;
            let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
            rows.collect()
        })
        .unwrap()
}

#[tokio::test]
async fn keeps_the_packages_characters_in_the_table_beside_the_built_in_ones_and_drops_those_gone() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path());
    write_arthur(&dir.path().join("packages"), &arthur_manifest("1.2.0"));
    app.characters.sync();
    let found = rows(&app);
    assert_eq!(found.len(), 2, "Alisa and the package's character");
    assert_eq!(found[1], ("arthur:arthur".into(), Some("arthur".into())));
    let view = app.characters.view().await.unwrap();
    let arthur = view.characters.iter().find(|character| character.id == "arthur:arthur").unwrap();
    assert_eq!(arthur.avatar.as_deref(), Some("/media/packages/arthur/avatar.webp"));
    assert_eq!(arthur.samples[0].url, "/media/packages/arthur/samples/1.opus");
    assert_eq!(arthur.package.as_ref().unwrap().license.as_deref(), Some("CC BY-NC 4.0, некоммерчески"));
    let json = serde_json::to_value(arthur).unwrap();
    assert_eq!(
        json["tones"][0],
        json!({"id": "irony", "title": "Ирония", "default": 1, "options": [{"label": "мало"}, {"label": "в меру"}, {"label": "много"}]})
    );
    assert!(view.characters[0].builtin && view.characters[0].id == "alice");

    // The choice by id of the old screen takes it too.
    app.personas.set(json!({"presetId": "arthur:arthur"}).as_object().unwrap(), None).unwrap();
    assert!(app.personas.system_prompt(None).text.contains(&wake_word_line()));

    std::fs::remove_dir_all(dir.path().join("packages/arthur")).unwrap();
    app.characters.sync();
    assert_eq!(rows(&app).len(), 1, "a package gone takes its characters' rows");
}

/// The list read again and again writes nothing: the row keeps its time and the settings revision stays, so the
/// computer of the pair takes no copy for a look (live, 11.11). A package updated changes its row, and the
/// revision with it.
#[tokio::test]
async fn reading_the_list_again_rewrites_no_row_and_moves_no_revision() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path());
    write_arthur(&dir.path().join("packages"), &arthur_manifest("1.2.0"));
    app.characters.view().await.unwrap();
    let revision = || crate::placement::transfer::revision(&app.db, &app.secrets).unwrap();
    let written = || -> String {
        app.db
            .with(|c| c.query_row("SELECT updated_at FROM characters WHERE id = 'arthur:arthur'", [], |row| row.get(0)))
            .unwrap()
    };
    let (before, at) = (revision(), written());
    // The times are to the millisecond: a row written anew would show it.
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    app.characters.view().await.unwrap();
    app.characters.sync();
    assert_eq!((revision(), written()), (before, at.clone()));

    std::fs::remove_dir_all(dir.path().join("packages/arthur")).unwrap();
    write_arthur(&dir.path().join("packages"), &arthur_manifest("1.3.0"));
    app.characters.view().await.unwrap();
    assert_eq!(revision(), before + 1);
    assert_ne!(written(), at);
}

#[tokio::test]
async fn a_character_that_does_not_check_out_stays_installed_and_is_not_chosen() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path());
    let mut broken = arthur_manifest("1.0.0");
    broken["contributes"]["characters"][0]["prompt"] = json!("");
    write_arthur(&dir.path().join("packages"), &broken);
    let view = app.characters.view().await.unwrap();
    let arthur = view.characters.iter().find(|character| character.id == "arthur:arthur").unwrap();
    assert!(arthur.problem.as_deref().unwrap().contains("нет prompt"));
    assert!(app.plugins.installed("arthur").is_some(), "the package stays");
    assert_eq!(rows(&app).len(), 1, "not offered for the choice by id");
    let refused = app.characters.choose("arthur:arthur", ChooseBody::default(), true, None).await.unwrap_err();
    assert_eq!(refused.status.as_u16(), 400);
}

#[tokio::test]
async fn a_character_its_feed_hides_is_not_offered_and_gives_its_place_to_alisa() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path());
    let mut manifest = arthur_manifest("1.2.0");
    manifest["contributes"]["characters"][0]["voiceOrigin"] = json!("unknown");
    write_arthur(&dir.path().join("packages"), &manifest);
    let body = ChooseBody { voice: Some(false), ..ChooseBody::default() };
    app.characters.choose("arthur:arthur", body, false, None).await.unwrap();
    let view = app.characters.view().await.unwrap();
    let arthur = view.characters.iter().find(|character| character.id == "arthur:arthur").unwrap();
    assert_eq!(arthur.voice_origin.as_deref(), Some("unknown"), "the panel says no one checked it");

    // Its feed hides it: not offered, Alisa answers; its package stays on disk.
    crate::plugins::registry::set_installed(&app.db, "arthur", crate::plugins::registry::SOURCE_CATALOG, false, None)
        .unwrap();
    crate::plugins::registry::set_hidden(&app.db, "arthur", true).unwrap();
    let view = app.characters.view().await.unwrap();
    assert!(view.characters.iter().all(|character| character.id != "arthur:arthur"));
    assert_eq!(view.active.id.as_deref(), Some(ALICE));
    assert!(app.plugins.installed("arthur").is_some());
    // Shown again: offered again; the choice stays Alisa's.
    crate::plugins::registry::set_hidden(&app.db, "arthur", false).unwrap();
    let view = app.characters.view().await.unwrap();
    assert!(view.characters.iter().any(|character| character.id == "arthur:arthur"));
    assert_eq!(view.active.id.as_deref(), Some(ALICE));
}

/// docs/plans/wake-word-station.md: an activator's word makes its character the house's — once, and only when that
/// character is here.
#[tokio::test]
async fn a_word_makes_its_character_the_houses_once_and_only_when_it_is_here() {
    use crate::ear::word::WordCharacter;
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path());
    write_arthur(&dir.path().join("packages"), &arthur_manifest("1.2.0"));
    app.characters.sync();
    assert!(!app.characters.answer_as("butler:otto", "Отто"), "a word whose character is not installed");
    assert!(app.characters.answer_as("arthur:arthur", "Артур"));
    assert_eq!(app.personas.chosen().and_then(|chosen| chosen.preset_id).as_deref(), Some("arthur:arthur"));
    assert!(app.personas.system_prompt(None).text.starts_with("Ты — Артур, дворецкий умной колонки."));
    assert!(!app.characters.answer_as("arthur:arthur", "Артур"), "answers already");
}

#[tokio::test]
async fn chooses_a_character_with_its_tones_and_tells_edits_and_a_newer_text_apart() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path());
    write_arthur(&dir.path().join("packages"), &arthur_manifest("1.2.0"));
    let body = ChooseBody {
        tones: Some(BTreeMap::from([("irony".to_string(), 2)])),
        voice: Some(false),
        ..ChooseBody::default()
    };
    let answer = app.characters.choose("arthur:arthur", body, false, None).await.unwrap();
    assert_eq!(answer.character.id, "arthur:arthur");
    let prompt = app.personas.system_prompt(None).text;
    assert!(
        prompt.starts_with("Ты — Артур, дворецкий умной колонки.\nШутишь при каждом удобном случае.\n"),
        "{prompt}"
    );
    assert!(prompt.contains(&wake_word_line()) && prompt.contains("Каждый ответ будет озвучен"));
    let active = app.characters.view().await.unwrap().active;
    assert_eq!((active.id.as_deref(), active.edited, &active.update), (Some("arthur:arthur"), false, &json!(false)));
    assert_eq!(active.tones, BTreeMap::from([("irony".to_string(), 2)]));

    // The owner's edit: kept; a newer text in the package is told, not applied.
    app.personas
        .set(json!({"presetId": "arthur:arthur", "text": "Ты — Артур. Отвечай стихами."}).as_object().unwrap(), None)
        .unwrap();
    let mut newer = arthur_manifest("1.3.0");
    newer["contributes"]["characters"][0]["prompt"] = json!("Ты — Артур, дворецкий. Новая версия.");
    std::fs::remove_dir_all(dir.path().join("packages/arthur")).unwrap();
    write_arthur(&dir.path().join("packages"), &newer);
    let active = app.characters.view().await.unwrap().active;
    assert!(active.edited);
    assert_eq!(active.update, json!("Ты — Артур, дворецкий. Новая версия.\nШутишь при каждом удобном случае."));
    assert!(app.personas.system_prompt(None).text.starts_with("Ты — Артур. Отвечай стихами."), "the edit stays");
    // «Обновить»: chosen again.
    let body = ChooseBody { voice: Some(false), tones: Some(active.tones), ..ChooseBody::default() };
    app.characters.choose("arthur:arthur", body, false, None).await.unwrap();
    let active = app.characters.view().await.unwrap().active;
    assert_eq!((active.edited, &active.update), (false, &json!(false)));
    assert!(app.personas.system_prompt(None).text.starts_with("Ты — Артур, дворецкий. Новая версия."));

    // Alisa, built in, in the positions given and without a voice of her own (hers is the one chosen).
    let tones = BTreeMap::from([("irony".to_string(), 0), ("warmth".to_string(), 2)]);
    let body = ChooseBody { tones: Some(tones), ..ChooseBody::default() };
    let answer = app.characters.choose("alice", body, false, None).await.unwrap();
    assert_eq!((answer.character.builtin, answer.voice.state), (true, VoiceState::None));
    assert_eq!(answer.character.tones.len(), 3);
    let prompt = app.personas.system_prompt(None).text;
    assert!(!prompt.contains(&wake_word_line()), "Alisa needs no such line");
    assert!(prompt.contains("без шуток") && prompt.contains("близкая подруга"));
    let active = app.characters.view().await.unwrap().active;
    assert_eq!(active.id.as_deref(), Some("alice"));
    assert_eq!(active.tones, BTreeMap::from([("detail".into(), 1), ("irony".into(), 0), ("warmth".into(), 2)]));
    assert_eq!((active.edited, &active.update, active.replaced_by), (false, &json!(false), None));
    // Its voice is not this caller's to change.
    let refused = app.characters.choose("arthur:arthur", ChooseBody::default(), false, None).await.unwrap_err();
    assert_eq!(refused.status.as_u16(), 403);
    assert_eq!(
        app.characters.choose("nobody", ChooseBody::default(), true, None).await.unwrap_err().status.as_u16(),
        404
    );
}
