//! The talk's routes.
//!
//! - `GET /api/v1/talk/input` → `{input, before?}`, `PUT … {input}` — «Как говорить с DuoWave». Anyone who sees the
//!   station reads the choice; whoever may switch the mode makes it — the choice switches the mode.
//! - `POST /api/v1/talk {text, via?, heard?, voice?, fresh?}` — a phrase into the talk. With
//!   `Accept: text/event-stream` the answer is a stream of events (`start`, `phrase`, `tool`, `done`, `error`), and
//!   closing it cancels the answer; without it — `{said, by}` once the answer is whole. `via` is what opened the
//!   phrase (`button`, `answer`, `text`); `heard` says where it was recognized (`home`, `cloud`, or
//!   `{by, ms?, what?}`); `voice` — the computer will say the answer aloud; `fresh` — the talk starts over. The right
//!   is the one a conversation by voice needs (`use:chat`).
//! - `POST /api/v1/talk/speech {text}` → `audio/wav`, 16-bit mono: one phrase of the answer in the household's own
//!   voice, for the computer's speakers. Alice's voice cannot speak here — 409 in words; so does a voice server
//!   that is not up yet. A phrase of up to 600 characters; the same right.
//! - `GET /api/v1/talk/now?device=` → `{device, text}`: what the brain knows now on a device (the computer without
//!   one) — the block «Сейчас» as the model gets it; empty when nothing goes on. The right to talk (`use:chat`).
//! - `GET /api/v1/works?device=&state=&group=` → the works, the newest first; `GET /api/v1/works/{id}` → one;
//!   `GET /api/v1/works?waiting=1` → «Ждёт вас»: the works whose question waits for the person
//!   (docs/plans/audio-interface-2.md, «К3»). The right to use the apps (`use:plugins`). For the page «Работы»
//!   (docs/plans/router-and-works.md, decision 17) each work says its `group` — `going`, `queued`, `asking`, `recent`
//!   (`?group=going,asking` picks groups; the recent ones go by their end, the newest first) — why it waits in the
//!   queue (`wait`), what its executor is at (`doing`, `step`), and for the admin the model that answers it (`model`)
//!   and what its deals spent (`spent`).
//! - `GET /api/v1/works/{id}/deals` → `{deals}`: the router's deals of the work, the newest first. The admin's
//!   (`settings.edit`), as the router's own list.
//! - `POST /api/v1/works/{id}/said` — the computer said the work's line to its end (the panel's voice): its question
//!   opens for the answer the panel listens for. `POST /api/v1/works/{id}/notes {text}` — words added to it («вдогонку»,
//!   or its new version); `POST /api/v1/works/{id}/do {action}` — `status`, `details`, `show`, `desk`, `park`, `stop`,
//!   `continue` (a work in a folder that ended goes on in the agent's session), and `answer` with `{answer}` — `yes`,
//!   `fit`, `no`, `park`, `details`: the work's question answered by a button, as if said aloud (a work in a folder
//!   held by a plan's limit: `yes` waits, `no` stops). Each answers `{said}`, the phrase to say or show. The same
//!   right.
//! - `POST /api/v1/talk {…, why?}`: a phrase with `via: answer` is the closed window's — `why` the microphone opened
//!   by itself (`question`, `work`, `ready`; docs/plans/audio-interface-2.md, «К2»); what the window does not take
//!   answers `dropped`, and nothing keeps it.
//! - `POST /api/v1/talk` answers `{said, by, work?, asks?, remind?, dropped?}`: the work the phrase began or added to; whether
//!   the answer asks (the device's question is open); a work's question nobody answered, asked once more.

use std::convert::Infallible;
use std::sync::Arc;

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use futures::StreamExt;
use serde::Serialize;
use serde_json::Value;
use tokio_stream::wrappers::UnboundedReceiverStream;

use super::executor::Doing;
use super::ready::Answer;
use super::works::{Refused, Work, WorkState, APP};
use super::{input, Activator, Heard, Origin, TalkEvent, Turn, Why, COMPUTER, MAX_TEXT};
use crate::app::App;
use crate::core::identity::permissions::{use_of, SETTINGS_EDIT, STATION_MODE, STATION_VIEW};
use crate::core::requests::Place;
use crate::dialog::Channel;
use crate::placement::service::Role;
use crate::platform::http::{ok, ApiError, ApiResult, Body, Caller};
use crate::router::deals::DealsQuery;
use crate::settings::definitions::VoiceInput;
use crate::skills::capabilities::CapabilityName;
use crate::utils::time::now_ms;

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/v1/talk/input", get(voice_input).put(set_voice_input))
        .route("/api/v1/talk", post(talk))
        .route("/api/v1/talk/speech", post(speech))
        .route("/api/v1/talk/now", get(now))
        .route("/api/v1/works", get(works))
        .route("/api/v1/works/{id}", get(work))
        .route("/api/v1/works/{id}/said", post(work_said))
        .route("/api/v1/works/{id}/notes", post(work_notes))
        .route("/api/v1/works/{id}/do", post(work_do))
        .route("/api/v1/works/{id}/deals", get(work_deals))
}

/// What a work's action says.
#[derive(Debug, Serialize)]
struct WorkSaid {
    said: String,
}

async fn work_said(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<WorkItem> {
    caller.permitted(&use_of(CapabilityName::Plugins))?;
    let work = app.talk.said_on(&id, COMPUTER).ok_or_else(|| ApiError::not_found(crate::tr!("Такой работы нет.")))?;
    Ok(ok(Shelf::new(&app, &caller).item(work)))
}

/// The panel's person on the computer, typing.
fn typed(caller: &Caller) -> Origin {
    Origin::computer(Activator::Text).by(caller.principal().map(|principal| principal.name.clone()))
}

async fn work_notes(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<Value>,
) -> ApiResult<WorkSaid> {
    caller.permitted(&use_of(CapabilityName::Plugins))?;
    app.talk.works().get(&id).ok_or_else(|| ApiError::not_found(crate::tr!("Такой работы нет.")))?;
    let text = body.get("text").and_then(Value::as_str).unwrap_or_default();
    let pack = super::pack::of(app.talk.lang(COMPUTER));
    let said =
        app.talk.add(typed(&caller), text, Some(&id), false).await.unwrap_or_else(|refusal| refusal.for_person(pack));
    Ok(ok(WorkSaid { said }))
}

async fn work_do(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<Value>,
) -> ApiResult<WorkSaid> {
    caller.permitted(&use_of(CapabilityName::Plugins))?;
    app.talk.works().get(&id).ok_or_else(|| ApiError::not_found(crate::tr!("Такой работы нет.")))?;
    let action = body.get("action").and_then(Value::as_str).unwrap_or_default();
    let pack = super::pack::of(app.talk.lang(COMPUTER));
    if action == "answer" {
        let answer = match body.get("answer").and_then(Value::as_str).unwrap_or_default() {
            "yes" => Answer::Yes,
            "fit" => Answer::Fit,
            "no" => Answer::No,
            "park" => Answer::Park,
            "details" => Answer::Details,
            _ => return Err(ApiError::field("answer", crate::tr!("ответ — yes, fit, no, park или details"))),
        };
        let said = app
            .talk
            .answer_work(typed(&caller), &id, answer)
            .await
            .ok_or_else(|| ApiError::conflict(crate::tr!("Работа ничего не спрашивает.")))?;
        return Ok(ok(WorkSaid { said }));
    }
    let said =
        app.talk.act(&typed(&caller), action, Some(&id)).await.unwrap_or_else(|refusal| refusal.for_person(pack));
    Ok(ok(WorkSaid { said }))
}

/// The router's deals of a work, the newest first: those since it began — an older work with the same id is not it.
#[derive(Debug, Serialize)]
struct WorkDealList {
    deals: Vec<crate::router::Deal>,
}

async fn work_deals(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<WorkDealList> {
    caller.permitted(SETTINGS_EDIT)?;
    let work = app.talk.works().get(&id).ok_or_else(|| ApiError::not_found(crate::tr!("Такой работы нет.")))?;
    let asked = DealsQuery {
        work: Some(work.id.clone()),
        from: Some(work.started_at.saturating_sub(SINCE_SLACK_MS)),
        limit: Some(crate::router::deals::LIST_MAX),
        ..DealsQuery::default()
    };
    Ok(ok(WorkDealList { deals: app.router.deals.list(&asked)? }))
}

/// The block «Сейчас» as a device's brain gets it.
#[derive(Debug, Serialize)]
struct Now {
    device: String,
    text: String,
}

async fn now(
    State(app): State<Arc<App>>,
    caller: Caller,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult<Now> {
    caller.permitted(&use_of(CapabilityName::Chat))?;
    let device =
        query.get("device").map(|device| device.trim()).filter(|device| !device.is_empty()).unwrap_or(COMPUTER);
    let channel = if device == COMPUTER { Channel::Computer } else { Channel::Station };
    let lang = crate::memory::text::lang_of(&app.personas.system_prompt_for(None, channel).text);
    Ok(ok(Now { device: device.to_string(), text: app.talk.now(device, lang) }))
}

async fn works(
    State(app): State<Arc<App>>,
    caller: Caller,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult<Vec<WorkItem>> {
    caller.permitted(&use_of(CapabilityName::Plugins))?;
    let shelf = Shelf::new(&app, &caller);
    if query.get("waiting").is_some_and(|waiting| waiting == "1" || waiting == "true") {
        return Ok(ok(app.talk.waiting().into_iter().map(|work| shelf.item(work)).collect()));
    }
    let device = query.get("device").map(String::as_str).filter(|device| !device.is_empty());
    let state = match query.get("state").map(String::as_str).filter(|state| !state.is_empty()) {
        Some(state) => Some(
            WorkState::parse(state)
                .ok_or_else(|| ApiError::field("state", crate::tr!("нет такого состояния работы")))?,
        ),
        None => None,
    };
    let mut groups = Vec::new();
    for name in query.get("group").map(String::as_str).unwrap_or_default().split(',').map(str::trim) {
        if name.is_empty() {
            continue;
        }
        let group = Group::parse(name)
            .ok_or_else(|| ApiError::field("group", crate::tr!("группы работ — going, queued, asking, recent")))?;
        groups.push(group);
    }
    // The groups are picked before the rest is looked up: the deals are read only for the works shown.
    let mut items: Vec<WorkItem> = app
        .talk
        .works()
        .all(device, state)
        .into_iter()
        .filter(|work| groups.is_empty() || groups.contains(&shelf.group(work)))
        .map(|work| shelf.item(work))
        .collect();
    // The recent ones by their end: a work begun long ago may have ended a minute ago.
    if !groups.is_empty() && groups.iter().all(|group| *group == Group::Recent) {
        items.sort_by_key(|item| std::cmp::Reverse(item.work.ended_at.unwrap_or(item.work.updated_at)));
    }
    Ok(ok(items))
}

async fn work(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<WorkItem> {
    caller.permitted(&use_of(CapabilityName::Plugins))?;
    let work = app.talk.works().get(&id).ok_or_else(|| ApiError::not_found(crate::tr!("Такой работы нет.")))?;
    Ok(ok(Shelf::new(&app, &caller).item(work)))
}

/// A deal's `at` is when its answer ended: never before its work began. A second of slack for the clock's step.
const SINCE_SLACK_MS: u64 = 1000;

/// Where a work stands on the page «Работы» (docs/plans/router-and-works.md, decision 17).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Group {
    /// Its executor is at it.
    Going,
    /// It waits in the queue (`wait` says why); later also a plan's limit or a budget.
    Queued,
    /// Its question waits for the person: «Ждёт вас».
    Asking,
    /// Ended: ready and decided, done, failed, stopped, interrupted — kept a month.
    Recent,
}

impl Group {
    fn parse(text: &str) -> Option<Group> {
        match text {
            "going" => Some(Group::Going),
            "queued" => Some(Group::Queued),
            "asking" => Some(Group::Asking),
            "recent" => Some(Group::Recent),
            _ => None,
        }
    }
}

/// Why a work waits in the queue: `busy` — as many of its kind go on as may at once; `soon` — a new one by voice
/// came from its device less than two minutes ago, `until` then; `day` — ten new ones by voice today; `next` — its
/// turn is now. `ahead` — how many of its kind wait before it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct Wait {
    why: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    until: Option<u64>,
    ahead: usize,
}

/// What a work's deals spent: their number, tokens and dollars (`usdMicro` of the priced ones; `unpriced` — how many
/// had no price).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct Spending {
    deals: u64,
    tokens: crate::router::Tokens,
    usd_micro: u64,
    unpriced: u64,
}

/// A work as the page «Работы» shows it: the record, and what goes with it now.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkItem {
    #[serde(flatten)]
    work: Work,
    group: Group,
    #[serde(skip_serializing_if = "Option::is_none")]
    wait: Option<Wait>,
    /// What an app's agent is at: `thinking`, `writing`, `reading`, `checking`, `trying`, `phrase`, `docs`, `journal`.
    #[serde(skip_serializing_if = "Option::is_none")]
    doing: Option<&'static str>,
    /// The executor's own words of where a background work is: the tool's step, the task's percent and words.
    #[serde(skip_serializing_if = "Option::is_none")]
    step: Option<String>,
    /// Who answers it: the last round heard while it goes on, else its newest deal. The admin's only.
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<crate::router::Answered>,
    /// What its deals spent. The admin's only.
    #[serde(skip_serializing_if = "Option::is_none")]
    spent: Option<Spending>,
}

/// What the works are matched with for the page: the questions waiting, the queue, the router for the admin.
struct Shelf<'a> {
    app: &'a App,
    asking: std::collections::HashSet<String>,
    queued: Vec<Work>,
    admin: bool,
}

impl<'a> Shelf<'a> {
    fn new(app: &'a App, caller: &Caller) -> Shelf<'a> {
        let asking = app.talk.waiting().into_iter().map(|work| work.id).collect();
        let mut queued = app.talk.works().all(None, Some(WorkState::Queued));
        queued.sort_by_key(|work| (work.started_at, work.id.parse::<u64>().unwrap_or(0)));
        Shelf { app, asking, queued, admin: caller.permitted(SETTINGS_EDIT).is_ok() }
    }

    /// Where a work stands: going, in the queue, asking — as «Ждёт вас» counts it — or recent.
    fn group(&self, work: &Work) -> Group {
        // A work in a folder going on may ask too: a plan's limit, «подождать или остановить?».
        if self.asking.contains(&work.id) {
            Group::Asking
        } else if work.going() {
            Group::Going
        } else if work.state.queued() {
            Group::Queued
        } else {
            Group::Recent
        }
    }

    fn item(&self, work: Work) -> WorkItem {
        let group = self.group(&work);
        let wait = (group == Group::Queued).then(|| self.wait(&work));
        let executor = self.app.talk.executor(&work.executor);
        let (doing, step) = match (&executor, group) {
            (Some(executor), Group::Going) if work.kind == APP => (executor.status(&work).map(doing_name), None),
            (Some(executor), Group::Going | Group::Asking) if work.going() => (None, executor.details(&work)),
            _ => (None, None),
        };
        let (model, spent) = if self.admin { self.spending(&work) } else { (None, None) };
        WorkItem { work, group, wait, doing, step, model, spent }
    }

    /// Why a work waits: what the limits say of it now, as the queue asks them (layer.rs, `run_queue`).
    fn wait(&self, work: &Work) -> Wait {
        let ahead =
            self.queued.iter().take_while(|other| other.id != work.id).filter(|other| other.kind == work.kind).count();
        let (why, until) = match self.app.talk.works().may_start_kind(&work.kind, &work.origin, work.again, now_ms()) {
            Ok(()) => ("next", None),
            Err(Refused::Busy) => ("busy", None),
            Err(Refused::TooSoon(ms)) => ("soon", Some(now_ms() + ms)),
            Err(Refused::Enough) => ("day", None),
        };
        Wait { why, until, ahead }
    }

    /// Who answers a work and what its deals spent: the router's journal since the work began.
    fn spending(&self, work: &Work) -> (Option<crate::router::Answered>, Option<Spending>) {
        let deals = match self.app.router.deals.of_work(&work.id, work.started_at.saturating_sub(SINCE_SLACK_MS)) {
            Ok(deals) => deals,
            Err(error) => {
                tracing::warn!(target: "talk", "the deals of work {} are not read: {error}", work.id);
                return (None, None);
            }
        };
        let heard = work.going().then(|| self.app.router.last_heard(&work.id)).flatten();
        let model = heard.or(deals.last);
        let sums = deals.sums;
        let spent = (sums.deals > 0).then_some(Spending {
            deals: sums.deals,
            tokens: sums.tokens,
            usd_micro: sums.usd_micro,
            unpriced: sums.unpriced,
        });
        (model, spent)
    }
}

/// What an app's agent is at, by name for the panel's words.
fn doing_name(doing: Doing) -> &'static str {
    match doing {
        Doing::Thinking => "thinking",
        Doing::Writing => "writing",
        Doing::Reading => "reading",
        Doing::Checking => "checking",
        Doing::Trying => "trying",
        Doing::Phrase => "phrase",
        Doing::Docs => "docs",
        Doing::Journal => "journal",
    }
}

/// One phrase at most this long: the panel asks for an answer phrase by phrase.
pub const MAX_PHRASE: usize = 600;

pub const NO_VOICE: &str = "Голос не выбран: выберите свой — «Персонаж» → «Голос».";
pub const VOICE_STARTING: &str = "Голос запускается: попробуйте через несколько секунд.";

/// A phrase said by the household's own voice, as a WAV the panel plays.
async fn speech(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<Value>) -> Result<Response, ApiError> {
    caller.permitted(&use_of(CapabilityName::Chat))?;
    let text = body.get("text").and_then(Value::as_str).map(str::trim).unwrap_or_default();
    if text.is_empty() {
        return Err(ApiError::bad_request(crate::tr!("Нет текста.")));
    }
    if text.chars().count() > MAX_PHRASE {
        return Err(ApiError::bad_request(crate::tr!("Фраза длиннее {max} знаков.", max = MAX_PHRASE)));
    }
    let voice = app.voice.for_computer().ok_or_else(|| ApiError::conflict(NO_VOICE))?;
    // The panel going away (a new «Сказать», the × of the strip) drops the request, and that stops the synthesis.
    let cancel = tokio_util::sync::CancellationToken::new();
    let _guard = cancel.clone().drop_guard();
    let wav = voice.synthesize_wav(text, &cancel).await.map_err(|error| {
        if crate::speech::tts::own_speech::is_unreachable(&error) {
            return ApiError::conflict(VOICE_STARTING);
        }
        let why: String = app.secrets.redact(&format!("{error:#}")).chars().take(200).collect();
        ApiError::bad_gateway(crate::tr!("Голос не ответил: {why}", why = why))
    })?;
    let mut response = (StatusCode::OK, wav).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("audio/wav"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

async fn voice_input(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<VoiceInput> {
    caller.permitted(STATION_VIEW)?;
    Ok(ok(input::current(&app.settings)))
}

async fn set_voice_input(
    State(app): State<Arc<App>>,
    caller: Caller,
    Body(body): Body<Value>,
) -> ApiResult<VoiceInput> {
    caller.permitted(STATION_MODE)?;
    let station = app.placement.role() == Role::Station;
    Ok(ok(input::choose(&app.modes, &app.settings, station, body.get("input"), caller.actor())?))
}

/// The whole answer, for a caller that does not read a stream.
#[derive(Debug, Serialize)]
struct Said {
    said: String,
    by: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    work: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    asks: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    remind: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    dropped: bool,
}

/// The phrase as the body gave it: what cannot be read is left out, never guessed.
fn turn_of(body: &Value) -> Result<Turn, ApiError> {
    let text: String = body
        .get("text")
        .and_then(Value::as_str)
        .map(|text| text.trim().chars().take(MAX_TEXT).collect())
        .unwrap_or_default();
    if text.is_empty() {
        return Err(ApiError::bad_request(crate::tr!("Скажите или напишите что-нибудь.")));
    }
    let place = |value: &str| match value {
        "home" => Some(Place::Own),
        "cloud" => Some(Place::Cloud),
        _ => None,
    };
    let heard = body.get("heard").and_then(|heard| match heard {
        Value::String(by) => place(by).map(|by| Heard { by, ms: None, what: None }),
        Value::Object(heard) => Some(Heard {
            by: heard.get("by").and_then(Value::as_str).and_then(place)?,
            ms: heard.get("ms").and_then(Value::as_u64),
            what: heard.get("what").and_then(Value::as_str).map(str::to_string),
        }),
        _ => None,
    });
    // A phrase somebody heard was said: after the button unless the body says it answers a question.
    let fallback = if heard.is_some() { Activator::Button } else { Activator::Text };
    let activator = body.get("via").and_then(Value::as_str).and_then(Activator::parse).unwrap_or(fallback);
    let mut origin = Origin::computer(activator);
    // Why the microphone opened by itself: the closed window's reason (an old panel says none — its question's).
    if activator == Activator::Answer {
        origin.why = Some(body.get("why").and_then(Value::as_str).and_then(Why::parse).unwrap_or(Why::Question));
    }
    Ok(Turn {
        text,
        origin,
        heard,
        voice: body.get("voice").and_then(Value::as_bool) == Some(true),
        fresh: body.get("fresh").and_then(Value::as_bool) == Some(true),
    })
}

async fn talk(
    State(app): State<Arc<App>>,
    caller: Caller,
    headers: HeaderMap,
    Body(body): Body<Value>,
) -> Result<Response, ApiError> {
    let principal = caller.permitted(&use_of(CapabilityName::Chat))?.clone();
    let turn = turn_of(&body)?;
    let mut talk = super::begin(&app, &principal, turn)?;
    let stream = headers
        .get(header::ACCEPT)
        .and_then(|accept| accept.to_str().ok())
        .is_some_and(|accept| accept.contains("text/event-stream"));
    if !stream {
        // The caller going away drops the guard, and that stops the answer.
        let _guard = talk.cancel.clone().drop_guard();
        while let Some(event) = talk.events.recv().await {
            match event {
                TalkEvent::Done { said, by, work, asks, remind, dropped, .. } => {
                    return Ok(ok(Said { said, by, work, asks, remind, dropped }).into_response())
                }
                TalkEvent::Error { message } => return Err(ApiError::bad_gateway(message)),
                _ => {}
            }
        }
        return Err(ApiError::bad_gateway(crate::tr!("Ответ оборвался.")));
    }
    let guard = talk.cancel.drop_guard();
    let body = UnboundedReceiverStream::new(talk.events).map(move |event| {
        let _ = &guard;
        let data = serde_json::to_string(&event).unwrap_or_else(|_| "{}".into());
        Ok::<_, Infallible>(format!("data: {data}\n\n"))
    });
    let mut response = (StatusCode::OK, axum::body::Body::from_stream(body)).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/event-stream; charset=utf-8"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache, no-transform"));
    headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
    Ok(response)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::talk::Via;

    #[test]
    fn reads_the_phrase_and_where_it_came_from() {
        let typed = turn_of(&json!({ "text": "  Который час?  " })).unwrap();
        assert_eq!(
            (typed.text.as_str(), typed.origin.activator, typed.origin.via),
            ("Который час?", Activator::Text, Via::Text)
        );
        assert_eq!((typed.heard, typed.voice, typed.fresh), (None, false, false));

        let said = turn_of(&json!({ "text": "Привет", "heard": "cloud", "voice": true, "fresh": true })).unwrap();
        assert_eq!(said.origin.activator, Activator::Button, "a phrase somebody heard was said after the button");
        assert_eq!(said.heard, Some(Heard { by: Place::Cloud, ms: None, what: None }));
        assert!(said.voice && said.fresh);

        let answer = turn_of(&json!({
            "text": "Да", "via": "answer", "heard": { "by": "home", "ms": 420, "what": "gigaam-v3" }
        }))
        .unwrap();
        assert_eq!((answer.origin.activator, answer.origin.device.as_str()), (Activator::Answer, "computer"));
        assert_eq!(answer.heard, Some(Heard { by: Place::Own, ms: Some(420), what: Some("gigaam-v3".into()) }));

        assert_eq!(turn_of(&json!({ "text": "Да", "heard": "the moon", "via": "word" })).unwrap().heard, None);
        assert_eq!(turn_of(&json!({ "text": "я".repeat(1500) })).unwrap().text.chars().count(), MAX_TEXT);
        for empty in [json!({}), json!({ "text": "  " }), json!({ "text": 5 })] {
            let refused = turn_of(&empty).unwrap_err();
            assert_eq!((refused.status.as_u16(), refused.message.as_str()), (400, "Скажите или напишите что-нибудь."));
        }
    }
}
