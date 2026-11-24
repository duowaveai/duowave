//! The settings only a computer has — added to «Основание»'s `settings::definitions` under its name: the work folders
//! and the agents' programs (src/works), the model a Турбо's centre named for a character's voice.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub use duowave_base::settings::definitions::*;

use crate::core::settings::{as_one_of, as_string, as_string_record, SettingDefinition, SettingScope};

/// A work folder (docs/plans/router-and-works.md, block A, 6; src/works): a folder on this computer the owner added in
/// the panel for the helper to work in. `kind` — how it is kept: only `git` in the first version (decision 9).
/// `agent` and `model` — its agent and the model for it; empty — the seat «Исполнитель» (Claude Code and its own
/// choice). `maxUsd` — the ceiling of one run. `prepare` — commands for a fresh work copy; `checks` — the folder's own
/// checks after a run; `allow` — what the agent may run without a question, as Claude Code writes its rules
/// (`Bash(npm test *)`; `git push` too only when listed, decision 21).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkFolder {
    pub id: String,
    pub name: String,
    pub path: String,
    #[serde(default = "git_kind")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_usd: Option<f64>,
    #[serde(default)]
    pub prepare: Vec<String>,
    #[serde(default)]
    pub checks: Vec<String>,
    #[serde(default)]
    pub allow: Vec<String>,
}

fn git_kind() -> String {
    "git".into()
}

/// The work folders: each computer's own — its paths stay here (placement/transfer.rs, `LOCAL_SETTINGS`).
pub const WORK_FOLDERS: SettingDefinition<Vec<WorkFolder>> = SettingDefinition {
    key: "work_folders",
    scope: SettingScope::Global,
    description: "Рабочие папки",
    sensitive: false,
    parse: |value| {
        let list = value.as_array()?;
        Some(list.iter().filter_map(|item| serde_json::from_value(item.clone()).ok()).collect())
    },
};

/// Where Claude Code's program is, when not on the search path (src/works/harness/claude_code.rs). Local.
pub const HARNESS_CLAUDE_PATH: SettingDefinition<String> = SettingDefinition {
    key: "harness_claude_path",
    scope: SettingScope::Global,
    description: "Где программа Claude Code",
    sensitive: false,
    parse: as_string,
};

/// How Claude Code pays for a run: `terminal` — its own sign-in, `key` — the gateway's key. Not set — «как в
/// терминале», and the panel asks to choose while it is not signed in. Local.
pub const HARNESS_CLAUDE_AUTH: SettingDefinition<String> = SettingDefinition {
    key: "harness_claude_auth",
    scope: SettingScope::Global,
    description: "Как платит Claude Code",
    sensitive: false,
    parse: |value| as_one_of(value, &["terminal", "key"]),
};

/// Where the programs of the agents' cards are, by the card's id, when not on the search path (stage 3б). Local.
pub const HARNESS_PATHS: SettingDefinition<BTreeMap<String, String>> = SettingDefinition {
    key: "harness_paths",
    scope: SettingScope::Global,
    description: "Где программы агентов",
    sensitive: false,
    parse: as_string_record,
};

/// On a Турбо: the model its centre named for a character's voice, by the character's id — the voice goes with it
/// instead of the one this computer would pick. The Турбо's own.
pub const TURBO_VOICE_ENGINES: SettingDefinition<BTreeMap<String, String>> = SettingDefinition {
    key: "turbo_voice_engines",
    scope: SettingScope::Global,
    description: "Модель голоса персонажа, названная хабом",
    sensitive: false,
    parse: as_string_record,
};
