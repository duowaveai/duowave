//! The routes (docs/plans/own-server.md, «Простой режим», «Точная настройка»): for each row of the table — who goes to
//! the internet — directly or through which connection, and what to do when that connection does not answer.
//!
//! - The mode: `off` — everything directly, as without the app; `blocked` («Где не работает», the default) — through
//!   a server only what does not work from the home's country; `all` — everything DuoWave sends to the internet but
//!   the home itself, Alice too (her server down — she goes directly); `only` («Только выбранные») — only the rows
//!   with their own route, the rest directly. The panel sets it from the switch, the pick and «напрямую то, что
//!   работает из страны дома» (mod.rs, `mode_of`).
//! - A row's own route over the mode: `auto` (the default), `direct`, `server` (the location chosen for all — the
//!   preferred connection), or a connection's id.
//! - «Авто» in `blocked`: the service works from the home's country — directly; it does not — through a connection
//!   whose exit country it works from; there is no such connection — «Нужно подключение», with the countries that
//!   would do. Where it works is the catalog's data (`reach.json`) or what this home learned (a site that did not open
//!   directly). No data: the brain, hearing and voice go directly, as before — trying «directly first» there would show
//!   the key from a country the service refuses; apps and MCP tools go directly and learn.
//! - A connection down: by «Если VPN не отвечает» for all — `safe` (the default): the brain, hearing, voice, tools
//!   and apps wait (the call fails at once, nothing goes out directly), Alice goes directly; `wait` — everyone waits;
//!   `direct` — everyone goes directly. A row's own word, where an older panel set one, still wins.
//! - No connection was ever reported (the app is not installed, or the owner has not said «да» to «VPN → Сеть
//!   шлюза»): everything directly, as before the app.

use serde::{Deserialize, Serialize};

use super::connections::Connection;
use super::registry::{Goer, Section};

/// The simple mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Off,
    #[default]
    Blocked,
    All,
    Only,
}

impl Mode {
    pub fn parse(text: &str) -> Option<Mode> {
        match text {
            "off" => Some(Mode::Off),
            "blocked" => Some(Mode::Blocked),
            "all" => Some(Mode::All),
            "only" => Some(Mode::Only),
            _ => None,
        }
    }

    pub fn as_text(self) -> &'static str {
        match self {
            Mode::Off => "off",
            Mode::Blocked => "blocked",
            Mode::All => "all",
            Mode::Only => "only",
        }
    }
}

/// A row's own route.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Route {
    #[default]
    Auto,
    Direct,
    /// Through the location chosen for all: the preferred connection, else the first that answers.
    Server,
    Via(String),
}

impl Route {
    pub fn parse(text: &str) -> Option<Route> {
        match text {
            "auto" => Some(Route::Auto),
            "direct" => Some(Route::Direct),
            "server" => Some(Route::Server),
            "" => None,
            id if id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') => {
                Some(Route::Via(id.to_string()))
            }
            _ => None,
        }
    }

    pub fn as_text(&self) -> &str {
        match self {
            Route::Auto => "auto",
            Route::Direct => "direct",
            Route::Server => "server",
            Route::Via(id) => id,
        }
    }
}

/// What a row does when its connection does not answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OnDown {
    Wait,
    Direct,
}

impl OnDown {
    pub fn parse(text: &str) -> Option<OnDown> {
        match text {
            "wait" => Some(OnDown::Wait),
            "direct" => Some(OnDown::Direct),
            _ => None,
        }
    }

    /// The row's default: Alice goes directly (decision 8 — better partly than not at all), the rest waits.
    pub fn default_for(section: Section) -> OnDown {
        if section == Section::Alice {
            OnDown::Direct
        } else {
            OnDown::Wait
        }
    }
}

/// «Если VPN не отвечает» for all the rows (owner 16.11: one setting over all, not one per row).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OnDownAll {
    /// Keys never go out from the home's address: the brain, hearing, voice, tools and apps wait; Alice directly.
    #[default]
    Safe,
    Wait,
    Direct,
}

impl OnDownAll {
    pub fn parse(text: &str) -> Option<OnDownAll> {
        match text {
            "safe" => Some(OnDownAll::Safe),
            "wait" => Some(OnDownAll::Wait),
            "direct" => Some(OnDownAll::Direct),
            _ => None,
        }
    }

    pub fn as_text(self) -> &'static str {
        match self {
            OnDownAll::Safe => "safe",
            OnDownAll::Wait => "wait",
            OnDownAll::Direct => "direct",
        }
    }

    /// What a row of this section does when its server does not answer.
    pub fn for_section(self, section: Section) -> OnDown {
        match self {
            OnDownAll::Safe => OnDown::default_for(section),
            OnDownAll::Wait => OnDown::Wait,
            OnDownAll::Direct => OnDown::Direct,
        }
    }
}

/// Where a service can be reached from, as the catalog says (`reach.json`, a feed entry's `reach`): the countries it
/// refuses, or the only ones it serves. ISO codes in upper case.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reach {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub only: Vec<String>,
}

impl Reach {
    /// Whether the service works from a country.
    pub fn works_from(&self, country: &str) -> bool {
        let country = country.to_ascii_uppercase();
        if !self.only.is_empty() {
            return self.only.iter().any(|one| one.eq_ignore_ascii_case(&country));
        }
        !self.blocked.iter().any(|one| one.eq_ignore_ascii_case(&country))
    }

    pub fn is_empty(&self) -> bool {
        self.blocked.is_empty() && self.only.is_empty()
    }
}

/// What this home learned of a row by trying: a site that did not open directly from here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Learned {
    /// Directly from here it did not open: a timeout, a reset, 403 or 451.
    pub blocked: bool,
    /// What happened, short: `timeout`, `reset`, `403`, `451`.
    #[serde(default)]
    pub why: String,
    pub at: String,
}

/// Why a row goes the way it goes: the panel says it under the route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Why {
    /// No connection was ever reported: as before the app.
    NoConnections,
    /// The simple mode «Выключено».
    Off,
    /// The simple mode «Всё через сервер».
    All,
    /// «Только выбранные», and this row is not one of them.
    NotChosen,
    /// The row's own route.
    Own,
    /// The home's country is not known (not asked, or «Не скажу»).
    NoCountry,
    /// Works from the home's country.
    HomeWorks,
    /// The catalog: does not work from the home's country.
    Catalog { country: String },
    /// This home tried: directly it did not open.
    Learned { why: String },
    /// No data about the service: directly, as before.
    NoData,
}

/// The decision for a row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Decision {
    Direct,
    /// Through this connection, which answers.
    Via {
        connection: String,
    },
    /// Through this connection, which does not answer: the call fails, nothing goes out directly.
    Down {
        connection: String,
    },
    /// No connection fits: the countries that would. Keyed rows wait (nothing goes out), the rest go directly.
    Need {
        countries: Vec<String>,
        wait: bool,
    },
}

impl Decision {
    /// The connection this decision goes through, if any.
    pub fn connection(&self) -> Option<&str> {
        match self {
            Decision::Via { connection } | Decision::Down { connection } => Some(connection),
            _ => None,
        }
    }

    /// Whether a call by it fails at once rather than go out.
    pub fn refuses(&self) -> bool {
        matches!(self, Decision::Down { .. } | Decision::Need { wait: true, .. })
    }
}

/// Everything a decision reads, but the row itself.
pub struct Context<'a> {
    pub mode: Mode,
    /// The home's country; None — not asked or «Не скажу».
    pub home: Option<&'a str>,
    pub connections: &'a [Connection],
    /// The connection «Всё через сервер» and «Авто» go through first: the panel's choice, else the first that answers.
    pub preferred: Option<&'a str>,
    /// Whether any connection was ever reported: none — everything directly.
    pub any_reported: bool,
    /// «Если VPN не отвечает» for the rows without their own word.
    pub on_down: OnDownAll,
}

/// A row's own settings (app_settings): its route, what it does when the server is down, what it learned.
#[derive(Debug, Clone, Default)]
pub struct RowRule {
    pub route: Route,
    pub on_down: Option<OnDown>,
    pub learned: Option<Learned>,
}

/// The countries the catalog's `reach` lets a service work from, of those the connections exit to — for «Нужно
/// подключение», a hint of which to add: all of `only`, or a few usual exits that `blocked` does not name.
fn countries_for(reach: &Reach) -> Vec<String> {
    if !reach.only.is_empty() {
        return reach.only.clone();
    }
    ["NL", "DE", "FI", "US"].iter().filter(|code| reach.works_from(code)).map(|code| code.to_string()).collect()
}

/// The decision for one row.
pub fn decide(goer: &Goer, reach: Option<&Reach>, rule: &RowRule, context: &Context) -> (Decision, Why) {
    let on_down = rule.on_down.unwrap_or_else(|| context.on_down.for_section(goer.section));
    let through = |id: &str| -> Decision {
        match context.connections.iter().find(|one| one.id == id) {
            Some(connection) if connection.up() => Decision::Via { connection: id.to_string() },
            Some(_) | None if on_down == OnDown::Direct => Decision::Direct,
            _ => Decision::Down { connection: id.to_string() },
        }
    };
    // Whoever sets the row's own route, gets it — but «Выключено» switches everything off, own rules too.
    if context.mode == Mode::Off {
        return (Decision::Direct, Why::Off);
    }
    if !context.any_reported {
        return (Decision::Direct, Why::NoConnections);
    }
    let preferred = || -> Option<&str> {
        context
            .preferred
            .filter(|id| context.connections.iter().any(|one| one.id == *id))
            .or_else(|| context.connections.iter().find(|one| one.up()).map(|one| one.id.as_str()))
            .or_else(|| context.connections.first().map(|one| one.id.as_str()))
    };
    match &rule.route {
        Route::Direct => return (Decision::Direct, Why::Own),
        Route::Via(id) => return (through(id), Why::Own),
        Route::Server => {
            return match preferred() {
                Some(id) => (through(id), Why::Own),
                None => (Decision::Direct, Why::Own),
            };
        }
        Route::Auto => {}
    }
    if context.mode == Mode::Only {
        return (Decision::Direct, Why::NotChosen);
    }
    if context.mode == Mode::All {
        return match preferred() {
            Some(id) => (through(id), Why::All),
            None => (Decision::Direct, Why::All),
        };
    }
    // «Где не работает».
    let Some(home) = context.home else { return (Decision::Direct, Why::NoCountry) };
    let learned = rule.learned.as_ref().filter(|learned| learned.blocked);
    let reach = reach.filter(|reach| !reach.is_empty());
    let why = match (reach, learned) {
        (Some(reach), _) if !reach.works_from(home) => Why::Catalog { country: home.to_string() },
        (Some(_), None) => return (Decision::Direct, Why::HomeWorks),
        (_, Some(learned)) => Why::Learned { why: learned.why.clone() },
        (None, None) => return (Decision::Direct, Why::NoData),
    };
    // A connection whose exit country the service works from: the preferred one first, then one that answers.
    let fits = |connection: &Connection| match (reach, connection.country.as_deref()) {
        (Some(reach), Some(country)) => reach.works_from(country),
        // The catalog has no data (learned by trying) or the exit country is not known yet: any connection fits.
        (None, _) => true,
        (Some(_), None) => false,
    };
    let fitting: Vec<&Connection> = context.connections.iter().filter(|one| fits(one)).collect();
    let chosen = context
        .preferred
        .and_then(|id| fitting.iter().find(|one| one.id == id))
        .or_else(|| fitting.iter().find(|one| one.up()))
        .or_else(|| fitting.first());
    match chosen {
        Some(connection) => (through(&connection.id), why),
        None => {
            let countries = reach.map(countries_for).unwrap_or_default();
            // A row with a key never goes out from a country the service refuses; the rest go directly and fail
            // there by themselves.
            (Decision::Need { countries, wait: goer.keyed }, why)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn goer(id: &str, section: Section, keyed: bool) -> Goer {
        Goer {
            id: id.into(),
            name: id.into(),
            section,
            hosts: vec![format!("{id}.example")],
            keyed,
            via_entry: false,
            reach_id: None,
        }
    }

    fn connection(id: &str, country: &str, up: bool) -> Connection {
        Connection::test(id, country, up)
    }

    /// The table of the plan's examples: a home in Russia, in Germany, in China; ChatGPT refuses Russia and China,
    /// Alice works only from Russia (and its neighbours), a weather app works everywhere.
    #[test]
    fn auto_by_the_homes_country() {
        let chatgpt = goer("provider:openai", Section::Brain, true);
        let alice = goer("system:alice", Section::Alice, false);
        let weather = goer("weather", Section::Apps, false);
        let openai = Reach { blocked: vec!["RU".into(), "CN".into()], only: vec![] };
        let yandex = Reach { blocked: vec![], only: vec!["RU".into(), "BY".into()] };
        let connections = vec![connection("nl", "NL", true), connection("ru", "RU", true)];
        let rule = RowRule::default();
        let at = |home: &'static str| Context {
            mode: Mode::Blocked,
            home: Some(home),
            connections: &connections,
            preferred: None,
            any_reported: true,
            on_down: OnDownAll::Safe,
        };
        let via = |id: &str| Decision::Via { connection: id.into() };

        // Russia: the model through the Netherlands, Alice and the weather directly.
        assert_eq!(decide(&chatgpt, Some(&openai), &rule, &at("RU")).0, via("nl"));
        assert_eq!(decide(&alice, Some(&yandex), &rule, &at("RU")), (Decision::Direct, Why::HomeWorks));
        assert_eq!(decide(&weather, None, &rule, &at("RU")), (Decision::Direct, Why::NoData));
        // Germany: the model directly, Alice through Russia.
        assert_eq!(decide(&chatgpt, Some(&openai), &rule, &at("DE")).0, Decision::Direct);
        assert_eq!(decide(&alice, Some(&yandex), &rule, &at("DE")), (via("ru"), Why::Catalog { country: "DE".into() }));
        // China: the model through the Netherlands, Alice through Russia — the same code.
        assert_eq!(decide(&chatgpt, Some(&openai), &rule, &at("CN")).0, via("nl"));
        assert_eq!(decide(&alice, Some(&yandex), &rule, &at("CN")).0, via("ru"));
    }

    #[test]
    fn a_connection_down_waits_for_keys_and_lets_alice_go_directly() {
        let chatgpt = goer("provider:openai", Section::Brain, true);
        let alice = goer("system:alice", Section::Alice, false);
        let connections = vec![connection("nl", "NL", false)];
        let context = Context {
            mode: Mode::All,
            home: Some("RU"),
            connections: &connections,
            preferred: None,
            any_reported: true,
            on_down: OnDownAll::Safe,
        };
        let rule = RowRule::default();
        assert_eq!(decide(&chatgpt, None, &rule, &context).0, Decision::Down { connection: "nl".into() });
        assert!(decide(&chatgpt, None, &rule, &context).0.refuses());
        assert_eq!(decide(&alice, None, &rule, &context).0, Decision::Direct, "Alice: better partly than not");
        let allowed = RowRule { on_down: Some(OnDown::Direct), ..RowRule::default() };
        assert_eq!(decide(&chatgpt, None, &allowed, &context).0, Decision::Direct, "the row's own choice");
        let alice_waits = RowRule { on_down: Some(OnDown::Wait), ..RowRule::default() };
        assert_eq!(decide(&alice, None, &alice_waits, &context).0, Decision::Down { connection: "nl".into() });
    }

    #[test]
    fn no_fitting_connection_needs_one_and_a_key_never_goes_out() {
        let chatgpt = goer("provider:openai", Section::Brain, true);
        let maps = goer("maps", Section::Apps, false);
        let openai = Reach { blocked: vec!["RU".into()], only: vec![] };
        let connections = vec![connection("ru", "RU", true)];
        let context = Context {
            mode: Mode::Blocked,
            home: Some("RU"),
            connections: &connections,
            preferred: None,
            any_reported: true,
            on_down: OnDownAll::Safe,
        };
        let (decision, _) = decide(&chatgpt, Some(&openai), &RowRule::default(), &context);
        assert!(matches!(&decision, Decision::Need { countries, wait: true } if countries.contains(&"NL".to_string())));
        assert!(decision.refuses());
        let (decision, _) = decide(&maps, Some(&openai), &RowRule::default(), &context);
        assert!(!decision.refuses(), "an app without a key goes directly and fails there by itself");
    }

    #[test]
    fn what_a_home_learned_sends_an_app_through_a_server() {
        let maps = goer("maps", Section::Apps, false);
        let connections = vec![connection("nl", "NL", true)];
        let context = Context {
            mode: Mode::Blocked,
            home: Some("RU"),
            connections: &connections,
            preferred: None,
            any_reported: true,
            on_down: OnDownAll::Safe,
        };
        let learned = RowRule {
            learned: Some(Learned { blocked: true, why: "451".into(), at: "now".into() }),
            ..RowRule::default()
        };
        assert_eq!(
            decide(&maps, None, &learned, &context),
            (Decision::Via { connection: "nl".into() }, Why::Learned { why: "451".into() })
        );
    }

    #[test]
    fn off_and_no_connections_are_as_before_the_app() {
        let chatgpt = goer("provider:openai", Section::Brain, true);
        let own = RowRule { route: Route::Via("nl".into()), ..RowRule::default() };
        let connections = vec![connection("nl", "NL", true)];
        let off = Context {
            mode: Mode::Off,
            home: Some("RU"),
            connections: &connections,
            preferred: None,
            any_reported: true,
            on_down: OnDownAll::Safe,
        };
        assert_eq!(decide(&chatgpt, None, &own, &off), (Decision::Direct, Why::Off));
        let never = Context {
            mode: Mode::All,
            home: Some("RU"),
            connections: &[],
            preferred: None,
            any_reported: false,
            on_down: OnDownAll::Safe,
        };
        assert_eq!(decide(&chatgpt, None, &own, &never), (Decision::Direct, Why::NoConnections));
    }

    #[test]
    fn a_rows_own_route_and_the_preferred_connection() {
        let chatgpt = goer("provider:openai", Section::Brain, true);
        let connections = vec![connection("nl", "NL", true), connection("de", "DE", true)];
        let context = Context {
            mode: Mode::All,
            home: Some("RU"),
            connections: &connections,
            preferred: Some("de"),
            any_reported: true,
            on_down: OnDownAll::Safe,
        };
        assert_eq!(decide(&chatgpt, None, &RowRule::default(), &context).0, Decision::Via { connection: "de".into() });
        let direct = RowRule { route: Route::Direct, ..RowRule::default() };
        assert_eq!(decide(&chatgpt, None, &direct, &context), (Decision::Direct, Why::Own));
        let gone = RowRule { route: Route::Via("old".into()), ..RowRule::default() };
        assert_eq!(decide(&chatgpt, None, &gone, &context).0, Decision::Down { connection: "old".into() });
    }

    /// «Только выбранные»: the rows ticked go through the location chosen for all (or their own connection), the rest
    /// directly; «Все, кроме выбранных» is `all` with the ticked rows `direct`.
    #[test]
    fn only_the_chosen_rows_go_through_the_server() {
        let chatgpt = goer("provider:openai", Section::Brain, true);
        let alice = goer("system:alice", Section::Alice, false);
        let weather = goer("weather", Section::Apps, false);
        let openai = Reach { blocked: vec!["RU".into()], only: vec![] };
        let connections = vec![connection("nl", "NL", true), connection("ru", "RU", true)];
        let context = Context {
            mode: Mode::Only,
            home: Some("RU"),
            connections: &connections,
            preferred: Some("nl"),
            any_reported: true,
            on_down: OnDownAll::Safe,
        };
        let chosen = RowRule { route: Route::Server, ..RowRule::default() };
        assert_eq!(
            decide(&chatgpt, Some(&openai), &chosen, &context),
            (Decision::Via { connection: "nl".into() }, Why::Own)
        );
        assert_eq!(decide(&weather, None, &RowRule::default(), &context), (Decision::Direct, Why::NotChosen));
        // Not chosen: directly even where the catalog says it does not work from home — the person said so.
        assert_eq!(decide(&chatgpt, Some(&openai), &RowRule::default(), &context).0, Decision::Direct);
        let dacha = RowRule { route: Route::Via("ru".into()), ..RowRule::default() };
        assert_eq!(decide(&alice, None, &dacha, &context).0, Decision::Via { connection: "ru".into() });
        // The location chosen for all is gone: the first that answers.
        let gone = Context { preferred: Some("old"), ..context };
        assert_eq!(decide(&chatgpt, None, &chosen, &gone).0, Decision::Via { connection: "nl".into() });
    }

    /// «Если VPN не отвечает» for all: `safe` — keys wait, Alice directly; `wait`; `direct`; a row's own word wins.
    #[test]
    fn if_the_vpn_does_not_answer_for_all() {
        let chatgpt = goer("provider:openai", Section::Brain, true);
        let alice = goer("system:alice", Section::Alice, false);
        let connections = vec![connection("nl", "NL", false)];
        let at = |on_down: OnDownAll| Context {
            mode: Mode::All,
            home: Some("RU"),
            connections: &connections,
            preferred: None,
            any_reported: true,
            on_down,
        };
        let down = Decision::Down { connection: "nl".into() };
        let rule = RowRule::default();
        assert_eq!(decide(&chatgpt, None, &rule, &at(OnDownAll::Safe)).0, down);
        assert_eq!(decide(&alice, None, &rule, &at(OnDownAll::Safe)).0, Decision::Direct);
        assert_eq!(decide(&alice, None, &rule, &at(OnDownAll::Wait)).0, down);
        assert_eq!(decide(&chatgpt, None, &rule, &at(OnDownAll::Direct)).0, Decision::Direct);
        let own = RowRule { on_down: Some(OnDown::Wait), ..RowRule::default() };
        assert_eq!(decide(&chatgpt, None, &own, &at(OnDownAll::Direct)).0, down, "a row's own word wins");
    }

    #[test]
    fn reach_only_and_blocked() {
        let only = Reach { blocked: vec![], only: vec!["RU".into()] };
        assert!(only.works_from("ru") && !only.works_from("DE"));
        let blocked = Reach { blocked: vec!["CN".into()], only: vec![] };
        assert!(blocked.works_from("DE") && !blocked.works_from("CN"));
        assert!(Reach::default().works_from("XX"));
        assert_eq!(Route::parse("c1a2b3"), Some(Route::Via("c1a2b3".into())));
        assert_eq!(Route::parse("../x"), None);
        assert_eq!(Route::parse("server"), Some(Route::Server));
        assert_eq!(Mode::parse("only"), Some(Mode::Only));
    }
}
