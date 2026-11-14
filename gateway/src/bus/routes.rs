//! `/api/v1/bus`: the bus for the panel (docs/plans/bus.md, «Пульт»). The links, modes, the owner's words and the
//! questions are the admin's at home (`plugins.manage`): they decide what apps may reach. The journal is read with the
//! right to the journal. A panel face — a card's button, a window — sends a letter as its own app, for the person
//! signed in (`/api/v1/bus/call`): the chain is that person's, never more.
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use super::address::{Address, Pattern};
use super::danger::DangerWord;
use super::links::{Link, LinkSource, Mode, Options};
use super::provider::BusError;
use super::{Level, Trail};
use crate::app::App;
use crate::core::identity::permissions::{use_of, JOURNAL_VIEW, PLUGINS_MANAGE};
use crate::platform::http::{ok, ApiError, ApiResult, Body, Caller, Zone};
use crate::skills::capabilities::CapabilityName;

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/v1/bus", get(overview))
        .route("/api/v1/bus/questions", get(questions))
        .route("/api/v1/bus/questions/{id}", post(answer))
        .route("/api/v1/bus/links", get(links).put(put_link).delete(remove_link))
        .route("/api/v1/bus/modes", put(put_mode))
        .route("/api/v1/bus/options", put(put_options))
        .route("/api/v1/bus/dangers", put(put_danger))
        .route("/api/v1/bus/journal", get(journal))
        .route("/api/v1/bus/apps/{id}", get(app_view))
        .route("/api/v1/bus/call", post(call))
        .route("/api/v1/bus/preview", get(preview))
}

/// A bus error as the API answers it: not found, forbidden, waiting for «да» (with the question's id), a fuse.
pub fn api_error(error: BusError) -> ApiError {
    let status = match &error {
        BusError::NotFound(_) => StatusCode::NOT_FOUND,
        BusError::Denied(_) => StatusCode::FORBIDDEN,
        BusError::NeedsYes { .. } => StatusCode::CONFLICT,
        BusError::Fuse(_) => StatusCode::TOO_MANY_REQUESTS,
        BusError::Failed(_) => StatusCode::BAD_GATEWAY,
    };
    let mut api = ApiError::new(status, error.to_string());
    let mut details = json!({ "code": error.code() });
    if let BusError::NeedsYes { id, .. } = &error {
        details["question"] = json!(id);
    }
    api.errors = Some(details);
    api
}

fn admin(caller: &Caller) -> Result<(), ApiError> {
    caller.zone_at_least(Zone::Home)?;
    caller.permitted(PLUGINS_MANAGE)?;
    Ok(())
}

fn address(text: &str) -> Result<Address, ApiError> {
    Address::parse(text).map_err(ApiError::bad_request)
}

fn pattern(text: &str) -> Result<Pattern, ApiError> {
    Pattern::parse(text).map_err(ApiError::bad_request)
}

/// Everything the «Связи» screen draws: who is there, the links, the modes, the switches, the questions.
async fn overview(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Value> {
    admin(&caller)?;
    let bus = &app.bus;
    let participants: Vec<Value> = ["app", "system", "device", "files"]
        .iter()
        .filter_map(|kind| bus.provider(kind))
        .flat_map(|provider| provider.participants())
        .map(|participant| json!(participant))
        .collect();
    Ok(ok(json!({
        "participants": participants,
        "links": bus.links.all(),
        "modes": bus.links.modes(),
        "options": bus.links.options(),
        "dangers": bus.links.dangers(),
        "questions": bus.consent.waiting(),
    })))
}

async fn questions(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Value> {
    admin(&caller)?;
    Ok(ok(json!({ "questions": app.bus.consent.waiting() })))
}

#[derive(Debug, Deserialize)]
struct AnswerBody {
    yes: bool,
}

/// A person's answer to a question: «да» or «нет».
async fn answer(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<AnswerBody>,
) -> ApiResult<Value> {
    admin(&caller)?;
    let question = app
        .bus
        .answer(&id, body.yes, caller.actor())?
        .ok_or_else(|| ApiError::not_found(crate::tr!("Этого вопроса уже нет: на него ответили или он устарел.")))?;
    Ok(ok(json!({ "question": question, "yes": body.yes })))
}

async fn links(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Value> {
    admin(&caller)?;
    Ok(ok(json!({ "links": app.bus.links.all() })))
}

#[derive(Debug, Deserialize)]
struct LinkBody {
    from: String,
    to: String,
    #[serde(default)]
    level: Option<String>,
}

/// A link put by hand: the narrowest for its pair decides, a lower one too («не знает» closes a door).
async fn put_link(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<LinkBody>) -> ApiResult<Value> {
    admin(&caller)?;
    let level = body
        .level
        .as_deref()
        .and_then(Level::parse)
        .ok_or_else(|| ApiError::bad_request(crate::tr!("level: none, see, listen, call или edit")))?;
    let mut link = Link::new(pattern(&body.from)?, pattern(&body.to)?, level, LinkSource::Owner);
    link.by = caller.principal().map(|principal| principal.name.clone());
    app.bus.links.grant(link, caller.actor())?;
    app.bus.changed("link", Address::parse(&body.from).ok().as_ref(), caller.actor());
    Ok(ok(json!({ "links": app.bus.links.all() })))
}

async fn remove_link(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<LinkBody>) -> ApiResult<Value> {
    admin(&caller)?;
    let removed = app.bus.links.remove(&pattern(&body.from)?, &pattern(&body.to)?, caller.actor())?;
    if removed {
        app.bus.changed("unlink", Address::parse(&body.from).ok().as_ref(), caller.actor());
    }
    Ok(ok(json!({ "removed": removed, "links": app.bus.links.all() })))
}

#[derive(Debug, Deserialize)]
struct ModeBody {
    /// A participant's own mode; none for the house's.
    #[serde(default)]
    address: Option<String>,
    /// `isolated`, `rules`, `free`; none drops a participant's own.
    #[serde(default)]
    mode: Option<String>,
}

async fn put_mode(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<ModeBody>) -> ApiResult<Value> {
    admin(&caller)?;
    let target = body.address.as_deref().map(address).transpose()?;
    let mode = match body.mode.as_deref() {
        None => None,
        Some(text) => {
            Some(Mode::parse(text).ok_or_else(|| ApiError::bad_request(crate::tr!("mode: isolated, rules или free")))?)
        }
    };
    app.bus.links.set_mode(target.as_ref(), mode, caller.actor())?;
    app.bus.changed("mode", target.as_ref(), caller.actor());
    Ok(ok(json!({ "modes": app.bus.links.modes() })))
}

async fn put_options(State(app): State<Arc<App>>, caller: Caller, Body(options): Body<Options>) -> ApiResult<Value> {
    admin(&caller)?;
    app.bus.links.set_options(&options, caller.actor())?;
    app.bus.changed("options", None, caller.actor());
    Ok(ok(json!({ "options": app.bus.links.options() })))
}

/// The owner's word on a dangerous handle: «всегда можно» for a caller, a mark taken off or put on.
async fn put_danger(State(app): State<Arc<App>>, caller: Caller, Body(word): Body<DangerWord>) -> ApiResult<Value> {
    admin(&caller)?;
    if let Some(mark) = word.mark.as_deref() {
        if mark != "none" && super::danger::Danger::parse(mark).is_none() {
            return Err(ApiError::bad_request(crate::tr!(
                "mark: none, money, delete, outside, locks, camera или access"
            )));
        }
    }
    app.bus.links.say_danger(word, caller.actor())?;
    app.bus.changed("danger", None, caller.actor());
    Ok(ok(json!({ "dangers": app.bus.links.dangers() })))
}

#[derive(Debug, Deserialize)]
struct JournalQuery {
    #[serde(default)]
    about: Option<String>,
    #[serde(default)]
    before: Option<i64>,
    #[serde(default)]
    limit: Option<u32>,
}

/// The journal's lines, the newest first: who wrote to whom and how it went — never what.
async fn journal(State(app): State<Arc<App>>, caller: Caller, Query(query): Query<JournalQuery>) -> ApiResult<Value> {
    caller.permitted(JOURNAL_VIEW)?;
    let lines = app.bus.journal.read(query.about.as_deref(), query.before, query.limit.unwrap_or(100));
    Ok(ok(json!({ "lines": lines })))
}

/// One app as the «Связи» part of its window shows it: what it can do, what it reaches and is reached by, its mode,
/// what it asked for, its questions waiting.
async fn app_view(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<Value> {
    admin(&caller)?;
    let me = Address::app(&id);
    let bus = &app.bus;
    let provider = bus.provider("app").ok_or_else(|| ApiError::not_found(crate::tr!("Приложений здесь нет.")))?;
    let participant = provider
        .participant(&me)
        .ok_or_else(|| ApiError::not_found(crate::tr!("Приложение выключено или его нет.")))?;
    let exact = Pattern::exact(&me);
    let mut links: Vec<Link> =
        bus.links.all().into_iter().filter(|link| link.from == exact || link.to == exact).collect();
    // A built-in app's rights from its install are links too, though not written as such (the apps' `implicit`):
    // shown with the source «при установке», so the owner sees them and can close one — a link of his own wins.
    let built_in = matches!(
        app.plugins.origin_of(&id),
        crate::plugins::registry::Origin::System | crate::plugins::registry::Origin::Bundled
    );
    if built_in {
        if let Some(package) = app.plugins.package(&id) {
            for permission in &package.manifest.permissions {
                if let crate::plugins::manifest::Permission::Bus { level, pattern } = permission {
                    if !links.iter().any(|link| link.from == exact && &link.to == pattern) {
                        let mut install = Link::new(exact.clone(), pattern.clone(), *level, LinkSource::Install);
                        install.at = None;
                        links.push(install);
                    }
                }
            }
        }
    }
    let reaches = bus.list(&Trail::app(&id), &me);
    let asked: Vec<String> = app
        .plugins
        .package(&id)
        .map(|package| {
            package
                .manifest
                .permissions
                .iter()
                .filter(|permission| matches!(permission, crate::plugins::manifest::Permission::Bus { .. }))
                .map(|permission| permission.name())
                .collect()
        })
        .unwrap_or_default();
    let questions: Vec<_> = bus.consent.waiting().into_iter().filter(|question| question.from == me).collect();
    Ok(ok(json!({
        "participant": participant,
        "handles": provider.handles(&me),
        "mode": bus.links.mode(&me),
        "ownMode": bus.links.modes().own.get(me.as_str()),
        "links": links,
        "reaches": reaches,
        "asked": asked,
        "questions": questions,
    })))
}

#[derive(Debug, Deserialize)]
struct PreviewQuery {
    /// The app whose card shows it.
    app: String,
    /// The picture's address in a folder: `files/home/Фото/лето.jpg`.
    address: String,
    #[serde(default)]
    size: Option<u32>,
}

/// A card's picture from a folder of the home folder (`picture: {address}`): made small and read as the app, for
/// the person who looks at the card — the app's link to the folder and the person's own rights both hold.
async fn preview(
    State(app): State<Arc<App>>,
    caller: Caller,
    Query(query): Query<PreviewQuery>,
) -> Result<axum::response::Response, ApiError> {
    let principal = caller.permitted(&use_of(CapabilityName::Plugins))?.clone();
    let trail = Trail::principal(&principal)
        .from_afar(caller.zone == Zone::Remote)
        .entering(&query.app)
        .map_err(|error| api_error(error.into()))?;
    let to = address(&query.address)?;
    let input = json!({ "size": query.size.unwrap_or(super::providers::files::preview::SIDE) });
    let shown = app.bus.call(trail, Address::app(&query.app), to, "preview", input, vec![]).await.map_err(api_error)?;
    super::providers::files::routes::jpeg(&shown)
}

#[derive(Debug, Deserialize)]
struct CallBody {
    /// The app whose face sends it: `app/<id>`.
    from: String,
    to: String,
    handle: String,
    #[serde(default)]
    input: Value,
    #[serde(default)]
    files: Vec<String>,
}

/// A letter from a panel face (a card's button, a window) as its own app, for the person signed in.
async fn call(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<CallBody>) -> ApiResult<Value> {
    let principal = caller.permitted(&use_of(CapabilityName::Plugins))?.clone();
    let from = address(&body.from)?;
    if from.kind() != super::address::APP || from.path() != from.name().unwrap_or_default() {
        return Err(ApiError::bad_request(crate::tr!(
            "from: адрес приложения, чьё это окошко или карточка (app/<id>)"
        )));
    }
    let files = body.files.iter().map(|file| address(file)).collect::<Result<Vec<_>, _>>()?;
    let id = from.name().unwrap_or_default().to_string();
    let trail = Trail::principal(&principal)
        .from_afar(caller.zone == Zone::Remote)
        .entering(&id)
        .map_err(|error| api_error(error.into()))?;
    let result =
        app.bus.call(trail, from, address(&body.to)?, &body.handle, body.input, files).await.map_err(api_error)?;
    Ok(ok(json!({ "result": result })))
}
