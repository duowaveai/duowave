//! Rounds for voice, and `wait` (docs/plans/thinking.md, §8; the owner's decision 1: C, by time, with `wait`). The
//! policy is a variant, `thinking.rounds.policy`: `today` (the dialog's own two or three rounds and 25 s, «Сейчас
//! посмотрю.» after a second) or `by_time`.
//!
//! `by_time`:
//! 1. Before each round, the time spent plus what this round is expected to take, against the phrase's
//!    `budget.first_word_ms` (the setting's for a phrase that gives none). It fits — the round goes on in silence.
//!    It does not, and nothing has been said yet — one `wait` first (`model`, with `expect_ms`), never two before a
//!    word.
//! 2. The expected time is the median of this model's rounds in the turns' lines, once there are enough of them
//!    (`rounds.journal`); a guess before (`rounds.guess`).
//! 3. The ceiling: rounds with tools before the first word (`rounds.ceiling`: voice 3, text 5); past it the round is
//!    words only. After the first word, today's rounds and times hold.
//! 4. The device's limit (`budget.limit_ms`, else `rounds.limit`): when the words would no longer fit, the round is
//!    the last, words only.
//! 5. A tool still running after `wait.tool_ms` gets a `wait` (`tool`); while nothing was said, Thinking says today's
//!    «Сейчас посмотрю.» before it, once a turn (`rounds.filler`): a device that ignores `wait` (the computer) would be
//!    silent otherwise.
//!
//! Its decisions go into the turn's line, so the policy runs in shadow on real timings while `today` is live.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::dialog::{Before, Steer};
use crate::door::WaitWhy;

use super::line::Line;
use super::out::Out;
use super::settings::{RoundsPolicy, Settings, Where};
use super::stats::median;

/// The expected times of the models' rounds, from the turns' lines.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Expected {
    /// A model → the median of its rounds, when it has enough of them.
    pub round_ms: HashMap<String, u64>,
}

impl Expected {
    /// The medians of the models with at least `enough` rounds in these lines.
    pub fn of(lines: &[Line], enough: usize) -> Expected {
        let mut by_model: HashMap<String, Vec<u64>> = HashMap::new();
        for line in lines {
            let Some(model) = &line.model else { continue };
            by_model.entry(model.clone()).or_default().extend(line.rounds.iter().map(|round| round.ms));
        }
        let round_ms = by_model
            .into_iter()
            .filter(|(_, rounds)| rounds.len() >= enough.max(1))
            .filter_map(|(model, mut rounds)| median(&mut rounds).map(|ms| (model, ms)))
            .collect();
        Expected { round_ms }
    }

    pub fn round(&self, model: &str, guess: u64) -> u64 {
        self.round_ms.get(model).copied().unwrap_or(guess)
    }
}

/// The numbers `by_time` decides with, for one turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    pub first_word_ms: u64,
    pub ceiling: usize,
    pub limit_ms: u64,
    pub words_ms: u64,
    pub guess_round_ms: u64,
    pub tool_ms: u64,
    /// A slow tool's filler words are said before its `wait`, once a turn, while nothing was said.
    pub filler: bool,
}

impl Limits {
    /// The phrase's budget first; the settings' numbers only for what it gives none of.
    pub fn of(settings: &Settings, place: Where<'_>, budget: &crate::door::Budget) -> Limits {
        Limits {
            first_word_ms: budget.first_word_ms.unwrap_or_else(|| settings.first_word_ms(place)),
            ceiling: settings.ceiling(place),
            limit_ms: budget.limit_ms.unwrap_or(settings.rounds.limit.ms),
            words_ms: settings.rounds.limit.words_ms,
            guess_round_ms: settings.rounds.guess.round_ms,
            tool_ms: settings.wait.tool_ms,
            filler: settings.rounds.filler.at(place, &true),
        }
    }
}

/// What `by_time` decided before a round.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    On,
    /// A `wait` first, with the round's expected time.
    Wait(u64),
    /// Words only: the ceiling, or the device's limit.
    Words,
}

/// `by_time`'s decision before a round.
pub fn decide(limits: &Limits, expected: &Expected, before: &Before, waited: bool) -> Decision {
    if before.elapsed_ms + limits.words_ms >= limits.limit_ms {
        return Decision::Words;
    }
    if before.said {
        // After the first word today's rounds and times hold.
        return Decision::On;
    }
    if before.round >= limits.ceiling {
        return Decision::Words;
    }
    let expect = expected.round(&before.model, limits.guess_round_ms);
    if before.elapsed_ms + expect > limits.first_word_ms && !waited {
        return Decision::Wait(expect);
    }
    Decision::On
}

/// The decisions written as the line keeps them: `wait@720:900`, `words@r3`.
fn written(decision: &Decision, before: &Before) -> Option<String> {
    match decision {
        Decision::On => None,
        Decision::Wait(expect) => Some(format!("wait@{}:{expect}", before.elapsed_ms)),
        Decision::Words => Some(format!("words@r{}", before.round)),
    }
}

/// The policy's hooks into a turn's rounds: live (it sends `wait`s and ends rounds) or in shadow (it only writes
/// what it would have done).
pub fn steer(
    policy: RoundsPolicy,
    shadow: bool,
    limits: Limits,
    expected: Expected,
    out: &Out,
    decisions: Arc<Mutex<Vec<String>>>,
) -> Steer {
    let live = policy == RoundsPolicy::ByTime;
    if !live && !shadow {
        return Steer::default();
    }
    let (out_before, out_slow) = (out.clone(), out.clone());
    let waited = Arc::new(Mutex::new(false));
    let (written_before, written_slow) = (decisions.clone(), decisions);
    let tool_ms = limits.tool_ms;
    let (filler, filled) = (limits.filler, Arc::new(std::sync::atomic::AtomicBool::new(false)));
    let before = Arc::new(move |before: &Before| {
        // A word said since the wait: the next wait may come again after it.
        let mut waited = waited.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if before.said {
            *waited = false;
        }
        let decision = decide(&limits, &expected, before, *waited);
        if let Some(line) = written(&decision, before) {
            written_before.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(line);
        }
        if !live {
            return false;
        }
        match decision {
            Decision::On => false,
            Decision::Wait(expect) => {
                *waited = true;
                out_before.wait(WaitWhy::Model, Some(expect), None);
                false
            }
            Decision::Words => true,
        }
    });
    let slow = Arc::new(move |names: &[String], said: bool| {
        written_slow
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(format!("wait_tool@{}", names.join(",")));
        if live {
            // A device that ignores `wait` (the computer) hears the filler: it is Thinking's to say, once a turn.
            if filler && !said && !filled.swap(true, std::sync::atomic::Ordering::SeqCst) {
                out_slow.say(&crate::dialog::LOOKUP_FILLER.phrase);
            }
            // The next words are late: the audio side fills the silence by its own rules too.
            out_slow.wait(WaitWhy::Tool, None, names.first().cloned());
        }
    });
    Steer { seen: None, before: Some(before), slow: live.then_some(slow), slow_ms: tool_ms }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> Limits {
        Limits {
            first_word_ms: 1500,
            ceiling: 3,
            limit_ms: 25_000,
            words_ms: 6_000,
            guess_round_ms: 1000,
            tool_ms: 1000,
            filler: false,
        }
    }

    fn before(round: usize, elapsed_ms: u64, said: bool) -> Before {
        Before { round, said, elapsed_ms, model: "m".into() }
    }

    #[test]
    fn a_round_that_fits_goes_on_in_silence_one_that_does_not_waits_once() {
        let expected = Expected { round_ms: HashMap::from([("m".to_string(), 800)]) };
        assert_eq!(decide(&limits(), &expected, &before(0, 0, false), false), Decision::On, "0 + 800 fits 1500");
        assert_eq!(
            decide(&limits(), &expected, &before(1, 720, false), false),
            Decision::Wait(800),
            "720 + 800 > 1500"
        );
        assert_eq!(
            decide(&limits(), &expected, &before(1, 720, false), true),
            Decision::On,
            "never two waits before a word"
        );
        assert_eq!(
            decide(&limits(), &expected, &before(3, 1900, false), true),
            Decision::Words,
            "the fourth round: words"
        );
        assert_eq!(decide(&limits(), &expected, &before(5, 2000, true), false), Decision::On, "after the first word");
        assert_eq!(
            decide(&limits(), &expected, &before(1, 19_500, true), false),
            Decision::Words,
            "the device's limit"
        );
    }

    #[test]
    fn the_expected_time_is_the_median_of_enough_rounds_else_the_guess() {
        let line = |model: &str, ms: &[u64]| Line {
            model: Some(model.into()),
            rounds: ms.iter().map(|ms| super::super::line::Round { ms: *ms, ..Default::default() }).collect(),
            ..Line::default()
        };
        let lines = [line("fast", &[700, 900, 800]), line("fast", &[600]), line("slow", &[3000])];
        let expected = Expected::of(&lines, 4);
        assert_eq!(expected.round("fast", 1000), 700, "the lower middle of 600, 700, 800, 900");
        assert_eq!(expected.round("slow", 1000), 1000, "one round is not enough: the guess");
    }

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use futures::StreamExt;
    use tokio_util::sync::CancellationToken;

    use crate::dialog::llm::{
        AssistantTurn, LlmClient, LlmEvent, LlmRequest, LlmStream, LlmTool, ToolCall, ToolChoice,
    };
    use crate::dialog::{DialogContext, ToolBox, ToolOutcome, LOOKUP_FILLER};
    use crate::door::What;

    /// A model whose n-th round takes `delays[n]` ms of the test's clock and calls `look` while n < `calls`.
    struct Timed {
        delays: Vec<u64>,
        calls: usize,
        n: AtomicUsize,
        requests: Mutex<Vec<LlmRequest>>,
    }

    impl LlmClient for Timed {
        fn description(&self) -> String {
            "timed".into()
        }

        fn tools(&self) -> bool {
            true
        }

        fn stream(&self, request: LlmRequest, _cancel: CancellationToken) -> LlmStream {
            let n = self.n.fetch_add(1, Ordering::SeqCst);
            self.requests.lock().unwrap().push(request);
            let (delay, call) = (self.delays.get(n).copied().unwrap_or(0), n < self.calls);
            futures::stream::once(async move { tokio::time::sleep(Duration::from_millis(delay)).await })
                .flat_map(move |_| {
                    let events: Vec<anyhow::Result<LlmEvent>> = if call {
                        vec![Ok(LlmEvent::Turn(AssistantTurn {
                            tool_calls: vec![ToolCall {
                                id: format!("c{n}"),
                                name: "look".into(),
                                input: Default::default(),
                                error: None,
                            }],
                            ..AssistantTurn::default()
                        }))]
                    } else {
                        vec![
                            Ok(LlmEvent::Text("Готово.".into())),
                            Ok(LlmEvent::Turn(AssistantTurn {
                                content: "Готово.".into(), ..AssistantTurn::default()
                            })),
                        ]
                    };
                    futures::stream::iter(events)
                })
                .boxed()
        }
    }

    /// The rounds of a timed model under `by_time`: what was said, the waits sent, the requests made.
    async fn rounds(delays: &[u64], calls: usize, limits: Limits) -> (Vec<String>, Vec<What>, Vec<LlmRequest>) {
        let timed = Arc::new(Timed {
            delays: delays.to_vec(),
            calls,
            n: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
        });
        let dialog = crate::dialog::dialog_service::tests::with_client(timed.clone());
        let (sender, mut receiver) = futures::channel::mpsc::unbounded();
        let out = Out::new(sender, "t-1", None, CancellationToken::new());
        let decisions = Arc::new(Mutex::new(Vec::new()));
        let steer = steer(RoundsPolicy::ByTime, false, limits, Expected::default(), &out, decisions);
        let mut toolbox = ToolBox::new(
            vec![LlmTool {
                name: "look".into(),
                description: "Look.".into(),
                parameters: serde_json::json!({"type": "object"}),
            }],
            |_, _| async {
                tokio::time::sleep(Duration::from_millis(1500)).await;
                Ok(ToolOutcome::from("seen"))
            },
        );
        toolbox.rounds = Some(8);
        // Today's filler would be said here; by time it is not.
        toolbox.filler = Some(LOOKUP_FILLER);
        let said: Vec<String> = dialog
            .answer_steered("k", "что там", DialogContext::default(), CancellationToken::new(), Some(toolbox), steer)
            .filter_map(|phrase| async move { phrase.ok() })
            .collect()
            .await;
        drop(out);
        let mut events = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            events.push(event.what);
        }
        let requests = timed.requests.lock().unwrap().clone();
        (said, events, requests)
    }

    fn waits(events: &[What]) -> Vec<(WaitWhy, Option<u64>)> {
        events
            .iter()
            .filter_map(|what| match what {
                What::Wait { why, expect_ms, .. } => Some((*why, *expect_ms)),
                _ => None,
            })
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn a_late_round_gets_one_wait_never_two_and_no_filler_phrase() {
        // Round 1 calls `look` at 300 ms; the tool takes 1.5 s; round 2 would start at 1.8 s: late, so one wait.
        let (said, events, _) = rounds(&[300, 300, 300], 2, limits()).await;
        assert_eq!(said, ["Готово."], "no «Сейчас посмотрю.» of Thinking's own");
        let model: Vec<_> = waits(&events).into_iter().filter(|(why, _)| *why == WaitWhy::Model).collect();
        assert_eq!(model, [(WaitWhy::Model, Some(1000))], "one wait before the late round, with its expected time");
        assert!(waits(&events).iter().any(|(why, _)| *why == WaitWhy::Tool), "a slow tool is told: {events:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn by_time_a_slow_tool_s_filler_is_thinking_s_to_say_once_before_its_wait() {
        let (said, events, _) = rounds(&[300, 300, 300], 2, Limits { filler: true, ..limits() }).await;
        assert_eq!(said, ["Готово."], "the model's words: the filler is not the answer's");
        let filler = events.iter().position(|what| matches!(what, What::Say { text } if text == "Сейчас посмотрю."));
        let tool = events.iter().position(|what| matches!(what, What::Wait { why: WaitWhy::Tool, .. }));
        assert!(matches!((filler, tool), (Some(filler), Some(tool)) if filler < tool), "{events:?}");
        let fillers = events.iter().filter(|what| matches!(what, What::Say { .. })).count();
        assert_eq!(fillers, 1, "once a turn: {events:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_round_that_fits_goes_on_in_silence() {
        let quick = Limits { first_word_ms: 60_000, ..limits() };
        let (said, events, _) = rounds(&[100, 100], 1, quick).await;
        assert_eq!(said, ["Готово."]);
        assert!(waits(&events).iter().all(|(why, _)| *why != WaitWhy::Model), "{events:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn the_fourth_round_before_the_first_word_is_words_only_and_the_limit_ends_the_tools() {
        let (_, _, requests) = rounds(&[100; 8], 8, limits()).await;
        assert_eq!(requests.len(), 4, "the ceiling of three rounds with tools, then words");
        assert_eq!(requests[3].tool_choice, ToolChoice::None);
        assert!(requests[..3].iter().all(|request| request.tool_choice == ToolChoice::Auto));
        let short = Limits { limit_ms: 4_000, words_ms: 1_000, ceiling: 10, ..limits() };
        let (_, _, requests) = rounds(&[100; 8], 8, short).await;
        let last = requests.last().unwrap();
        assert_eq!(last.tool_choice, ToolChoice::None, "past the device's limit: words only");
        assert!(requests.len() < 8, "{}", requests.len());
    }

    #[test]
    fn a_policy_s_limits_take_the_phrase_s_budget_first() {
        let settings = Settings::default();
        let place = Where::of(crate::door::Kind::Station, true);
        let given = crate::door::Budget { voice: true, first_word_ms: Some(1200), limit_ms: Some(60_000) };
        let limits = Limits::of(&settings, place, &given);
        assert_eq!((limits.first_word_ms, limits.limit_ms, limits.ceiling), (1200, 60_000, 3));
        let none =
            Limits::of(&settings, Where::of(crate::door::Kind::Computer, false), &crate::door::Budget::default());
        assert_eq!((none.first_word_ms, none.limit_ms, none.ceiling), (1500, 25_000, 5));
    }
}
