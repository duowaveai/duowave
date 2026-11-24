//! The tests of `characters/contributes.rs` that need the whole gateway (`App`): its crate does not have it, the
//! program does.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{json, Value};

use crate::characters::contributes::*;
use crate::plugins::registry::{read_package, tests::write_package, Candidate};

pub fn arthur_manifest(version: &str) -> Value {
    json!({
        "schema": 1, "kind": "character", "id": "arthur", "version": version, "api": "^2",
        "title": "Артур", "description": "ИИ-дворецкий: «сэр», сухой юмор",
        "runs": ["pc", "station", "stationServer"], "license": "CC BY-NC 4.0, некоммерчески",
        "contributes": { "characters": [{
            "id": "arthur", "gender": "male", "color": "#7A8CA5", "avatar": "avatar.webp",
            "prompt": "Ты — Артур, дворецкий умной колонки.",
            "tones": [{ "id": "irony", "title": "Ирония", "default": 1, "options": [
                { "label": "мало", "line": "Шутишь редко." },
                { "label": "в меру", "line": "Шутишь сухо и к месту." },
                { "label": "много", "line": "Шутишь при каждом удобном случае." }
            ] }],
            "samples": [{ "text": "Чай подан, сэр.", "file": "samples/1.opus" }],
            "fallbackVoice": "piper-denis",
            "voices": {
                "omnivoice": { "file": "voices/omnivoice.pt", "model": "c5fdb5cc" },
                "cosyvoice3": { "file": "voices/cosyvoice3.pt", "model": "0000000" },
                "f5tts": { "file": "voices/f5.pt", "model": "1" }
            },
            "ref": { "file": "voices/ref.wav", "text": "Чай подан, сэр. Изволите ещё что-нибудь?" },
            "voiceOrigin": "synthetic"
        }] }
    })
}

/// The package on disk with its files (made up: no voice is real).
pub fn write_arthur(root: &Path, manifest: &Value) -> CharacterPackage {
    let dir = write_package(root, manifest, None);
    let assets = dir.join("assets");
    std::fs::create_dir_all(assets.join("samples")).unwrap();
    std::fs::create_dir_all(assets.join("voices")).unwrap();
    std::fs::write(assets.join("avatar.webp"), b"RIFF\0\0\0\0WEBPVP8 ").unwrap();
    std::fs::write(assets.join("samples/1.opus"), b"OggS").unwrap();
    std::fs::write(assets.join("voices/omnivoice.pt"), b"snapshot omnivoice").unwrap();
    std::fs::write(assets.join("voices/cosyvoice3.pt"), b"snapshot cosyvoice3").unwrap();
    std::fs::write(assets.join("voices/ref.wav"), b"RIFF wav").unwrap();
    let (id, version) = (manifest["id"].as_str().unwrap(), manifest["version"].as_str().unwrap());
    match read_package(&dir, id, version) {
        Candidate::Ready(package) => crate::plugins::character_package(&package),
        Candidate::Broken { problems, .. } => panic!("{problems:?}"),
    }
}

fn with(mut manifest: Value, pointer: &str, value: Value) -> Value {
    *manifest.pointer_mut(pointer).unwrap_or_else(|| panic!("{pointer}")) = value;
    manifest
}

fn problem(manifest: Value) -> String {
    let root = tempfile::tempdir().unwrap();
    let package = write_arthur(root.path(), &manifest);
    match read(&package).remove(0) {
        Err(rejected) => rejected.problem,
        Ok(character) => panic!("accepted: {character:?}"),
    }
}

#[test]
fn reads_the_plans_character_and_skips_an_engine_it_does_not_know() {
    let root = tempfile::tempdir().unwrap();
    let package = write_arthur(root.path(), &arthur_manifest("1.2.0"));
    let character = read(&package).remove(0).unwrap();
    assert_eq!((character.id.as_str(), character.title.as_str()), ("arthur:arthur", "Артур"));
    assert_eq!(character.gender, Some(Gender::Male));
    assert_eq!(character.avatar.as_deref(), Some("avatar.webp"));
    assert_eq!(character.voices.keys().collect::<Vec<_>>(), ["cosyvoice3", "omnivoice"]);
    assert_eq!(character.skipped, ["f5tts"], "an engine this gateway does not know is skipped");
    assert_eq!(character.voices["omnivoice"].model.as_deref(), Some("c5fdb5cc"));
    assert_eq!(character.reference.as_ref().unwrap().file, "voices/ref.wav");
    assert_eq!(character.tones[0].default, 1);
    assert_eq!(character.license.as_deref(), Some("CC BY-NC 4.0, некоммерчески"));
    assert!(character.speaks());
    let cat = tempfile::tempdir().unwrap();
    let bublik = with(arthur_manifest("1.0.0"), "/contributes/characters/0/gender", json!("any"));
    assert_eq!(read(&write_arthur(cat.path(), &bublik)).remove(0).unwrap().gender, Some(Gender::Any));
}

#[test]
fn composes_the_prompt_with_the_tones_chosen_or_their_defaults() {
    let root = tempfile::tempdir().unwrap();
    let character = read(&write_arthur(root.path(), &arthur_manifest("1.2.0"))).remove(0).unwrap();
    assert_eq!(character.text(None), "Ты — Артур, дворецкий умной колонки.\nШутишь сухо и к месту.");
    let much = BTreeMap::from([("irony".to_string(), 2)]);
    assert!(character.text(Some(&much)).ends_with("\nШутишь при каждом удобном случае."));
    let wrong = BTreeMap::from([("irony".to_string(), 9), ("nothing".to_string(), 0)]);
    assert_eq!(character.positions(Some(&wrong)), BTreeMap::from([("irony".to_string(), 1)]));
}

#[test]
fn takes_a_voice_of_no_known_origin_and_keeps_the_word() {
    let root = tempfile::tempdir().unwrap();
    let manifest = with(arthur_manifest("1.0.0"), "/contributes/characters/0/voiceOrigin", json!("unknown"));
    let found = read(&write_arthur(root.path(), &manifest));
    assert_eq!(found[0].as_ref().unwrap().voice_origin.as_deref(), Some("unknown"));
}

#[test]
fn hides_a_character_that_does_not_check_out_and_says_why() {
    let cases = [
        (with(arthur_manifest("1.0.0"), "/contributes/characters/0/prompt", json!("")), "нет prompt"),
        (
            with(arthur_manifest("1.0.0"), "/contributes/characters/0/prompt", json!("а".repeat(MAX_PROMPT + 1))),
            "prompt: до 4000",
        ),
        (with(arthur_manifest("1.0.0"), "/contributes/characters/0/avatar", json!("../secret.png")), "без «..»"),
        (with(arthur_manifest("1.0.0"), "/contributes/characters/0/avatar", json!("avatar.svg")), "webp, png"),
        (with(arthur_manifest("1.0.0"), "/contributes/characters/0/avatar", json!("missing.png")), "нет файла"),
        (with(arthur_manifest("1.0.0"), "/contributes/characters/0/color", json!("blue")), "color"),
        (with(arthur_manifest("1.0.0"), "/contributes/characters/0/gender", json!("robot")), "gender"),
        (with(arthur_manifest("1.0.0"), "/contributes/characters/0/voiceOrigin", json!("actor")), "voiceOrigin"),
        (with(arthur_manifest("1.0.0"), "/contributes/characters/0/tones/0/default", json!(5)), "default"),
        (with(arthur_manifest("1.0.0"), "/contributes/characters/0/ref/text", json!("")), "слова образца"),
        (with(arthur_manifest("1.0.0"), "/contributes/characters/0/id", json!("Артур")), "id:"),
        (with(arthur_manifest("1.0.0"), "/contributes/characters", json!([])), "непустой список"),
    ];
    for (manifest, expected) in cases {
        let found = problem(manifest);
        assert!(found.contains(expected), "{expected}: {found}");
    }
    let mut anonymous = arthur_manifest("1.0.0");
    anonymous["contributes"]["characters"][0].as_object_mut().unwrap().remove("voiceOrigin");
    assert!(problem(anonymous).contains("voiceOrigin"), "whose voice it is must be said");
    // The rest of the package's characters stand.
    let mut two = arthur_manifest("1.0.0");
    let mut second = two["contributes"]["characters"][0].clone();
    second["id"] = json!("arthur-junior");
    second["prompt"] = json!("");
    two["contributes"]["characters"].as_array_mut().unwrap().push(second);
    let root = tempfile::tempdir().unwrap();
    let found = read(&write_arthur(root.path(), &two));
    assert!(found[0].is_ok());
    assert_eq!(found[1].as_ref().unwrap_err().id, "arthur:arthur-junior");
}
