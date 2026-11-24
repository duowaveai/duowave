//! «Где сейчас» (docs/plans/home-and-helpers.md, «Помощники»): who gives each ability now — hearing, the model, the
//! voice, messages, apps, the history — and why not the first. One chain per ability: the helpers that could give it,
//! in the list's order (the regulator and liveness are judged, not filtered, so the reason can name them), then this
//! gateway's own road, then Yandex where it applies. In use is the first that passes; `why` says what stopped the
//! first. The model's row is «Мозг»'s own answer (stage 4). On a computer that takes its own parts from itself, its
//! built-in helper («Этот компьютер») is the first road of hearing and voice (stage 6); the clouds' roads go under the
//! regulator on every gateway.

use std::sync::Arc;

use serde::Serialize;

use super::contract::Ability;
use super::{reach_of, Helpers, Parts, Reach};
use crate::config::{ModelProfile, TtsProvider};
use crate::core::availability::State;
use crate::operations::networks::is_local_url;
use crate::placement::service::Role;
use crate::settings::definitions::PanelMode;

/// Who gives an ability now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum By {
    /// A helper of the list.
    Helper,
    /// This gateway itself.
    Here,
    /// A cloud by keys.
    Cloud,
    Yandex,
    /// Nobody.
    None,
}

/// One ability's row.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NowRow {
    pub ability: Ability,
    pub by: By,
    /// The helper's entry id, when a helper gives it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub helper: Option<String>,
    /// For people: the helper's, the cloud's, the model's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reach: Option<Reach>,
    /// The road in use is the first of its chain.
    pub first: bool,
    /// Why not the first, for people.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

/// The abilities shown, in their order: слух, мозг, голос, сообщения, приложения, история.
pub const SHOWN: [Ability; 6] =
    [Ability::Hearing, Ability::Model, Ability::Voice, Ability::Messages, Ability::Apps, Ability::History];

/// One road of a chain, and what stops it now (None — it gives).
pub struct Road {
    pub by: By,
    pub helper: Option<String>,
    pub name: Option<String>,
    pub reach: Option<Reach>,
    pub problem: Option<String>,
}

impl Road {
    pub fn of(by: By, name: Option<String>, reach: Option<Reach>, problem: Option<String>) -> Road {
        Road { by, helper: None, name, reach, problem }
    }
}

/// The row of a chain: its first road that gives, and why the first did not.
pub fn row(ability: Ability, roads: Vec<Road>) -> NowRow {
    let why = roads.first().and_then(|road| road.problem.clone());
    match roads.into_iter().enumerate().find(|(_, road)| road.problem.is_none()) {
        Some((at, road)) => NowRow {
            ability,
            by: road.by,
            helper: road.helper,
            name: road.name,
            reach: road.reach,
            first: at == 0,
            why: (at > 0).then_some(why).flatten(),
        },
        None => NowRow { ability, by: By::None, helper: None, name: None, reach: None, first: false, why },
    }
}

/// Every shown ability's row.
pub fn rows(parts: &Parts) -> Vec<NowRow> {
    let station = parts.placement.role() == Role::Station;
    let yandex_allowed = parts.modes.current() != PanelMode::Own;
    let yandex = || {
        let problem = (!yandex_allowed).then(|| crate::core::tx("в режиме «Ваш ИИ» Яндекс выключен"));
        Road::of(By::Yandex, None, None, problem)
    };
    let helpers = &parts.helpers;
    // This computer's own parts, when it takes them from itself (a computer that is not in the station role): first.
    let own = helpers.own.get();
    let ours = |ability: Ability| own.is_some_and(|own| own.gives_here(ability));
    let silent = |ability: Ability| own.is_some_and(|own| own.silent(ability));
    SHOWN
        .into_iter()
        .map(|ability| {
            if ability == Ability::Model {
                return model_row(&parts.brain, &parts.helpers, &parts.modes, station);
            }
            // Hearing and voice go the helpers' way on a computer too (stage 6): its own helpers after itself.
            let through = station
                || ability == Ability::Messages
                || (own.is_some() && matches!(ability, Ability::Hearing | Ability::Voice));
            let mut roads = if through { helper_roads(helpers, ability) } else { Vec::new() };
            match ability {
                Ability::Hearing => {
                    if ours(Ability::Hearing) {
                        let problem =
                            silent(Ability::Hearing).then(|| crate::tr!("слух на этом компьютере не отвечает"));
                        roads.insert(0, built_in(problem));
                    }
                    let asr = &parts.config.speech.asr;
                    if asr.enabled && !is_local_url(Some(&asr.base_url)) {
                        // The .env's cloud hearing goes under the regulator, a computer's too (its chain, stage 6).
                        roads.push(cloud(helpers, &asr.base_url));
                    }
                    roads.push(yandex());
                }
                Ability::Voice => {
                    // Alice's voice is the first road only when a person chose it; when ours has nobody to speak
                    // with (the helpers away, the cloud kept in) she stands in, and the chain says why.
                    if parts.voice.chosen_provider() == TtsProvider::Yandex && yandex_allowed {
                        roads = vec![yandex()];
                    } else {
                        if ours(Ability::Voice) {
                            let problem =
                                silent(Ability::Voice).then(|| crate::tr!("голос на этом компьютере не отвечает"));
                            roads.insert(0, built_in(problem));
                        }
                        if let Some(own) = parts.voice.own_speech() {
                            let url = own.base_url();
                            if !is_local_url(Some(&url)) {
                                roads.push(cloud(helpers, &url));
                            } else if !ours(Ability::Voice) {
                                // Our voice at home; on a computer that takes it from itself, the built-in road is it.
                                // A server elsewhere in the home is named by its host: a speaker runs none itself.
                                roads.push(Road::of(By::Here, home_name(&url), Some(Reach::Home), None));
                            }
                        }
                        if station {
                            roads.push(yandex());
                        }
                    }
                }
                Ability::Messages => {
                    if own_bot(parts) {
                        roads.insert(0, Road::of(By::Here, None, Some(Reach::Home), None));
                    }
                }
                _ => roads.push(Road::of(By::Here, None, Some(Reach::Home), None)),
            }
            row(ability, roads)
        })
        .collect()
}

/// A cloud's road: the regulator keeps it out unless it lets foreign roads out.
fn cloud(helpers: &Helpers, url: &str) -> Road {
    let problem = (!helpers.lets(Reach::Foreign)).then(|| crate::tr!("Регулятор не пускает облако"));
    Road::of(By::Cloud, cloud_name(url), Some(Reach::Foreign), problem)
}

/// The built-in helper's road (stage 6): «Этот компьютер», at home — giving, unless its server was found closed.
fn built_in(problem: Option<String>) -> Road {
    Road {
        by: By::Helper,
        helper: Some(super::SELF.to_string()),
        name: Some(crate::tr!("Этот компьютер")),
        reach: Some(Reach::Home),
        problem,
    }
}

/// Why no helper gives the ability now, for people and for «Мозг» (clean-path check 19.11: an app on the Турбо was
/// said to be «off» whatever stopped it): what stops the first helper that would — switched off, «Не брать у него…»,
/// the regulator, no key, off the line (asleep, off or without a network), not ready —, or that the home has none
/// that gives it. None when one gives it now.
pub fn why_not(helpers: &Helpers, ability: Ability) -> Option<String> {
    let snap = helpers.snap();
    // What stops the first helper that would give it; else the first that is on the line and does not.
    let (mut first, mut refuses) = (None, None);
    for entry in snap.list.iter().filter(|entry| !helpers.is_self(entry) && helpers.twin_of(entry, &snap).is_none()) {
        let problem = if !entry.enabled {
            crate::tr!("{name} выключен в «Доме»", name = entry.name)
        } else {
            let Some(remote) = helpers.remote(entry) else {
                first.get_or_insert_with(|| crate::tr!("у {name} нет ключа", name = entry.name));
                continue;
            };
            if !remote.declares(ability) {
                refuses.get_or_insert_with(|| crate::tr!("{name} не даёт этого вашему дому", name = entry.name));
                continue;
            }
            if entry.without.contains(&ability) {
                crate::tr!("у {name} это не берётся («Не брать у него…»)", name = entry.name)
            } else if !snap.away.lets(reach_of(entry)) {
                crate::tr!("Регулятор не пускает: {name}", name = entry.name)
            } else if !remote.up() {
                crate::tr!("{name} не на связи — спит, выключен или без сети", name = entry.name)
            } else if !remote.gives(ability) {
                crate::tr!("{name} пока не готов", name = entry.name)
            } else {
                return None;
            }
        };
        first.get_or_insert(problem);
    }
    Some(first.or(refuses).unwrap_or_else(|| crate::tr!("в доме нет Турбо, который это даёт")))
}

/// The name of the helper that gives the ability now, as people see it in the list («Mac»).
pub fn giver_name(helpers: &Helpers, ability: Ability) -> Option<String> {
    let remote = helpers.pick_remote(ability)?;
    helpers.entry(remote.id()).map(|entry| entry.name)
}

/// The helpers that could give the ability, in order, each with what stops it now: switched on, not this gateway,
/// not the same as an earlier one on the line, not switched off for it, keyed, declaring it. A sticky one (the
/// history) has only its first.
pub fn helper_roads(helpers: &Helpers, ability: Ability) -> Vec<Road> {
    let snap = helpers.snap();
    let candidates: Vec<_> = snap
        .list
        .iter()
        .filter(|entry| entry.enabled && !helpers.is_self(entry) && helpers.twin_of(entry, &snap).is_none())
        .filter_map(|entry| helpers.remote(entry).map(|remote| (entry, remote)))
        .filter(|(_, remote)| remote.declares(ability))
        .collect();
    let candidates = match ability.sticky() {
        true => candidates.into_iter().take(1).collect(),
        false => candidates,
    };
    candidates
        .into_iter()
        .filter(|(entry, _)| !entry.without.contains(&ability))
        .map(|(entry, remote)| {
            let reach = reach_of(entry);
            let problem = if !snap.away.lets(reach) {
                Some(crate::tr!("Регулятор не пускает: {name}", name = entry.name))
            } else if !remote.up() {
                Some(crate::tr!("{name} не на связи", name = entry.name))
            } else if !remote.gives(ability) {
                // No bot by the registry's rule (`no_bot`): its contacts answer so, or its contract says so.
                match ability == Ability::Messages && remote.no_bot() {
                    true => Some(crate::tr!("{name}: не настроен бот Telegram", name = entry.name)),
                    false => Some(crate::tr!("{name} пока не готов", name = entry.name)),
                }
            } else {
                None
            };
            Road {
                by: By::Helper,
                helper: Some(entry.id.clone()),
                name: Some(entry.name.clone()),
                reach: Some(reach),
                problem,
            }
        })
        .collect()
}

/// The model's row: «Мозг»'s own answer for the fast profile (dialog/brain/brain_service.rs) — the chosen model;
/// another in its place, the backup or the next helper, and why; or none may be asked: then Alice on a station whose
/// mode lets Yandex (the owner, 15.11: in «Вместе» such a question goes to her), else nobody — the words say why.
fn model_row(
    brain: &Arc<crate::dialog::brain::BrainService>,
    helpers: &Arc<crate::helpers::Helpers>,
    modes: &Arc<crate::ear::mode::ModeService>,
    station: bool,
) -> NowRow {
    let now = brain.now(ModelProfile::Fast);
    let (by, name) = match now.helper.as_deref() {
        // A model server of this computer (stage 6): the built-in helper's row.
        Some(super::SELF) => (By::Helper, Some(crate::tr!("Этот компьютер"))),
        Some(id) => (By::Helper, helpers.entry(id).map(|entry| entry.name).or(now.title)),
        None if now.reach == Some(Reach::Foreign) => (By::Cloud, now.title),
        None => (By::Here, now.title),
    };
    let ability = Ability::Model;
    match now.state {
        State::Off => {
            // The words say why none may be asked; with no model at all there are none, and nobody answers.
            let alice = now.reason.is_some() && station && modes.yandex_allowed();
            let by = if alice { By::Yandex } else { By::None };
            NowRow { ability, by, helper: None, name: None, reach: None, first: false, why: now.reason }
        }
        state => NowRow {
            ability,
            by,
            helper: now.helper,
            name,
            reach: now.reach,
            first: state == State::Ready,
            why: now.reason,
        },
    }
}

/// This computer's own Telegram bot is set up.
fn own_bot(parts: &Parts) -> bool {
    (parts.own_bot)()
}

/// A server at home that is not this machine, for people: its host. One at a loopback address is this gateway's own
/// and goes unnamed.
pub fn home_name(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let own = match parsed.host()? {
        url::Host::Ipv4(ip) => ip.is_loopback(),
        url::Host::Ipv6(ip) => ip.is_loopback(),
        url::Host::Domain(name) => name.eq_ignore_ascii_case("localhost"),
    };
    (!own).then(|| parsed.host_str().unwrap_or_default().to_string())
}

/// A cloud by its address, for people: the known ones by name, else the host without its `api.`.
pub fn cloud_name(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let host = parsed.host_str()?;
    Some(match host {
        "api.openai.com" => "OpenAI".to_string(),
        "api.groq.com" => "Groq".to_string(),
        "api.elevenlabs.io" => "ElevenLabs".to_string(),
        other => other.strip_prefix("api.").unwrap_or(other).to_string(),
    })
}
