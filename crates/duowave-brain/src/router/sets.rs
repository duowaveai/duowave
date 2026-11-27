//! Sets of models (docs/plans/work-systems.md, «Sets and limits», owner decision 4): on what a work thinks, chosen
//! as a default browser is. A set maps the tiers (low, mid, high) to models, per agent or system: a work's run takes
//! `mid`, a cheaper step `low`, a review `high`. Three come with DuoWave (D5): «Экономно», «Обычно», «Качество»; a
//! person adds their own, as data in settings. The choice at start, in this order: the words of the phrase («сделай
//! подешевле …»), the panel's switch, the folder's set, the home's. None of them — the models as before a set: the
//! folder's, the seat «Исполнитель», the agent's own.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::core::settings::{SettingDefinition, SettingScope};

use super::seats::CLAUDE_CODE;

pub const ECONOMY: &str = "economy";
pub const USUAL: &str = "usual";
pub const QUALITY: &str = "quality";

/// One's own sets at most.
const OWN_MAX: usize = 20;

/// The step of a work a model is for: its run (`Mid`), a cheaper step (`Low`), a review (`High`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Low,
    Mid,
    High,
}

/// An agent's or a system's models by tier; a tier not given — the next one up, then down.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tiers {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub low: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub high: Option<String>,
}

impl Tiers {
    pub fn of(&self, tier: Tier) -> Option<&str> {
        let order = match tier {
            Tier::Low => [&self.low, &self.mid, &self.high],
            Tier::Mid => [&self.mid, &self.high, &self.low],
            Tier::High => [&self.high, &self.mid, &self.low],
        };
        order.into_iter().find_map(|model| model.as_deref().map(str::trim).filter(|model| !model.is_empty()))
    }
}

/// A set: its id, the title people see, and the models by agent (`claude-code`, a card's id) or system.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSet {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub models: BTreeMap<String, Tiers>,
    /// It came with DuoWave: one's own of its id takes its place.
    #[serde(default)]
    pub bundled: bool,
}

/// One's own sets and the home's default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkSets {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub own: Vec<ModelSet>,
}

/// The sets of works. Travels with the settings package.
pub const WORK_SETS: SettingDefinition<WorkSets> = SettingDefinition {
    key: "work_sets",
    scope: SettingScope::Global,
    description: "Наборы моделей для работ",
    sensitive: false,
    parse: |value| serde_json::from_value(value.clone()).ok(),
};

/// The three that come with DuoWave. Claude Code's models go by its own short names; an agent of a card gets its
/// models in a set of one's own, as their names are the agent's.
pub fn bundled() -> Vec<ModelSet> {
    let claude = |low: &str, mid: &str, high: &str| Tiers {
        low: Some(low.into()),
        mid: Some(mid.into()),
        high: Some(high.into()),
    };
    // T3 Code names the same models by their full ids, with its provider Claude (the hand check of 27.11: its
    // «thread start» begins nothing without a model).
    let t3 = |tiers: &Tiers| Tiers {
        low: tiers.low.as_deref().map(full_claude),
        mid: tiers.mid.as_deref().map(full_claude),
        high: tiers.high.as_deref().map(full_claude),
    };
    let set = |id: &str, title: &str, tiers: Tiers| ModelSet {
        id: id.into(),
        title: title.into(),
        models: BTreeMap::from([(T3_CODE.to_string(), t3(&tiers)), (CLAUDE_CODE.to_string(), tiers)]),
        bundled: true,
    };
    vec![
        set(ECONOMY, "Экономно", claude("haiku", "haiku", "sonnet")),
        set(USUAL, "Обычно", claude("haiku", "sonnet", "opus")),
        set(QUALITY, "Качество", claude("sonnet", "opus", "opus")),
    ]
}

/// The system T3 Code, by its id.
pub const T3_CODE: &str = "t3code";

/// Claude Code's short name of a model as its full id.
fn full_claude(short: &str) -> String {
    format!("claude-{short}-5-5")
}

/// Every set: the bundled ones (one's own of the same id in its place), then one's own.
pub fn all(sets: &WorkSets) -> Vec<ModelSet> {
    let mut all: Vec<ModelSet> = bundled()
        .into_iter()
        .map(|bundled| sets.own.iter().find(|own| own.id == bundled.id).cloned().unwrap_or(bundled))
        .collect();
    let taken: Vec<String> = all.iter().map(|set| set.id.clone()).collect();
    all.extend(sets.own.iter().filter(|own| !taken.contains(&own.id)).cloned());
    all
}

pub fn find(sets: &WorkSets, id: &str) -> Option<ModelSet> {
    all(sets).into_iter().find(|set| set.id == id)
}

/// A set's model for an agent or a system at a tier; none — the set does not reach it.
pub fn model_of(set: &ModelSet, who: &str, tier: Tier) -> Option<String> {
    set.models.get(who).and_then(|tiers| tiers.of(tier)).map(str::to_string)
}

/// The set a step cheaper: «Качество» → «Обычно» → «Экономно»; one's own → «Экономно»; «Экономно» — none.
pub fn cheaper(id: &str) -> Option<&'static str> {
    match id {
        ECONOMY => None,
        QUALITY => Some(USUAL),
        _ => Some(ECONOMY),
    }
}

/// The set of a work: the one said or chosen for it, else its folder's, else the home's.
pub fn chosen<'a>(said: Option<&'a str>, folder: Option<&'a str>, sets: &'a WorkSets) -> Option<&'a str> {
    let given = |id: Option<&'a str>| id.map(str::trim).filter(|id| !id.is_empty());
    given(said).or(given(folder)).or(given(sets.default.as_deref()))
}

/// One's own set as the panel sends it: a plain id, a title, model names without spaces.
pub fn check(set: &ModelSet) -> Result<(), String> {
    let plain = !set.id.is_empty()
        && set.id.len() <= 40
        && set.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !plain {
        return Err("id: a-z, 0-9 and «-», at most 40".into());
    }
    if set.title.trim().is_empty() || set.title.chars().count() > 40 {
        return Err("title: 1 to 40 characters".into());
    }
    if set.models.len() > 20 {
        return Err("models: at most 20 agents or systems".into());
    }
    let model = |model: &Option<String>| {
        model.as_deref().is_none_or(|model| model.len() <= 100 && !model.chars().any(char::is_whitespace))
    };
    if !set.models.values().all(|tiers| model(&tiers.low) && model(&tiers.mid) && model(&tiers.high)) {
        return Err("models: a model's name without spaces, at most 100".into());
    }
    Ok(())
}

/// One's own sets as the panel sends them: each checked, at most 20, no id twice.
pub fn check_all(sets: &WorkSets) -> Result<(), String> {
    if sets.own.len() > OWN_MAX {
        return Err(format!("own: at most {OWN_MAX} sets"));
    }
    for (at, set) in sets.own.iter().enumerate() {
        check(set)?;
        if sets.own[..at].iter().any(|before| before.id == set.id) {
            return Err(format!("own: «{}» twice", set.id));
        }
    }
    if let Some(default) = &sets.default {
        if find(sets, default).is_none() {
            return Err(format!("default: no set «{default}»"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_set_gives_a_model_by_tier_and_the_choice_goes_phrase_folder_home() {
        let sets = WorkSets::default();
        let economy = find(&sets, ECONOMY).unwrap();
        assert_eq!(model_of(&economy, CLAUDE_CODE, Tier::Mid).as_deref(), Some("haiku"));
        // T3 Code gets the same models by their full ids (the hand check of 27.11).
        assert_eq!(model_of(&economy, T3_CODE, Tier::Mid).as_deref(), Some("claude-haiku-5-5"));
        assert_eq!(model_of(&find(&sets, QUALITY).unwrap(), CLAUDE_CODE, Tier::Mid).as_deref(), Some("opus"));
        assert_eq!(model_of(&economy, "droid", Tier::Mid), None, "a card's agent is reached by one's own set");
        let home = WorkSets { default: Some(USUAL.into()), own: Vec::new() };
        assert_eq!(chosen(Some(ECONOMY), Some(QUALITY), &home), Some(ECONOMY));
        assert_eq!(chosen(None, Some(QUALITY), &home), Some(QUALITY));
        assert_eq!(chosen(None, None, &home), Some(USUAL));
        assert_eq!(chosen(None, None, &sets), None, "no set: the models as before");
        assert_eq!(
            (cheaper(QUALITY), cheaper(USUAL), cheaper(ECONOMY), cheaper("mine")),
            (Some(USUAL), Some(ECONOMY), None, Some(ECONOMY))
        );
    }

    #[test]
    fn ones_own_set_takes_the_bundled_ones_place_and_is_checked() {
        let mut droid = ModelSet { id: ECONOMY.into(), title: "Экономно".into(), ..ModelSet::default() };
        droid.models.insert("droid".into(), Tiers { mid: Some("glm-4.6".into()), ..Tiers::default() });
        let sets = WorkSets { default: Some(ECONOMY.into()), own: vec![droid.clone()] };
        assert_eq!(all(&sets).len(), 3);
        let economy = find(&sets, ECONOMY).unwrap();
        assert_eq!(model_of(&economy, "droid", Tier::Low).as_deref(), Some("glm-4.6"), "a tier not given: the next");
        assert!(check_all(&sets).is_ok());
        let mut bad = droid;
        bad.id = "Мой набор".into();
        assert!(check(&bad).is_err());
        let twice = WorkSets { default: None, own: vec![sets.own[0].clone(), sets.own[0].clone()] };
        assert!(check_all(&twice).is_err());
        assert!(check_all(&WorkSets { default: Some("nothing".into()), own: Vec::new() }).is_err());
    }
}
