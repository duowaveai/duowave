//! A draft's card (docs/plans/voice-creation.md, «Черновик: кто просил и где он сейчас»): who asked and what, the face
//! people see it with, and where it is — `working → ready → installed | parked`, or `failed`, `stopped`. The gateway
//! keeps it beside the drafts, `packages-dev/.meta/<id>.json`: a name with a dot is out of reach of a draft's file API,
//! so neither the agent nor a person edits it as a draft's file. A draft without one (made before it, by hand, by
//! Claude Code) gets one read from what is known: `golos-…` is the station's, the rest the panel's; the stage from the
//! agent's work.
//!
//! What the draft needs (`needs`: its manifest's settings, plain and private, never their values) and the line that
//! says it is ready (`ready_line`) are the gateway's, read from the manifest, not the agent's words: the station says
//! that line, the panel writes it.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::activity::{By, Job, StepKind};
use crate::plugins::manifest::{Manifest, PackageKind, SettingType};

/// The face people see a draft with: what it becomes on the desk or for the station. One app may have several faces
/// (a widget and phrases); the draft's is the one it was asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Face {
    Widget,
    Skill,
    Game,
    Theme,
}

impl Face {
    pub fn parse(text: &str) -> Option<Face> {
        match text {
            "widget" => Some(Face::Widget),
            "skill" | "plugin" => Some(Face::Skill),
            "game" | "quiz" => Some(Face::Game),
            "theme" => Some(Face::Theme),
            _ => None,
        }
    }

    /// «виджет», «навык», «игра», «тема».
    pub fn word(self) -> &'static str {
        match self {
            Face::Widget => "виджет",
            Face::Skill => "навык",
            Face::Game => "игра",
            Face::Theme => "тема",
        }
    }

    /// As in «Делаю виджет», «Делаю игру».
    pub fn accusative(self) -> &'static str {
        match self {
            Face::Widget => "виджет",
            Face::Skill => "навык",
            Face::Game => "игру",
            Face::Theme => "тему",
        }
    }

    /// «Ему» or «Ей»: who needs a key.
    fn dative(self) -> &'static str {
        match self {
            Face::Widget | Face::Skill => "Ему",
            Face::Game | Face::Theme => "Ей",
        }
    }

    /// The template a new draft of this face starts from (templates.rs).
    pub fn template(self) -> &'static str {
        match self {
            Face::Widget => "widget",
            Face::Skill => "plugin",
            Face::Game => "quiz",
            Face::Theme => "theme",
        }
    }

    /// The face a manifest shows: a theme, a widget of the desk, or phrases for the station.
    pub fn of(manifest: &Manifest) -> Face {
        if manifest.kind == PackageKind::Theme {
            return Face::Theme;
        }
        let slots = manifest.contributes.panel.as_ref().and_then(|panel| panel.get("slots")).and_then(Value::as_array);
        if slots.is_some_and(|slots| slots.iter().any(|slot| slot.as_str() == Some("desk.widgets"))) {
            return Face::Widget;
        }
        Face::Skill
    }
}

/// Where a draft is: the agent works on it, it waits «на примерке», it went on the desk (installed), it waits in the
/// workshop («в черновиках»), the agent failed or was stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Stage {
    Working,
    Ready,
    Installed,
    Parked,
    Failed,
    Stopped,
}

impl Stage {
    pub fn as_str(self) -> &'static str {
        match self {
            Stage::Working => "working",
            Stage::Ready => "ready",
            Stage::Installed => "installed",
            Stage::Parked => "parked",
            Stage::Failed => "failed",
            Stage::Stopped => "stopped",
        }
    }

    /// How a run of the agent that ended so leaves the draft.
    pub fn after(kind: StepKind) -> Stage {
        match kind {
            StepKind::Failed => Stage::Failed,
            StepKind::Stopped => Stage::Stopped,
            _ => Stage::Ready,
        }
    }
}

/// The draft's card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftMeta {
    /// Who asked: the panel (someone signed in) or a station by voice.
    pub by: By,
    /// The person signed in, or the station (its device tag): the station that asked hears «Готово».
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub who: Option<String>,
    /// The words of the request.
    pub asked: String,
    pub kind: Face,
    pub stage: Stage,
    /// When the card changed, ms since the epoch.
    pub updated: u64,
    /// The agent's last words of its last run: what it did on its own, or its one question — said with «Готово».
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Kept by the gateway: false for a card read from what is known (a draft made before cards).
    #[serde(default = "kept")]
    pub kept: bool,
}

fn kept() -> bool {
    true
}

impl DraftMeta {
    /// The card of a draft that has none: whose by its id, the face from its manifest, the stage from the agent's work
    /// (none — it waits in the workshop).
    pub fn known(id: &str, manifest: Option<&Manifest>, job: Option<&Job>, updated: u64) -> DraftMeta {
        let by = if id.starts_with("golos") { By::Voice } else { By::Panel };
        let stage = match job {
            Some(job) if job.busy => Stage::Working,
            Some(job) => job.last.as_ref().map(|step| Stage::after(step.kind)).unwrap_or(Stage::Parked),
            None => Stage::Parked,
        };
        DraftMeta {
            by: job.map(|job| job.by).unwrap_or(by),
            who: None,
            asked: job.map(|job| job.asked.clone()).unwrap_or_default(),
            kind: manifest.map(Face::of).unwrap_or(Face::Skill),
            stage,
            updated: job.map(|job| job.ended.unwrap_or(job.since)).unwrap_or(updated),
            note: None,
            kept: false,
        }
    }
}

/// A setting the draft needs, without its value: plain ones may be said, private ones (`secret`) only typed. A
/// private one without a default must be filled before the draft goes on the desk.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Need {
    pub key: String,
    pub title: String,
    pub r#type: SettingType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
    /// A plain one's default; a private one never shows one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    pub secret: bool,
    pub required: bool,
}

/// What the draft's manifest asks to be filled in.
pub fn needs_of(manifest: &Manifest) -> Vec<Need> {
    manifest
        .settings
        .iter()
        .map(|setting| Need {
            key: setting.key.clone(),
            title: setting.title.clone(),
            r#type: setting.r#type,
            description: setting.description.clone(),
            options: setting.options.clone(),
            default: setting.default.clone().filter(|_| !setting.secret),
            secret: setting.secret,
            required: setting.secret && setting.default.is_none(),
        })
        .collect()
}

/// «нужен» + «ключ Todoist», «нужна» + «ссылка iCal»: the first word's ending picks the form (a key, a link, a name).
fn needed(title: &str) -> (&'static str, String) {
    let first = title.split_whitespace().next().unwrap_or_default().to_lowercase();
    let what: String = {
        let mut chars = title.trim().chars();
        match chars.next() {
            Some(first) => first.to_lowercase().chain(chars).collect(),
            None => String::new(),
        }
    };
    let form = if first.ends_with("мя") || first.ends_with('о') || first.ends_with('е') {
        "нужно"
    } else if first.ends_with('а') || first.ends_with('я') {
        "нужна"
    } else {
        "нужен"
    };
    (form, what)
}

/// «Готово: виджет «Дела на сегодня». Ему нужен ключ Todoist. Примерите — или сразу на стол?» — what the station
/// says and the panel writes when the agent finished.
pub fn ready_line(face: Face, title: &str, needs: &[Need]) -> String {
    let mut line = format!("Готово: {} «{}».", face.word(), title.trim());
    let required: Vec<&Need> = needs.iter().filter(|need| need.required).collect();
    match required.as_slice() {
        [] => {}
        [one] => {
            let (form, what) = needed(&one.title);
            line.push_str(&format!(" {} {form} {what}.", face.dative()));
        }
        many => {
            let titles: Vec<String> = many.iter().map(|need| needed(&need.title).1).collect();
            line.push_str(&format!(" {} нужны: {}.", face.dative(), titles.join(", ")));
        }
    }
    line.push_str(match face {
        Face::Widget => " Примерите — или сразу на стол?",
        _ => " Примерите — или сразу включить?",
    });
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn manifest(settings: Value, panel: Option<Value>) -> Manifest {
        let mut value = json!({
            "schema": 1, "kind": "plugin", "id": "todo", "version": "0.1.0", "title": "Дела на сегодня", "api": "^1",
            "runs": ["pc"], "permissions": ["net:api.todoist.com"], "settings": settings, "contributes": {}
        });
        if let Some(panel) = panel {
            value["contributes"]["panel"] = panel;
        }
        crate::plugins::manifest::check(&value).unwrap()
    }

    #[test]
    fn says_what_is_ready_and_what_it_needs_from_the_manifest() {
        let todo = manifest(
            json!([
                { "key": "token", "type": "string", "title": "Ключ Todoist", "secret": true },
                { "key": "count", "type": "number", "title": "Показывать", "default": 5 },
                { "key": "pin", "type": "string", "title": "Код", "secret": true, "default": "" }
            ]),
            Some(json!({ "slots": ["desk.widgets"] })),
        );
        assert_eq!(Face::of(&todo), Face::Widget);
        let needs = needs_of(&todo);
        let shown: Vec<(&str, bool, bool, Option<&Value>)> =
            needs.iter().map(|need| (need.key.as_str(), need.secret, need.required, need.default.as_ref())).collect();
        assert_eq!(
            shown,
            [("token", true, true, None), ("count", false, false, Some(&json!(5))), ("pin", true, false, None)],
            "a private one with a default is not required, and its default is not shown"
        );
        assert_eq!(
            ready_line(Face::Widget, "Дела на сегодня", &needs),
            "Готово: виджет «Дела на сегодня». Ему нужен ключ Todoist. Примерите — или сразу на стол?"
        );
        let link = manifest(json!([{ "key": "ics", "type": "string", "title": "Ссылка iCal", "secret": true }]), None);
        assert_eq!(Face::of(&link), Face::Skill);
        assert_eq!(
            ready_line(Face::Game, "Викторина", &needs_of(&link)),
            "Готово: игра «Викторина». Ей нужна ссылка iCal. Примерите — или сразу включить?"
        );
        assert_eq!(ready_line(Face::Theme, "Лес", &[]), "Готово: тема «Лес». Примерите — или сразу включить?");
        let both = manifest(
            json!([
                { "key": "token", "type": "string", "title": "Ключ Todoist", "secret": true },
                { "key": "ics", "type": "string", "title": "Ссылка iCal", "secret": true }
            ]),
            None,
        );
        assert_eq!(
            ready_line(Face::Skill, "Дела", &needs_of(&both)),
            "Готово: навык «Дела». Ему нужны: ключ Todoist, ссылка iCal. Примерите — или сразу включить?"
        );
    }

    #[test]
    fn reads_a_card_for_a_draft_made_before_cards() {
        let card = DraftMeta::known("golos-2", None, None, 7);
        assert_eq!(
            (card.by, card.stage, card.kind, card.kept, card.updated),
            (By::Voice, Stage::Parked, Face::Skill, false, 7)
        );
        let job = Job {
            draft: "radio".into(),
            by: By::Panel,
            asked: "добавь станцию".into(),
            since: 10,
            busy: true,
            ended: None,
            last: None,
        };
        let card = DraftMeta::known("radio", None, Some(&job), 7);
        assert_eq!(
            (card.by, card.stage, card.asked.as_str(), card.updated),
            (By::Panel, Stage::Working, "добавь станцию", 10)
        );
        // A card on disk without `kept` reads as one the gateway keeps.
        let read: DraftMeta = serde_json::from_value(json!({
            "by": "voice", "who": "kitchen", "asked": "сделай игру", "kind": "game", "stage": "ready", "updated": 1
        }))
        .unwrap();
        assert!(read.kept);
    }
}
