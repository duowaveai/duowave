//! The changes made on a copy holder while the home's centre is away (docs/plans/home-and-devices.md, stage 3): the
//! pass of the home's panel on the computer (src/placement/proxy.rs) cannot reach the centre, so a person's edit is
//! kept here (`home_queue`, migration 0016) and the panel hears «сохранено — дойдёт до центра». When the centre answers
//! again, the queue goes to it in order, each change under the person who made it (`send`): in the settings the last
//! change wins, the one before stays in the centre's history. A change the centre refuses (no right, gone) is kept with
//! its reason and not sent again.
use std::time::Duration;

use axum::body::Bytes;
use rusqlite::params;
use serde::Serialize;

use crate::placement::peer::{PeerClient, HELPER_PERSON, HELPER_PERSON_ID, HELPER_ROLE};
use crate::storage::Db;
use crate::utils::time::now_iso;

/// Changes kept at most: past them a new one is refused rather than an old one dropped.
pub const MAX_WAITING: usize = 500;
/// A change's body at most.
pub const MAX_BODY: usize = 256 * 1024;

/// A change to keep.
pub struct Change<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub content_type: Option<&'a str>,
    pub body: &'a Bytes,
    pub person: &'a str,
    pub person_id: Option<&'a str>,
    pub role: &'a str,
}

/// Keeps a change for the centre. Refused when the queue is full or the body too big: the panel says so.
pub fn keep(db: &Db, change: &Change<'_>) -> Result<i64, String> {
    if change.body.len() > MAX_BODY {
        return Err(crate::tr!("Центр не на связи, а правка слишком большая, чтобы ждать его."));
    }
    db.transaction(|transaction| {
        let waiting: i64 =
            transaction.query_row("SELECT COUNT(*) FROM home_queue WHERE refused = 0", [], |row| row.get(0))?;
        if waiting as usize >= MAX_WAITING {
            return Ok(None);
        }
        transaction.execute(
            "INSERT INTO home_queue (created_at, method, path, content_type, body, person, person_id, role)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                now_iso(),
                change.method,
                change.path,
                change.content_type,
                change.body.as_ref(),
                change.person,
                change.person_id,
                change.role
            ],
        )?;
        Ok(Some(transaction.last_insert_rowid()))
    })
    .map_err(|error| error.to_string())?
    .ok_or_else(|| crate::tr!("Центр давно не на связи: правок накопилось слишком много. Подождите его."))
}

/// A waiting change as the panel lists it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Waiting {
    pub id: i64,
    pub created_at: String,
    pub method: String,
    pub path: String,
    pub person: String,
    pub refused: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub fn list(db: &Db) -> Vec<Waiting> {
    db.with(|connection| {
        connection
            .prepare("SELECT id, created_at, method, path, person, refused, error FROM home_queue ORDER BY id")?
            .query_map([], |row| {
                Ok(Waiting {
                    id: row.get(0)?,
                    created_at: row.get(1)?,
                    method: row.get(2)?,
                    path: row.get(3)?,
                    person: row.get(4)?,
                    refused: row.get::<_, i64>(5)? != 0,
                    error: row.get(6)?,
                })
            })?
            .collect()
    })
    .unwrap_or_default()
}

struct Row {
    id: i64,
    method: String,
    path: String,
    content_type: Option<String>,
    body: Vec<u8>,
    person: String,
    person_id: Option<String>,
    role: String,
}

fn waiting(db: &Db) -> Vec<Row> {
    db.with(|connection| {
        connection
            .prepare(
                "SELECT id, method, path, content_type, body, person, person_id, role FROM home_queue
                 WHERE refused = 0 ORDER BY id",
            )?
            .query_map([], |row| {
                Ok(Row {
                    id: row.get(0)?,
                    method: row.get(1)?,
                    path: row.get(2)?,
                    content_type: row.get(3)?,
                    body: row.get::<_, Option<Vec<u8>>>(4)?.unwrap_or_default(),
                    person: row.get(5)?,
                    person_id: row.get(6)?,
                    role: row.get(7)?,
                })
            })?
            .collect()
    })
    .unwrap_or_default()
}

/// Sends the waiting changes to the centre, in order. A change it took goes from the queue; one it refused stays
/// with the reason; the centre away again — the rest wait for the next look.
pub async fn send(db: &Db, client: &PeerClient) -> Result<(), String> {
    let rows = waiting(db);
    if rows.is_empty() {
        return Ok(());
    }
    let mut sent = 0;
    for row in rows {
        let method = reqwest::Method::from_bytes(row.method.as_bytes()).map_err(|error| error.to_string())?;
        let mut request = crate::utils::http::client()
            .request(method, format!("{}{}", client.base(), row.path))
            .bearer_auth(client.key())
            .header(HELPER_PERSON, row.person.clone())
            .header(HELPER_ROLE, row.role.clone())
            .timeout(Duration::from_secs(30))
            .body(row.body);
        if let Some(id) = &row.person_id {
            request = request.header(HELPER_PERSON_ID, id.clone());
        }
        if let Some(kind) = &row.content_type {
            request = request.header(reqwest::header::CONTENT_TYPE, kind.clone());
        }
        match request.send().await {
            Ok(response) if response.status().is_success() => {
                let _ = db.with(|c| c.execute("DELETE FROM home_queue WHERE id = ?1", params![row.id]));
                sent += 1;
            }
            Ok(response) if response.status().is_client_error() => {
                let status = response.status();
                let text: String = response.text().await.unwrap_or_default().chars().take(300).collect();
                tracing::warn!(target: "home", "a waiting change to {} refused by the centre: {status}", row.path);
                let _ = db.with(|c| {
                    c.execute(
                        "UPDATE home_queue SET refused = 1, tries = tries + 1, error = ?2 WHERE id = ?1",
                        params![row.id, format!("{status}: {text}")],
                    )
                });
            }
            Ok(response) => {
                let error = format!("HTTP {}", response.status());
                let _ = db.with(|c| {
                    c.execute(
                        "UPDATE home_queue SET tries = tries + 1, error = ?2 WHERE id = ?1",
                        params![row.id, error],
                    )
                });
                return Err(error);
            }
            Err(error) => return Err(error.to_string()),
        }
    }
    if sent > 0 {
        tracing::info!(target: "home", "{sent} change(s) made while the centre was away went to it");
    }
    Ok(())
}

/// Forgets a refused change (the panel's «Убрать»).
pub fn forget(db: &Db, id: i64) -> bool {
    db.with(|c| c.execute("DELETE FROM home_queue WHERE id = ?1 AND refused = 1", params![id])).unwrap_or(0) > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_waits_in_order_and_a_refused_one_is_kept_with_its_reason() {
        let db = Db::memory();
        let body = Bytes::from_static(b"{\"days\":9}");
        let change = |path: &'static str| Change {
            method: "PUT",
            path,
            content_type: Some("application/json"),
            body: &body,
            person: "Мама",
            person_id: Some("u1"),
            role: "admin",
        };
        let first = keep(&db, &change("/api/v1/requests/keep")).unwrap();
        let second = keep(&db, &change("/api/v1/brain")).unwrap();
        assert!(first < second);
        let listed = list(&db);
        assert_eq!(
            listed.iter().map(|waiting| waiting.path.as_str()).collect::<Vec<_>>(),
            ["/api/v1/requests/keep", "/api/v1/brain"]
        );
        assert!(!forget(&db, first), "only a refused one is forgotten by hand");
        db.with(|c| c.execute("UPDATE home_queue SET refused = 1, error = 'HTTP 403' WHERE id = ?1", params![first]))
            .unwrap();
        assert_eq!(waiting(&db).len(), 1, "a refused one is not sent again");
        assert!(forget(&db, first));
        let big = Bytes::from(vec![b'x'; MAX_BODY + 1]);
        let too_big = Change { body: &big, ..change("/api/v1/brain") };
        assert!(keep(&db, &too_big).is_err());
    }
}
