//! The tests of `characters/voices.rs` that need the whole gateway (`App`): its crate does not have it, the program
//! does.

use crate::characters::contributes::KNOWN_ENGINES;
use crate::characters::voices::*;
use crate::characters::{ChooseBody, VoiceState};
use crate::computer::tests::voice_lab_testing::lab;
use crate::settings::definitions::TURBO_VOICE_ENGINES;
use crate::whole_tests::characters_contributes::{arthur_manifest, write_arthur};

fn engines() -> Vec<String> {
    crate::voice_lab::clone_engines::ENGINE_IDS.iter().map(|engine| engine.as_str().to_string()).collect()
}

#[test]
fn knows_the_engines_the_voice_lab_knows() {
    assert_eq!(KNOWN_ENGINES, engines().iter().map(String::as_str).collect::<Vec<_>>().as_slice());
}

/// A plan asked by the package (the wizard's, before the character is here) finds the model named for a character
/// of it; a character's own id only its own.
#[test]
fn the_model_named_for_a_character_is_found_by_its_package_too() {
    let named = std::collections::BTreeMap::from([
        ("rick:rick".to_string(), "cosyvoice3".to_string()),
        ("arthur:arthur".to_string(), "omnivoice".to_string()),
    ]);
    assert_eq!(named_in(named.clone(), "rick:rick").as_deref(), Some("cosyvoice3"));
    assert_eq!(named_in(named.clone(), "rick").as_deref(), Some("cosyvoice3"));
    assert_eq!(named_in(named.clone(), "rick:summer"), None);
    assert_eq!(named_in(named.clone(), "ric"), None);
    assert_eq!(named_in(named, "morty"), None);
}

/// A package removed takes the models its centre named for its characters with it; others' stay.
#[tokio::test]
async fn the_models_named_for_a_removed_package_go_with_it() {
    let lab = lab().await;
    let app = &lab.app;
    app.characters.name_engine("rick:rick", "cosyvoice3").unwrap();
    app.characters.name_engine("arthur:arthur", "omnivoice").unwrap();
    app.characters.forget_engines("rick");
    let named = app.settings.get(&TURBO_VOICE_ENGINES, None).unwrap();
    assert_eq!(named, std::collections::BTreeMap::from([("arthur:arthur".to_string(), "omnivoice".to_string())]));
    app.characters.forget_engines("nobody");
    assert_eq!(app.settings.get(&TURBO_VOICE_ENGINES, None).unwrap().len(), 1);
}

#[tokio::test]
async fn lays_out_the_snapshots_that_fit_and_removes_them_with_the_package() {
    let lab = lab().await;
    let app = &lab.app;
    let packages = app.config.plugins.packages_dir.clone();
    write_arthur(&packages, &arthur_manifest("1.2.0"));
    app.characters.sync();
    let store = &app.pc.voice_lab.store;
    let omnivoice = store.voice("c-arthur-arthur-omnivoice").expect("the snapshot that fits OmniVoice");
    assert_eq!((omnivoice.origin.as_deref(), omnivoice.name.as_str()), (Some("package"), "Артур"));
    assert_eq!(omnivoice.words.as_deref(), Some("Чай подан, сэр. Изволите ещё что-нибудь?"));
    assert!(omnivoice.has_ref);
    assert!(store.voice("c-arthur-arthur-cosyvoice3").is_none(), "a snapshot of other weights is not laid out");

    // The package removed: its voices go with it.
    std::fs::remove_dir_all(packages.join("arthur")).unwrap();
    app.characters.sync();
    assert!(store.voice("c-arthur-arthur-omnivoice").is_none(), "the package's voices go with it");
    assert!(store.package_voices().is_empty());

    // Back, and chosen: OmniVoice is installed here — it speaks at once, and holds its package while it does
    // (as any own voice chosen: the voice lab keeps it until another is chosen).
    write_arthur(&packages, &arthur_manifest("1.2.0"));
    let view = app.characters.view().await.unwrap();
    let arthur = view.characters.iter().find(|character| character.id == "arthur:arthur").unwrap();
    assert_eq!(arthur.voice.state, VoiceState::Ready);
    assert_eq!(arthur.voice.card_id.as_deref(), Some("own:c-arthur-arthur-omnivoice"));
    let body = ChooseBody { anyway: true, ..ChooseBody::default() };
    let answer = app.characters.choose("arthur:arthur", body, true, None).await.unwrap();
    assert_eq!(answer.voice.state, VoiceState::Ready);
    assert_eq!(app.pc.models.chosen_voice().unwrap().model_id, "own:c-arthur-arthur-omnivoice");
    assert_eq!(app.characters.before_removal("arthur").unwrap_err().status.as_u16(), 409);
    app.characters.before_removal("palych").unwrap();
}

#[tokio::test]
async fn a_character_whose_engine_is_not_here_says_what_it_needs_and_installs_it_only_when_asked() {
    let lab = lab().await;
    let app = &lab.app;
    let mut cosy_only = arthur_manifest("1.0.0");
    let character = &mut cosy_only["contributes"]["characters"][0];
    character["voices"] = serde_json::json!({ "cosyvoice3": { "file": "voices/cosyvoice3.pt", "model": "074ca6dc" } });
    character.as_object_mut().unwrap().remove("ref");
    write_arthur(&app.config.plugins.packages_dir, &cosy_only);
    app.characters.sync();
    assert!(
        app.pc.voice_lab.store.voice("c-arthur-arthur-cosyvoice3").is_some(),
        "laid out though its engine is not here"
    );
    let view = app.characters.view().await.unwrap();
    let arthur = view.characters.iter().find(|character| character.id == "arthur:arthur").unwrap();
    assert_eq!((arthur.voice.state, arthur.voice.engine.as_deref()), (VoiceState::Needs, Some("cosyvoice3")));
    assert_eq!(arthur.voice.needs.as_ref().unwrap()["kind"], "engine");
    let answer = app.characters.choose("arthur:arthur", ChooseBody::default(), true, None).await.unwrap();
    assert_eq!(answer.voice.state, VoiceState::Needs, "the character is chosen, its voice waits for the owner's word");
    assert!(answer.voice.pending.is_none());
    assert!(app.personas.chosen().unwrap().preset_id.as_deref() == Some("arthur:arthur"));
}
