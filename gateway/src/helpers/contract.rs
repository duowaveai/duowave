//! The helper's contract (docs/plans/home-and-helpers.md, block 1, «Договор помощника»): the list of abilities, the
//! contract's version and the answer «что я умею» — the same for every helper.
//!
//! A helper says what it gives in the field `contract` of `GET /home/v1/status` (home_server/serve.rs):
//!
//! ```json
//! {"version": 1, "helper": {"id": "8f0c…", "name": "Компьютер"},
//!  "abilities": [{"id": "hearing", "ready": true, "details": {"model": "v3_e2e_rnnt", "languages": ["ru"]}}, …]}
//! ```
//!
//! A status without the field — a computer of 0.11 and before — is version 0: its abilities are read from the old
//! fields (`asr`, `tts`, `llm`, `voice`) and from the routes its program's version has, with no probing. The routes
//! `/home/v1/*` do not change: the contract describes them. Reading never fails on a helper's status: an ability a
//! newer helper names and this gateway does not know yet is passed by. A JSON answer with neither `contract` nor the
//! `peerApi` of every computer's status is no helper's.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::core::identity::home::clean_name;

/// The contract's version this gateway speaks. 2 (docs/plans/home-and-devices.md): the home's character and its voice
/// (`/home/v1/character`), the home's Telegram (`/home/v1/messenger/bot`), an app's code installed over the link
/// (`/home/v1/plugins/install`) and every package listed with its kind and state (`/home/v1/plugins`). A helper of
/// version 1 still gives its abilities; what came with 2 waits for it: «Турбо другой версии — обновите оба».
pub const VERSION: u32 = 2;
/// The longest helper id a contract may name: a gateway's id is a UUID.
const ID_MAX: usize = 64;

/// The first program with the messenger's routes (`/home/v1/messenger/*`, release 0.3.0).
const MESSAGES_SINCE: (u64, u64, u64) = (0, 3, 0);
/// The first program with the history's routes (`/home/v1/history/*`, release 0.6.0).
const HISTORY_SINCE: (u64, u64, u64) = (0, 6, 0);
/// The first program whose `asr` is false also while its own engine loads its model (release 0.6.0); before it `asr`
/// said only whether a recognition server was there.
const ENGINE_SINCE: (u64, u64, u64) = (0, 6, 0);

/// What a helper gives. Version 1 has the first seven — what a computer gives already; the later blocks' abilities
/// have their names here already and come with a new contract version (`since`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ability {
    /// Recognition, `/home/v1/asr`: the old part `asr`.
    #[serde(alias = "asr")]
    Hearing,
    /// Our voice, `/home/v1/tts`: the old part `tts`.
    #[serde(alias = "tts")]
    Voice,
    /// A model of its own, `/home/v1/llm`: the old part `llm`.
    #[serde(alias = "llm")]
    Model,
    /// Its Telegram bot for «передай …», `/home/v1/messenger/*`.
    Messages,
    /// Calls of the apps with `prefer: home`, `/home/v1/plugins`.
    Apps,
    /// The apps' history kept there too, `/home/v1/history/*`.
    History,
    /// The packages' catalog for a station without internet of its own, `/home/v1/catalog`.
    Catalog,
    /// Long tasks: «начать → как дела → результат» (block 4).
    Tasks,
    /// «Сейф»: the home's sealed copy (block 2).
    Vault,
    /// Search by meaning (block 4).
    Vectors,
    /// Big files, their contents streamed (block 4).
    Files,
    /// The panel from outside the home (block 3).
    Panel,
}

impl Ability {
    /// The abilities of version 1, in the order a helper names them.
    pub const V1: [Ability; 7] = [
        Ability::Hearing,
        Ability::Voice,
        Ability::Model,
        Ability::Messages,
        Ability::Apps,
        Ability::History,
        Ability::Catalog,
    ];

    /// Every ability this gateway knows.
    pub const ALL: [Ability; 12] = [
        Ability::Hearing,
        Ability::Voice,
        Ability::Model,
        Ability::Messages,
        Ability::Apps,
        Ability::History,
        Ability::Catalog,
        Ability::Tasks,
        Ability::Vault,
        Ability::Vectors,
        Ability::Files,
        Ability::Panel,
    ];

    /// By its name or its old part's ("asr", "tts", "llm"); unknown → None.
    pub fn parse(name: &str) -> Option<Ability> {
        Ability::ALL.into_iter().find(|ability| ability.as_str() == name || ability.legacy() == Some(name))
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Ability::Hearing => "hearing",
            Ability::Voice => "voice",
            Ability::Model => "model",
            Ability::Messages => "messages",
            Ability::Apps => "apps",
            Ability::History => "history",
            Ability::Catalog => "catalog",
            Ability::Tasks => "tasks",
            Ability::Vault => "vault",
            Ability::Vectors => "vectors",
            Ability::Files => "files",
            Ability::Panel => "panel",
        }
    }

    /// The old part's name for `/api/v1/home-server*` and `HomeServerSetting.without`: "asr", "tts", "llm",
    /// "messages"; none for the rest.
    pub fn legacy(self) -> Option<&'static str> {
        match self {
            Ability::Hearing => Some("asr"),
            Ability::Voice => Some("tts"),
            Ability::Model => Some("llm"),
            Ability::Messages => Some("messages"),
            _ => None,
        }
    }

    /// The contract version that brought it: 1 for `V1`; 2 for the later blocks' until their block sets its own.
    pub fn since(self) -> u32 {
        if Ability::V1.contains(&self) {
            1
        } else {
            2
        }
    }

    /// Stays with its first helper rather than going down the list, since its data lives there: the history.
    pub fn sticky(self) -> bool {
        self == Ability::History
    }

    /// Its bit in a set of abilities (`Abilities`): one per ability, in the order they are declared.
    pub const fn bit(self) -> u16 {
        1 << (self as u16)
    }
}

/// A set of abilities as bits (`Ability::bit`): `Copy`, so a context that holds one stays `Copy` (the registry's, stage
/// 5 of the plan). Said as the list of its names, in the order the abilities are declared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Abilities(u16);

impl Abilities {
    pub const NONE: Abilities = Abilities(0);
    /// The seven of contract 1.
    pub const V1: Abilities = Abilities::of(&Ability::V1);

    pub const fn of(list: &[Ability]) -> Abilities {
        let mut bits = 0;
        let mut at = 0;
        while at < list.len() {
            bits |= list[at].bit();
            at += 1;
        }
        Abilities(bits)
    }

    pub const fn has(self, ability: Ability) -> bool {
        self.0 & ability.bit() != 0
    }

    pub const fn with(self, ability: Ability) -> Abilities {
        Abilities(self.0 | ability.bit())
    }

    pub const fn union(self, other: Abilities) -> Abilities {
        Abilities(self.0 | other.0)
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn iter(self) -> impl Iterator<Item = Ability> {
        Ability::ALL.into_iter().filter(move |ability| self.has(*ability))
    }
}

impl FromIterator<Ability> for Abilities {
    fn from_iter<I: IntoIterator<Item = Ability>>(abilities: I) -> Abilities {
        abilities.into_iter().fold(Abilities::NONE, Abilities::with)
    }
}

impl Serialize for Abilities {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.iter())
    }
}

/// Who answers as a helper: its gateway's own id and name. Empty when it does not say (version 0): unknown.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelperIdentity {
    pub id: String,
    pub name: String,
}

impl HelperIdentity {
    /// What a helper says of itself — its contract's `helper`, a pairing's (link.rs) —, as the travelling list keeps it
    /// and the panel shows it: its id only when it reads as a gateway's (1–64 letters, digits and hyphens; unknown
    /// otherwise), its name cleaned and at most 60 characters.
    pub fn read(helper: Option<&Value>) -> HelperIdentity {
        let text = |key: &str| helper.and_then(|helper| helper.get(key)).and_then(Value::as_str).unwrap_or("");
        let id = text("id");
        let gateway_id =
            (1..=ID_MAX).contains(&id.len()) && id.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
        HelperIdentity {
            id: if gateway_id { id.to_string() } else { String::new() },
            name: clean_name(text("name")).chars().take(super::NAME_MAX).collect(),
        }
    }
}

/// One ability as a helper declares it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AbilityState {
    pub id: Ability,
    /// It can give it now; declared and not ready — not yet (its model still loading, no bot set up).
    pub ready: bool,
    /// Per ability, what the helper knows for sure: hearing `{model?, languages?}`, voice `{voice?, languages?}`,
    /// model `{languages?}`; empty when nothing is known. Later: `room` for long tasks.
    pub details: Map<String, Value>,
}

impl AbilityState {
    /// Declared, with nothing known beyond whether it is ready.
    pub fn new(id: Ability, ready: bool) -> AbilityState {
        AbilityState { id, ready, details: Map::new() }
    }
}

/// A helper's answer «что я умею».
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Contract {
    /// 0: a status without a contract, read by the old scheme.
    pub version: u32,
    pub helper: HelperIdentity,
    /// The abilities it declares, each once; an ability left out is not given there.
    pub abilities: Vec<AbilityState>,
    /// The helper speaks the link (the plan's stage 7): its version, 1. Absent before.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<u32>,
}

impl Contract {
    /// The contract this gateway answers with: of its own version.
    pub fn new(helper: HelperIdentity, abilities: Vec<AbilityState>) -> Contract {
        Contract { version: VERSION, helper, abilities, link: None }
    }

    /// What it declares, as a set.
    pub fn declared(&self) -> Abilities {
        self.abilities.iter().map(|state| state.id).collect()
    }

    pub fn get(&self, ability: Ability) -> Option<&AbilityState> {
        self.abilities.iter().find(|state| state.id == ability)
    }

    pub fn declares(&self, ability: Ability) -> bool {
        self.get(ability).is_some()
    }

    pub fn ready(&self, ability: Ability) -> bool {
        self.get(ability).is_some_and(|state| state.ready)
    }

    /// A version-0 route that answered 404 (messages on a computer before 0.3.0): no longer declared.
    pub fn forget(&mut self, ability: Ability) {
        self.abilities.retain(|state| state.id != ability);
    }
}

/// A `/home/v1/status` answer as read.
#[derive(Debug, Clone, PartialEq)]
pub struct HelperStatus {
    pub contract: Contract,
    /// The helper's program version (`version` of the status), when it says one.
    pub program: Option<String>,
}

impl HelperStatus {
    /// None when it is no helper's status — nothing that speaks `/home/v1` answered: not a JSON object, or one with
    /// neither `contract` nor `peerApi`, which the status of every computer has had since the first one (0.1.1) — a
    /// page, a health route that answers every address. A `contract` that is an object is read as it says; without
    /// one a computer's status is version 0.
    pub fn read(status: &Value) -> Option<HelperStatus> {
        let fields = status.as_object()?;
        let program =
            fields.get("version").and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty()).map(str::to_string);
        let contract = match fields.get("contract").and_then(Value::as_object) {
            Some(contract) => read_contract(contract),
            None if fields.get("peerApi").is_some_and(Value::is_u64) => version_0(fields, program.as_deref()),
            None => return None,
        };
        Some(HelperStatus { contract, program })
    }
}

/// A contract as a helper wrote it: each ability on its own — an unknown id is passed by, a repeated one keeps the
/// first, a missing `ready` is not ready; a version above this gateway's is read by what this gateway knows. What it
/// says of itself is read as the travelling list keeps it (`HelperIdentity::read`).
fn read_contract(contract: &Map<String, Value>) -> Contract {
    // A contract that names no version is the first one; version 0 is the status without a contract, never this.
    let version = contract
        .get("version")
        .and_then(Value::as_u64)
        .map_or(1, |version| u32::try_from(version).unwrap_or(u32::MAX).max(1));
    let helper = HelperIdentity::read(contract.get("helper"));
    let mut abilities: Vec<AbilityState> = Vec::new();
    for item in contract.get("abilities").and_then(Value::as_array).into_iter().flatten() {
        let Some(id) = item.get("id").and_then(Value::as_str).and_then(Ability::parse) else { continue };
        if abilities.iter().any(|known| known.id == id) {
            continue;
        }
        abilities.push(AbilityState {
            id,
            ready: item.get("ready").and_then(Value::as_bool).unwrap_or(false),
            details: item.get("details").and_then(Value::as_object).cloned().unwrap_or_default(),
        });
    }
    // The link's version (stage 7), when the helper says one.
    let link = contract.get("link").and_then(Value::as_u64).and_then(|link| u32::try_from(link).ok());
    Contract { version, helper, abilities, link }
}

/// A status without a contract (a computer of 0.11 and before), by the old scheme, as a computer of now declares the
/// same parts: `tts` and `llm` have only ever said whether its voice and its model are there — true declares them
/// ready, false leaves them out; `asr` is hearing ready, and false leaves it out only before 0.6.0 — since then it is
/// also its own engine still loading its model: declared, not ready yet. The messenger and the history by the release
/// that brought their routes (no or an odd version — as one that has them); the bot's readiness is its contacts' read,
/// not the status's. Apps and the catalog are in every release that answers `/home/v1/status`.
fn version_0(fields: &Map<String, Value>, program: Option<&str>) -> Contract {
    let yes = |key: &str| fields.get(key).and_then(Value::as_bool).unwrap_or(false);
    let released = program.and_then(release);
    let has = |since: (u64, u64, u64)| released.is_none_or(|released| released >= since);
    let mut abilities = Vec::new();
    if yes("asr") || has(ENGINE_SINCE) {
        abilities.push(AbilityState::new(Ability::Hearing, yes("asr")));
    }
    if yes("tts") {
        let mut voice = AbilityState::new(Ability::Voice, true);
        if let Some(pair) = fields.get("voice").and_then(Value::as_str).filter(|pair| !pair.is_empty()) {
            voice.details.insert("voice".into(), Value::from(pair));
        }
        abilities.push(voice);
    }
    if yes("llm") {
        abilities.push(AbilityState::new(Ability::Model, true));
    }
    if has(MESSAGES_SINCE) {
        abilities.push(AbilityState::new(Ability::Messages, false));
    }
    abilities.push(AbilityState::new(Ability::Apps, true));
    if has(HISTORY_SINCE) {
        abilities.push(AbilityState::new(Ability::History, true));
    }
    abilities.push(AbilityState::new(Ability::Catalog, true));
    Contract { version: 0, helper: HelperIdentity::default(), abilities, link: None }
}

/// Whether a program's version `major.minor.patch` (its first three numbers; a suffix like `-dev` aside) is at least
/// `wanted`; None when it does not read as one. Every reading of a program's version goes through it.
pub fn version_at_least(version: &str, wanted: (u32, u32, u32)) -> Option<bool> {
    let wanted = (u64::from(wanted.0), u64::from(wanted.1), u64::from(wanted.2));
    release(version).map(|released| released >= wanted)
}

/// The first three numbers of `major.minor.patch` — `0.11.1`, `0.12.0-dev` → its release; fewer, or not numbers, →
/// None.
fn release(version: &str) -> Option<(u64, u64, u64)> {
    let core = version.trim().trim_start_matches('v').split(['-', '+']).next()?;
    let mut numbers = core.split('.').map(|part| part.parse::<u64>().ok());
    Some((numbers.next()??, numbers.next()??, numbers.next()??))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// What a computer of 0.11.1 answers at `/home/v1/status`, as it is (home_server/serve.rs before the contract).
    fn status_0_11_1() -> Value {
        json!({ "version": "0.11.1", "peerApi": 2, "asr": true, "tts": true, "voice": "silero/baya", "llm": false })
    }

    fn ids(contract: &Contract) -> Vec<Ability> {
        contract.abilities.iter().map(|state| state.id).collect()
    }

    #[test]
    fn abilities_keep_their_names_and_the_old_parts_read_as_the_new_ones() {
        assert_eq!(Ability::parse("asr"), Some(Ability::Hearing));
        assert_eq!(Ability::parse("tts"), Some(Ability::Voice));
        assert_eq!(Ability::parse("llm"), Some(Ability::Model));
        assert_eq!(Ability::parse("messages"), Some(Ability::Messages));
        assert_eq!((Ability::parse("gpu"), Ability::parse("Hearing"), Ability::parse("")), (None, None, None));
        for ability in Ability::ALL {
            assert_eq!(Ability::parse(ability.as_str()), Some(ability));
            assert_eq!(serde_json::to_value(ability).unwrap(), json!(ability.as_str()), "the JSON name is its name");
            assert_eq!(serde_json::from_value::<Ability>(json!(ability.as_str())).unwrap(), ability);
            if let Some(old) = ability.legacy() {
                assert_eq!(Ability::parse(old), Some(ability), "{old}");
                assert_eq!(serde_json::from_value::<Ability>(json!(old)).unwrap(), ability, "{old}");
            }
            assert_eq!(ability.since() == 1, Ability::V1.contains(&ability), "{ability:?}");
        }
        let old: Vec<&str> = Ability::ALL.iter().filter_map(|ability| ability.legacy()).collect();
        assert_eq!(old, ["asr", "tts", "llm", "messages"], "the old parts, and no others");
        let sticky: Vec<Ability> = Ability::ALL.into_iter().filter(|ability| ability.sticky()).collect();
        assert_eq!(sticky, [Ability::History]);
    }

    #[test]
    fn a_set_of_abilities_keeps_its_bits_and_says_its_names() {
        let set = Abilities::of(&[Ability::Voice, Ability::History]);
        assert!(set.has(Ability::Voice) && set.has(Ability::History) && !set.has(Ability::Model));
        assert_eq!(serde_json::to_value(set).unwrap(), json!(["voice", "history"]), "in the declaration order");
        assert_eq!(
            serde_json::to_value(Abilities::of(&[Ability::History, Ability::Voice])).unwrap(),
            json!(["voice", "history"])
        );
        assert_eq!(Abilities::V1.iter().collect::<Vec<_>>(), Ability::V1, "the seven of contract 1");
        assert!(Abilities::NONE.is_empty() && !Abilities::V1.has(Ability::Tasks));
        assert_eq!(
            Abilities::NONE.with(Ability::Model).union(set),
            Abilities::of(&[Ability::Voice, Ability::Model, Ability::History])
        );
        assert_eq!(
            [Ability::Hearing, Ability::Hearing].into_iter().collect::<Abilities>(),
            Abilities::of(&[Ability::Hearing])
        );
        let bits: Vec<u16> = Ability::ALL.iter().map(|ability| ability.bit()).collect();
        assert!(bits.iter().all(|bit| bit.count_ones() == 1), "one bit each");
        assert_eq!(bits.iter().fold(0, |all, bit| all | bit).count_ones(), 12, "and never the same one");
    }

    #[test]
    fn program_versions_compare_by_their_first_three_numbers() {
        assert_eq!(version_at_least("0.6.0", (0, 6, 0)), Some(true));
        assert_eq!(version_at_least("0.5.9", (0, 6, 0)), Some(false));
        assert_eq!(version_at_least("0.12.0-dev", (0, 6, 0)), Some(true), "a suffix aside");
        assert_eq!(version_at_least("1.0.0", (0, 99, 99)), Some(true));
        assert_eq!(version_at_least("0.10.1", (0, 9, 0)), Some(true), "as numbers, not as text");
        assert_eq!((version_at_least("x", (0, 6, 0)), version_at_least("0.6", (0, 6, 0))), (None, None));
    }

    #[test]
    fn an_answer_without_a_contract_reads_as_version_0_with_the_same_parts() {
        use Ability::{Apps, Catalog, Hearing, History, Messages, Model, Voice};
        let read = HelperStatus::read(&status_0_11_1()).unwrap();
        assert_eq!(read.program.as_deref(), Some("0.11.1"));
        let contract = &read.contract;
        assert_eq!(contract.version, 0);
        assert_eq!(contract.helper, HelperIdentity::default(), "a computer of 0.11 does not name itself");
        // Its parts as a computer of now declares them — no model of its own there — and the routes its release has.
        assert_eq!(ids(contract), [Hearing, Voice, Messages, Apps, History, Catalog]);
        assert!(contract.ready(Hearing) && contract.ready(Voice));
        let voice = contract.get(Voice).unwrap();
        assert_eq!(voice.details, Map::from_iter([("voice".to_string(), json!("silero/baya"))]));
        assert!(contract.get(Hearing).unwrap().details.is_empty());
        // Its bot is known by its contacts, not by the status.
        assert!(contract.declares(Messages) && !contract.ready(Messages));
        for ability in [Apps, History, Catalog] {
            assert!(contract.ready(ability), "{ability:?}");
        }

        // `tts` and `llm` have only ever said whether the part is there: false is none, left out as a computer of now
        // leaves it out. `asr` false since 0.6.0 is also its own engine with the model not in yet: declared, not ready.
        let quiet =
            json!({ "version": "0.11.1", "peerApi": 2, "asr": false, "tts": false, "voice": null, "llm": true });
        let read_quiet = HelperStatus::read(&quiet).unwrap().contract;
        assert_eq!(ids(&read_quiet), [Hearing, Model, Messages, Apps, History, Catalog]);
        assert!(!read_quiet.ready(Hearing) && read_quiet.ready(Model));
        assert!(
            read_quiet.get(Hearing).unwrap().details.is_empty() && read_quiet.get(Model).unwrap().details.is_empty()
        );
        // Before 0.6.0 `asr` said only whether a recognition server was there: false is none.
        let mut older = quiet.clone();
        older["version"] = json!("0.5.1");
        assert_eq!(ids(&HelperStatus::read(&older).unwrap().contract), [Model, Messages, Apps, Catalog]);
        older["asr"] = json!(true);
        assert!(HelperStatus::read(&older).unwrap().contract.ready(Hearing));

        // A route that answered 404 later is forgotten: its part is no longer declared.
        let mut forgotten = read.contract.clone();
        forgotten.forget(Messages);
        assert!(!forgotten.declares(Messages) && forgotten.declares(History));
    }

    #[test]
    fn an_older_computer_has_no_history_and_an_older_still_no_messages() {
        use Ability::{Apps, Catalog, Hearing, Messages, Model, Voice};
        // Every part there, a model too: what differs is the routes its release has.
        let every = || {
            let mut status = status_0_11_1();
            status["llm"] = json!(true);
            status
        };
        let at = |version: Value| {
            let mut status = every();
            status["version"] = version;
            ids(&HelperStatus::read(&status).unwrap().contract)
        };
        assert_eq!(at(json!("0.6.0")), Ability::V1);
        assert_eq!(at(json!("0.5.1")), [Hearing, Voice, Model, Messages, Apps, Catalog]);
        assert_eq!(at(json!("0.3.0")), [Hearing, Voice, Model, Messages, Apps, Catalog]);
        assert_eq!(at(json!("0.2.0")), [Hearing, Voice, Model, Apps, Catalog]);
        assert_eq!(at(json!("0.1.1")), [Hearing, Voice, Model, Apps, Catalog]);
        assert_eq!(
            at(json!("0.3.0-dev.2")),
            [Hearing, Voice, Model, Messages, Apps, Catalog],
            "by its release's numbers"
        );
        assert_eq!(at(json!("v0.12.4+build.7")), Ability::V1);
        // A program that says no version, or an odd one: as one that has every route.
        assert_eq!(at(Value::Null), Ability::V1);
        assert_eq!(at(json!("0.5")), Ability::V1);
        assert_eq!(at(json!("новая")), Ability::V1);
        assert_eq!(at(json!(5)), Ability::V1);
        let mut unnamed = every();
        unnamed.as_object_mut().unwrap().remove("version");
        let read = HelperStatus::read(&unnamed).unwrap();
        assert_eq!((ids(&read.contract), read.program), (Ability::V1.to_vec(), None));
    }

    #[test]
    fn a_version_1_contract_is_read_as_it_says_and_unknown_abilities_are_passed_by() {
        let status = json!({
            "version": "0.13.0", "peerApi": 2, "asr": false, "tts": false, "voice": null, "llm": true,
            "contract": {
                "version": 1,
                "link": 1,
                "helper": { "id": "8f0c2a", "name": "Компьютер Mac" },
                "abilities": [
                    { "id": "hearing", "ready": true, "details": { "model": "v3_e2e_rnnt", "languages": ["ru"] } },
                    { "id": "gpu", "ready": true },
                    { "id": "tasks", "ready": false, "details": { "room": { "disk": 40 } } },
                    { "id": "hearing", "ready": false },
                    { "id": "voice" },
                    { "ready": true },
                    "model"
                ]
            }
        });
        let read = HelperStatus::read(&status).unwrap();
        assert_eq!(read.program.as_deref(), Some("0.13.0"));
        let contract = read.contract;
        assert_eq!((contract.version, contract.link), (1, Some(1)), "it speaks the link");
        assert_eq!(contract.helper, HelperIdentity { id: "8f0c2a".into(), name: "Компьютер Mac".into() });
        assert_eq!(
            ids(&contract),
            [Ability::Hearing, Ability::Tasks, Ability::Voice],
            "gpu passed by, the first hearing"
        );
        assert!(contract.ready(Ability::Hearing), "the first of the two");
        assert_eq!(contract.get(Ability::Hearing).unwrap().details["languages"], json!(["ru"]));
        assert_eq!(contract.get(Ability::Tasks).unwrap().details["room"], json!({ "disk": 40 }));
        let voice = contract.get(Ability::Voice).unwrap();
        assert!(!voice.ready && voice.details.is_empty(), "no ready: not ready; no details: none known");
        // Its old fields are for stations of 0.11: the contract is what it gives.
        assert!(!contract.declares(Ability::Model) && !contract.declares(Ability::Apps));

        // A newer contract: read by what this gateway knows.
        let mut newer = status.clone();
        newer["contract"]["version"] = json!(2);
        let newer = HelperStatus::read(&newer).unwrap().contract;
        assert_eq!((newer.version, ids(&newer)), (2, vec![Ability::Hearing, Ability::Tasks, Ability::Voice]));

        // No version, no helper, no list: the first version, a helper unknown, nothing given.
        let bare = HelperStatus::read(&json!({ "contract": {} })).unwrap();
        let nothing = Contract { version: 1, helper: HelperIdentity::default(), abilities: Vec::new(), link: None };
        assert_eq!(bare.contract, nothing);
        assert_eq!(bare.program, None);
        let zero = HelperStatus::read(&json!({ "contract": { "version": 0, "abilities": [{ "id": "apps" }] } }));
        assert_eq!(zero.unwrap().contract.version, 1, "a contract is never the status without one");
    }

    /// What a helper says of itself goes into the travelling list and onto the panel (review): an id that does not read
    /// as a gateway's is unknown, a name is cleaned of control and turning characters and cut at 60.
    #[test]
    fn what_a_helper_says_of_itself_is_read_only_as_a_gateways_id_and_a_clean_short_name() {
        let told = |id: Value, name: Value| {
            HelperStatus::read(&json!({ "contract": { "helper": { "id": id, "name": name } } }))
                .unwrap()
                .contract
                .helper
        };
        let uuid = "0b9a6c1e-5d2f-4c8a-9e3b-7f1d2a4c6e80";
        assert_eq!(
            told(json!(uuid), json!("Компьютер Mac")),
            HelperIdentity { id: uuid.into(), name: "Компьютер Mac".into() }
        );
        for odd in [json!("a".repeat(65)), json!("pc 1"), json!("pc/../1"), json!("ид"), json!(""), json!(7)] {
            assert_eq!(told(odd.clone(), json!("x")).id, "", "{odd}: unknown");
        }
        assert_eq!(told(json!("a".repeat(64)), json!("x")).id.len(), 64, "a long one that reads as an id");
        let name = told(json!("pc"), json!(format!("Ком\u{202E}пьютер\n{}", "я".repeat(100)))).name;
        assert!(name.starts_with("Компьютер я") && name.chars().count() == 60, "{name}");
    }

    #[test]
    fn the_contract_reads_back_as_it_was_written() {
        let mut hearing = AbilityState::new(Ability::Hearing, true);
        hearing.details.insert("languages".into(), json!(["ru", "en"]));
        let written = Contract::new(
            HelperIdentity { id: "a1b2".into(), name: "Компьютер".into() },
            vec![hearing, AbilityState::new(Ability::Messages, false), AbilityState::new(Ability::Catalog, true)],
        );
        assert_eq!((written.version, written.link), (VERSION, None));
        let status = json!({ "version": "0.12.0", "contract": written });
        assert_eq!(status["contract"]["abilities"][1], json!({ "id": "messages", "ready": false, "details": {} }));
        assert!(status["contract"].get("link").is_none(), "no link: not said");
        assert_eq!(HelperStatus::read(&status).unwrap().contract, written);
        assert_eq!(written.declared(), Abilities::of(&[Ability::Hearing, Ability::Messages, Ability::Catalog]));
        // One that speaks the link says so, and is read so.
        let linked = Contract { link: Some(1), ..written };
        let status = json!({ "version": "0.13.0", "contract": linked });
        assert_eq!(status["contract"]["link"], json!(1));
        assert_eq!(HelperStatus::read(&status).unwrap().contract, linked);
    }

    #[test]
    fn what_is_not_a_json_object_is_no_helper() {
        for answer in [json!("<!doctype html><title>Router</title>"), json!([1, 2]), json!(true), Value::Null] {
            assert_eq!(HelperStatus::read(&answer), None, "{answer}");
        }
    }

    #[test]
    fn a_json_object_with_neither_a_contract_nor_peer_api_is_no_helper() {
        // A health route that answers every address, another program's status: JSON, but no computer's. The status of
        // every computer has had `peerApi` since the first one (0.1.1); from this gateway on it has `contract` too.
        let others = [
            json!({}),
            json!({ "status": "ok" }),
            json!({ "version": "1.4.2", "asr": true, "tts": true, "llm": true }),
            json!({ "version": "0.11.1", "peerApi": "2", "asr": true }),
            json!({ "version": "0.11.1", "asr": true, "contract": "soon" }),
        ];
        for answer in others {
            assert_eq!(HelperStatus::read(&answer), None, "{answer}");
        }
        // A computer's status with a `contract` that is no object: its old fields.
        let odd = json!({ "version": "0.11.1", "peerApi": 2, "asr": true, "contract": "soon" });
        let odd = HelperStatus::read(&odd).unwrap().contract;
        assert_eq!((odd.version, odd.ready(Ability::Hearing)), (0, true));
    }
}
