//! Links, modes and the owner's words (docs/plans/bus.md): who reaches whom and how far. They are settings
//! (`bus.links`, `bus.modes`, `bus.options`, `bus.dangers`), so they travel with the settings package between the
//! computer and the station, and a move keeps them: an address names no place.
//!
//! A link names many by patterns (`app/morning` → `app/*`); for a pair, the narrowest link decides — the target's
//! pattern first, then the caller's — so the owner may close one door in a wide wall.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::address::{Address, Pattern};
use super::danger::DangerWord;
use super::level::Level;
use crate::core::events::Actor;
use crate::platform::http::ApiError;
use crate::settings::definitions::{BUS_DANGERS, BUS_LINKS, BUS_MODES, BUS_OPTIONS};
use crate::settings::SettingsService;
use crate::utils::time::now_iso;

/// Who made a link.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LinkSource {
    /// The owner, by hand.
    #[default]
    Owner,
    /// A person's «да» when the app first reached for it.
    Answer,
    /// A person's «нет» to that question: remembered, so the app does not ask again (the owner may change it).
    Refused,
    /// The install: a right of the package the admin approved with it.
    Install,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Link {
    pub from: Pattern,
    pub to: Pattern,
    pub level: Level,
    #[serde(default)]
    pub source: LinkSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    /// Who made it, for people.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
}

impl Link {
    pub fn new(from: Pattern, to: Pattern, level: Level, source: LinkSource) -> Link {
        Link { from, to, level, source, at: Some(now_iso()), by: None }
    }

    /// A person's word that stands: the owner's own link or a «нет». An app does not ask past it; a «да» is no such
    /// ceiling — an app that asks for more later is asked again.
    pub fn settled(&self) -> bool {
        matches!(self.source, LinkSource::Owner | LinkSource::Refused)
    }
}

/// How a participant talks to the others.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Talks to nobody, nobody reaches it: «Отдельно».
    Isolated,
    /// Only what it asked for and was allowed: «По правилам» (decision 1).
    #[default]
    Rules,
    /// Sees and calls everything open; the personal and the dangerous still go through people: «Свободно».
    Free,
}

impl Mode {
    pub fn parse(text: &str) -> Option<Mode> {
        match text {
            "isolated" => Some(Mode::Isolated),
            "rules" => Some(Mode::Rules),
            "free" => Some(Mode::Free),
            _ => None,
        }
    }
}

/// The house's mode and each participant's own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Modes {
    #[serde(default)]
    pub house: Mode,
    #[serde(default)]
    pub own: BTreeMap<String, Mode>,
}

/// The bus's switches.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Options {
    /// Apps may ask each other in words through a cloud model when no person started the chain (decision 4, off).
    #[serde(default)]
    pub cloud_asks: bool,
}

/// Links read out of their settings, a list skipping what does not parse.
pub fn parse_links(value: &Value) -> Option<Vec<Link>> {
    let list = value.as_array()?;
    Some(list.iter().filter_map(|item| serde_json::from_value(item.clone()).ok()).collect())
}

#[derive(Default)]
struct Cached {
    links: Vec<Link>,
    modes: Modes,
    options: Options,
    dangers: Vec<DangerWord>,
}

pub struct Links {
    settings: Arc<SettingsService>,
    cache: RwLock<Option<Arc<Cached>>>,
}

impl Links {
    pub fn new(settings: Arc<SettingsService>) -> Links {
        Links { settings, cache: RwLock::new(None) }
    }

    fn read(&self) -> Arc<Cached> {
        if let Some(cached) = self.cache.read().unwrap_or_else(|poisoned| poisoned.into_inner()).as_ref() {
            return cached.clone();
        }
        let cached = Arc::new(Cached {
            links: self.settings.get(&BUS_LINKS, None).unwrap_or_default(),
            modes: self.settings.get(&BUS_MODES, None).unwrap_or_default(),
            options: self.settings.get(&BUS_OPTIONS, None).unwrap_or_default(),
            dangers: self.settings.get(&BUS_DANGERS, None).unwrap_or_default(),
        });
        *self.cache.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(cached.clone());
        cached
    }

    /// Reads the settings again on the next question: they changed past us (an import, the settings' API).
    pub fn forget(&self) {
        *self.cache.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }

    pub fn all(&self) -> Vec<Link> {
        self.read().links.clone()
    }

    /// The link that decides for a pair: the narrowest by the target, then by the caller. None when no link covers
    /// it.
    pub fn link(&self, from: &Address, to: &Address) -> Option<Link> {
        self.read()
            .links
            .iter()
            .filter(|link| link.from.matches(from) && link.to.matches(to))
            .max_by_key(|link| (link.to.specificity(), link.from.specificity()))
            .cloned()
    }

    pub fn level(&self, from: &Address, to: &Address) -> Level {
        self.link(from, to).map(|link| link.level).unwrap_or(Level::None)
    }

    pub fn modes(&self) -> Modes {
        self.read().modes.clone()
    }

    /// A participant's mode: its own, or the house's.
    pub fn mode(&self, address: &Address) -> Mode {
        let cached = self.read();
        cached.modes.own.get(address.as_str()).copied().unwrap_or(cached.modes.house)
    }

    pub fn options(&self) -> Options {
        self.read().options.clone()
    }

    pub fn dangers(&self) -> Vec<DangerWord> {
        self.read().dangers.clone()
    }

    fn write_links(&self, links: &Vec<Link>, actor: Option<Actor>) -> Result<(), ApiError> {
        self.settings.set(&BUS_LINKS, links, actor)?;
        self.forget();
        Ok(())
    }

    /// Puts a link, replacing the one for the same pair of patterns.
    pub fn grant(&self, link: Link, actor: Option<Actor>) -> Result<(), ApiError> {
        let mut links = self.all();
        links.retain(|old| !(old.from == link.from && old.to == link.to));
        links.push(link);
        self.write_links(&links, actor)
    }

    /// Takes a link away; whether there was one.
    pub fn remove(&self, from: &Pattern, to: &Pattern, actor: Option<Actor>) -> Result<bool, ApiError> {
        let mut links = self.all();
        let before = links.len();
        links.retain(|old| !(&old.from == from && &old.to == to));
        if links.len() == before {
            return Ok(false);
        }
        self.write_links(&links, actor)?;
        Ok(true)
    }

    /// Everything a participant had: its links both ways and its mode (an app removed).
    pub fn forget_participant(&self, address: &Address, actor: Option<Actor>) -> Result<(), ApiError> {
        let exact = Pattern::exact(address);
        let mut links = self.all();
        let before = links.len();
        links.retain(|old| old.from != exact && old.to != exact);
        if links.len() != before {
            self.write_links(&links, actor.clone())?;
        }
        let mut modes = self.modes();
        if modes.own.remove(address.as_str()).is_some() {
            self.settings.set(&BUS_MODES, &modes, actor)?;
            self.forget();
        }
        Ok(())
    }

    /// The house's mode (`address` none) or a participant's own; `None` as a mode drops the participant's own.
    pub fn set_mode(
        &self,
        address: Option<&Address>,
        mode: Option<Mode>,
        actor: Option<Actor>,
    ) -> Result<(), ApiError> {
        let mut modes = self.modes();
        match (address, mode) {
            (None, Some(mode)) => modes.house = mode,
            (None, None) => modes.house = Mode::default(),
            (Some(address), Some(mode)) => {
                modes.own.insert(address.to_string(), mode);
            }
            (Some(address), None) => {
                modes.own.remove(address.as_str());
            }
        }
        self.settings.set(&BUS_MODES, &modes, actor)?;
        self.forget();
        Ok(())
    }

    pub fn set_options(&self, options: &Options, actor: Option<Actor>) -> Result<(), ApiError> {
        self.settings.set(&BUS_OPTIONS, options, actor)?;
        self.forget();
        Ok(())
    }

    /// Puts the owner's word on one handle for one caller, replacing the one said before for them.
    pub fn say_danger(&self, word: DangerWord, actor: Option<Actor>) -> Result<(), ApiError> {
        let mut words = self.dangers();
        words.retain(|old| !(old.target == word.target && old.caller == word.caller));
        if word.always || word.mark.is_some() {
            words.push(word);
        }
        self.settings.set(&BUS_DANGERS, &words, actor)?;
        self.forget();
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::core::events::EventBus;
    use crate::storage::Db;

    pub(crate) fn links() -> Links {
        let db = Arc::new(Db::open(None).unwrap());
        Links::new(Arc::new(SettingsService::new(db, EventBus::new())))
    }

    fn pattern(text: &str) -> Pattern {
        Pattern::parse(text).unwrap()
    }

    fn address(text: &str) -> Address {
        Address::parse(text).unwrap()
    }

    #[test]
    fn the_narrowest_link_decides() {
        let links = links();
        links.grant(Link::new(pattern("app/morning"), pattern("app/*"), Level::Call, LinkSource::Owner), None).unwrap();
        links
            .grant(Link::new(pattern("app/morning"), pattern("app/lock"), Level::None, LinkSource::Owner), None)
            .unwrap();
        links.grant(Link::new(pattern("*"), pattern("app/weather"), Level::See, LinkSource::Owner), None).unwrap();
        assert_eq!(links.level(&address("app/morning"), &address("app/news")), Level::Call);
        assert_eq!(links.level(&address("app/morning"), &address("app/lock")), Level::None);
        assert_eq!(links.level(&address("app/other"), &address("app/weather")), Level::See);
        assert_eq!(links.level(&address("app/other"), &address("app/news")), Level::None);
        // The same pair of patterns again replaces, never doubles.
        links
            .grant(Link::new(pattern("app/morning"), pattern("app/*"), Level::Listen, LinkSource::Answer), None)
            .unwrap();
        assert_eq!(links.all().len(), 3);
        assert_eq!(links.level(&address("app/morning"), &address("app/news")), Level::Listen);
        assert!(links.remove(&pattern("app/morning"), &pattern("app/*"), None).unwrap());
        assert!(!links.remove(&pattern("app/morning"), &pattern("app/*"), None).unwrap());
    }

    #[test]
    fn modes_default_to_the_houses_rules_and_a_removed_app_leaves_nothing() {
        let links = links();
        let app = address("app/radio");
        assert_eq!(links.mode(&app), Mode::Rules);
        links.set_mode(Some(&app), Some(Mode::Isolated), None).unwrap();
        links.set_mode(None, Some(Mode::Free), None).unwrap();
        assert_eq!(links.mode(&app), Mode::Isolated);
        assert_eq!(links.mode(&address("app/other")), Mode::Free);
        links
            .grant(Link::new(Pattern::exact(&app), pattern("system/notify"), Level::Call, LinkSource::Install), None)
            .unwrap();
        links.forget_participant(&app, None).unwrap();
        assert_eq!(links.mode(&app), Mode::Free);
        assert!(links.all().is_empty());
        assert!(!links.options().cloud_asks);
    }

    #[test]
    fn a_link_that_does_not_parse_is_skipped_not_the_whole_list() {
        let parsed = parse_links(&serde_json::json!([
            {"from": "app/a", "to": "app/b", "level": "call"},
            {"from": "App/bad", "to": "app/b", "level": "call"},
            {"from": "app/a", "to": "app/c", "level": "fly"}
        ]))
        .unwrap();
        assert_eq!(parsed.len(), 1);
    }
}
