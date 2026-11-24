//! The tests of `plugins/workshop/mod.rs` that need the whole gateway (`App`): its crate does not have it, the program
//! does.

use std::path::PathBuf;

use crate::plugins::workshop::*;

#[test]
fn keeps_a_drafts_files_to_plain_paths_and_nothing_a_browser_runs_in_its_assets() {
    assert_eq!(draft_path("backend.js").unwrap(), PathBuf::from("backend.js"));
    assert_eq!(draft_path("./manifest.json").unwrap(), PathBuf::from("manifest.json"));
    assert_eq!(draft_path("assets/stations.json").unwrap(), PathBuf::from("assets/stations.json"));
    for bad in
        ["../x", "assets/../../etc", "assets/.hidden", "other.js", "lib/a.js", "assets/page.html", "assets/a.svg", ""]
    {
        assert!(draft_path(bad).is_err(), "{bad}");
    }
}

fn env(dir: &std::path::Path) -> crate::config::Env {
    let text = |sub: &str| dir.join(sub).to_string_lossy().into_owned();
    crate::config::Env::from_pairs([
        ("STATE_DIR", text("state")),
        ("DIALOG_PROVIDER", "echo".into()),
        ("PACKAGES_DIR", text("packages")),
        ("DATA_DIR", text("data")),
        ("VOICE_LAB_DIR", text("voice-lab")),
        ("TTS_MODELS_DIR", text("voices")),
    ])
}

/// The check after a run keeps its verdict only on the card it began with: a new run begun on the draft, or the
/// draft parked or stopped while it went, decides instead — on 19.11 the old run's «готово» was written over the
/// new run's «работает», and the new run's work never ended. A card that says «работает» with no run going stops
/// on «Остановить», and the agent is no longer «at» the draft.
#[tokio::test]
async fn the_check_after_a_run_keeps_its_verdict_only_on_the_card_it_began_with() {
    let dir = tempfile::tempdir().unwrap();
    let app = crate::app::App::build(crate::config::Config::for_tests(&env(dir.path()))).unwrap();
    let workshop = app.workshop.clone();
    workshop.create("widget-1", Some("widget"), Some("Часы"), None).unwrap();
    let card = DraftMeta {
        by: activity::By::Panel,
        who: None,
        asked: "сделай виджет: часы".into(),
        kind: Face::Widget,
        stage: Stage::Working,
        updated: 0,
        note: None,
        kept: true,
        work: None,
        unchanged: false,
    };
    workshop.keep_card("widget-1", card, Some("made")).unwrap();
    let at_end = workshop.kept_card("widget-1").unwrap();
    // A new run began while the last one was checked: its card is kept anew.
    std::thread::sleep(std::time::Duration::from_millis(3));
    workshop.keep_card("widget-1", DraftMeta { asked: "доделай".into(), ..at_end.clone() }, Some("made")).unwrap();
    workshop.checked_after_run("widget-1", at_end, false, true).await;
    let kept = workshop.kept_card("widget-1").unwrap();
    assert_eq!((kept.stage, kept.asked.as_str()), (Stage::Working, "доделай"), "the new run's card stands");

    // Nothing came after it: the check decides.
    workshop.checked_after_run("widget-1", kept, false, true).await;
    assert!(matches!(workshop.card("widget-1").stage, Stage::Ready | Stage::Unfinished));

    // «работает» with no run going: «Остановить» stops it.
    let working = DraftMeta { stage: Stage::Working, ..workshop.card("widget-1") };
    workshop.keep_card("widget-1", working, None).unwrap();
    assert!(workshop.working("widget-1"), "the card says the agent works");
    assert!(workshop.stop("widget-1"));
    assert_eq!(workshop.card("widget-1").stage, Stage::Stopped);
    assert!(!workshop.working("widget-1"));
    assert!(!workshop.stop("widget-1"), "nothing goes on now");
}
