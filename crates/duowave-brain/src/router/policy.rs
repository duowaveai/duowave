//! The router's policy (docs/plans/router-and-works.md, block A, 4): in which order to take the models the frames let
//! through for an ask. Ours is the table — rules as data, the first that fits is taken (`TablePolicy`); someone
//! else's policy takes its place behind the same trait in block Б.
//!
//! The table is the setting `router_table`, with a number of its schema; the default is built in. A rule names a seat,
//! when it holds (the step of the budget, the agent, whether tools are needed) and the model: `seat` — what stands on
//! the seat; a name — that model (for an agent, its own name of it: Claude Code's `haiku`); `{provider, model}` — a
//! model of «Мозг». A rule whose model is not offered is passed over. No rule fits — the seat's model.
//!
//! The default holds the conversation, the workshop, the memory and the apps where they are: it has rules only for the
//! seat «Исполнитель» — at the step `cheaper` Claude Code goes on `haiku`; any other agent stays on its model unless a
//! rule of the table names a cheaper one for it.

use serde::{Deserialize, Serialize};

use super::budgets::Act;
use super::pool::Price;
use super::{Place, Reason, Seat};

/// The schema the table is written in.
pub const SCHEMA: u32 = 1;
/// A table has at most this many rules.
const RULES_MAX: usize = 100;

/// An ask for a model: the seat, why the caller would take the seat's model (the conversation's rule by the phrase),
/// the agent of a work in a folder, the work, whether tools are needed, the step of the budget reached. No text of the
/// phrase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    pub seat: Seat,
    pub reason: Reason,
    pub agent: Option<String>,
    pub work: Option<String>,
    pub tools: bool,
    pub step: Option<Act>,
}

impl Ask {
    pub fn of(seat: Seat, reason: Reason) -> Ask {
        Ask { seat, reason, agent: None, work: None, tools: false, step: None }
    }
}

/// A model the frames may let through, with its passport: who and which, where it is, its price if known; whether it
/// is what stands on the seat, and whether «Мозг» already held it to the regulator «Что может уходить из дома» (a
/// seat's model of «Мозг» — it is answered by words or another model when the regulator keeps it in).
#[derive(Debug, Clone, PartialEq)]
pub struct Offer {
    pub provider: String,
    pub model: String,
    pub place: Place,
    pub price: Option<Price>,
    pub seat: bool,
    pub framed: bool,
}

/// A model in a rule: what stands on the seat, a name, or a model of «Мозг» by its provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ModelRef {
    Pick { provider: String, model: String },
    Named(String),
}

impl ModelRef {
    /// What stands on the seat.
    pub fn is_seat(&self) -> bool {
        matches!(self, ModelRef::Named(name) if name == "seat")
    }

    /// Whether an offer is this model.
    pub fn names(&self, offer: &Offer) -> bool {
        match self {
            ModelRef::Named(name) if name == "seat" => offer.seat,
            ModelRef::Named(name) => offer.model.eq_ignore_ascii_case(name.trim()),
            ModelRef::Pick { provider, model } => offer.provider == *provider && offer.model == *model,
        }
    }
}

/// When a rule holds; what is not said holds always.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct When {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<Act>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<bool>,
}

/// A rule of the table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub id: String,
    pub seat: Seat,
    #[serde(default, skip_serializing_if = "When::is_always")]
    pub when: When,
    pub model: ModelRef,
}

impl When {
    fn is_always(&self) -> bool {
        *self == When::default()
    }
}

impl Rule {
    /// Whether it holds for the ask.
    pub fn fits(&self, ask: &Ask) -> bool {
        let when = &self.when;
        self.seat == ask.seat
            && when.budget.is_none_or(|step| ask.step == Some(step))
            && when.agent.as_deref().is_none_or(|agent| ask.agent.as_deref() == Some(agent))
            && when.tools.is_none_or(|tools| tools == ask.tools)
    }

    /// Why its model, for the deal: the budget's step when it answers one, else the rule itself.
    pub fn reason(&self) -> Reason {
        match self.when.budget {
            Some(step) => Reason::Budget(step),
            None => Reason::Table(self.id.clone()),
        }
    }
}

/// The table: its schema and its rules, in order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Table {
    pub schema: u32,
    #[serde(default)]
    pub rules: Vec<Rule>,
}

impl Default for Table {
    /// The default, built in: Claude Code goes cheaper at the budget's step; everything else on its seat.
    fn default() -> Self {
        Table {
            schema: SCHEMA,
            rules: vec![
                Rule {
                    id: "cheaper".into(),
                    seat: Seat::Worker,
                    when: When {
                        budget: Some(Act::Cheaper),
                        agent: Some(super::seats::CLAUDE_CODE.into()),
                        tools: None,
                    },
                    model: ModelRef::Named("haiku".into()),
                },
                Rule {
                    id: "worker".into(),
                    seat: Seat::Worker,
                    when: When::default(),
                    model: ModelRef::Named("seat".into()),
                },
            ],
        }
    }
}

impl Table {
    /// Whether it may be taken: our schema, not too many rules, each with an id of its own. Why not, in words.
    pub fn check(&self) -> Result<(), String> {
        if self.schema != SCHEMA {
            return Err(crate::tr!("Схема таблицы — {schema}.", schema = SCHEMA));
        }
        if self.rules.len() > RULES_MAX {
            return Err(crate::tr!("Правил в таблице не больше {max}.", max = RULES_MAX));
        }
        let mut ids = std::collections::HashSet::new();
        for rule in &self.rules {
            let id = rule.id.trim();
            if id.is_empty() || id.len() > 60 || !ids.insert(id) {
                return Err(crate::tr!("У каждого правила своё имя: «{id}».", id = rule.id));
            }
            match &rule.model {
                ModelRef::Named(name) if name.trim().is_empty() => {
                    return Err(crate::tr!("Правило «{id}» не называет модель.", id = rule.id))
                }
                ModelRef::Pick { provider, model } if provider.trim().is_empty() || model.trim().is_empty() => {
                    return Err(crate::tr!("Правило «{id}» не называет модель.", id = rule.id))
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// The models its fitting rules name that are not the seat's, for the caller to offer: a seat's rules only.
    pub fn named(&self, ask: &Ask) -> Vec<ModelRef> {
        let mut named: Vec<ModelRef> = Vec::new();
        for rule in self.rules.iter().filter(|rule| rule.fits(ask) && !rule.model.is_seat()) {
            if !named.contains(&rule.model) {
                named.push(rule.model.clone());
            }
        }
        named
    }
}

/// An offer in the policy's order, and why it: none — the ask's own reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ranked {
    pub offer: usize,
    pub reason: Option<Reason>,
}

/// The order to take the offers in for an ask.
pub trait Policy: Send + Sync {
    fn rank(&self, ask: &Ask, offers: &[Offer]) -> Vec<Ranked>;
}

/// Ours: the table's fitting rules in order, each the offer it names; then the seat's model, by the ask's reason.
pub struct TablePolicy<'a>(pub &'a Table);

impl Policy for TablePolicy<'_> {
    fn rank(&self, ask: &Ask, offers: &[Offer]) -> Vec<Ranked> {
        let mut ranked: Vec<Ranked> = Vec::new();
        let mut take = |offer: usize, reason: Option<Reason>| {
            if !ranked.iter().any(|taken| taken.offer == offer) {
                ranked.push(Ranked { offer, reason });
            }
        };
        for rule in self.0.rules.iter().filter(|rule| rule.fits(ask)) {
            if let Some(at) = offers.iter().position(|offer| rule.model.names(offer)) {
                // The seat's own model by a plain rule keeps the ask's reason: nothing was chosen in its place.
                let reason = if offers[at].seat && rule.when.budget.is_none() { None } else { Some(rule.reason()) };
                take(at, reason);
            }
        }
        if let Some(at) = offers.iter().position(|offer| offer.seat) {
            take(at, None);
        }
        ranked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(model: &str, seat: bool) -> Offer {
        Offer {
            provider: "claude-code".into(),
            model: model.into(),
            place: Place::Cloud,
            price: None,
            seat,
            framed: false,
        }
    }

    fn worker(step: Option<Act>, agent: &str) -> Ask {
        Ask { step, agent: Some(agent.into()), ..Ask::of(Seat::Worker, Reason::Seat) }
    }

    #[test]
    fn the_default_sends_claude_code_cheaper_at_the_budgets_step() {
        let table = Table::default();
        let ask = worker(Some(Act::Cheaper), "claude-code");
        assert_eq!(table.named(&ask), [ModelRef::Named("haiku".into())]);
        let offers = [offer("sonnet", true), offer("haiku", false)];
        let ranked = TablePolicy(&table).rank(&ask, &offers);
        assert_eq!(ranked[0], Ranked { offer: 1, reason: Some(Reason::Budget(Act::Cheaper)) });
        assert_eq!(ranked[1], Ranked { offer: 0, reason: None });
        // Before the step: the seat's model, by the ask's reason.
        let before = TablePolicy(&table).rank(&worker(None, "claude-code"), &offers);
        assert_eq!(before[0], Ranked { offer: 0, reason: None });
        // Another agent: no cheaper model named for it.
        let droid = worker(Some(Act::Cheaper), "droid");
        assert!(table.named(&droid).is_empty());
        assert_eq!(TablePolicy(&table).rank(&droid, &[offer("glm", true)])[0].offer, 0);
    }

    #[test]
    fn the_default_leaves_the_conversation_on_its_seat() {
        let table = Table::default();
        for seat in [Seat::Fast, Seat::Smart, Seat::Workshop, Seat::Memory, Seat::App, Seat::Background] {
            let ask = Ask::of(seat, Reason::RuleShort);
            assert!(table.named(&ask).is_empty());
            assert_eq!(TablePolicy(&table).rank(&ask, &[offer("m", true)]), [Ranked { offer: 0, reason: None }]);
        }
    }

    #[test]
    fn the_first_rule_that_fits_and_is_offered_is_taken() {
        let table: Table = serde_json::from_value(serde_json::json!({"schema": 1, "rules": [
            {"id": "gone", "seat": "smart", "model": {"provider": "nobody", "model": "x"}},
            {"id": "tools", "seat": "smart", "when": {"tools": true}, "model": {"provider": "openrouter", "model": "big"}},
            {"id": "smart", "seat": "smart", "model": {"provider": "openrouter", "model": "mid"}}
        ]}))
        .unwrap();
        table.check().unwrap();
        let offers = |models: &[&str]| -> Vec<Offer> {
            models
                .iter()
                .enumerate()
                .map(|(at, model)| Offer { provider: "openrouter".into(), ..offer(model, at == 0) })
                .collect()
        };
        let ask = Ask { tools: true, ..Ask::of(Seat::Smart, Reason::RuleHard) };
        assert_eq!(table.named(&ask).len(), 3);
        let ranked = TablePolicy(&table).rank(&ask, &offers(&["seat-model", "big", "mid"]));
        assert_eq!(ranked[0], Ranked { offer: 1, reason: Some(Reason::Table("tools".into())) });
        let plain = Ask::of(Seat::Smart, Reason::RuleHard);
        let ranked = TablePolicy(&table).rank(&plain, &offers(&["seat-model", "mid"]));
        assert_eq!(ranked[0], Ranked { offer: 1, reason: Some(Reason::Table("smart".into())) });
        // Nothing it names is offered: the seat's.
        let ranked = TablePolicy(&table).rank(&plain, &offers(&["seat-model"]));
        assert_eq!(ranked, [Ranked { offer: 0, reason: None }]);
    }

    #[test]
    fn a_table_says_what_is_wrong_with_it() {
        let mut table = Table::default();
        assert!(table.check().is_ok());
        table.schema = 2;
        assert!(table.check().is_err());
        let twice: Table = serde_json::from_value(serde_json::json!({"schema": 1, "rules": [
            {"id": "a", "seat": "fast", "model": "x"}, {"id": "a", "seat": "smart", "model": "y"}
        ]}))
        .unwrap();
        assert!(twice.check().is_err());
        assert!(serde_json::from_value::<Table>(serde_json::json!({"schema": 1, "rules": [
            {"id": "a", "seat": "planner", "model": "x"}
        ]}))
        .is_err());
        // Round trip: the default as the API gives it.
        let text = serde_json::to_value(Table::default()).unwrap();
        assert_eq!(text["rules"][0]["when"], serde_json::json!({"budget": "cheaper", "agent": "claude-code"}));
        assert_eq!(text["rules"][1]["model"], "seat");
    }
}
