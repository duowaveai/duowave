//! The router's API (docs/plans/router-and-works.md, «API»): the deals, their sums by day, a price written by hand;
//! the seats, the table, the frames and the budgets (stage 2). For whoever may change the gateway's settings — the
//! admin.
//!
//! - `GET /api/v1/router` → `{seats, placed, table, tableDefault, frames: {away}, budgets}`: the seats as set (only
//!   «Исполнитель» has its row here; «Мозг» keeps the rest), which seats a model is put on, the table that holds,
//!   whether it is the default, the regulator, the general budget.
//! - `PUT /api/v1/router/seats` — `{worker: {agent?, model?}}`; an empty agent — Claude Code, a model `auto` or empty —
//!   the agent's own. The answer is `GET`'s.
//! - `PUT /api/v1/router/table` — `{schema: 1, rules: [{id, seat, when?: {budget?, agent?, tools?}, model}]}`;
//!   `{}` — back to the default. It holds from the next ask. The answer is `GET`'s.
//! - `GET /api/v1/router/budgets` → `{works}`; `PUT` the same — `{works: {measure, limit, steps}}`, `{}` — back to the
//!   default (five dollars a work).
//! - `PUT /api/v1/works/{id}/budget` — a work's own budget `{measure?, limit, steps?}`; `{}` or `null` takes it away.
//!   → `{work, budget, scope}`: its own budget, or none.
//!
//! - `GET /api/v1/router/deals?from=&to=&seat=&work=&limit=` — the deals, the newest first; `from` and `to` are ms
//!   since the epoch, a moment (RFC 3339) or a day `YYYY-MM-DD` (UTC; `to` a day — to its end).
//! - `GET /api/v1/router/stats?by=seat,model&from=&to=` — the days summed by `day`, `seat`, `provider`, `model`,
//!   `via`, `billing`, and the total.
//! - `GET /api/v1/router/prices/{provider}/{model}` — the price a deal of the model is counted at now, whether it is
//!   written by hand, and the price without the written one (the provider's or the snapshot's): what the panel shows
//!   before one writes a price or takes it away.
//! - `PUT /api/v1/router/prices/{provider}/{model}` — `{input, output, cacheRead?, cacheWrite?, context?}` in dollars
//!   per million tokens; `{}` takes the written price away. The model may hold slashes (`anthropic/claude-…`). The
//!   answer is the same as `GET`'s.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query};
use axum::routing::{get, put};
use axum::Router;
use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;
use serde_json::{Map, Value};

use super::budgets::{Budget, Budgets};
use super::deals::{day_of, By, Deal, DealsQuery, StatsQuery, Sums};
use super::policy::Table;
use super::pool::{Price, PriceSource, Written};
use super::seats::Seats;
use super::settings::{BUDGETS, ROUTER_SEATS, ROUTER_TABLE};
use super::Seat;
use crate::core::identity::permissions::SETTINGS_EDIT;
use crate::platform::http::{ok, ApiError, ApiResult, Body, Caller, Provides, RouteState, Uses};
use crate::settings::SettingsService;
use crate::talk::TalkLayer;

/// The biggest price one may write: dollars per million tokens.
const PRICE_MAX: f64 = 100_000.0;
const DAY_MS: u64 = 86_400_000;

/// The router's routes use the router, the settings that keep its seats, table and budgets, and the talk layer: a
/// work's budget is set on a work it knows.
pub fn routes<S>() -> Router<S>
where
    S: RouteState + Provides<Arc<super::Router>> + Provides<Arc<SettingsService>> + Provides<Arc<TalkLayer>>,
{
    Router::new()
        .route("/api/v1/router", get(router_view))
        .route("/api/v1/router/seats", put(set_seats))
        .route("/api/v1/router/table", put(set_table))
        .route("/api/v1/router/budgets", get(budgets).put(set_budgets))
        .route("/api/v1/work-sets", get(work_sets).put(set_work_sets))
        .route("/api/v1/works/{id}/budget", put(set_work_budget))
        .route("/api/v1/router/deals", get(deals))
        .route("/api/v1/router/stats", get(stats))
        .route("/api/v1/router/prices/{provider}/{*model}", get(price_now).put(price))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RouterView {
    seats: Seats,
    /// The seats a model is put on, in order; the others are shown only in «Расход».
    placed: Vec<Seat>,
    table: Table,
    /// The table is the default built in: none is set, or the one set is not ours to read.
    table_default: bool,
    frames: Frames,
    budgets: Budgets,
}

#[derive(Debug, Serialize)]
struct Frames {
    away: crate::core::helpers::Away,
}

fn view_of(router: &super::Router) -> RouterView {
    let table = router.table();
    RouterView {
        seats: router.seats(),
        placed: Seat::PLACED.to_vec(),
        table_default: table == Table::default(),
        table,
        frames: Frames { away: router.away() },
        budgets: router.general(),
    }
}

async fn router_view(Uses(router): Uses<Arc<super::Router>>, caller: Caller) -> ApiResult<RouterView> {
    caller.permitted(SETTINGS_EDIT)?;
    Ok(ok(view_of(&router)))
}

/// A body that is an object, or nothing: `{}` and `null` are «back to the default».
/// The sets of works (router/sets.rs): every one there is, one's own, the home's default.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SetsView {
    sets: Vec<super::sets::ModelSet>,
    #[serde(skip_serializing_if = "Option::is_none")]
    default: Option<String>,
    own: Vec<super::sets::ModelSet>,
}

fn sets_view(sets: super::sets::WorkSets) -> SetsView {
    SetsView { sets: super::sets::all(&sets), default: sets.default, own: sets.own }
}

async fn work_sets(Uses(settings): Uses<Arc<SettingsService>>, caller: Caller) -> ApiResult<SetsView> {
    caller.permitted(SETTINGS_EDIT)?;
    Ok(ok(sets_view(settings.get(&super::sets::WORK_SETS, None).unwrap_or_default())))
}

/// One's own sets and the home's default, whole: `{default?, own: [{id, title, models: {agent: {low, mid, high}}}]}`.
async fn set_work_sets(
    Uses(settings): Uses<Arc<SettingsService>>,
    caller: Caller,
    Body(asked): Body<super::sets::WorkSets>,
) -> ApiResult<SetsView> {
    caller.permitted(SETTINGS_EDIT)?;
    let mut asked = asked;
    for set in &mut asked.own {
        set.bundled = false;
    }
    super::sets::check_all(&asked).map_err(ApiError::bad_request)?;
    settings.set(&super::sets::WORK_SETS, &asked, caller.actor())?;
    Ok(ok(sets_view(asked)))
}

fn given(body: &Value) -> Option<&Value> {
    match body {
        Value::Null => None,
        Value::Object(map) if map.is_empty() => None,
        other => Some(other),
    }
}

async fn set_seats(
    Uses(router): Uses<Arc<super::Router>>,
    Uses(settings): Uses<Arc<SettingsService>>,
    Uses(talk): Uses<Arc<TalkLayer>>,
    caller: Caller,
    Body(body): Body<Value>,
) -> ApiResult<RouterView> {
    caller.permitted(SETTINGS_EDIT)?;
    match given(&body) {
        None => settings.forget(&ROUTER_SEATS)?,
        Some(body) => {
            let mut seats: Seats = serde_json::from_value(body.clone())
                .map_err(|_| ApiError::bad_request(crate::tr!("Места — поле worker с agent и model.")))?;
            let worker = &mut seats.worker;
            worker.agent = worker.agent.as_deref().map(str::trim).filter(|agent| !agent.is_empty()).map(Into::into);
            worker.model = worker.model.as_deref().map(str::trim).filter(|model| !model.is_empty()).map(Into::into);
            if worker.agent.as_deref().is_some_and(|agent| agent.len() > 100)
                || worker.model.as_deref().is_some_and(|model| model.len() > 200)
            {
                return Err(ApiError::field("worker", crate::tr!("Слишком длинное имя агента или модели.")));
            }
            // The agents a computer's works know (its port); a gateway without works takes the name as it is.
            let port = talk.folders().and_then(|folders| folders.port());
            if let (Some(agent), Some(port)) = (worker.agent.as_deref(), port) {
                if !port.agents().iter().any(|(id, _)| id == agent) {
                    return Err(ApiError::field(
                        "worker.agent",
                        crate::tr!("Такого агента нет: поставьте его карточку."),
                    ));
                }
            }
            settings.set(&ROUTER_SEATS, &seats, caller.actor())?;
        }
    }
    Ok(ok(view_of(&router)))
}

async fn set_table(
    Uses(router): Uses<Arc<super::Router>>,
    Uses(settings): Uses<Arc<SettingsService>>,
    caller: Caller,
    Body(body): Body<Value>,
) -> ApiResult<RouterView> {
    caller.permitted(SETTINGS_EDIT)?;
    match given(&body) {
        None => settings.forget(&ROUTER_TABLE)?,
        Some(body) => {
            let table: Table = serde_json::from_value(body.clone()).map_err(|error| {
                ApiError::bad_request(crate::tr!("Таблицу не прочитать: {error}", error = error.to_string()))
            })?;
            table.check().map_err(ApiError::bad_request)?;
            settings.set(&ROUTER_TABLE, &table, caller.actor())?;
        }
    }
    // From the next ask on, without waiting for the event.
    router.forget_view();
    Ok(ok(view_of(&router)))
}

async fn budgets(Uses(router): Uses<Arc<super::Router>>, caller: Caller) -> ApiResult<Budgets> {
    caller.permitted(SETTINGS_EDIT)?;
    Ok(ok(router.general()))
}

async fn set_budgets(
    Uses(router): Uses<Arc<super::Router>>,
    Uses(settings): Uses<Arc<SettingsService>>,
    caller: Caller,
    Body(body): Body<Value>,
) -> ApiResult<Budgets> {
    caller.permitted(SETTINGS_EDIT)?;
    match given(&body) {
        None => settings.forget(&BUDGETS)?,
        Some(body) => {
            let budgets: Budgets = serde_json::from_value(body.clone()).map_err(|_| {
                ApiError::bad_request(crate::tr!("Общий бюджет — поле works с measure, limit и steps."))
            })?;
            budgets.works.check().map_err(|why| ApiError::field("works", why))?;
            settings.set(&BUDGETS, &budgets, caller.actor())?;
        }
    }
    Ok(ok(router.general()))
}

#[derive(Debug, Serialize)]
struct WorkBudget {
    work: String,
    budget: Option<Budget>,
}

async fn set_work_budget(
    Uses(router): Uses<Arc<super::Router>>,
    Uses(talk): Uses<Arc<TalkLayer>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<Value>,
) -> ApiResult<WorkBudget> {
    caller.permitted(SETTINGS_EDIT)?;
    let since = talk.began_at(&id).ok_or_else(|| ApiError::not_found(crate::tr!("Такой работы нет.")))?;
    let budget = match given(&body) {
        None => None,
        Some(body) => {
            let budget: Budget = serde_json::from_value(body.clone())
                .map_err(|_| ApiError::bad_request(crate::tr!("Бюджет — объект с полями measure, limit и steps.")))?;
            budget.check().map_err(ApiError::bad_request)?;
            Some(budget)
        }
    };
    router.budgets.set(&id, since, budget.as_ref(), crate::utils::time::now_ms())?;
    // A work waiting on its budget's question goes on when its budget grew: the door is asked again.
    if budget.is_some() {
        talk.budget_changed(&id);
    }
    Ok(ok(WorkBudget { work: id, budget }))
}

#[derive(Debug, Serialize)]
struct DealList {
    deals: Vec<Deal>,
}

async fn deals(
    Uses(router): Uses<Arc<super::Router>>,
    caller: Caller,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult<DealList> {
    caller.permitted(SETTINGS_EDIT)?;
    let given = |name: &str| query.get(name).map(|value| value.trim()).filter(|value| !value.is_empty());
    let limit = match given("limit") {
        Some(limit) => Some(limit.parse::<usize>().map_err(|_| ApiError::field("limit", "a whole number"))?),
        None => None,
    };
    let asked = DealsQuery {
        from: given("from").map(|from| moment(from, false)).transpose()?,
        to: given("to").map(|to| moment(to, true)).transpose()?,
        seat: given("seat").map(str::to_string),
        work: given("work").map(str::to_string),
        limit,
    };
    Ok(ok(DealList { deals: router.deals.list(&asked)? }))
}

#[derive(Debug, Serialize)]
struct Stats {
    by: Vec<&'static str>,
    rows: Vec<Sums>,
    total: Sums,
}

async fn stats(
    Uses(router): Uses<Arc<super::Router>>,
    caller: Caller,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult<Stats> {
    caller.permitted(SETTINGS_EDIT)?;
    let given = |name: &str| query.get(name).map(|value| value.trim()).filter(|value| !value.is_empty());
    let mut by = Vec::new();
    for name in given("by").unwrap_or("").split(',').map(str::trim).filter(|name| !name.is_empty()) {
        let Some(one) = By::parse(name) else {
            let names: Vec<&str> = By::ALL.iter().map(|by| by.as_str()).collect();
            return Err(ApiError::field("by", crate::tr!("Итоги считаются по: {names}.", names = names.join(", "))));
        };
        if !by.contains(&one) {
            by.push(one);
        }
    }
    let from = given("from").map(|from| moment(from, false).map(day_of)).transpose()?;
    // `to` a day is that whole day; a moment — the day it falls in.
    let to = given("to").map(|to| moment(to, false).map(day_of)).transpose()?;
    let asked = StatsQuery { from, to, by: by.clone() };
    let rows = router.deals.stats(&asked)?;
    let total = router.deals.stats(&StatsQuery { by: Vec::new(), ..asked })?.into_iter().next().unwrap_or_default();
    Ok(ok(Stats { by: by.iter().map(|by| by.as_str()).collect(), rows, total }))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PriceNow {
    provider: String,
    model: String,
    /// The price a deal of this model is counted at now: the written one, else the provider's or the snapshot's.
    price: Option<Price>,
    #[serde(skip_serializing_if = "Option::is_none")]
    context: Option<u64>,
    /// A price is written by hand.
    written: bool,
    /// The price without the written one: the provider's or the snapshot's; what taking it away brings back.
    unwritten: Option<Price>,
    /// The provider signs in with a plan: its dollars are what the key would cost.
    plan: bool,
}

/// What the pool says of a model's price now.
fn price_of(router: &super::Router, provider: String, model: String) -> PriceNow {
    let book = router.pool.book(&provider);
    let preset = book.preset.as_deref();
    let price = router.pool.price(&provider, preset, &model);
    PriceNow {
        written: price.as_ref().is_some_and(|price| price.from == PriceSource::Manual),
        unwritten: router.pool.unwritten(&provider, preset, &model),
        context: router.pool.context(&provider, preset, &model),
        plan: book.plan,
        price,
        provider,
        model,
    }
}

/// The provider and the model of the path, trimmed and checked.
fn named(provider: String, model: String) -> Result<(String, String), ApiError> {
    let (provider, model) = (provider.trim().to_string(), model.trim().trim_start_matches('/').to_string());
    if provider.is_empty() || model.is_empty() || provider.len() > 200 || model.len() > 300 {
        return Err(ApiError::bad_request(crate::tr!("Нужны провайдер и модель.")));
    }
    Ok((provider, model))
}

async fn price_now(
    Uses(router): Uses<Arc<super::Router>>,
    caller: Caller,
    Path((provider, model)): Path<(String, String)>,
) -> ApiResult<PriceNow> {
    caller.permitted(SETTINGS_EDIT)?;
    let (provider, model) = named(provider, model)?;
    Ok(ok(price_of(&router, provider, model)))
}

async fn price(
    Uses(router): Uses<Arc<super::Router>>,
    caller: Caller,
    Path((provider, model)): Path<(String, String)>,
    Body(body): Body<Map<String, Value>>,
) -> ApiResult<PriceNow> {
    caller.permitted(SETTINGS_EDIT)?;
    let (provider, model) = named(provider, model)?;
    let written = written(&body)?;
    router.pool.write(&provider, &model, written.as_ref())?;
    Ok(ok(price_of(&router, provider, model)))
}

/// The price in the body; None — none written (it is taken away).
fn written(body: &Map<String, Value>) -> Result<Option<Written>, ApiError> {
    let dollars = |field: &str| -> Result<Option<f64>, ApiError> {
        match body.get(field) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Number(number)) => match number.as_f64() {
                Some(value) if value.is_finite() && (0.0..=PRICE_MAX).contains(&value) => Ok(Some(value)),
                _ => Err(ApiError::field(
                    field,
                    crate::tr!("Цена — число долларов за миллион токенов, от 0 до 100 000."),
                )),
            },
            Some(_) => {
                Err(ApiError::field(field, crate::tr!("Цена — число долларов за миллион токенов, от 0 до 100 000.")))
            }
        }
    };
    let price =
        Written {
            input: dollars("input")?,
            output: dollars("output")?,
            cache_read: dollars("cacheRead")?,
            cache_write: dollars("cacheWrite")?,
            context: match body.get("context") {
                None | Some(Value::Null) => None,
                Some(value) => Some(value.as_u64().filter(|context| *context > 0).ok_or_else(|| {
                    ApiError::field("context", crate::tr!("Предел контекста — целое число токенов."))
                })?),
            },
        };
    if price == Written::default() {
        return Ok(None);
    }
    if price.input.is_none() || price.output.is_none() {
        return Err(ApiError::bad_request(crate::tr!("Нужны цена входа и цена выхода.")));
    }
    Ok(Some(price))
}

/// A moment in ms since the epoch: a number of them, RFC 3339, or a day by UTC — its start, or with `end` its end
/// (the next day's start).
fn moment(text: &str, end: bool) -> Result<u64, ApiError> {
    if let Ok(ms) = text.parse::<u64>() {
        return Ok(ms);
    }
    if let Ok(at) = DateTime::parse_from_rfc3339(text) {
        return Ok(at.with_timezone(&Utc).timestamp_millis().max(0) as u64);
    }
    if let Ok(day) = NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        let start = day.and_hms_opt(0, 0, 0).map(|at| at.and_utc().timestamp_millis().max(0) as u64).unwrap_or(0);
        return Ok(if end { start + DAY_MS } else { start });
    }
    Err(ApiError::bad_request(crate::tr!("Время — число миллисекунд, момент RFC 3339 или день ГГГГ-ММ-ДД.")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_a_moment_in_ms_a_moment_or_a_day() {
        assert_eq!(moment("1794700800000", false).unwrap(), 1_794_700_800_000);
        assert_eq!(moment("2026-11-15", false).unwrap(), 1_794_700_800_000);
        assert_eq!(moment("2026-11-15", true).unwrap(), 1_794_700_800_000 + DAY_MS);
        assert_eq!(moment("2026-11-15T03:00:00+03:00", false).unwrap(), 1_794_700_800_000);
        assert!(moment("вчера", false).is_err());
    }

    #[test]
    fn reads_a_price_written_by_hand() {
        let body = |value: Value| value.as_object().cloned().unwrap();
        let full = written(&body(json!({"input": 3, "output": 15.5, "cacheRead": 0.3, "context": 200000}))).unwrap();
        assert_eq!(
            full,
            Some(Written {
                input: Some(3.0),
                output: Some(15.5),
                cache_read: Some(0.3),
                cache_write: None,
                context: Some(200000)
            })
        );
        assert_eq!(written(&body(json!({}))).unwrap(), None, "nothing: taken away");
        assert_eq!(written(&body(json!({"input": null, "output": null}))).unwrap(), None);
        assert!(written(&body(json!({"input": 3}))).is_err(), "the output's price too");
        assert!(written(&body(json!({"input": -1, "output": 1}))).is_err());
        assert!(written(&body(json!({"input": "3", "output": 1}))).is_err());
        assert!(written(&body(json!({"input": 1, "output": 1, "context": 1.5}))).is_err());
    }
}
