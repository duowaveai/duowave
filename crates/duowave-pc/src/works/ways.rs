//! The ways of our system (docs/plans/work-systems.md, stage 6; D4 — built on the owner's word): «сразу» is one run,
//! as before; «с проверкой» reviews the work done in a fresh session on another model, read only, by the task, the
//! difference and a list of risks, and its blocking remarks go back to the writer, two rounds at most; «с планом»
//! writes a plan first, in a read-only session, waits for «да», then does the work by it and reviews it.
//!
//! The prompts follow PurpleFlow's (github.com/PurpleSchool/purpleflow, MIT): the review's list of risks, «blocking»
//! against «notes», a remark blocking only with a concrete case; the plan's steps, what must not break and its one
//! command of checks.

use serde::Deserialize;

use crate::router::sets::{self, Tier, WorkSets};

pub const REVIEW: &str = "review";
pub const PLAN: &str = "plan";
/// How many times the review's remarks go back to the writer before the person decides.
pub const REVIEW_ROUNDS: u32 = 2;
/// What a read-only session may run: reading the copy's history and difference.
pub const READ_ONLY: [&str; 3] = ["Bash(git diff:*)", "Bash(git log:*)", "Bash(git status:*)"];
/// The head of the plan in the writer's task.
const PLAN_HEAD: &str = "The plan agreed with the person:";

/// A way as asked: `review`, `plan`; anything else — at once.
pub fn parse(way: Option<&str>) -> Option<&'static str> {
    match way.map(str::trim) {
        Some(REVIEW) => Some(REVIEW),
        Some(PLAN) => Some(PLAN),
        _ => None,
    }
}

/// The way's work is reviewed after it: «с проверкой» and «с планом».
pub fn reviews(way: Option<&str>) -> bool {
    parse(way).is_some()
}

/// The planner's words: a plan only, nothing changed, in the task's language, short enough to say aloud.
pub fn plan_prompt(task: &str) -> String {
    format!(
        "<duowave-plan>\nYou plan a work in this folder and change nothing. The task:\n\n{task}\n\nRead what you need. \
         Answer with the plan only, in the language of the task: three to seven short numbered steps, then «Не \
         сломать:» with what must keep working, then «Проверка:» with one command that checks the work. No code."
    )
}

/// The writer's task with the plan the person agreed to.
pub fn with_plan(task: &str, plan: &str) -> String {
    format!("{task}\n\n{PLAN_HEAD}\n{plan}")
}

/// The reviewer's words: the task, the plan if there is one, the difference from the base, the list of risks; the
/// answer as JSON.
pub fn review_prompt(task: &str, base: Option<&str>, plan: Option<&str>) -> String {
    let diff = match base {
        Some(base) => format!("`git diff {base}`"),
        None => "`git diff HEAD~1` and `git status`".to_string(),
    };
    let plan = plan.map(|plan| format!("The plan it followed:\n{plan}\n\n")).unwrap_or_default();
    format!(
        "<duowave-review>\nYou review a work done in this copy and change nothing. The task:\n\n{task}\n\n{plan}See the \
         difference with {diff} and read what you need. Check: the task is done in full; nothing that worked is \
         broken; money and limits; states that can get stuck; a failure in the middle; repeats and races; rights, \
         data and migrations. A remark is blocking only with a concrete case that goes wrong. Answer with JSON only: \
         {{\"verdict\": \"ok\" or \"fix\", \"blocking\": [one line each], \"notes\": [what may wait]}}."
    )
}

/// The writer's words after a review: what must be fixed.
pub fn fix_prompt(blocking: &[String]) -> String {
    let list: Vec<String> = blocking.iter().map(|remark| format!("- {remark}")).collect();
    format!(
        "The review found what must be fixed:\n{}\nFix it, then say in two sentences what you changed.",
        list.join("\n")
    )
}

/// The reviewer's answer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Verdict {
    #[serde(default)]
    pub verdict: String,
    #[serde(default)]
    pub blocking: Vec<String>,
    #[serde(default)]
    pub notes: Vec<String>,
}

impl Verdict {
    /// Nothing to fix: `ok`, or no blocking remark.
    pub fn clean(&self) -> bool {
        self.verdict.trim() == "ok" || self.blocking.iter().all(|remark| remark.trim().is_empty())
    }
}

/// The verdict in the reviewer's last message: its JSON, wherever it stands in the text.
pub fn verdict_of(text: &str) -> Option<Verdict> {
    let (start, end) = (text.find('{')?, text.rfind('}')?);
    let verdict: Verdict = serde_json::from_str(text.get(start..=end)?).ok()?;
    (!verdict.verdict.is_empty()).then_some(verdict)
}

/// The reviewer's model: the set's strongest for the agent (the work's set, else «Обычно»), so that it is not weaker
/// than the writer's; none — the agent's own.
pub fn reviewer_model(all: &WorkSets, set: Option<&str>, agent: &str) -> Option<String> {
    let set = sets::find(all, set.unwrap_or(sets::USUAL))?;
    sets::model_of(&set, agent, Tier::High)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_verdict_is_read_from_the_reviewers_words_and_the_ways_are_parsed() {
        let said =
            "Посмотрел дифф.\n{\"verdict\": \"fix\", \"blocking\": [\"нет проверки на пустой email\"], \"notes\": []}";
        let verdict = verdict_of(said).unwrap();
        assert!(!verdict.clean());
        assert_eq!(verdict.blocking, ["нет проверки на пустой email"]);
        assert!(verdict_of("{\"verdict\": \"ok\"}").unwrap().clean());
        assert_eq!(verdict_of("no json here"), None);
        assert_eq!(
            (parse(Some("plan")), parse(Some("review")), parse(Some("x")), parse(None)),
            (Some(PLAN), Some(REVIEW), None, None)
        );
        assert!(reviews(Some("plan")) && !reviews(None));
        let sets = WorkSets::default();
        assert_eq!(reviewer_model(&sets, None, "claude-code").as_deref(), Some("opus"));
        assert_eq!(reviewer_model(&sets, Some("economy"), "claude-code").as_deref(), Some("sonnet"));
        assert!(review_prompt("поправь", Some("abc123"), Some("1. Шаг")).contains("`git diff abc123`"));
        assert!(fix_prompt(&["a".into(), "b".into()]).contains("- a\n- b"));
    }
}
