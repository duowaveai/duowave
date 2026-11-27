//! The seats (docs/plans/router-and-works.md, block A, 3). The seats of «Мозг» keep their settings as they are —
//! `brain` («Быстрая», «Умная», «Запасная») and `workshop.agent` —, so older gateways and the settings package read
//! them as before. The new setting `router_seats` holds the one seat «Мозг» has no row for: «Исполнитель» (`worker`),
//! the agent of a work in a folder — the agent and its model, or the agent's own choice of model (`auto`). A work
//! folder's own agent and model come before the seat; a work asked with an agent comes before its folder. The default:
//! Claude Code with its own choice. The memory's, the apps' and the background works' seats take what «Мозг» has —
//! «Быстрая» or «Умная», or a model of their own settings; they have no row here.

use serde::{Deserialize, Serialize};

/// Claude Code, the agent built in (src/works/harness/claude_code.rs).
pub const CLAUDE_CODE: &str = "claude-code";
/// The word for «the agent's own choice of model».
pub const AUTO: &str = "auto";

/// The setting `router_seats`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seats {
    #[serde(default)]
    pub worker: WorkerSeat,
}

/// «Исполнитель»: which agent, on which model. None — Claude Code; a model of none or `auto` — the agent's own.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerSeat {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl WorkerSeat {
    /// Its agent: the one set, else Claude Code.
    pub fn agent(&self) -> &str {
        self.agent.as_deref().map(str::trim).filter(|agent| !agent.is_empty()).unwrap_or(CLAUDE_CODE)
    }

    /// Its model; None — the agent's own choice.
    pub fn model(&self) -> Option<&str> {
        named(self.model.as_deref())
    }
}

/// An agent as named: none when empty.
fn given(agent: Option<&str>) -> Option<&str> {
    agent.map(str::trim).filter(|agent| !agent.is_empty())
}

/// A model as named: none when empty or `auto`.
fn named(model: Option<&str>) -> Option<&str> {
    model.map(str::trim).filter(|model| !model.is_empty() && !model.eq_ignore_ascii_case(AUTO))
}

/// Who does a work in a folder and on which model: the work's own ask, else its folder's, else the seat «Исполнитель».
/// An agent that is gone (`known` says no) is passed over: the folder's to the seat's, the seat's to Claude Code. A
/// model goes with the agent it was named for: the folder's model holds for the folder's agent or when the folder
/// names none, the seat's — only for the seat's agent.
pub fn worker_of(
    asked: (Option<&str>, Option<&str>),
    folder: (Option<&str>, Option<&str>),
    seat: &WorkerSeat,
    known: impl Fn(&str) -> bool,
) -> (String, Option<String>) {
    let (asked_agent, asked_model) = (given(asked.0), named(asked.1));
    let (folder_agent, folder_model) = (given(folder.0).filter(|agent| known(agent)), named(folder.1));
    let seat_agent = Some(seat.agent()).filter(|agent| known(agent));
    let agent = asked_agent.or(folder_agent).or(seat_agent).unwrap_or(CLAUDE_CODE).to_string();
    let folders = folder_agent.is_none() || folder_agent == Some(agent.as_str());
    let model = asked_model
        .or(folder_model.filter(|_| folders))
        .or(seat.model().filter(|_| seat_agent == Some(agent.as_str())))
        .map(str::to_string);
    (agent, model)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seat(agent: Option<&str>, model: Option<&str>) -> WorkerSeat {
        WorkerSeat { agent: agent.map(str::to_string), model: model.map(str::to_string) }
    }

    #[test]
    fn the_seat_gives_the_agent_and_the_model_when_nothing_else_does() {
        let all = |_: &str| true;
        let empty = WorkerSeat::default();
        assert_eq!(worker_of((None, None), (None, None), &empty, all), (CLAUDE_CODE.into(), None));
        let droid = seat(Some("droid"), Some("glm-4.6"));
        assert_eq!(worker_of((None, None), (None, None), &droid, all), ("droid".into(), Some("glm-4.6".into())));
        assert_eq!(worker_of((None, None), (None, None), &seat(None, Some("auto")), all), (CLAUDE_CODE.into(), None));
    }

    #[test]
    fn the_folder_comes_before_the_seat_and_the_ask_before_the_folder() {
        let all = |_: &str| true;
        let droid = seat(Some("droid"), Some("glm-4.6"));
        // The folder's agent: the seat's model is not for it.
        assert_eq!(worker_of((None, None), (Some(CLAUDE_CODE), None), &droid, all), (CLAUDE_CODE.into(), None));
        assert_eq!(
            worker_of((None, None), (Some(CLAUDE_CODE), Some("sonnet")), &droid, all),
            (CLAUDE_CODE.into(), Some("sonnet".into()))
        );
        // A folder with a model and no agent: the seat's agent on the folder's model.
        assert_eq!(worker_of((None, None), (None, Some("glm-5")), &droid, all), ("droid".into(), Some("glm-5".into())));
        // The work's own ask.
        assert_eq!(
            worker_of((Some("opencode"), None), (Some(CLAUDE_CODE), Some("sonnet")), &droid, all),
            ("opencode".into(), None)
        );
        assert_eq!(
            worker_of((None, Some("haiku")), (Some(CLAUDE_CODE), Some("sonnet")), &droid, all),
            (CLAUDE_CODE.into(), Some("haiku".into()))
        );
    }

    #[test]
    fn an_agent_that_is_gone_is_passed_over() {
        let only_claude = |agent: &str| agent == CLAUDE_CODE;
        let droid = seat(Some("droid"), Some("glm-4.6"));
        assert_eq!(worker_of((None, None), (Some("droid"), None), &droid, only_claude), (CLAUDE_CODE.into(), None));
    }
}
