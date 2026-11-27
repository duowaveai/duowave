//! The door's messages and its trait (docs/plans/audio-thinking-contract.md, «Messages»).
//!
//! - The audio side sends a `Phrase` (it opens a turn) and `News`: a turn cut, a question nobody answered, a hand done,
//!   what became of a notice.
//! - Thinking answers a phrase with `Event`s, in the order they are to be said, the last one `end`; outside a turn it
//!   sends `Outside` messages: a notice, its withdrawal, a hand.
//! - The audio side may ask what the home's devices are (`Devices`).
//!
//! The same messages go in-process (the `Thinking` trait) and over the passage as JSON (rule 4). Every message carries
//! the version `v`; each side ignores the messages and the fields it does not know (rule 8): an unknown kind reads as
//! `Unknown`, an unknown field is skipped.

pub mod hands;
#[cfg(any(test, feature = "testing"))]
pub mod script;

use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use hands::HandSpec;

/// The door's version: a removed field or a field that means something else makes a new one.
pub const V: u32 = 1;

fn v() -> u32 {
    V
}

/// A person said something to us: it opens a turn (the contract's «The phrase»).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Phrase {
    #[serde(default = "v")]
    pub v: u32,
    /// The turn: the audio side makes the id, and every event of the answer carries it.
    pub turn: String,
    pub text: String,
    /// The phrase's language, from recognition or the home's setting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    /// Other readings, when recognition gives them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alts: Vec<String>,
    #[serde(default)]
    pub who: Who,
    #[serde(default)]
    pub to: To,
    /// The device that heard.
    #[serde(rename = "where")]
    pub place: Place,
    /// The groups of hands this device has now, as its ear declares them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hands: Vec<String>,
    pub opened: Opened,
    #[serde(default)]
    pub context: Context,
    #[serde(default)]
    pub budget: Budget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heard: Option<Heard>,
    /// When it was said, Unix milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<u64>,
}

/// Who speaks, when it is known, and how sure.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Who {
    /// The person's id: the one signed in to the panel; later a voice match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person: Option<String>,
    /// Their name, for the journal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub sure: Sure,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sure {
    Known,
    Guess,
    #[default]
    #[serde(other)]
    Unknown,
}

/// Which agent the phrase is said to, as far as the audio side knows (the contract's «Who it is said to»). `agent`
/// None — nobody was named: Thinking picks the room's own, else the home's.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct To {
    /// `<package>:<character>`, or another agent's id.
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub by: By,
    /// The word or the button as people know it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// How the audio side knows whom a phrase is said to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum By {
    /// The activator word names its agent.
    Word,
    /// A button or a key bound to an agent.
    Button,
    /// The panel's talk with an agent the person chose.
    Chat,
    /// Said in the window after this agent spoke.
    Window,
    /// Nothing named anyone.
    #[default]
    #[serde(other)]
    Default,
}

/// The device that heard: its room and kind, and whether it has a screen and a speaker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    pub device: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,
    pub kind: Kind,
    #[serde(default)]
    pub screen: bool,
    #[serde(default)]
    pub speaker: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Station,
    Computer,
    Phone,
    Box,
    #[serde(other)]
    Other,
}

/// What opened the microphone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Opened {
    Word,
    Button,
    /// We listened after our own speech (the window).
    Window,
    /// Typed.
    Text,
    /// A tap on a screen's choice (reserved: `show`).
    Tap,
    #[serde(other)]
    Other,
}

/// Where in the talk the phrase stands.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Context {
    /// The window was opened for this question of ours: an `ask`, or a notice's question.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer_to: Option<String>,
    /// What the window opened after.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<After>,
    /// Said over our speech in this turn: the audio side has sent `stop` for it already.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub over: Option<String>,
    /// Start the talk over.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fresh: bool,
}

/// What the window opened after: Thinking's question, a work's line, a notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum After {
    Ask,
    Work,
    Notice,
    #[serde(other)]
    Other,
}

/// How long the device may stay silent, and whether the answer is heard.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Budget {
    /// The answer will be heard, not read.
    #[serde(default)]
    pub voice: bool,
    /// How long the device may stay silent before the person feels it; past it the audio side fills the silence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_word_ms: Option<u64>,
    /// How long the device waits for the next event before it gives up with `stop` (`timeout`, rule 7): Thinking may
    /// end the turn by itself before that.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit_ms: Option<u64>,
}

/// Where and how fast the phrase was recognised, for the journal's step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Heard {
    /// `home` or `cloud`.
    pub by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// One event of an answer: its turn, the agent speaking (`as`), and what it is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    #[serde(default = "v")]
    pub v: u32,
    pub turn: String,
    #[serde(rename = "as", default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    #[serde(flatten)]
    pub what: What,
}

impl Event {
    pub fn new(turn: &str, speaker: Option<&str>, what: What) -> Event {
        Event { v: V, turn: turn.to_string(), speaker: speaker.map(str::to_string), what }
    }
}

/// What Thinking says inside a turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum What {
    /// The journal's request of the turn and where Thinking thinks: `home` or `cloud`. It comes first, once.
    Begun { request: String, by: String },
    /// A piece of the answer, about one sentence.
    Say { text: String },
    /// The next `say` will be late.
    Wait {
        why: WaitWhy,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect_ms: Option<u64>,
        /// What is being waited for, for people: a tool's name.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    /// A question to the person: the audio side says `text` and opens the window; the reply comes as a phrase with
    /// `context.answer_to`. No `text` — the question was the turn's last `say`.
    Ask {
        question: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(default)]
        expects: Expects,
    },
    /// A device should act; the audio side replies `hands_done`.
    Hands {
        call: String,
        device: String,
        hand: String,
        #[serde(default)]
        args: Value,
    },
    /// The phrase started, queued or added to a long work. The spoken line is a separate `say`.
    Work {
        work: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        state: Option<String>,
    },
    /// Not ours: give the phrase to the device's own assistant. `capability`: whose the phrase is, for the device's
    /// journal (a skill's name); `text`: the words the assistant gets instead of the phrase (a quick rule's rewrite,
    /// as text — the voice says what the assistant misreads).
    Handoff {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        capability: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    /// The next phrase on this device goes on with the same skill for a while (a skill's own dialog). An empty
    /// `capability` with `ttl_ms` 0: what went on before is over.
    Follow { capability: String, ttl_ms: u64 },
    /// Thinking's part of the turn is over.
    End {
        outcome: Outcome,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<Failure>,
        /// Everything said in the turn, for the journal and the strip.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        said: String,
        #[serde(default)]
        ms: u64,
        /// What broke, for the journal and the device's own words of failure (`failed`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitWhy {
    Search,
    Tool,
    Model,
    Work,
    #[serde(other)]
    Other,
}

/// What a question expects.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Expects {
    #[default]
    Free,
    YesNo,
    Choice {
        options: Vec<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Answered,
    Asked,
    /// Heard in the window and not for us: nothing was kept, nothing said.
    Dropped,
    HandedOff,
    Failed,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Failure {
    NoModel,
    Limit,
    Error,
    #[serde(other)]
    Other,
}

/// What the audio side tells Thinking besides a phrase.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct News {
    #[serde(default = "v")]
    pub v: u32,
    #[serde(flatten)]
    pub what: Told,
}

impl News {
    pub fn new(what: Told) -> News {
        News { v: V, what }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Told {
    /// Our answer was cut.
    Stop {
        turn: String,
        why: StopWhy,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        heard: Option<Played>,
    },
    /// The window for a question closed without a reply.
    NoAnswer { question: String },
    /// The turn's answer was said to its end, at this time (Unix ms): what counts from the end of the speech starts.
    Said {
        turn: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at: Option<u64>,
    },
    /// A device did what `hands` asked, or could not.
    HandsDone {
        call: String,
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// What became of a notice (or a `show`).
    Delivered {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        notice: Option<String>,
        outcome: Delivery,
        device: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        heard: Option<Played>,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopWhy {
    /// The person said «стоп».
    Word,
    /// They spoke over us.
    Over,
    /// A new word or a new phrase.
    New,
    /// The device dropped off.
    Gone,
    /// Thinking went silent past the device's limit (rule 7).
    Timeout,
    #[serde(other)]
    Other,
}

/// How much of the answer was played, when the device tells.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Played {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chars: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Delivery {
    /// Said to its end.
    Said,
    /// Said, and cut.
    Cut,
    /// Not now: it waits for the next talk.
    Later,
    /// Not said, and it will not be.
    Dropped,
    /// Shown as a notification or a card instead of the voice.
    Shown,
    #[serde(other)]
    Other,
}

/// What Thinking says outside a turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Outside {
    #[serde(default = "v")]
    pub v: u32,
    #[serde(flatten)]
    pub what: Unasked,
}

impl Outside {
    pub fn new(what: Unasked) -> Outside {
        Outside { v: V, what }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Unasked {
    Notice(Notice),
    /// The notice is no longer needed: seen on the panel, or the work was stopped.
    Withdraw {
        notice: String,
    },
    /// A device should act outside a talk.
    Hands {
        call: String,
        device: String,
        hand: String,
        #[serde(default)]
        args: Value,
    },
    #[serde(other)]
    Unknown,
}

/// Something to tell a person unasked: a work is ready or asks something, a reminder, a schedule, an app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notice {
    pub notice: String,
    pub text: String,
    /// The agent telling it: its voice says it, and its window takes the reply.
    #[serde(rename = "as", default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    pub why: NoticeWhy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work: Option<String>,
    /// When the notice asks something: saying it opens the window for this question.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
    pub urgency: Urgency,
    /// Hints of whom and where: a person, a device, a room.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<Hint>,
    /// After this time (Unix milliseconds) the notice is no longer said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<u64>,
    /// What the notice's question expects, as an `ask`'s: free, yes or no, one of the options.
    #[serde(default)]
    pub expects: Expects,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeWhy {
    WorkReady,
    WorkQuestion,
    Reminder,
    Schedule,
    App,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Urgency {
    /// As soon as the device is free.
    Now,
    /// After the next talk on the device.
    NextTalk,
    Whenever,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hint {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,
}

/// One of the home's ears, as `devices()` tells it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Device {
    pub device: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,
    pub kind: Kind,
    #[serde(default)]
    pub screen: bool,
    #[serde(default)]
    pub speaker: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hands: Vec<HandSpec>,
    /// Someone talks with it, or it rings.
    #[serde(default)]
    pub busy: bool,
}

/// An answer's events, in the order they are to be said; the last one is `end`.
pub type Events = BoxStream<'static, Event>;
/// What Thinking says outside a turn.
pub type Notices = BoxStream<'static, Outside>;

/// Thinking, as the audio side sees it: one door (the contract's rule 1). Behind it may stand our talk layer, a live
/// model, an own agent — the audio side does not know which.
pub trait Thinking: Send + Sync {
    /// A phrase opens a turn. One turn per device at a time: the audio side stops the one running first (rule 6).
    fn phrase(&self, phrase: Phrase) -> Events;
    /// What became of a turn, a question, a hand or a notice.
    fn tell(&self, news: News);
    /// What Thinking says outside a turn, from now on: each call is a listener of its own.
    fn notices(&self) -> Notices;
}

/// What the home's ears are: the audio side answers it.
pub trait Devices: Send + Sync {
    fn devices(&self) -> Vec<Device>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_phrase_of_the_contract_reads_and_writes_back() {
        let written = json!({
            "v": 1,
            "turn": "t-1",
            "text": "сделай потише и скажи, что завтра с погодой",
            "lang": "ru",
            "who": { "sure": "unknown" },
            "to": { "agent": null, "by": "word", "name": "Алиса" },
            "where": { "device": "kitchen", "room": "kitchen", "kind": "station", "screen": false, "speaker": true },
            "hands": ["sound", "player", "timers"],
            "opened": "word",
            "context": {},
            "budget": { "voice": true, "first_word_ms": 1500 },
            "heard": { "by": "home", "ms": 420, "model": "gigaam" },
        });
        let phrase: Phrase = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(phrase.place.kind, Kind::Station);
        assert_eq!(phrase.to.by, By::Word);
        assert_eq!(phrase.budget.first_word_ms, Some(1500));
        assert_eq!(serde_json::to_value(&phrase).unwrap(), written);
    }

    #[test]
    fn an_event_carries_its_turn_and_who_speaks_and_its_kind_flat() {
        let event = Event::new("t-1", Some("butler:otto"), What::Say { text: "Поставил.".into() });
        assert_eq!(
            serde_json::to_value(&event).unwrap(),
            json!({"v": 1, "turn": "t-1", "as": "butler:otto", "type": "say", "text": "Поставил."})
        );
        let ask: Event =
            serde_json::from_value(json!({"turn": "t-2", "type": "ask", "question": "q-1", "expects": "yes_no"}))
                .unwrap();
        assert_eq!(ask.v, V, "a message without a version is the first one");
        assert_eq!(ask.what, What::Ask { question: "q-1".into(), text: None, expects: Expects::YesNo });
    }

    #[test]
    fn what_a_side_does_not_know_is_read_as_unknown_never_as_an_error() {
        let later: Event =
            serde_json::from_value(json!({"v": 2, "turn": "t-1", "type": "show", "card": {"title": "Рецепт"}}))
                .unwrap();
        assert_eq!(later.what, What::Unknown);
        let news: News = serde_json::from_value(json!({"type": "draft", "turn": "t-1", "text": "сделай"})).unwrap();
        assert_eq!(news.what, Told::Unknown);
        let phrase: Phrase = serde_json::from_value(json!({
            "turn": "t-3", "text": "да", "where": {"device": "computer", "kind": "glasses"}, "opened": "blink",
            "tone": "tired"
        }))
        .unwrap();
        assert_eq!((phrase.place.kind, phrase.opened), (Kind::Other, Opened::Other));
    }

    #[test]
    fn news_and_notices_have_the_contract_s_names() {
        let stop = News::new(Told::Stop {
            turn: "t-1".into(),
            why: StopWhy::Word,
            heard: Some(Played { chars: Some(120), ms: None }),
        });
        assert_eq!(
            serde_json::to_value(&stop).unwrap(),
            json!({"v": 1, "type": "stop", "turn": "t-1", "why": "word", "heard": {"chars": 120}})
        );
        let notice = Outside::new(Unasked::Notice(Notice {
            notice: "n-1".into(),
            text: "Курс валют: готово. На стол или примерить?".into(),
            speaker: None,
            why: NoticeWhy::WorkReady,
            work: Some("w-1".into()),
            question: Some("q-2".into()),
            urgency: Urgency::Now,
            to: None,
            until: None,
            expects: Expects::Choice { options: vec!["на стол".into(), "примерить".into()] },
        }));
        let json = serde_json::to_value(&notice).unwrap();
        assert_eq!(json["type"], "notice");
        assert_eq!(json["why"], "work_ready");
        assert_eq!(serde_json::from_value::<Outside>(json).unwrap(), notice);
    }
}
