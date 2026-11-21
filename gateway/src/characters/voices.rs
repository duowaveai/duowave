//! The characters' voices (a PC's build): what their packages bring, laid out in the voice lab as voices of its own —
//! `voices/c-<package>-<character>-<engine>/` with `meta.json` saying `origin: "package"` — for every engine the
//! package has a snapshot for, installed or not (they are 13–200 KB). A snapshot taken with other weights than the
//! engine's here is not laid out: that voice is taken again from the package's reference when it is chosen. A package
//! removed takes its voices with it.
//!
//! Choosing a character's voice goes through the models (src/models): its card `own:c-…`, with what it needs when
//! its engine is not here, and the pending choice that switches it on when the engine is installed.

use std::collections::HashSet;
use std::sync::Arc;

use serde_json::{json, Value};

use super::contributes::{Character, KNOWN_ENGINES};
use super::{fallback_cards, ChosenVoice, FallbackView, VoiceState, VoiceView};
use crate::core::events::Actor;
use crate::models::models_service::{engine_needs, Chosen, EngineState, VoiceRequest, OWN_PREFIX};
use crate::models::{ModelsService, OwnVoices};
use crate::platform::http::ApiError;
use crate::settings::definitions::{VoiceRecipe, TURBO_VOICE_ENGINES};
use crate::settings::SettingsService;
use crate::utils::time::now_iso;
use crate::voice_lab::clone_engines::{same_revision, spec, EngineId};
use crate::voice_lab::VoiceLabService;

/// A character's voice in the voice lab, for one engine.
pub fn voice_id(character: &Character, engine: &str) -> String {
    format!("c-{}-{}-{}", character.package, character.local, engine)
}

/// The model named for a character (`<package>:<local>`); by a package's id — the one named for a character of it.
fn named_in(mut named: std::collections::BTreeMap<String, String>, character: &str) -> Option<String> {
    if let Some(engine) = named.remove(character) {
        return Some(engine);
    }
    if character.contains(':') {
        return None;
    }
    let theirs = format!("{character}:");
    named.into_iter().find(|(id, _)| id.starts_with(&theirs)).map(|(_, engine)| engine)
}

/// The built-in Piper voices, any of which may stand in for a hidden character's voice.
const ANY_PIPER: [&str; 4] = ["piper-denis", "piper-dmitri", "piper-ruslan", "piper-irina"];

pub struct CharacterVoices {
    lab: Arc<VoiceLabService>,
    models: Arc<ModelsService>,
    /// The voice lab as the models see it: the engines' states in one language.
    own: Arc<dyn OwnVoices>,
    /// Where the model a Турбо's centre named for a character is kept (`TURBO_VOICE_ENGINES`).
    settings: Arc<SettingsService>,
}

impl CharacterVoices {
    pub fn new(
        lab: Arc<VoiceLabService>,
        models: Arc<ModelsService>,
        own: Arc<dyn OwnVoices>,
        settings: Arc<SettingsService>,
    ) -> Arc<CharacterVoices> {
        Arc::new(CharacterVoices { lab, models, own, settings })
    }

    /// The model the centre named for this character's voice here (clean-path check 18.11, item 40), if any.
    /// Asked by a package's id (a plan before its character is here), the one named for a character of it.
    pub fn named_engine(&self, character: &str) -> Option<String> {
        named_in(self.settings.get(&TURBO_VOICE_ENGINES, None)?, character)
    }

    /// A package removed: the models its centre named for its characters' voices go with it.
    pub fn forget_engines(&self, package: &str) {
        let Some(mut named) = self.settings.get(&TURBO_VOICE_ENGINES, None) else { return };
        if crate::home::character::drop_package(&mut named, package) {
            if let Err(error) = self.settings.set(&TURBO_VOICE_ENGINES, &named, None) {
                tracing::warn!(target: "characters", "{package}: the models named for it not forgotten: {}", error.message);
            }
        }
    }

    /// Keeps the model the centre named for a character's voice: its voice goes with it from now on.
    pub fn name_engine(&self, character: &str, engine: &str) -> Result<(), ApiError> {
        let mut named = self.settings.get(&TURBO_VOICE_ENGINES, None).unwrap_or_default();
        if named.get(character).map(String::as_str) == Some(engine) {
            return Ok(());
        }
        named.insert(character.to_string(), engine.to_string());
        self.settings.set(&TURBO_VOICE_ENGINES, &named, None)
    }

    /// For a Турбо's plan: the model its voice would go with here (`wanted` first — the centre's word —, then the one
    /// the centre named before, then its own pick) and its fallback voice.
    pub async fn turbo_choice(
        &self,
        character: &Character,
        wanted: Option<&str>,
    ) -> (Option<String>, Option<FallbackView>) {
        let engines = self.own.engines().await;
        let engine = wanted
            .and_then(|wanted| self.usable(character, &engines, wanted))
            .or_else(|| self.pick(character, &engines))
            .map(|(engine, _)| engine.id.clone());
        (engine, self.fallback(character).await)
    }

    /// An engine by its id when the character's voice can go with it here: it runs here, and the package has a
    /// snapshot for it, or a reference to take one, or the voice is made already. Whether the voice is made.
    fn usable<'a>(
        &self,
        character: &Character,
        engines: &'a [EngineState],
        id: &str,
    ) -> Option<(&'a EngineState, bool)> {
        let engine = engines.iter().find(|engine| engine.id == id && engine.status != "unsupported")?;
        if !self.fits(engine) {
            return None;
        }
        let made = self.lab.store.voice(&voice_id(character, &engine.id)).is_some();
        let possible = made || character.voices.contains_key(&engine.id) || character.reference.is_some();
        possible.then_some((engine, made))
    }

    /// The revision of an engine's weights here: the installed one's, else the one this gateway installs.
    fn revision(&self, engine: EngineId) -> String {
        self.lab.installed_revision(engine).unwrap_or_else(|| spec(engine).revision.to_string())
    }

    /// The packages' snapshots laid out as voices; the voices of characters gone removed (not the one speaking).
    pub fn lay_out(&self, characters: &[Character]) {
        let store = &self.lab.store;
        let existing: std::collections::HashMap<String, serde_json::Map<String, Value>> =
            store.package_voices().into_iter().collect();
        let mut wanted = HashSet::new();
        for character in characters {
            for engine in KNOWN_ENGINES {
                wanted.insert(voice_id(character, engine));
            }
            for (engine, snapshot) in &character.voices {
                let Some(engine_id) = EngineId::parse(engine) else { continue };
                let id = voice_id(character, engine);
                let fits =
                    snapshot.model.as_deref().is_some_and(|model| same_revision(model, &self.revision(engine_id)));
                if !fits {
                    // Taken again from its reference when chosen; a voice already taken for it stays.
                    continue;
                }
                let current = existing.get(&id).is_some_and(|meta| {
                    meta.get("taken").and_then(Value::as_str) == Some("snapshot")
                        && meta.get("packageVersion").and_then(Value::as_str) == Some(character.version.as_str())
                        && store.voice(&id).is_some()
                });
                if current {
                    continue;
                }
                let snapshot_file = character.asset(&snapshot.file);
                let reference = character.reference.as_ref().map(|reference| character.asset(&reference.file));
                let mut files: Vec<(&str, &std::path::Path)> = vec![("voice.pt", snapshot_file.as_path())];
                if let Some(reference) = reference.as_deref().filter(|reference| reference.is_file()) {
                    files.push(("ref.wav", reference));
                }
                let mut meta = json!({
                    "id": id,
                    "name": character.title,
                    "engine": engine,
                    "origin": "package",
                    "package": character.package,
                    "packageVersion": character.version,
                    "character": character.local,
                    "model": snapshot.model,
                    "taken": "snapshot",
                    "createdAt": now_iso(),
                    "samples": [],
                });
                if let Some(reference) = &character.reference {
                    meta["reference"] = json!({ "words": reference.text });
                }
                match store.place_voice(&id, &files, &meta) {
                    Ok(()) => tracing::info!(target: "characters", "{}: voice {id} laid out", character.id),
                    Err(error) => {
                        tracing::warn!(target: "characters", "{}: voice {id} not laid out: {error}", character.id)
                    }
                }
            }
        }
        let speaking = self.speaking();
        for id in existing.keys().filter(|id| !wanted.contains(*id)) {
            if speaking.as_deref() == Some(id.as_str()) {
                tracing::warn!(target: "characters", "voice {id}: its character is gone, but it speaks — kept until another is chosen");
                continue;
            }
            match store.remove(id) {
                Ok(()) => tracing::info!(target: "characters", "voice {id} removed with its package"),
                Err(error) => tracing::warn!(target: "characters", "voice {id} not removed: {error}"),
            }
        }
    }

    /// The own voice speaking now, by its id in the voice lab.
    fn speaking(&self) -> Option<String> {
        self.models.chosen_voice()?.model_id.strip_prefix(OWN_PREFIX).map(str::to_string)
    }

    /// The voices of characters their feeds hide (docs/plans/custom-feeds.md): one waiting for its engine is
    /// forgotten; one that speaks gives the place to a fallback here — the character's own, a like one by its gender,
    /// else any Piper voice that is here. With none here it keeps speaking, and the log says so.
    pub fn step_aside(&self, hidden: &[Character]) {
        let theirs = |card: &str| {
            let id = card.strip_prefix(OWN_PREFIX).unwrap_or(card);
            hidden.iter().find(|character| id.starts_with(&format!("c-{}-{}-", character.package, character.local)))
        };
        if self.models.pending_card().as_deref().and_then(theirs).is_some() {
            self.models.clear_pending(None);
        }
        let Some(speaking) = self.speaking() else { return };
        let Some(character) = theirs(&speaking) else { return };
        let wanted = character
            .fallback_voice
            .iter()
            .map(String::as_str)
            .chain(fallback_cards(character.gender).iter().copied())
            .chain(ANY_PIPER);
        let card = wanted
            .into_iter()
            .find(|card| self.models.voice_card(card).is_some_and(|(_, installed)| installed))
            .map(str::to_string);
        let Some(card) = card else {
            tracing::warn!(target: "characters", "{}: hidden, but no fallback voice is here — its voice keeps speaking", character.id);
            return;
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else { return };
        let (models, id) = (self.models.clone(), character.id.clone());
        runtime.spawn(async move {
            match models.choose(VoiceRequest::card(&card), None).await {
                Ok(_) => tracing::info!(target: "characters", "{id}: hidden — {card} speaks now"),
                Err(error) => {
                    tracing::warn!(target: "characters", "{id}: hidden, but {card} is not on: {}", error.message)
                }
            }
        });
    }

    /// A package may go only when none of its characters' voices speaks.
    pub fn before_removal(&self, package: &str) -> Result<(), ApiError> {
        let prefix = format!("c-{package}-");
        match self.speaking() {
            Some(id) if id.starts_with(&prefix) => Err(ApiError::conflict(crate::tr!(
                "Голос этого персонажа говорит сейчас — сначала выберите другой голос."
            ))),
            _ => Ok(()),
        }
    }

    /// The fallback voice: the character's own choice when this gateway has it, else a like voice by its gender; one
    /// that is downloaded first.
    async fn fallback(&self, character: &Character) -> Option<FallbackView> {
        let mut known = Vec::new();
        let wanted =
            character.fallback_voice.iter().map(String::as_str).chain(fallback_cards(character.gender).iter().copied());
        for card in wanted {
            if let Some((title, installed)) = self.models.voice_card(card) {
                known.push(FallbackView { card_id: card.to_string(), title, installed });
            }
        }
        let installed = known.iter().position(|card| card.installed);
        match installed {
            Some(index) => Some(known.swap_remove(index)),
            None => known.into_iter().next(),
        }
    }

    /// The engine a character's voice goes with here, and how it stands: one with a voice ready first, then one
    /// installed that can take it from the reference, then one being installed, then one to install.
    /// Whether an engine has room here: installed or coming already, or its install fits on the disk — one that does
    /// not is never downloaded (clean-path check 18.11, item 40).
    fn fits(&self, engine: &EngineState) -> bool {
        engine.status != "missing" || engine.disk_mb <= self.lab.free_disk_mb()
    }

    /// The model the centre named for it goes first, when it can (clean-path check 18.11, item 40). A model to install
    /// that does not fit on the disk is not picked.
    fn pick<'a>(&self, character: &Character, engines: &'a [EngineState]) -> Option<(&'a EngineState, bool)> {
        if let Some(named) = self.named_engine(&character.id).and_then(|named| self.usable(character, engines, &named))
        {
            return Some(named);
        }
        let usable = |engine: &EngineState| {
            let made = self.lab.store.voice(&voice_id(character, &engine.id)).is_some();
            let snapshot = character.voices.contains_key(&engine.id);
            (made, made || snapshot || character.reference.is_some())
        };
        let order = |status: &str, made_only: bool| {
            engines.iter().find(|engine| {
                let (made, possible) = usable(engine);
                engine.status == status && self.fits(engine) && if made_only { made } else { possible }
            })
        };
        order("ready", true)
            .map(|engine| (engine, true))
            .or_else(|| order("ready", false).map(|engine| (engine, false)))
            .or_else(|| order("installing", false).map(|engine| (engine, usable(engine).0)))
            .or_else(|| {
                // To install: one the package has a snapshot for first.
                engines
                    .iter()
                    .find(|engine| {
                        engine.status == "missing" && self.fits(engine) && character.voices.contains_key(&engine.id)
                    })
                    .or_else(|| order("missing", false))
                    .map(|engine| (engine, usable(engine).0))
            })
    }

    /// How a character's voice stands here.
    pub async fn plan(&self, character: &Character) -> VoiceView {
        let fallback = self.fallback(character).await;
        if !character.speaks() {
            return match fallback {
                Some(fallback) => {
                    VoiceView { state: VoiceState::Fallback, fallback: Some(fallback), ..VoiceView::none() }
                }
                None => VoiceView::none(),
            };
        }
        let engines = self.own.engines().await;
        let Some((engine, _made)) = self.pick(character, &engines) else {
            let no_room = engines.iter().any(|engine| engine.status == "missing" && !self.fits(engine));
            let room =
                no_room.then(|| crate::tr!("Модели голоса этого персонажа не помещаются на диск этого компьютера."));
            let reason =
                room.or_else(|| engines.iter().find_map(|engine| engine.reason.clone())).unwrap_or_else(|| {
                    crate::tr!("Голос этого персонажа здесь не зазвучит: ни одна его модель тут не работает.")
                });
            let state = if fallback.is_some() { VoiceState::Fallback } else { VoiceState::Unsupported };
            return VoiceView { state, fallback, reason: Some(reason), ..VoiceView::none() };
        };
        let id = voice_id(character, &engine.id);
        let job = self.own.job();
        let (state, needs, progress) = match engine.status.as_str() {
            "ready" => {
                // Being taken from its reference now.
                let imprinting =
                    job.as_ref().is_some_and(|job| job.running && job.kind == "imprint" && job.engine == engine.id);
                let made = self.lab.store.voice(&id).is_some();
                if !made && imprinting {
                    (VoiceState::Installing, None, job.map(|job| job.percent))
                } else {
                    (VoiceState::Ready, None, None)
                }
            }
            "installing" => {
                let progress = job.filter(|job| job.running && job.kind == "install").map(|job| job.percent);
                (VoiceState::Installing, serde_json::to_value(engine_needs(engine, Vec::new())).ok(), progress)
            }
            _ => (VoiceState::Needs, serde_json::to_value(engine_needs(engine, Vec::new())).ok(), None),
        };
        VoiceView {
            state,
            engine: Some(engine.id.clone()),
            card_id: Some(format!("{OWN_PREFIX}{id}")),
            fallback,
            needs,
            progress,
            reason: None,
        }
    }

    /// What its voice is imprinted from when its package has no snapshot that fits this engine.
    fn recipe(&self, character: &Character, engine: &str) -> Option<VoiceRecipe> {
        let reference = character.reference.as_ref()?;
        Some(VoiceRecipe {
            voice: voice_id(character, engine),
            engine: engine.to_string(),
            reference: character.asset(&reference.file).to_string_lossy().into_owned(),
            text: reference.text.clone(),
            name: character.title.clone(),
            package: character.package.clone(),
            version: character.version.clone(),
            character: character.local.clone(),
        })
    }

    /// The character's voice chosen: its own when its engine is here (taken from its reference first when needed);
    /// its engine downloaded with `install` — the fallback speaking meanwhile — and switched on when it is there;
    /// the fallback where its own cannot be.
    pub async fn choose(
        &self,
        character: &Character,
        install: bool,
        anyway: bool,
        actor: Option<Actor>,
    ) -> Result<ChosenVoice, ApiError> {
        let plan = self.plan(character).await;
        let own = |card: &str, install: bool| {
            let id = card.strip_prefix(OWN_PREFIX).unwrap_or(card);
            let engine = plan.engine.clone().unwrap_or_default();
            let made = self.lab.store.voice(id).is_some();
            VoiceRequest {
                install,
                anyway,
                recipe: if made { None } else { self.recipe(character, &engine) },
                ..VoiceRequest::card(card)
            }
        };
        let answer = |chosen: Chosen, state: VoiceState| match chosen {
            Chosen::Active(_) => ChosenVoice { state, needs: None, pending: None },
            Chosen::Pending(pending) => {
                ChosenVoice { state: VoiceState::Installing, needs: None, pending: serde_json::to_value(pending).ok() }
            }
        };
        match plan.state {
            VoiceState::Ready => {
                let card = plan.card_id.clone().unwrap_or_default();
                // A voice to take from its reference is local work: no download to agree to.
                let chosen = self.models.choose(own(&card, true), actor).await?;
                Ok(answer(chosen, VoiceState::Ready))
            }
            VoiceState::Installing | VoiceState::Needs if !install => {
                Ok(ChosenVoice { state: plan.state, needs: plan.needs, pending: None })
            }
            VoiceState::Installing | VoiceState::Needs => {
                // The fallback speaks while the engine comes.
                if let Some(fallback) = plan.fallback.as_ref().filter(|fallback| fallback.installed) {
                    if let Err(error) = self.models.choose(VoiceRequest::card(&fallback.card_id), actor.clone()).await {
                        tracing::warn!(target: "characters", "{}: the fallback voice is not on: {}", character.id, error.message);
                    }
                }
                let card = plan.card_id.clone().unwrap_or_default();
                let chosen = self.models.choose(own(&card, true), actor).await?;
                Ok(answer(chosen, VoiceState::Ready))
            }
            VoiceState::Fallback => match plan.fallback {
                Some(fallback) if fallback.installed => {
                    self.models.choose(VoiceRequest::card(&fallback.card_id), actor).await?;
                    Ok(ChosenVoice { state: VoiceState::Fallback, needs: None, pending: None })
                }
                Some(fallback) => {
                    let request = VoiceRequest { install, ..VoiceRequest::card(&fallback.card_id) };
                    match self.models.choose(request, actor).await {
                        Ok(chosen) => Ok(answer(chosen, VoiceState::Fallback)),
                        // Its file is not here and it was not asked for: what it needs.
                        Err(error) if error.status == axum::http::StatusCode::CONFLICT => Ok(ChosenVoice {
                            state: VoiceState::Fallback,
                            needs: error.errors.and_then(|errors| errors.get("needs").cloned()),
                            pending: None,
                        }),
                        Err(error) => Err(error),
                    }
                }
                None => Ok(ChosenVoice { state: VoiceState::Fallback, needs: None, pending: None }),
            },
            VoiceState::Unsupported | VoiceState::None => {
                Ok(ChosenVoice { state: plan.state, needs: None, pending: None })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::characters::contributes::tests::{arthur_manifest, write_arthur};
    use crate::characters::ChooseBody;
    use crate::voice_lab::service::testing::lab;

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
        let store = &app.voice_lab.store;
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
        assert_eq!(app.models.chosen_voice().unwrap().model_id, "own:c-arthur-arthur-omnivoice");
        assert_eq!(app.characters.before_removal("arthur").unwrap_err().status.as_u16(), 409);
        app.characters.before_removal("palych").unwrap();
    }

    #[tokio::test]
    async fn a_character_whose_engine_is_not_here_says_what_it_needs_and_installs_it_only_when_asked() {
        let lab = lab().await;
        let app = &lab.app;
        let mut cosy_only = arthur_manifest("1.0.0");
        let character = &mut cosy_only["contributes"]["characters"][0];
        character["voices"] =
            serde_json::json!({ "cosyvoice3": { "file": "voices/cosyvoice3.pt", "model": "074ca6dc" } });
        character.as_object_mut().unwrap().remove("ref");
        write_arthur(&app.config.plugins.packages_dir, &cosy_only);
        app.characters.sync();
        assert!(
            app.voice_lab.store.voice("c-arthur-arthur-cosyvoice3").is_some(),
            "laid out though its engine is not here"
        );
        let view = app.characters.view().await.unwrap();
        let arthur = view.characters.iter().find(|character| character.id == "arthur:arthur").unwrap();
        assert_eq!((arthur.voice.state, arthur.voice.engine.as_deref()), (VoiceState::Needs, Some("cosyvoice3")));
        assert_eq!(arthur.voice.needs.as_ref().unwrap()["kind"], "engine");
        let answer = app.characters.choose("arthur:arthur", ChooseBody::default(), true, None).await.unwrap();
        assert_eq!(
            answer.voice.state,
            VoiceState::Needs,
            "the character is chosen, its voice waits for the owner's word"
        );
        assert!(answer.voice.pending.is_none());
        assert!(app.personas.chosen().unwrap().preset_id.as_deref() == Some("arthur:arthur"));
    }
}
