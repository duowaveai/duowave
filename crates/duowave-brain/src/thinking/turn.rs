//! A turn that reached the brain (docs/plans/thinking.md, §2, steps 3–6): one request of the journal, the reader when
//! the phrase is about works, then the model with the card and the rounds. The same request, the same history and the
//! same events as the talk layer gives today (`talk::begin`), so `OurThinking` changes nothing for people until a
//! setting does.

use std::sync::Arc;
use std::time::Instant;

use futures::StreamExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::config::ModelProfile;
use crate::core::identity::Principal;
use crate::core::requests::{Place, RequestHandle, RequestKind, RequestPhase, RequestStart, RequestStatus};
use crate::dialog::DialogContext;
use crate::door::{Expects, Failure, Kind, Outcome, Phrase, WaitWhy};
use crate::talk::{Brain, Parts, TalkEvent, Turn, Via, Why, NO_MODEL};

use super::card;
use super::out::{ask_and_end, work_of, Ended, Out};
use super::settings::{Settings, Where};

/// The conversation's key in the short history: a computer's talk is its person's, as the panel's chat is
/// (`panel:<person>`); a device of its own keeps its own.
pub fn history_key(phrase: &Phrase, principal: &Principal) -> String {
    match phrase.place.kind {
        Kind::Computer | Kind::Phone | Kind::Other => format!("panel:{}", principal.id),
        Kind::Station | Kind::Box => phrase.place.device.clone(),
    }
}

/// The prompt's channel: a station's words, or a computer's.
pub(super) fn channel(kind: Kind) -> crate::dialog::Channel {
    match kind {
        Kind::Station | Kind::Box => crate::dialog::Channel::Station,
        Kind::Computer | Kind::Phone | Kind::Other => crate::dialog::Channel::Computer,
    }
}

/// The request of the journal for the brain's answer: where it was heard, where it came from.
fn trip_of(parts: &Parts, principal: &Principal, turn: &Turn, brain: &Brain) -> Arc<RequestHandle> {
    let kind = if turn.origin.via == Via::Voice { RequestKind::Voice } else { RequestKind::Text };
    let trip = (parts.journal)(RequestStart {
        device: turn.origin.device.clone(),
        session: None,
        kind,
        text: Some(turn.text.clone()),
        parent: None,
    });
    if let Some(heard) = &turn.heard {
        trip.heard_before(heard.by, heard.what.as_deref(), heard.ms.unwrap_or(0));
    }
    trip.set_origin(turn.origin.clone().of(principal));
    trip.routed(Some("chat"), "model");
    trip.step(RequestPhase::Think, Some(brain.by), Some(&brain.what));
    trip
}

/// The voice's step, once the first phrase is ready, when it will be heard.
fn speaking(parts: &Parts, trip: &RequestHandle, voice: bool) {
    if voice {
        let voice = parts.voice.own_choice().map(|choice| choice.voice);
        trip.step(RequestPhase::Speak, Some(Place::Own), voice.as_deref());
    }
}

/// What the brain's part of a turn needs.
pub struct Brainwork<'a> {
    pub parts: &'a Parts,
    pub principal: &'a Principal,
    pub phrase: &'a Phrase,
    pub turn: &'a Turn,
    pub settings: &'a Settings,
    pub out: &'a Out,
    pub cancel: &'a CancellationToken,
    /// The turn's line, as the rounds go.
    pub record: &'a super::line::Record,
    /// The apps' names for people (the catalog's lines).
    pub titles: Option<super::catalog::AppTitles>,
    /// The places' usual tools, as the last night counted them.
    pub usual: super::usual::Usual,
    /// The hands of the place, when the device has any.
    pub place: Option<std::sync::Arc<super::hands::Place>>,
    /// The models' expected round times (rounds.rs).
    pub expected: super::rounds::Expected,
    /// Who it is said to (agent.rs).
    pub agent: &'a super::agent::Agent,
    /// «Чутьё», asked Thinking's kinds (sense.rs).
    pub sense: &'a (dyn Fn(crate::sense::Kind, &str, serde_json::Value) -> crate::sense::Answer + Send + Sync),
}

/// The brain answers the turn: the request, the reader, the model. Its events go through `out`, to `end`.
pub async fn answer(work: Brainwork<'_>) -> Ended {
    let Brainwork {
        parts,
        principal,
        phrase,
        turn,
        settings,
        out,
        cancel,
        record,
        titles,
        usual,
        place: hands,
        expected,
        agent,
        sense,
    } = work;
    let device = turn.origin.device.clone();
    if !parts.dialog.enabled() {
        // No model: its words, then the end, as the talk layer refuses.
        out.say(NO_MODEL);
        out.end_said(Outcome::Failed, Some(Failure::NoModel), String::new(), Some(NO_MODEL.to_string()));
        return Ended { outcome: Outcome::Failed, said: String::new(), work: None, error: Some(NO_MODEL.into()) };
    }
    let key = super::agent::history_key(&history_key(phrase, principal), agent, settings);
    if turn.fresh {
        parts.dialog.forget(&key);
    }
    let brain = Brain::of(parts);
    let trip = trip_of(parts, principal, turn, &brain);
    out.begun(&trip.id(), brain.place());
    let started = Instant::now();
    let since = crate::utils::time::now_ms();
    let place = Where::of(phrase.place.kind, phrase.budget.voice);
    let context = DialogContext {
        spoken: true,
        channel: channel(phrase.place.kind),
        // A spoken answer: a few sentences (docs/plans/audio-interface.md, decision 14).
        max_tokens: Some(crate::dialog::SPEECH_MAX_TOKENS),
        device: matches!(phrase.place.kind, Kind::Station | Kind::Box).then(|| device.clone()),
        // The device's time zone, as its last request said (a station's); else the home's (the station's last).
        timezone: parts.skills.device_context(&device).timezone.or_else(crate::dialog::llm::limits::home_timezone),
        // The agent of the turn brings its persona, its view of memory and its note in the history.
        persona: agent.prompt.clone(),
        viewer: agent.own().then(|| agent.id.clone()).flatten(),
        note: super::agent::note_of(agent),
        ..DialogContext::default()
    };
    // The model's profile: today's rule by the phrase where the station's chain had it, and why, for the router's
    // deal.
    let station = matches!(phrase.place.kind, Kind::Station | Kind::Box);
    let profile = if station && settings.senses("profile") {
        match sense(crate::sense::Kind::Profile, &turn.text, serde_json::json!({"command": false})).pick.as_deref() {
            Some("smart") => ModelProfile::Smart,
            _ => ModelProfile::Fast,
        }
    } else {
        ModelProfile::Fast
    };
    let context = DialogContext {
        profile: Some(profile),
        reason: station.then(|| crate::skills::quick_commands::profile_reason(&turn.text, profile)),
        ..context
    };
    let origin = turn.origin.clone().of(principal);

    // A phrase about the works is read first: the layer does and says it (reader.rs).
    let reader = settings
        .fast
        .runs("reader", phrase.place.kind)
        .then(|| parts.talk.reader())
        .flatten()
        .filter(|reader| reader.wants(&device, &turn.text));
    if let Some(reader) = reader {
        let read = tokio::select! {
            _ = cancel.cancelled() => {
                trip.finish_unclaimed(RequestStatus::Cancelled, None);
                return Ended { outcome: Outcome::Other, said: out.said(), work: None, error: None };
            }
            read = reader.read(&key, origin.clone(), &turn.text, context.clone(), cancel.clone()) => read,
        };
        if let Some(read) = read {
            trip.step(RequestPhase::Tool, Some(Place::Own), Some("talk:read"));
            record.with(|line| line.fast = Some("reader".into()));
            speaking(parts, &trip, turn.voice);
            trip.answered(&read.said);
            trip.finish_unclaimed(RequestStatus::Answered, None);
            out.say(&read.said);
            let work = parts.talk.touched_since(&device, since);
            let (asks, remind) = crate::talk::closing(parts, &device, &read.said, turn.voice, false);
            return end_of_answer(parts, out, read.said, work, asks, remind);
        }
    }

    // The tools' talk events (a tool at work) are said as `wait`.
    let (tool_events, mut tool_heard) = mpsc::unbounded_channel::<TalkEvent>();
    let waits = {
        let out = out.clone();
        tokio::spawn(async move {
            while let Some(event) = tool_heard.recv().await {
                if let TalkEvent::Tool { name } = event {
                    out.wait(WaitWhy::Tool, None, Some(name));
                }
            }
        })
    };
    let mut variant = settings.card_variant(place);
    // D7: a model at home keeps every tool.
    if variant != super::settings::CardVariant::All && brain.by == Place::Own && settings.card.local_all {
        variant = super::settings::CardVariant::All;
    }
    let opened = super::more::Opened::default();
    // The provider of the fast profile's model: its delivery and its cache's rounds.
    let provider = match parts.dialog.brain().provider {
        crate::config::DialogProvider::Anthropic => Some("anthropic".to_string()),
        crate::config::DialogProvider::OpenAi => Some("openai".to_string()),
        _ => None,
    };
    let on_provider = super::settings::Where { provider: provider.as_deref(), ..place };
    let delivery = settings.delivery(on_provider);
    let lang = phrase.lang.clone().unwrap_or_else(|| parts.talk.lang(&device).as_str().to_string());
    let made = card::maker(variant).make(card::Input {
        parts,
        principal,
        text: &turn.text,
        origin,
        trip: trip.clone(),
        events: tool_events,
        brain: brain.clone(),
        hands: &phrase.hands,
        settings,
        device: &device,
        lang: &lang,
        usual: usual.top(&device, settings.card.caps.usual),
        used: usual.counts(&device),
        titles,
        place: hands,
        opened: Some(opened.clone()),
        delivery,
        provider: provider.clone(),
        picked: settings.senses("groups").then(|| {
            let answer = sense(crate::sense::Kind::Groups, &turn.text, serde_json::Value::Null);
            answer.p.into_iter().map(|(group, _)| group).collect()
        }),
    });
    record.with(|line| {
        line.variants.card = super::line::name_of(&made.variant);
        if let Some(start) = &made.start {
            let card = line.card.get_or_insert_with(Default::default);
            card.groups = start.catalog.groups.len() as u32;
            card.picked = start.picked.clone();
            card.from = start.from.clone();
        }
    });
    let shadow_card = made
        .start
        .as_ref()
        .filter(|_| made.variant == super::settings::CardVariant::All)
        .map(|start| (start.names(), start.tokens()));
    // The rounds' policy: live, or in shadow beside today's (rounds.rs).
    let policy = settings.rounds_policy(place);
    let limits = super::rounds::Limits::of(settings, place, &phrase.budget);
    let decisions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let steered =
        super::rounds::steer(policy, settings.shadows("rounds"), limits.clone(), expected, out, decisions.clone());
    let mut toolbox = made.toolbox;
    // A turn's later rounds cached too, where the setting says so for this provider.
    if let Some(toolbox) = toolbox.as_mut() {
        toolbox.cache_history = settings.card.cache_rounds.at(on_provider, &false);
    }
    let actual = if made.variant == super::settings::CardVariant::All {
        super::settings::Delivery::Append
    } else {
        super::more::delivered(delivery, provider.as_deref())
    };
    record.with(|line| line.variants.delivery = super::line::name_of(&actual));
    if policy == super::settings::RoundsPolicy::ByTime {
        if let Some(toolbox) = toolbox.as_mut() {
            // No spoken filler of Thinking's own: the door's `wait` takes its place.
            toolbox.filler = None;
            toolbox.rounds = Some(toolbox.rounds.unwrap_or(2).max(limits.ceiling));
            toolbox.budget_ms = Some(limits.limit_ms);
        }
    }
    let steer =
        crate::dialog::Steer { before: steered.before, slow: steered.slow, slow_ms: steered.slow_ms, ..record.steer() };
    let mut phrases = parts.dialog.answer_steered(&key, &turn.text, context, cancel.clone(), toolbox, steer);
    let mut said: Vec<String> = Vec::new();
    let ended = loop {
        let next = tokio::select! {
            _ = cancel.cancelled() => {
                trip.finish_unclaimed(RequestStatus::Cancelled, None);
                break Ended { outcome: Outcome::Other, said: said.join(" "), work: None, error: None };
            }
            next = phrases.next() => next,
        };
        match next {
            Some(Ok(text)) => {
                if said.is_empty() {
                    speaking(parts, &trip, turn.voice);
                }
                said.push(text.clone());
                trip.answered(&said.join(" "));
                out.say(&text);
            }
            Some(Err(error)) => {
                if cancel.is_cancelled() {
                    trip.finish_unclaimed(RequestStatus::Cancelled, None);
                    break Ended { outcome: Outcome::Other, said: said.join(" "), work: None, error: None };
                }
                let message: String = parts.secrets.redact(&format!("{error:#}")).chars().take(300).collect();
                trip.finish_unclaimed(RequestStatus::Failed, Some(&message));
                tracing::info!(target: "thinking", "an answer on {device} broke off: {message}");
                let pack = crate::talk::pack::of(parts.talk.lang(&device));
                out.say(pack.failed);
                out.end_said(Outcome::Failed, Some(Failure::Error), String::new(), Some(message.clone()));
                break Ended { outcome: Outcome::Failed, said: String::new(), work: None, error: Some(message) };
            }
            None => {
                let status = if said.is_empty() { RequestStatus::Silent } else { RequestStatus::Answered };
                trip.finish_unclaimed(status, None);
                let work = parts.talk.touched_since(&device, since);
                let said = said.join(" ");
                // A work's line («Делаю: … Что-то ещё добавить?») is the layer's question, not the brain's.
                let (asks, remind) = crate::talk::closing(parts, &device, &said, turn.voice, work.is_none());
                break end_of_answer(parts, out, said, work, asks, remind);
            }
        }
    };
    waits.abort();
    // The groups the model opened, on the rounds that opened them.
    let mut opens = opened.all().into_iter();
    record.with(|line| {
        for round in line.rounds.iter_mut().filter(|round| round.calls.iter().any(|call| call == super::card::OPEN)) {
            round.opened = opens.next().unwrap_or_default();
        }
    });
    // The rounds' policy's decisions: live, or what it would have done.
    let decided = decisions.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
    if policy == super::settings::RoundsPolicy::ByTime || settings.shadows("rounds") {
        let live = policy == super::settings::RoundsPolicy::ByTime;
        record.with(|line| {
            line.shadow.push(super::line::Shadow {
                part: "rounds".into(),
                variant: "by_time".into(),
                hit: if live { "live" } else { "shadow" }.into(),
                decisions: decided,
                ..super::line::Shadow::default()
            })
        });
    }
    // The card in shadow: would it have been enough for what was called?
    if let Some((tools, tokens)) = shadow_card {
        record.with(|line| {
            let shadow = super::shadow::card("card", tokens, tools, line);
            line.shadow.push(shadow);
        });
    }
    tracing::debug!(target: "thinking", "the brain answered on {device} in {} ms", started.elapsed().as_millis());
    ended
}

/// The end of an answer: the work it touched, the reminder of a question nobody answered, the question it asks.
fn end_of_answer(
    parts: &Parts,
    out: &Out,
    said: String,
    work: Option<String>,
    asks: bool,
    remind: Option<String>,
) -> Ended {
    if let Some(id) = &work {
        out.send(work_of(parts, id));
    }
    let why = if remind.is_some() {
        Some(Why::Ready)
    } else if asks {
        Some(Why::Question)
    } else {
        None
    };
    if let Some(remind) = &remind {
        out.say(remind);
    }
    let outcome = ask_and_end(out, why, Expects::Free, said.clone());
    Ended { outcome, said, work, error: None }
}
