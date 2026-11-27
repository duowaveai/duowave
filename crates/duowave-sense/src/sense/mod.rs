//! «Чутьё» (docs/plans/audio-thinking-contract.md, «Чутьё»): one service with one way to ask, used by both sides of
//! the door. A kind of question and its state go in; a pick among the options, with how sure, comes out.
//!
//! Each kind runs a cascade of steps: the words of the language pack first, then (stage 4) a small decision model at
//! home, then the big model through the router. A step that is not sure passes the question on. Today there is the
//! words step alone — the word lists (words.rs) and the thought's numbers (thought.rs), which were the talk layer's —, so
//! every answer is `by: words`. The model steps come from above: the gateway gives them, so «Чутьё» stands right above
//! the base floor and knows nothing of the brain.
//!
//! | Kind | Asked by | Question | Words step |
//! |---|---|---|---|
//! | `finished` | audio | is the thought over, or wait longer? | thought.rs |
//! | `window` | audio | a phrase in the window: `close` it, or a phrase for Thinking | words.rs, `window.close` |
//! | `for_us` | audio | is it ours, or the device's own assistant's (said with a station's word)? | the station's words |
//! | `answer`, `work`, `quick`, `groups`, `profile` | Thinking | — | none yet: Thinking's own plan |
//!
//! Every question goes to «Чутьё»'s journal (the log's target `sense`): the kind, the answer, the step, the time.

pub mod thought;
pub mod words;

use std::sync::LazyLock;
use std::time::Instant;

use serde::Serialize;

pub use thought::{End, Thought};
pub use words::Words;

/// A kind of question.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Finished,
    ForUs,
    Window,
    Answer,
    Work,
    Quick,
    Groups,
    Profile,
}

/// What the question is asked about: a piece's place in its thought, the words that belong to another assistant.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    /// The thought's first piece: a short word ends a thought only there.
    pub first: bool,
    /// The words of the home's stations (their «Алиса» and the words of their activator apps): a phrase that begins
    /// with one is theirs (`for_us`).
    pub others: Vec<String>,
    /// The state of Thinking's kinds (`answer`, `work`, `quick`, `groups`, `profile`): the question open, the works in
    /// focus, the groups, the hands of the place — as the steps that read it agree. The words step does not.
    pub data: serde_json::Value,
}

/// A question to «Чутьё».
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub kind: Kind,
    pub text: String,
    /// The phrase's language; None — the home's.
    pub lang: Option<String>,
    pub state: State,
    /// What may be picked; empty — the kind's own options.
    pub options: Vec<String>,
    pub deadline_ms: Option<u64>,
}

impl Question {
    pub fn new(kind: Kind, text: &str) -> Question {
        Question {
            kind,
            text: text.to_string(),
            lang: None,
            state: State::default(),
            options: Vec::new(),
            deadline_ms: None,
        }
    }
}

/// Which step answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum By {
    Words,
    Small,
    Model,
}

/// An answer: the pick (None — nobody was sure), how sure of each option, the step and the time it took.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Answer {
    pub pick: Option<String>,
    pub p: Vec<(String, f32)>,
    pub by: By,
    pub ms: u64,
}

/// One step of the cascade: it answers the kinds it knows, or passes (None).
pub trait Step: Send + Sync {
    fn by(&self) -> By;
    fn ask(&self, question: &Question) -> Option<Answer>;
}

/// The words step: the language pack's lists and the thought's numbers.
pub struct WordsStep;

impl WordsStep {
    fn words(lang: Option<&str>) -> &'static Words {
        match lang {
            Some(code) => words::of(code),
            None => words::household(),
        }
    }

    fn sure(pick: &str) -> Option<Answer> {
        Some(Answer { pick: Some(pick.to_string()), p: vec![(pick.to_string(), 1.0)], by: By::Words, ms: 0 })
    }
}

impl Step for WordsStep {
    fn by(&self) -> By {
        By::Words
    }

    fn ask(&self, question: &Question) -> Option<Answer> {
        let words = WordsStep::words(question.lang.as_deref());
        match question.kind {
            Kind::Finished => {
                let end = thought::judge(&question.text, words, question.state.first).end;
                WordsStep::sure(match end {
                    End::Short => "short",
                    End::Plain => "plain",
                    End::Hanging => "hanging",
                    End::Wait => "wait",
                })
            }
            Kind::Window => WordsStep::sure(if words.closes(&question.text) { "close" } else { "phrase" }),
            Kind::ForUs => {
                let heard = words.split(&question.text);
                let theirs = question.state.others.iter().any(|word| {
                    let word = words.split(word);
                    !word.is_empty() && heard.starts_with(&word)
                });
                WordsStep::sure(if theirs { "station" } else { "us" })
            }
            _ => None,
        }
    }
}

/// What a phrase in the window is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    /// A word that closes the window, the whole phrase: «хватит», «спасибо».
    Close,
    /// A phrase for Thinking.
    Phrase,
}

/// Whose a phrase heard without the button is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForUs {
    Us,
    /// Said with a station's word: the station takes it.
    Station,
}

/// «Чутьё»: its steps, asked in order.
pub struct Sense {
    steps: Vec<Box<dyn Step>>,
}

impl Sense {
    /// «Чутьё» with these steps, the first asked first.
    pub fn new(steps: Vec<Box<dyn Step>>) -> Sense {
        Sense { steps }
    }

    /// The cascade: the first step that is sure answers. Nobody sure — no pick, by the last step asked.
    pub fn ask(&self, question: &Question) -> Answer {
        let started = Instant::now();
        let mut answer = None;
        for step in &self.steps {
            if let Some(found) = step.ask(question) {
                answer = Some(found);
                break;
            }
        }
        let by = self.steps.last().map_or(By::Words, |step| step.by());
        let mut answer = answer.unwrap_or(Answer { pick: None, p: Vec::new(), by, ms: 0 });
        answer.ms = started.elapsed().as_millis() as u64;
        tracing::debug!(target: "sense", kind = ?question.kind, pick = ?answer.pick, by = ?answer.by, ms = answer.ms, "asked");
        answer
    }

    /// Kind `finished`: how the piece ends its thought, how long to wait, the text to add (thought.rs). The numbers
    /// are the words step's; a model step may only make the wait longer (stage 4).
    pub fn finished(&self, text: &str, lang: Option<&str>, first: bool) -> Thought {
        let mut question = Question::new(Kind::Finished, text);
        question.lang = lang.map(str::to_string);
        question.state.first = first;
        let _ = self.ask(&question);
        thought::judge(text, WordsStep::words(lang), first)
    }

    /// Kind `window`: whether a phrase heard in the window closes it.
    pub fn window(&self, text: &str, lang: Option<&str>) -> Window {
        let mut question = Question::new(Kind::Window, text);
        question.lang = lang.map(str::to_string);
        match self.ask(&question).pick.as_deref() {
            Some("close") => Window::Close,
            _ => Window::Phrase,
        }
    }

    /// Kind `for_us`: whether a phrase the computer heard by itself was said to a station (stage 3, К9).
    pub fn for_us(&self, text: &str, lang: Option<&str>, others: &[String]) -> ForUs {
        let mut question = Question::new(Kind::ForUs, text);
        question.lang = lang.map(str::to_string);
        question.state.others = others.to_vec();
        match self.ask(&question).pick.as_deref() {
            Some("station") => ForUs::Station,
            _ => ForUs::Us,
        }
    }
}

static HOME: LazyLock<Sense> = LazyLock::new(|| Sense::new(vec![Box::new(WordsStep)]));
static SET: std::sync::OnceLock<Sense> = std::sync::OnceLock::new();

/// The home's «Чутьё» as the gateway builds it, once at its start: its steps with the ones from above (Thinking's
/// kinds by the talk layer's rules; the model steps later). False — it was set already.
pub fn set_home(sense: Sense) -> bool {
    SET.set(sense).is_ok()
}

/// The home's «Чутьё»: as it was set, else its words step alone.
pub fn home() -> &'static Sense {
    SET.get().unwrap_or(&HOME)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_step_answers_the_audio_side_s_kinds_and_passes_the_rest() {
        let sense = home();
        let thought = sense.finished("Сделай виджет, чтобы", Some("ru"), true);
        assert_eq!((thought.end, thought.wait_ms), (End::Hanging, thought::HANGING_MS));
        assert_eq!(sense.window("Хватит.", Some("ru")), Window::Close);
        assert_eq!(sense.window("и ещё кнопку", Some("ru")), Window::Phrase);
        let answer = sense.ask(&Question::new(Kind::Groups, "что у меня завтра"));
        assert_eq!((answer.pick, answer.by), (None, By::Words), "Thinking's kinds have no step yet");
    }

    #[test]
    fn a_phrase_with_a_station_s_word_is_the_station_s() {
        let sense = home();
        let others = vec!["Алиса".to_string(), "Компьютер".to_string()];
        assert_eq!(sense.for_us("Алиса, включи музыку", Some("ru"), &others), ForUs::Station);
        assert_eq!(sense.for_us("компьютер который час", Some("ru"), &others), ForUs::Station);
        assert_eq!(sense.for_us("и ещё юань", Some("ru"), &others), ForUs::Us);
        assert_eq!(
            sense.for_us("скажи Алисе привет", Some("ru"), &others),
            ForUs::Us,
            "only a phrase that begins with it"
        );
        assert_eq!(sense.for_us("Алиса, включи музыку", Some("ru"), &[]), ForUs::Us, "no station — no word of its");
    }
}
