//! Характер, которым Алиса отвечает. Карточки живут в базе — сеются из встроенных при старте, upsert по id, без
//! дублей — и выбор ссылается на них по id. Текст хранится рядом: применяется то, что сказано, а не что в карточке
//! сейчас (карточку могли обновить). Читается на каждый вопрос — смена без рестарта. Приоритет: характер из пульта,
//! затем DIALOG_SYSTEM_PROMPT_FILE (полный промт, прежний механизм из env), затем стандартный. Правила речи шлюз
//! добавляет к любому характеру сам.

use std::path::PathBuf;
use std::sync::Arc;

use rusqlite::params;
use serde::Serialize;
use serde_json::{Map, Value};

use super::system_prompt::{
    alice_positions, alice_text, build_character_prompt, build_system_prompt, is_legacy_text, is_package_character,
    legacy, load_system_prompt, persona_presets, Legacy, PersonaPreset, SystemPrompt, ALICE, DEFAULT_PERSONA,
};
use crate::config::DialogConfig;
use crate::core::events::{Actor, EventBus, GatewayEvent};
use crate::platform::http::ApiError;
use crate::settings::definitions::{Persona, PERSONA};
use crate::settings::SettingsService;
use crate::storage::Db;
use crate::utils::time::now_iso;

/// A card's text edited by its owner: a character's prompt with its tones runs to some 5000 characters.
const MAX_EDITED: usize = 6000;

/// Откуда характер: из пульта, из файла DIALOG_SYSTEM_PROMPT_FILE или стандартный.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PersonaSource {
    Panel,
    File,
    Default,
}

/// Характер, как его показывает пульт: говорящий текст и карточки на выбор.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonaView {
    /// Текст, которым Алиса отвечает сейчас (стандартный, если пульт ничего не выбирал).
    pub persona: String,
    /// id карточки, чей текст говорит; null — свой характер.
    pub active_preset: Option<String>,
    pub source: PersonaSource,
    pub presets: Vec<PersonaPreset>,
}

pub struct PersonaService {
    system_prompt_file: Option<PathBuf>,
    db: Arc<Db>,
    settings: Arc<SettingsService>,
    events: EventBus,
}

impl PersonaService {
    /// The service, with the built-in cards seeded: a repeated seed updates them, never duplicates.
    pub fn new(
        config: &DialogConfig,
        db: Arc<Db>,
        settings: Arc<SettingsService>,
        events: EventBus,
    ) -> rusqlite::Result<PersonaService> {
        let service = PersonaService { system_prompt_file: config.system_prompt_file.clone(), db, settings, events };
        service.seed()?;
        Ok(service)
    }

    /// At each start. A card's row is written only when this build changed the card: written anew with its time at
    /// every start, it moved the settings revision, and the computer of the pair took a copy for nothing
    /// (characters/mod.rs, `store`).
    fn seed(&self) -> rusqlite::Result<()> {
        let at = now_iso();
        self.db.transaction(|t| {
            for preset in persona_presets() {
                t.execute(
                    "INSERT INTO characters (id, title, description, badge, text, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT (id) DO UPDATE SET title = excluded.title, description = excluded.description,
                     badge = excluded.badge, text = excluded.text, updated_at = excluded.updated_at
                     WHERE characters.title IS NOT excluded.title OR characters.description IS NOT excluded.description
                     OR characters.badge IS NOT excluded.badge OR characters.text IS NOT excluded.text",
                    params![preset.id, preset.title, preset.description, preset.badge, preset.text, at],
                )?;
            }
            // The cards of before (Стандартная, Ироничная, …) are Alisa's positions now: their rows go; a package's stay.
            let ids: Vec<String> = persona_presets().into_iter().map(|preset| preset.id).collect();
            let mut statement = t.prepare("SELECT id FROM characters WHERE package IS NULL")?;
            let stale: Vec<String> = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
                .into_iter()
                .filter(|id| !ids.contains(id))
                .collect();
            drop(statement);
            for id in stale {
                t.execute("DELETE FROM characters WHERE id = ?1", params![id])?;
            }
            Ok(())
        })
    }

    /// Системный промт для модели: характер человека (свой, если он задан), затем характер пульта с правилами речи,
    /// а без него — файл из env или встроенный.
    pub fn system_prompt(&self, user_id: Option<&str>) -> SystemPrompt {
        match self.stored(user_id) {
            // A package's character is not Alisa: the gateway tells it the word that wakes the station.
            Some(persona) if persona.preset_id.as_deref().is_some_and(is_package_character) => {
                SystemPrompt { text: build_character_prompt(&persona.text), problem: None }
            }
            Some(persona) => SystemPrompt { text: build_system_prompt(&persona.text), problem: None },
            None => load_system_prompt(self.system_prompt_file.as_deref()),
        }
    }

    /// Выбор пульта: карточка по id, или свой текст (пустой — вернуться к стандартному). Карточка с текстом — её текст,
    /// поправленный владельцем («Тонкая настройка»): карточка остаётся выбранной, правка хранится в выборе и не
    /// пропадает, когда пакет персонажа обновится.
    pub fn set(&self, body: &Map<String, Value>, actor: Option<Actor>) -> Result<(), ApiError> {
        if let Some(preset_id) = body.get("presetId").and_then(Value::as_str) {
            // A card of before (an older panel, the dashboard's list): Alisa in its positions, or where it went.
            match legacy(Some(preset_id), "") {
                Some(Legacy::Alice(tones)) if !body.contains_key("text") => {
                    return self.save(alice(Some(&tones)), actor);
                }
                Some(Legacy::Replaced(package)) => {
                    return Err(ApiError::bad_request(format!(
                        "Мажордома заменил персонаж из каталога ({package}): он в «Персонаже»."
                    )));
                }
                _ => {}
            }
            if preset_id == ALICE && !body.contains_key("text") {
                // Alisa again: in the positions she had, the default ones the first time.
                let before = self.stored(None).filter(|stored| stored.preset_id.as_deref() == Some(ALICE));
                return self.save(alice(before.as_ref().and_then(|before| before.tones.as_ref())), actor);
            }
            let preset = self
                .characters()?
                .into_iter()
                .find(|character| character.id == preset_id)
                .ok_or_else(|| ApiError::bad_request(format!("нет такой карточки характера: {preset_id}")))?;
            let edited = body.get("text").and_then(Value::as_str).map(str::trim).filter(|text| !text.is_empty());
            let Some(edited) = edited else {
                let base = Some(preset.text.clone());
                return self.save(Persona { preset_id: Some(preset.id), text: preset.text, base, tones: None }, actor);
            };
            if edited.encode_utf16().count() > MAX_EDITED {
                return Err(ApiError::bad_request(format!("Слишком длинно — до {MAX_EDITED} символов.")));
            }
            // The same card as before keeps the text it gave then and its tones: what an edit is measured against.
            let before =
                self.settings.get(&PERSONA, None).filter(|stored| stored.preset_id.as_deref() == Some(preset_id));
            let (base, tones) = match before {
                Some(before) => (before.base.or(Some(preset.text)), before.tones),
                None => (Some(preset.text), None),
            };
            return self.save(Persona { preset_id: Some(preset.id), text: edited.to_string(), base, tones }, actor);
        }
        let text = body.get("text").and_then(Value::as_str).map(str::trim).unwrap_or("");
        let length = text.encode_utf16().count();
        if length > 2000 {
            return Err(ApiError::bad_request("Слишком длинно — до 2000 символов."));
        }
        if !text.is_empty() && length < 10 {
            return Err(ApiError::bad_request("Опишите характер хотя бы в паре слов."));
        }
        self.save(Persona::plain(None, text.to_string()), actor)
    }

    /// Характер для карточек: что говорит сейчас и какая из них горит.
    pub fn view(&self) -> Result<PersonaView, ApiError> {
        let stored = self.stored(None);
        let presets = self.characters()?;
        let file = match (&stored, &self.system_prompt_file) {
            (None, Some(file)) => Some(load_system_prompt(Some(file))).filter(|prompt| prompt.problem.is_none()),
            _ => None,
        };
        Ok(match stored {
            Some(stored) => PersonaView {
                persona: stored.text,
                active_preset: stored.preset_id,
                source: PersonaSource::Panel,
                presets,
            },
            None => match file {
                Some(file) => {
                    PersonaView { persona: file.text, active_preset: None, source: PersonaSource::File, presets }
                }
                None => PersonaView {
                    persona: DEFAULT_PERSONA.to_string(),
                    active_preset: Some(ALICE.into()),
                    source: PersonaSource::Default,
                    presets,
                },
            },
        })
    }

    /// The cards in the database, in the order they were seeded.
    fn characters(&self) -> rusqlite::Result<Vec<PersonaPreset>> {
        self.db.with(|c| {
            let mut statement =
                c.prepare("SELECT id, title, description, badge, text FROM characters ORDER BY rowid")?;
            let rows = statement.query_map([], |row| {
                let badge: Option<String> = row.get(3)?;
                Ok(PersonaPreset {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    description: row.get(2)?,
                    badge: badge.filter(|badge| !badge.is_empty()),
                    text: row.get(4)?,
                })
            })?;
            rows.collect()
        })
    }

    /// A character chosen as a whole (src/characters): its text with the tones chosen, the text it gave kept as the
    /// measure of later edits.
    pub fn choose(&self, choice: Persona, actor: Option<Actor>) -> Result<(), ApiError> {
        self.save(choice, actor)
    }

    /// The choice stored for the household (not a person's own).
    pub fn chosen(&self) -> Option<Persona> {
        self.stored(None)
    }

    fn save(&self, choice: Persona, actor: Option<Actor>) -> Result<(), ApiError> {
        self.settings.set(&PERSONA, &choice, actor.clone())?;
        self.events.emit(GatewayEvent::PersonaChanged { preset_id: choice.preset_id, actor });
        Ok(())
    }

    /// Что сохранено (для человека — его собственный, если есть); карточку без id узнаём по точному совпадению
    /// текста. Пустой текст — стандартный характер.
    fn stored(&self, user_id: Option<&str>) -> Option<Persona> {
        self.settings.get(&PERSONA, user_id).map(upgrade)
    }

    /// The catalog's character that took the place of the card chosen before (the butler → Артур), if it did.
    pub fn replaced_by(&self) -> Option<&'static str> {
        let stored = self.settings.get(&PERSONA, None)?;
        match legacy(stored.preset_id.as_deref(), &stored.text) {
            Some(Legacy::Replaced(package)) => Some(package),
            _ => None,
        }
    }
}

/// Alisa in these positions: her text, kept as the measure of later edits, and the positions.
fn alice(tones: Option<&std::collections::BTreeMap<String, usize>>) -> Persona {
    let positions = alice_positions(tones);
    let text = alice_text(Some(&positions));
    Persona { preset_id: Some(ALICE.into()), text: text.clone(), base: Some(text), tones: Some(positions) }
}

/// A choice as it reads now. The cards of before (till 07.11.2026) are Alisa in some positions — an unedited text of
/// theirs speaks as Alisa's text in them, an edited one stays as it was, edited; the butler's text stays one's own
/// (its card is the catalog's Артур now). A choice without an id is a card when its text is the card's word for word.
fn upgrade(stored: Persona) -> Persona {
    let id = stored.preset_id.as_deref().filter(|id| !id.is_empty());
    if id.is_some_and(|id| id == ALICE || is_package_character(id)) {
        return stored;
    }
    match legacy(id, &stored.text) {
        Some(Legacy::Alice(tones)) => {
            let mut chosen = alice(Some(&tones));
            if !is_legacy_text(&stored.text) && stored.text != alice_text(None) {
                chosen.text = stored.text;
            }
            chosen
        }
        Some(Legacy::Replaced(_)) => Persona::plain(None, stored.text),
        None if id.is_none() && stored.text == alice_text(None) => alice(None),
        None => stored,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Env};
    use crate::dialog::system_prompt::default_system_prompt;
    use serde_json::json;

    fn service(file: Option<PathBuf>) -> PersonaService {
        let mut config = Config::for_tests(&Env::default()).dialog;
        config.system_prompt_file = file;
        let db = Arc::new(Db::memory());
        let events = EventBus::new();
        let settings = Arc::new(SettingsService::new(db.clone(), events.clone()));
        PersonaService::new(&config, db, settings, events).unwrap()
    }

    fn body(value: serde_json::Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap()
    }

    #[test]
    fn seeds_alisa_once_drops_the_cards_of_before_and_speaks_her_until_one_is_chosen() {
        let personas = service(None);
        // A table from before Alisa had regulators: its cards go at the next seed, a package's character stays.
        personas
            .db
            .with(|c| {
                c.execute_batch(
                    "INSERT INTO characters (id, title, description, text, updated_at) VALUES ('irony', 'Ироничная', '', 'x', 'now');
                     INSERT INTO characters (id, title, description, text, updated_at, package) VALUES ('arthur:arthur', 'Артур', '', 'y', 'now', 'arthur');",
                )
            })
            .unwrap();
        personas.seed().unwrap();
        let view = personas.view().unwrap();
        let ids: Vec<&str> = view.presets.iter().map(|preset| preset.id.as_str()).collect();
        assert_eq!(ids, ["alice", "arthur:arthur"], "a second seed updates, never duplicates");
        assert_eq!(view.presets[0].badge.as_deref(), Some("встроенная"));
        assert_eq!(view.persona, DEFAULT_PERSONA);
        assert_eq!(view.active_preset.as_deref(), Some("alice"));
        assert_eq!(view.source, PersonaSource::Default);
        assert_eq!(personas.system_prompt(None), SystemPrompt { text: default_system_prompt(), problem: None });
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["activePreset"], json!("alice"));
    }

    /// A start seeds the cards again: a card this build did not change keeps its row, its time with it, and the
    /// settings revision stays; a card that differs is written as the build has it.
    #[test]
    fn a_start_rewrites_no_card_the_build_did_not_change() {
        let personas = service(None);
        let alice = || -> (String, String) {
            personas
                .db
                .with(|c| {
                    c.query_row("SELECT text, updated_at FROM characters WHERE id = 'alice'", [], |row| {
                        Ok((row.get(0)?, row.get(1)?))
                    })
                })
                .unwrap()
        };
        let before = alice();
        // The times are to the millisecond: a row written anew would show it.
        std::thread::sleep(std::time::Duration::from_millis(5));
        personas.seed().unwrap();
        assert_eq!(alice(), before);

        personas.db.with(|c| c.execute("UPDATE characters SET text = 'older' WHERE id = 'alice'", [])).unwrap();
        personas.seed().unwrap();
        let after = alice();
        assert_eq!(after.0, before.0, "the build's card again");
        assert_ne!(after.1, before.1);
    }

    #[test]
    fn takes_alisa_or_ones_own_text_and_says_what_it_cannot_take() {
        let personas = service(None);
        let mut events = personas.events.subscribe();
        personas.set(&body(json!({"presetId": "alice"})), None).unwrap();
        let view = personas.view().unwrap();
        assert_eq!(view.active_preset.as_deref(), Some("alice"));
        assert_eq!(view.source, PersonaSource::Panel);
        assert_eq!(personas.system_prompt(None).text, default_system_prompt());
        let mut names = Vec::new();
        while let Ok(event) = events.try_recv() {
            names.push(event.name());
        }
        assert_eq!(names, vec!["settings.changed", "persona.changed"]);

        personas.set(&body(json!({"text": "  Ты — Алиса, пиратка на покое.  "})), None).unwrap();
        let view = personas.view().unwrap();
        assert_eq!(view.persona, "Ты — Алиса, пиратка на покое.");
        assert_eq!(view.active_preset, None, "one's own character lights no card");
        assert!(personas.system_prompt(None).text.starts_with("Ты — Алиса, пиратка на покое.\nКаждый ответ"));

        // The text of a card without its id is that card.
        personas.set(&body(json!({"text": DEFAULT_PERSONA})), None).unwrap();
        assert_eq!(personas.view().unwrap().active_preset.as_deref(), Some("alice"));

        let missing = personas.set(&body(json!({"presetId": "pirate"})), None).unwrap_err();
        assert_eq!(missing.message, "нет такой карточки характера: pirate");
        let butler = personas.set(&body(json!({"presetId": "butler"})), None).unwrap_err();
        assert!(butler.message.starts_with("Мажордома заменил персонаж из каталога (arthur)"));
        assert_eq!(
            personas.set(&body(json!({"text": "Коротко"})), None).unwrap_err().message,
            "Опишите характер хотя бы в паре слов."
        );
        let long = "а".repeat(2001);
        assert_eq!(
            personas.set(&body(json!({"text": long})), None).unwrap_err().message,
            "Слишком длинно — до 2000 символов."
        );

        personas.set(&body(json!({"text": ""})), None).unwrap();
        assert_eq!(personas.view().unwrap().source, PersonaSource::Default, "empty goes back to the standard one");

        // Alisa with the owner's edit: she stays chosen, the edit is what is said, her text with the tones its measure.
        personas.set(&body(json!({"presetId": "irony"})), None).unwrap();
        personas.set(&body(json!({"presetId": "alice", "text": "Ты — Алиса, и говоришь стихами."})), None).unwrap();
        let chosen = personas.chosen().unwrap();
        assert_eq!(chosen.preset_id.as_deref(), Some("alice"));
        assert_eq!(chosen.text, "Ты — Алиса, и говоришь стихами.");
        assert!(chosen.base.unwrap().contains("лёгкая ирония"));
        assert_eq!(chosen.tones.unwrap()["irony"], 2, "the positions stay with the edit");
        assert!(personas.system_prompt(None).text.starts_with("Ты — Алиса, и говоришь стихами.\nКаждый ответ"));
        // Alisa chosen again keeps her positions.
        personas.set(&body(json!({"presetId": "alice"})), None).unwrap();
        assert!(personas.chosen().unwrap().text.contains("лёгкая ирония"));
    }

    #[test]
    fn reads_a_choice_of_before_as_alisa_and_the_butlers_as_ones_own_text() {
        let personas = service(None);
        let keep = |preset_id: Option<&str>, text: &str| {
            let stored = Persona::plain(preset_id.map(str::to_string), text.to_string());
            personas.settings.set(&PERSONA, &stored, None).unwrap();
        };
        let warm = crate::dialog::system_prompt::tests::legacy_text("warm");
        keep(Some("warm"), &warm);
        let chosen = personas.chosen().unwrap();
        assert_eq!(
            (chosen.preset_id.as_deref(), chosen.tones.as_ref().map(|tones| tones["warmth"])),
            (Some("alice"), Some(2))
        );
        assert_eq!(chosen.base.as_ref(), Some(&chosen.text), "an unedited card of before is not an edit");
        assert!(chosen.text.starts_with(DEFAULT_PERSONA) && chosen.text.contains("близкая подруга"));
        assert_eq!(personas.replaced_by(), None);

        keep(Some("business"), "Ты — Алиса, деловая, но со своими словами.");
        let chosen = personas.chosen().unwrap();
        assert_eq!(chosen.preset_id.as_deref(), Some("alice"));
        assert_eq!(chosen.text, "Ты — Алиса, деловая, но со своими словами.", "an edit of before stays");
        assert_ne!(chosen.base.as_ref(), Some(&chosen.text));

        let butler = crate::dialog::system_prompt::tests::legacy_text("butler");
        keep(Some("butler"), &butler);
        let chosen = personas.chosen().unwrap();
        assert_eq!((chosen.preset_id, chosen.text.as_str()), (None, butler.as_str()), "the butler's text speaks on");
        assert_eq!(personas.replaced_by(), Some("arthur"));
        assert!(personas.system_prompt(None).text.starts_with("Ты — Артур, мажордом"));
    }

    #[test]
    fn reads_the_prompt_file_while_the_panel_has_chosen_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("system-prompt.txt");
        std::fs::write(&file, "Ты — Боб из файла.").unwrap();
        let personas = service(Some(file.clone()));
        let view = personas.view().unwrap();
        assert_eq!(
            (view.persona.as_str(), view.active_preset, view.source),
            ("Ты — Боб из файла.", None, PersonaSource::File)
        );
        assert_eq!(personas.system_prompt(None).text, "Ты — Боб из файла.");
        std::fs::remove_file(&file).unwrap();
        let view = personas.view().unwrap();
        assert_eq!(view.source, PersonaSource::Default, "a file that is gone is the standard character");
        assert!(personas.system_prompt(None).problem.is_some());
    }
}
