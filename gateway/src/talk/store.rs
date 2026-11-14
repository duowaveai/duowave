//! MagicPlace by voice (docs/plans/magicplace.md, «Голос»). The store opens by any name people know a store by — the
//! names are the language pack's and never on the screen: they are other companies' marks. A package of the catalog
//! installs after a spoken «да», the same way as the panel's button does and under the same switch as the desk by
//! voice (`talk.desk_by_voice`): the install gives an app no rights — it asks for them in its window, in the panel.
//! «Что обновилось» names the new versions and updates them after «да»; «обнови всё» updates at once; «что нового в
//! магазине» names what came into the catalog in a month.
//!
//! The rule hears without the model, beside the layer's own rules (rules.rs), so a station's phrase and a computer's
//! are heard alike; a name it hears is looked up in the catalog's copy here. «Поставь X» is the store's only when X is
//! a package of the catalog not installed here — «поставь радио» is the radio's; «установи X» when X is in the
//! catalog. The panel opens the store on `magicplace.open`, on the package when there is one.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, OnceLock, Weak};
use std::time::Instant;

use regex::Regex;
use serde::{Deserialize, Serialize};

use super::pack::{self, Pack, StoreWords};
use super::ready::Answer;
use crate::core::events::{Actor, EventBus, GatewayEvent};
use crate::memory::text::Lang;
use crate::plugins::catalog::{CatalogEntryView, CatalogView, EntryState, PackageCatalog};
use crate::settings::definitions::TALK_DESK_BY_VOICE;
use crate::settings::SettingsService;

/// «Новое» is what came into the catalog this many days ago or less.
const NEW_DAYS: i64 = 30;
/// «Что нового» names this many at most.
const NEW_SAID: usize = 3;

/// What the store heard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "store", rename_all = "camelCase")]
pub enum StoreHeard {
    /// The store, in the panel.
    Open,
    /// Install the package named: its id when the catalog has it; else the words, to say there is none.
    Install {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        name: String,
    },
    /// «Что обновилось» (`now`: false — the new versions and the question) or «обнови всё» (`now`: true).
    Updates { now: bool },
    /// «Что нового в магазине».
    New,
    /// The answer to the store's question on the device.
    Answer { yes: bool },
}

/// A pack's store words, compiled once.
struct Compiled {
    open: Vec<Regex>,
    install: Vec<Regex>,
    character: Vec<Regex>,
    install_maybe: Vec<Regex>,
    put_maybe: Vec<Regex>,
    updates: Vec<Regex>,
    update_all: Vec<Regex>,
    new: Vec<Regex>,
}

fn compile(patterns: &[&str]) -> Vec<Regex> {
    patterns.iter().map(|pattern| Regex::new(pattern).expect("a valid pattern of a pack")).collect()
}

impl Compiled {
    fn of(words: &StoreWords) -> Compiled {
        Compiled {
            open: compile(words.open),
            install: compile(words.install),
            character: compile(words.character),
            install_maybe: compile(words.install_maybe),
            put_maybe: compile(words.put_maybe),
            updates: compile(words.updates),
            update_all: compile(words.update_all),
            new: compile(words.new),
        }
    }
}

static RU: LazyLock<Compiled> = LazyLock::new(|| Compiled::of(pack::of(Lang::Ru).store));
static EN: LazyLock<Compiled> = LazyLock::new(|| Compiled::of(pack::of(Lang::En).store));

fn compiled(lang: Lang) -> &'static Compiled {
    match lang {
        Lang::En => &EN,
        Lang::Ru | Lang::Other => &RU,
    }
}

fn matches(rules: &[Regex], text: &str) -> bool {
    rules.iter().any(|rule| rule.is_match(text))
}

/// The name a rule of the list takes from the whole phrase.
fn name_in(rules: &[Regex], text: &str) -> Option<String> {
    rules.iter().find_map(|rule| {
        let name = rule.captures(text)?.name("name")?.as_str().trim();
        (!name.is_empty()).then(|| name.to_string())
    })
}

/// Whether a phrase as it was said names Yandex: «Яндекс Маркет», «Яндекс, открой маркет» are Yandex's to answer.
/// The router's text drops a leading «Яндекс» as a wake word, so the store asks the phrase itself.
pub fn names_yandex(said: &str) -> bool {
    said.to_lowercase().split(|c: char| !c.is_alphanumeric()).any(|word| word == "яндекс" || word == "yandex")
}

/// A question waiting for the device's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Waiting {
    Install { id: String, version: String, title: String },
    Update { packages: Vec<(String, String)> },
}

/// Who opens the store: a panel listens — it opens there — or none does.
pub type Panels = Arc<dyn Fn() -> bool + Send + Sync>;

pub struct Store {
    events: EventBus,
    catalog: OnceLock<Weak<PackageCatalog>>,
    settings: OnceLock<Arc<SettingsService>>,
    panels: OnceLock<Panels>,
    /// The question each device was asked, and when.
    asked: Mutex<HashMap<String, (Waiting, Instant)>>,
}

impl Store {
    pub fn new(events: EventBus) -> Store {
        Store {
            events,
            catalog: OnceLock::new(),
            settings: OnceLock::new(),
            panels: OnceLock::new(),
            asked: Mutex::new(HashMap::new()),
        }
    }

    /// Once the catalog stands: what it installs from, the switch it obeys, whether a panel listens.
    pub fn attach(&self, catalog: Weak<PackageCatalog>, settings: Arc<SettingsService>, panels: Panels) {
        let _ = self.catalog.set(catalog);
        let _ = self.settings.set(settings);
        let _ = self.panels.set(panels);
    }

    fn catalog(&self) -> Option<Arc<PackageCatalog>> {
        self.catalog.get().and_then(Weak::upgrade)
    }

    fn questions(&self) -> std::sync::MutexGuard<'_, HashMap<String, (Waiting, Instant)>> {
        self.asked.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Whether a «да» said to a device installs (`talk.desk_by_voice`, on by default).
    fn by_voice(&self) -> bool {
        self.settings.get().is_none_or(|settings| settings.get(&TALK_DESK_BY_VOICE, None).unwrap_or(true))
    }

    fn panel_open(&self) -> bool {
        self.panels.get().is_some_and(|open| open())
    }

    /// What the store hears in a phrase (normalized) on a device speaking `lang`; None — not the store's.
    pub fn hear(&self, lang: Lang, text: &str) -> Option<StoreHeard> {
        let catalog = self.catalog();
        hear_with(lang, text, |name, character| catalog.as_ref().and_then(|catalog| catalog.named(name, character)))
    }

    /// The answer to the device's question, as the pack reads it; None — nothing is asked, or it is not an answer.
    pub fn answer_of(&self, device: &str, pack: &Pack, words: &str) -> Option<StoreHeard> {
        {
            let mut questions = self.questions();
            match questions.get(device) {
                Some((_, at)) if at.elapsed() < super::asked::answer_for(device) => {}
                Some(_) => {
                    questions.remove(device);
                    return None;
                }
                None => return None,
            }
        }
        match super::ready::answer_of(pack, words)? {
            Answer::Yes => Some(StoreHeard::Answer { yes: true }),
            Answer::No | Answer::Drop | Answer::Park => Some(StoreHeard::Answer { yes: false }),
            // «Покажи», «открой»: the store, on the package the question is about — it is open there already.
            Answer::Fit => Some(StoreHeard::Open),
            _ => None,
        }
    }

    /// The panel opens the store — on a package, with its install sheet — wherever one listens: whether one does.
    fn open(&self, device: &str, item: Option<&str>, install: bool) -> bool {
        self.events.emit(GatewayEvent::MagicPlaceOpen {
            item: item.map(str::to_string),
            install,
            device: device.to_string(),
        });
        self.panel_open()
    }

    fn ask(&self, device: &str, waiting: Waiting) {
        self.questions().insert(device.to_string(), (waiting, Instant::now()));
    }

    /// Acts on what was heard: the phrase to say, and whether it asks a question (the device listens after it).
    pub async fn act(&self, heard: StoreHeard, device: &str, lang: Lang) -> (String, bool) {
        let pack = pack::of(lang);
        let say = &pack.store.say;
        match heard {
            StoreHeard::Open => {
                let open = self.open(device, None, false);
                ((if open { say.opened } else { say.where_is }).to_string(), false)
            }
            StoreHeard::Install { id: None, name } => {
                let open = self.open(device, None, false);
                let mut said = pack::fill(say.not_found, &[("name", &name)]);
                if open {
                    said.push_str(say.look_in_panel);
                }
                (said, false)
            }
            StoreHeard::Install { id: Some(id), name } => self.install(device, pack, &id, &name).await,
            StoreHeard::Updates { now } => self.updates(device, pack, now).await,
            StoreHeard::New => (self.whats_new(pack).await, false),
            StoreHeard::Answer { yes } => (self.answered(device, pack, yes).await, false),
        }
    }

    /// The catalog as the panel sees it, or what to say when there is none to install from.
    async fn view(&self, pack: &Pack) -> Result<(Arc<PackageCatalog>, CatalogView), String> {
        let off = || pack.store.say.catalog_off.to_string();
        let catalog = self.catalog().ok_or_else(off)?;
        let view = catalog.view(false).await;
        if !view.configured || !view.enabled {
            return Err(off());
        }
        Ok((catalog, view))
    }

    async fn install(&self, device: &str, pack: &Pack, id: &str, name: &str) -> (String, bool) {
        let say = &pack.store.say;
        let (_, view) = match self.view(pack).await {
            Ok(found) => found,
            Err(said) => return (said, false),
        };
        let Some(entry) = view.packages.iter().find(|entry| entry.id == id) else {
            return (pack::fill(say.not_found, &[("name", name)]), false);
        };
        let title = entry.title.as_str();
        match entry.state {
            EntryState::Installed => (pack::fill(say.already, &[("title", title)]), false),
            EntryState::Update => {
                let newer = pack::fill(say.already_newer, &[("title", title), ("version", &entry.version)]);
                self.open(device, Some(id), false);
                if !self.by_voice() {
                    return (format!("{newer}{}", say.update_in_panel), false);
                }
                self.ask(device, Waiting::Update { packages: vec![(entry.id.clone(), entry.title.clone())] });
                (format!("{newer}{}", say.update_one), true)
            }
            EntryState::Incompatible | EntryState::NeedsSignature => {
                let why = entry.reason.clone().unwrap_or_default();
                (pack::fill(say.not_here, &[("title", title), ("why", why.trim_end_matches('.'))]) + ".", false)
            }
            EntryState::Available => {
                let open = self.open(device, Some(id), true);
                if !self.by_voice() {
                    let template = if open { say.in_panel } else { say.in_panel_closed };
                    return (pack::fill(template, &[("title", title)]), false);
                }
                self.ask(
                    device,
                    Waiting::Install {
                        id: entry.id.clone(),
                        version: entry.version.clone(),
                        title: entry.title.clone(),
                    },
                );
                (question(pack, entry), true)
            }
        }
    }

    async fn updates(&self, device: &str, pack: &Pack, now: bool) -> (String, bool) {
        let say = &pack.store.say;
        let (catalog, view) = match self.view(pack).await {
            Ok(found) => found,
            Err(said) => return (said, false),
        };
        let newer: Vec<(String, String)> = view
            .packages
            .iter()
            .filter(|entry| entry.state == EntryState::Update)
            .map(|entry| (entry.id.clone(), entry.title.clone()))
            .collect();
        let waiting: Vec<&str> =
            view.packages.iter().filter(|entry| entry.waits.is_some()).map(|entry| entry.title.as_str()).collect();
        let waits =
            if waiting.is_empty() { String::new() } else { pack::fill(say.waits, &[("list", &list(pack, &waiting))]) };
        if newer.is_empty() {
            return (format!("{}{waits}", say.fresh), false);
        }
        let titles: Vec<&str> = newer.iter().map(|(_, title)| title.as_str()).collect();
        let named = pack::fill(say.updates, &[("list", &list(pack, &titles))]);
        if !self.by_voice() {
            self.open(device, None, false);
            return (format!("{named}{waits}{}", say.update_in_panel), false);
        }
        // «Обнови всё» was the question's answer said first: no question again.
        if now {
            return (self.update(&catalog, device, pack, &newer).await, false);
        }
        self.open(device, None, false);
        self.ask(device, Waiting::Update { packages: newer });
        (format!("{named}{waits}{}", say.update_all), true)
    }

    /// Updates these, one after another: what was updated and what was not.
    async fn update(
        &self,
        catalog: &PackageCatalog,
        device: &str,
        pack: &Pack,
        packages: &[(String, String)],
    ) -> String {
        let say = &pack.store.say;
        let (mut done, mut failed) = (Vec::new(), Vec::new());
        for (id, title) in packages {
            match catalog.install(id, None, true, Some(voice_actor(device))).await {
                Ok(_) => done.push(title.as_str()),
                Err(error) => {
                    tracing::warn!(target: "plugins", "MagicPlace by voice: {id} not updated: {}", error.message);
                    failed.push(title.as_str());
                }
            }
        }
        let mut said =
            if done.is_empty() { String::new() } else { pack::fill(say.updated, &[("list", &list(pack, &done))]) };
        if !failed.is_empty() {
            said.push_str(&pack::fill(say.update_failed, &[("list", &list(pack, &failed))]));
        }
        said.trim().to_string()
    }

    async fn whats_new(&self, pack: &Pack) -> String {
        let say = &pack.store.say;
        let view = match self.view(pack).await {
            Ok((_, view)) => view,
            Err(said) => return said,
        };
        let since = chrono::Utc::now() - chrono::Duration::days(NEW_DAYS);
        let mut fresh: Vec<(&CatalogEntryView, chrono::DateTime<chrono::FixedOffset>)> = view
            .packages
            .iter()
            .filter(|entry| entry.state == EntryState::Available)
            .filter_map(|entry| {
                let at = chrono::DateTime::parse_from_rfc3339(entry.added_at.as_deref()?).ok()?;
                (at >= since).then_some((entry, at))
            })
            .collect();
        fresh.sort_by_key(|(_, at)| std::cmp::Reverse(*at));
        if fresh.is_empty() {
            return say.nothing_new.to_string();
        }
        let titles: Vec<&str> = fresh.iter().take(NEW_SAID).map(|(entry, _)| entry.title.as_str()).collect();
        pack::fill(say.new, &[("list", &list(pack, &titles))])
    }

    async fn answered(&self, device: &str, pack: &Pack, yes: bool) -> String {
        let say = &pack.store.say;
        let Some((waiting, _)) = self.questions().remove(device) else { return String::new() };
        if !yes {
            return say.declined.to_string();
        }
        let Some(catalog) = self.catalog() else { return say.catalog_off.to_string() };
        match waiting {
            Waiting::Install { id, version, title } => {
                match catalog.install(&id, Some(&version), false, Some(voice_actor(device))).await {
                    Ok(_) => pack::fill(say.installed, &[("title", &title)]),
                    Err(error) => {
                        pack::fill(
                            say.install_failed,
                            &[("title", &title), ("why", error.message.trim_end_matches('.'))],
                        ) + "."
                    }
                }
            }
            Waiting::Update { packages } => self.update(&catalog, device, pack, &packages).await,
        }
    }
}

/// Who installed by voice, for the audit log: the household at that device (as «На стол голосом»).
fn voice_actor(device: &str) -> Actor {
    Actor {
        kind: crate::core::identity::principal::PrincipalKind::Household,
        id: format!("voice:{device}"),
        name: crate::tr!("голосом, {device}", device = device),
    }
}

/// What the store hears in a phrase, with the catalog's name lookup given: (id, installed here) of the package these
/// words name; `character` — characters only.
pub fn hear_with(lang: Lang, text: &str, named: impl Fn(&str, bool) -> Option<(String, bool)>) -> Option<StoreHeard> {
    let rules = compiled(lang);
    let text = text.trim();
    if matches(&rules.open, text) {
        return Some(StoreHeard::Open);
    }
    if matches(&rules.update_all, text) {
        return Some(StoreHeard::Updates { now: true });
    }
    if matches(&rules.updates, text) {
        return Some(StoreHeard::Updates { now: false });
    }
    if matches(&rules.new, text) {
        return Some(StoreHeard::New);
    }
    if let Some(name) = name_in(&rules.character, text) {
        return Some(StoreHeard::Install { id: named(&name, true).map(|(id, _)| id), name });
    }
    if let Some(name) = name_in(&rules.install, text) {
        return Some(StoreHeard::Install { id: named(&name, false).map(|(id, _)| id), name });
    }
    if let Some(name) = name_in(&rules.install_maybe, text) {
        if let Some((id, _)) = named(&name, false) {
            return Some(StoreHeard::Install { id: Some(id), name });
        }
    }
    if let Some(name) = name_in(&rules.put_maybe, text) {
        if let Some((id, false)) = named(&name, false) {
            return Some(StoreHeard::Install { id: Some(id), name });
        }
    }
    None
}

/// The question before an install: what it is, what it will download, that it asks its rights itself.
fn question(pack: &Pack, entry: &CatalogEntryView) -> String {
    let say = &pack.store.say;
    let about = entry.description.as_deref().map(first_sentence).filter(|about| !about.is_empty());
    let about = about.map(|about| format!(": {about}")).unwrap_or_default();
    let downloads: Vec<String> = entry
        .downloads
        .iter()
        .map(|download| {
            let template = if download.when == "install" { say.download_now } else { say.download_later };
            pack::fill(template, &[("title", &download.title), ("mb", &megabytes(pack, download.mb))])
        })
        .collect();
    let downloads = if downloads.is_empty() {
        String::new()
    } else {
        pack::fill(say.downloads, &[("list", &downloads.join("; "))])
    };
    let rights = if entry.place.is_some() && !entry.permissions.is_empty() { say.rights } else { "" };
    pack::fill(
        say.ask_install,
        &[("title", &entry.title), ("about", &about), ("downloads", &downloads), ("rights", rights)],
    )
}

/// «50 мегабайт», «1 megabyte».
fn megabytes(pack: &Pack, mb: u64) -> String {
    format!("{mb} {}", pack.store.say.megabytes[(pack.work.say.plural)(mb)])
}

/// A description's first sentence, said after the title: up to its first stop, «:» or «;», the first letter small
/// when its word is an ordinary one («Мини-сериалы» — «мини-сериалы»; «DuoWave», «AI» stay).
fn first_sentence(text: &str) -> String {
    let end = text.find(['.', '!', '?', ':', ';', '\n']).unwrap_or(text.len());
    let sentence = text[..end].trim();
    let mut chars = sentence.chars();
    let Some(first) = chars.next() else { return String::new() };
    let rest: String = chars.collect();
    let word = rest.split(char::is_whitespace).next().unwrap_or_default();
    let ordinary = !word.is_empty() && !word.chars().any(char::is_uppercase);
    if ordinary {
        first.to_lowercase().chain(rest.chars()).collect()
    } else {
        sentence.to_string()
    }
}

/// Titles in their quotes: «Лента», «Артур» и «Бублик».
fn list(pack: &Pack, titles: &[&str]) -> String {
    let (open, close) = pack.store.say.quotes;
    let quoted: Vec<String> = titles.iter().map(|title| format!("{open}{title}{close}")).collect();
    match quoted.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{}{}{last}", rest.join(", "), pack.store.say.and),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::text::normalize;

    /// The catalog of the tests: «Лента» (feed, English «Feed») not installed, «Артур» (a character) installed.
    fn named(name: &str, character: bool) -> Option<(String, bool)> {
        let said = normalize(name);
        let alike = |title: &str| {
            let title = normalize(title);
            title.split(' ').all(|word| said.split(' ').any(|said| crate::skills::text::alike(said, word)))
                && said.split(' ').all(|said| title.split(' ').any(|word| crate::skills::text::alike(said, word)))
        };
        if !character && (alike("Лента") || alike("Feed")) {
            return Some(("feed".into(), false));
        }
        if alike("Артур") {
            return Some(("arthur".into(), true));
        }
        None
    }

    fn heard(lang: Lang, said: &str) -> Option<StoreHeard> {
        hear_with(lang, &normalize(said), named)
    }

    #[test]
    fn every_name_of_a_store_opens_magicplace_in_its_language() {
        for said in [
            "магазин",
            "Открой магазин приложений",
            "каталог",
            "маркет",
            "плей маркет",
            "плеймаркет",
            "открой гугл плей",
            "плей стор",
            "апп стор",
            "эпп стор",
            "аппстор",
            "рустор",
            "галерея приложений",
            "MagicPlace",
            "мэджик плейс",
            "меджик плейс",
            "мейджик плейс",
            "зайди в app store",
            "открой Google Play",
            "Алиса, открой магазин",
        ] {
            assert_eq!(heard(Lang::Ru, said), Some(StoreHeard::Open), "{said}");
        }
        for said in [
            "store",
            "app store",
            "Open the App Store",
            "play store",
            "play market",
            "google play",
            "market",
            "app gallery",
            "galaxy store",
            "open magic place",
        ] {
            assert_eq!(heard(Lang::En, said), Some(StoreHeard::Open), "{said}");
        }
        assert_eq!(heard(Lang::Ru, "что купить в магазине"), None, "a shopping list's words stay theirs");
        assert_eq!(heard(Lang::Ru, "открой яндекс маркет"), None, "Yandex's own store is not ours");
    }

    #[test]
    fn install_takes_a_package_of_the_catalog_by_any_of_its_names() {
        let feed = Some(StoreHeard::Install { id: Some("feed".into()), name: "ленту".into() });
        assert_eq!(heard(Lang::Ru, "поставь ленту"), feed);
        assert_eq!(heard(Lang::Ru, "установи ленту"), feed);
        assert_eq!(
            heard(Lang::Ru, "поставь Feed"),
            Some(StoreHeard::Install { id: Some("feed".into()), name: "feed".into() }),
            "an English title in a Russian phrase"
        );
        assert_eq!(
            heard(Lang::En, "install the feed"),
            Some(StoreHeard::Install { id: Some("feed".into()), name: "feed".into() })
        );
        assert_eq!(
            heard(Lang::Ru, "поставь персонажа Артура"),
            Some(StoreHeard::Install { id: Some("arthur".into()), name: "артура".into() })
        );
        // «Поставь» also plays and sets: what the catalog has not stays the radio's, the timer's, the music's.
        assert_eq!(heard(Lang::Ru, "поставь радио"), None);
        assert_eq!(heard(Lang::Ru, "поставь сказку"), None);
        assert_eq!(heard(Lang::Ru, "установи будильник на 7"), None);
        assert_eq!(heard(Lang::Ru, "поставь артура"), None, "installed already: «поставь» is not the store's");
        assert_eq!(
            heard(Lang::Ru, "установи артура"),
            Some(StoreHeard::Install { id: Some("arthur".into()), name: "артура".into() }),
            "«установи» is: it says «уже стоит»"
        );
        // Asked of the store by name — the store answers there is none.
        assert_eq!(
            heard(Lang::Ru, "поставь приложение калькулятор"),
            Some(StoreHeard::Install { id: None, name: "калькулятор".into() })
        );
        assert_eq!(
            heard(Lang::Ru, "установи калькулятор из магазина"),
            Some(StoreHeard::Install { id: None, name: "калькулятор".into() })
        );
    }

    #[test]
    fn updates_and_whats_new() {
        assert_eq!(heard(Lang::Ru, "что обновилось"), Some(StoreHeard::Updates { now: false }));
        assert_eq!(heard(Lang::Ru, "есть обновления?"), Some(StoreHeard::Updates { now: false }));
        assert_eq!(heard(Lang::Ru, "обнови всё"), Some(StoreHeard::Updates { now: true }));
        assert_eq!(heard(Lang::Ru, "что нового в магазине"), Some(StoreHeard::New));
        assert_eq!(heard(Lang::En, "what's updated"), Some(StoreHeard::Updates { now: false }));
        assert_eq!(heard(Lang::En, "any updates"), Some(StoreHeard::Updates { now: false }));
        assert_eq!(heard(Lang::En, "update all"), Some(StoreHeard::Updates { now: true }));
        assert_eq!(heard(Lang::En, "what's new in the store"), Some(StoreHeard::New));
        assert_eq!(heard(Lang::Ru, "что нового в ленте"), None, "the feed's own phrase");
    }

    #[test]
    fn yandex_named_in_the_phrase_is_yandexs() {
        assert!(
            names_yandex("Яндекс Маркет") && names_yandex("Яндекс, открой маркет") && names_yandex("yandex market")
        );
        assert!(!names_yandex("Алиса, открой маркет") && !names_yandex("плей маркет"));
    }

    #[test]
    fn the_question_names_the_package_what_it_downloads_and_its_rights() {
        use crate::plugins::catalog::{DownloadView, PreviewView};
        let download = |id: &str, title: &str, mb: u64, when: &str| DownloadView {
            id: id.into(),
            title: title.into(),
            mb,
            when: when.into(),
        };
        let entry = CatalogEntryView {
            id: "feed".into(),
            kind: crate::plugins::manifest::PackageKind::App,
            version: "0.4.1".into(),
            title: "Лента".into(),
            description: Some("Мини-сериалы, которые снимает для вас ваш компьютер: выбираете развилку.".into()),
            license: None,
            verified: true,
            runs: vec!["pc"],
            min_gateway: Some("0.9.0".into()),
            needs: serde_json::json!([]),
            preview: PreviewView { image: None, samples: Vec::new() },
            installed: None,
            state: EntryState::Available,
            reason: None,
            publisher: None,
            source: None,
            own: false,
            runtime: Some("node".into()),
            permissions: vec!["call:system/brain".into()],
            signed: true,
            faces: vec!["widget".into()],
            place: Some("computer"),
            recommended: true,
            added_at: None,
            texts: Vec::new(),
            downloads: vec![
                download("node", "Node", 50, "install"),
                download("render", "Remotion и браузер для рендера серий", 250, "first-use"),
            ],
            waits: None,
        };
        assert_eq!(
            question(pack::of(Lang::Ru), &entry),
            "«Лента»: мини-сериалы, которые снимает для вас ваш компьютер. Скачает Node — около 50 мегабайт, сразу; \
             Remotion и браузер для рендера серий — около 250 мегабайт, при первом использовании. Свои права спросит \
             в окне. Поставить?"
        );
        let character = CatalogEntryView { place: None, permissions: Vec::new(), downloads: Vec::new(), ..entry };
        assert_eq!(
            question(pack::of(Lang::Ru), &character),
            "«Лента»: мини-сериалы, которые снимает для вас ваш компьютер. Поставить?"
        );
    }

    #[test]
    fn lists_join_with_the_packs_and() {
        let ru = pack::of(Lang::Ru);
        assert_eq!(list(ru, &["Лента"]), "«Лента»");
        assert_eq!(list(ru, &["Лента", "Артур", "Бублик"]), "«Лента», «Артур» и «Бублик»");
        assert_eq!(list(pack::of(Lang::En), &["Feed", "Arthur"]), "\"Feed\" and \"Arthur\"");
        assert_eq!(megabytes(ru, 1), "1 мегабайт");
        assert_eq!(megabytes(ru, 3), "3 мегабайта");
        assert_eq!(first_sentence("DuoWave: всё дома"), "DuoWave");
    }

    #[test]
    fn an_answer_counts_only_while_the_question_waits() {
        let store = Store::new(EventBus::new());
        let ru = pack::of(Lang::Ru);
        assert_eq!(store.answer_of("kitchen", ru, "да"), None, "nothing was asked");
        store.ask("kitchen", Waiting::Install { id: "feed".into(), version: "0.4.1".into(), title: "Лента".into() });
        assert_eq!(store.answer_of("hall", ru, "да"), None, "another device was not asked");
        assert_eq!(store.answer_of("kitchen", ru, "да"), Some(StoreHeard::Answer { yes: true }));
        assert_eq!(store.answer_of("kitchen", ru, "нет"), Some(StoreHeard::Answer { yes: false }));
        assert_eq!(store.answer_of("kitchen", ru, "который час"), None, "not an answer: the phrase goes on");
        store.questions().insert(
            "kitchen".into(),
            (Waiting::Update { packages: Vec::new() }, Instant::now() - crate::talk::asked::ANSWER_FOR),
        );
        assert_eq!(store.answer_of("kitchen", ru, "да"), None, "too late");
    }
}
