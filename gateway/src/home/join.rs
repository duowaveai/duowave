//! Into the home by a code (docs/plans/home-setup-flow.md, stage 2). A computer that wants to be the home's Turbo, or a
//! station with DuoWave of its own that wants to be its ear, asks the home's centre; the centre makes a code of six
//! digits and tells it (join_say.rs: aloud through its station, a window of the app on a computer centre, a line on
//! stdout of a server) and shows it to the home's admins with «Разрешить». The asking device sends the code back — or
//! an admin allows it — and gets what joins it:
//!
//! - on a station centre, the pair's key, as `POST /api/v1/placement/pair` gives it to its admin (one key for every
//!   device yet — a key of each device's own is a later stage: «Потом» of the plan);
//! - on a computer centre, a computer offers its own connection key and its link's address (`link`): the centre pairs
//!   with it over the link (src/helpers/link.rs), as «Подключить Турбо» with that key does.
//!
//! The requests live in memory — a restart of the centre closes them. A code lives 5 minutes, works once and takes 3
//! tries; asking is counted per source as signing in is (access/throttle.rs), and a few requests at most are open at
//! once. The asker holds its request by a token of its own: another device on the Wi-Fi neither reads it nor tries
//! codes on it, and only the admins see the code.
//!
//! - `POST /api/v1/home/join` `{kind: turbo|ear, name, address?, link?: {url, key}}` — from the home network, no
//!   sign-in: `{id, token, expiresAt, told}`.
//! - `POST /api/v1/home/join/{id}/code` `{token, code}` — the asker: the right code joins, the grant in the answer.
//! - `GET /api/v1/home/join/{id}?token=` — the asker: pending, joined (the grant, once), denied, closed.
//! - `POST /api/v1/home/join/{id}/say` `{token}` — «Сказать ещё раз».
//! - `GET /api/v1/home/joins` — the admins: the requests with their codes while they are open.
//! - `POST /api/v1/home/join/{id}/approve`, `/deny` — an admin: «Разрешить», «Это не я».
//!
//! Each change is `home.join` on the live channel (its subject the request's id): the admins' panels read the list
//! again and show the request's card.

use std::sync::{Arc, Mutex, MutexGuard};

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::Router;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use subtle::ConstantTimeEq;

use crate::access::throttle::LoginThrottle;
use crate::app::App;
use crate::core::events::{Actor, GatewayEvent};
use crate::core::identity::permissions::USERS_MANAGE;
use crate::platform::http::{created, ok, ApiError, ApiResult, Body, Caller, Zone};
use crate::utils::ids::{random_hex, uuid};
use crate::utils::time::now_ms;

/// A code lives this long.
pub const CODE_MS: u64 = 5 * 60_000;
/// Wrong codes a request takes before it closes.
pub const TRIES: u8 = 3;
/// Requests open at once; one more waits until one ends.
const OPEN_AT_MOST: usize = 4;
/// «Сказать ещё раз» at most once in this long.
const SAY_AGAIN_MS: u64 = 5_000;
/// An ended request is kept this long: its asker reads how it ended, the panel shows it.
const KEPT_MS: u64 = 10 * 60_000;
/// Requests a source may make in the window below before it waits.
const ASKS: u32 = 6;
const ASKS_WINDOW_MS: u64 = 15 * 60_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JoinKind {
    /// A computer: the home's muscles.
    Turbo,
    /// A station with DuoWave of its own: it hears for the centre.
    Ear,
}

impl JoinKind {
    pub fn as_str(self) -> &'static str {
        match self {
            JoinKind::Turbo => "turbo",
            JoinKind::Ear => "ear",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JoinStatus {
    Pending,
    Joined,
    Denied,
    Closed,
}

/// How the centre told the code.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Told {
    /// Aloud, through a station of the centre that can speak now (an outcome comes a moment later).
    pub spoken: bool,
    /// In a window of the app on this computer.
    pub window: bool,
    /// On stdout of a server without a screen.
    pub printed: bool,
}

/// A computer's own connection key and its link's address: a computer centre pairs with it (src/helpers/link.rs).
#[derive(Debug, Clone, Deserialize)]
pub struct LinkOffer {
    pub url: String,
    pub key: String,
}

struct JoinRequest {
    id: String,
    token: String,
    kind: JoinKind,
    name: String,
    address: Option<String>,
    source: String,
    code: String,
    tries_left: u8,
    expires_ms: u64,
    said_ms: u64,
    status: JoinStatus,
    told: Told,
    link: Option<LinkOffer>,
    /// What joins the asker, kept for it to take once when an admin allowed it.
    grant: Option<Value>,
    taken: bool,
    /// Why it closed, in the household's words.
    why: Option<String>,
    ended_ms: u64,
}

impl JoinRequest {
    fn open(&self) -> bool {
        self.status == JoinStatus::Pending
    }

    fn end(&mut self, status: JoinStatus, why: Option<String>, now: u64) {
        self.status = status;
        self.why = why;
        self.ended_ms = now;
    }
}

/// A request as the admins see it: the code while it is open.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinView {
    pub id: String,
    pub kind: JoinKind,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    /// Where the request came from, as the centre saw it.
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    pub status: JoinStatus,
    pub tries_left: u8,
    pub expires_at: String,
    pub told: Told,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

/// A request as its asker sees it: never the code; the grant once.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AskerView {
    pub id: String,
    pub status: JoinStatus,
    pub tries_left: u8,
    pub expires_at: String,
    pub told: Told,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant: Option<Value>,
}

/// A new request: what its asker keeps, and what the centre tells.
#[derive(Debug, Clone)]
pub struct Asked {
    pub id: String,
    pub token: String,
    pub code: String,
    pub expires_ms: u64,
}

/// What the asker's code did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodeOutcome {
    Right,
    Wrong { tries_left: u8 },
}

/// A request this computer made to a home's centre — the asker's side (`POST /api/v1/placement/pair-code`,
/// placement/routes.rs): where the centre is and the token the request is held by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outgoing {
    pub address: String,
    pub port: u16,
    pub token: String,
    pub expires_ms: u64,
    /// `Turbo` — this computer joins the home; `Ear` — this computer, the home's Turbo, takes that station in as the
    /// centre's ear (stage 5): the station says the code, the household types it here.
    pub kind: JoinKind,
}

/// The open requests of this centre, and this device's own requests to another.
pub struct Joins {
    requests: Mutex<Vec<JoinRequest>>,
    asks: LoginThrottle,
    outgoing: Mutex<std::collections::HashMap<String, Outgoing>>,
}

impl Default for Joins {
    fn default() -> Self {
        Joins::new()
    }
}

fn iso(ms: u64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(i64::try_from(ms).unwrap_or(i64::MAX))
        .map(crate::utils::time::iso)
        .unwrap_or_default()
}

/// Six digits from the system's generator (rand's thread generator is a CSPRNG).
fn new_code() -> String {
    format!("{:06}", rand::random_range(0..1_000_000u32))
}

/// The digits only: «47 19 82» and «47-19-82» are 471982.
fn digits(code: &str) -> String {
    code.chars().filter(char::is_ascii_digit).collect()
}

fn same(given: &str, expected: &str) -> bool {
    !expected.is_empty() && given.len() == expected.len() && bool::from(given.as_bytes().ct_eq(expected.as_bytes()))
}

impl Joins {
    pub fn new() -> Joins {
        Joins {
            requests: Mutex::new(Vec::new()),
            asks: LoginThrottle::new(ASKS, ASKS_WINDOW_MS, ASKS_WINDOW_MS),
            outgoing: Mutex::new(std::collections::HashMap::new()),
        }
    }

    fn outgoing_lock(&self) -> MutexGuard<'_, std::collections::HashMap<String, Outgoing>> {
        self.outgoing.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// This device asked a centre in: kept until a while after its code ran out.
    pub fn keep_outgoing(&self, id: &str, outgoing: Outgoing, now: u64) {
        let mut kept = self.outgoing_lock();
        kept.retain(|_, old| now < old.expires_ms.saturating_add(KEPT_MS));
        kept.insert(id.to_string(), outgoing);
    }

    pub fn outgoing(&self, id: &str) -> Option<Outgoing> {
        self.outgoing_lock().get(id).cloned()
    }

    pub fn forget_outgoing(&self, id: &str) {
        self.outgoing_lock().remove(id);
    }

    fn lock(&self) -> MutexGuard<'_, Vec<JoinRequest>> {
        self.requests.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Ends what ran out and drops what ended long ago.
    fn tidy(requests: &mut Vec<JoinRequest>, now: u64) {
        for request in requests.iter_mut() {
            if request.open() && now >= request.expires_ms {
                request.end(JoinStatus::Closed, Some(crate::tr!("Код больше не действует: прошло 5 минут.")), now);
            }
        }
        requests.retain(|request| request.open() || now.saturating_sub(request.ended_ms) < KEPT_MS);
    }

    /// A new request from `source`: refused while the source asked too often or too many are open.
    pub fn ask(
        &self,
        kind: JoinKind,
        name: &str,
        address: Option<String>,
        link: Option<LinkOffer>,
        source: &str,
        now: u64,
    ) -> Result<Asked, ApiError> {
        let wait = self.asks.attempt(source, now);
        if wait > 0 {
            let minutes = wait.div_ceil(60_000).max(1);
            return Err(ApiError::too_many(crate::tr!(
                "Слишком много запросов — подождите {minutes} мин.",
                minutes = minutes
            )));
        }
        let mut requests = self.lock();
        Self::tidy(&mut requests, now);
        if requests.iter().filter(|request| request.open()).count() >= OPEN_AT_MOST {
            return Err(ApiError::too_many(crate::tr!(
                "У дома уже несколько запросов на подключение: подождите, пока они закончатся."
            )));
        }
        let asked =
            Asked { id: uuid(), token: random_hex(16), code: new_code(), expires_ms: now.saturating_add(CODE_MS) };
        requests.push(JoinRequest {
            id: asked.id.clone(),
            token: asked.token.clone(),
            kind,
            name: name.to_string(),
            address,
            source: source.to_string(),
            code: asked.code.clone(),
            tries_left: TRIES,
            expires_ms: asked.expires_ms,
            said_ms: now,
            status: JoinStatus::Pending,
            told: Told::default(),
            link,
            grant: None,
            taken: false,
            why: None,
            ended_ms: 0,
        });
        Ok(asked)
    }

    fn find<'a>(requests: &'a mut [JoinRequest], id: &str) -> Result<&'a mut JoinRequest, ApiError> {
        requests.iter_mut().find(|request| request.id == id).ok_or_else(|| {
            ApiError::not_found(crate::tr!("Такого запроса нет: он закончился или центр перезапустился."))
        })
    }

    /// The asker's own request: its token proves it.
    fn mine<'a>(requests: &'a mut [JoinRequest], id: &str, token: &str) -> Result<&'a mut JoinRequest, ApiError> {
        let request = Self::find(requests, id)?;
        if !same(token, &request.token) {
            return Err(ApiError::forbidden(crate::tr!("Это не ваш запрос.")));
        }
        Ok(request)
    }

    fn still_open(request: &JoinRequest) -> Result<(), ApiError> {
        if request.open() {
            return Ok(());
        }
        Err(ApiError::conflict(request.why.clone().unwrap_or_else(|| match request.status {
            JoinStatus::Joined => crate::tr!("Уже подключено."),
            JoinStatus::Denied => crate::tr!("В доме ответили «Это не я»."),
            _ => crate::tr!("Запрос закрыт."),
        })))
    }

    /// The asker's code: right — the request is the asker's to join (`Joined`, its grant comes from the handler);
    /// wrong — one try fewer, none left — closed.
    pub fn code(&self, id: &str, token: &str, code: &str, now: u64) -> Result<CodeOutcome, ApiError> {
        let mut requests = self.lock();
        Self::tidy(&mut requests, now);
        let request = Self::mine(&mut requests, id, token)?;
        Self::still_open(request)?;
        if same(&digits(code), &request.code) {
            request.end(JoinStatus::Joined, None, now);
            request.taken = true;
            return Ok(CodeOutcome::Right);
        }
        request.tries_left = request.tries_left.saturating_sub(1);
        if request.tries_left == 0 {
            request.end(
                JoinStatus::Closed,
                Some(crate::tr!("Три неверных кода: запрос закрыт. Попросите новый код.")),
                now,
            );
        }
        Ok(CodeOutcome::Wrong { tries_left: request.tries_left })
    }

    /// An admin's «Разрешить»: joined without the code; the asker takes the grant with its next look.
    pub fn approve(&self, id: &str, now: u64) -> Result<(), ApiError> {
        let mut requests = self.lock();
        Self::tidy(&mut requests, now);
        let request = Self::find(&mut requests, id)?;
        Self::still_open(request)?;
        request.end(JoinStatus::Joined, None, now);
        Ok(())
    }

    /// An admin's «Это не я».
    pub fn deny(&self, id: &str, now: u64) -> Result<(), ApiError> {
        let mut requests = self.lock();
        Self::tidy(&mut requests, now);
        let request = Self::find(&mut requests, id)?;
        Self::still_open(request)?;
        request.end(JoinStatus::Denied, Some(crate::tr!("В доме ответили «Это не я»: подключение отклонено.")), now);
        Ok(())
    }

    /// The grant made: kept for the asker when an admin allowed it (`keep`), else handed in the answer.
    fn granted(&self, id: &str, grant: Value, keep: bool) {
        if let Ok(request) = Self::find(&mut self.lock(), id) {
            if keep {
                request.grant = Some(grant);
            }
        }
    }

    /// Making the grant failed: the request closes with why.
    fn failed(&self, id: &str, why: &str, now: u64) {
        if let Ok(request) = Self::find(&mut self.lock(), id) {
            request.end(JoinStatus::Closed, Some(why.to_string()), now);
        }
    }

    /// The station did not say the code after all: the asker is told to look at the screens.
    pub fn told_failed(&self, id: &str) {
        self.told(id, |told| told.spoken = false);
    }

    fn told(&self, id: &str, edit: impl FnOnce(&mut Told)) {
        if let Ok(request) = Self::find(&mut self.lock(), id) {
            edit(&mut request.told);
        }
    }

    /// «Сказать ещё раз»: the code to tell again, at most once in a few seconds.
    pub fn say_again(&self, id: &str, token: &str, now: u64) -> Result<Option<(String, String, JoinKind)>, ApiError> {
        let mut requests = self.lock();
        Self::tidy(&mut requests, now);
        let request = Self::mine(&mut requests, id, token)?;
        Self::still_open(request)?;
        if now.saturating_sub(request.said_ms) < SAY_AGAIN_MS {
            return Ok(None);
        }
        request.said_ms = now;
        Ok(Some((request.code.clone(), request.name.clone(), request.kind)))
    }

    /// The asker's look: the grant once, when an admin allowed it.
    pub fn read(&self, id: &str, token: &str, now: u64) -> Result<AskerView, ApiError> {
        let mut requests = self.lock();
        Self::tidy(&mut requests, now);
        let request = Self::mine(&mut requests, id, token)?;
        let grant = if request.status == JoinStatus::Joined && !request.taken && request.grant.is_some() {
            request.taken = true;
            request.grant.clone()
        } else {
            None
        };
        Ok(AskerView {
            id: request.id.clone(),
            status: request.status,
            tries_left: request.tries_left,
            expires_at: iso(request.expires_ms),
            told: request.told.clone(),
            why: request.why.clone(),
            grant,
        })
    }

    /// The admins' list: the open ones first, the newest first.
    pub fn list(&self, now: u64) -> Vec<JoinView> {
        let mut requests = self.lock();
        Self::tidy(&mut requests, now);
        let mut views: Vec<JoinView> = requests
            .iter()
            .rev()
            .map(|request| JoinView {
                id: request.id.clone(),
                kind: request.kind,
                name: request.name.clone(),
                address: request.address.clone(),
                source: request.source.clone(),
                code: request.open().then(|| request.code.clone()),
                status: request.status,
                tries_left: request.tries_left,
                expires_at: iso(request.expires_ms),
                told: request.told.clone(),
                why: request.why.clone(),
            })
            .collect();
        views.sort_by_key(|view| view.status != JoinStatus::Pending);
        views
    }

    fn kind_and_link(&self, id: &str) -> Option<(JoinKind, Option<LinkOffer>, String)> {
        let requests = self.lock();
        let request = requests.iter().find(|request| request.id == id)?;
        Some((request.kind, request.link.clone(), request.name.clone()))
    }
}

// ─── The routes ──────────────────────────────────────────────

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/v1/home/join", post(ask))
        .route("/api/v1/home/joins", get(list))
        .route("/api/v1/home/join/{id}", get(read))
        .route("/api/v1/home/join/{id}/code", post(code))
        .route("/api/v1/home/join/{id}/say", post(say))
        .route("/api/v1/home/join/{id}/approve", post(approve))
        .route("/api/v1/home/join/{id}/deny", post(deny))
}

fn changed(app: &App, id: &str, change: &str, actor: Option<Actor>) {
    app.events.emit(GatewayEvent::HomeJoin { id: id.to_string(), change: change.to_string(), actor });
}

/// This gateway is the home's centre: not an ear of another's, and — on a computer — not the Turbo of a station's.
fn centre_here(app: &App) -> Result<(), ApiError> {
    if let Some((ear, _)) = super::ear::centre(app) {
        return Err(ApiError::conflict(crate::tr!(
            "Это не центр дома: подключайтесь к центру, {address}.",
            address = ear.address
        )));
    }
    #[cfg(feature = "pc")]
    if let Some(centre) = super::devices::centre_elsewhere(app) {
        let address = centre.record().map(|record| record.address).unwrap_or_default();
        return Err(ApiError::conflict(crate::tr!(
            "Это не центр дома: подключайтесь к центру, {address}.",
            address = address
        )));
    }
    Ok(())
}

/// The admins only: a person with the right to manage people, never a key (as the pair's key is given, routes.rs).
fn admin(caller: &Caller) -> Result<(), ApiError> {
    use crate::core::identity::principal::PrincipalKind;
    let principal = caller.permitted(USERS_MANAGE)?;
    if principal.kind != PrincipalKind::User {
        return Err(ApiError::forbidden(crate::tr!("Запросы на подключение видят только администраторы дома.")));
    }
    Ok(())
}

fn text<'a>(body: &'a Value, field: &str) -> Option<&'a str> {
    body.get(field).and_then(Value::as_str).map(str::trim).filter(|text| !text.is_empty())
}

/// The name a device asks in with: cleaned, short, never empty.
fn asker_name(body: &Value, kind: JoinKind) -> String {
    let name: String = text(body, "name").unwrap_or_default().chars().filter(|c| !c.is_control()).take(60).collect();
    let name = name.trim().to_string();
    if !name.is_empty() {
        return name;
    }
    match kind {
        JoinKind::Turbo => crate::tr!("Компьютер"),
        JoinKind::Ear => crate::tr!("Колонка"),
    }
}

async fn ask(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<Value>) -> ApiResult<Value> {
    caller.zone_at_least(Zone::Home)?;
    centre_here(&app)?;
    let kind: JoinKind = serde_json::from_value(body.get("kind").cloned().unwrap_or(Value::Null))
        .map_err(|_| ApiError::bad_request(crate::tr!("kind: turbo или ear")))?;
    let link = match body.get("link") {
        None | Some(Value::Null) => None,
        Some(link) => Some(
            serde_json::from_value::<LinkOffer>(link.clone())
                .map_err(|_| ApiError::bad_request(crate::tr!("link: url и key — адрес связи и ключ подключения")))?,
        ),
    };
    let name = asker_name(&body, kind);
    let address = text(&body, "address").map(str::to_string);
    let asked = app.joins.ask(kind, &name, address, link, &caller.source, now_ms())?;
    tracing::info!(target: "home", "join: «{name}» asks in as {} from {}", kind.as_str(), caller.source);
    let told = super::join_say::tell(app.clone(), &asked.id, &name, kind, &asked.code, asked.expires_ms);
    app.joins.told(&asked.id, |kept| *kept = told.clone());
    changed(&app, &asked.id, "asked", None);
    Ok(created(json!({
        "id": asked.id,
        "token": asked.token,
        "expiresAt": iso(asked.expires_ms),
        "tries": TRIES,
        "told": told,
    })))
}

#[derive(Debug, Default, Deserialize)]
struct TokenQuery {
    #[serde(default)]
    token: String,
}

async fn read(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
    Query(query): Query<TokenQuery>,
) -> ApiResult<AskerView> {
    caller.zone_at_least(Zone::Home)?;
    Ok(ok(app.joins.read(&id, &query.token, now_ms())?))
}

async fn code(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<Value>,
) -> ApiResult<Value> {
    caller.zone_at_least(Zone::Home)?;
    let token = text(&body, "token").unwrap_or_default();
    let given = text(&body, "code").unwrap_or_default();
    match app.joins.code(&id, token, given, now_ms())? {
        CodeOutcome::Wrong { tries_left } => {
            changed(&app, &id, if tries_left == 0 { "closed" } else { "wrong" }, None);
            Err(ApiError::forbidden(if tries_left == 0 {
                crate::tr!("Три неверных кода: запрос закрыт. Попросите новый код.")
            } else {
                crate::tr!("Код не подошёл. Осталось попыток: {tries}.", tries = tries_left)
            }))
        }
        CodeOutcome::Right => {
            let grant = join(&app, &id, None).await?;
            app.joins.granted(&id, grant.clone(), false);
            changed(&app, &id, "joined", None);
            Ok(created(json!({ "status": JoinStatus::Joined, "grant": grant })))
        }
    }
}

async fn say(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<Value>,
) -> ApiResult<Value> {
    caller.zone_at_least(Zone::Home)?;
    let token = text(&body, "token").unwrap_or_default();
    let Some((code, name, kind)) = app.joins.say_again(&id, token, now_ms())? else {
        return Ok(ok(json!({ "said": false })));
    };
    let told = super::join_say::tell(app.clone(), &id, &name, kind, &code, now_ms());
    app.joins.told(&id, |kept| *kept = told.clone());
    Ok(created(json!({ "said": true, "told": told })))
}

async fn list(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Vec<JoinView>> {
    admin(&caller)?;
    Ok(ok(app.joins.list(now_ms())))
}

async fn approve(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<Value> {
    admin(&caller)?;
    app.joins.approve(&id, now_ms())?;
    let grant = join(&app, &id, caller.actor()).await?;
    app.joins.granted(&id, grant, true);
    changed(&app, &id, "joined", caller.actor());
    Ok(created(json!({ "status": JoinStatus::Joined })))
}

async fn deny(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<Value> {
    admin(&caller)?;
    app.joins.deny(&id, now_ms())?;
    changed(&app, &id, "denied", caller.actor());
    Ok(created(json!({ "status": JoinStatus::Denied })))
}

/// What joins the asker of a request that was let in: the pair's key of a station centre, or the link a computer
/// centre opens to the asker's own connection key. A failure closes the request and says why.
async fn join(app: &Arc<App>, id: &str, actor: Option<Actor>) -> Result<Value, ApiError> {
    let Some((kind, link, name)) = app.joins.kind_and_link(id) else {
        return Err(ApiError::not_found(crate::tr!("Такого запроса нет: он закончился или центр перезапустился.")));
    };
    let made = grant(app, kind, link, &name, actor).await;
    match made {
        Ok(grant) => {
            tracing::info!(target: "home", "join: «{name}» joined as {}", kind.as_str());
            Ok(grant)
        }
        Err(error) => {
            app.joins.failed(id, &error.message, now_ms());
            changed(app, id, "closed", None);
            Err(error)
        }
    }
}

async fn grant(
    app: &Arc<App>,
    kind: JoinKind,
    link: Option<LinkOffer>,
    name: &str,
    actor: Option<Actor>,
) -> Result<Value, ApiError> {
    use crate::placement::service::Role;
    if app.placement.role() == Role::Station {
        let key = app.placement.pair_key()?;
        // The key only: the asker reached this centre at an address and port it knows better than the centre does
        // (a port published elsewhere, a test's own socket).
        return Ok(json!({ "key": key }));
    }
    match (kind, link) {
        (JoinKind::Turbo, Some(link)) => {
            let id = crate::helpers::routes::pair_with_key(app, &link.url, &link.key, Some(name), actor).await?;
            Ok(json!({ "helper": id }))
        }
        (JoinKind::Turbo, None) => Err(ApiError::bad_request(crate::tr!(
            "Центр дома — компьютер: Турбо подключается к нему своим ключом подключения. Обновите DuoWave на этом компьютере."
        ))),
        (JoinKind::Ear, _) => Err(ApiError::conflict(crate::tr!(
            "Ухом колонка подключается к центру на колонке, а центр этого дома — компьютер."
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000_000;

    fn asked(joins: &Joins, source: &str) -> Asked {
        joins.ask(JoinKind::Turbo, "Ноутбук", None, None, source, NOW).unwrap()
    }

    /// The code lives 5 minutes and works once: right before its time — joined; the same code again — refused; after
    /// its time — closed, «прошло 5 минут».
    #[test]
    fn a_code_works_once_and_only_in_its_five_minutes() {
        let joins = Joins::new();
        let first = asked(&joins, "192.168.1.20");
        assert_eq!(first.code.len(), 6);
        assert!(first.code.chars().all(|c| c.is_ascii_digit()));
        let spaced = format!("{} {} {}", &first.code[..2], &first.code[2..4], &first.code[4..]);
        assert_eq!(joins.code(&first.id, &first.token, &spaced, NOW + 1000).unwrap(), CodeOutcome::Right);
        assert_eq!(joins.read(&first.id, &first.token, NOW + 2000).unwrap().status, JoinStatus::Joined);
        let again = joins.code(&first.id, &first.token, &first.code, NOW + 3000).unwrap_err();
        assert_eq!(again.status.as_u16(), 409, "once");

        let late = asked(&joins, "192.168.1.21");
        let at = NOW + CODE_MS + 1;
        let error = joins.code(&late.id, &late.token, &late.code, at).unwrap_err();
        assert_eq!(error.status.as_u16(), 409);
        assert!(error.message.contains("5 минут"), "{}", error.message);
        assert_eq!(joins.read(&late.id, &late.token, at).unwrap().status, JoinStatus::Closed);
    }

    /// Three wrong codes close the request: the right one after them does not join.
    #[test]
    fn three_wrong_codes_close_the_request() {
        let joins = Joins::new();
        let request = asked(&joins, "192.168.1.20");
        let wrong = if request.code == "000000" { "111111" } else { "000000" };
        assert_eq!(joins.code(&request.id, &request.token, wrong, NOW).unwrap(), CodeOutcome::Wrong { tries_left: 2 });
        assert_eq!(joins.code(&request.id, &request.token, wrong, NOW).unwrap(), CodeOutcome::Wrong { tries_left: 1 });
        assert_eq!(joins.code(&request.id, &request.token, wrong, NOW).unwrap(), CodeOutcome::Wrong { tries_left: 0 });
        let view = joins.read(&request.id, &request.token, NOW).unwrap();
        assert_eq!(view.status, JoinStatus::Closed);
        assert!(joins.code(&request.id, &request.token, &request.code, NOW).is_err(), "the right code too late");
    }

    /// A source that asks too often waits; so does the home with a few requests open at once.
    #[test]
    fn asking_is_limited_per_source_and_open_requests_at_once() {
        let joins = Joins::new();
        for at in 0..OPEN_AT_MOST {
            joins.ask(JoinKind::Turbo, "Ноутбук", None, None, &format!("192.168.1.{}", 30 + at), NOW).unwrap();
        }
        let crowded = joins.ask(JoinKind::Turbo, "Ещё", None, None, "192.168.1.99", NOW).unwrap_err();
        assert_eq!(crowded.status.as_u16(), 429, "{}", crowded.message);

        let joins = Joins::new();
        let source = "192.168.1.20";
        for _ in 0..ASKS {
            let request = asked(&joins, source);
            joins.deny(&request.id, NOW).unwrap();
        }
        let refused = joins.ask(JoinKind::Turbo, "Ноутбук", None, None, source, NOW).unwrap_err();
        assert_eq!(refused.status.as_u16(), 429);
        assert!(asked(&joins, "192.168.1.21").code.len() == 6, "another source asks as before");
    }

    /// «Разрешить» joins without the code; the grant goes to the asker once — the second look has none.
    #[test]
    fn approval_joins_without_the_code_and_the_grant_is_given_once() {
        let joins = Joins::new();
        let request = asked(&joins, "192.168.1.20");
        joins.approve(&request.id, NOW).unwrap();
        joins.granted(&request.id, json!({ "key": "the-pair-key" }), true);
        let first = joins.read(&request.id, &request.token, NOW).unwrap();
        assert_eq!((first.status, first.grant), (JoinStatus::Joined, Some(json!({ "key": "the-pair-key" }))));
        assert_eq!(joins.read(&request.id, &request.token, NOW).unwrap().grant, None, "once");
        assert!(joins.approve(&request.id, NOW).is_err() && joins.deny(&request.id, NOW).is_err());
    }

    /// Another device neither reads the request nor tries codes on it without its token; the admins' list has the code
    /// only while the request is open.
    #[test]
    fn a_stranger_neither_sees_nor_tries_the_code() {
        let joins = Joins::new();
        let request = asked(&joins, "192.168.1.20");
        assert_eq!(joins.read(&request.id, "not-the-token", NOW).unwrap_err().status.as_u16(), 403);
        let tried = joins.code(&request.id, "not-the-token", &request.code, NOW).unwrap_err();
        assert_eq!(tried.status.as_u16(), 403);
        assert_eq!(joins.read(&request.id, &request.token, NOW).unwrap().tries_left, TRIES, "no try spent");
        let asker = serde_json::to_value(joins.read(&request.id, &request.token, NOW).unwrap()).unwrap();
        assert!(!asker.to_string().contains(&request.code), "the asker's view never has the code: {asker}");
        assert_eq!(joins.list(NOW)[0].code.as_deref(), Some(request.code.as_str()));
        joins.deny(&request.id, NOW).unwrap();
        assert_eq!(joins.list(NOW)[0].code, None, "closed: no code");
    }

    /// «Сказать ещё раз» says it again once in a few seconds, and not on a closed request.
    #[test]
    fn saying_again_waits_a_few_seconds() {
        let joins = Joins::new();
        let request = asked(&joins, "192.168.1.20");
        assert_eq!(joins.say_again(&request.id, &request.token, NOW + 1000).unwrap(), None);
        let again = joins.say_again(&request.id, &request.token, NOW + SAY_AGAIN_MS).unwrap();
        assert_eq!(again.map(|(code, _, _)| code), Some(request.code.clone()));
        joins.deny(&request.id, NOW + SAY_AGAIN_MS).unwrap();
        assert!(joins.say_again(&request.id, &request.token, NOW + 3 * SAY_AGAIN_MS).is_err());
    }
}

/// The routes as a station centre serves them, from the home network.
#[cfg(test)]
mod route_tests {
    use std::net::SocketAddr;

    use axum::body::Body as HttpBody;
    use axum::extract::ConnectInfo;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use serde_json::{json, Value};
    use tower::ServiceExt;

    use crate::app::App;
    use crate::config::{Config, Env};

    fn station() -> std::sync::Arc<App> {
        let env = Env::from_pairs([("STATE_DIR", ""), ("DUOWAVE_ROLE", "station")]);
        App::build(Config::for_tests(&env)).unwrap()
    }

    async fn send(
        app: &std::sync::Arc<App>,
        method: &str,
        uri: &str,
        token: Option<&str>,
        body: Value,
        from: &str,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder().method(method).uri(uri).header("content-type", "application/json");
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let mut request = request.body(HttpBody::from(body.to_string())).unwrap();
        request.extensions_mut().insert(ConnectInfo(SocketAddr::new(from.parse().unwrap(), 5000)));
        let response = crate::app::router(app.clone()).oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    /// A computer asks in from the Wi-Fi with no sign-in; the admin sees the code, a person who is not an admin and an
    /// unsigned caller do not; the right code gives the pair's key — the one `placement/pair` gives the station's admin.
    #[tokio::test]
    async fn a_computer_joins_a_station_centre_by_the_code_only_the_admin_sees() {
        let app = station();
        let admin = app.access.auth.setup("admin-password-1", "Мама", "test").await.unwrap().unwrap().token;
        let (status, asked) = send(
            &app,
            "POST",
            "/api/v1/home/join",
            None,
            json!({ "kind": "turbo", "name": "Ноутбук" }),
            "192.168.1.20",
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{asked}");
        let (id, token) = (asked["message"]["id"].as_str().unwrap(), asked["message"]["token"].as_str().unwrap());
        assert!(asked["message"].get("code").is_none(), "the asker is never told the code: {asked}");

        let (status, _) = send(&app, "GET", "/api/v1/home/joins", None, Value::Null, "192.168.1.20").await;
        assert!(status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN, "{status}");
        let (status, list) = send(&app, "GET", "/api/v1/home/joins", Some(&admin), Value::Null, "192.168.1.21").await;
        assert_eq!(status, StatusCode::OK, "{list}");
        let code = list["message"][0]["code"].as_str().unwrap().to_string();
        assert_eq!(
            (list["message"][0]["name"].as_str(), list["message"][0]["kind"].as_str()),
            (Some("Ноутбук"), Some("turbo"))
        );

        let uri = format!("/api/v1/home/join/{id}/code");
        let (status, joined) =
            send(&app, "POST", &uri, None, json!({ "token": token, "code": code }), "192.168.1.20").await;
        assert_eq!(status, StatusCode::CREATED, "{joined}");
        assert_eq!(joined["message"]["grant"]["key"].as_str(), app.placement.peer_key(), "the pair's key");
    }

    /// «Разрешить» from the admin's panel: the asker's next look takes the key, once; «Это не я» closes another one.
    #[tokio::test]
    async fn the_admin_allows_or_turns_down_and_the_asker_reads_how_it_ended() {
        let app = station();
        let admin = app.access.auth.setup("admin-password-1", "Мама", "test").await.unwrap().unwrap().token;
        let ask = |name: &'static str| {
            let app = app.clone();
            async move {
                let body = json!({ "kind": "turbo", "name": name });
                let (_, asked) = send(&app, "POST", "/api/v1/home/join", None, body, "192.168.1.20").await;
                (
                    asked["message"]["id"].as_str().unwrap().to_string(),
                    asked["message"]["token"].as_str().unwrap().to_string(),
                )
            }
        };
        let (id, token) = ask("Ноутбук").await;
        let (status, _) =
            send(&app, "POST", &format!("/api/v1/home/join/{id}/approve"), Some(&admin), json!({}), "192.168.1.21")
                .await;
        assert_eq!(status, StatusCode::CREATED);
        let look = format!("/api/v1/home/join/{id}?token={token}");
        let (_, first) = send(&app, "GET", &look, None, Value::Null, "192.168.1.20").await;
        assert_eq!(first["message"]["status"], "joined");
        assert_eq!(first["message"]["grant"]["key"].as_str(), app.placement.peer_key());
        let (_, second) = send(&app, "GET", &look, None, Value::Null, "192.168.1.20").await;
        assert!(second["message"].get("grant").is_none(), "once: {second}");

        let (other, other_token) = ask("Чужой").await;
        send(&app, "POST", &format!("/api/v1/home/join/{other}/deny"), Some(&admin), json!({}), "192.168.1.21").await;
        let (_, denied) = send(
            &app,
            "GET",
            &format!("/api/v1/home/join/{other}?token={other_token}"),
            None,
            Value::Null,
            "192.168.1.20",
        )
        .await;
        assert_eq!(denied["message"]["status"], "denied");
        assert!(denied["message"].get("grant").is_none());
    }
}
