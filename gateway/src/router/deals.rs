//! The journal of deals (docs/plans/router-and-works.md, block A, 1): one row per answer of a model, written after the
//! answer with its day's sums in the same transaction, so the days always add up to the deals. Deals are kept 90
//! days, the days a year (the owner's default 14). No text of what was asked or answered.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension, Row};
use serde::Serialize;

use super::pool::Price;
use super::Tokens;
use crate::storage::Db;

/// The gateway called the model itself.
pub const VIA_BRAIN: &str = "brain";
/// Deals are kept this long.
pub const KEEP_DEALS_DAYS: u64 = 90;
/// The days of the deals are kept this long.
pub const KEEP_DAYS_DAYS: u64 = 365;
/// A list of deals holds at most this many.
pub const LIST_MAX: usize = 500;
const LIST_DEFAULT: usize = 100;
const DAY_MS: u64 = 86_400_000;

/// A deal as it is kept and listed.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Deal {
    pub id: i64,
    /// When the answer ended, ms since the epoch.
    pub at: u64,
    pub seat: String,
    pub via: String,
    pub provider: String,
    pub model: String,
    pub place: String,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub part: Option<String>,
    pub tokens: Tokens,
    pub rounds: u32,
    pub ms: u64,
    /// Millionths of a dollar; null — no price is known.
    pub usd_micro: Option<u64>,
    /// The price it was counted at.
    pub price: Option<Price>,
    pub billing: String,
    pub outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check: Option<String>,
}

/// Which deals: from and to (ms since the epoch, `to` not included), on a seat, of a work; the newest first.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DealsQuery {
    pub from: Option<u64>,
    pub to: Option<u64>,
    pub seat: Option<String>,
    pub work: Option<String>,
    pub limit: Option<usize>,
}

/// What the days are summed by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum By {
    Day,
    Seat,
    Provider,
    Model,
    Via,
    Billing,
}

impl By {
    pub const ALL: [By; 6] = [By::Day, By::Seat, By::Provider, By::Model, By::Via, By::Billing];

    pub fn as_str(self) -> &'static str {
        match self {
            By::Day => "day",
            By::Seat => "seat",
            By::Provider => "provider",
            By::Model => "model",
            By::Via => "via",
            By::Billing => "billing",
        }
    }

    pub fn parse(text: &str) -> Option<By> {
        By::ALL.into_iter().find(|by| by.as_str() == text)
    }
}

/// Which days and summed how: from and to (`YYYY-MM-DD`, both included), by what.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StatsQuery {
    pub from: Option<String>,
    pub to: Option<String>,
    pub by: Vec<By>,
}

/// One line of the sums: the values it is summed by (`by`), then the sums.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sums {
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub of: std::collections::BTreeMap<String, String>,
    pub deals: u64,
    pub tokens: Tokens,
    pub rounds: u64,
    pub ms: u64,
    /// The dollars of the deals with a price, in millionths.
    pub usd_micro: u64,
    /// How many deals had no price: the dollars leave them out.
    pub unpriced: u64,
}

/// Who answered a work: the provider, the model, the seat, and where it is — `home`, `mine`, `cloud`; unknown while a
/// round is only heard and no deal is written yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Answered {
    pub provider: String,
    pub model: String,
    pub seat: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub place: Option<String>,
}

/// One work's deals summed, and who answered the newest of them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorkDeals {
    pub sums: Sums,
    pub last: Option<Answered>,
}

pub struct Deals {
    db: Arc<Db>,
}

impl Deals {
    pub fn new(db: Arc<Db>) -> Deals {
        Deals { db }
    }

    /// A deal and its day, in one transaction: the new deal's id.
    pub fn write(&self, deal: &Deal) -> rusqlite::Result<i64> {
        let price = deal.price.as_ref().map(|price| serde_json::to_string(price).unwrap_or_default());
        let usd = deal.usd_micro.map(|usd| usd as i64);
        self.db.transaction(|t| {
            t.execute(
                "INSERT INTO model_deals (at, seat, via, provider, model, place, reason, request, work, part, tokens_in,
                    tokens_out, tokens_cache_read, tokens_cache_write, rounds, ms, usd_micro, price, billing, outcome,
                    checked)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)",
                params![
                    deal.at as i64,
                    deal.seat,
                    deal.via,
                    deal.provider,
                    deal.model,
                    deal.place,
                    deal.reason,
                    deal.request,
                    deal.work,
                    deal.part,
                    deal.tokens.input as i64,
                    deal.tokens.output as i64,
                    deal.tokens.cache_read as i64,
                    deal.tokens.cache_write as i64,
                    deal.rounds as i64,
                    deal.ms as i64,
                    usd,
                    price,
                    deal.billing,
                    deal.outcome,
                    deal.check,
                ],
            )?;
            let id = t.last_insert_rowid();
            t.execute(
                "INSERT INTO model_deals_days (day, seat, provider, model, via, billing, deals, tokens_in, tokens_out,
                    tokens_cache_read, tokens_cache_write, rounds, ms, usd_micro, unpriced)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                 ON CONFLICT (day, seat, provider, model, via, billing) DO UPDATE SET
                    deals = deals + 1,
                    tokens_in = tokens_in + excluded.tokens_in,
                    tokens_out = tokens_out + excluded.tokens_out,
                    tokens_cache_read = tokens_cache_read + excluded.tokens_cache_read,
                    tokens_cache_write = tokens_cache_write + excluded.tokens_cache_write,
                    rounds = rounds + excluded.rounds,
                    ms = ms + excluded.ms,
                    usd_micro = usd_micro + excluded.usd_micro,
                    unpriced = unpriced + excluded.unpriced",
                params![
                    day_of(deal.at),
                    deal.seat,
                    deal.provider,
                    deal.model,
                    deal.via,
                    deal.billing,
                    deal.tokens.input as i64,
                    deal.tokens.output as i64,
                    deal.tokens.cache_read as i64,
                    deal.tokens.cache_write as i64,
                    deal.rounds as i64,
                    deal.ms as i64,
                    usd.unwrap_or(0),
                    i64::from(usd.is_none()),
                ],
            )?;
            Ok(id)
        })
    }

    /// The deals asked for, the newest first.
    pub fn list(&self, query: &DealsQuery) -> rusqlite::Result<Vec<Deal>> {
        let limit = query.limit.unwrap_or(LIST_DEFAULT).clamp(1, LIST_MAX) as i64;
        self.db.with(|c| {
            let mut statement = c.prepare(
                "SELECT id, at, seat, via, provider, model, place, reason, request, work, part, tokens_in, tokens_out,
                        tokens_cache_read, tokens_cache_write, rounds, ms, usd_micro, price, billing, outcome, checked
                 FROM model_deals
                 WHERE (?1 IS NULL OR at >= ?1) AND (?2 IS NULL OR at < ?2) AND (?3 IS NULL OR seat = ?3)
                   AND (?4 IS NULL OR work = ?4)
                 ORDER BY at DESC, id DESC LIMIT ?5",
            )?;
            let rows = statement.query_map(
                params![
                    query.from.map(|from| from as i64),
                    query.to.map(|to| to as i64),
                    query.seat,
                    query.work,
                    limit
                ],
                deal_of,
            )?;
            rows.collect()
        })
    }

    /// The days summed by `by` (nothing — one line for all), the biggest spending first.
    pub fn stats(&self, query: &StatsQuery) -> rusqlite::Result<Vec<Sums>> {
        // The columns come from `By`, never from what was asked: the query is built of known names only.
        let columns: Vec<&str> = query.by.iter().map(|by| by.as_str()).collect();
        let group = if columns.is_empty() { String::new() } else { format!(" GROUP BY {}", columns.join(", ")) };
        let picked = columns.iter().map(|column| format!("{column}, ")).collect::<String>();
        let sql = format!(
            "SELECT {picked}SUM(deals), SUM(tokens_in), SUM(tokens_out), SUM(tokens_cache_read),
                    SUM(tokens_cache_write), SUM(rounds), SUM(ms), SUM(usd_micro), SUM(unpriced)
             FROM model_deals_days WHERE (?1 IS NULL OR day >= ?1) AND (?2 IS NULL OR day <= ?2){group}
             ORDER BY SUM(usd_micro) DESC, SUM(deals) DESC"
        );
        self.db.with(|c| {
            let mut statement = c.prepare(&sql)?;
            let rows = statement.query_map(params![query.from, query.to], |row| {
                let mut of = std::collections::BTreeMap::new();
                for (index, column) in columns.iter().enumerate() {
                    of.insert(column.to_string(), row.get::<_, String>(index)?);
                }
                let n = columns.len();
                let sum = |index: usize| -> rusqlite::Result<u64> {
                    Ok(row.get::<_, Option<i64>>(n + index)?.unwrap_or(0).max(0) as u64)
                };
                Ok(Sums {
                    of,
                    deals: sum(0)?,
                    tokens: Tokens { input: sum(1)?, output: sum(2)?, cache_read: sum(3)?, cache_write: sum(4)? },
                    rounds: sum(5)?,
                    ms: sum(6)?,
                    usd_micro: sum(7)?,
                    unpriced: sum(8)?,
                })
            })?;
            // With nothing to sum by and no days, SQLite still gives one line of empty sums: none.
            rows.filter(|row| !matches!(row, Ok(sums) if sums.deals == 0)).collect()
        })
    }

    /// What one work's deals came to since it began (the page «Работы», decision 17): the sums, and who answered its
    /// newest deal. `since` keeps out the deals of an older work with the same id — ids start over once the old works
    /// are gone (talk/works.rs keeps them a month, deals stay 90 days).
    pub fn of_work(&self, work: &str, since: u64) -> rusqlite::Result<WorkDeals> {
        self.db.with(|c| {
            let sums = c.query_row(
                "SELECT COUNT(*), SUM(tokens_in), SUM(tokens_out), SUM(tokens_cache_read), SUM(tokens_cache_write),
                        SUM(rounds), SUM(ms), SUM(usd_micro), SUM(usd_micro IS NULL)
                 FROM model_deals WHERE work = ?1 AND at >= ?2",
                params![work, since as i64],
                |row| {
                    let sum = |index: usize| -> rusqlite::Result<u64> {
                        Ok(row.get::<_, Option<i64>>(index)?.unwrap_or(0).max(0) as u64)
                    };
                    Ok(Sums {
                        of: Default::default(),
                        deals: sum(0)?,
                        tokens: Tokens { input: sum(1)?, output: sum(2)?, cache_read: sum(3)?, cache_write: sum(4)? },
                        rounds: sum(5)?,
                        ms: sum(6)?,
                        usd_micro: sum(7)?,
                        unpriced: sum(8)?,
                    })
                },
            )?;
            let last = c
                .query_row(
                    "SELECT provider, model, seat, place FROM model_deals WHERE work = ?1 AND at >= ?2
                     ORDER BY at DESC, id DESC LIMIT 1",
                    params![work, since as i64],
                    |row| {
                        Ok(Answered {
                            provider: row.get(0)?,
                            model: row.get(1)?,
                            seat: row.get(2)?,
                            place: Some(row.get(3)?),
                        })
                    },
                )
                .optional()?;
            Ok(WorkDeals { sums, last })
        })
    }

    /// The old ones go: deals older than 90 days, days older than a year. How many of each went.
    pub fn prune(&self, now: u64) -> rusqlite::Result<(usize, usize)> {
        let deals_before = now.saturating_sub(KEEP_DEALS_DAYS * DAY_MS) as i64;
        let days_before = day_of(now.saturating_sub(KEEP_DAYS_DAYS * DAY_MS));
        self.db.transaction(|t| {
            let deals = t.execute("DELETE FROM model_deals WHERE at < ?1", params![deals_before])?;
            let days = t.execute("DELETE FROM model_deals_days WHERE day < ?1", params![days_before])?;
            Ok((deals, days))
        })
    }
}

/// The day of a moment, by UTC: `YYYY-MM-DD`.
pub fn day_of(ms: u64) -> String {
    DateTime::<Utc>::from_timestamp_millis(ms as i64).unwrap_or_default().format("%Y-%m-%d").to_string()
}

fn deal_of(row: &Row<'_>) -> rusqlite::Result<Deal> {
    let count = |index: usize| -> rusqlite::Result<u64> { Ok(row.get::<_, i64>(index)?.max(0) as u64) };
    let price: Option<String> = row.get(18)?;
    Ok(Deal {
        id: row.get(0)?,
        at: count(1)?,
        seat: row.get(2)?,
        via: row.get(3)?,
        provider: row.get(4)?,
        model: row.get(5)?,
        place: row.get(6)?,
        reason: row.get(7)?,
        request: row.get(8)?,
        work: row.get(9)?,
        part: row.get(10)?,
        tokens: Tokens { input: count(11)?, output: count(12)?, cache_read: count(13)?, cache_write: count(14)? },
        rounds: count(15)? as u32,
        ms: count(16)?,
        usd_micro: row.get::<_, Option<i64>>(17)?.map(|usd| usd.max(0) as u64),
        price: price.and_then(|price| serde_json::from_str(&price).ok()),
        billing: row.get(19)?,
        outcome: row.get(20)?,
        check: row.get(21)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::pool::PriceSource;

    fn deal(at: u64, seat: &str, model: &str, usd: Option<u64>, tokens: Tokens) -> Deal {
        Deal {
            id: 0,
            at,
            seat: seat.into(),
            via: VIA_BRAIN.into(),
            provider: "openrouter".into(),
            model: model.into(),
            place: "cloud".into(),
            reason: "seat".into(),
            request: Some("r1".into()),
            work: None,
            part: None,
            tokens,
            rounds: 2,
            ms: 1500,
            usd_micro: usd,
            price: usd.map(|_| Price {
                input: Some(3.0),
                output: Some(15.0),
                cache_read: Some(0.3),
                cache_write: None,
                from: PriceSource::Snapshot,
            }),
            billing: "key".into(),
            outcome: "ok".into(),
            check: None,
        }
    }

    const DAY: u64 = 86_400_000;
    /// 2026-11-15 00:00 UTC.
    const NOV_15: u64 = 1_794_700_800_000;

    #[test]
    fn writes_a_deal_with_its_day_and_reads_it_back() {
        let deals = Deals::new(Arc::new(Db::memory()));
        let tokens = Tokens { input: 20, output: 40, cache_read: 160, cache_write: 0 };
        let id = deals.write(&deal(NOV_15 + 1000, "smart", "m", Some(708), tokens)).unwrap();
        let listed = deals.list(&DealsQuery::default()).unwrap();
        assert_eq!(listed, vec![Deal { id, ..deal(NOV_15 + 1000, "smart", "m", Some(708), tokens) }]);
        let json = serde_json::to_value(&listed[0]).unwrap();
        assert_eq!(json["usdMicro"], 708);
        assert_eq!(json["price"]["from"], "snapshot");
        assert_eq!(json["tokens"]["cacheRead"], 160);
        assert!(json.get("work").is_none() && json.get("check").is_none(), "{json}");
    }

    #[test]
    fn the_days_add_up_to_the_deals() {
        let deals = Deals::new(Arc::new(Db::memory()));
        let t = |input| Tokens { input, output: 1, cache_read: 0, cache_write: 0 };
        for (at, seat, model, usd, tokens) in [
            (NOV_15 + 10, "fast", "a", Some(100), t(10)),
            (NOV_15 + 20, "fast", "a", Some(50), t(5)),
            (NOV_15 + 30, "fast", "a", None, t(7)),
            (NOV_15 + 40, "smart", "b", Some(1000), t(70)),
            (NOV_15 + DAY + 5, "fast", "a", Some(1), t(1)),
        ] {
            deals.write(&deal(at, seat, model, usd, tokens)).unwrap();
        }
        let all = deals.stats(&StatsQuery::default()).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!((all[0].deals, all[0].usd_micro, all[0].unpriced, all[0].tokens.input), (5, 1151, 1, 93));
        let listed = deals.list(&DealsQuery::default()).unwrap();
        assert_eq!(listed.iter().map(|deal| deal.usd_micro.unwrap_or(0)).sum::<u64>(), all[0].usd_micro);
        assert_eq!(listed.iter().map(|deal| deal.tokens.input).sum::<u64>(), all[0].tokens.input);

        let by_seat = deals.stats(&StatsQuery { by: vec![By::Seat, By::Model], ..StatsQuery::default() }).unwrap();
        let lines: Vec<(String, String, u64, u64)> = by_seat
            .iter()
            .map(|sums| (sums.of["seat"].clone(), sums.of["model"].clone(), sums.deals, sums.usd_micro))
            .collect();
        assert_eq!(lines, [("smart".into(), "b".into(), 1, 1000), ("fast".into(), "a".into(), 4, 151)]);

        let first_day =
            StatsQuery { from: Some("2026-11-15".into()), to: Some("2026-11-15".into()), by: vec![By::Day] };
        let day = deals.stats(&first_day).unwrap();
        assert_eq!((day.len(), day[0].of["day"].as_str(), day[0].deals), (1, "2026-11-15", 4));
        assert!(deals
            .stats(&StatsQuery { from: Some("2027-01-01".into()), ..StatsQuery::default() })
            .unwrap()
            .is_empty());
    }

    #[test]
    fn lists_by_time_seat_and_work_newest_first() {
        let deals = Deals::new(Arc::new(Db::memory()));
        let t = Tokens::default();
        deals.write(&deal(NOV_15 + 10, "fast", "a", None, t)).unwrap();
        deals.write(&Deal { work: Some("w1".into()), ..deal(NOV_15 + 20, "workshop", "b", None, t) }).unwrap();
        deals.write(&deal(NOV_15 + 30, "fast", "c", None, t)).unwrap();
        let models = |query: DealsQuery| -> Vec<String> {
            deals.list(&query).unwrap().into_iter().map(|deal| deal.model).collect()
        };
        assert_eq!(models(DealsQuery::default()), ["c", "b", "a"]);
        assert_eq!(models(DealsQuery { seat: Some("fast".into()), ..DealsQuery::default() }), ["c", "a"]);
        assert_eq!(models(DealsQuery { work: Some("w1".into()), ..DealsQuery::default() }), ["b"]);
        let window = DealsQuery { from: Some(NOV_15 + 20), to: Some(NOV_15 + 30), ..DealsQuery::default() };
        assert_eq!(models(window), ["b"]);
        assert_eq!(models(DealsQuery { limit: Some(1), ..DealsQuery::default() }), ["c"]);
    }

    #[test]
    fn a_works_deals_add_up_from_its_start_and_name_who_answered_last() {
        let deals = Deals::new(Arc::new(Db::memory()));
        let t = |input| Tokens { input, output: 2, cache_read: 0, cache_write: 0 };
        let of = |work: &str, deal: Deal| Deal { work: Some(work.into()), ..deal };
        // An older work with the same id, before this one began: not this one's.
        deals.write(&of("7", deal(NOV_15 - DAY, "workshop", "old", Some(9_000), t(900)))).unwrap();
        deals.write(&of("7", deal(NOV_15 + 10, "fast", "a", Some(100), t(10)))).unwrap();
        deals.write(&of("7", deal(NOV_15 + 20, "fast", "b", None, t(5)))).unwrap();
        deals.write(&of("8", deal(NOV_15 + 30, "smart", "c", Some(1), t(1)))).unwrap();
        let seven = deals.of_work("7", NOV_15).unwrap();
        assert_eq!(
            (seven.sums.deals, seven.sums.usd_micro, seven.sums.unpriced, seven.sums.tokens.input, seven.sums.rounds),
            (2, 100, 1, 15, 4)
        );
        let last = seven.last.unwrap();
        assert_eq!((last.model.as_str(), last.seat.as_str(), last.place.as_deref()), ("b", "fast", Some("cloud")));
        let none = deals.of_work("9", NOV_15).unwrap();
        assert_eq!((none.sums.deals, none.sums.usd_micro, none.last), (0, 0, None));
    }

    #[test]
    fn cleans_deals_after_ninety_days_and_days_after_a_year() {
        let deals = Deals::new(Arc::new(Db::memory()));
        let t = Tokens { input: 1, ..Tokens::default() };
        let now = NOV_15 + 400 * DAY;
        for age in [10, 89, 91, 200, 364, 366] {
            deals.write(&deal(now - age * DAY, "fast", &format!("{age}"), Some(age), t)).unwrap();
        }
        assert_eq!(deals.prune(now).unwrap(), (4, 1));
        let left: Vec<String> =
            deals.list(&DealsQuery::default()).unwrap().into_iter().map(|deal| deal.model).collect();
        assert_eq!(left, ["10", "89"]);
        // The days stay a year: the deals of 91, 200 and 364 days ago are still counted in them.
        let all = deals.stats(&StatsQuery::default()).unwrap();
        assert_eq!((all[0].deals, all[0].usd_micro), (5, 10 + 89 + 91 + 200 + 364));
        assert_eq!(deals.prune(now).unwrap(), (0, 0));
        assert_eq!(day_of(NOV_15), "2026-11-15");
    }
}
