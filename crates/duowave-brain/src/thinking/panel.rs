//! The panel's talk through the door (docs/plans/thinking.md, M4, D1): a phrase typed in the panel's chat, or said
//! after its «Сказать», goes to `OurThinking` as a phrase of the computer — `opened: text` for a typed one,
//! `budget.voice` when the computer will say the answer — and its events come back as the talk's (`TalkEvent`), so the
//! panel's stream stays as it is. Typed phrases go first: a round there costs no silence.

use futures::StreamExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::core::identity::Principal;
use crate::core::requests::Place as Where;
use crate::door::{
    self, Budget, By, Context, Heard, Kind, News, Opened, Outcome, Phrase, Place, StopWhy, To, Told, What, Who,
};
use crate::talk::{Activator, Talk, TalkEvent, Turn, COMPUTER};

use super::OurThinking;

/// The panel's phrase in the door's words.
pub fn phrase_of(principal: &Principal, turn: &Turn) -> Phrase {
    let opened = match turn.origin.activator {
        Activator::Text => Opened::Text,
        Activator::Answer => Opened::Window,
        Activator::Word => Opened::Word,
        _ => Opened::Button,
    };
    let answer_to = (turn.origin.activator == Activator::Answer)
        .then(|| turn.origin.why.map(|why| why.as_str().to_string()))
        .flatten();
    Phrase {
        v: door::V,
        turn: format!("panel-{}", crate::utils::ids::uuid()),
        text: turn.text.clone(),
        lang: None,
        alts: Vec::new(),
        who: Who { person: Some(principal.id.clone()), name: Some(principal.name.clone()), sure: door::Sure::Known },
        to: To { agent: turn.origin.character.clone(), by: By::Chat, name: turn.origin.name.clone() },
        place: Place { device: COMPUTER.to_string(), room: None, kind: Kind::Computer, screen: true, speaker: true },
        hands: Vec::new(),
        opened,
        context: Context { answer_to, fresh: turn.fresh, ..Context::default() },
        budget: Budget { voice: turn.voice, first_word_ms: None, limit_ms: None },
        heard: turn.heard.as_ref().map(|heard| Heard {
            by: if heard.by == Where::Cloud { "cloud" } else { "home" }.to_string(),
            ms: heard.ms,
            model: heard.what.clone(),
        }),
        at: Some(crate::utils::time::now_ms()),
    }
}

/// The panel's phrase through ours: its events as the talk's. Cancelling the talk (the panel went away) stops the turn.
pub fn talk(thinking: &std::sync::Arc<OurThinking>, principal: &Principal, turn: &Turn) -> Talk {
    let phrase = phrase_of(principal, turn);
    let id = phrase.turn.clone();
    let mut events = door::Thinking::phrase(&**thinking, phrase);
    let (sender, receiver) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let (stop, thinking) = (cancel.clone(), thinking.clone());
    tokio::spawn(async move {
        let (mut by, mut work, mut asks) = ("home", None::<String>, false);
        loop {
            let event = tokio::select! {
                _ = stop.cancelled() => {
                    door::Thinking::tell(&*thinking, News::new(Told::Stop { turn: id.clone(), why: StopWhy::Gone, heard: None }));
                    return;
                }
                event = events.next() => event,
            };
            let Some(event) = event else { return };
            let talk = match event.what {
                What::Begun { request, by: place } => {
                    by = if place == "cloud" { "cloud" } else { "home" };
                    TalkEvent::Start { request, by }
                }
                What::Say { text } => TalkEvent::Phrase { text },
                What::Wait { name: Some(name), .. } => TalkEvent::Tool { name },
                What::Work { work: id, .. } => {
                    work = Some(id);
                    continue;
                }
                What::Ask { .. } => {
                    asks = true;
                    continue;
                }
                What::End { outcome: Outcome::Failed, error, said, .. } => {
                    let _ = sender.send(TalkEvent::Error { message: error.unwrap_or(said) });
                    return;
                }
                What::End { outcome, said, ms, .. } => {
                    let dropped = outcome == Outcome::Dropped;
                    let _ = sender.send(TalkEvent::Done { said, by, ms, work, asks, remind: None, dropped });
                    return;
                }
                _ => continue,
            };
            if sender.send(talk).is_err() {
                stop.cancel();
            }
        }
    });
    Talk { events: receiver, cancel }
}
