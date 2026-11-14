//! The ladder: one step for each pair of participants (docs/plans/bus.md). Each step holds the ones below it:
//! whoever may call an app also hears its news and sees it; whoever has nothing does not know it exists.
use serde::{Deserialize, Serialize};

use super::address::FILES;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// It does not exist for them.
    #[default]
    None,
    /// Knows it is there and what it can do (a folder: its list).
    See,
    /// Hears its news (a folder: «a file came»).
    Listen,
    /// Asks it to do things, by code or in words (a folder: reads its files).
    Call,
    /// Changes it: settings, data, code (a folder: puts, changes, removes).
    Edit,
}

pub const LEVELS: &[Level] = &[Level::None, Level::See, Level::Listen, Level::Call, Level::Edit];

impl Level {
    pub fn parse(text: &str) -> Option<Level> {
        match text {
            "none" => Some(Level::None),
            "see" => Some(Level::See),
            "listen" => Some(Level::Listen),
            "call" => Some(Level::Call),
            "edit" => Some(Level::Edit),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Level::None => "none",
            Level::See => "see",
            Level::Listen => "listen",
            Level::Call => "call",
            Level::Edit => "edit",
        }
    }

    /// What reaching this step is, in people's words for a question: «звать», «читать».
    pub fn verb(self, kind: &str) -> String {
        match (self, kind == FILES) {
            (Level::None, _) => crate::tr!("ничего"),
            (Level::See, _) => crate::tr!("видеть"),
            (Level::Listen, _) => crate::tr!("слышать"),
            (Level::Call, false) => crate::tr!("звать"),
            (Level::Call, true) => crate::tr!("читать файлы в папке"),
            (Level::Edit, false) => crate::tr!("править"),
            (Level::Edit, true) => crate::tr!("менять файлы в папке"),
        }
    }

    /// The step in people's words, as it reads for a participant of this kind.
    pub fn word(self, kind: &str) -> String {
        match (self, kind == FILES) {
            (Level::None, _) => crate::tr!("не знает"),
            (Level::See, _) => crate::tr!("видит"),
            (Level::Listen, _) => crate::tr!("слышит"),
            (Level::Call, false) => crate::tr!("зовёт"),
            (Level::Call, true) => crate::tr!("читает"),
            (Level::Edit, _) => crate::tr!("правит"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_step_holds_the_ones_below() {
        assert!(Level::Edit > Level::Call && Level::Call > Level::Listen && Level::Listen > Level::See);
        assert!(Level::See > Level::None);
        for level in LEVELS {
            assert_eq!(Level::parse(level.as_str()), Some(*level));
        }
        assert_eq!(Level::Call.word("files"), "читает");
        assert_eq!(Level::Call.word("app"), "зовёт");
    }
}
