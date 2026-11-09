//! Who changed what, from the gateway's events, into `audit_log`; the newest 5000 entries are kept. And
//! `/api/v1/audit`, for whoever may read it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::routing::get;
use axum::Router;
use rusqlite::params;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::app::App;
use crate::core::events::{EventBus, GatewayEvent};
use crate::core::identity::permissions::AUDIT_VIEW;
use crate::platform::http::{ok, ApiResult, Caller};
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
    "homeServer.up",
    "homeServer.down",
    "homeServer.changed",
    "placement.moved",
    "gateway.state",
    "safety.fired",
    "stationGateway.updated",
];

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
        if let GatewayEvent::SettingsChanged { key, .. } = event {
            // The token secret is nobody's business; a connector's setting has its own, fuller row
            // (connector.changed: which fields, the secrets too).
            if key == AUTH_SECRET.key || key.starts_with("connector_") {
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

pub fn routes() -> Router<Arc<App>> {
    Router::new().route("/api/v1/audit", get(recent))
}

/// The newest entries first; `limit` from 1 to 1000, 100 when absent or not a number.
async fn recent(
    State(app): State<Arc<App>>,
    caller: Caller,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult<Vec<AuditEntry>> {
    caller.permitted(AUDIT_VIEW)?;
    let limit = query.get("limit").and_then(|limit| parse_int(limit)).filter(|limit| *limit != 0).unwrap_or(100);
    Ok(ok(app.access.audit.recent(limit)?))
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
