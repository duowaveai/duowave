//! The layer's rules (docs/plans/audio-interface.md, «Правила работы и пакет языка»): what they catch never reaches
//! the model and costs no tokens. Their words are the language pack's; the frame knows none. Without a pack's word the
//! same way goes through the brain with the works' tools, in any language.
//!
//! | Rule | What it does |
//! |---|---|
//! | how it goes («как там виджет») | the line of the work's progress |
//! | «вдогонку» («и ещё…» with a thing in focus) | a note, or a new version of a finished thing |
//! | show («примерить») | «Примерка» in the panel |
//! | the desk («на стол») | the thing on the desk |
//! | details («подробнее») | the executor's last words |
//! | stop a work («отмени виджет») | stops the one named |
//! | the door («мастерская», «выйти из мастерской») | opens and closes it |
//! | start (the old template «сделай виджет: …») | a new work without the model |
//!
//! They know the device and its state: the router asks them with the request's input (`ExtraRules::matched_with`), and
//! they read the layer of that device. They come first among the extra rules, in place of the voice workshop's; the
//! open door comes before the built-in rules too, the station's controls apart (`ExtraRules::first`).

use std::sync::{Arc, LazyLock, Weak};

use futures::future::BoxFuture;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::layer::TalkLayer;
use super::origin::Origin;
use super::pack::{self, Pack};
use super::work_tools::Found;
use super::works::{Face, Focus};
use crate::memory::text::Lang;
use crate::skills::capabilities::{CapabilityName, Intent};
use crate::skills::quick_commands::QuickMatch;
use crate::skills::router::{ExtraRules, RouteInput, EXECUTOR};
use crate::skills::skill::{ExecutionContext, Outcome};
use crate::utils::time::now_ms;

/// The rule's name in the journal.
const RULE: &str = "talk:works";
/// The intent's action: what was heard rides in its `heard`.
pub const ACTION: &str = "talk";

/// What a rule heard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Heard {
    Status {
        thing: Option<String>,
    },
    Add {
        text: String,
        /// The thing named (the reader names it); None — the thing in focus.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thing: Option<String>,
    },
    Show {
        thing: Option<String>,
    },
    Desk {
        thing: Option<String>,
    },
    Details,
    Stop {
        thing: Option<String>,
    },
    DoorOpen,
    DoorClose,
    Start {
        face: Option<Face>,
        ask: String,
        /// Its short name, when the reader gave one («Курс доллара»); else the face's word until the agent names it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    /// A phrase through the open door: a request, or an addition to the thing in work or waiting.
    Door {
        text: String,
    },
    /// A work's line on its station (the station calls back for it, ready.rs): its question opens.
    Ready {
        work: String,
    },
    /// A scrap of speech or a word that says nothing to do («Ос»): asked again, not made into a work.
    Noise,
    /// The answer to the device's question about a work (ready.rs).
    Answer {
        work: String,
        question: super::asked::Kind,
        again: bool,
        answer: super::ready::Answer,
    },
    /// MagicPlace by voice (store.rs): open it, install, the updates, the new — and the answer to its question.
    Store {
        heard: super::store::StoreHeard,
    },
}

/// A pack's rules, compiled once.
struct Compiled {
    status: Vec<Regex>,
    add: Vec<Regex>,
    show: Vec<Regex>,
    desk: Vec<Regex>,
    details: Vec<Regex>,
    stop: Vec<Regex>,
    door_open: Vec<Regex>,
    door_close: Vec<Regex>,
    start: Vec<Regex>,
}

fn compile(patterns: &[&str]) -> Vec<Regex> {
    patterns.iter().map(|pattern| Regex::new(pattern).expect("a valid pattern of a pack")).collect()
}

impl Compiled {
    fn of(pack: &Pack) -> Compiled {
        let rules = &pack.work.rules;
        Compiled {
            status: compile(rules.status),
            add: compile(rules.add),
            show: compile(rules.show),
            desk: compile(rules.desk),
            details: compile(rules.details),
            stop: compile(rules.stop),
            door_open: compile(rules.door_open),
            door_close: compile(rules.door_close),
            start: compile(rules.start),
        }
    }
}

static RU: LazyLock<Compiled> = LazyLock::new(|| Compiled::of(pack::of(Lang::Ru)));
static EN: LazyLock<Compiled> = LazyLock::new(|| Compiled::of(pack::of(Lang::En)));

fn compiled(lang: Lang) -> &'static Compiled {
    match lang {
        Lang::En => &EN,
        Lang::Ru | Lang::Other => &RU,
    }
}

/// The words as the rules read them: a colon that only separates («сделай виджет: часы») gone, a time («в 10:30») keeps
/// its own.
fn words_of(normalized: &str) -> String {
    normalized.replace(": ", " ").trim().trim_end_matches(':').trim().to_string()
}

/// The first rule of a list that takes the whole phrase, and its named group, if any.
fn first<'a>(rules: &[Regex], text: &'a str, group: &str) -> Option<Option<&'a str>> {
    rules.iter().find_map(|rule| {
        let found = rule.captures(text)?;
        Some(found.name(group).map(|value| value.as_str().trim()).filter(|value| !value.is_empty()))
    })
}

/// The face a word of things names: «виджет» → a widget.
fn face_of(pack: &Pack, word: &str) -> Option<Face> {
    let index = pack.work.rules.things.iter().find(|(thing, _)| *thing == word).map(|(_, index)| *index)?;
    [Face::Widget, Face::Skill, Face::Game, Face::Theme].get(index).copied()
}

/// Whether «напиши <word> …» is about a thing and not a message to someone: the word is a thing's, or «мне»/«нам»
/// with a thing's word after it (src/skills/quick_commands.rs, the rule `message`). Every pack's words count.
pub fn names_a_thing(word: &str, rest: &str) -> bool {
    [pack::of(Lang::Ru), pack::of(Lang::En)].iter().any(|pack| {
        let rules = &pack.work.rules;
        let thing = |word: &str| rules.things.iter().any(|(thing, _)| *thing == word);
        thing(word) || (rules.for_us.contains(&word) && rest.split_whitespace().next().is_some_and(thing))
    })
}

/// The layer's rules over the layer.
pub struct TalkRules {
    layer: Weak<TalkLayer>,
}

impl TalkRules {
    pub fn new(layer: &Arc<TalkLayer>) -> Arc<TalkRules> {
        Arc::new(TalkRules { layer: Arc::downgrade(layer) })
    }

    /// Whether a thing a rule names is the layer's: a word of things (the thing in focus), or a work of the layer that
    /// is in play — by its id (the name only leads to it): being made, in the queue, waiting for an answer, talked
    /// about in the last half hour, or gone lately. An app on the desk no such work is about is not: «останови
    /// музыку», «поставь радио», «покажи новости» are the apps' own words, and the layer once took them (15.11). The
    /// rest the reader reads. None — the words are not about a thing of ours: the rule lets them go.
    fn ours(&self, layer: &TalkLayer, pack: &Pack, device: &str, thing: Option<&str>) -> Option<Option<String>> {
        let Some(thing) = thing else { return Some(None) };
        let words: Vec<&str> = thing.split_whitespace().collect();
        if words.iter().all(|word| face_of(pack, word).is_some() || pack.work.rules.for_us.contains(word)) {
            return Some(None);
        }
        let now = now_ms();
        let in_play = |work: &super::works::Work| {
            work.going()
                || work.state.queued()
                || work.awaiting()
                || now.saturating_sub(work.updated_at) < super::works::FOCUS_MS
        };
        let ours = match layer.find(device, Some(thing)) {
            Found::One(work) | Found::Gone(work) => in_play(&work),
            Found::Many(works) => works.iter().any(in_play),
            Found::Thing(..) | Found::None => false,
        };
        ours.then(|| Some(thing.to_string()))
    }

    /// What the layer's rules hear in a phrase (normalized) on a device; None — not the layer's.
    pub fn hear(&self, normalized: &str, device: &str) -> Option<Heard> {
        let layer = self.layer.upgrade()?;
        let lang = layer.lang(device);
        let pack = pack::of(lang);
        let rules = compiled(lang);
        let words = words_of(normalized);
        let text = words.as_str();
        // The exit before the entry: «выйди из мастерской» has the workshop's word too.
        let words = &pack.work.rules;
        if first(&rules.door_close, text, "").is_some() || words.door_close_any.matches(text) {
            return Some(Heard::DoorClose);
        }
        if first(&rules.door_open, text, "").is_some() || words.door_open_any.matches(text) {
            return Some(Heard::DoorOpen);
        }
        // MagicPlace before «покажи …» and «поставь …» of the works: those let go of what is not theirs, and «покажи
        // магазин», «поставь ленту» would end there. A thing of the layer in play named after «поставь» stays the
        // desk's.
        if let Some(store) = layer.store.hear(lang, text) {
            let desk = match &store {
                super::store::StoreHeard::Install { name, .. } => {
                    matches!(self.ours(&layer, pack, device, Some(name.as_str())), Some(Some(_)))
                }
                _ => false,
            };
            if !desk {
                return Some(Heard::Store { heard: store });
            }
        }
        if let Some(found) = rules.start.iter().find_map(|rule| rule.captures(text)) {
            let face = found.name("what").and_then(|what| face_of(pack, what.as_str()));
            let ask = found.name("ask").map(|ask| ask.as_str().trim().to_string()).unwrap_or_default();
            let title = super::work_tools::short_name(&ask);
            return Some(if ask.is_empty() { Heard::DoorOpen } else { Heard::Start { face, ask, title } });
        }
        let focus = layer.works.focus(device, now_ms());
        let in_focus = focus != Focus::None;
        if in_focus {
            if let Some(Some(added)) = first(&rules.add, text, "text") {
                return Some(Heard::Add { text: added.to_string(), thing: None });
            }
            if first(&rules.details, text, "").is_some() {
                return Some(Heard::Details);
            }
        }
        if let Some(thing) = first(&rules.show, text, "thing") {
            let thing = self.ours(&layer, pack, device, thing)?;
            return (in_focus || thing.is_some()).then_some(Heard::Show { thing });
        }
        if let Some(thing) = first(&rules.desk, text, "thing") {
            let thing = self.ours(&layer, pack, device, thing)?;
            return (in_focus || thing.is_some()).then_some(Heard::Desk { thing });
        }
        if let Some(thing) = first(&rules.stop, text, "thing") {
            return self.ours(&layer, pack, device, thing).map(|thing| Heard::Stop { thing });
        }
        if let Some(thing) = first(&rules.status, text, "thing") {
            // «Долго ещё?» without a thing is about the work going on.
            if thing.is_none() && !in_focus {
                return None;
            }
            return self.ours(&layer, pack, device, thing).map(|thing| Heard::Status { thing });
        }
        None
    }

    /// The open door takes the phrase, before the built-in rules: the layer's own words stay its commands (how it
    /// goes, the desk, the exit). With a reader (talk/reader.rs) that is all it takes: the rest goes on, and the reader
    /// tells a wish from an addition, a question, a scrap of speech or the exit in other words — taking everything as a
    /// wish made «Ос» and «выйди из режима мастерской» into works (live 14.11). Without one, anything else is a
    /// request or an addition, as before.
    pub fn through_door(&self, normalized: &str, device: &str) -> Option<Heard> {
        let layer = self.layer.upgrade()?;
        if !layer.door.is_open(device) {
            return None;
        }
        let words = words_of(normalized);
        let text = words.as_str();
        if text.is_empty() {
            return None;
        }
        layer.door.touch(device);
        // The layer's own words stay what they are inside the door — how it goes, try it on, the desk, the exit; the
        // rest is a wish or an addition, the template's words too.
        let busy = match layer.works.focus(device, now_ms()) {
            Focus::One(work) => work.going() || work.awaiting(),
            Focus::Many(_) => true,
            Focus::None => false,
        };
        let reads = layer.reader().is_some_and(|reader| reader.ready());
        match self.hear(text, device) {
            Some(Heard::Start { .. }) | None if reads => None,
            // With a thing in work, the template's words add to it too.
            Some(Heard::Start { .. }) if busy => Some(Heard::Door { text: text.to_string() }),
            None => Some(Heard::Door { text: text.to_string() }),
            heard => heard,
        }
    }

    /// A phrase said without the wake word to the device's question (asked.rs): the pack reads its answer; what it
    /// cannot read is not the question's.
    pub fn through_question(&self, normalized: &str, input: &RouteInput) -> Option<Heard> {
        if input.wake_word {
            return None;
        }
        self.answer_on(normalized, &input.device_tag)
    }

    /// The answer to the device's question, as the pack reads it; None when nothing is asked or it is not an answer.
    /// A work's question first; else MagicPlace's («Поставить «Ленту»?»).
    pub fn answer_on(&self, normalized: &str, device: &str) -> Option<Heard> {
        let layer = self.layer.upgrade()?;
        let pack = pack::of(layer.lang(device));
        let Some(question) = layer.asked.open(device) else {
            let heard = layer.store.answer_of(device, pack, &words_of(normalized))?;
            return Some(Heard::Store { heard });
        };
        let answer = super::ready::answer_of(pack, &words_of(normalized))?;
        Some(Heard::Answer { work: question.work, question: question.kind, again: question.again, answer })
    }

    /// Acts on what was heard: the phrase to say, and whether it asks (the station listens after it).
    pub async fn act(&self, heard: Heard, origin: Origin) -> Option<String> {
        let layer = self.layer.upgrade()?;
        let device = origin.device.clone();
        let pack = pack::of(layer.lang(&device));
        let said = match heard {
            Heard::Ready { work } => return layer.ready_said(&work, &device),
            Heard::Answer { work, question, again, answer } => {
                return Some(layer.answer(origin, &work, question, again, answer).await);
            }
            Heard::Store { heard } => {
                let (said, asks) = layer.store.act(heard, &device, layer.lang(&device)).await;
                // The store's question is the device's newest: a work's earlier one is not awaited any more.
                if asks {
                    layer.asked.answered(&device);
                }
                return Some(said);
            }
            Heard::Status { thing } => layer.act(&origin, "status", thing.as_deref()).await,
            Heard::Add { text, thing } => layer.add(origin, &text, thing.as_deref(), false).await,
            Heard::Noise => Ok(pack.work.say.noise.to_string()),
            Heard::Show { thing } => layer.act(&origin, "show", thing.as_deref()).await,
            Heard::Desk { thing } => layer.act(&origin, "desk", thing.as_deref()).await,
            Heard::Details => layer.act(&origin, "details", None).await,
            Heard::Stop { thing } => layer.act(&origin, "stop", thing.as_deref()).await,
            Heard::DoorOpen => {
                layer.door.open(&device);
                Ok(pack.work.say.door_open.to_string())
            }
            Heard::DoorClose => {
                layer.door.close(&device);
                Ok(pack.work.say.door_closed.to_string())
            }
            Heard::Start { face, ask, title } => layer.start(origin, &ask, face, title.as_deref(), false),
            Heard::Door { text } => {
                // Without a reader: the thing in work or waiting for an answer takes the words; else they are a new
                // thing's wish. Words that say nothing to do — a scrap of speech, a lone question word — are neither.
                let said = super::work_tools::stems(&text).len();
                if said == 0 || (said < 2 && text.trim_end().ends_with('?')) {
                    return Some(pack.work.say.noise.to_string());
                }
                let busy = match layer.works.focus(&device, now_ms()) {
                    Focus::One(work) => work.going() || work.awaiting(),
                    Focus::Many(_) => true,
                    Focus::None => false,
                };
                if busy {
                    layer.add(origin, &text, None, false).await
                } else {
                    let rules = compiled(layer.lang(&device));
                    let face = rules
                        .start
                        .iter()
                        .find_map(|rule| rule.captures(&text))
                        .and_then(|found| face_of(pack, found.name("what")?.as_str()));
                    layer.start(origin, &text, face, super::work_tools::short_name(&text).as_deref(), false)
                }
            }
        };
        Some(said.unwrap_or_else(|refusal| refusal.for_person(pack)))
    }

    /// The intent a rule's match is: the plugins' bridge serves it (plugins/skill.rs).
    pub(super) fn intent_of(heard: &Heard) -> Option<QuickMatch> {
        let mut intent = Intent::new(CapabilityName::Plugins, ACTION);
        intent.params.insert(EXECUTOR.into(), json!(CapabilityName::Plugins.as_str()));
        intent.params.insert("heard".into(), serde_json::to_value(heard).ok()?);
        Some(QuickMatch { intent, rule: RULE })
    }

    /// Whether the intent is the layer's.
    pub fn handles(&self, intent: &Intent) -> bool {
        intent.action == ACTION && intent.params.contains_key("heard")
    }

    /// The layer's intent on a station: its phrase, and the station listens after a question.
    pub fn execute<'a>(
        &'a self,
        intent: &'a Intent,
        context: &'a ExecutionContext,
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async move {
            let heard: Heard = serde_json::from_value(intent.params.get("heard").cloned().unwrap_or_default())?;
            let speaker = context.speaker.as_ref().map(|speaker| speaker.name.clone());
            let origin = Origin::station(&context.device_tag, true).by(speaker);
            let since = now_ms();
            let Some(said) = self.act(heard, origin).await else { return Ok(Outcome::default()) };
            let listen = said.trim_end().ends_with('?');
            // The history keeps the work's line, not the phrase.
            let kept = self.layer.upgrade().and_then(|layer| layer.fact_since(&context.device_tag, since));
            Ok(Outcome { speech: Some(said), listen, kept, ..Outcome::default() })
        })
    }
}

impl ExtraRules for TalkRules {
    fn matched(&self, _normalized: &str) -> Option<QuickMatch> {
        None
    }

    fn matched_with(&self, normalized: &str, input: &RouteInput) -> Option<QuickMatch> {
        let heard = self.hear(normalized, &input.device_tag)?;
        // «Яндекс Маркет», «Яндекс, открой маркет»: the router's text lost «Яндекс» as a wake word, the phrase did not —
        // Yandex's own store is not MagicPlace.
        if matches!(heard, Heard::Store { .. }) && super::store::names_yandex(&input.text) {
            return None;
        }
        TalkRules::intent_of(&heard)
    }

    /// Before the built-in rules: the answer to the device's question, then the open door.
    fn first(&self, normalized: &str, input: &RouteInput) -> Option<QuickMatch> {
        let heard =
            self.through_question(normalized, input).or_else(|| self.through_door(normalized, &input.device_tag))?;
        TalkRules::intent_of(&heard)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::events::EventBus;
    use crate::talk::works::{NewWork, APP};

    fn layer_with(title: &str) -> (Arc<TalkLayer>, Arc<TalkRules>) {
        let layer = TalkLayer::new(None, EventBus::new());
        let rules = TalkRules::new(&layer);
        if !title.is_empty() {
            layer.works.create(NewWork {
                kind: APP,
                face: Some(Face::Widget),
                executor: "workshop".into(),
                target: Some("golos-vidzhet-1".into()),
                title: title.into(),
                brief: "испанские слова дня".into(),
                origin: Origin::station("kitchen", true),
                again: false,
            });
        }
        (layer, rules)
    }

    /// The apps' own words stay theirs: an app on the desk no work in play is about is not the layer's thing —
    /// «останови музыку», «поставь радио», «покажи новости» go on to the apps (live 15.11). A thing made by voice
    /// and in play is, by the id its work keeps.
    #[test]
    fn an_app_on_the_desk_is_not_the_layers_unless_a_work_in_play_is_about_it() {
        use crate::talk::executor::{fake::Shelf, Presence};
        let layer = TalkLayer::new(None, EventBus::new());
        let shelf = Shelf::with(&[
            ("music", "Музыка", Presence::OnDesk),
            ("radio", "Радио", Presence::OnDesk),
            ("news", "Новости", Presence::OnDesk),
        ]);
        layer.add_executor(shelf.clone());
        let rules = TalkRules::new(&layer);
        let hear = |text: &str| rules.hear(text, "kitchen");
        assert_eq!(hear("останови музыку"), None);
        assert_eq!(hear("поставь радио"), None);
        assert_eq!(hear("покажи новости"), None);

        shelf.put("widget-1", "Испанский: слова дня", Presence::Draft);
        layer.works.create(NewWork {
            kind: APP,
            face: Some(Face::Widget),
            executor: "workshop".into(),
            target: Some("widget-1".into()),
            title: "Испанский: слова дня".into(),
            brief: "испанские слова дня".into(),
            origin: Origin::station("kitchen", true),
            again: false,
        });
        assert_eq!(hear("останови испанский"), Some(Heard::Stop { thing: Some("испанский".into()) }));
    }

    #[test]
    fn hears_the_works_words_of_the_device_and_lets_go_of_the_rest() {
        let (_layer, rules) = layer_with("Испанский: слова дня");
        let hear = |text: &str| rules.hear(text, "kitchen");
        assert_eq!(hear("как там виджет"), Some(Heard::Status { thing: None }));
        assert_eq!(hear("что с испанским"), Some(Heard::Status { thing: Some("испанским".into()) }));
        assert_eq!(hear("долго еще"), Some(Heard::Status { thing: None }));
        assert_eq!(hear("как там погода"), None, "not a thing of ours: the weather's");
        assert_eq!(
            hear("и еще кнопку чтобы слушать произношение"),
            Some(Heard::Add {
                text: "кнопку чтобы слушать произношение".into(), thing: None
            })
        );
        assert_eq!(hear("а также крупнее"), Some(Heard::Add { text: "крупнее".into(), thing: None }));
        assert_eq!(hear("примерить"), Some(Heard::Show { thing: None }));
        assert_eq!(hear("покажи погоду"), None);
        assert_eq!(hear("на стол"), Some(Heard::Desk { thing: None }));
        assert_eq!(hear("поставь испанский"), Some(Heard::Desk { thing: Some("испанский".into()) }));
        assert_eq!(hear("поставь будильник на семь"), None);
        assert_eq!(hear("подробнее"), Some(Heard::Details));
        assert_eq!(hear("отмени виджет"), Some(Heard::Stop { thing: None }));
        assert_eq!(hear("останови испанский"), Some(Heard::Stop { thing: Some("испанский".into()) }));
        assert_eq!(hear("отмени будильник"), None);
        assert_eq!(hear("мастерская"), Some(Heard::DoorOpen));
        assert_eq!(hear("выйти из мастерской"), Some(Heard::DoorClose));
        assert_eq!(
            hear("сделай виджет мои дела"),
            Some(Heard::Start {
                face: Some(Face::Widget), ask: "мои дела".into(), title: Some("Мои дела".into())
            })
        );
        assert_eq!(hear("сделай игру"), Some(Heard::DoorOpen), "nothing to make yet: the door asks what");
        assert_eq!(hear("какая погода"), None);
        // Another device has nothing in focus: «и ещё…» there is not an addition.
        assert_eq!(rules.hear("и еще кнопку", "hall"), None);
        assert_eq!(rules.hear("подробнее", "hall"), None);
        assert_eq!(rules.hear("как там виджет", "hall"), Some(Heard::Status { thing: None }), "a thing's word is ours");
    }

    #[test]
    fn hears_the_template_in_a_typed_phrase_after_a_thing_was_put_on_the_desk() {
        let (layer, rules) = layer_with("Курс доллара");
        layer.works.update("1", |work| work.state = crate::talk::works::WorkState::Done);
        let text = crate::skills::text::normalize("Сделай виджет: часы с секундами");
        assert_eq!(
            rules.hear(&text, "kitchen"),
            Some(Heard::Start {
                face: Some(Face::Widget),
                ask: "часы с секундами".into(),
                title: Some("Часы с секундами".into())
            }),
            "{text}"
        );
    }

    #[test]
    fn the_english_pack_has_its_own_words() {
        let layer = TalkLayer::new(None, EventBus::new());
        layer.set_langs(Arc::new(|_| Lang::En));
        let rules = TalkRules::new(&layer);
        assert_eq!(
            rules.hear("make a widget my tasks for today", "kitchen"),
            Some(Heard::Start {
                face: Some(Face::Widget),
                ask: "my tasks for today".into(),
                title: Some("My tasks for today".into())
            })
        );
        assert_eq!(rules.hear("open the workshop", "kitchen"), Some(Heard::DoorOpen));
        assert_eq!(rules.hear("how is the widget", "kitchen"), Some(Heard::Status { thing: None }));
        assert_eq!(rules.hear("сделай виджет мои дела", "kitchen"), None, "the device's language only");
    }

    #[test]
    fn a_message_to_someone_is_not_a_thing_and_a_thing_is_not_a_message() {
        assert!(names_a_thing("навык", "который будит"));
        assert!(names_a_thing("мне", "виджет с погодой"));
        assert!(!names_a_thing("маме", "что я задержусь"));
        assert!(!names_a_thing("мне", "что-нибудь"));
    }

    #[tokio::test]
    async fn the_open_door_takes_every_phrase_as_a_wish_or_an_addition_and_its_words_close_it() {
        let (layer, rules) = layer_with("");
        assert_eq!(rules.through_door("часы с секундами", "kitchen"), None, "the door is shut");
        let origin = || Origin::station("kitchen", true);
        let pack = pack::of(Lang::Ru);
        assert_eq!(rules.act(Heard::DoorOpen, origin()).await.as_deref(), Some(pack.work.say.door_open));
        assert!(layer.now("kitchen", Lang::Ru).contains("Мастерская открыта."));
        assert_eq!(
            rules.through_door("часы с секундами", "kitchen"),
            Some(Heard::Door { text: "часы с секундами".into() })
        );
        assert_eq!(rules.through_door("выйти из мастерской", "kitchen"), Some(Heard::DoorClose));
        assert_eq!(rules.through_door("часы", "hall"), None, "another device's door");
        assert_eq!(rules.act(Heard::DoorClose, origin()).await.as_deref(), Some(pack.work.say.door_closed));
        assert_eq!(rules.through_door("часы с секундами", "kitchen"), None);
    }
}

/// Step 4 of docs/plans/audio-interface.md on a whole gateway: the works' words of a station are the layer's without
/// the model (episode B), the open door takes a phrase without a template as a wish while «громче» stays the station's,
/// and «напиши навык…» is not a message.
#[cfg(test)]
mod gateway {
    use crate::config::{Config, Env};
    use crate::skills::capabilities::CapabilityName;
    use crate::skills::router::{Decision, ModelState, Resolution, RouteInput};
    use crate::skills::skill::{ExecutionContext, Outcome};
    use crate::utils::time::now_ms;

    fn env(dir: &std::path::Path) -> Env {
        let text = |sub: &str| dir.join(sub).to_string_lossy().into_owned();
        Env::from_pairs([
            ("STATE_DIR", String::new()),
            ("DIALOG_PROVIDER", "echo".into()),
            ("PACKAGES_DIR", text("packages")),
            ("DATA_DIR", text("data")),
            ("VOICE_LAB_DIR", text("voice-lab")),
            ("TTS_MODELS_DIR", text("voices")),
        ])
    }

    /// What the router decides for a phrase of the kitchen's station, and what the station says when it is ours.
    async fn say(
        app: &std::sync::Arc<crate::app::App>,
        text: &str,
    ) -> (Option<&'static str>, CapabilityName, Option<Outcome>) {
        let input = RouteInput {
            text: text.into(),
            device_tag: "kitchen".into(),
            wake_word: true,
            model: ModelState { enabled: true, tools: true },
            now: now_ms(),
            ..RouteInput::default()
        };
        match app.skills.router.decide(&input) {
            Decision::Intent { intent, resolution, rule, .. } => {
                let outcome = match (&resolution, rule) {
                    (Resolution::Own(skill), Some("talk:works")) => {
                        let context = ExecutionContext {
                            text: text.into(),
                            ..ExecutionContext::new("kitchen", chrono::Utc::now())
                        };
                        Some(skill.execute(&intent, &context).await.unwrap())
                    }
                    _ => None,
                };
                (rule, intent.capability, outcome)
            }
            other => panic!("not a rule's: {other:?}"),
        }
    }

    fn speech(outcome: &Option<Outcome>) -> String {
        outcome.as_ref().and_then(|outcome| outcome.speech.clone()).unwrap_or_default()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_works_words_and_the_door_are_the_layers_and_the_stations_controls_stay_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let app = crate::app::App::build(Config::for_tests(&env(dir.path()))).unwrap();

        // Nothing in work: the words of a thing are still the layer's, and it says so.
        let (rule, _, said) = say(&app, "Алиса, как там виджет?").await;
        assert_eq!((rule, speech(&said).as_str()), (Some("talk:works"), "Сейчас в работе ничего нет."));
        assert_eq!(say(&app, "Алиса, какая погода?").await.0.filter(|rule| *rule == "talk:works"), None);

        // The door: a phrase without a template becomes a wish.
        let (_, _, said) = say(&app, "Алиса, мастерская").await;
        assert_eq!(speech(&said), "Мастерская открыта. Что делаем?");
        assert!(said.as_ref().is_some_and(|outcome| outcome.listen), "the station listens for the answer");
        let (rule, capability, _) = say(&app, "Алиса, громче").await;
        assert_ne!(rule, Some("talk:works"), "the station's controls stay its own");
        assert_eq!(capability, CapabilityName::Volume);
        let (rule, _, said) = say(&app, "Алиса, часы с секундами").await;
        assert_eq!(rule, Some("talk:works"));
        assert_eq!(
            speech(&said),
            "Делаю: часы с секундами. Около минуты. Можно добавлять детали по ходу. Что-то ещё добавить?"
        );
        let work = app.talk.works().all(Some("kitchen"), None).remove(0);
        assert_eq!((work.brief.as_str(), work.device()), ("часы с секундами", "kitchen"));

        // Episode B: how it goes, from the work's progress, without the model.
        let (rule, _, said) = say(&app, "Алиса, как там виджет?").await;
        assert_eq!(rule, Some("talk:works"));
        assert!(speech(&said).starts_with("В работе "), "{}", speech(&said));
        let (_, _, said) = say(&app, "Алиса, выйти из мастерской").await;
        assert_eq!(speech(&said), "Мастерская закрыта.");
        assert!(!app.talk.now("kitchen", crate::memory::text::Lang::Ru).contains("Мастерская открыта."));

        // «Напиши навык…» is a thing to make, not a message to someone.
        let (rule, capability, _) = say(&app, "Алиса, напиши навык будильник для кота").await;
        assert_eq!((rule, capability), (Some("talk:works"), CapabilityName::Plugins));
        let (_, capability, _) = say(&app, "Алиса, напиши маме что я задержусь").await;
        assert_eq!(capability, CapabilityName::Calls);
    }
}
