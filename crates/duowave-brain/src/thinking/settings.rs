//! Thinking's settings (docs/plans/thinking.md, «Everything is a setting»): one home-wide JSON value, `thinking`, with
//! a schema number. Every field is optional and has its default here; an unknown field is skipped; a stored value of a
//! newer schema than this one reads as the defaults (its fields may mean something else). The code holds only the
//! defaults: every number, list and order of Thinking is one of these.
//!
//! A setting that may differ by what matters (`Varies`) is a value, or a map from where it holds to its value:
//! `"variant": "card"`, or `"variant": {"default": "all", "text": "card", "anthropic": "all"}`. The narrowest key wins:
//! the provider, then the kind of device (`station`, `computer`, `phone`, `box`), then `voice` or `text`, then
//! `default`.
//!
//! `GET`/`PUT /api/v1/thinking/settings` (routes.rs) read and change it; the panel's hidden developer options show it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::core::settings::{SettingDefinition, SettingScope};
use crate::door::Kind;
use crate::settings::SettingsService;

/// The schema this gateway reads: a field that changes meaning gives a new number.
pub const SCHEMA: u32 = 1;

/// The setting `thinking`. Not set — the defaults.
pub const THINKING: SettingDefinition<Settings> = SettingDefinition {
    key: "thinking",
    scope: SettingScope::Global,
    description: "Мышление",
    sensitive: false,
    parse: |value| serde_json::from_value(value.clone()).ok(),
};

/// Where a turn stands, for a setting that varies: the kind of device, voice or text, the provider of its model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Where<'a> {
    pub kind: Kind,
    pub voice: bool,
    pub provider: Option<&'a str>,
}

impl Where<'_> {
    pub fn of(kind: Kind, voice: bool) -> Where<'static> {
        Where { kind, voice, provider: None }
    }
}

/// The name of a kind of device, as the settings' keys and the turn's line write it.
pub fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Station => "station",
        Kind::Computer => "computer",
        Kind::Phone => "phone",
        Kind::Box => "box",
        Kind::Other => "other",
    }
}

/// A value, or a value by where it holds (see the module's words).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Varies<T> {
    One(T),
    By(BTreeMap<String, T>),
}

impl<T: Clone> Varies<T> {
    /// The value for this place: the narrowest key that is set, else `default`, else `fallback`.
    pub fn at(&self, place: Where<'_>, fallback: &T) -> T {
        match self {
            Varies::One(value) => value.clone(),
            Varies::By(map) => {
                let mode = if place.voice { "voice" } else { "text" };
                let keys = [place.provider, Some(kind_name(place.kind)), Some(mode), Some("default")];
                keys.into_iter().flatten().find_map(|key| map.get(key)).cloned().unwrap_or_else(|| fallback.clone())
            }
        }
    }
}

/// Which Thinking stands behind the door.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Implementation {
    /// The talk layer wrapped (talk/thinking.rs, `TalkThinking`): the stand-in of the audio stage 3.
    Talk,
    /// `OurThinking` (this module).
    Ours,
}

/// The card's variants (card.rs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CardVariant {
    /// Every tool, as before the card.
    All,
    /// The start card: the catalog, the place's usual tools, the phrase's tools, «спросить ещё».
    Card,
    /// The card, its groups picked by «Чутьё»'s model step as well.
    CardSense,
}

/// The rounds' policies (rounds.rs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoundsPolicy {
    /// Two rounds with tools, three with MCP or apps, 25 s; «Сейчас посмотрю.» after a second.
    Today,
    /// By the device's time, with `wait` (decision 1).
    ByTime,
}

/// How the opened tools reach the provider (§6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Delivery {
    Append,
    Deferred,
    Use,
}

/// For how long a word's choice of agent holds (decision 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WordScope {
    Talk,
    Home,
}

/// Whose the history of a place is (decision 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryView {
    /// One per place, each turn marked with who answered; another agent's turns come as notes.
    Shared,
    /// Each agent its own.
    Own,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Fast {
    /// The fast route's steps, in order: `answer`, `quick`, `door`, `works`, `assistant`, `reader`.
    pub steps: Vec<String>,
    /// The kinds of device a step runs for; a step not named runs for every kind (D6: `quick` for stations).
    pub devices: BTreeMap<String, Vec<String>>,
}

impl Default for Fast {
    fn default() -> Fast {
        Fast {
            steps: ["answer", "quick", "door", "works", "assistant", "reader"].map(String::from).to_vec(),
            devices: BTreeMap::from([("quick".to_string(), vec!["station".to_string()])]),
        }
    }
}

impl Fast {
    /// The step runs for this kind of device.
    pub fn runs(&self, step: &str, kind: Kind) -> bool {
        self.steps.iter().any(|name| name == step)
            && self.devices.get(step).is_none_or(|kinds| kinds.iter().any(|name| name == kind_name(kind)))
    }
}

/// The card's sizes (§4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Caps {
    /// The card's tools besides the works group, in tokens.
    pub tools: u32,
    /// The catalog of groups, in tokens.
    pub catalog: u32,
    /// «How to ask for more», in tokens.
    pub ask: u32,
    /// The place's usual tools.
    pub usual: usize,
    /// The phrase's tools.
    pub phrase: usize,
    /// Groups one `open` may open.
    pub open: usize,
}

impl Default for Caps {
    fn default() -> Caps {
        Caps { tools: 600, catalog: 300, ask: 60, usual: 3, phrase: 3, open: 3 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Card {
    pub variant: Varies<CardVariant>,
    pub caps: Caps,
    /// The place's usual tools are counted over this many days.
    pub usual_days: u32,
    /// How the journal finds similar phrases: `first_two_words`.
    pub similar: String,
    /// The memory's wake-up on the card, in tokens.
    pub wake_up_tokens: Varies<u32>,
    /// A model at home (Ollama, LM Studio) keeps every tool whatever the variant (D7), until its own numbers show it
    /// opens groups well.
    pub local_all: bool,
    /// A turn's later rounds cached up to their last message, by provider (§9): writing the cache costs more with
    /// some.
    pub cache_rounds: Varies<bool>,
}

impl Default for Card {
    fn default() -> Card {
        Card {
            variant: Varies::One(CardVariant::All),
            caps: Caps::default(),
            usual_days: 14,
            similar: "first_two_words".into(),
            wake_up_tokens: Varies::One(1000),
            local_all: true,
            cache_rounds: Varies::One(false),
        }
    }
}

/// Expected times until the journal has enough rounds to count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Guess {
    pub round_ms: u64,
    pub tool_ms: u64,
}

impl Default for Guess {
    fn default() -> Guess {
        Guess { round_ms: 1000, tool_ms: 1000 }
    }
}

/// Where the expected times come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Journal {
    pub days: u32,
    /// Rounds of a model before its median is trusted.
    pub rounds: usize,
}

impl Default for Journal {
    fn default() -> Journal {
        Journal { days: 14, rounds: 20 }
    }
}

/// The time a turn may take when the phrase gives no limit, and the part of it kept for words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Limit {
    pub ms: u64,
    pub words_ms: u64,
}

impl Default for Limit {
    fn default() -> Limit {
        Limit { ms: 25_000, words_ms: 6_000 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rounds {
    pub policy: Varies<RoundsPolicy>,
    /// Rounds with tools before the first word.
    pub ceiling: Varies<usize>,
    pub guess: Guess,
    pub journal: Journal,
    /// Only for a phrase that gives no `budget.first_word_ms`.
    pub first_word_ms: Varies<u64>,
    /// Only for a phrase that gives no limit of its own.
    pub limit: Limit,
    /// By time, a slow tool's filler words («Сейчас посмотрю.») are said too, before its `wait`: a device that
    /// ignores `wait` (the computer) would be silent otherwise — the filler is Thinking's `say` (the audio side, 27.11).
    pub filler: Varies<bool>,
}

impl Default for Rounds {
    fn default() -> Rounds {
        Rounds {
            policy: Varies::One(RoundsPolicy::Today),
            ceiling: Varies::By(BTreeMap::from([("voice".to_string(), 3), ("text".to_string(), 5)])),
            guess: Guess::default(),
            journal: Journal::default(),
            first_word_ms: Varies::One(1500),
            limit: Limit::default(),
            filler: Varies::One(true),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Wait {
    /// After the first word, a tool expected to run longer than this gets a `wait` first.
    pub tool_ms: u64,
}

impl Default for Wait {
    fn default() -> Wait {
        Wait { tool_ms: 1000 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Agents {
    /// How the agent of a turn is picked: `to`, `room`, `home`.
    pub order: Vec<String>,
    pub word_scope: WordScope,
    pub history: HistoryView,
}

impl Default for Agents {
    fn default() -> Agents {
        Agents {
            order: ["to", "room", "home"].map(String::from).to_vec(),
            word_scope: WordScope::Talk,
            history: HistoryView::Shared,
        }
    }
}

/// What came next is watched this long; the same words again within `repeated_ms` are a repeat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Next {
    pub watch_ms: u64,
    pub repeated_ms: u64,
}

impl Default for Next {
    fn default() -> Next {
        Next { watch_ms: 60_000, repeated_ms: 30_000 }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Line {
    pub next: Next,
}

/// Which of Thinking's kinds go through «Чутьё» (sense.rs), and whether a kind's model step runs, in shadow first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SenseKinds {
    /// The kinds asked of «Чутьё»: `answer`, `work`, `quick`, `groups`, `profile`.
    pub kinds: Vec<String>,
    /// The kinds whose model step's pick is used; the others' model steps run in shadow (once «Чутьё» has them).
    pub live: Vec<String>,
}

impl Default for SenseKinds {
    fn default() -> SenseKinds {
        SenseKinds {
            kinds: ["answer", "work", "quick", "groups", "profile"].map(String::from).to_vec(),
            live: Vec::new(),
        }
    }
}

/// One of the home's own groups (§5): its id, its line for the model, its tools, its words for picking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnGroup {
    pub id: String,
    #[serde(default)]
    pub line: String,
    #[serde(default)]
    pub tools: Vec<String>,
    /// Stems of words by language: `{"ru": ["календар"]}`.
    #[serde(default)]
    pub words: BTreeMap<String, Vec<String>>,
}

/// The setting `thinking`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub schema: u32,
    pub implementation: Varies<Implementation>,
    pub fast: Fast,
    pub card: Card,
    pub rounds: Rounds,
    pub wait: Wait,
    pub delivery: Varies<Delivery>,
    pub agents: Agents,
    pub line: Line,
    /// The parts whose variants run in shadow: `card`, `rounds`.
    pub shadow: Vec<String>,
    /// The home's own groups, added to the built-in ones.
    pub groups: Vec<OwnGroup>,
    pub sense: SenseKinds,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            schema: SCHEMA,
            // The station stays on its own chain until its checks by hand pass (as M0 did for the computer):
            // `{"station": "ours"}` puts it behind ours.
            implementation: Varies::By(BTreeMap::from([
                ("station".to_string(), Implementation::Talk),
                ("default".to_string(), Implementation::Ours),
            ])),
            fast: Fast::default(),
            card: Card::default(),
            rounds: Rounds::default(),
            wait: Wait::default(),
            delivery: Varies::One(Delivery::Append),
            agents: Agents::default(),
            line: Line::default(),
            shadow: vec!["card".into(), "rounds".into()],
            groups: Vec::new(),
            sense: SenseKinds::default(),
        }
    }
}

impl Settings {
    /// The home's settings as they stand now: the stored value when this schema can read it, else the defaults.
    pub fn of(settings: &SettingsService) -> Settings {
        match settings.get(&THINKING, None) {
            Some(stored) if stored.schema <= SCHEMA => stored,
            Some(stored) => {
                tracing::warn!(
                    target: "thinking",
                    "the setting «thinking» has schema {}, newer than {SCHEMA}: the defaults hold",
                    stored.schema
                );
                Settings::default()
            }
            None => Settings::default(),
        }
    }

    /// A short mark of these settings, so the numbers from before and after a change are not mixed: `s-3f9a`.
    pub fn mark(&self) -> String {
        let json = serde_json::to_string(self).unwrap_or_default();
        let digest = Sha256::digest(json.as_bytes());
        format!("s-{:02x}{:02x}", digest[0], digest[1])
    }

    pub fn implementation(&self, place: Where<'_>) -> Implementation {
        self.implementation.at(place, &Implementation::Ours)
    }

    /// Ours takes the station's phrases (the station's session sends them plain): `implementation` for a station's
    /// voice is `ours`.
    pub fn takes_station(&self) -> bool {
        self.implementation(Where::of(Kind::Station, true)) == Implementation::Ours
    }

    pub fn card_variant(&self, place: Where<'_>) -> CardVariant {
        self.card.variant.at(place, &CardVariant::All)
    }

    pub fn rounds_policy(&self, place: Where<'_>) -> RoundsPolicy {
        self.rounds.policy.at(place, &RoundsPolicy::Today)
    }

    pub fn ceiling(&self, place: Where<'_>) -> usize {
        self.rounds.ceiling.at(place, &if place.voice { 3 } else { 5 })
    }

    pub fn delivery(&self, place: Where<'_>) -> Delivery {
        self.delivery.at(place, &Delivery::Append)
    }

    pub fn first_word_ms(&self, place: Where<'_>) -> u64 {
        self.rounds.first_word_ms.at(place, &1500)
    }

    pub fn wake_up_tokens(&self, place: Where<'_>) -> u32 {
        self.card.wake_up_tokens.at(place, &1000)
    }

    /// A part's variants run in shadow.
    pub fn shadows(&self, part: &str) -> bool {
        self.shadow.iter().any(|name| name == part)
    }

    /// A kind of Thinking's goes through «Чутьё».
    pub fn senses(&self, kind: &str) -> bool {
        self.sense.kinds.iter().any(|name| name == kind)
    }

    /// A kind's model step is used, not only in shadow.
    pub fn sense_live(&self, kind: &str) -> bool {
        self.sense.live.iter().any(|name| name == kind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_empty_setting_is_the_defaults_and_a_field_set_holds_alone() {
        let empty: Settings = serde_json::from_value(json!({})).unwrap();
        assert_eq!(empty, Settings::default());
        let set: Settings =
            serde_json::from_value(json!({"card": {"caps": {"usual": 2}}, "unknown": true, "rounds": {"guess": {}}}))
                .unwrap();
        assert_eq!(set.card.caps.usual, 2);
        assert_eq!(set.card.caps.tools, 600, "the other caps keep their defaults");
        assert_eq!(set.rounds.guess, Guess::default());
    }

    #[test]
    fn the_narrowest_key_wins_provider_then_kind_then_voice_or_text_then_default() {
        let settings: Settings = serde_json::from_value(json!({
            "card": {"variant": {"default": "all", "text": "card", "station": "all", "anthropic": "card_sense"}},
        }))
        .unwrap();
        let text = Where::of(Kind::Computer, false);
        assert_eq!(settings.card_variant(text), CardVariant::Card);
        assert_eq!(settings.card_variant(Where::of(Kind::Computer, true)), CardVariant::All);
        assert_eq!(settings.card_variant(Where::of(Kind::Station, false)), CardVariant::All, "the kind before text");
        let claude = Where { provider: Some("anthropic"), ..text };
        assert_eq!(settings.card_variant(claude), CardVariant::CardSense);
        assert_eq!(Settings::default().ceiling(Where::of(Kind::Station, true)), 3);
        assert_eq!(Settings::default().ceiling(Where::of(Kind::Computer, false)), 5);
    }

    #[test]
    fn a_step_runs_where_its_devices_say_and_the_mark_follows_a_change() {
        let settings = Settings::default();
        assert!(settings.fast.runs("quick", Kind::Station));
        assert!(!settings.fast.runs("quick", Kind::Computer), "D6: the quick rules stay the stations'");
        assert!(settings.fast.runs("reader", Kind::Computer));
        let mut changed = settings.clone();
        changed.card.caps.usual = 2;
        assert_ne!(settings.mark(), changed.mark());
        assert_eq!(settings.mark(), Settings::default().mark());
        assert!(settings.mark().starts_with("s-") && settings.mark().len() == 6);
    }

    #[test]
    fn a_newer_schema_reads_as_the_defaults() {
        let store = SettingsService::new(
            std::sync::Arc::new(crate::storage::Db::memory()),
            crate::core::events::EventBus::new(),
        );
        let mut newer = Settings { schema: SCHEMA + 1, ..Settings::default() };
        newer.card.caps.usual = 1;
        store.set(&THINKING, &newer, None).unwrap();
        assert_eq!(Settings::of(&store), Settings::default());
        let mut ours = Settings::default();
        ours.card.caps.usual = 1;
        store.set(&THINKING, &ours, None).unwrap();
        assert_eq!(Settings::of(&store).card.caps.usual, 1);
    }
}
