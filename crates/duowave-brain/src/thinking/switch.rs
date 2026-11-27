//! Which Thinking stands behind the door (docs/plans/thinking.md, §1 and §11): one door for the audio side, several
//! Thinkings behind it. Per phrase the switch picks one — by the agent the phrase is said to (a second agent's own
//! Thinking, stage M10), else by the setting `thinking.implementation` for the phrase's kind of device and voice or
//! text (ours, or the talk layer's stand-in) — and keeps whose each turn, hand call and notice is, so the news about
//! them reaches the one that made them. The audio side still sees one door (the contract's rule 1).
//!
//! - A notice comes from the Thinking that stands for its device's kind now: the others' notices of the same works
//!   are not said twice.
//! - One turn per device still holds (rule 6): the audio side stops the one running first, whoever's it is.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures::StreamExt;

use crate::door::{self, Events, Kind, News, Notices, Outside, Phrase, Told, Unasked, What};
use crate::settings::SettingsService;

use super::settings::{Implementation, Settings, Where};

/// Whose a turn, a call or a notice is.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Whose {
    Talk,
    Ours,
    /// A second agent's Thinking, by its agent's id.
    Agent(String),
}

/// The door's one side, with several Thinkings behind it.
pub struct Switch {
    talk: Arc<dyn door::Thinking>,
    ours: Arc<dyn door::Thinking>,
    /// Second agents' Thinkings (M10): `to.agent` → its Thinking.
    agents: Mutex<HashMap<String, Arc<dyn door::Thinking>>>,
    settings: Arc<SettingsService>,
    turns: Arc<Mutex<HashMap<String, Whose>>>,
    /// A device → its turn going on: a new phrase there stops it, whoever's it is (the contract's rule 6).
    devices: Arc<Mutex<HashMap<String, String>>>,
    /// The turns ended lately: `said` comes after their end.
    ended: Arc<Mutex<std::collections::VecDeque<(String, Whose)>>>,
    calls: Arc<Mutex<HashMap<String, Whose>>>,
    notices: Arc<Mutex<HashMap<String, Whose>>>,
}

/// How many ended turns the switch remembers for their `said`.
const ENDED_KEPT: usize = 32;

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The kind of the device a notice is for: the computer, or a station.
fn kind_of(device: Option<&str>) -> Kind {
    match device {
        Some(crate::talk::COMPUTER) | None => Kind::Computer,
        Some(_) => Kind::Station,
    }
}

impl Switch {
    pub fn new(talk: Arc<dyn door::Thinking>, ours: Arc<dyn door::Thinking>, settings: Arc<SettingsService>) -> Switch {
        Switch {
            talk,
            ours,
            agents: Mutex::new(HashMap::new()),
            settings,
            turns: Arc::new(Mutex::new(HashMap::new())),
            devices: Arc::new(Mutex::new(HashMap::new())),
            ended: Arc::new(Mutex::new(std::collections::VecDeque::new())),
            calls: Arc::new(Mutex::new(HashMap::new())),
            notices: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// A second agent's Thinking takes the phrases said to it (`to.agent`) from now on.
    pub fn add_agent(&self, agent: &str, thinking: Arc<dyn door::Thinking>) {
        lock(&self.agents).insert(agent.to_string(), thinking);
    }

    pub fn remove_agent(&self, agent: &str) {
        lock(&self.agents).remove(agent);
    }

    fn thinking(&self, whose: &Whose) -> Option<Arc<dyn door::Thinking>> {
        match whose {
            Whose::Talk => Some(self.talk.clone()),
            Whose::Ours => Some(self.ours.clone()),
            Whose::Agent(agent) => lock(&self.agents).get(agent).cloned(),
        }
    }

    /// Who takes this phrase.
    pub fn whose(&self, phrase: &Phrase) -> Whose {
        if let Some(agent) = phrase.to.agent.as_deref().filter(|agent| lock(&self.agents).contains_key(*agent)) {
            return Whose::Agent(agent.to_string());
        }
        let settings = Settings::of(&self.settings);
        match settings.implementation(Where::of(phrase.place.kind, phrase.budget.voice)) {
            Implementation::Talk => Whose::Talk,
            Implementation::Ours => Whose::Ours,
        }
    }
}

impl door::Thinking for Switch {
    fn phrase(&self, phrase: Phrase) -> Events {
        let whose = self.whose(&phrase);
        let Some(thinking) = self.thinking(&whose) else { return self.ours.phrase(phrase) };
        let turn = phrase.turn.clone();
        // One turn per device: the one going on there stops, whoever's it is.
        let before = lock(&self.devices).insert(phrase.place.device.clone(), turn.clone());
        if let Some(before) = before.filter(|before| *before != turn) {
            let going = lock(&self.turns).get(&before).cloned();
            if let Some(thinking) = going.and_then(|going| self.thinking(&going)) {
                thinking.tell(News::new(Told::Stop { turn: before, why: door::StopWhy::New, heard: None }));
            }
        }
        lock(&self.turns).insert(turn.clone(), whose.clone());
        let (turns, calls, ended) = (self.turns.clone(), self.calls.clone(), self.ended.clone());
        thinking
            .phrase(phrase)
            .inspect(move |event| match &event.what {
                What::Hands { call, .. } => {
                    lock(&calls).insert(call.clone(), whose.clone());
                }
                What::End { .. } => {
                    if let Some(whose) = lock(&turns).remove(&turn) {
                        let mut ended = lock(&ended);
                        ended.push_back((turn.clone(), whose));
                        while ended.len() > ENDED_KEPT {
                            ended.pop_front();
                        }
                    }
                }
                _ => {}
            })
            .boxed()
    }

    fn tell(&self, news: News) {
        let whose = match &news.what {
            Told::Stop { turn, .. } => lock(&self.turns).get(turn).cloned(),
            Told::Said { turn, .. } => lock(&self.ended)
                .iter()
                .rev()
                .find(|(ended, _)| ended == turn)
                .map(|(_, whose)| whose.clone())
                .or_else(|| lock(&self.turns).get(turn).cloned()),
            Told::HandsDone { call, .. } => lock(&self.calls).remove(call),
            Told::Delivered { notice: Some(notice), .. } => lock(&self.notices).get(notice).cloned(),
            Told::NoAnswer { .. } | Told::Delivered { notice: None, .. } | Told::Unknown => None,
        };
        match whose.and_then(|whose| self.thinking(&whose)) {
            Some(thinking) => thinking.tell(news),
            // Nobody known: every Thinking hears it (a question's window, a notice from before a restart).
            None => {
                self.talk.tell(news.clone());
                self.ours.tell(news.clone());
                let agents: Vec<_> = lock(&self.agents).values().cloned().collect();
                for agent in agents {
                    agent.tell(news.clone());
                }
            }
        }
    }

    fn notices(&self) -> Notices {
        let (talk, ours) = (self.talk.notices(), self.ours.notices());
        let tagged =
            futures::stream::select(talk.map(|outside| (Whose::Talk, outside)), ours.map(|o| (Whose::Ours, o)));
        let agents: Vec<_> = lock(&self.agents).iter().map(|(id, thinking)| (id.clone(), thinking.clone())).collect();
        let mut streams: Vec<futures::stream::BoxStream<'static, (Whose, Outside)>> = vec![tagged.boxed()];
        for (id, thinking) in agents {
            streams.push(
                thinking
                    .notices()
                    .map(move |mut outside| {
                        // An agent's notice is said in its voice: its `as`.
                        if let Unasked::Notice(notice) = &mut outside.what {
                            notice.speaker.get_or_insert_with(|| id.clone());
                        }
                        (Whose::Agent(id.clone()), outside)
                    })
                    .boxed(),
            );
        }
        let me =
            SwitchView { settings: self.settings.clone(), notices: self.notices.clone(), calls: self.calls.clone() };
        futures::stream::select_all(streams)
            .filter_map(move |(whose, outside)| {
                let keep = me.keep(&whose, &outside);
                async move { keep.then_some(outside) }
            })
            .boxed()
    }
}

/// What the notices' filter needs of the switch, without holding the switch.
struct SwitchView {
    settings: Arc<SettingsService>,
    notices: Arc<Mutex<HashMap<String, Whose>>>,
    calls: Arc<Mutex<HashMap<String, Whose>>>,
}

impl SwitchView {
    /// Who says the notices of a kind of device now.
    fn speaks_for(&self, kind: Kind) -> Whose {
        match Settings::of(&self.settings).implementation(Where::of(kind, true)) {
            Implementation::Talk => Whose::Talk,
            Implementation::Ours => Whose::Ours,
        }
    }

    /// A notice is said when its Thinking stands for its device's kind now; an agent's always. A withdrawal goes from
    /// the one whose the notice was; of a notice nobody knows (from before a restart), from the one that speaks now.
    fn keep(&self, whose: &Whose, outside: &Outside) -> bool {
        match &outside.what {
            Unasked::Notice(notice) => {
                let kind = kind_of(notice.to.as_ref().and_then(|hint| hint.device.as_deref()));
                let keep = matches!(whose, Whose::Agent(_)) || *whose == self.speaks_for(kind);
                if keep {
                    lock(&self.notices).insert(notice.notice.clone(), whose.clone());
                }
                keep
            }
            Unasked::Withdraw { notice } => {
                let known = lock(&self.notices).get(notice).cloned();
                match known {
                    Some(known) => known == *whose,
                    None => matches!(whose, Whose::Agent(_)) || *whose == self.speaks_for(Kind::Computer),
                }
            }
            Unasked::Hands { call, .. } => {
                lock(&self.calls).insert(call.clone(), whose.clone());
                true
            }
            Unasked::Unknown => false,
        }
    }
}
