//! The hands of the place (docs/plans/thinking.md, §7; the contract's «Hands»): what the device that heard can do,
//! as tools for the model and as the quick rules' acts. They replace `ToolsKind` and `Capability.station` in Thinking:
//! a talk is offered the hands of the place it happens in, whatever the device.
//!
//! - **Where they come from.** The phrase's `hands` names the device's groups; `devices()` gives each hand's spec. A
//!   device that declares a group alone (the station today, `HandSpec::group`) gets today's tools of the group, by the
//!   table below; one that names its hands one by one gets a tool per hand, its parameters from `args`.
//! - **A hand as a tool.** It keeps today's name where the table has one (`set_volume` is `sound.set`), so a model
//!   that knows today's names keeps them; else the hand's id with `_` (`sound.set` → `sound_set`). Each gets a `room`
//!   to name another device's room.
//! - **A call.** The tool sends `hands {call, device, hand, args}`; the audio side runs it and answers `hands_done`,
//!   whose note or error is the tool's result for the model, in words. No answer in time is an error too.
//! - **A risky hand** (`risky`): the model is told to ask first; the reply «да» in the window runs it (the fast route's
//!   `answer`), «нет» lets it go.
//! - **Which device acts.** The one that heard, unless the call names another room, or the phrase does («в спальне
//!   выключи музыку»): then the device of that room that has the hand's group.
//! - **The device's own assistant** (Alice on a station) is the group `assistant` in `hands`: only then is a phrase
//!   handed off — no model may answer, or a quick rule says it is the assistant's.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};
use tokio::sync::oneshot;

use crate::core::identity::permissions::use_of;
use crate::core::identity::{can, Principal};
use crate::dialog::llm::{LlmTool, ToolCall};
use crate::dialog::ToolOutcome;
use crate::door::{Device, HandSpec, Phrase, What};
use crate::skills::capabilities::CapabilityName;
use crate::skills::router::{Continuation, Decision, ModelState, Resolution, RouteInput};
use crate::skills::routes::RouteStep;
use crate::skills::tools::tool_spec;

use super::fast::Ctx;
use super::out::Out;

/// The group of a device's own assistant: only with it is a phrase handed off.
pub const ASSISTANT: &str = "assistant";

/// How long a hand may take before its call is an error for the model.
pub const ANSWER_MS: u64 = 8_000;

/// How long a risky hand's question waits for its «да».
const PENDING_MS: u64 = 60_000;

/// The skills' own dialogs going on, by device (a game's next move, «На сколько?» of a timer): the next phrase there
/// goes on with the same skill while it lasts, as the station's session keeps it from `follow` today.
#[derive(Default)]
pub struct Dialogs(Mutex<HashMap<String, (CapabilityName, u64)>>);

impl Dialogs {
    /// A skill's outcome: its dialog goes on for `ttl_ms`; none (an empty capability, 0) — what went on is over.
    pub fn follow(&self, device: &str, capability: &str, ttl_ms: u64) {
        let mut dialogs = self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        match crate::skills::capabilities::is_capability(capability).filter(|_| ttl_ms > 0) {
            Some(capability) => {
                dialogs.insert(device.to_string(), (capability, crate::utils::time::now_ms() + ttl_ms));
            }
            None => {
                dialogs.remove(device);
            }
        }
    }

    /// The dialog going on on the device, as the router of the skills reads it.
    pub fn of(&self, device: &str) -> Option<Continuation> {
        let now = crate::utils::time::now_ms();
        let dialogs = self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        dialogs.get(device).filter(|(_, until)| *until > now).map(|(capability, until)| Continuation {
            capability: *capability,
            executor: RouteStep::Own,
            until: *until,
        })
    }
}

/// Today's tools as hands: the tool, the group of hands it acts with (the capability's name, as the station's ear
/// declares its groups), whether it is risky. A device that declares a group alone takes its hands by the group's
/// name, the act as an intent in the arguments (`{action, ...}`): the ear runs it as it runs today's call.
pub const TABLE: &[(&str, &str, bool)] = &[
    ("set_volume", "volume", false),
    ("control_playback", "playback", false),
    ("bluetooth", "bluetooth", false),
    ("set_timer", "timers", false),
    ("manage_timer", "timers", false),
    ("set_alarm", "alarms", false),
    ("manage_alarm", "alarms", false),
    ("do_not_disturb", "do_not_disturb", false),
    ("play_radio", "radio", false),
    ("play_music", "music", false),
    ("tell_tale", "tales", false),
    ("phone_call", "calls", true),
    ("send_voice_message", "calls", true),
    ("recognize_music", "music_recognition", false),
    ("morning_show", "morning_show", false),
];

/// One hand as Thinking offers it.
#[derive(Debug, Clone, PartialEq)]
pub struct Hand {
    pub hand: String,
    pub group: String,
    pub tool: LlmTool,
    pub risky: bool,
    /// A group declared alone: the call goes as an intent (`{action, ...}`) to the group.
    pub intent: bool,
}

/// A tool's name for a hand named one by one: its id with `_` (`lamp.toggle` → `lamp_toggle`).
pub fn tool_name(hand: &str) -> String {
    hand.replace('.', "_")
}

/// The capability of a group of hands (`volume`): what the person must be allowed (`use:volume`).
fn capability_of(group: &str) -> Option<CapabilityName> {
    crate::skills::capabilities::is_capability(group)
}

/// The `room` every hand's tool takes.
fn with_room(mut parameters: Value) -> Value {
    if !parameters.is_object() {
        parameters = json!({"type": "object", "properties": {}});
    }
    if let Some(properties) = parameters.as_object_mut().and_then(|object| {
        object.entry("properties").or_insert_with(|| json!({}));
        object.get_mut("properties").and_then(Value::as_object_mut)
    }) {
        properties.insert(
            "room".into(),
            json!({"type": "string", "description": "Another room, only when the person names one"}),
        );
    }
    parameters
}

/// The hands of the place a phrase was said in, that this person may use.
pub fn of_place(phrase: &Phrase, devices: &[Device], principal: &Principal) -> Vec<Hand> {
    let declared: Vec<HandSpec> = devices
        .iter()
        .find(|device| device.device == phrase.place.device)
        .map(|device| device.hands.clone())
        .unwrap_or_default();
    let mut hands = Vec::new();
    for group in phrase.hands.iter().filter(|group| group.as_str() != ASSISTANT) {
        let named: Vec<&HandSpec> =
            declared.iter().filter(|spec| &spec.group == group && spec.hand != *group).collect();
        if named.is_empty() {
            // The group alone: today's tools of it, by the table.
            for (tool, _, risky) in TABLE.iter().filter(|(_, of, _)| of == group) {
                let Some(spec) = tool_spec(tool) else { continue };
                if !can(Some(principal), &use_of(spec.capability)) {
                    continue;
                }
                let mut tool = spec.tool.clone();
                tool.parameters = with_room(tool.parameters);
                hands.push(Hand { hand: group.clone(), group: group.clone(), tool, risky: *risky, intent: true });
            }
            continue;
        }
        for spec in named {
            if capability_of(&spec.group).is_some_and(|capability| !can(Some(principal), &use_of(capability))) {
                continue;
            }
            // The hand in English for the model (`about`), else its title for people.
            let description = spec.about.clone().unwrap_or_else(|| spec.title.clone());
            let tool = LlmTool { name: tool_name(&spec.hand), description, parameters: with_room(spec.args.clone()) };
            hands.push(Hand {
                hand: spec.hand.clone(),
                group: spec.group.clone(),
                tool,
                risky: spec.risky,
                intent: false,
            });
        }
    }
    hands
}

/// A room's stem as a phrase says it: «спальня» is said «в спальне».
fn stem(room: &str) -> String {
    let plain = super::words::plain(room);
    let count = plain.chars().count();
    if count > 4 {
        plain.chars().take(count - 1).collect()
    } else {
        plain
    }
}

/// The device that acts: the one that heard, unless the call or the phrase names another room whose device has the
/// hand's group.
pub fn target(phrase: &Phrase, devices: &[Device], group: &str, room: Option<&str>) -> Option<String> {
    let has_group = |device: &&Device| {
        device.hands.iter().any(|spec| spec.group == group)
            || (device.device == phrase.place.device && phrase.hands.iter().any(|g| g == group))
    };
    if let Some(room) = room.filter(|room| !room.trim().is_empty()) {
        let wanted = stem(room);
        return devices
            .iter()
            .filter(has_group)
            .find(|device| {
                device
                    .room
                    .as_deref()
                    .is_some_and(|name| stem(name) == wanted || super::words::plain(name).starts_with(&wanted))
            })
            .map(|device| device.device.clone());
    }
    let said = super::words::plain(&phrase.text);
    let other = devices
        .iter()
        .filter(|device| device.device != phrase.place.device)
        .filter(has_group)
        .find(|device| device.room.as_deref().is_some_and(|name| said.contains(&stem(name))));
    Some(other.map(|device| device.device.clone()).unwrap_or_else(|| phrase.place.device.clone()))
}

/// What a hand's device answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Done {
    pub ok: bool,
    pub note: Option<String>,
    pub error: Option<String>,
}

/// The calls waiting for `hands_done`.
#[derive(Default)]
pub struct Calls(Mutex<HashMap<String, oneshot::Sender<Done>>>);

impl Calls {
    fn wait(&self, call: &str) -> oneshot::Receiver<Done> {
        let (sender, receiver) = oneshot::channel();
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(call.to_string(), sender);
        receiver
    }

    /// The device's answer to a call; false when nobody waits for it.
    pub fn done(&self, call: &str, done: Done) -> bool {
        let waiting = self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(call);
        waiting.is_some_and(|sender| sender.send(done).is_ok())
    }
}

/// A risky hand waiting for its «да».
#[derive(Debug, Clone)]
pub struct PendingHand {
    pub device: String,
    pub hand: String,
    pub args: Value,
    pub since: Instant,
}

/// The risky hands asked about, by the device that asked.
#[derive(Default)]
pub struct Pending(Mutex<HashMap<String, PendingHand>>);

impl Pending {
    pub fn ask(&self, heard_on: &str, hand: PendingHand) {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(heard_on.to_string(), hand);
    }

    /// The hand a device's question waits on, while it is fresh; taken.
    pub fn take(&self, heard_on: &str) -> Option<PendingHand> {
        let taken = self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(heard_on)?;
        (taken.since.elapsed() < Duration::from_millis(PENDING_MS)).then_some(taken)
    }

    pub fn waiting(&self, heard_on: &str) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(heard_on)
            .is_some_and(|hand| hand.since.elapsed() < Duration::from_millis(PENDING_MS))
    }
}

/// A turn's hands: what the place has, and how a call reaches its device.
pub struct Place {
    pub parts: crate::talk::Parts,
    pub principal: Principal,
    pub hands: Vec<Hand>,
    pub devices: Vec<Device>,
    pub phrase: Phrase,
    pub out: Out,
    pub calls: Arc<Calls>,
    pub pending: Arc<Pending>,
    pub answer_ms: u64,
    /// The skills' own dialogs: a skill the model called may go on with the next phrase.
    pub dialogs: Arc<Dialogs>,
}

impl Place {
    pub fn hand_of(&self, tool: &str) -> Option<&Hand> {
        self.hands.iter().find(|hand| hand.tool.name == tool)
    }

    /// Sends a hand to a device and waits for what it did.
    pub async fn send(&self, device: &str, hand: &str, args: Value) -> Done {
        send(&self.out, &self.calls, device, hand, args, self.answer_ms).await
    }

    /// Our skill does an intent on a device: its commands go as hands, what it found or said is the model's result.
    /// None — no skill of ours serves it here.
    pub async fn skill(&self, device: &str, intent: &crate::skills::capabilities::Intent) -> Option<ToolOutcome> {
        let skill = match self.parts.skills.router.resolve(intent, false, Some(&self.principal)) {
            Resolution::Own(skill) => skill,
            _ => return None,
        };
        let cancel = tokio_util::sync::CancellationToken::new();
        let context = context_on(&self.parts, device, &self.phrase, &self.principal, &cancel);
        let outcome = match skill.execute(intent, &context).await {
            Ok(outcome) => outcome,
            Err(error) => return Some(ToolOutcome::error(format!("error: {error}"))),
        };
        commands(&self.out, device, &outcome.commands);
        if let Some(continuation) = &outcome.continuation {
            let capability = continuation.capability.as_str();
            self.dialogs.follow(&self.phrase.place.device, capability, continuation.ttl_ms);
            self.out.send(What::Follow { capability: capability.to_string(), ttl_ms: continuation.ttl_ms });
        }
        let content = outcome.data.or(outcome.speech).unwrap_or_else(|| "done".into());
        Some(ToolOutcome::from(content))
    }

    /// A model's call of a hand: the device that acts, the question first when it is risky, the device's answer in
    /// words.
    pub async fn run(&self, call: &ToolCall) -> ToolOutcome {
        let Some(hand) = self.hand_of(&call.name) else {
            return ToolOutcome::error(format!("error: {} is not a hand of this place", call.name));
        };
        let words = super::words::of(self.phrase.lang.as_deref().unwrap_or("ru"));
        let mut args = Map::clone(&call.input);
        let room = args.remove("room").and_then(|room| room.as_str().map(str::to_string));
        let Some(device) = target(&self.phrase, &self.devices, &hand.group, room.as_deref()) else {
            return ToolOutcome::error(words.hands.no_device.clone());
        };
        // A group declared alone (the station's): our skill does the call's intent, and its commands go to the ear
        // as hands, as the station's answer carries them today (`ear::hands::hand_of`); the skill's words are the
        // model's result.
        if hand.intent {
            let call = ToolCall { input: args.clone(), ..call.clone() };
            let intent = match crate::skills::tools::intent_of(&call) {
                Ok(intent) => intent,
                Err(error) => return ToolOutcome::error(format!("error: {error}")),
            };
            if hand.risky {
                let args = serde_json::to_value(&intent).unwrap_or(Value::Null);
                self.pending.ask(
                    &self.phrase.place.device,
                    PendingHand { device, hand: hand.hand.clone(), args, since: Instant::now() },
                );
                return ToolOutcome::from(words.hands.ask_first.clone());
            }
            return match self.skill(&device, &intent).await {
                Some(outcome) => outcome,
                None => ToolOutcome::from(words.hands.no_device.clone()),
            };
        }
        if hand.risky {
            self.pending.ask(
                &self.phrase.place.device,
                PendingHand { device, hand: hand.hand.clone(), args: Value::Object(args), since: Instant::now() },
            );
            return ToolOutcome::from(words.hands.ask_first.clone());
        }
        let done = self.send(&device, &hand.hand, Value::Object(args)).await;
        outcome_of(&done, words)
    }
}

/// What the model reads of a device's answer.
pub fn outcome_of(done: &Done, words: &super::words::Words) -> ToolOutcome {
    match (done.ok, &done.error) {
        (true, _) => ToolOutcome::from(done.note.clone().unwrap_or_else(|| words.hands.done.clone())),
        (false, Some(error)) => ToolOutcome::error(format!("error: {error}")),
        (false, None) => ToolOutcome::error(format!("error: {}", words.hands.no_answer)),
    }
}

/// `hands` through the door, and the device's answer, or none in time.
pub async fn send(out: &Out, calls: &Calls, device: &str, hand: &str, args: Value, answer_ms: u64) -> Done {
    let call = format!("h-{}-{}", out.turn(), crate::utils::ids::uuid().chars().take(8).collect::<String>());
    let answer = calls.wait(&call);
    out.send(What::Hands { call: call.clone(), device: device.to_string(), hand: hand.to_string(), args });
    match tokio::time::timeout(Duration::from_millis(answer_ms), answer).await {
        Ok(Ok(done)) => done,
        _ => {
            calls.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&call);
            Done { ok: false, note: None, error: None }
        }
    }
}

/// A skill's commands to the ear, as hands: the station puts them into its answer (it sends no `hands_done`).
pub fn commands(out: &Out, device: &str, commands: &[crate::ear::Command]) {
    for (index, command) in commands.iter().enumerate() {
        let (hand, args) = crate::ear::hands::hand_of(command);
        out.send(What::Hands { call: format!("{}:c{index}", out.turn()), device: device.to_string(), hand, args });
    }
}

/// What the router of the skills makes of the phrase, as a station's would: the skill's dialog going on there, the
/// device's own assistant when it has one (`assistant` in `hands`), what the services know of the device.
fn decided(ctx: &Ctx<'_>) -> Decision {
    let input = RouteInput {
        text: without_rooms(&ctx.turn.text, ctx.phrase, ctx.devices.unwrap_or_default()),
        device_tag: ctx.phrase.place.device.clone(),
        continuation: ctx.skill_dialog,
        yandex_available: ctx.phrase.hands.iter().any(|group| group == ASSISTANT),
        wake_word: ctx.phrase.opened == crate::door::Opened::Word,
        principal: Some(ctx.principal.clone()),
        ..RouteInput::default()
    };
    ctx.parts.skills.router.decide(&filled(&ctx.parts.skills, Some(&ctx.parts.dialog), input))
}

/// The router's input with what the services know, as the station's session fills it: the device's state its last
/// request carried (the player, the timers), whether something rings there, the model's state. Without `dialog` (a
/// words step of «Чутьё») the model is taken as there.
pub fn filled(
    skills: &crate::skills::SkillsService,
    dialog: Option<&crate::dialog::DialogService>,
    input: RouteInput,
) -> RouteInput {
    use crate::config::ModelProfile;
    let now = crate::utils::time::now_ms();
    let device_state = skills.device_state(&input.device_tag).map(|(state, _)| state);
    let ringing = skills.ringing_now(&input.device_tag, device_state.as_ref(), now);
    let model = match dialog {
        Some(dialog) => ModelState {
            enabled: dialog.enabled(),
            tools: dialog.tools_enabled(),
            unreachable_fast: !dialog.reachable(ModelProfile::Fast),
            unreachable_smart: !dialog.reachable(ModelProfile::Smart),
            limited_fast: dialog.limit(ModelProfile::Fast).is_some(),
            limited_smart: dialog.limit(ModelProfile::Smart).is_some(),
            limit_to_alice: dialog.on_limit() == crate::settings::definitions::OnLimit::Alice,
        },
        None => ModelState { enabled: true, tools: true, ..ModelState::default() },
    };
    RouteInput { device_state, ringing, model, now, ..input }
}

/// A skill's context on a device, as the station's session gives it today: the device's state its last request
/// carried, its time zone, where it fetches our /media.
pub fn context_on(
    parts: &crate::talk::Parts,
    device: &str,
    phrase: &Phrase,
    principal: &Principal,
    cancel: &tokio_util::sync::CancellationToken,
) -> crate::skills::skill::ExecutionContext {
    let known = parts.skills.device_context(device);
    crate::skills::skill::ExecutionContext {
        text: phrase.text.clone(),
        timezone: known.timezone,
        device_state: parts.skills.device_state(device).map(|(state, _)| state),
        media_base_url: known.media_base_url,
        cancel: cancel.clone(),
        speaker: Some(principal.clone()),
        ..crate::skills::skill::ExecutionContext::new(device, chrono::Utc::now())
    }
}

/// The phrase without the words that name another device's room («в спальне»): the rules read the act.
pub fn without_rooms(text: &str, phrase: &Phrase, devices: &[Device]) -> String {
    let stems: Vec<String> = devices
        .iter()
        .filter(|device| device.device != phrase.place.device)
        .filter_map(|device| device.room.as_deref().map(stem))
        .collect();
    if stems.is_empty() {
        return text.to_string();
    }
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut kept: Vec<&str> = Vec::new();
    for word in words {
        let plain = super::words::plain(word);
        if stems.iter().any(|stem| plain.starts_with(stem.as_str())) {
            // The preposition before it goes too: «в», «во», «на».
            if kept
                .last()
                .is_some_and(|last| matches!(super::words::plain(last).as_str(), "в" | "во" | "на" | "in" | "on"))
            {
                kept.pop();
            }
            continue;
        }
        kept.push(word);
    }
    kept.join(" ")
}

/// A quick rule's skill: the intent the rules chose, served by our skill on the device it acts on.
pub struct Quick {
    pub skill: Arc<dyn crate::skills::skill::OwnSkill>,
    pub intent: crate::skills::capabilities::Intent,
    pub device: String,
}

/// A quick rule of the place for this phrase, served by our skill: only where the phrase's device has hands.
pub fn quick(ctx: &Ctx<'_>) -> Option<Quick> {
    if ctx.phrase.hands.is_empty() {
        return None;
    }
    let Decision::Intent { intent, resolution: Resolution::Own(skill), .. } = decided(ctx) else { return None };
    let devices = ctx.devices.unwrap_or_default();
    let device = target(ctx.phrase, devices, intent.capability.as_str(), None)
        .unwrap_or_else(|| ctx.phrase.place.device.clone());
    Some(Quick { skill, intent, device })
}

/// The question a station's own listening opens (a game's next move): the talk layer's name for it.
pub const STATION_LISTENS: &str = crate::talk::thinking::STATION_LISTENS;

/// The special hand of a skill that records the answer's speech into a file (`{file}`): the station's ear writes it.
pub const RECORD_AUDIO: &str = crate::talk::thinking::RECORD_AUDIO;

/// How many skills' hooks for the end of the speech are kept for their `said`.
const SPOKEN_KEPT: usize = 16;

/// The skills' hooks for the end of the speech (a turn timer starts then), by turn: run when the device tells `said`.
#[derive(Default)]
pub struct Spoken(Mutex<std::collections::VecDeque<(String, crate::skills::skill::SpokenHook)>>);

impl Spoken {
    pub fn keep(&self, turn: &str, hook: crate::skills::skill::SpokenHook) {
        let mut spoken = self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        spoken.retain(|(kept, _)| kept != turn);
        spoken.push_back((turn.to_string(), hook));
        while spoken.len() > SPOKEN_KEPT {
            spoken.pop_front();
        }
    }

    /// The turn was heard to its end (`at`, ms since the epoch; none — now): its skill's hook runs, once.
    pub fn said(&self, turn: &str, at: Option<u64>) {
        let hook = {
            let mut spoken = self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            spoken.iter().position(|(kept, _)| kept == turn).and_then(|at| spoken.remove(at)).map(|(_, hook)| hook)
        };
        if let Some(hook) = hook {
            hook(at.unwrap_or_else(crate::utils::time::now_ms));
        }
    }
}

/// What a skill's turn leaves behind it: its dialog on the device, its hook for the end of the speech.
pub struct Leaves<'a> {
    pub dialogs: &'a Dialogs,
    pub spoken: &'a Spoken,
}

/// A quick rule's turn, in the order the station's chain has it today (talk/thinking.rs, `outcome`): the assistant
/// first when the phrase is its after all (an empty «Своя музыка»), the hook for the end of the speech kept, the
/// commands as hands, «listen after» before the words, a recording, the skill going on; then its words, phrase by
/// phrase (its data put into words by the model, as a station's are), kept in the history.
#[allow(clippy::too_many_arguments)]
pub async fn skill_turn(
    parts: &crate::talk::Parts,
    principal: &Principal,
    phrase: &Phrase,
    turn: &crate::talk::Turn,
    out: &Out,
    cancel: &tokio_util::sync::CancellationToken,
    quick: Quick,
    leaves: Leaves<'_>,
) -> super::out::Ended {
    use crate::core::requests::RequestStatus;
    use crate::door::{Expects, Failure, Outcome};
    use futures::StreamExt;
    // Every turn is a request of the journal: this one our skill answers, by the quick rule.
    let origin = turn.origin.clone().of(principal);
    let trip = crate::talk::journal(parts, turn, &origin, quick.intent.capability.as_str(), "quick");
    out.begun(&trip.id(), "home");
    let context = context_on(parts, &quick.device, phrase, principal, cancel);
    let pack = crate::talk::pack::of(parts.talk.lang(&phrase.place.device));
    let outcome = match quick.skill.execute(&quick.intent, &context).await {
        Ok(outcome) => outcome,
        Err(error) => {
            out.say(pack.failed);
            let message = format!("{error:#}");
            trip.finish_unclaimed(RequestStatus::Failed, Some(&message));
            out.end(Outcome::Failed, Some(Failure::Error), Some(message.clone()));
            return super::out::Ended {
                outcome: Outcome::Failed,
                said: String::new(),
                work: None,
                error: Some(message),
            };
        }
    };
    let crate::skills::skill::Outcome {
        speech,
        phrases,
        commands: acts,
        listen,
        data,
        continuation,
        record_audio,
        done,
        kept,
        on_spoken,
        to_yandex,
    } = outcome;
    // Nothing of ours after all: the device's assistant takes it where it can; where it cannot, our words are said.
    if to_yandex && phrase.hands.iter().any(|group| group == ASSISTANT) {
        out.send(What::Handoff { capability: Some(quick.intent.capability.as_str().to_string()), text: None });
    }
    if let Some(hook) = on_spoken {
        leaves.spoken.keep(out.turn(), hook);
    }
    commands(out, &quick.device, &acts);
    if listen {
        // Before the words: the station's listening goes with its answer.
        out.ask(STATION_LISTENS, None, Expects::Free);
    }
    if let Some(file) = record_audio {
        out.send(What::Hands {
            call: format!("{}:record", out.turn()),
            device: quick.device.clone(),
            hand: RECORD_AUDIO.to_string(),
            args: json!({ "file": file }),
        });
    }
    // The skill going on (a game's next move); none: an earlier one is over.
    let (capability, ttl_ms) = continuation
        .as_ref()
        .map(|continuation| (continuation.capability.as_str().to_string(), continuation.ttl_ms))
        .unwrap_or_default();
    leaves.dialogs.follow(&phrase.place.device, &capability, ttl_ms);
    out.send(What::Follow { capability, ttl_ms });
    let key = super::turn::history_key(phrase, principal);
    if let Some(mut phrases) = phrases {
        while let Some(Ok(text)) = phrases.next().await {
            out.say(&text);
        }
    } else if let Some(data) = data.filter(|_| parts.dialog.enabled()) {
        // The model puts the skill's data into words.
        let context = crate::dialog::DialogContext {
            facts: Some(data),
            spoken: true,
            channel: super::turn::channel(phrase.place.kind),
            timezone: parts
                .skills
                .device_context(&phrase.place.device)
                .timezone
                .or_else(crate::dialog::llm::limits::home_timezone),
            device: Some(phrase.place.device.clone()),
            profile: Some(crate::config::ModelProfile::Fast),
            max_tokens: Some(crate::dialog::SPEECH_MAX_TOKENS),
            ..crate::dialog::DialogContext::default()
        };
        let mut phrases = parts.dialog.answer(&key, &phrase.text, context, cancel.clone(), None);
        while let Some(Ok(text)) = phrases.next().await {
            out.say(&text);
        }
    } else if let Some(speech) = speech.filter(|speech| !speech.is_empty()) {
        let remembered = kept.as_deref().unwrap_or(&speech);
        parts.dialog.remember(&key, &phrase.text, remembered, crate::dialog::AnsweredBy::Skill, done.as_deref());
        for text in crate::dialog::split_phrases(&speech) {
            out.say(&text);
        }
    }
    let said = out.said();
    trip.answered(&said);
    trip.finish_unclaimed(if said.is_empty() { RequestStatus::Silent } else { RequestStatus::Answered }, None);
    if listen {
        out.end(Outcome::Asked, None, None);
        return super::out::Ended { outcome: Outcome::Asked, said, work: None, error: None };
    }
    out.end(Outcome::Answered, None, None);
    super::out::Ended { outcome: Outcome::Answered, said, work: None, error: None }
}

/// The phrase handed to the device's own assistant: whose it is, and the words it gets instead (a rule's rewrite).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Handed {
    pub capability: Option<String>,
    pub text: Option<String>,
}

/// The phrase is the device's own assistant's: the device has one, and no model may answer, or a quick rule says it
/// is the assistant's (or goes on in the assistant's own dialog).
pub fn for_the_assistant(ctx: &Ctx<'_>) -> Option<Handed> {
    if !ctx.phrase.hands.iter().any(|group| group == ASSISTANT) {
        return None;
    }
    if !ctx.parts.dialog.enabled() {
        return Some(Handed { capability: Some(CapabilityName::Chat.as_str().to_string()), text: None });
    }
    match decided(ctx) {
        Decision::YandexDialog { capability } => {
            Some(Handed { capability: Some(capability.as_str().to_string()), text: None })
        }
        Decision::ModelAway => Some(Handed { capability: Some(CapabilityName::Chat.as_str().to_string()), text: None }),
        Decision::Intent { intent, resolution: Resolution::Yandex, .. } => {
            // A phrase read as another command goes to the assistant as that command, as text.
            let text = intent
                .params
                .get("yandexText")
                .and_then(Value::as_str)
                .filter(|said| *said != ctx.turn.text)
                .map(str::to_string);
            Some(Handed { capability: Some(intent.capability.as_str().to_string()), text })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_skill_s_dialog_lasts_its_time_on_its_device_and_its_hook_runs_once_at_said() {
        use std::sync::atomic::{AtomicU64, Ordering::SeqCst};
        let dialogs = Dialogs::default();
        dialogs.follow("kitchen", "timers", 30_000);
        let going = dialogs.of("kitchen").expect("the timer's question goes on");
        assert_eq!((going.capability, going.executor), (CapabilityName::Timers, RouteStep::Own));
        assert!(dialogs.of("bedroom").is_none(), "another device's phrase is not the skill's");
        dialogs.follow("kitchen", "", 0);
        assert!(dialogs.of("kitchen").is_none(), "a skill that does not go on lets the earlier one go");
        let spoken = Spoken::default();
        let heard = Arc::new(AtomicU64::new(0));
        {
            let heard = heard.clone();
            spoken.keep("t-1", Box::new(move |at| heard.store(at, SeqCst)));
        }
        spoken.said("t-0", Some(7));
        assert_eq!(heard.load(SeqCst), 0, "a turn not kept tells nothing");
        spoken.said("t-1", Some(42));
        assert_eq!(heard.load(SeqCst), 42);
        spoken.said("t-1", Some(99));
        assert_eq!(heard.load(SeqCst), 42, "once");
    }
    use crate::core::identity::principal::household_principal;
    use crate::core::identity::Role;
    use crate::door::Kind;

    fn phrase(text: &str, hands: &[&str]) -> Phrase {
        let mut phrase: Phrase = serde_json::from_value(json!({
            "turn": "t-1", "text": text, "where": {"device": "kitchen", "room": "кухня", "kind": "station"}, "opened": "word",
        }))
        .unwrap();
        phrase.hands = hands.iter().map(|group| group.to_string()).collect();
        phrase
    }

    fn device(id: &str, room: &str, groups: &[&str]) -> Device {
        Device {
            device: id.into(),
            room: Some(room.into()),
            kind: Kind::Station,
            screen: false,
            speaker: true,
            hands: groups.iter().map(|group| HandSpec::group(group, group)).collect(),
            busy: false,
        }
    }

    #[test]
    fn a_group_alone_gives_today_s_tools_by_the_table_with_a_room() {
        let owner = household_principal(Role::Admin, None);
        let hands = of_place(&phrase("громче", &["volume", "timers", "assistant"]), &[], &owner);
        let names: Vec<&str> = hands.iter().map(|hand| hand.tool.name.as_str()).collect();
        assert_eq!(names, ["set_volume", "set_timer", "manage_timer"]);
        assert_eq!((hands[0].hand.as_str(), hands[0].intent), ("volume", true), "the group alone takes it");
        assert!(hands[0].tool.parameters["properties"]["room"].is_object());
        let guest = household_principal(Role::Guest, None);
        let calls = of_place(&phrase("позвони маме", &["calls"]), &[], &guest);
        assert!(calls.is_empty(), "a guest may not call");
        let theirs = of_place(&phrase("позвони маме", &["calls"]), &[], &owner);
        assert!(theirs.iter().all(|hand| hand.risky));
    }

    #[test]
    fn hands_named_one_by_one_are_tools_of_their_own() {
        let owner = household_principal(Role::Admin, None);
        let mut kitchen = device("kitchen", "кухня", &[]);
        kitchen.hands = vec![HandSpec {
            hand: "lamp.toggle".into(),
            group: "lights".into(),
            title: "Лампа".into(),
            args: json!({"type": "object", "properties": {"on": {"type": "boolean"}}}),
            risky: false,
            about: Some("Turn the lamp on or off".into()),
        }];
        let hands = of_place(&phrase("включи лампу", &["lights"]), &[kitchen], &owner);
        assert_eq!(hands.len(), 1);
        assert_eq!(
            (hands[0].tool.name.as_str(), hands[0].tool.description.as_str()),
            ("lamp_toggle", "Turn the lamp on or off")
        );
        assert!(!hands[0].intent && hands[0].hand == "lamp.toggle");
        assert!(hands[0].tool.parameters["properties"]["on"].is_object());
    }

    #[test]
    fn the_device_that_heard_acts_unless_another_room_is_named() {
        let devices = [
            device("kitchen", "кухня", &["playback"]),
            device("bedroom", "спальня", &["playback"]),
            device("hall", "зал", &[]),
        ];
        let heard = phrase("выключи музыку", &["playback"]);
        assert_eq!(target(&heard, &devices, "playback", None), Some("kitchen".into()));
        let there = phrase("в спальне выключи музыку", &["playback"]);
        assert_eq!(target(&there, &devices, "playback", None), Some("bedroom".into()));
        assert_eq!(target(&heard, &devices, "playback", Some("спальня")), Some("bedroom".into()));
        assert_eq!(target(&heard, &devices, "playback", Some("зал")), None, "the hall has no player");
    }
}
