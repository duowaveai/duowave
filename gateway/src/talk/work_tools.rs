//! The works' tools for the brain (docs/plans/audio-interface.md, «Три инструмента и готовая фраза»), given as a
//! connector gives its tools — so they reach the station's answers and the panel's talk alike, with the speaker
//! known and the right to use the apps (`use:plugins`):
//!
//! - `work_start {brief, face?, title?, new?}` — begins a new thing from the whole wish of the conversation;
//! - `work_add {text, thing?, copy?}` — adds to the work going on («вдогонку»: the agent takes it in between its
//!   steps, nothing starts over), or begins a new version of a finished thing, of an app on the desk too;
//! - `work_do {action, thing?}` — how it goes, what was done, try it on, the desk, the drafts, stop.
//!
//! A tool's answer is a ready phrase («Делаю: испанские слова дня. Около минуты. Можно добавлять.»): said as it is,
//! the answer ends there — no second round of the model to retell it. A question which thing and a refusal are the
//! layer's phrases too: a model that retold «not now» once said «Делаю…» instead (live 14.11). The history keeps
//! the work's line in place of the phrase (`ToolContribution::kept`). `thing` is a label of the block «Сейчас»
//! (`[р3]`) or a name; without it — the thing in focus of the device.

use std::collections::HashSet;
use std::sync::{Arc, Weak};

use futures::future::BoxFuture;
use futures::FutureExt;
use serde_json::{json, Map, Value};

use super::asked::{Kind, Pending};
use super::executor::{Closed, Doing, Started};
use super::layer::TalkLayer;
use super::origin::{Activator, Origin};
use super::pack::{self, fill};
use super::works::{End, Face, Focus, NewWork, Refused, Work, WorkState, APP, SIMILAR_MS};
use super::workshop::WORKSHOP;
use crate::core::connectors::{
    Connector, ConnectorKind, ConnectorManifest, ConnectorStatus, ToolContext, ToolContribution,
};
use crate::core::identity::permissions::use_of;
use crate::dialog::llm::LlmTool;
use crate::skills::capabilities::CapabilityName;
use crate::utils::time::now_ms;

/// Why a work's action was not done: a ready phrase of the pack, said as it is — by the rules, the reader and the
/// tools alike.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    NoBrief,
    NoText,
    /// Ten new works by voice today. Two at once and a new one by voice once in two minutes are no refusal: the work
    /// waits in the queue and starts by itself.
    Enough,
    /// A thing alike of the last half hour: change it, or make a new one? Its title.
    Similar {
        title: String,
    },
    /// More than one thing fits: their titles and labels.
    Which(Vec<(String, String)>),
    /// The thing named is gone (removed from the desk and the drafts): make it again? Its title.
    Gone {
        title: String,
    },
    /// No thing in work or talked about lately.
    NoThing,
    /// A built-in app: not changed; one's own copy may be made. Its title.
    BuiltIn(String),
    Failed(String),
    UnknownAction,
}

impl Refusal {
    /// What the person hears.
    pub fn for_person(&self, pack: &pack::Pack) -> String {
        let say = &pack.work.say;
        match self {
            Refusal::NoBrief | Refusal::NoText | Refusal::UnknownAction => say.what.to_string(),
            Refusal::Enough => say.enough.to_string(),
            Refusal::Similar { title, .. } => fill(say.similar, &[("title", title)]),
            Refusal::Which(things) => {
                let list: Vec<String> =
                    things.iter().map(|(title, _)| fill(pack.work.ask, &[("text", title)])).collect();
                fill(say.which, &[("list", &list.join(say.or))])
            }
            Refusal::NoThing => say.nothing.to_string(),
            Refusal::BuiltIn(title) => fill(say.built_in, &[("title", title)]),
            Refusal::Gone { title } => fill(say.gone, &[("title", title)]),
            Refusal::Failed(why) => fill(say.failed, &[("why", why)]),
        }
    }
}

/// The words of a title or a wish that tell one thing from another: their stems, without the faces' words.
pub(super) fn stems(text: &str) -> HashSet<String> {
    let faces: HashSet<String> = [pack::of(crate::memory::text::Lang::Ru), pack::of(crate::memory::text::Lang::En)]
        .iter()
        .flat_map(|pack| pack.work.faces.iter())
        .map(|face| stem(face))
        .collect();
    crate::memory::text::normalize(text)
        .split_whitespace()
        .filter(|word| word.chars().count() >= 4)
        .map(stem)
        .filter(|stem| !faces.contains(stem))
        .collect()
}

fn stem(word: &str) -> String {
    word.chars().take(5).collect()
}

/// Whether two texts are about one thing: half of the shorter one's stems at least are the other's.
pub(super) fn alike(a: &HashSet<String>, b: &HashSet<String>) -> bool {
    let shared = a.intersection(b).count();
    shared > 0 && shared * 2 >= a.len().min(b.len())
}

/// Whether a title is a new thing's name until its maker names it («Новый виджет»): no thing is alike by it.
pub(super) fn unnamed(title: &str) -> bool {
    [pack::of(crate::memory::text::Lang::Ru), pack::of(crate::memory::text::Lang::En)]
        .iter()
        .any(|pack| pack.work.unnamed.contains(&title))
}

/// A short wish is a name too: «сделай виджет: часы с секундами» — «Часы с секундами»; a long one is not.
pub(super) fn short_name(ask: &str) -> Option<String> {
    let ask = ask.trim().trim_end_matches(['.', '!', '?']);
    if ask.is_empty() || ask.split_whitespace().count() > 4 {
        return None;
    }
    let mut chars = ask.chars();
    let first = chars.next()?;
    Some(first.to_uppercase().chain(chars).collect())
}

/// The first letter small, unless the word is all capitals («MIDI»): «Делаю: испанские слова дня».
pub(super) fn in_sentence(title: &str) -> String {
    let mut chars = title.trim().chars();
    let Some(first) = chars.next() else { return String::new() };
    let rest: String = chars.collect();
    if rest.chars().next().is_some_and(char::is_uppercase) {
        return title.trim().to_string();
    }
    first.to_lowercase().chain(rest.chars()).collect()
}

/// Which work a `thing` names on a device — or an app of an executor's that no work names. An answer of one lookup,
/// never kept: its size does not matter.
#[allow(clippy::large_enum_variant)]
pub(super) enum Found {
    One(Work),
    Many(Vec<Work>),
    Thing(&'static str, Started),
    /// Named, but its thing is gone: removed from the desk and the drafts (by its id, TalkLayer::recheck).
    Gone(Work),
    None,
}

impl TalkLayer {
    /// The device of a tool's call, and where its phrase came from.
    fn origin_of(context: &ToolContext) -> Origin {
        let mut origin = context.origin.clone().unwrap_or_else(|| match &context.station {
            Some(station) => Origin::station(&station.device, true),
            None => Origin::computer(Activator::Button),
        });
        if origin.who.is_none() {
            origin.who = Some(context.principal.name.clone()).filter(|name| !name.is_empty());
        }
        origin
    }

    /// `work_start`: a new thing from the whole wish. The limits first (two at once, by voice once in two minutes and
    /// ten a day), then a thing alike of the last half hour is the layer's question, not a new thing: «Поправить … или
    /// новую?» keeps the wish until the answer (ready.rs). A thing without a name is «Новый виджет» until its maker
    /// names it — never the request itself.
    pub fn start(
        &self,
        origin: Origin,
        brief: &str,
        face: Option<Face>,
        title: Option<&str>,
        new: bool,
    ) -> Result<String, Refusal> {
        let brief = brief.trim();
        if brief.is_empty() {
            return Err(Refusal::NoBrief);
        }
        let title = title.map(str::trim).filter(|title| !title.is_empty());
        let face = face.unwrap_or(Face::Widget);
        let now = now_ms();
        let lang = self.lang(&origin.device);
        if !new {
            // Alike by the names of things, not by the words of the wishes: «сделай» and «красивый» once made a
            // dollar's rate «like» the clock (live 14.11).
            let wanted: HashSet<String> = stems(title.unwrap_or(brief));
            let alike = self
                .works
                .all(Some(&origin.device), None)
                .into_iter()
                .filter(|work| now.saturating_sub(work.updated_at) < SIMILAR_MS && !unnamed(&work.title))
                .find(|work| alike(&stems(&work.title), &wanted));
            if let Some(work) = alike {
                let wish = Pending {
                    text: brief.to_string(),
                    title: title.map(str::to_string),
                    face: Some(face),
                    question: Some(fill(pack::of(lang).work.say.similar, &[("title", &work.title)])),
                    ..Pending::default()
                };
                self.asked.ask_with(&origin.device, &work.id, Kind::Similar, wish);
                return Err(Refusal::Similar { title: work.title });
            }
        }
        // The limits: ten a day is a refusal; two at once and a new one by voice once in two minutes are the queue —
        // a wish is never lost, and no model retells «not now» as «Делаю…» (live 14.11).
        let wait = match self.works.may_start(&origin, false, now) {
            Ok(()) => None,
            Err(Refused::Enough) => return Err(Refusal::Enough),
            Err(Refused::Busy) => Some(None),
            Err(Refused::TooSoon(wait)) => Some(Some(wait)),
        };
        self.executor(WORKSHOP).ok_or_else(|| Refusal::Failed("no workshop here".into()))?;
        let work = self.works.create(NewWork {
            kind: APP,
            face: Some(face),
            executor: WORKSHOP.into(),
            target: None,
            title: title.map(str::to_string).unwrap_or_else(|| pack::of(lang).work.unnamed[face.index()].to_string()),
            brief: brief.to_string(),
            origin,
            again: false,
        });
        let say = &pack::of(lang).work.say;
        if let Some(wait) = wait {
            self.enqueue(&work, wait);
            let title = in_sentence(&work.title);
            return Ok(match wait {
                Some(wait) => fill(say.queued_in, &[("in", &say.duration(wait)), ("title", &title)]),
                None => fill(say.queued_next, &[("title", &title)]),
            });
        }
        let work = self.launch(work)?;
        Ok(fill(self.started_words(work.device()), &[("title", &in_sentence(&work.title))]))
    }

    /// The work a `thing` names on a device: a label of the block («р3», «[w3]», «3»), a title, or — named not at
    /// all — the thing in focus.
    pub(super) fn find(&self, device: &str, thing: Option<&str>) -> Found {
        let thing = thing.map(str::trim).filter(|thing| !thing.is_empty());
        let Some(thing) = thing else {
            return match self.works.focus(device, now_ms()) {
                Focus::One(work) => Found::One(work),
                Focus::Many(works) => Found::Many(works),
                Focus::None => Found::None,
            };
        };
        let label = thing.trim_start_matches('[').trim_end_matches(']');
        let digits = label.trim_start_matches(|c: char| c.is_alphabetic());
        if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
            if let Some(work) = self.works.get(digits) {
                return if work.removed() { Found::Gone(work) } else { Found::One(work) };
            }
        }
        let wanted = stems(thing);
        let mut matched: Vec<Work> = Vec::new();
        let mut gone: Option<Work> = None;
        for work in self.works.all(Some(device), None) {
            let same =
                |known: &Work| known.target.is_some() && known.target == work.target && known.executor == work.executor;
            if !alike(&stems(&work.title), &wanted) {
                continue;
            }
            if work.removed() {
                gone.get_or_insert(work);
            } else if !matched.iter().any(same) {
                matched.push(work);
            }
        }
        match matched.len() {
            0 => {}
            1 => return Found::One(matched.remove(0)),
            _ => {
                // The thing being made now is the one meant: «добавь кнопку» named the new «Испанский» while
                // yesterday's namesake stood on the desk, and «Какую: «Испанский: слова дня» или «Испанский: слова
                // дня»?» could not be answered (live 15.11).
                let going: Vec<usize> = (0..matched.len()).filter(|&at| matched[at].going()).collect();
                if let [at] = going.as_slice() {
                    return Found::One(matched.remove(*at));
                }
                // Namesakes: the one talked about last.
                if matched.iter().all(|work| work.title == matched[0].title) {
                    matched.sort_by_key(|work| std::cmp::Reverse(work.updated_at));
                    return Found::One(matched.remove(0));
                }
                return Found::Many(matched);
            }
        }
        // An app on the desk or a draft no work of this device names — by its id or its name; else a thing that was
        // there and is gone.
        match self.executor(WORKSHOP).and_then(|executor| executor.thing(thing)) {
            Some(started) => match self.works.of_target(WORKSHOP, &started.target).filter(|work| !work.removed()) {
                Some(work) => Found::One(work),
                None => Found::Thing(WORKSHOP, started),
            },
            None => gone.map_or(Found::None, Found::Gone),
        }
    }

    /// `work_add`: words for the thing in work go to that work («вдогонку»); for a thing that is done — an app on the
    /// desk too — they begin its new version. A built-in app is not changed: the person is asked about a copy.
    pub async fn add(&self, origin: Origin, text: &str, thing: Option<&str>, copy: bool) -> Result<String, Refusal> {
        let text = text.trim();
        if text.is_empty() {
            return Err(Refusal::NoText);
        }
        let device = origin.device.clone();
        let say = &pack::of(self.lang(&device)).work.say;
        let (executor_name, target, title) = match self.find(&device, thing) {
            Found::Many(works) => return Err(self.which(&device, &works, "add", Some(text))),
            Found::Gone(work) => return Err(self.gone(&device, &work)),
            Found::None => return Err(Refusal::NoThing),
            Found::One(work) => {
                let executor =
                    self.executor(&work.executor).ok_or_else(|| Refusal::Failed("its executor is gone".into()))?;
                if work.going() && executor.add(&work, text) {
                    let note = super::works::note_of(text, origin.clone(), now_ms());
                    if let Some(work) = self.works.update(&work.id, |work| work.notes.push(note)) {
                        self.told(&work, "note");
                    }
                    return Ok(fill(say.noted, &[("text", &in_sentence(text))]));
                }
                let target = work.target.clone().ok_or_else(|| Refusal::Failed("there is no thing yet".into()))?;
                (executor.name(), target, work.title.clone())
            }
            Found::Thing(executor, started) => (executor, started.target, started.title),
        };
        let executor = self.executor(executor_name).ok_or_else(|| Refusal::Failed("its executor is gone".into()))?;
        let opened = match executor.open(&target, copy).await {
            Ok(opened) => opened,
            Err(Closed::BuiltIn(title)) => {
                // «Сделать свою копию?» keeps the change until the answer (ready.rs).
                let change = Pending { text: text.to_string(), thing: thing.map(str::to_string), ..Pending::default() };
                self.asked.ask_with(&device, "", Kind::Copy, change);
                return Err(Refusal::BuiltIn(title));
            }
            Err(Closed::Failed(why)) => return Err(Refusal::Failed(why)),
        };
        // Two at once: the new version waits in the queue (a version is never held by the two minutes).
        let busy = self.works.may_start(&origin, true, now_ms()).is_err();
        let face = self.works.of_target(executor_name, &opened.target).and_then(|work| work.face);
        let work = self.works.create(NewWork {
            kind: APP,
            face,
            executor: executor_name.to_string(),
            target: Some(opened.target.clone()),
            title: if opened.title.is_empty() { title } else { opened.title },
            brief: text.to_string(),
            origin,
            again: true,
        });
        if busy {
            self.enqueue(&work, None);
            return Ok(fill(say.queued_next, &[("title", &work.title)]));
        }
        let work = self.launch(work)?;
        tracing::info!(target: "talk", "work {}: a new version of {}", work.id, opened.target);
        let version = self.version_of(&work).to_string();
        Ok(fill(say.again, &[("title", &work.title), ("version", &version)]))
    }

    /// The question about a thing that is gone — make it again? — the layer's own: its wish waits for the answer.
    pub(super) fn gone(&self, device: &str, work: &Work) -> Refusal {
        let refusal = Refusal::Gone { title: work.title.clone() };
        let wish = Pending {
            text: work.brief.clone(),
            title: Some(work.title.clone()),
            face: work.face,
            question: Some(refusal.for_person(pack::of(self.lang(device)))),
            ..Pending::default()
        };
        self.asked.ask_with(device, &work.id, Kind::Remake, wish);
        refusal
    }

    /// The question which of these things — the layer's own: the action waits for the answer («первая», «вторая»,
    /// «последняя», or the reader's reading), as «Поправить … или новую?» does.
    pub(super) fn which(&self, device: &str, works: &[Work], act: &str, text: Option<&str>) -> Refusal {
        let pack = pack::of(self.lang(device));
        let refusal = Refusal::Which(
            works.iter().map(|work| (work.title.clone(), format!("{}{}", pack.work.label, work.id))).collect(),
        );
        let waits = Pending {
            text: text.unwrap_or_default().to_string(),
            act: Some(act.to_string()),
            choices: works.iter().map(|work| work.id.clone()).collect(),
            question: Some(refusal.for_person(pack)),
            ..Pending::default()
        };
        self.asked.ask_with(device, "", Kind::Which, waits);
        refusal
    }

    /// `work_do`: how it goes, what was done, try it on, the desk, the drafts, stop.
    pub async fn act(&self, origin: &Origin, action: &str, thing: Option<&str>) -> Result<String, Refusal> {
        let device = origin.device.as_str();
        let say = &pack::of(self.lang(device)).work.say;
        let work = match self.find(device, thing) {
            Found::One(work) => work,
            Found::Many(works) => return Err(self.which(device, &works, action, None)),
            Found::Gone(work) => return Err(self.gone(device, &work)),
            Found::None if action == "status" && thing.is_none() => return Ok(say.nothing.to_string()),
            Found::None | Found::Thing(..) => return Err(Refusal::NoThing),
        };
        let executor = self.executor(&work.executor).ok_or_else(|| Refusal::Failed("its executor is gone".into()))?;
        let title = in_sentence(&work.title);
        // Talked about: the thing stays in focus, and the history keeps its line for this phrase.
        self.works.update(&work.id, |_| {});
        self.acted_on(device, &work.id);
        let by = super::executor::ByVoice { device: device.to_string(), who: origin.who.clone() };
        Ok(match action {
            "status" if work.going() && matches!(work.kind.as_str(), super::works::TOOL | super::works::TASK) => {
                let words = &pack::of(self.lang(device)).work.background;
                let since = say.duration(now_ms().saturating_sub(work.started_at));
                let step = executor.details(&work).unwrap_or_else(|| words.waiting_answer.to_string());
                fill(words.working, &[("title", &work.title), ("since", &since), ("step", &step)])
            }
            "status" if work.going() => {
                let since = say.duration(now_ms().saturating_sub(work.started_at));
                let doing = executor.status(&work).unwrap_or(Doing::Thinking);
                let pack = pack::of(self.lang(device));
                fill(say.working, &[("since", &since), ("doing", super::now::doing_words(pack, doing))])
            }
            "status" if work.state.queued() => fill(say.waiting, &[("title", &work.title)]),
            "status" => fill(say.ready, &[("title", &work.title)]),
            "details" => executor.details(&work).unwrap_or_else(|| say.no_details.to_string()),
            "show" => match executor.put(&work, End::Fitting, by).await.map_err(Refusal::Failed)? {
                super::executor::Put::Shown { open: true } => say.shown.to_string(),
                _ => say.tile_waits.to_string(),
            },
            "park" => {
                executor.put(&work, End::Parked, by).await.map_err(Refusal::Failed)?;
                say.parked.to_string()
            }
            "desk" => self.desk(origin, &work).await?,
            // Off the queue: it never began.
            "stop" if work.state.queued() => {
                if let Some(work) = self.works.update(&work.id, |work| work.state = WorkState::Stopped) {
                    self.told(&work, "stopped");
                }
                fill(say.stopped, &[("title", &title)])
            }
            "stop" if executor.stop(&work) => fill(say.stopped, &[("title", &title)]),
            "stop" => fill(say.not_going, &[("title", &work.title)]),
            _ => return Err(Refusal::UnknownAction),
        })
    }
}

/// The connector that gives the brain the works' tools.
pub struct TalkConnector {
    layer: Weak<TalkLayer>,
}

impl TalkConnector {
    pub fn new(layer: &Arc<TalkLayer>) -> TalkConnector {
        TalkConnector { layer: Arc::downgrade(layer) }
    }
}

fn text<'a>(input: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    input.get(key).and_then(Value::as_str).map(str::trim).filter(|text| !text.is_empty())
}

fn object(properties: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required, "additionalProperties": false })
}

pub fn start_tool() -> LlmTool {
    LlmTool {
        name: "work_start".into(),
        description: "Start making a new thing for the home: a widget for the desk (by default), a skill, a game or a \
                      theme. Use it whenever the person wants something that does not exist yet, in any words — \
                      never tell them a phrase to say instead. Put the whole wish from the conversation into `brief`, \
                      in the person's own words: what it shows or does, every detail they gave. The answer is said to \
                      the person as it is."
            .into(),
        parameters: object(
            json!({
                "brief": { "type": "string", "description": "The whole wish, gathered from the conversation, in the person's words." },
                "face": { "type": "string", "enum": ["widget", "skill", "game", "theme"], "description": "What it becomes; a widget when the person did not say." },
                "title": { "type": "string", "description": "Its short name, one to four words: «Испанский: слова дня»." },
                "new": { "type": "boolean", "description": "The person confirmed they want a new thing although a similar one was made lately." }
            }),
            &["brief"],
        ),
    }
}

pub fn add_tool() -> LlmTool {
    LlmTool {
        name: "work_add".into(),
        description:
            "Add to a thing or change it: what the person adds while it is being made («и ещё кнопку…») goes \
                      into that same work — never start it over; for a thing that is ready or already on the desk it \
                      makes a new version of that thing — never a new thing. The answer is said to the person as it is."
                .into(),
        parameters: object(
            json!({
                "text": { "type": "string", "description": "What to add or change, in the person's words." },
                "thing": { "type": "string", "description": "Its label from «Сейчас» ([р3]) or its name; leave it out for the thing in work or talked about last." },
                "copy": { "type": "boolean", "description": "The person agreed to make their own copy of a built-in app to change it." }
            }),
            &["text"],
        ),
    }
}

pub fn do_tool() -> LlmTool {
    LlmTool {
        name: "work_do".into(),
        description: "Do something with a thing being made or made lately: `status` — how it goes; `details` — what \
                      was done; `show` — open it in the panel to try on; `desk` — put it on the desk; `park` — keep it \
                      in the drafts; `stop` — stop the work. The answer is said to the person as it is."
            .into(),
        parameters: object(
            json!({
                "action": { "type": "string", "enum": ["status", "details", "show", "desk", "park", "stop"] },
                "thing": { "type": "string", "description": "Its label from «Сейчас» ([р3]) or its name; leave it out for the thing in work or talked about last." }
            }),
            &["action"],
        ),
    }
}

impl Connector for TalkConnector {
    fn manifest(&self) -> ConnectorManifest {
        ConnectorManifest {
            id: "talk".into(),
            // Not listed: the works are the talk's own; the kind is only a label.
            kind: ConnectorKind::Agent,
            title: crate::tr!("Работы"),
            description: crate::tr!("Что делается по просьбам из разговора: начать, узнать, как идёт, остановить"),
            cloud: false,
            env: Vec::new(),
            capabilities: Some(vec![CapabilityName::Plugins]),
            setup: None,
        }
    }

    fn listed(&self) -> bool {
        false
    }

    fn status(&self) -> BoxFuture<'_, ConnectorStatus> {
        Box::pin(async move { ConnectorStatus::ready(crate::tr!("включены")) })
    }

    fn tools(&self) -> Vec<ToolContribution> {
        if self.layer.upgrade().is_none() {
            return Vec::new();
        }
        // Every answer — a refusal too — is the layer's phrase, said as it is: the model has nothing to retell, so it
        // cannot turn «not now» into «Делаю…» (docs/plans/audio-interface.md, «Доводка после проверки руками 14.11»).
        type Run = fn(Arc<TalkLayer>, Map<String, Value>, Origin) -> BoxFuture<'static, Result<String, Refusal>>;
        let tool = |tool: LlmTool, run: Run| {
            let (layer, kept_layer) = (self.layer.clone(), self.layer.clone());
            ToolContribution {
                tool,
                permission: use_of(CapabilityName::Plugins),
                run: Arc::new(move |input: Map<String, Value>, context: ToolContext| {
                    let layer = layer.clone();
                    async move {
                        let layer = layer.upgrade().ok_or_else(|| anyhow::anyhow!("the talk layer is gone"))?;
                        let origin = TalkLayer::origin_of(&context);
                        let pack = pack::of(layer.lang(&origin.device));
                        Ok(run(layer, input, origin).await.unwrap_or_else(|refusal| refusal.for_person(pack)))
                    }
                    .boxed()
                }),
                read_only: false,
                outside: false,
                spoken: true,
                kept: Some(Arc::new(move |context: &ToolContext, since: u64| {
                    kept_layer.upgrade()?.fact_since(&TalkLayer::origin_of(context).device, since)
                })),
            }
        };
        vec![
            tool(start_tool(), |layer, input, origin| {
                async move {
                    let face = text(&input, "face").and_then(Face::parse);
                    let new = input.get("new").and_then(Value::as_bool) == Some(true);
                    let brief = text(&input, "brief").unwrap_or_default();
                    layer.start(origin, brief, face, text(&input, "title"), new)
                }
                .boxed()
            }),
            tool(add_tool(), |layer, input, origin| {
                async move {
                    let copy = input.get("copy").and_then(Value::as_bool) == Some(true);
                    layer.add(origin, text(&input, "text").unwrap_or_default(), text(&input, "thing"), copy).await
                }
                .boxed()
            }),
            tool(do_tool(), |layer, input, origin| {
                async move { layer.act(&origin, text(&input, "action").unwrap_or_default(), text(&input, "thing")).await }
                    .boxed()
            }),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::events::EventBus;
    use crate::core::identity::principal::household_principal;
    use crate::core::identity::Role;
    use tokio_util::sync::CancellationToken;

    /// A refusal is the layer's phrase like a success: the model gets nothing to retell (live 14.11: a refused «not
    /// now» came out as «Делаю новый виджет…»). The history keeps a work's line only when a work was acted on.
    #[tokio::test]
    async fn a_refusal_of_the_works_tools_is_the_layers_phrase_and_a_work_acted_on_is_kept_as_its_line() {
        let layer = TalkLayer::new(None, EventBus::new());
        let tools = TalkConnector::new(&layer).tools();
        let add = tools.iter().find(|tool| tool.tool.name == "work_add").expect("work_add");
        let context = ToolContext {
            principal: household_principal(Role::Admin, None),
            cancel: CancellationToken::new(),
            station: None,
            origin: Some(Origin::computer(Activator::Button)),
        };
        let input = |value: Value| value.as_object().cloned().unwrap_or_default();
        let since = now_ms();
        let said = (add.run)(input(json!({ "text": "слова покрупнее" })), context.clone()).await.expect("said");
        assert_eq!(said, "Сейчас в работе ничего нет.");
        let kept = add.kept.as_ref().expect("a fact for the history");
        assert_eq!(kept(&context, since), None, "nothing acted on: the history keeps the phrase");

        let work = layer.works.create(crate::talk::works::NewWork {
            kind: crate::talk::works::APP,
            face: Some(Face::Widget),
            executor: WORKSHOP.into(),
            target: Some("widget-1".into()),
            title: "Испанский: слова дня".into(),
            brief: "испанские слова дня".into(),
            origin: Origin::computer(Activator::Button),
            again: false,
        });
        // A phrase of this device acted on the work (began it, asked about it, put it on the desk).
        let since = now_ms();
        layer.acted_on(crate::talk::origin::COMPUTER, &work.id);
        let fact = kept(&context, since).expect("the work's line");
        assert!(fact.starts_with("(В работе [р1]: виджет «Испанский: слова дня»"), "{fact}");
        assert_eq!(kept(&context, now_ms() + 1), None, "a later phrase acted on nothing yet");
    }

    /// A thing is alike by its name, not by the words of the wishes: on 14.11 «сделай» and «красивый» made a
    /// dollar's rate «like» the clock. Alike, the wish waits for the layer's question; a name is never the request.
    #[test]
    fn a_thing_is_alike_by_its_name_and_the_wish_waits_for_the_layers_question() {
        let layer = TalkLayer::new(None, EventBus::new());
        // Typed: the voice's limit of a new thing in two minutes is not this test's.
        let origin = Origin::computer(Activator::Text);
        layer.works.create(crate::talk::works::NewWork {
            kind: crate::talk::works::APP,
            face: Some(Face::Widget),
            executor: WORKSHOP.into(),
            target: Some("widget-2".into()),
            title: "Часы".into(),
            brief: "сделай красивые часы вот графикой".into(),
            origin: origin.clone(),
            again: false,
        });
        let dollar = layer.start(
            origin.clone(),
            "Добавь новый виджет, чтобы посмотреть курс доллара, сделай его красивым",
            None,
            None,
            false,
        );
        assert_eq!(dollar, Err(Refusal::Failed("no workshop here".into())), "not alike: on to the workshop");
        assert_eq!(layer.asked.open(crate::talk::origin::COMPUTER), None);

        let clock = layer.start(origin, "часы с секундами", None, Some("Часы с секундами"), false);
        assert_eq!(clock, Err(Refusal::Similar { title: "Часы".into() }));
        let asked = layer.asked.open(crate::talk::origin::COMPUTER).expect("the layer's question");
        assert_eq!((asked.work.as_str(), asked.kind), ("1", Kind::Similar));
        let wish = asked.pending.expect("the wish waits");
        assert_eq!((wish.text.as_str(), wish.title.as_deref()), ("часы с секундами", Some("Часы с секундами")));

        assert_eq!(short_name("часы с секундами"), Some("Часы с секундами".into()));
        assert_eq!(short_name("я учу испанский, дай мне слова дня"), None, "a long wish is no name");
        assert!(unnamed("Новый виджет") && unnamed("New game") && !unnamed("Часы"));
    }

    /// Namesakes are no question: the one being made now is meant, else the one talked about last (live 15.11: a new
    /// «Испанский: слова дня» beside yesterday's, «Какую: «Испанский: слова дня» или «Испанский: слова дня»?» in a loop).
    #[test]
    fn of_namesakes_the_one_being_made_or_talked_about_last_is_meant() {
        let layer = TalkLayer::new(None, EventBus::new());
        let origin = Origin::computer(Activator::Text);
        let thing = |target: &str| crate::talk::works::NewWork {
            kind: crate::talk::works::APP,
            face: Some(Face::Widget),
            executor: WORKSHOP.into(),
            target: Some(target.into()),
            title: "Испанский: слова дня".into(),
            brief: "испанские слова дня".into(),
            origin: origin.clone(),
            again: false,
        };
        let yesterday = layer.works.create(thing("widget-1"));
        layer.works.update(&yesterday.id, |work| work.state = crate::talk::works::WorkState::Done);
        let today = layer.works.create(thing("widget-4"));
        let found = |layer: &TalkLayer| match layer.find(crate::talk::origin::COMPUTER, Some("Испанский: слова дня"))
        {
            Found::One(work) => Some(work.id),
            _ => None,
        };
        assert_eq!(found(&layer), Some(today.id.clone()), "the one being made");
        layer.works.update(&today.id, |work| work.state = crate::talk::works::WorkState::Done);
        std::thread::sleep(std::time::Duration::from_millis(5));
        layer.works.update(&yesterday.id, |_| {});
        assert_eq!(found(&layer), Some(yesterday.id), "both ready: the one talked about last");
    }

    /// A thing removed is known by its id, never its name: off the desk with its draft kept — in the drafts; its
    /// draft deleted too — gone, «убрано» for the brain, and named it asks to be made again (live 15.11: a deleted
    /// widget stayed «на столе» and was still found by its name).
    #[tokio::test]
    async fn a_thing_removed_by_its_id_is_gone_for_its_works_and_named_it_asks_to_be_made_again() {
        use crate::talk::asked::Kind;
        use crate::talk::executor::{fake::Shelf, Presence};
        use crate::talk::ready::Answer;
        use crate::talk::works::{WorkResult, WorkState};
        let layer = TalkLayer::new(None, EventBus::new());
        let shelf = Shelf::with(&[("widget-1", "Испанский: слова дня", Presence::OnDesk)]);
        layer.add_executor(shelf.clone());
        let origin = Origin::computer(Activator::Text);
        let work = layer.works.create(crate::talk::works::NewWork {
            kind: crate::talk::works::APP,
            face: Some(Face::Widget),
            executor: WORKSHOP.into(),
            target: Some("widget-1".into()),
            title: "Испанский: слова дня".into(),
            brief: "испанские слова дня".into(),
            origin: origin.clone(),
            again: false,
        });
        layer.works.update(&work.id, |work| {
            work.state = WorkState::Done;
            work.result = Some(WorkResult { line: String::new(), end: Some(End::Desk), ..WorkResult::default() });
        });
        let end =
            |layer: &TalkLayer| layer.works.get(&work.id).and_then(|work| work.result).and_then(|result| result.end);

        layer.recheck("widget-1");
        assert_eq!(end(&layer), Some(End::Desk), "still on the desk: nothing to change");
        shelf.put("widget-1", "Испанский: слова дня", Presence::Draft);
        layer.recheck("widget-1");
        assert_eq!(end(&layer), Some(End::Parked), "off the desk, its draft kept");
        shelf.put("widget-1", "Испанский: слова дня", Presence::Gone);
        layer.recheck("widget-1");
        assert_eq!(end(&layer), Some(End::Removed));
        let now = layer.now(crate::talk::origin::COMPUTER, crate::memory::text::Lang::Ru);
        assert!(now.contains("«Испанский: слова дня» — убрано"), "{now}");

        let said = layer.act(&origin, "status", Some("испанский")).await;
        assert_eq!(said, Err(Refusal::Gone { title: "Испанский: слова дня".into() }));
        let asked = layer.asked.open(crate::talk::origin::COMPUTER).expect("the layer's question");
        assert_eq!(asked.kind, Kind::Remake);
        let made = layer.answer(origin, &asked.work, Kind::Remake, false, Answer::Yes).await;
        assert!(made.starts_with("Делаю: испанский: слова дня."), "{made}");
        assert_eq!(shelf.started.lock().unwrap().last().map(String::as_str), Some("испанские слова дня"));
    }

    #[test]
    fn tells_one_thing_from_another_by_the_words_of_its_title_and_wish() {
        let spanish = stems("Испанский: слова дня. Я учу испанский, дай мне виджет со словами дня");
        assert!(alike(&stems("испанский"), &spanish));
        assert!(alike(&stems("виджет с испанскими глаголами"), &spanish), "«виджет» tells nothing apart");
        assert!(!alike(&stems("виджет с курсом доллара"), &spanish));
        assert!(!alike(&stems("виджет"), &spanish), "a face alone is no thing");
        assert_eq!(in_sentence("Испанские слова дня"), "испанские слова дня");
        assert_eq!(in_sentence("MIDI-учитель"), "MIDI-учитель");
    }
}
