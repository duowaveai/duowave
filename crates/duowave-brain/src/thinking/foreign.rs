//! A second agent's Thinking (docs/plans/thinking.md, §11; own-agent.md): an own agent — Hermes, OpenClaw — behind the
//! same door. The adapter turns a phrase into the agent's request and its answer into `say` and `end`; the switch
//! (switch.rs) sends it the phrases said to it (`to.agent`) and every event of its turns carries its `as`. It reaches
//! our tools and notices through the home's line (`/mcp/home`), and the devices only through the door's `hands`,
//! which is the agent's own business on its side.

use std::sync::Arc;

use futures::future::BoxFuture;
use futures::StreamExt;

use crate::door::{self, Event, Events, Failure, News, Notices, Outcome, Phrase, Told, What};

/// What a foreign agent is asked: the phrase, as the door gives it; its answer is its words.
pub trait ForeignAgent: Send + Sync {
    fn ask(&self, phrase: &Phrase) -> BoxFuture<'static, anyhow::Result<String>>;
    /// A turn of it was stopped.
    fn stop(&self, _turn: &str) {}
}

/// The adapter of a foreign agent behind the door.
pub struct Foreign {
    pub id: String,
    pub agent: Arc<dyn ForeignAgent>,
}

impl door::Thinking for Foreign {
    fn phrase(&self, phrase: Phrase) -> Events {
        let (id, agent) = (self.id.clone(), self.agent.clone());
        let turn = phrase.turn.clone();
        futures::stream::once(async move {
            let answer = agent.ask(&phrase).await;
            let event = |what: What| Event::new(&turn, Some(&id), what);
            match answer {
                Ok(text) => vec![
                    event(What::Say { text: text.clone() }),
                    event(What::End { outcome: Outcome::Answered, reason: None, said: text, ms: 0, error: None }),
                ],
                Err(error) => vec![event(What::End {
                    outcome: Outcome::Failed,
                    reason: Some(Failure::Error),
                    said: String::new(),
                    ms: 0,
                    error: Some(format!("{error:#}")),
                })],
            }
        })
        .flat_map(futures::stream::iter)
        .boxed()
    }

    fn tell(&self, news: News) {
        if let Told::Stop { turn, .. } = news.what {
            self.agent.stop(&turn);
        }
    }

    fn notices(&self) -> Notices {
        futures::stream::empty().boxed()
    }
}
