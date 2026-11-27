//! `/home/v1/systems/call` (docs/plans/work-systems.md, stage 9; block В of docs/plans/router-and-works.md): this
//! computer's work systems for the hub — a station — over the home link, by the helpers' ability `works`. One letter,
//! one answer, the contract's own messages as JSON: `{verb, system?, work?, …}`.
//!
//! - `systems` — every system with what it can, whether it is ready and there, and this computer's work folders with
//!   who does their works by its own setup (router/who.rs);
//! - `places`, `start`, `list`, `status`, `tell`, `stop`, `answer`, `result`, `open` — the contract's verbs of one
//!   system; a refusal comes back as `{refused: {why, cannot}}`;
//! - `events` — the registry's journal after a number (`since`), with the last number.

use std::sync::Arc;

use axum::routing::post;
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use super::serve::{allowed, Door, Needs};
use crate::helpers::contract::Ability;
use crate::platform::http::{ok, ApiError, ApiResult, Body, Caller, Provides, RouteState, Uses};
use crate::settings::SettingsService;
use crate::talk::system::{Refusal, Reply, Start, When, WorkSystem, DUOWAVE};
use crate::talk::systems::Systems;

pub const PATH: &str = "/home/v1/systems/call";

/// A letter of the hub.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Letter {
    pub verb: String,
    #[serde(default)]
    pub system: Option<String>,
    #[serde(default)]
    pub work: Option<String>,
    #[serde(default)]
    pub start: Option<Start>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub when: Option<When>,
    #[serde(default)]
    pub question: Option<String>,
    #[serde(default)]
    pub reply: Option<Reply>,
    #[serde(default)]
    pub since: Option<u64>,
}

pub fn router<S>() -> Router<S>
where
    S: RouteState + Provides<Door> + Provides<Arc<Systems>> + Provides<Arc<SettingsService>>,
{
    Router::new().route(PATH, post(call))
}

async fn call(
    Uses(door): Uses<Door>,
    Uses(systems): Uses<Arc<Systems>>,
    Uses(settings): Uses<Arc<SettingsService>>,
    caller: Caller,
    Body(letter): Body<Letter>,
) -> ApiResult<Value> {
    allowed(&door, &caller, Needs::Ability(Ability::Works))?;
    answer(&systems, &settings, letter).await.map(ok)
}

/// A refusal as the hub reads it back.
fn refused(refusal: Refusal) -> Value {
    json!({ "refused": refusal })
}

/// The answer to a letter: what the routes give, apart from the door's check.
pub async fn answer(systems: &Systems, settings: &SettingsService, letter: Letter) -> Result<Value, ApiError> {
    let system = || -> Result<Arc<dyn WorkSystem>, ApiError> {
        let id = letter.system.as_deref().unwrap_or_default();
        systems.get(id).ok_or_else(|| ApiError::not_found(crate::tr!("Такой системы нет.")))
    };
    let work = || {
        letter.work.clone().ok_or_else(|| ApiError::field("work", crate::tr!("Нужно поле {field}.", field = "work")))
    };
    let done = |result: Result<(), Refusal>| match result {
        Ok(()) => json!({}),
        Err(refusal) => refused(refusal),
    };
    Ok(match letter.verb.as_str() {
        "systems" => {
            let mut list = Vec::new();
            for system in systems.all() {
                let ready = system.ready().await;
                list.push(json!({ "about": system.about(), "can": system.can(), "ready": ready, "present": system.present() }));
            }
            let setup = settings.get(&crate::router::who::WORK_SYSTEMS, None).unwrap_or_default();
            let places = match systems.get(DUOWAVE) {
                Some(ours) => ours.places().await,
                None => Vec::new(),
            };
            let folders: Vec<Value> = places
                .into_iter()
                .map(|place| {
                    let system = crate::router::who::who_gets(&setup, None, Some(&place.id)).to_string();
                    json!({ "folder": place.id, "name": place.title, "path": place.path, "system": system })
                })
                .collect();
            json!({ "systems": list, "folders": folders })
        }
        "places" => json!(system()?.places().await),
        "start" => {
            let start = letter
                .start
                .clone()
                .ok_or_else(|| ApiError::field("start", crate::tr!("Нужно поле {field}.", field = "start")))?;
            match system()?.start(start).await {
                Ok(seen) => json!(seen),
                Err(refusal) => refused(refusal),
            }
        }
        "list" => json!(system()?.list().await),
        "status" => json!(system()?.status(&work()?).await),
        "tell" => {
            let text = letter.text.clone().unwrap_or_default();
            done(system()?.tell(&work()?, &text, letter.when.unwrap_or(When::After)).await)
        }
        "stop" => done(system()?.stop(&work()?).await),
        "answer" => {
            let question = letter.question.clone().unwrap_or_default();
            let reply = letter.reply.clone().unwrap_or(Reply::No);
            done(system()?.answer(&work()?, &question, reply).await)
        }
        "result" => json!(system()?.result(&work()?).await),
        "open" => json!(system()?.open(&work()?)),
        "events" => {
            let (events, last) = systems.journal_since(letter.since.unwrap_or(0));
            json!({ "events": events, "last": last })
        }
        other => return Err(ApiError::bad_request(crate::tr!("Нет такого глагола: {verb}.", verb = other))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::events::EventBus;
    use crate::storage::Db;
    use crate::talk::system::stand_in::StandIn;

    fn letter(value: Value) -> Letter {
        serde_json::from_value(value).unwrap()
    }

    /// The whole way of stage 9, with the home link a call in memory: a phrase said to a hub with no folders of its
    /// own begins the work on the Турбо, in the system its setup gives the folder (bb), in bb's project at the
    /// folder's path; bb's end comes back to the hub by the events, and the hub's work ends with it.
    #[tokio::test]
    async fn a_phrase_said_to_the_hub_begins_the_work_in_bb_on_the_turbo_and_its_end_comes_back() {
        use crate::talk::system::SystemState;
        // The Турбо: our runner with the folder «Сайт» (as a stand-in), bb, and bb as the folder's system.
        let computer = Systems::new();
        computer.add(Arc::new(StandIn::full(DUOWAVE)));
        let bb = Arc::new(StandIn::full("bb"));
        computer.add(bb.clone());
        let settings = Arc::new(SettingsService::new(Arc::new(Db::memory()), EventBus::new()));
        let mut setup = crate::router::who::WorkSystemsSetup::default();
        setup.folders.insert("p1".into(), "bb".into());
        settings.set(&crate::router::who::WORK_SYSTEMS, &setup, None).unwrap();
        // The hub: no folders, the Турбо's systems over the link.
        let hub = crate::talk::TalkLayer::new(None, EventBus::new());
        let hub_systems = Systems::new();
        let executor = crate::talk::systems::SystemExecutor::new(&hub, &hub_systems, false);
        let (to, given) = (computer.clone(), settings.clone());
        let call: crate::helpers::remote_systems::Call = Arc::new(move |value: Value| {
            let (to, given) = (to.clone(), given.clone());
            Box::pin(async move {
                let letter: Letter = serde_json::from_value(value).map_err(|error| error.to_string())?;
                answer(&to, &given, letter).await.map_err(|error| error.to_string())
            })
        });
        let source: crate::helpers::remote_systems::Source = Arc::new(move || vec![("mac".to_string(), call.clone())]);
        let remote = crate::helpers::remote_systems::RemoteSystems::new(&hub_systems, source);
        let folders = remote.clone();
        executor.set_remote_folders(Arc::new(move || folders.folders()));
        remote.look().await;
        assert!(hub_systems.get("bb@mac").is_some(), "the Турбо's bb is a system of the hub");
        assert_eq!(remote.folders()[0].system, "bb@mac");

        let mut admin = crate::talk::origin::Origin::computer(crate::talk::origin::Activator::Button);
        admin.role = Some(crate::core::identity::Role::Admin);
        let said = hub.start_folder_said(admin, "сделай тёмную тему", Some("сайт"), None).unwrap();
        assert!(said.starts_with("Взяла в работу в папке «Сайт», у Stand-in bb"), "{said}");
        let mut begun = Vec::new();
        for _ in 0..200 {
            begun = bb.list().await;
            if !begun.is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(begun[0].place.as_deref(), Some("p1"), "bb's project at the folder's path");
        let work = hub.works.all(None, None).into_iter().find(|work| work.system.as_deref() == Some("bb@mac")).unwrap();
        bb.end(&begun[0].id, SystemState::Ready, "Тёмная тема готова.");
        remote.look().await;
        for _ in 0..200 {
            if hub.works.get(&work.id).is_some_and(|work| !work.going()) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let ended = hub.works.get(&work.id).unwrap();
        assert!(!ended.going(), "{:?}", ended.state);
        assert!(ended.result.map(|result| result.line).unwrap_or_default().contains("Тёмная тема готова"));
    }

    /// The hub's letters reach the systems here as the contract's verbs; its events come back from the journal.
    #[tokio::test]
    async fn the_hubs_letters_reach_the_systems_here_and_the_events_come_back() {
        let systems = Systems::new();
        let bb = Arc::new(StandIn::full("bb"));
        systems.add(bb.clone());
        let settings = SettingsService::new(Arc::new(Db::memory()), EventBus::new());
        let listed = answer(&systems, &settings, letter(json!({ "verb": "systems" }))).await.unwrap();
        assert_eq!(listed["systems"][0]["about"]["id"], "bb");
        assert_eq!(listed["systems"][0]["can"]["start"], true);
        let start =
            json!({ "verb": "start", "system": "bb", "start": { "task": "сделай тёмную тему", "place": "p1" } });
        let seen = answer(&systems, &settings, letter(start)).await.unwrap();
        let work = seen["id"].as_str().unwrap().to_string();
        let told = json!({ "verb": "tell", "system": "bb", "work": work, "text": "и футер", "when": "after" });
        assert_eq!(answer(&systems, &settings, letter(told)).await.unwrap(), json!({}));
        assert_eq!(bb.told(&work), ["и футер"]);
        let events = answer(&systems, &settings, letter(json!({ "verb": "events", "since": 0 }))).await.unwrap();
        assert_eq!(events["events"][0]["system"], "bb");
        assert_eq!(events["events"][0]["event"]["event"], "began");
        let last = events["last"].as_u64().unwrap();
        let none = answer(&systems, &settings, letter(json!({ "verb": "events", "since": last }))).await.unwrap();
        assert_eq!(none["events"], json!([]));
        let refused =
            answer(&systems, &settings, letter(json!({ "verb": "stop", "system": "bb", "work": "nothing" }))).await;
        assert!(refused.unwrap()["refused"]["why"].is_string());
        assert!(answer(&systems, &settings, letter(json!({ "verb": "status", "system": "t9", "work": "w" })))
            .await
            .is_err());
    }
}
