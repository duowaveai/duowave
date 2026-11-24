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

pub mod catchphrases;
pub mod contributes;
pub mod controller;

use std::collections::{BTreeMap, HashSet};

use futures::future::BoxFuture;
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
use crate::settings::definitions::Persona;
use crate::storage::Db;
use crate::utils::time::now_iso;
use contributes::CharacterPackages;
pub use contributes::{Character, Gender, Rejected, Tone};
pub use controller::routes;

crate::route_needs!(Arc<CharactersService>);

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

/// The characters' voices on a computer (crates/duowave-pc, `characters::voices`: the voice lab and the model shop) —
/// what the characters ask of them. The station's build has none: a character's own voice is the computer's there,
/// the fallback speaks.
pub trait CharacterVoicing: Send + Sync {
    /// On a Турбо: the model a character's voice would go with here, and its fallback voice.
    fn turbo_choice<'a>(
        &'a self,
        character: &'a Character,
        wanted: Option<&'a str>,
    ) -> BoxFuture<'a, (Option<String>, Option<FallbackView>)>;
    /// How a character's voice stands on this computer.
    fn plan<'a>(&'a self, character: &'a Character) -> BoxFuture<'a, VoiceView>;
    /// On a Турбо: the model its centre named for a character's voice.
    fn name_engine(&self, character: &str, engine: &str) -> Result<(), ApiError>;
    /// A package removed: the models its centre named for its characters' voices go with it.
    fn forget_engines(&self, package: &str);
    /// The packages' characters as read now: their voices laid out.
    fn lay_out(&self, characters: &[Character]);
    /// Characters hidden by their feed: their voices step aside.
    fn step_aside(&self, hidden: &[Character]);
    /// A character's voice chosen (installed when asked, anyway when asked).
    fn choose<'a>(
        &'a self,
        character: &'a Character,
        install: bool,
        anyway: bool,
        actor: Option<Actor>,
    ) -> BoxFuture<'a, Result<ChosenVoice, ApiError>>;
    /// Before a package goes: its characters' voices go with it, and none of them may be the one speaking.
    fn before_removal(&self, package: &str) -> Result<(), ApiError>;
}

pub struct CharactersService {
    db: Arc<Db>,
    events: EventBus,
    personas: Arc<PersonaService>,
    host: Arc<dyn CharacterPackages>,
    /// The packages' characters as last read.
    known: RwLock<Vec<Result<Character, Rejected>>>,
    /// The characters' voices on a computer (`CharacterVoicing`); none on the station.
    voices: std::sync::OnceLock<Arc<dyn CharacterVoicing>>,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl CharactersService {
    pub fn new(
        db: Arc<Db>,
        events: EventBus,
        personas: Arc<PersonaService>,
        host: Arc<dyn CharacterPackages>,
    ) -> Arc<Self> {
        let service = Arc::new(CharactersService {
            db,
            events,
            personas,
            host,
            known: RwLock::new(Vec::new()),
            voices: std::sync::OnceLock::new(),
            tasks: Mutex::new(Vec::new()),
        });
        service.sync();
        service
    }

    /// The voice lab and the models: the characters' voices (a computer's build, crates/duowave-pc).
    pub fn link_voices(&self, voices: Arc<dyn CharacterVoicing>) {
        if self.voices.set(voices).is_err() {
            tracing::warn!(target: "characters", "the voices are linked already");
        }
    }

    fn voices(&self) -> Option<Arc<dyn CharacterVoicing>> {
        self.voices.get().cloned()
    }

    /// On a Турбо (clean-path check 18.11, item 40): the model a character's voice would go with here — `wanted`, the
    /// one its centre named, else this computer's pick — and its fallback voice. None without the computer's voices.
    pub async fn turbo_choice(
        &self,
        character: &Character,
        wanted: Option<&str>,
    ) -> (Option<String>, Option<FallbackView>) {
        match self.voices() {
            Some(voices) => voices.turbo_choice(character, wanted).await,
            None => (None, None),
        }
    }

    /// How a character's voice stands on this computer — its own, or the fallback and why its own does not go here.
    pub async fn voice_here(&self, character: &Character) -> Option<VoiceView> {
        Some(self.voices()?.plan(character).await)
    }

    /// On a Турбо: the model its centre named for a character's voice is kept, and the voice goes with it.
    pub fn name_engine(&self, character: &str, engine: &str) -> Result<(), ApiError> {
        match self.voices() {
            Some(voices) => voices.name_engine(character, engine),
            None => Ok(()),
        }
    }

    /// A package removed: the models its centre named for its characters' voices here go with it (a Турбо's, the PC's
    /// build; nothing on the station).
    pub fn forget_engines(&self, package: &str) {
        if let Some(voices) = self.voices() {
            voices.forget_engines(package);
        }
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

    /// The character an own voice's model id speaks for (`own:c-<package>-<character>-<engine>`), if it is installed.
    pub fn of_voice(&self, model_id: &str) -> Option<Character> {
        let voice = model_id.strip_prefix("own:c-")?;
        self.known()
            .into_iter()
            .filter_map(Result::ok)
            .find(|character| voice.starts_with(&format!("{}-{}-", character.package, character.local)))
    }

    /// Who answers here now (the persona's card id), if anyone was chosen.
    pub fn active_id(&self) -> Option<String> {
        self.personas.view().ok()?.active_preset
    }

    /// The first character of an installed package, checked.
    pub fn of_package(&self, package: &str) -> Option<Character> {
        self.known().into_iter().filter_map(Result::ok).find(|character| character.package == package)
    }

    /// The installed packages' characters read again: the table follows them (rows of packages gone go), and their
    /// voices are laid out in the voice lab or removed with them. A package its feed hides counts as gone, and more: its
    /// character that was chosen gives the place to Alisa, its voice that speaks to a fallback.
    pub fn sync(&self) {
        self.host.rescan();
        let mut found = Vec::new();
        for package in self.host.visible() {
            found.extend(contributes::read(&package));
        }
        self.step_aside_hidden();
        for rejected in found.iter().filter_map(|found| found.as_ref().err()) {
            tracing::warn!(target: "characters", "{} {}: not offered — {}", rejected.id, rejected.version, rejected.problem);
        }
        let valid: Vec<Character> = found.iter().filter_map(|found| found.as_ref().ok().cloned()).collect();
        if let Err(error) = self.store(&valid) {
            tracing::warn!(target: "characters", "the characters' table is not synced: {error}");
        }
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
            .hidden()
            .into_iter()
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
        if let Some(voices) = self.voices() {
            voices.step_aside(&hidden);
        }
    }

    fn package_ref(&self, package: &str, version: &str, license: Option<String>) -> PackageRef {
        let verified = self.host.verified(package);
        PackageRef { id: package.to_string(), version: version.to_string(), license, verified }
    }

    async fn voice_of(&self, character: &Character) -> VoiceView {
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
            self.personas.choose(persona_of(&character, body.tones.as_ref()), actor.clone())?;
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

    /// A character's words as a room's «Кто отвечает здесь» keeps them (docs/plans/home-and-devices.md, «Комнаты»):
    /// a package's character with its tones as its package sets them, a card of the built-in ones as it is. Its voice
    /// stays the home's.
    pub fn persona_for(&self, id: &str) -> Result<Persona, ApiError> {
        if let Some(character) = self.character(id) {
            return Ok(persona_of(&character, None));
        }
        let card = persona_presets().into_iter().find(|preset| preset.id == id);
        let Some(card) = card else {
            return Err(ApiError::not_found(crate::tr!("Нет персонажа {id}.", id = id)));
        };
        if card.id == ALICE {
            let positions = alice_positions(None);
            let text = alice_text(Some(&positions));
            return Ok(Persona {
                preset_id: Some(card.id.clone()),
                text: text.clone(),
                base: Some(text),
                tones: Some(positions),
            });
        }
        Ok(Persona { preset_id: Some(card.id.clone()), text: card.text.clone(), base: Some(card.text), tones: None })
    }

    /// The voice as it is, untouched: what the character's would need.
    async fn voice_left(&self, character: &Character) -> ChosenVoice {
        let plan = self.voice_of(character).await;
        ChosenVoice { state: plan.state, needs: plan.needs, pending: None }
    }

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

    /// Before a package goes: its characters' voices go with it, and none of them may be the one speaking.
    pub fn before_removal(&self, package: &str) -> Result<(), ApiError> {
        if let Some(voices) = self.voices() {
            return voices.before_removal(package);
        }
        Ok(())
    }
}

/// A package's character as the house's persona: its text with these tones (its own by default), the text it gave
/// kept as the measure of later edits.
fn persona_of(character: &Character, tones: Option<&BTreeMap<String, usize>>) -> Persona {
    let positions = character.positions(tones);
    let text = character.text(Some(&positions));
    Persona { preset_id: Some(character.id.clone()), text: text.clone(), base: Some(text), tones: Some(positions) }
}

/// An activator's word called its character (src/activator, `WordCharacter`): one character lives in the house today,
/// so the word's becomes the house's — as «Выбрать» in the panel: its text at once (the answer to the word's request is
/// already its), its voice right after (on the computer, where the voice lab is; nothing is downloaded for it).
impl crate::ear::word::WordCharacter for CharactersService {
    fn answer_as(&self, character: &str, word: &str) -> bool {
        let Some(character) = self.character(character) else { return false };
        if self.personas.chosen().and_then(|chosen| chosen.preset_id).as_deref() == Some(character.id.as_str()) {
            return false;
        }
        let actor = Actor {
            kind: crate::core::identity::PrincipalKind::System,
            id: "activator".into(),
            name: crate::tr!("слово «{word}»", word = word),
        };
        if let Err(error) = self.personas.choose(persona_of(&character, None), Some(actor.clone())) {
            tracing::warn!(target: "characters", "{}: not switched by its word: {}", character.id, error.message);
            return false;
        }
        tracing::info!(target: "characters", "{} answers now: its word «{word}»", character.id);
        if let Some(voices) = self.voices() {
            tokio::spawn(async move {
                if let Err(error) = voices.choose(&character, false, false, Some(actor)).await {
                    tracing::warn!(target: "characters", "{}: its voice not switched: {}", character.id, error.message);
                }
            });
        }
        true
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

/// A package's models for its characters' voices dropped — by the package and by each of its characters
/// (`<package>:*`), in the centre's choices or a Турбо's named ones; whether any was there.
pub fn drop_package(chosen: &mut std::collections::BTreeMap<String, String>, package: &str) -> bool {
    let theirs = format!("{package}:");
    let before = chosen.len();
    chosen.retain(|kept, _| kept != package && !kept.starts_with(&theirs));
    chosen.len() != before
}

/// A package removed: the models chosen for its characters' voices go with it — here (the centre's choices) and in what
/// a Турбо's centre named to it, so a package installed again starts afresh.
pub fn forget_package(settings: &crate::settings::SettingsService, characters: &CharactersService, package: &str) {
    if let Some(mut chosen) = settings.get(&crate::settings::definitions::HOME_VOICE_ENGINES, None) {
        if drop_package(&mut chosen, package) {
            if let Err(error) = settings.set(&crate::settings::definitions::HOME_VOICE_ENGINES, &chosen, None) {
                tracing::warn!(target: "home", "{package}: its voice models not forgotten: {}", error.message);
            }
        }
    }
    characters.forget_engines(package);
}
