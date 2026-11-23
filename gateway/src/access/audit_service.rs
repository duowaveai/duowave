//! Who changed what, from the gateway's events, into `audit_log`; the newest 5000 entries are kept. And
//! `/api/v1/audit`, for whoever may read it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::extract::Query;
use axum::routing::get;
use axum::Router;
use rusqlite::params;
use serde::Serialize;
use serde_json::{Map, Value};

use super::AccessService;
use crate::core::events::{EventBus, GatewayEvent};
use crate::core::identity::permissions::AUDIT_VIEW;
use crate::platform::http::{ok, ApiResult, Caller, Provides, RouteState, Uses};
use crate::settings::definitions::AUTH_SECRET;
use crate::storage::Db;
use crate::utils::time::now_iso;

/// What the audit log keeps: changes to how the house runs and the sign-ins; not the traffic of requests.
pub const AUDITED: &[&str] = &[
    "settings.changed",
    "mode.changed",
    "routes.changed",
    "voice.changed",
    "persona.changed",
    "model.installed",
    "model.removed",
    "model.server.restarted",
    "voice.engine.installed",
    "voice.own.created",
    "voice.own.renamed",
    "voice.own.deleted",
    "voice.job.finished",
    "brain.changed",
    "mcp.changed",
    "packages.changed",
    "connector.changed",
    "memory.changed",
    "sleep.changed",
    "user.created",
    "user.updated",
    "user.deleted",
    "apiKey.created",
    "apiKey.revoked",
    "auth.login",
    "auth.failed",
    "station.pending",
    "station.updated",
    "station.switched",
    "station.firmware",
    "station.modOff",
    // The helpers: one row for each happening (`homeServer.*` still reach the panels live, not the log).
    "helpers.changed",
    "helpers.up",
    "helpers.down",
    // Who this computer helps: a home paired or removed, what it gives, a pairing refused (the live comings and goings
    // of its gateways are no change); the connection key changed.
    "helpers.clients",
    "helpers.key",
    "placement.moved",
    "gateway.state",
    "safety.fired",
    "stationGateway.updated",
];

/// Settings whose changes have a row of their own: the helpers and the regulator (`helpers.changed`), the home and
/// this gateway (`gateway.state`), the pins and «Помогаю» (`helpers.changed`, `helpers.clients`). «Адрес снаружи»
/// (`helper_outside`) has none: its `settings.changed` is its row.
const OWN_ROWS: &[&str] = &["helpers", "away", "home_identity", "gateway_self", "helper_pins", "helper_clients"];

const KEPT_ROWS: i64 = 5000;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AuditActor {
    pub kind: Option<String>,
    pub id: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AuditEntry {
    pub id: i64,
    pub at: String,
    pub action: String,
    pub actor: Option<AuditActor>,
    pub details: Value,
}

pub struct AuditService {
    db: Arc<Db>,
    writes: AtomicU64,
}

impl AuditService {
    pub fn new(db: Arc<Db>) -> AuditService {
        AuditService { db, writes: AtomicU64::new(0) }
    }

    /// Writes every audited event from now on.
    pub fn start(self: &Arc<Self>, events: &EventBus) {
        let audit = self.clone();
        events.on("audit", move |event| audit.write(&event));
    }

    pub fn recent(&self, limit: i64) -> rusqlite::Result<Vec<AuditEntry>> {
        self.db.with(|c| {
            let mut statement = c.prepare(
                "SELECT id, at, action, actor_kind, actor_id, actor_name, details FROM audit_log ORDER BY id DESC LIMIT ?1",
            )?;
            let rows = statement.query_map(params![limit.clamp(1, 1000)], |row| {
                let actor_id: Option<String> = row.get(4)?;
                let details: String = row.get(6)?;
                Ok(AuditEntry {
                    id: row.get(0)?,
                    at: row.get(1)?,
                    action: row.get(2)?,
                    actor: match actor_id {
                        Some(id) => Some(AuditActor { kind: row.get(3)?, id: Some(id), name: row.get(5)? }),
                        None => None,
                    },
                    details: serde_json::from_str(&details).unwrap_or_else(|_| Value::Object(Map::new())),
                })
            })?;
            rows.collect()
        })
    }

    /// One event into the log: its payload without the actor as the details, the actor in its own columns.
    pub fn write(&self, event: &GatewayEvent) {
        let name = event.name();
        if !AUDITED.contains(&name) {
            return;
        }
        // A plugin's own comings and goings (its code loaded, unloaded, failed) are nobody's change.
        if let GatewayEvent::PackagesChanged { actor: None, .. } = event {
            return;
        }
        // What the household said or the model remembered is the memory's own business (its «Новое»), not a
        // person's change of how the house runs.
        if let GatewayEvent::MemoryChanged { actor: None, .. } = event {
            return;
        }
        // A gateway of a home this computer helps coming and going is no change of how the house runs.
        if let GatewayEvent::HelperClients { change, .. } = event {
            if change == "online" || change == "offline" {
                return;
            }
        }
        // The night's own timer is not a person: only «Запустить сон сейчас» and «Откатить ночь» are logged.
        if let GatewayEvent::SleepChanged { actor: None, .. } = event {
            return;
        }
        // What a helper says of itself — its contract read at a look, the first after a start too, its readiness — is
        // the helper's own: the panels and «Мозг» hear it live, the log does not (a helper whose readiness flaps
        // would fill it). Its comings and goings are rows (`helpers.up`, `helpers.down`).
        if let GatewayEvent::HelpersChanged { actor: None, change, .. } = event {
            if change == "contract" {
                return;
            }
        }
        if let GatewayEvent::SettingsChanged { key, .. } = event {
            // The token secret is nobody's business; a connector's setting has its own, fuller row
            // (connector.changed: which fields, the secrets too); so have the helpers' and the gateway's own
            // (helpers.changed, gateway.state).
            if key == AUTH_SECRET.key || key.starts_with("connector_") || OWN_ROWS.contains(&key.as_str()) {
                return;
            }
        }
        let mut details = match event.payload() {
            Value::Object(object) => object,
            _ => Map::new(),
        };
        let actor = details.remove("actor");
        let field = |name: &str| actor.as_ref().and_then(|actor| actor.get(name)).and_then(Value::as_str);
        let written = self.db.with(|c| {
            c.execute(
                "INSERT INTO audit_log (at, action, actor_kind, actor_id, actor_name, details) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![now_iso(), name, field("kind"), field("id"), field("name"), Value::Object(details).to_string()],
            )
        });
        match written {
            Ok(_) => {
                if (self.writes.fetch_add(1, Ordering::Relaxed) + 1).is_multiple_of(100) {
                    if let Err(error) = self.prune() {
                        tracing::warn!("audit log not pruned: {error}");
                    }
                }
            }
            Err(error) => tracing::warn!("{name} not written: {error}"),
        }
    }

    fn prune(&self) -> rusqlite::Result<()> {
        self.db.with(|c| {
            let max: Option<i64> = c.query_row("SELECT max(id) FROM audit_log", [], |row| row.get(0))?;
            if let Some(max) = max.filter(|max| *max > KEPT_ROWS) {
                c.execute("DELETE FROM audit_log WHERE id < ?1", params![max - KEPT_ROWS])?;
            }
            Ok(())
        })
    }
}

/// JavaScript's parseInt: leading spaces, a sign, the digits up to the first that is not one; None without any.
pub(crate) fn parse_int(text: &str) -> Option<i64> {
    let text = text.trim_start();
    let (sign, digits) = match text.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, text.strip_prefix('+').unwrap_or(text)),
    };
    let end = digits.bytes().position(|b| !b.is_ascii_digit()).unwrap_or(digits.len());
    let digits = &digits[..end];
    if digits.is_empty() {
        return None;
    }
    Some(sign * digits.parse::<i64>().unwrap_or(i64::MAX))
}

pub fn routes<S: RouteState + Provides<Arc<AccessService>>>() -> Router<S> {
    Router::new().route("/api/v1/audit", get(recent))
}

/// The newest entries first; `limit` from 1 to 1000, 100 when absent or not a number.
async fn recent(
    Uses(access): Uses<Arc<AccessService>>,
    caller: Caller,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult<Vec<AuditEntry>> {
    caller.permitted(AUDIT_VIEW)?;
    let limit = query.get("limit").and_then(|limit| parse_int(limit)).filter(|limit| *limit != 0).unwrap_or(100);
    Ok(ok(access.audit.recent(limit)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::events::Actor;
    use crate::core::identity::principal::PrincipalKind;

    fn actor() -> Actor {
        Actor { kind: PrincipalKind::User, id: "u1".into(), name: "Мама".into() }
    }

    #[test]
    fn keeps_who_changed_what_but_never_the_signing_secret() {
        let audit = AuditService::new(Arc::new(Db::memory()));
        audit.write(&GatewayEvent::UserCreated {
            user_id: "u2".into(),
            name: "Аня".into(),
            role: "member".into(),
            actor: Some(actor()),
        });
        audit.write(&GatewayEvent::AuthFailed { name: None, source: "10.0.0.5".into() });
        audit.write(&GatewayEvent::SettingsChanged { key: AUTH_SECRET.key.into(), user_id: None, actor: None });
        audit.write(&GatewayEvent::StationConnected { session_id: "s".into(), device_tag: "d".into() });
        let entries = audit.recent(100).unwrap();
        assert_eq!(
            entries.iter().map(|entry| entry.action.as_str()).collect::<Vec<_>>(),
            ["auth.failed", "user.created"]
        );
        assert_eq!(entries[0].actor, None);
        assert_eq!(entries[0].details, serde_json::json!({"source": "10.0.0.5"}));
        assert_eq!(
            entries[1].actor,
            Some(AuditActor { kind: Some("user".into()), id: Some("u1".into()), name: Some("Мама".into()) })
        );
        assert_eq!(entries[1].details, serde_json::json!({"userId": "u2", "name": "Аня", "role": "member"}));
        // «Адрес снаружи» decides what a proxied pairing binds: its change is a row of its own, with who made it.
        let outside = crate::settings::definitions::HELPER_OUTSIDE.key;
        audit.write(&GatewayEvent::SettingsChanged { key: outside.into(), user_id: None, actor: Some(actor()) });
        let entries = audit.recent(1).unwrap();
        assert_eq!(
            (entries[0].action.as_str(), &entries[0].details),
            ("settings.changed", &serde_json::json!({"key": outside}))
        );
        assert_eq!(entries[0].actor.as_ref().and_then(|actor| actor.name.as_deref()), Some("Мама"));
    }

    /// A change of the helpers is one happening, one row: `helpers.changed`, not also the setting it wrote; and the
    /// old `homeServer.*` the shell still emits for older panels are no rows of their own.
    #[tokio::test]
    async fn a_change_of_the_helpers_writes_one_row() {
        use crate::helpers::tests::{contract, stand, station, ALL, KEY};
        let computer = stand(contract("pc", ALL)).await;
        let station = station(&[]);
        let audit = Arc::new(AuditService::new(station.db.clone()));
        let mut heard = station.events.subscribe();
        let input = crate::helpers::AddInput { url: computer.url.clone(), key: KEY.into(), ..Default::default() };
        station.helpers.add(input, Some(actor())).await.unwrap();
        station.helpers.set_away(crate::helpers::Away::Mine, Some(actor())).unwrap();
        audit.write(&GatewayEvent::HomeServerChanged { configured: true, actor: None });
        while let Ok(event) = heard.try_recv() {
            audit.write(&event);
        }
        let rows = audit.recent(100).unwrap();
        let actions: Vec<&str> = rows.iter().rev().map(|entry| entry.action.as_str()).collect();
        assert_eq!(actions, ["helpers.changed", "helpers.changed"], "no settings.changed of «helpers» or «away»");
        assert_eq!(rows[1].details["change"], "added");
        assert_eq!(rows[0].details, serde_json::json!({ "change": "away" }));
        assert_eq!(rows[0].actor.as_ref().and_then(|actor| actor.name.as_deref()), Some("Мама"));
    }

    /// What a helper says of itself is nobody's change (review): its contract read at a look — the first after a start
    /// too — and its readiness that changes reach the panels live, not the log, so a helper whose readiness flaps
    /// never fills it; its coming is a row.
    #[tokio::test]
    async fn a_helpers_own_contract_is_heard_live_and_not_logged() {
        use crate::helpers::tests::{contract, stand, station, ALL};
        let computer = stand(contract("pc", ALL)).await;
        let station = station(&[]);
        station.put("home-a", &computer.url, crate::helpers::HelperKind::Home);
        let audit = AuditService::new(station.db.clone());
        let mut heard = station.events.subscribe();
        station.helpers.check("home-a").await.unwrap();
        let mut loading = ALL.to_vec();
        loading[0] = ("hearing", false);
        *computer.status.lock().unwrap() = contract("pc", &loading);
        station.helpers.check("home-a").await.unwrap();
        let mut contracts = 0;
        while let Ok(event) = heard.try_recv() {
            if matches!(&*event, GatewayEvent::HelpersChanged { change, .. } if change == "contract") {
                contracts += 1;
            }
            audit.write(&event);
        }
        assert_eq!(contracts, 2, "the first read and the readiness that changed, live");
        let actions: Vec<String> = audit.recent(100).unwrap().into_iter().map(|entry| entry.action).collect();
        assert_eq!(actions, ["helpers.up"]);
    }

    #[test]
    fn keeps_the_newest_five_thousand() {
        let db = Arc::new(Db::memory());
        db.with(|c| {
            for _ in 0..5100 {
                c.execute("INSERT INTO audit_log (at, action, details) VALUES ('now', 'auth.login', '{}')", [])?;
            }
            Ok(())
        })
        .unwrap();
        let audit = AuditService::new(db.clone());
        for _ in 0..100 {
            audit.write(&GatewayEvent::AuthFailed { name: None, source: "x".into() });
        }
        let count: i64 = db.with(|c| c.query_row("SELECT count(*) FROM audit_log", [], |row| row.get(0))).unwrap();
        assert_eq!(count, KEPT_ROWS + 1);
        assert_eq!(audit.recent(5000).unwrap().len(), 1000, "a page is 1000 at most");
    }

    #[test]
    fn reads_a_limit_as_parse_int_did() {
        assert_eq!(parse_int("20"), Some(20));
        assert_eq!(parse_int(" 20abc"), Some(20));
        assert_eq!(parse_int("-5"), Some(-5));
        assert_eq!(parse_int("abc"), None);
        assert_eq!(parse_int(""), None);
    }
}
