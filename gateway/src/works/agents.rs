//! The agents of the cards (docs/plans/router-and-works.md, 7б; decision 19): a card from MagicPlace — a package of
//! the kind `agent` — says what program to install, how to run it over ACP and how to check its sign-in; the socket
//! runs it through `AcpAgent`. Only cards installed from the catalog are taken (a card names a program to run). The
//! program is looked for by the path the panel set (`harness_paths`), else by its name on the search path. Installing
//! a card downloads and runs nothing; the person installs the agent's program themselves.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock};

use super::harness::acp::{AcpAgent, Known};
use crate::plugins::manifest::AgentCard;
use crate::settings::definitions::HARNESS_PATHS;
use crate::settings::SettingsService;

/// An installed card: its id, title, description and the card itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub card: AgentCard,
}

/// The cards installed from the catalog now (the plugins' host, app.rs).
pub type Cards = Arc<dyn Fn() -> Vec<Card> + Send + Sync>;

pub struct Agents {
    cards: OnceLock<Cards>,
    settings: Arc<SettingsService>,
    known: Known,
}

impl Agents {
    pub fn new(settings: Arc<SettingsService>) -> Agents {
        Agents { cards: OnceLock::new(), settings, known: Arc::new(Mutex::new(HashMap::new())) }
    }

    pub fn set_cards(&self, cards: Cards) {
        let _ = self.cards.set(cards);
    }

    pub fn cards(&self) -> Vec<Card> {
        self.cards.get().map(|cards| cards()).unwrap_or_default()
    }

    fn paths(&self) -> BTreeMap<String, String> {
        self.settings.get(&HARNESS_PATHS, None).unwrap_or_default()
    }

    fn agent(&self, card: Card, paths: &BTreeMap<String, String>) -> Arc<AcpAgent> {
        let path = paths.get(&card.id).cloned();
        Arc::new(AcpAgent::new(card.id, card.title, card.card, path, self.known.clone()))
    }

    pub fn list(&self) -> Vec<Arc<AcpAgent>> {
        let paths = self.paths();
        self.cards().into_iter().map(|card| self.agent(card, &paths)).collect()
    }

    pub fn get(&self, id: &str) -> Option<Arc<AcpAgent>> {
        let paths = self.paths();
        self.cards().into_iter().find(|card| card.id == id).map(|card| self.agent(card, &paths))
    }

    /// A card's program found anew (its path set or changed): what was learned of its protocol is forgotten.
    pub fn forget(&self, id: &str) {
        self.known.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(id);
    }
}
