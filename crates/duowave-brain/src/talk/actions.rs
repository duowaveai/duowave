//! The actions on a work (docs/plans/work-systems.md, «Words that steer works»), by id: what the packs' words of today
//! ask («как там», «и ещё…», «стоп», «подробнее», «открой»), and which of them fit a work now by its system's flags.
//! Words come to an action, never to a verb of a system: a new phrase or a new language changes no code (stage 7
//! moves the words to «Чутьё»).

use super::system::Can;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    /// «как там …»
    Status,
    /// «и ещё …», «добавь …»
    Add,
    /// «стоп», «останови»
    Stop,
    /// «подробнее», «что написал агент»
    Details,
    /// «открой …»
    Open,
    /// «да», «нет», a choice: the answer to a question of the work
    Answer,
    /// «следи за этой»: a work begun outside is said aloud like ours
    Watch,
    /// «не следи»: quiet again
    Unwatch,
}

impl Action {
    pub const ALL: [Action; 8] = [
        Action::Status,
        Action::Add,
        Action::Stop,
        Action::Details,
        Action::Open,
        Action::Answer,
        Action::Watch,
        Action::Unwatch,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Action::Status => "status",
            Action::Add => "add",
            Action::Stop => "stop",
            Action::Details => "details",
            Action::Open => "open",
            Action::Answer => "answer",
            Action::Watch => "watch",
            Action::Unwatch => "unwatch",
        }
    }

    pub fn parse(id: &str) -> Option<Action> {
        Action::ALL.into_iter().find(|action| action.id() == id)
    }

    /// Whether a work of a system with these flags takes it: no `tell_now` and no `tell_after` — no words; no `stop`
    /// — not from here; «как там» always, from the state and the time when there are no steps.
    pub fn fits(self, can: &Can) -> bool {
        match self {
            Action::Status | Action::Watch | Action::Unwatch => true,
            Action::Add => can.tell_now || can.tell_after,
            Action::Stop => can.stop,
            Action::Details => can.result || can.steps,
            Action::Open => can.open,
            Action::Answer => can.questions,
        }
    }

    /// What fits a work of these flags, in order.
    pub fn fitting(can: &Can) -> Vec<Action> {
        Action::ALL.into_iter().filter(|action| action.fits(can)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flags_say_what_fits() {
        let bare = Can { list: true, ..Can::default() };
        assert_eq!(Action::fitting(&bare), vec![Action::Status, Action::Watch, Action::Unwatch]);
        let after = Can { tell_after: true, stop: true, ..Can::default() };
        assert!(Action::Add.fits(&after) && Action::Stop.fits(&after) && !Action::Open.fits(&after));
        for action in Action::ALL {
            assert_eq!(Action::parse(action.id()), Some(action));
        }
    }
}
