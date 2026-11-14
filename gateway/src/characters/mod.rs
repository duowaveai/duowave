//! Characters (docs/plans/characters.md, section 4): who answers — a character and its voice, chosen in one go. The
//! built-in one is Alisa (`alice`) with regulators of how she speaks — the cards of before (Стандартная, Ироничная,
//! Тёплая, Деловая) are her positions, the butler is the catalog's Артур; a character a package of the kind `character`
//! brings is `<package>:<character>` and joins her in the `characters` table, so the persona's choice by id works for
//! both.
//! The table follows the installed packages: synced at start and whenever a package comes, goes or is updated.
//!
//! A character's text is its prompt with a line for each tone's position; the gateway adds the line about the word
//! that wakes the station and the rules of speech (src/dialog/system_prompt.rs). What it said when chosen is kept with
//! the choice: an owner's edit is never lost, and a newer text of its package is told, never applied unasked.
//!
//! Its voice is the computer's (`voices.rs`, the `pc` build): its package's snapshots laid out in the voice lab at
//! install, taken again from its reference when the engine's weights are other ones, and switched on through the
//! models — with what it needs when its engine is not here. The station's build says the voice is the computer's.

pub mod contributes;
pub mod controller;
#[cfg(feature = "pc")]
pub mod voices;

use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};

use rusqlite::params;
use serde::Serialize;
use serde_json::Value;

use crate::core::events::{Actor, EventBus, GatewayEvent};
use crate::dialog::persona_service::PersonaSource;
use crate::dialog::system_prompt::{
    alice_positions, alice_text, alice_tones, is_package_character, persona_presets, ALICE,
};
use crate::dialog::PersonaService;
use crate::platform::http::ApiError;
use crate::plugins::manifest::PackageKind;
use crate::plugins::PluginHost;
use crate::settings::definitions::Persona;
use crate::storage::Db;
use crate::utils::time::now_iso;
pub use contributes::{Character, Gender, Rejected, Tone};
pub use controller::routes;

/// A package's character, for the panel's link to its files.
pub fn media_url(package: &str, path: &str) -> String {
    let parts: Vec<String> = path.split('/').map(crate::skills::js::encode_uri_component).collect();
    format!("/media/packages/{package}/{}", parts.join("/"))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageRef {
    pub id: String,
    pub version: String,
    pub license: Option<String>,
    pub verified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SampleView {
    pub text: String,
    pub url: String,
}

/// A character's voice as it stands here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum VoiceState {
    /// Its own voice speaks when chosen (its engine is here).
    Ready,
    /// Its engine is not here: `needs` says what to download.
    Needs,
    /// Its engine is being installed, or its snapshot taken.
    Installing,
    /// No engine of its runs here, and no fallback voice is known.
    Unsupported,
    /// Its own voice cannot be here: the fallback speaks.
    Fallback,
    /// A character without a voice of its own: the voice stays as it is.
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FallbackView {
    pub card_id: String,
    pub title: String,
    /// Its file is here: it can speak at once.
    pub installed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceView {
    pub state: VoiceState,
    /// The engine its voice speaks with, or would.
    pub engine: Option<String>,
    /// The voice's card in the models (`own:c-<package>-<character>-<engine>`).
    pub card_id: Option<String>,
    pub fallback: Option<FallbackView>,
    /// What it needs first, as a voice card's `needs`.
    pub needs: Option<Value>,
    pub progress: Option<u32>,
    pub reason: Option<String>,
}

impl VoiceView {
    pub fn none() -> VoiceView {
        VoiceView {
            state: VoiceState::None,
            engine: None,
            card_id: None,
            fallback: None,
            needs: None,
            progress: None,
            reason: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CharacterView {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub badge: Option<String>,
    pub builtin: bool,
    pub package: Option<PackageRef>,
    pub gender: Option<Gender>,
    pub color: Option<String>,
    /// `/media/packages/<package>/<path>`.
    pub avatar: Option<String>,
    pub samples: Vec<SampleView>,
    pub tones: Vec<Tone>,
    pub voice: VoiceView,
    /// Why it is not offered: its package's `contributes.characters` does not check out.
    pub problem: Option<String>,
    /// Whose voice it is, as its package says: synthetic, consent, unknown (no one checked — the panel says so).
    pub voice_origin: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveView {
    /// The card that speaks; null — one's own text or the prompt file.
    pub id: Option<String>,
    pub source: PersonaSource,
    /// The text said was edited by hand.
    pub edited: bool,
    /// Its package has a newer text for it — that text (with the tones chosen), applied by choosing it again; false
    /// when there is none.
    pub update: Value,
    pub tones: BTreeMap<String, usize>,
    /// The card chosen before is a catalog's character now (the butler → `arthur`): the package to offer.
    pub replaced_by: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CharactersView {
    pub active: ActiveView,
    pub characters: Vec<CharacterView>,
}

/// What became of a choice's voice.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChosenVoice {
    pub state: VoiceState,
    pub needs: Option<Value>,
    /// The voice waiting for its engine or its snapshot (the models' `ttsPending`).
    pub pending: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChooseAnswer {
    pub character: CharacterView,
    pub voice: ChosenVoice,
}

/// What a choice asks: the tones' positions, and for the voice — whether to touch it at all (`voice`), download its
/// engine (`install`), start it on tight memory (`anyway`).
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct ChooseBody {
    #[serde(default)]
    pub tones: Option<BTreeMap<String, usize>>,
    #[serde(default)]
    pub install: bool,
    #[serde(default)]
    pub anyway: bool,
    #[serde(default)]
    pub voice: Option<bool>,
}

pub struct CharactersService {
    db: Arc<Db>,
    events: EventBus,
    personas: Arc<PersonaService>,
    host: Arc<PluginHost>,
    /// The packages' characters as last read.
    known: RwLock<Vec<Result<Character, Rejected>>>,
    #[cfg(feature = "pc")]
    voices: std::sync::OnceLock<Arc<voices::CharacterVoices>>,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl CharactersService {
    pub fn new(db: Arc<Db>, events: EventBus, personas: Arc<PersonaService>, host: Arc<PluginHost>) -> Arc<Self> {
        let service = Arc::new(CharactersService {
            db,
            events,
            personas,
            host,
            known: RwLock::new(Vec::new()),
            #[cfg(feature = "pc")]
            voices: std::sync::OnceLock::new(),
            tasks: Mutex::new(Vec::new()),
        });
        service.sync();
        service
    }

    /// The voice lab and the models: the characters' voices (a PC's build).
    #[cfg(feature = "pc")]
    pub fn link_voices(&self, voices: Arc<voices::CharacterVoices>) {
        if self.voices.set(voices).is_err() {
            tracing::warn!(target: "characters", "the voices are linked already");
        }
    }

    #[cfg(feature = "pc")]
    fn voices(&self) -> Option<Arc<voices::CharacterVoices>> {
        self.voices.get().cloned()
    }

    /// The packages followed: their characters synced whenever one comes, goes or is updated.
    pub fn start(self: &Arc<Self>) {
        self.sync();
        let me = Arc::downgrade(self);
        let task = self.events.on("characters", move |event| {
            if let GatewayEvent::PackagesChanged { change, .. } = &*event {
                // Hidden or shown again by its feed (docs/plans/custom-feeds.md): its characters go or come back.
                if matches!(change.as_str(), "installed" | "updated" | "removed" | "hidden" | "shown") {
                    if let Some(characters) = me.upgrade() {
                        characters.sync();
                    }
                }
            }
        });
        self.tasks.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(task);
    }

    pub fn shutdown(&self) {
        for task in self.tasks.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).drain(..) {
            task.abort();
        }
    }

    fn known(&self) -> Vec<Result<Character, Rejected>> {
        self.known.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    /// A package's character by its full id, checked.
    pub fn character(&self, id: &str) -> Option<Character> {
        self.known().into_iter().filter_map(Result::ok).find(|character| character.id == id)
    }

    /// The installed packages' characters read again: the table follows them (rows of packages gone go), and their
    /// voices are laid out in the voice lab or removed with them. A package its feed hides counts as gone, and more: its
    /// character that was chosen gives the place to Alisa, its voice that speaks to a fallback.
    pub fn sync(&self) {
        self.host.rescan();
        let mut found = Vec::new();
        for package in self.host.visible_all() {
            if package.manifest.kind == PackageKind::Character {
                found.extend(contributes::read(&package));
            }
        }
        self.step_aside_hidden();
        for rejected in found.iter().filter_map(|found| found.as_ref().err()) {
            tracing::warn!(target: "characters", "{} {}: not offered — {}", rejected.id, rejected.version, rejected.problem);
        }
        let valid: Vec<Character> = found.iter().filter_map(|found| found.as_ref().ok().cloned()).collect();
        if let Err(error) = self.store(&valid) {
            tracing::warn!(target: "characters", "the characters' table is not synced: {error}");
        }
        #[cfg(feature = "pc")]
        if let Some(voices) = self.voices() {
            voices.lay_out(&valid);
        }
        *self.known.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = found;
    }

    /// The table: a row for each character of a package (its text with the default tones), none for a package gone.
    /// A row is written only when something of it changed. The list is read again at every look at it (`view` — a
    /// panel's dashboard looks every 15 s), and a row written anew each time, with its time, moved the settings
    /// revision: the computer of the pair took a copy of the station's settings every minute, its ten copies held the
    /// last ten minutes only, and with the settings on the computer it would send them — the station's gateway starting
    /// again on each (live, 11.11).
    fn store(&self, valid: &[Character]) -> rusqlite::Result<()> {
        let at = now_iso();
        self.db.transaction(|t| {
            for character in valid {
                let tones = serde_json::to_string(&character.tones).unwrap_or_else(|_| "[]".into());
                t.execute(
                    "INSERT INTO characters (id, title, description, badge, text, updated_at, package, version, color, tones, avatar)
                     VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                     ON CONFLICT (id) DO UPDATE SET title = excluded.title, description = excluded.description,
                     text = excluded.text, updated_at = excluded.updated_at, package = excluded.package,
                     version = excluded.version, color = excluded.color, tones = excluded.tones, avatar = excluded.avatar
                     WHERE characters.title IS NOT excluded.title OR characters.description IS NOT excluded.description
                     OR characters.text IS NOT excluded.text OR characters.package IS NOT excluded.package
                     OR characters.version IS NOT excluded.version OR characters.color IS NOT excluded.color
                     OR characters.tones IS NOT excluded.tones OR characters.avatar IS NOT excluded.avatar",
                    params![
                        character.id,
                        character.title,
                        character.description.clone().unwrap_or_default(),
                        character.text(None),
                        at,
                        character.package,
                        character.version,
                        character.color,
                        tones,
                        character.avatar,
                    ],
                )?;
            }
            let ids: HashSet<&str> = valid.iter().map(|character| character.id.as_str()).collect();
            let stored: Vec<String> = {
                let mut statement = t.prepare("SELECT id FROM characters WHERE package IS NOT NULL")?;
                let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            for id in stored.iter().filter(|id| !ids.contains(id.as_str())) {
                t.execute("DELETE FROM characters WHERE id = ?1", params![id])?;
            }
            Ok(())
        })
    }

    /// The characters of the packages their feeds hide: the chosen one of them gives the place to Alisa (in the
    /// positions she had), and the voice of one that speaks gives it to a fallback.
    fn step_aside_hidden(&self) {
        let hidden: Vec<Character> = self
            .host
            .hidden_all()
            .into_iter()
            .filter(|package| package.manifest.kind == PackageKind::Character)
            .flat_map(|package| contributes::read(&package))
            .filter_map(Result::ok)
            .collect();
        if hidden.is_empty() {
            return;
        }
        let chosen = self.personas.chosen().and_then(|chosen| chosen.preset_id);
        if let Some(id) = chosen.filter(|id| hidden.iter().any(|character| character.id == *id)) {
            let mut body = serde_json::Map::new();
            body.insert("presetId".into(), Value::String(ALICE.into()));
            match self.personas.set(&body, None) {
                Ok(()) => tracing::info!(target: "characters", "{id}: hidden by its feed — Alisa answers now"),
                Err(error) => {
                    tracing::warn!(target: "characters", "{id}: hidden, but Alisa is not chosen: {}", error.message)
                }
            }
        }
        #[cfg(feature = "pc")]
        if let Some(voices) = self.voices() {
            voices.step_aside(&hidden);
        }
    }

    fn package_ref(&self, package: &str, version: &str, license: Option<String>) -> PackageRef {
        let (_, verified) = self.host.origin(package);
        PackageRef { id: package.to_string(), version: version.to_string(), license, verified }
    }

    async fn voice_of(&self, character: &Character) -> VoiceView {
        #[cfg(feature = "pc")]
        if let Some(voices) = self.voices() {
            return voices.plan(character).await;
        }
        station_voice(character)
    }

    async fn view_of(&self, character: &Character) -> CharacterView {
        CharacterView {
            id: character.id.clone(),
            title: character.title.clone(),
            description: character.description.clone(),
            badge: None,
            builtin: false,
            package: Some(self.package_ref(&character.package, &character.version, character.license.clone())),
            gender: character.gender,
            color: character.color.clone(),
            avatar: character.avatar.as_ref().map(|avatar| media_url(&character.package, avatar)),
            samples: character
                .samples
                .iter()
                .map(|sample| SampleView {
                    text: sample.text.clone(),
                    url: media_url(&character.package, &sample.file),
                })
                .collect(),
            tones: character.tones.clone(),
            voice: self.voice_of(character).await,
            problem: None,
            voice_origin: character.voice_origin.clone(),
        }
    }

    fn rejected_view(&self, rejected: &Rejected) -> CharacterView {
        CharacterView {
            id: rejected.id.clone(),
            title: rejected.title.clone(),
            description: None,
            badge: None,
            builtin: false,
            package: Some(self.package_ref(&rejected.package, &rejected.version, None)),
            gender: None,
            color: None,
            avatar: None,
            samples: Vec::new(),
            tones: Vec::new(),
            voice: VoiceView::none(),
            problem: Some(rejected.problem.clone()),
            voice_origin: None,
        }
    }

    /// The built-in cards as the panel shows them now (the talk without a station names the built-in one its way).
    fn builtin_views(&self) -> Vec<CharacterView> {
        persona_presets()
            .into_iter()
            .map(|preset| self.personas.shown(preset))
            .map(|preset| CharacterView {
                gender: (preset.id == ALICE).then_some(Gender::Female),
                tones: if preset.id == ALICE { alice_tones() } else { Vec::new() },
                id: preset.id,
                title: preset.title,
                description: Some(preset.description),
                badge: preset.badge,
                builtin: true,
                package: None,
                color: None,
                avatar: None,
                samples: Vec::new(),
                voice: VoiceView::none(),
                problem: None,
                voice_origin: None,
            })
            .collect()
    }

    /// Who answers now: the card, whether its text was edited, whether its package has a newer text.
    fn active(&self) -> Result<ActiveView, ApiError> {
        let view = self.personas.view()?;
        let chosen = self.personas.chosen();
        let edited =
            chosen.as_ref().is_some_and(|chosen| chosen.base.as_ref().is_some_and(|base| *base != chosen.text));
        let newer = |base: Option<&String>, now: String| match base {
            Some(base) if *base != now => Value::String(now),
            _ => Value::Bool(false),
        };
        let (update, tones) = match (view.active_preset.as_deref(), &chosen) {
            (Some(id), Some(chosen)) if is_package_character(id) => match self.character(id) {
                Some(character) => (
                    newer(chosen.base.as_ref(), character.text(chosen.tones.as_ref())),
                    character.positions(chosen.tones.as_ref()),
                ),
                None => (Value::Bool(false), chosen.tones.clone().unwrap_or_default()),
            },
            (Some(ALICE), Some(chosen)) => {
                let positions = alice_positions(chosen.tones.as_ref());
                (newer(chosen.base.as_ref(), alice_text(Some(&positions))), positions)
            }
            (Some(ALICE), None) => (Value::Bool(false), alice_positions(None)),
            (Some(id), Some(chosen)) => {
                let card = persona_presets().into_iter().find(|preset| preset.id == id);
                let update = card.map_or(Value::Bool(false), |card| newer(chosen.base.as_ref(), card.text));
                (update, BTreeMap::new())
            }
            _ => (Value::Bool(false), BTreeMap::new()),
        };
        let replaced_by = self.personas.replaced_by();
        Ok(ActiveView { id: view.active_preset, source: view.source, edited, update, tones, replaced_by })
    }

    /// Every character: the built-in ones, then the packages' (those that do not check out with their reason).
    pub async fn view(&self) -> Result<CharactersView, ApiError> {
        self.sync();
        let mut characters = self.builtin_views();
        for found in self.known() {
            characters.push(match &found {
                Ok(character) => self.view_of(character).await,
                Err(rejected) => self.rejected_view(rejected),
            });
        }
        Ok(CharactersView { active: self.active()?, characters })
    }

    /// The character from the next answer on — at once — and its voice: switched on when it is here; what it needs
    /// when its engine is not (`install` downloads it and switches it on then); the fallback voice where it cannot
    /// be. `voice_allowed`: the caller may change the voice (`models.manage`).
    pub async fn choose(
        &self,
        id: &str,
        body: ChooseBody,
        voice_allowed: bool,
        actor: Option<Actor>,
    ) -> Result<ChooseAnswer, ApiError> {
        self.sync();
        let wants_voice = body.voice != Some(false);
        if let Some(rejected) = self.known().into_iter().filter_map(Result::err).find(|rejected| rejected.id == id) {
            return Err(ApiError::bad_request(crate::tr!(
                "Персонаж не выбирается: {problem}.",
                problem = rejected.problem
            )));
        }
        if let Some(character) = self.character(id) {
            if wants_voice && !voice_allowed && (character.speaks() || character.fallback_voice.is_some()) {
                return Err(ApiError::forbidden(
                    crate::tr!("Голос персонажа меняет тот, у кого право на голоса (models.manage); выберите без голоса: \"voice\": false."),
                ));
            }
            let positions = character.positions(body.tones.as_ref());
            let text = character.text(Some(&positions));
            let persona = Persona {
                preset_id: Some(character.id.clone()),
                text: text.clone(),
                base: Some(text),
                tones: Some(positions),
            };
            self.personas.choose(persona, actor.clone())?;
            tracing::info!(target: "characters", "{} answers now", character.id);
            let voice = if wants_voice {
                self.choose_voice(&character, &body, actor).await?
            } else {
                self.voice_left(&character).await
            };
            return Ok(ChooseAnswer { character: self.view_of(&character).await, voice });
        }
        let card = persona_presets().into_iter().find(|preset| preset.id == id);
        let Some(card) = card else {
            return Err(ApiError::not_found(crate::tr!("Нет персонажа {id}.", id = id)));
        };
        let persona = if card.id == ALICE {
            // Her voice is the one chosen: only the text and the positions change.
            let positions = alice_positions(body.tones.as_ref());
            let text = alice_text(Some(&positions));
            Persona { preset_id: Some(card.id.clone()), text: text.clone(), base: Some(text), tones: Some(positions) }
        } else {
            Persona { preset_id: Some(card.id.clone()), text: card.text.clone(), base: Some(card.text), tones: None }
        };
        self.personas.choose(persona, actor)?;
        let character = self.builtin_views().into_iter().find(|view| view.id == id).unwrap_or_else(|| unreachable!());
        Ok(ChooseAnswer { character, voice: ChosenVoice { state: VoiceState::None, needs: None, pending: None } })
    }

    /// The voice as it is, untouched: what the character's would need.
    async fn voice_left(&self, character: &Character) -> ChosenVoice {
        let plan = self.voice_of(character).await;
        ChosenVoice { state: plan.state, needs: plan.needs, pending: None }
    }

    #[cfg(feature = "pc")]
    async fn choose_voice(
        &self,
        character: &Character,
        body: &ChooseBody,
        actor: Option<Actor>,
    ) -> Result<ChosenVoice, ApiError> {
        match self.voices() {
            Some(voices) => voices.choose(character, body.install, body.anyway, actor).await,
            None => Ok(self.voice_left(character).await),
        }
    }

    #[cfg(not(feature = "pc"))]
    async fn choose_voice(
        &self,
        character: &Character,
        _body: &ChooseBody,
        _actor: Option<Actor>,
    ) -> Result<ChosenVoice, ApiError> {
        Ok(self.voice_left(character).await)
    }

    /// Before a package goes: its characters' voices go with it, and none of them may be the one speaking.
    pub fn before_removal(&self, package: &str) -> Result<(), ApiError> {
        #[cfg(feature = "pc")]
        if let Some(voices) = self.voices() {
            return voices.before_removal(package);
        }
        let _ = package;
        Ok(())
    }
}

/// Without the voice lab (the station's build): a character's own voice is the computer's; the fallback speaks.
fn station_voice(character: &Character) -> VoiceView {
    if !character.speaks() && character.fallback_voice.is_none() {
        return VoiceView::none();
    }
    let card =
        character.fallback_voice.clone().or_else(|| fallback_cards(character.gender).first().map(|id| id.to_string()));
    VoiceView {
        state: VoiceState::Fallback,
        engine: None,
        card_id: None,
        fallback: card.map(|card| FallbackView { title: card.clone(), card_id: card, installed: false }),
        needs: None,
        progress: None,
        reason: Some(crate::tr!("Голоса персонажей звучат с компьютера: их модели там.")),
    }
}

/// Voices like the character's, when its own fallback is not on this gateway: by its gender.
pub fn fallback_cards(gender: Option<Gender>) -> &'static [&'static str] {
    match gender {
        Some(Gender::Male) => &["piper-denis", "piper-dmitri", "piper-ruslan"],
        Some(Gender::Female) => &["piper-irina"],
        Some(Gender::Any) | None => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::contributes::tests::{arthur_manifest, write_arthur};
    use super::*;
    use crate::app::App;
    use crate::config::{Config, Env};
    use crate::dialog::system_prompt::WAKE_WORD_LINE;
    use serde_json::json;

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
        assert!(app.personas.system_prompt(None).text.contains(WAKE_WORD_LINE));

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
                .with(|c| {
                    c.query_row("SELECT updated_at FROM characters WHERE id = 'arthur:arthur'", [], |row| row.get(0))
                })
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
        crate::plugins::registry::set_installed(
            &app.db,
            "arthur",
            crate::plugins::registry::SOURCE_CATALOG,
            false,
            None,
        )
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
        assert!(prompt.contains(WAKE_WORD_LINE) && prompt.contains("Каждый ответ будет озвучен"));
        let active = app.characters.view().await.unwrap().active;
        assert_eq!(
            (active.id.as_deref(), active.edited, &active.update),
            (Some("arthur:arthur"), false, &json!(false))
        );
        assert_eq!(active.tones, BTreeMap::from([("irony".to_string(), 2)]));

        // The owner's edit: kept; a newer text in the package is told, not applied.
        app.personas
            .set(
                json!({"presetId": "arthur:arthur", "text": "Ты — Артур. Отвечай стихами."}).as_object().unwrap(),
                None,
            )
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
        assert!(!prompt.contains(WAKE_WORD_LINE), "Alisa needs no such line");
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
}
