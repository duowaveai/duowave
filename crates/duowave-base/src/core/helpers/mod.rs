//! The words of a home's helpers («Турбо») that the floors below the house share: a helper's kind and how far its
//! road goes, the regulator «Что может уходить из дома», an entry of the setting `helpers`, the contract of
//! `/home/v1` (contract.rs) and the reasons «Свой» gives way. The helpers themselves — the list, the link, the checks —
//! are the house's (src/helpers).

pub mod contract;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use self::contract::{Abilities, Ability};

/// Where a helper is, as a person says it: «дома», «мой сервер», «Облако DuoWave».
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HelperKind {
    Home,
    Own,
    Cloud,
}

impl HelperKind {
    /// How far its road goes by its kind alone: home — at home; one's own server and DuoWave's cloud — mine.
    pub fn reach(self) -> Reach {
        match self {
            HelperKind::Home => Reach::Home,
            HelperKind::Own | HelperKind::Cloud => Reach::Mine,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            HelperKind::Home => "home",
            HelperKind::Own => "own",
            HelperKind::Cloud => "cloud",
        }
    }

    pub fn parse(text: &str) -> Option<HelperKind> {
        [HelperKind::Home, HelperKind::Own, HelperKind::Cloud].into_iter().find(|kind| kind.as_str() == text)
    }
}

/// How far a road goes, for the regulator: home (this gateway, helpers at a home address), mine (one's own server, the
/// DuoWave cloud), foreign (clouds by keys).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Reach {
    Home,
    Mine,
    Foreign,
}

/// «Что может уходить из дома»: «Только дома» · «Дом и моё облако» · «Любые облака» (the default).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Away {
    Home,
    Mine,
    #[default]
    Any,
}

impl Away {
    /// Whether a road of this reach may be taken: «Только дома» — home only; «Дом и моё облако» — home and mine; «Любые
    /// облака» — all.
    pub fn lets(self, reach: Reach) -> bool {
        match self {
            Away::Home => reach == Reach::Home,
            Away::Mine => reach != Reach::Foreign,
            Away::Any => true,
        }
    }

    pub fn parse(text: &str) -> Option<Away> {
        serde_json::from_value(Value::String(text.to_string())).ok()
    }
}

/// A helper as the panel shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HelperState {
    /// Not looked at yet, or no key to look with.
    Unknown,
    Up,
    Down,
    /// Switched off.
    Off,
    /// This gateway itself.
    #[serde(rename = "self")]
    Itself,
    /// The same helper as an earlier entry, which is on the line.
    Twin,
}

/// One entry of the setting `helpers`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelperSetting {
    /// `[a-z0-9-]{1,40}`, never a route word (`RESERVED`); `home-computer` for the pair's door (legacy.rs).
    pub id: String,
    /// What people see: 1–60 characters.
    pub name: String,
    /// `http(s)://host[:port][/path]`, no trailing slash, no query.
    pub url: String,
    pub kind: HelperKind,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// The abilities not taken from it («Не брать у него…»); names this gateway does not know are passed by.
    #[serde(default, skip_serializing_if = "Vec::is_empty", deserialize_with = "abilities_lenient")]
    pub without: Vec<Ability>,
    /// The id its contract named at its last look (`contract.helper.id`): a gateway knows itself in a list by it, and
    /// the same helper reached twice by it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub helper_id: Option<String>,
}

fn yes() -> bool {
    true
}

/// A list of ability names; the ones this gateway does not know (a newer gateway's) are passed by.
pub fn abilities_lenient<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<Ability>, D::Error> {
    let names: Vec<Value> = Vec::deserialize(deserializer)?;
    Ok(sorted(names.iter().filter_map(Value::as_str).filter_map(Ability::parse)))
}

/// Abilities sorted in their declaration order, each once.
pub fn sorted(abilities: impl IntoIterator<Item = Ability>) -> Vec<Ability> {
    abilities.into_iter().collect::<Abilities>().iter().collect()
}

/// Words of `/api/v1/helpers/<word>` beside `{id}`: never an id. `self` is the built-in helper's row (stage 6).
pub const RESERVED: [&str; 5] = ["order", "away", "key", "clients", "self"];
/// An entry's id: `[a-z0-9-]{1,40}`, not a route word.
pub fn valid_id(id: &str) -> bool {
    (1..=40).contains(&id.len())
        && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !RESERVED.contains(&id)
}

/// The setting `helpers` as read: the items that do not read, whose id is no id, or whose id came before, are passed
/// by (a hand edit, a newer gateway's shape).
pub fn parse_helpers(value: &Value) -> Option<Vec<HelperSetting>> {
    let mut list: Vec<HelperSetting> = Vec::new();
    for item in value.as_array()? {
        let Ok(entry) = serde_json::from_value::<HelperSetting>(item.clone()) else { continue };
        if valid_id(&entry.id) && !list.iter().any(|known| known.id == entry.id) {
            list.push(entry);
        }
    }
    Some(list)
}

/// The longest name of a helper.
pub const NAME_MAX: usize = 60;
/// Why «Свой» gave way to «Вместе», or came back (static; their English is in services.json).
pub const AWAY: &str = "Турбо не на связи";
pub const BACK: &str = "Турбо снова на связи";
pub const NO_HEARING: &str = "нет Турбо со слухом";
pub const NO_VOICE: &str = "нет Турбо с голосом";
pub const GIVEN: &str = "Турбо снова даёт слух и голос";
pub const HEARING_NOT_READY: &str = "слух Турбо ещё не готов";
pub const VOICE_NOT_READY: &str = "голос Турбо ещё не готов";
pub const SHUT: &str = "регулятор не пускает Турбо";

/// The Turbo's voice failed lately: when, and why in its words.
static VOICE_FAILED: std::sync::Mutex<Option<(std::time::Instant, String)>> = std::sync::Mutex::new(None);

/// The Turbo's voice is not asked again for this long after it failed: the phrase goes to Alice's voice at once
/// instead of waiting for the same refusal every time.
pub const VOICE_REST: std::time::Duration = std::time::Duration::from_secs(2 * 60);

/// The station's answers tell how the Turbo's voice went: `Some(why)` — it failed and Alice spoke; `None` — it spoke.
pub fn note_turbo_voice(failed: Option<&str>) {
    let mut kept = VOICE_FAILED.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    *kept = failed.map(|why| (std::time::Instant::now(), why.chars().take(600).collect()));
}

/// The Turbo's voice failed within `within`: why.
pub fn turbo_voice_failed(within: std::time::Duration) -> Option<String> {
    let kept = VOICE_FAILED.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    kept.as_ref().filter(|(at, _)| at.elapsed() < within).map(|(_, why)| why.clone())
}
