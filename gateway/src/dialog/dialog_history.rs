//! The recent exchange per device, for follow-up questions. A long pause starts the dialog over.

use std::collections::HashMap;

use crate::dialog::llm::{AssistantTurn, LlmMessage};
use crate::utils::time::now_ms;

pub struct DialogHistory {
    /// Question and answer pairs kept.
    max_turns: usize,
    ttl_ms: u64,
    now: Box<dyn Fn() -> u64 + Send + Sync>,
    dialogs: HashMap<String, Dialog>,
}

struct Dialog {
    turns: Vec<LlmMessage>,
    at: u64,
}

impl DialogHistory {
    pub fn new(max_turns: usize, ttl_ms: u64) -> DialogHistory {
        DialogHistory::with_clock(max_turns, ttl_ms, now_ms)
    }

    /// With its own clock: tests.
    pub fn with_clock(max_turns: usize, ttl_ms: u64, now: impl Fn() -> u64 + Send + Sync + 'static) -> DialogHistory {
        DialogHistory { max_turns, ttl_ms, now: Box::new(now), dialogs: HashMap::new() }
    }

    /// The questions and answers so far: user and assistant turns in order.
    pub fn turns(&mut self, key: &str) -> Vec<LlmMessage> {
        let Some(dialog) = self.dialogs.get(key) else { return Vec::new() };
        if (self.now)().saturating_sub(dialog.at) > self.ttl_ms {
            self.dialogs.remove(key);
            return Vec::new();
        }
        dialog.turns.clone()
    }

    pub fn forget(&mut self, key: &str) {
        self.dialogs.remove(key);
    }

    /// `source`: where the answer came from (real data, a skill), kept with the question for the model to see.
    pub fn remember(&mut self, key: &str, question: &str, answer: &str, source: Option<&str>) {
        if self.max_turns == 0 || answer.is_empty() {
            return;
        }
        let asked = match source {
            Some(source) => format!("{question}\n\n({source})"),
            None => question.to_string(),
        };
        let mut turns = self.turns(key);
        turns.push(LlmMessage::User(asked));
        turns.push(LlmMessage::Assistant(AssistantTurn { content: answer.to_string(), ..AssistantTurn::default() }));
        let keep = self.max_turns * 2;
        if turns.len() > keep {
            turns.drain(..turns.len() - keep);
        }
        self.dialogs.insert(key.to_string(), Dialog { turns, at: (self.now)() });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    fn said(turns: &[LlmMessage]) -> Vec<String> {
        turns
            .iter()
            .map(|turn| match turn {
                LlmMessage::User(text) => format!("user: {text}"),
                LlmMessage::Assistant(turn) => format!("assistant: {}", turn.content),
                LlmMessage::Tool(_) => "tool".into(),
            })
            .collect()
    }

    #[test]
    fn keeps_the_last_exchanges_per_device_until_a_long_pause() {
        let clock = Arc::new(AtomicU64::new(1_000));
        let now = clock.clone();
        let mut history = DialogHistory::with_clock(2, 60_000, move || now.load(Ordering::SeqCst));
        history.remember("a", "раз", "один", None);
        history.remember("a", "два", "два", Some("Ответ дан навыком колонки по настоящим данным."));
        history.remember("a", "три", "три", None);
        history.remember("a", "пусто", "", None);
        assert_eq!(
            said(&history.turns("a")),
            vec![
                "user: два\n\n(Ответ дан навыком колонки по настоящим данным.)",
                "assistant: два",
                "user: три",
                "assistant: три"
            ]
        );
        assert!(history.turns("b").is_empty(), "another device has its own dialog");
        clock.store(61_000, Ordering::SeqCst);
        assert_eq!(history.turns("a").len(), 4, "exactly the TTL still counts");
        clock.store(61_001, Ordering::SeqCst);
        assert!(history.turns("a").is_empty(), "a long pause starts over");
        history.remember("a", "снова", "да", None);
        history.forget("a");
        assert!(history.turns("a").is_empty());
        let mut none = DialogHistory::new(0, 60_000);
        none.remember("a", "q", "a", None);
        assert!(none.turns("a").is_empty(), "DIALOG_HISTORY_TURNS=0 keeps nothing");
    }
}
