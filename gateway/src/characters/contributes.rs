//! A character package's `contributes.characters` (docs/plans/characters.md, section 1), read and checked here — the
//! plugin host keeps it as it is. A character that does not check out is not offered: its package stays installed,
//! and the reason is told with it. Every file it names is inside the package's `assets/`.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use crate::plugins::manifest::{valid_id, PackageKind};
use crate::plugins::registry::Package;

/// The engines a character's voice may be given for (voice_lab's ENGINE_IDS): others are skipped.
pub const KNOWN_ENGINES: &[&str] = &["omnivoice", "cosyvoice3", "voxcpm"];

const MAX_CHARACTERS: usize = 10;
pub const MAX_PROMPT: usize = 4000;
const MAX_TONES: usize = 8;
const MAX_OPTIONS: usize = 5;
const MAX_LINE: usize = 300;
const MAX_SAMPLES: usize = 3;
const MAX_AVATAR: u64 = 256 * 1024;
const MAX_SAMPLE: u64 = 64 * 1024;
const MAX_SNAPSHOT: u64 = 4 * 1024 * 1024;
const MAX_REFERENCE: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Gender {
    Male,
    Female,
    /// Neither (a cat): the fallback voice is its own `fallbackVoice` only.
    Any,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToneOption {
    pub label: String,
    #[serde(skip)]
    pub line: String,
}

/// A regulator: each position puts one line into the prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Tone {
    pub id: String,
    pub title: String,
    /// The position chosen when nothing else is.
    pub default: usize,
    pub options: Vec<ToneOption>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sample {
    pub text: String,
    /// A path in assets/.
    pub file: String,
}

/// A voice snapshot for one engine: a file in assets/ and the weights' revision it was taken with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub file: String,
    pub model: Option<String>,
}

/// The recording a snapshot is taken from again, and its words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    pub file: String,
    pub text: String,
}

/// A character of a package, checked.
#[derive(Debug, Clone, PartialEq)]
pub struct Character {
    /// `<package>:<character>`.
    pub id: String,
    pub local: String,
    pub package: String,
    pub version: String,
    pub license: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub gender: Option<Gender>,
    pub color: Option<String>,
    pub avatar: Option<String>,
    pub prompt: String,
    pub tones: Vec<Tone>,
    pub samples: Vec<Sample>,
    pub fallback_voice: Option<String>,
    /// By engine id, the known engines only.
    pub voices: BTreeMap<String, Snapshot>,
    /// Engines this gateway does not know, skipped.
    pub skipped: Vec<String>,
    pub reference: Option<Reference>,
    pub voice_origin: Option<String>,
    /// The package's assets/.
    pub assets: PathBuf,
}

impl Character {
    /// A file of its package's assets.
    pub fn asset(&self, path: &str) -> PathBuf {
        self.assets.join(path)
    }

    /// The tones' positions by id: the chosen ones that fit, the defaults for the rest.
    pub fn positions(&self, chosen: Option<&BTreeMap<String, usize>>) -> BTreeMap<String, usize> {
        self.tones
            .iter()
            .map(|tone| {
                let position = chosen
                    .and_then(|chosen| chosen.get(&tone.id))
                    .copied()
                    .filter(|position| *position < tone.options.len())
                    .unwrap_or(tone.default);
                (tone.id.clone(), position)
            })
            .collect()
    }

    /// Its text with these tones: the prompt, then a line for each tone's position.
    pub fn text(&self, chosen: Option<&BTreeMap<String, usize>>) -> String {
        compose(&self.prompt, &self.tones, &self.positions(chosen))
    }

    /// Whether it has a voice of its own: a snapshot for an engine this gateway knows, or a reference to take one.
    pub fn speaks(&self) -> bool {
        !self.voices.is_empty() || self.reference.is_some()
    }
}

/// A character that did not check out: shown with its reason, never offered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejected {
    pub id: String,
    pub package: String,
    pub version: String,
    pub title: String,
    pub problem: String,
}

/// The prompt and a line for each tone's position (positions out of range take the tone's default).
pub fn compose(prompt: &str, tones: &[Tone], positions: &BTreeMap<String, usize>) -> String {
    let mut text = prompt.trim().to_string();
    for tone in tones {
        let position = positions.get(&tone.id).copied().filter(|position| *position < tone.options.len());
        let Some(option) = tone.options.get(position.unwrap_or(tone.default)) else { continue };
        let line = option.line.trim();
        if !line.is_empty() {
            text.push('\n');
            text.push_str(line);
        }
    }
    text
}

/// A path in the package's assets/: relative, no `..`, no hidden part; the file there, of one of the kinds and no
/// bigger than `max`.
fn asset(assets: &Path, value: Option<&Value>, what: &str, kinds: &[&str], max: u64) -> Result<String, String> {
    let path = value.and_then(Value::as_str).map(str::trim).filter(|path| !path.is_empty());
    let path = path.ok_or_else(|| crate::tr!("{what}: путь к файлу в assets/", what = what))?;
    let parts: Vec<Component> = Path::new(path).components().collect();
    let inside =
        parts.iter().all(|part| matches!(part, Component::Normal(name) if !name.to_string_lossy().starts_with('.')));
    if !inside || path.contains('\\') {
        return Err(crate::tr!("{what}: «{path}» — путь внутри assets/, без «..»", what = what, path = path));
    }
    let extension = Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if !kinds.contains(&extension.as_str()) {
        return Err(crate::tr!(
            "{what}: «{path}» — нужен файл {kinds}",
            what = what,
            path = path,
            kinds = kinds.join(", ")
        ));
    }
    let file = assets.join(path);
    let meta = std::fs::symlink_metadata(&file)
        .map_err(|_| crate::tr!("{what}: нет файла «{path}» в assets/", what = what, path = path))?;
    if !meta.is_file() {
        return Err(crate::tr!("{what}: «{path}» — не файл", what = what, path = path));
    }
    if meta.len() > max {
        return Err(crate::tr!("{what}: «{path}» больше {size} КБ", what = what, path = path, size = max / 1024));
    }
    Ok(path.to_string())
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::trim).filter(|text| !text.is_empty()).map(str::to_string)
}

/// A tone's id: as a character's.
fn tone_id(id: &str) -> bool {
    valid_id(id)
}

fn tones(value: Option<&Value>, problems: &mut Vec<String>) -> Vec<Tone> {
    let Some(value) = value else { return Vec::new() };
    let Some(items) = value.as_array() else {
        problems.push(crate::tr!("tones: список регуляторов"));
        return Vec::new();
    };
    if items.len() > MAX_TONES {
        problems.push(crate::tr!("tones: не больше {max}", max = MAX_TONES));
    }
    let mut found: Vec<Tone> = Vec::new();
    for (index, item) in items.iter().enumerate().take(MAX_TONES) {
        let at = format!("tones[{index}]");
        let id = text(item, "id").unwrap_or_default();
        if !tone_id(&id) {
            problems.push(crate::tr!("{at}: id — строчные латинские буквы, цифры и «-»", at = at));
            continue;
        }
        if found.iter().any(|tone| tone.id == id) {
            problems.push(crate::tr!("{at}: id «{id}» дважды", at = at, id = id));
            continue;
        }
        let title: String = text(item, "title").unwrap_or_else(|| id.clone()).chars().take(40).collect();
        let options: Vec<ToneOption> = match item.get("options").and_then(Value::as_array) {
            Some(options) if (2..=MAX_OPTIONS).contains(&options.len()) => options
                .iter()
                .map(|option| ToneOption {
                    label: text(option, "label").unwrap_or_default().chars().take(30).collect(),
                    line: option.get("line").and_then(Value::as_str).unwrap_or("").trim().to_string(),
                })
                .collect(),
            _ => {
                problems.push(crate::tr!("{at}: options — от 2 до {max} положений", at = at, max = MAX_OPTIONS));
                continue;
            }
        };
        if options.iter().any(|option| option.label.is_empty()) {
            problems.push(crate::tr!("{at}: у каждого положения нужна label", at = at));
            continue;
        }
        if options.iter().any(|option| option.line.chars().count() > MAX_LINE) {
            problems.push(crate::tr!("{at}: строка положения — до {max} знаков", at = at, max = MAX_LINE));
            continue;
        }
        let default = match item.get("default") {
            None => 0,
            Some(position) => match position.as_u64().map(|position| position as usize) {
                Some(position) if position < options.len() => position,
                _ => {
                    problems.push(crate::tr!(
                        "{at}: default — номер положения от 0 до {last}",
                        at = at,
                        last = options.len() - 1
                    ));
                    continue;
                }
            },
        };
        found.push(Tone { id, title, default, options });
    }
    found
}

/// One character of a package, or why it is not one.
fn character(package: &Package, value: &Value, index: usize) -> Result<Character, Rejected> {
    let manifest = &package.manifest;
    let assets = package.assets();
    let local = text(value, "id").filter(|id| valid_id(id));
    let id = format!("{}:{}", manifest.id, local.clone().unwrap_or_else(|| index.to_string()));
    let title: String = text(value, "title").unwrap_or_else(|| manifest.title.clone()).chars().take(80).collect();
    let rejected = |problems: Vec<String>| Rejected {
        id: id.clone(),
        package: manifest.id.clone(),
        version: manifest.version.to_string(),
        title: title.clone(),
        problem: problems.join("; "),
    };
    let mut problems = Vec::new();
    if local.is_none() {
        problems.push(crate::tr!("id: строчные латинские буквы, цифры и «-», с буквы"));
    }
    let description = text(value, "description")
        .or_else(|| manifest.description.clone())
        .map(|description| description.chars().take(500).collect());
    let prompt = text(value, "prompt").unwrap_or_default();
    if prompt.is_empty() {
        problems.push(crate::tr!("нет prompt"));
    } else if prompt.chars().count() > MAX_PROMPT {
        problems.push(crate::tr!("prompt: до {max} знаков", max = MAX_PROMPT));
    }
    let gender = match text(value, "gender").as_deref() {
        None => None,
        Some("male") => Some(Gender::Male),
        Some("female") => Some(Gender::Female),
        Some("any" | "neutral") => Some(Gender::Any),
        Some(other) => {
            problems.push(crate::tr!("gender «{other}»: male, female или any", other = other));
            None
        }
    };
    let color = text(value, "color");
    let hex = |color: &str| {
        color.strip_prefix('#').is_some_and(|digits| {
            matches!(digits.len(), 3 | 6) && digits.chars().all(|digit| digit.is_ascii_hexdigit())
        })
    };
    if color.as_deref().is_some_and(|color| !hex(color)) {
        problems.push(crate::tr!("color: цвет вида #7A8CA5"));
    }
    let avatar = match value.get("avatar") {
        None => None,
        // No SVG: a picture that runs script on the panel's origin.
        avatar => match asset(&assets, avatar, "avatar", &["webp", "png"], MAX_AVATAR) {
            Ok(path) => Some(path),
            Err(problem) => {
                problems.push(problem);
                None
            }
        },
    };
    let tones = tones(value.get("tones"), &mut problems);
    let mut samples = Vec::new();
    match value.get("samples") {
        None => {}
        Some(Value::Array(items)) if items.len() <= MAX_SAMPLES => {
            for (index, item) in items.iter().enumerate() {
                let what = format!("samples[{index}]");
                match asset(&assets, item.get("file"), &what, &["opus", "ogg"], MAX_SAMPLE) {
                    Ok(file) => samples.push(Sample {
                        text: text(item, "text").unwrap_or_default().chars().take(200).collect(),
                        file,
                    }),
                    Err(problem) => problems.push(problem),
                }
            }
        }
        Some(_) => problems.push(crate::tr!("samples: не больше {max} фраз", max = MAX_SAMPLES)),
    }
    let fallback_voice = text(value, "fallbackVoice");
    if fallback_voice.as_deref().is_some_and(|voice| {
        voice.len() > 100 || !voice.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:".contains(c))
    }) {
        problems.push(crate::tr!("fallbackVoice: id карточки голоса, например piper-denis"));
    }
    let mut voices = BTreeMap::new();
    let mut skipped = Vec::new();
    match value.get("voices") {
        None => {}
        Some(Value::Object(by_engine)) => {
            for (engine, snapshot) in by_engine {
                if !KNOWN_ENGINES.contains(&engine.as_str()) {
                    skipped.push(engine.clone());
                    continue;
                }
                match asset(&assets, snapshot.get("file"), &format!("voices.{engine}"), &["pt"], MAX_SNAPSHOT) {
                    Ok(file) => {
                        voices.insert(engine.clone(), Snapshot { file, model: text(snapshot, "model") });
                    }
                    Err(problem) => problems.push(problem),
                }
            }
        }
        Some(_) => problems.push(crate::tr!("voices: снимки по id модели")),
    }
    let reference = match value.get("ref") {
        None => None,
        Some(reference) => {
            let file = asset(&assets, reference.get("file"), "ref", &["wav"], MAX_REFERENCE);
            match (file, text(reference, "text")) {
                (Ok(file), Some(words)) => Some(Reference { file, text: words.chars().take(500).collect() }),
                (Err(problem), _) => {
                    problems.push(problem);
                    None
                }
                (Ok(_), None) => {
                    problems.push(crate::tr!("ref: нужен text — слова образца"));
                    None
                }
            }
        }
    };
    let voice_origin = text(value, "voiceOrigin");
    let speaks = value.get("voices").is_some() || value.get("ref").is_some();
    match voice_origin.as_deref() {
        // `unknown`: no one checked whose it is — only from an own source or by hand (docs/plans/custom-feeds.md); the
        // panel says so beside the character.
        Some("synthetic" | "consent" | "unknown") => {}
        Some(other) => {
            problems.push(crate::tr!("voiceOrigin «{other}»: synthetic, consent или unknown", other = other))
        }
        // A voice of someone who never agreed is not taken: whose it is must be said.
        None if speaks => {
            problems.push(crate::tr!("voiceOrigin: synthetic (голос по описанию) или consent (диктор с согласием)"))
        }
        None => {}
    }
    if !problems.is_empty() {
        return Err(rejected(problems));
    }
    Ok(Character {
        id,
        local: local.unwrap_or_default(),
        package: manifest.id.clone(),
        version: manifest.version.to_string(),
        license: manifest.license.clone(),
        title,
        description,
        gender,
        color,
        avatar,
        prompt,
        tones,
        samples,
        fallback_voice,
        voices,
        skipped,
        reference,
        voice_origin,
        assets,
    })
}

/// The characters of a package of the kind `character`: each checked, or why not.
pub fn read(package: &Package) -> Vec<Result<Character, Rejected>> {
    let manifest = &package.manifest;
    if manifest.kind != PackageKind::Character {
        return Vec::new();
    }
    let whole = |problem: String| {
        vec![Err(Rejected {
            id: manifest.id.clone(),
            package: manifest.id.clone(),
            version: manifest.version.to_string(),
            title: manifest.title.clone(),
            problem: problem.to_string(),
        })]
    };
    let Some(list) = manifest.contributes.characters.as_ref() else {
        return whole(crate::tr!("нет contributes.characters"));
    };
    let Some(items) = list.as_array().filter(|items| !items.is_empty()) else {
        return whole(crate::tr!("contributes.characters: непустой список персонажей"));
    };
    let mut found: Vec<Result<Character, Rejected>> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let mut checked = if index >= MAX_CHARACTERS {
            let id = format!("{}:{index}", manifest.id);
            Err(Rejected {
                id,
                package: manifest.id.clone(),
                version: manifest.version.to_string(),
                title: manifest.title.clone(),
                problem: crate::tr!("не больше {max} персонажей в пакете", max = MAX_CHARACTERS),
            })
        } else {
            character(package, item, index)
        };
        let id = match &checked {
            Ok(character) => character.id.clone(),
            Err(rejected) => rejected.id.clone(),
        };
        let twice = found.iter().any(|known| match known {
            Ok(known) => known.id == id,
            Err(known) => known.id == id,
        });
        if twice {
            checked = Err(Rejected {
                id: format!("{id}#{index}"),
                package: manifest.id.clone(),
                version: manifest.version.to_string(),
                title: manifest.title.clone(),
                problem: crate::tr!("персонаж «{id}» дважды", id = id),
            });
        }
        found.push(checked);
    }
    found
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::plugins::registry::{read_package, tests::write_package, Candidate};
    use serde_json::json;
    use std::sync::Arc;

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
    pub fn write_arthur(root: &Path, manifest: &Value) -> Arc<Package> {
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
            Candidate::Ready(package) => package,
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
}
