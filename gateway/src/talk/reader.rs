//! The reader (docs/plans/audio-interface.md, «Доводка после проверки руками 14.11», 1): a phrase about the works is
//! read into an act before any conversation, and the layer does and says it. The brain's model reads it — one call of
//! the tool `read`, nothing said — and the act is the rules' `Heard`. Asked in words, a model got «Готово: слова
//! теперь крупнее» out of «сделай в испанском слова покрупнее» without a call (live 14.11); read first, the same
//! phrase is a new version of the thing, «выйди из режима мастерской» is the exit and «Ос» a scrap of speech.
//!
//! A phrase is read when the device is in the works' context: the workshop's door is open, the layer asked a question,
//! a work goes on or was talked about in the last `READ_WINDOW_MS`, or the phrase has a word of things («виджет»,
//! «игру») or names a work of the device. A verb of making is no sign: «сделай так, чтобы музыка замолчала» and
//! «поставь таймер» are commands. Else — and when the model reads it as conversation — the brain answers as before,
//! with the works' tools. A model that answers the call in words gets its
//! JSON read all the same: some servers do not hold a model to a tool.

use std::sync::{Arc, Weak};

use serde::Deserialize;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::asked::Kind;
use super::layer::TalkLayer;
use super::origin::Origin;
use super::pack;
use super::ready::Answer;
use super::rules::{Heard, TalkRules};
use super::work_tools::{alike, stems};
use super::works::Face;
use crate::dialog::llm::{LlmTool, ToolCall};
use crate::dialog::{AnsweredBy, DialogContext, DialogService};
use crate::utils::time::now_ms;

/// A work talked about this long ago keeps the device's phrases read first.
pub const READ_WINDOW_MS: u64 = 10 * 60_000;
/// The reader's tool.
pub const READ: &str = "read";

/// What a phrase asks of the works.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Act {
    New,
    Add,
    Status,
    Show,
    Desk,
    Details,
    Stop,
    Open,
    Close,
    Answer,
    Noise,
    Talk,
}

/// A phrase as the model read it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Reading {
    pub act: Act,
    #[serde(default)]
    pub thing: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub face: Option<String>,
    #[serde(default)]
    pub answer: Option<String>,
}

pub fn read_tool() -> LlmTool {
    LlmTool {
        name: READ.into(),
        description: "Read the person's last phrase before anything is said: what it asks of the home's things being \
                      made — widgets for the desk, skills, games, themes; «Сейчас» lists them — or that it is plain \
                      conversation. Call it once and say nothing."
            .into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "act": {
                    "type": "string",
                    "enum": ["new", "add", "status", "show", "desk", "details", "stop", "open", "close", "answer", "noise", "talk"]
                },
                "thing": { "type": "string", "description": "The thing it is about, by its id as «Сейчас» shows it: a work's label ([р3]) or the id of an app that is on ([weather]); its name only when it has neither; leave it out for the thing talked about last." },
                "text": { "type": "string", "description": "new: the whole wish in the person's words, with what the conversation added; add: what to add or change." },
                "title": { "type": "string", "description": "new: the thing's short name, one to four words, a noun phrase («Курс доллара», «Часы с секундами») — never the request itself." },
                "face": { "type": "string", "enum": ["widget", "skill", "game", "theme"], "description": "new: what it becomes; a widget when the person did not say." },
                "answer": { "type": "string", "enum": ["yes", "fit", "no", "park", "details", "redo", "drop", "new", "edit", "first", "second", "third", "last"], "description": "answer: the person's answer to the question waiting; to «change it, or make a new one?» — edit or new; to «which one?» — first, second, third or last as they were named." }
            },
            "required": ["act"],
            "additionalProperties": false
        }),
    }
}

/// What the reader is told before it reads.
const INSTRUCTIONS: &str = "Read the person's last phrase first. Do not answer it in words: call `read` once.
- new — they want a new thing made: a widget for the desk, a skill, a game, a theme. `text`: the whole wish; `title`: \
its short name.
- add — they add to a thing or want it changed. Words for a thing being made go into that same work; a thing that is \
ready or already on the desk gets a new version. Size, colours, beauty, layout are changes of the thing too.
- status — how a thing goes, whether it is ready («как там виджет?», «делаешь?»). show — open it to try on. desk — put \
it on the desk. details — what was done. stop — stop its work: only when they ask in so many words to stop, cancel or not make it («останови», «отмени», «не делай»); a «нет» or «не надо» alone is never stop.
- open — open the workshop. close — leave the workshop, in any words («выйди из режима мастерской», «хватит \
мастерской»).
- answer — the answer to the question waiting for one (below), in `answer`.
- noise — a scrap of speech, a lone word or a sound that says nothing to do («Ос», «Виджут»).
- talk — anything else: a question about the world, chat, thanks, a command for the home not about the things being \
made (music, timers, the weather).
Never read words that only ask, thank or leave as `new`.";

/// What the model's choice reads — its words and its calls: the call of `read`, or the same JSON in its words; None —
/// nothing readable.
pub fn reading_of(text: &str, calls: &[ToolCall]) -> Option<Reading> {
    if let Some(call) = calls.iter().find(|call| call.name == READ) {
        return serde_json::from_value(serde_json::Value::Object(call.input.clone())).ok();
    }
    let (start, end) = (text.find('{')?, text.rfind('}')?);
    (start < end).then(|| serde_json::from_str(&text[start..=end]).ok()).flatten()
}

/// What the reader did: the layer's phrase, and whether it asks (the station listens after it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Read {
    pub said: String,
    pub listen: bool,
}

/// Reads the phrases about the works with the brain's model.
pub struct Reader {
    layer: Weak<TalkLayer>,
    dialog: Weak<DialogService>,
    rules: Arc<TalkRules>,
}

impl Reader {
    pub fn new(layer: &Arc<TalkLayer>, dialog: &Arc<DialogService>) -> Arc<Reader> {
        Arc::new(Reader { layer: Arc::downgrade(layer), dialog: Arc::downgrade(dialog), rules: TalkRules::new(layer) })
    }

    /// Whether a model can read: one is chosen and it calls tools.
    pub fn ready(&self) -> bool {
        self.dialog.upgrade().is_some_and(|dialog| dialog.enabled() && dialog.tools_enabled())
    }

    /// Whether a device's phrase is read before the talk.
    pub fn wants(&self, device: &str, text: &str) -> bool {
        self.ready() && self.layer.upgrade().is_some_and(|layer| layer.wants_reading(device, text))
    }

    /// Reads a phrase of the conversation `key` and acts on it: what the layer said — the history keeps the work's
    /// line — or None: the brain answers it (conversation, nothing readable, the model failed).
    pub async fn read(
        &self,
        key: &str,
        origin: Origin,
        text: &str,
        context: DialogContext,
        cancel: CancellationToken,
    ) -> Option<Read> {
        self.read_only(key, origin, text, context, cancel, None).await
    }

    /// The same, taking only the acts `only` names (the closed window, docs/plans/audio-interface-2.md, «К2»): a
    /// phrase read as another act — a new thing, conversation, the door — is not acted on and not remembered: None.
    pub async fn read_only(
        &self,
        key: &str,
        origin: Origin,
        text: &str,
        context: DialogContext,
        cancel: CancellationToken,
        only: Option<&[Act]>,
    ) -> Option<Read> {
        let (layer, dialog) = (self.layer.upgrade()?, self.dialog.upgrade()?);
        let device = origin.device.clone();
        let instructions = layer.reading_instructions(&device);
        let choice = match dialog.decide(key, text, context, cancel, vec![read_tool()], Some(instructions)).await {
            Ok(choice) => choice,
            Err(error) => {
                tracing::warn!(target: "talk", "the reader could not read a phrase on {device}: {error:#}");
                return None;
            }
        };
        let Some(reading) = reading_of(&choice.text, &choice.calls) else {
            tracing::info!(target: "talk", "the reader read nothing on {device}: the brain answers");
            return None;
        };
        tracing::info!(target: "talk", "read on {device}: {:?}", reading.act);
        if only.is_some_and(|only| !only.contains(&reading.act)) {
            tracing::info!(target: "talk", "the window on {device} does not take {:?}", reading.act);
            return None;
        }
        let heard = layer.heard_of(&device, text, reading)?;
        let since = now_ms();
        let said = self.rules.act(heard, origin).await?;
        // The history keeps the work's line, not the phrase (change 2).
        let kept = layer.fact_since(&device, since);
        dialog.remember(key, text, kept.as_deref().unwrap_or(&said), AnsweredBy::Skill, None);
        let listen = said.trim_end().ends_with('?');
        Some(Read { said, listen })
    }
}

impl TalkLayer {
    /// Whether a phrase on a device is about the works: read before the talk. The desk's apps by name do not count —
    /// «какая погода» is not about the app «Погода».
    pub fn wants_reading(&self, device: &str, text: &str) -> bool {
        if self.door.is_open(device) || self.asked.open(device).is_some() {
            return true;
        }
        let now = now_ms();
        let works = self.works.all(Some(device), None);
        if works.iter().any(|work| work.going() || now.saturating_sub(work.updated_at) < READ_WINDOW_MS) {
            return true;
        }
        let normalized = crate::skills::text::normalize(text);
        let rules = &pack::of(self.lang(device)).work.rules;
        if normalized.split_whitespace().any(|word| rules.things.iter().any(|(thing, _)| *thing == word)) {
            return true;
        }
        let wanted = stems(text);
        !wanted.is_empty() && works.iter().any(|work| alike(&stems(&work.title), &wanted))
    }

    /// The reader's instructions for a device: the acts, and what waits there — the open door, a question.
    fn reading_instructions(&self, device: &str) -> String {
        let mut text = INSTRUCTIONS.to_string();
        if self.door.is_open(device) {
            text.push_str("\nThe workshop is open: a wish that is not about the thing being made is a new thing.");
        }
        if let Some(question) = self.asked.open(device) {
            let asked = question.pending.and_then(|pending| pending.question).or_else(|| {
                self.works.get(&question.work).and_then(|work| work.question.map(|question| question.text))
            });
            match asked {
                Some(asked) => text.push_str(&format!("\nA question waits for its answer: «{asked}».")),
                None => text.push_str("\nA question about a work waits for its answer."),
            }
            if question.kind == Kind::Extra {
                text.push_str(
                    "\nIt asks whether to add anything to the thing being made: «нет», «не надо», «ничего», «так \
                     оставь» are `answer` no — the work goes on; something to add is add; stop only in words of \
                     stopping.",
                );
            }
        }
        text
    }

    /// What the layer hears in a reading: None — conversation, or an answer when nothing is asked.
    pub(super) fn heard_of(&self, device: &str, phrase: &str, reading: Reading) -> Option<Heard> {
        let filled =
            |value: Option<String>| value.map(|value| value.trim().to_string()).filter(|value| !value.is_empty());
        let thing = filled(reading.thing);
        let text = filled(reading.text).unwrap_or_else(|| phrase.trim().to_string());
        // To «Что-то ещё добавить?» a model's «stop» stands only on words of stopping: «нет, не надо» is the answer
        // «nothing to add» (owner 20.11).
        if reading.act == Act::Stop {
            if let Some(question) = self.asked.open(device).filter(|question| question.kind == Kind::Extra) {
                let normalized = crate::skills::text::normalize(phrase);
                let answer = super::ready::extra_answer(pack::of(self.lang(device)), &normalized)?;
                if answer != Answer::Drop {
                    return Some(Heard::Answer { work: question.work, question: Kind::Extra, again: false, answer });
                }
            }
        }
        Some(match reading.act {
            Act::New => Heard::Start {
                face: filled(reading.face).as_deref().and_then(Face::parse),
                ask: text,
                title: filled(reading.title),
            },
            Act::Add => Heard::Add { text, thing },
            Act::Status => Heard::Status { thing },
            Act::Show => Heard::Show { thing },
            Act::Desk => Heard::Desk { thing },
            Act::Details => Heard::Details,
            Act::Stop => Heard::Stop { thing },
            Act::Open => Heard::DoorOpen,
            Act::Close => Heard::DoorClose,
            Act::Noise => Heard::Noise,
            Act::Answer => {
                let question = self.asked.open(device)?;
                let answer = match filled(reading.answer)?.as_str() {
                    "yes" => Answer::Yes,
                    "fit" => Answer::Fit,
                    "no" => Answer::No,
                    "park" => Answer::Park,
                    "details" => Answer::Details,
                    "drop" => Answer::Drop,
                    "new" => Answer::New,
                    "edit" => Answer::Edit,
                    "first" => Answer::Choice(0),
                    "second" => Answer::Choice(1),
                    "third" => Answer::Choice(2),
                    "last" => Answer::Choice(usize::MAX),
                    "redo" => Answer::Redo { text },
                    _ => return None,
                };
                Heard::Answer { work: question.work, question: question.kind, again: question.again, answer }
            }
            Act::Talk => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::events::EventBus;
    use crate::talk::asked::Kind;
    use crate::talk::origin::{Activator, COMPUTER};
    use crate::talk::works::{NewWork, APP};
    use serde_json::{Map, Value};

    fn choice(text: &str, call: Option<Value>) -> Option<Reading> {
        let calls: Vec<ToolCall> = call
            .map(|input| ToolCall {
                id: "r1".into(),
                name: READ.into(),
                input: input.as_object().cloned().unwrap_or_else(Map::new),
                error: None,
            })
            .into_iter()
            .collect();
        reading_of(text, &calls)
    }

    #[test]
    fn reads_the_call_or_the_same_json_in_words_and_nothing_else() {
        assert_eq!(
            choice("", Some(json!({ "act": "add", "thing": "р1", "text": "слова покрупнее" }))),
            Some(Reading {
                act: Act::Add,
                thing: Some("р1".into()),
                text: Some("слова покрупнее".into()),
                title: None,
                face: None,
                answer: None,
            })
        );
        assert_eq!(choice("Вот: {\"act\": \"close\"}", None).map(|reading| reading.act), Some(Act::Close));
        assert_eq!(choice("Готово: слова теперь крупнее.", None), None, "words are no reading");
        assert_eq!(choice("", Some(json!({ "act": "dance" }))), None, "an act it does not know");
    }

    fn work(layer: &TalkLayer, title: &str) -> String {
        layer
            .works
            .create(NewWork {
                kind: APP,
                face: Some(Face::Widget),
                executor: "workshop".into(),
                target: Some("widget-1".into()),
                title: title.into(),
                brief: "испанские слова дня".into(),
                origin: Origin::computer(Activator::Button),
                again: false,
            })
            .id
    }

    /// The phrases of 14.11 that went wrong: each is read when the device is in the works' context, and only then.
    #[test]
    fn a_phrase_is_read_in_the_works_context_and_conversation_is_not() {
        let layer = TalkLayer::new(None, EventBus::new());
        assert!(!layer.wants_reading(COMPUTER, "Который час?"), "nothing goes on: the brain answers");
        assert!(!layer.wants_reading(COMPUTER, "Какая погода завтра?"));
        assert!(layer.wants_reading(COMPUTER, "Виджет с курсом доллара."), "a word of things");
        assert!(layer.wants_reading(COMPUTER, "Можешь поправить плагин и сделать его более красивым?"));
        assert!(!layer.wants_reading(COMPUTER, "Сделай так, чтобы музыка замолчала."), "a command, not a thing");
        assert!(!layer.wants_reading(COMPUTER, "Поставь таймер."));
        layer.door.open(COMPUTER);
        assert!(layer.wants_reading(COMPUTER, "Ос."), "the open door: every phrase");
        layer.door.close(COMPUTER);
        work(&layer, "Испанский: слова дня");
        assert!(layer.wants_reading(COMPUTER, "Они что-то крупнее не стали."), "a work goes on");
        assert!(!layer.wants_reading("kitchen", "Они что-то крупнее не стали."), "another device's work");
    }

    #[test]
    fn a_reading_is_what_the_rules_hear_and_conversation_is_none() {
        let layer = TalkLayer::new(None, EventBus::new());
        let read = |act: Act| Reading { act, thing: None, text: None, title: None, face: None, answer: None };
        let new = Reading {
            text: Some("виджет с курсом доллара к рублю".into()),
            title: Some("Курс доллара".into()),
            ..read(Act::New)
        };
        assert_eq!(
            layer.heard_of(COMPUTER, "Виджет с курсом доллара.", new),
            Some(Heard::Start {
                face: None,
                ask: "виджет с курсом доллара к рублю".into(),
                title: Some("Курс доллара".into())
            })
        );
        let add =
            Reading { thing: Some("р1".into()), text: Some("слова покрупнее".into()), ..read(Act::Add) };
        assert_eq!(
            layer.heard_of(COMPUTER, "Сделай в испанском слова покрупнее", add),
            Some(Heard::Add { text: "слова покрупнее".into(), thing: Some("р1".into()) })
        );
        assert_eq!(layer.heard_of(COMPUTER, "Выйди из режима мастерской", read(Act::Close)), Some(Heard::DoorClose));
        assert_eq!(layer.heard_of(COMPUTER, "Ос", read(Act::Noise)), Some(Heard::Noise));
        assert_eq!(layer.heard_of(COMPUTER, "Спасибо", read(Act::Talk)), None);
        let yes = Reading { answer: Some("yes".into()), ..read(Act::Answer) };
        assert_eq!(layer.heard_of(COMPUTER, "Давай", yes.clone()), None, "nothing asked: not an answer");
        let id = work(&layer, "Часы");
        layer.asked.ask(COMPUTER, &id, Kind::Ready, false);
        assert_eq!(
            layer.heard_of(COMPUTER, "Давай", yes),
            Some(Heard::Answer { work: id, question: Kind::Ready, again: false, answer: Answer::Yes })
        );
    }
}
