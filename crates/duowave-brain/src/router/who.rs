//! Who gets a work (docs/plans/work-systems.md, «Who gets a work», owner decision 8): a plain task goes by the setup,
//! as a default browser does; a system's name in the phrase is the exception, for one work. The order: named in the
//! phrase; the work folder's own system; the home's default; ours (`duowave`). Nothing is guessed from the task's
//! content.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::core::settings::{SettingDefinition, SettingScope};
use crate::talk::system::DUOWAVE;

/// The setup: the home's default system and a folder's own, by the folder's id.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkSystemsSetup {
    /// The home's default; none — ours.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub folders: BTreeMap<String, String>,
}

/// Who gets the works: the home's default and the folders' own. Travels with the settings package.
pub const WORK_SYSTEMS: SettingDefinition<WorkSystemsSetup> = SettingDefinition {
    key: "work_systems",
    scope: SettingScope::Global,
    description: "Кому поручать работы",
    sensitive: false,
    parse: |value| serde_json::from_value(value.clone()).ok(),
};

/// The system for a work: the one named, else the folder's, else the home's, else ours.
pub fn who_gets<'a>(setup: &'a WorkSystemsSetup, named: Option<&'a str>, folder: Option<&str>) -> &'a str {
    named
        .or_else(|| folder.and_then(|folder| setup.folders.get(folder)).map(String::as_str))
        .or(setup.default.as_deref())
        .unwrap_or(DUOWAVE)
}

/// The answer to «Что поручать …?» when a system is connected (D8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Assign {
    /// «Все работы»: the home's default.
    All,
    /// «Работы в его проектах»: the system of these work folders (its places).
    Places(Vec<String>),
    /// «Только когда назову»: the defaults are not its.
    Named,
}

impl Assign {
    pub fn parse(mode: &str, folders: Vec<String>) -> Option<Assign> {
        match mode {
            "all" => Some(Assign::All),
            "places" => Some(Assign::Places(folders)),
            "named" => Some(Assign::Named),
            _ => None,
        }
    }
}

/// The setup with a system's answer taken: what was the system's before goes, what is asked comes.
pub fn assign(setup: &mut WorkSystemsSetup, system: &str, assign: &Assign) {
    if setup.default.as_deref() == Some(system) {
        setup.default = None;
    }
    setup.folders.retain(|_, own| own != system);
    match assign {
        Assign::All => setup.default = (system != DUOWAVE).then(|| system.to_string()),
        Assign::Places(folders) => {
            for folder in folders {
                setup.folders.insert(folder.clone(), system.to_string());
            }
        }
        Assign::Named => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_said_goes_over_the_folder_over_the_home_over_ours() {
        let mut setup = WorkSystemsSetup::default();
        assert_eq!(who_gets(&setup, None, Some("site")), DUOWAVE);
        assign(&mut setup, "bb", &Assign::All);
        assert_eq!(who_gets(&setup, None, Some("site")), "bb");
        assign(&mut setup, "t3code", &Assign::Places(vec!["site".into()]));
        assert_eq!(who_gets(&setup, None, Some("site")), "t3code");
        assert_eq!(who_gets(&setup, None, Some("blog")), "bb");
        assert_eq!(who_gets(&setup, Some("claude-code"), Some("site")), "claude-code");
        // «Только когда назову»: its defaults go.
        assign(&mut setup, "bb", &Assign::Named);
        assert_eq!(who_gets(&setup, None, Some("blog")), DUOWAVE);
        assign(&mut setup, "t3code", &Assign::Named);
        assert_eq!(setup, WorkSystemsSetup::default());
    }
}
