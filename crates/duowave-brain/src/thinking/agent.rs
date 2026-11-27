//! Who it is said to (docs/plans/thinking.md, §11; the contract's «Who it is said to»): the agent of a turn, its
//! persona, its view of memory, and its place in the history. The audio side fills `to` and speaks in the voice of
//! `as`; Thinking picks the agent, in the order of the setting `thinking.agents.order`:
//!
//! 1. `to`: the agent the phrase names, when it is installed; a reply in the window goes to the agent that spoke last
//!    on the device, while the window lasts (the owner's decision 2: a word's choice lasts for the talk);
//! 2. `room`: the device's room's own («Кто отвечает здесь»);
//! 3. `home`: the home's (`personas.chosen`).
//!
//! An agent other than the room's and the home's brings its own persona (its package's text) and its own view of
//! memory (its layer). The history of a place stays one (decision 3: `agents.history: shared`), each turn of another
//! agent marked with who answered; `own` keeps each agent's history apart.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::dialog::{Channel, PersonaService};
use crate::door::{By, Kind, Opened, Phrase};
use crate::settings::SettingsService;

use super::settings::{HistoryView, Settings};

/// How long a word's agent keeps the device's talk after it spoke.
pub const TALK_MS: u64 = 60_000;

/// The agent of a turn.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Agent {
    /// `<package>:<character>` or a card's id; None — the built-in character.
    pub id: Option<String>,
    /// Why this one: `to`, `window`, `room`, `home`.
    pub by: &'static str,
    /// Its title for the history's notes.
    pub title: Option<String>,
    /// Its persona as the model's instructions, when it is not the room's nor the home's.
    pub prompt: Option<String>,
}

impl Agent {
    /// The agent's own persona and memory are used: it is neither the room's nor the home's.
    pub fn own(&self) -> bool {
        self.prompt.is_some()
    }
}

/// The agents that spoke last on each device, for the replies in their window.
#[derive(Default)]
pub struct Talks(Mutex<HashMap<String, (Agent, Instant)>>);

impl Talks {
    pub fn spoke(&self, device: &str, agent: &Agent) {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(device.to_string(), (agent.clone(), Instant::now()));
    }

    fn last(&self, device: &str) -> Option<Agent> {
        let talks = self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        talks
            .get(device)
            .filter(|(_, at)| at.elapsed() < Duration::from_millis(TALK_MS))
            .map(|(agent, _)| agent.clone())
    }
}

/// An installed character's persona: its title and its text as the model's instructions on the place's channel.
pub type Characters = std::sync::Arc<dyn Fn(&str) -> Option<(String, String)> + Send + Sync>;

/// The room's own character of a device.
fn room_of(settings: &SettingsService, device: &str) -> Option<String> {
    settings.get(&crate::settings::definitions::ROOMS, None)?.remove(device)?.persona?.preset_id
}

/// The prompt of a character's text on a channel, as the characters' own prompts are built.
pub fn prompt_of(text: &str, channel: Channel) -> String {
    match channel {
        Channel::Station => crate::dialog::system_prompt::build_character_prompt(text),
        Channel::Computer => crate::dialog::system_prompt::build_computer_prompt(text),
    }
}

/// The agent a phrase is said to.
pub fn pick(
    phrase: &Phrase,
    settings: &Settings,
    store: &SettingsService,
    personas: &PersonaService,
    characters: Option<&Characters>,
    talks: &Talks,
) -> Agent {
    let channel = match phrase.place.kind {
        Kind::Station | Kind::Box => Channel::Station,
        _ => Channel::Computer,
    };
    let home = personas.chosen().and_then(|persona| persona.preset_id);
    let named = |id: &str, by: &'static str| -> Option<Agent> {
        let room = room_of(store, &phrase.place.device);
        // The room's or the home's own: their persona is the room's prompt as today.
        if Some(id) == room.as_deref() || Some(id) == home.as_deref() {
            return Some(Agent { id: Some(id.to_string()), by, title: None, prompt: None });
        }
        let (title, text) = characters.and_then(|characters| characters(id))?;
        Some(Agent { id: Some(id.to_string()), by, title: Some(title), prompt: Some(prompt_of(&text, channel)) })
    };
    for step in &settings.agents.order {
        match step.as_str() {
            "to" => {
                if let Some(agent) = phrase.to.agent.as_deref().and_then(|id| named(id, "to")) {
                    return agent;
                }
                // A reply in the window goes to whoever spoke last there, while the talk lasts.
                let in_window = phrase.opened == Opened::Window || phrase.to.by == By::Window;
                if in_window {
                    if let Some(agent) = talks.last(&phrase.place.device) {
                        return Agent { by: "window", ..agent };
                    }
                }
            }
            "room" => {
                if let Some(id) = room_of(store, &phrase.place.device) {
                    return Agent { id: Some(id), by: "room", title: None, prompt: None };
                }
            }
            "home" => return Agent { id: home, by: "home", title: None, prompt: None },
            _ => {}
        }
    }
    Agent { id: home, by: "home", title: None, prompt: None }
}

/// The history's key of a turn: the place's, or the agent's own beside it.
pub fn history_key(place_key: &str, agent: &Agent, settings: &Settings) -> String {
    match (settings.agents.history, agent.own(), &agent.id) {
        (HistoryView::Own, true, Some(id)) => format!("{place_key}#{id}"),
        _ => place_key.to_string(),
    }
}

/// The note the shared history keeps of another agent's turn: who answered.
pub fn note_of(agent: &Agent) -> Option<String> {
    agent.title.as_ref().filter(|_| agent.own()).map(|title| format!("Ответил(а) {title}."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::events::EventBus;
    use crate::storage::Db;
    use std::sync::Arc;

    fn phrase(agent: Option<&str>, opened: Opened) -> Phrase {
        let mut phrase: Phrase =
            serde_json::from_value(serde_json::json!({"turn": "t", "text": "поставь таймер", "where": {"device": "kitchen", "kind": "station"}, "opened": "word"}))
                .unwrap();
        phrase.to.agent = agent.map(str::to_string);
        phrase.opened = opened;
        phrase
    }

    #[test]
    fn a_named_agent_answers_its_talk_then_the_home_s_character_again() {
        let db = Arc::new(Db::memory());
        let store = SettingsService::new(db.clone(), EventBus::new());
        let personas = PersonaService::new(
            &crate::config::Config::for_tests(&crate::config::Env::from_pairs([("STATE_DIR", String::new())])).dialog,
            db,
            Arc::new(SettingsService::new(Arc::new(Db::memory()), EventBus::new())),
            EventBus::new(),
        )
        .unwrap();
        let characters: Characters =
            Arc::new(|id| (id == "butler:otto").then(|| ("Отто".to_string(), "Ты дворецкий Отто.".to_string())));
        let (settings, talks) = (Settings::default(), Talks::default());
        let otto =
            pick(&phrase(Some("butler:otto"), Opened::Word), &settings, &store, &personas, Some(&characters), &talks);
        assert_eq!((otto.id.as_deref(), otto.by), (Some("butler:otto"), "to"));
        assert!(otto.prompt.as_deref().is_some_and(|prompt| prompt.contains("Ты дворецкий Отто.")));
        talks.spoke("kitchen", &otto);
        let reply = pick(&phrase(None, Opened::Window), &settings, &store, &personas, Some(&characters), &talks);
        assert_eq!((reply.id.as_deref(), reply.by), (Some("butler:otto"), "window"), "a reply in the window: Otto's");
        let plain = pick(&phrase(None, Opened::Word), &settings, &store, &personas, Some(&characters), &talks);
        assert_eq!(plain.by, "home", "a new phrase that names nobody: the home's character");
        let unknown =
            pick(&phrase(Some("nobody:x"), Opened::Word), &settings, &store, &personas, Some(&characters), &talks);
        assert_eq!(unknown.by, "home", "an agent not installed: the home's");
        assert_eq!(history_key("kitchen", &otto, &settings), "kitchen", "shared by default");
        let mut own = Settings::default();
        own.agents.history = HistoryView::Own;
        assert_eq!(history_key("kitchen", &otto, &own), "kitchen#butler:otto");
        assert_eq!(note_of(&otto).as_deref(), Some("Ответил(а) Отто."));
    }
}
