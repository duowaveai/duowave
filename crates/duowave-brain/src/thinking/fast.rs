//! The fast route (docs/plans/thinking.md, §3): what answers a phrase before any model. One list of steps, in the
//! order the setting `thinking.fast.steps` gives, for every device; each step runs for the kinds of device the setting
//! `thinking.fast.devices` names (D6: the quick rules are the stations'). A step with nothing to do passes the phrase
//! on; the first that takes it answers, and its name goes into the turn's line.
//!
//! | Step | What it takes | Today's piece |
//! |---|---|---|
//! | `answer` | a phrase in the window: a reply to our question, or one the closed window drops | `talk/window.rs`, `talk/rules.rs` (`answer_on`) |
//! | `quick` | a quick rule: our skill, or a hand of the place (hands.rs) | `skills/router.rs`, `skills/quick_commands.rs` |
//! | `door` | everything, while the workshop's door is open on the device | `talk/rules.rs` (`through_door`) |
//! | `works` | «как там», «и ещё…», «на стол», «подробнее», «отмени» | `talk/rules.rs` (`hear`) |
//! | `assistant` | what is the device's own assistant's, when the device has one (the group `assistant`) | `skills/router.rs` |
//! | `reader` | a phrase about works, read into an act by one call of a model | `talk/reader.rs` |
//!
//! The reader shares the model's request in the journal (one request, one trip), so it runs last of them, right before
//! the model, wherever the list puts it; the list still decides whether it runs.

use std::sync::Arc;

use crate::core::identity::Principal;
use crate::door::{Device, Kind, Phrase};
use crate::talk::{asked, rules, window, Activator, Parts, Talk, Turn};

use super::settings::Settings;

/// What a step sees of the turn.
pub struct Ctx<'a> {
    pub parts: &'a Parts,
    pub principal: &'a Principal,
    pub turn: &'a Turn,
    pub phrase: &'a Phrase,
    /// The phrase as the rules read it (`skills::text::normalize`).
    pub normalized: String,
    /// The home's devices with their hands, as the audio side tells them.
    pub devices: Option<&'a [Device]>,
    /// The risky hands waiting for their «да».
    pub pending: &'a super::hands::Pending,
    /// «Чутьё», asked Thinking's kinds (sense.rs): its answers go into the turn's line.
    pub sense: &'a (dyn Fn(crate::sense::Kind, &str, serde_json::Value) -> crate::sense::Answer + Send + Sync),
    /// A skill's own dialog going on on the device (a game's next move): the rules give it the next phrase.
    pub skill_dialog: Option<crate::skills::router::Continuation>,
}

/// What a step did with the phrase.
pub enum Taken {
    /// Today's pieces answer it: their talk.
    Talk(Talk),
    /// It is the device's own assistant's: `handoff`, with whose it is and the words it gets.
    Handoff(super::hands::Handed),
    /// A quick rule: our skill does it on the place, with no model (hands.rs).
    Skill(super::hands::Quick),
    /// A risky hand the person said «да» to.
    Confirmed(super::hands::PendingHand),
    /// These words, and the end: a risky hand let go.
    Say(String),
}

/// One step of the fast route.
pub trait FastStep: Send + Sync {
    fn name(&self) -> &'static str;
    fn take(&self, ctx: &Ctx<'_>) -> Option<Taken>;
}

/// A reply to our question, or a phrase in the closed window: what the window does not take, nothing keeps.
pub struct Answer;

impl FastStep for Answer {
    fn name(&self) -> &'static str {
        "answer"
    }

    fn take(&self, ctx: &Ctx<'_>) -> Option<Taken> {
        // A risky hand's «Позвонить маме?»: its «да» runs it, its «нет» lets it go; anything else is a new phrase.
        let heard_on = &ctx.phrase.place.device;
        if ctx.pending.waiting(heard_on) {
            let words = super::words::of(ctx.phrase.lang.as_deref().unwrap_or("ru"));
            let reply = (ctx.sense)(crate::sense::Kind::Answer, &ctx.turn.text, super::sense::yes_no());
            let reply = match reply.pick.as_deref() {
                Some("yes") => super::words::Reply::Yes,
                Some("no") => super::words::Reply::No,
                _ => super::words::Reply::Other,
            };
            match reply {
                super::words::Reply::Yes => {
                    let hand = ctx.pending.take(heard_on)?;
                    let_go_of_the_brain_s_question(ctx.parts, heard_on);
                    return Some(Taken::Confirmed(hand));
                }
                super::words::Reply::No => {
                    ctx.pending.take(heard_on);
                    let_go_of_the_brain_s_question(ctx.parts, heard_on);
                    return Some(Taken::Say(words.hands.refused.clone()));
                }
                super::words::Reply::Other => {
                    ctx.pending.take(heard_on);
                }
            }
        }
        // The reply to a skill on a station — its own dialog going on (a game's next move, «На сколько?»), or the
        // station's listening it asked for — is the skill's: the quick rules give it to it, not the closed window.
        let station_listened = ctx.phrase.context.answer_to.as_deref() == Some(super::hands::STATION_LISTENS);
        if ctx.skill_dialog.is_some() || station_listened {
            return None;
        }
        // A station hears for itself: a phrase it listened for, or one typed to it, is a phrase of its own — the closed
        // window («К2», a computer's microphone hearing by itself) drops nothing there and closes nothing; only an
        // answer to a question is the window's.
        let station = matches!(ctx.phrase.place.kind, Kind::Station | Kind::Box);
        if station
            && ctx.turn.origin.activator == Activator::Answer
            && matches!(
                window::sift(ctx.parts, &ctx.turn.text, &ctx.turn.origin.device),
                window::Sift::Drop | window::Sift::Close
            )
        {
            return None;
        }
        if let Some(talk) = window::begin(ctx.parts, ctx.principal, ctx.turn) {
            return Some(Taken::Talk(talk));
        }
        let_go_of_the_brain_s_question(ctx.parts, &ctx.turn.origin.device);
        if ctx.turn.origin.activator != Activator::Answer {
            return None;
        }
        let rules = rules::TalkRules::new(&ctx.parts.talk);
        let heard = rules.answer_on(&ctx.normalized, &ctx.turn.origin.device)?;
        Some(Taken::Talk(crate::talk::by_heard(ctx.parts, ctx.principal, ctx.turn, rules, heard)))
    }
}

/// The brain's own question is answered now — or the person went on with the button: it is not awaited any more.
pub fn let_go_of_the_brain_s_question(parts: &Parts, device: &str) {
    if parts.talk.asked.open_any(device).is_some_and(|question| question.kind == asked::Kind::Talk) {
        parts.talk.asked.take(device);
    }
}

/// Everything, while the workshop's door is open on the device.
pub struct Door;

impl FastStep for Door {
    fn name(&self) -> &'static str {
        "door"
    }

    fn take(&self, ctx: &Ctx<'_>) -> Option<Taken> {
        if !asks_works(ctx, "door") {
            return None;
        }
        let rules = rules::TalkRules::new(&ctx.parts.talk);
        let heard = rules.through_door(&ctx.normalized, &ctx.turn.origin.device)?;
        Some(Taken::Talk(crate::talk::by_heard(ctx.parts, ctx.principal, ctx.turn, rules, heard)))
    }
}

/// The works' words: «как там», «и ещё…», «на стол», «подробнее», «отмени».
pub struct Works;

impl FastStep for Works {
    fn name(&self) -> &'static str {
        "works"
    }

    fn take(&self, ctx: &Ctx<'_>) -> Option<Taken> {
        if !asks_works(ctx, "works") {
            return None;
        }
        let rules = rules::TalkRules::new(&ctx.parts.talk);
        let heard = rules.hear(&ctx.normalized, &ctx.turn.origin.device)?;
        Some(Taken::Talk(crate::talk::by_heard(ctx.parts, ctx.principal, ctx.turn, rules, heard)))
    }
}

/// «Чутьё»'s kind `work` says the phrase is the works': the workshop's door or the works' words.
fn asks_works(ctx: &Ctx<'_>, which: &str) -> bool {
    let data = serde_json::json!({"device": ctx.turn.origin.device});
    (ctx.sense)(crate::sense::Kind::Work, &ctx.turn.text, data).pick.as_deref() == Some(which)
}

/// A quick rule of the place: a hand, with no model (hands.rs). Only where the phrase's device has hands.
pub struct QuickRules;

impl FastStep for QuickRules {
    fn name(&self) -> &'static str {
        "quick"
    }

    fn take(&self, ctx: &Ctx<'_>) -> Option<Taken> {
        if ctx.phrase.hands.is_empty() {
            return None;
        }
        let follow = ctx
            .skill_dialog
            .map(|dialog| serde_json::json!({"capability": dialog.capability.as_str(), "until": dialog.until}));
        let data = serde_json::json!({
            "device": ctx.phrase.place.device,
            "wake": ctx.phrase.opened == crate::door::Opened::Word,
            "follow": follow,
        });
        // The rules read the act without the room it is for («в спальне выключи музыку»).
        let text = super::hands::without_rooms(&ctx.turn.text, ctx.phrase, ctx.devices.unwrap_or_default());
        (ctx.sense)(crate::sense::Kind::Quick, &text, data).pick?;
        super::hands::quick(ctx).map(Taken::Skill)
    }
}

/// What belongs to the device's own assistant: only a device that says it has one (the group `assistant`).
pub struct Assistant;

impl FastStep for Assistant {
    fn name(&self) -> &'static str {
        "assistant"
    }

    fn take(&self, ctx: &Ctx<'_>) -> Option<Taken> {
        super::hands::for_the_assistant(ctx).map(Taken::Handoff)
    }
}

/// The steps before the model, in the setting's order, those that run for this kind of device. The reader is not
/// among them: it shares the model's request (see the module's words).
pub fn steps(settings: &Settings, kind: Kind) -> Vec<Arc<dyn FastStep>> {
    settings
        .fast
        .steps
        .iter()
        .filter(|name| settings.fast.runs(name, kind))
        .filter_map(|name| -> Option<Arc<dyn FastStep>> {
            match name.as_str() {
                "answer" => Some(Arc::new(Answer)),
                "quick" => Some(Arc::new(QuickRules)),
                "door" => Some(Arc::new(Door)),
                "works" => Some(Arc::new(Works)),
                "assistant" => Some(Arc::new(Assistant)),
                _ => None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_steps_follow_the_setting_s_order_and_devices() {
        let settings = Settings::default();
        let names = |kind| steps(&settings, kind).iter().map(|step| step.name()).collect::<Vec<_>>();
        assert_eq!(names(Kind::Computer), ["answer", "door", "works", "assistant"]);
        assert_eq!(names(Kind::Station), ["answer", "quick", "door", "works", "assistant"]);
        let mut other = Settings::default();
        other.fast.steps = vec!["works".into(), "answer".into(), "nonsense".into()];
        other.fast.devices.clear();
        let names: Vec<_> = steps(&other, Kind::Computer).iter().map(|step| step.name()).collect();
        assert_eq!(names, ["works", "answer"]);
    }
}
