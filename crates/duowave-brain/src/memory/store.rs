//! The memory's tables (migration 0006): entities and their other names, facts with dates, episodes and the
//! memory's own log of changes. Plain synchronous SQL on the gateway's one connection, like the other stores; every
//! read of records takes the asker (`Viewer`) and keeps to what they may see.

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;

use super::model::{EntityKind, FactKind, Origin, Source, Viewer, Visibility};
use super::text;
use crate::utils::{ids::uuid, time::now_iso};

/// A node of the household graph.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entity {
    pub id: String,
    pub kind: EntityKind,
    pub name: String,
    pub aliases: Vec<String>,
    pub account_id: Option<String>,
    pub contact: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// A fact as stored, with its labels.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Fact {
    pub id: String,
    pub subject_id: String,
    pub relation: Option<String>,
    pub object_id: Option<String>,
    pub value: Option<String>,
    pub text: String,
    pub lang: String,
    pub kind: FactKind,
    pub viewpoint: Option<String>,
    pub valid_from: Option<String>,
    pub valid_to: Option<String>,
    pub closed_reason: Option<String>,
    pub replaced_by: Option<String>,
    pub confidence: f64,
    pub importance: i64,
    pub pinned: bool,
    pub mentions: i64,
    pub source: Source,
    pub episode_id: Option<String>,
    pub origin: Origin,
    pub visibility: Visibility,
    pub visibility_owner: Option<String>,
    pub owner_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// What a new fact says; the store adds the id, the language and the times.
#[derive(Debug, Clone)]
pub struct NewFact {
    pub subject_id: String,
    pub relation: Option<String>,
    pub object_id: Option<String>,
    pub value: Option<String>,
    pub text: String,
    pub kind: FactKind,
    pub viewpoint: Option<String>,
    pub valid_from: Option<String>,
    pub confidence: f64,
    pub importance: i64,
    pub pinned: bool,
    pub source: Source,
    pub episode_id: Option<String>,
    pub origin: Origin,
    pub visibility: Visibility,
    pub visibility_owner: Option<String>,
    pub owner_id: Option<String>,
}

/// One question and its answer, word for word, for 7 days.
#[derive(Debug, Clone)]
pub struct NewEpisode {
    pub station: Option<String>,
    pub character_id: Option<String>,
    pub question: String,
    pub answer: String,
    pub origin: Origin,
}

/// A line of the memory's own log.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub id: String,
    pub at: String,
    pub actor_kind: String,
    pub actor_id: Option<String>,
    pub action: String,
    pub target: String,
    pub target_id: String,
    pub summary: String,
    #[serde(skip)]
    pub before: Option<String>,
    #[serde(skip)]
    pub after: Option<String>,
    pub reason: Option<String>,
    /// The night of the sleep that made it.
    pub run_id: Option<String>,
    pub undone_at: Option<String>,
}

const FACT_COLUMNS: &str =
    "f.id, f.subject_id, f.relation, f.object_id, f.value, f.text, f.lang, f.kind, f.viewpoint, \
    f.valid_from, f.valid_to, f.closed_reason, f.replaced_by, f.confidence, f.importance, f.pinned, f.mentions, \
    f.source, f.episode_id, f.said_by_kind, f.said_by_id, f.channel, f.station, f.audience, f.heard_by, f.visibility, \
    f.visibility_owner, f.owner_id, f.created_at, f.updated_at";

/// What a viewer may see of `f` (a fact or an episode aliased as `f`): the household's records, their own, those
/// shared with them, their character's and their world's. Unknown parameters are NULL and match nothing private.
const VISIBLE: &str = "(f.visibility = 'household' \
    OR (f.visibility = 'person' AND f.visibility_owner = :person) \
    OR (f.visibility = 'people' AND EXISTS (SELECT 1 FROM memory_visible_to v WHERE v.record_id = f.id AND v.person_id = :person)) \
    OR (f.visibility = 'character' AND f.visibility_owner = :character) \
    OR (f.visibility = 'world' AND f.visibility_owner = :world))";

fn fact_from(row: &Row) -> rusqlite::Result<Fact> {
    let kind: String = row.get(7)?;
    let source: String = row.get(17)?;
    let said_by_kind: String = row.get(19)?;
    let channel: String = row.get(21)?;
    let audience: String = row.get(23)?;
    let visibility: String = row.get(25)?;
    Ok(Fact {
        id: row.get(0)?,
        subject_id: row.get(1)?,
        relation: row.get(2)?,
        object_id: row.get(3)?,
        value: row.get(4)?,
        text: row.get(5)?,
        lang: row.get(6)?,
        kind: FactKind::parse(&kind).unwrap_or(FactKind::Fact),
        viewpoint: row.get(8)?,
        valid_from: row.get(9)?,
        valid_to: row.get(10)?,
        closed_reason: row.get(11)?,
        replaced_by: row.get(12)?,
        confidence: row.get(13)?,
        importance: row.get(14)?,
        pinned: row.get::<_, i64>(15)? != 0,
        mentions: row.get(16)?,
        source: Source::parse(&source).unwrap_or(Source::Panel),
        episode_id: row.get(18)?,
        origin: Origin {
            said_by_kind: super::model::SpeakerKind::parse(&said_by_kind).unwrap_or(super::model::SpeakerKind::System),
            said_by_id: row.get(20)?,
            channel: super::model::Channel::parse(&channel).unwrap_or(super::model::Channel::System),
            station: row.get(22)?,
            audience: super::model::Audience::parse(&audience).unwrap_or(super::model::Audience::Room),
            heard_by: row.get(24)?,
        },
        visibility: Visibility::parse(&visibility).unwrap_or(Visibility::Household),
        visibility_owner: row.get(26)?,
        owner_id: row.get(27)?,
        created_at: row.get(28)?,
        updated_at: row.get(29)?,
    })
}

fn viewer_params(viewer: &Viewer) -> [(&'static str, Option<String>); 3] {
    [(":person", viewer.person.clone()), (":character", viewer.character.clone()), (":world", viewer.world.clone())]
}

// ───────── Entities ─────────

/// The household itself: one entity, created on first use, named in the household's language.
pub fn household(conn: &Connection, name: &str) -> rusqlite::Result<String> {
    let found: Option<String> = conn
        .query_row("SELECT id FROM memory_entities WHERE kind = 'household' ORDER BY created_at LIMIT 1", [], |r| {
            r.get(0)
        })
        .optional()?;
    match found {
        Some(id) => Ok(id),
        None => Ok(insert_entity(conn, EntityKind::Household, name)?.id),
    }
}

pub fn insert_entity(conn: &Connection, kind: EntityKind, name: &str) -> rusqlite::Result<Entity> {
    let id = uuid();
    let now = now_iso();
    conn.execute(
        "INSERT INTO memory_entities (id, kind, name, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)",
        params![id, kind.as_str(), name.trim(), now],
    )?;
    Ok(Entity {
        id,
        kind,
        name: name.trim().to_string(),
        aliases: Vec::new(),
        account_id: None,
        contact: None,
        created_at: now.clone(),
        updated_at: now,
    })
}

/// Every entity with its other names: a household has tens of them, so matching is done in Rust.
pub fn entities(conn: &Connection) -> rusqlite::Result<Vec<Entity>> {
    let mut statement = conn.prepare(
        "SELECT id, kind, name, account_id, contact, created_at, updated_at FROM memory_entities ORDER BY name COLLATE NOCASE",
    )?;
    let mut list: Vec<Entity> = statement
        .query_map([], |row| {
            let kind: String = row.get(1)?;
            Ok(Entity {
                id: row.get(0)?,
                kind: EntityKind::parse(&kind).unwrap_or(EntityKind::Thing),
                name: row.get(2)?,
                aliases: Vec::new(),
                account_id: row.get(3)?,
                contact: row.get(4)?,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut statement = conn.prepare("SELECT entity_id, alias FROM memory_aliases ORDER BY alias COLLATE NOCASE")?;
    let aliases: Vec<(String, String)> =
        statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
    for (entity_id, alias) in aliases {
        if let Some(entity) = list.iter_mut().find(|e| e.id == entity_id) {
            entity.aliases.push(alias);
        }
    }
    Ok(list)
}

pub fn entity(conn: &Connection, id: &str) -> rusqlite::Result<Option<Entity>> {
    Ok(entities(conn)?.into_iter().find(|e| e.id == id))
}

/// The entity a name means, in any case form, by its name or one of its other names. Several matches: the person
/// first, then the shortest name.
pub fn find_entity(conn: &Connection, name: &str) -> rusqlite::Result<Option<Entity>> {
    let wanted = text::stems(name);
    if wanted.is_empty() {
        return Ok(None);
    }
    let mut found: Vec<Entity> = entities(conn)?
        .into_iter()
        .filter(|e| e.kind != EntityKind::Household)
        .filter(|e| std::iter::once(&e.name).chain(e.aliases.iter()).any(|n| text::stems(n) == wanted))
        .collect();
    found.sort_by_key(|e| (e.kind != EntityKind::Person, e.name.chars().count()));
    Ok(found.into_iter().next())
}

/// The entities a text mentions, in any case form: «у Маши аллергия» mentions Маша.
pub fn mentioned_entities(conn: &Connection, said: &str) -> rusqlite::Result<Vec<Entity>> {
    Ok(entities(conn)?
        .into_iter()
        .filter(|e| e.kind != EntityKind::Household)
        .filter(|e| std::iter::once(&e.name).chain(e.aliases.iter()).any(|n| text::mentions(said, n)))
        .collect())
}

pub fn rename_entity(conn: &Connection, id: &str, name: &str) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE memory_entities SET name = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, name.trim(), now_iso()],
    )
}

pub fn add_alias(conn: &Connection, entity_id: &str, alias: &str) -> rusqlite::Result<usize> {
    let norm = text::stems(alias).join(" ");
    if norm.is_empty() {
        return Ok(0);
    }
    conn.execute(
        "INSERT OR IGNORE INTO memory_aliases (entity_id, alias, norm) VALUES (?1, ?2, ?3)",
        params![entity_id, alias.trim(), norm],
    )
}

pub fn remove_alias(conn: &Connection, entity_id: &str, alias: &str) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM memory_aliases WHERE entity_id = ?1 AND alias = ?2", params![entity_id, alias])
}

/// Two entries for one person: the facts, links and other names move to `into`, the old name becomes another name.
pub fn merge_entities(conn: &Connection, from: &str, into: &str) -> rusqlite::Result<()> {
    let old: Option<String> =
        conn.query_row("SELECT name FROM memory_entities WHERE id = ?1", [from], |r| r.get(0)).optional()?;
    conn.execute("UPDATE memory_facts SET subject_id = ?2 WHERE subject_id = ?1", params![from, into])?;
    conn.execute("UPDATE memory_facts SET object_id = ?2 WHERE object_id = ?1", params![from, into])?;
    conn.execute(
        "INSERT OR IGNORE INTO memory_aliases (entity_id, alias, norm) SELECT ?2, alias, norm FROM memory_aliases WHERE entity_id = ?1",
        params![from, into],
    )?;
    if let Some(old) = old {
        add_alias(conn, into, &old)?;
    }
    conn.execute("DELETE FROM memory_entities WHERE id = ?1", [from])?;
    conn.execute("UPDATE memory_entities SET updated_at = ?2 WHERE id = ?1", params![into, now_iso()])?;
    Ok(())
}

/// Erases an entity with every fact about it.
pub fn delete_entity(conn: &Connection, id: &str) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM memory_entities WHERE id = ?1 AND kind != 'household'", [id])
}

// ───────── Facts ─────────

pub fn insert_fact(conn: &Connection, new: &NewFact) -> rusqlite::Result<Fact> {
    let id = uuid();
    let now = now_iso();
    let lang = text::lang_of(&new.text).as_str();
    conn.execute(
        "INSERT INTO memory_facts (id, subject_id, relation, object_id, value, text, lang, kind, viewpoint, valid_from, \
         confidence, importance, pinned, source, episode_id, said_by_kind, said_by_id, channel, station, audience, \
         heard_by, visibility, visibility_owner, owner_id, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?25)",
        params![
            id,
            new.subject_id,
            new.relation,
            new.object_id,
            new.value,
            new.text.trim(),
            lang,
            new.kind.as_str(),
            new.viewpoint,
            new.valid_from,
            new.confidence,
            new.importance,
            new.pinned as i64,
            new.source.as_str(),
            new.episode_id,
            new.origin.said_by_kind.as_str(),
            new.origin.said_by_id,
            new.origin.channel.as_str(),
            new.origin.station,
            new.origin.audience.as_str(),
            new.origin.heard_by,
            new.visibility.as_str(),
            new.visibility_owner,
            new.owner_id,
            now,
        ],
    )?;
    fact(conn, &id).map(|f| f.expect("the fact was just written"))
}

/// A fact by id, whatever its state and labels: for the panel's actions, which check the right separately.
pub fn fact(conn: &Connection, id: &str) -> rusqlite::Result<Option<Fact>> {
    conn.query_row(&format!("SELECT {FACT_COLUMNS} FROM memory_facts f WHERE f.id = ?1"), [id], fact_from).optional()
}

/// The facts a viewer may see: current ones only, or closed ones too.
pub fn visible_facts(conn: &Connection, viewer: &Viewer, current_only: bool) -> rusqlite::Result<Vec<Fact>> {
    let current = if current_only { "AND f.valid_to IS NULL" } else { "" };
    let sql = format!(
        "SELECT {FACT_COLUMNS} FROM memory_facts f WHERE {VISIBLE} {current} AND f.rejected = 0 ORDER BY f.created_at DESC"
    );
    let mut statement = conn.prepare(&sql)?;
    let params = viewer_params(viewer);
    let rows = statement.query_map(
        &[
            (params[0].0, &params[0].1 as &dyn rusqlite::ToSql),
            (params[1].0, &params[1].1),
            (params[2].0, &params[2].1),
        ][..],
        fact_from,
    )?;
    rows.collect()
}

/// The facts about an entity a viewer may see (as subject or as the other side), current and closed.
pub fn facts_of(conn: &Connection, viewer: &Viewer, entity_id: &str) -> rusqlite::Result<Vec<Fact>> {
    let sql = format!(
        "SELECT {FACT_COLUMNS} FROM memory_facts f WHERE (f.subject_id = :entity OR f.object_id = :entity) AND {VISIBLE} \
         AND f.rejected = 0 ORDER BY f.pinned DESC, f.importance DESC, f.created_at DESC"
    );
    let mut statement = conn.prepare(&sql)?;
    let params = viewer_params(viewer);
    let entity = Some(entity_id.to_string());
    let rows = statement.query_map(
        &[
            (":entity", &entity as &dyn rusqlite::ToSql),
            (params[0].0, &params[0].1),
            (params[1].0, &params[1].1),
            (params[2].0, &params[2].1),
        ][..],
        fact_from,
    )?;
    rows.collect()
}

/// Closes a fact: it stays in the history with the date and the reason, and no longer goes into the prompt.
pub fn close_fact(conn: &Connection, id: &str, reason: &str, replaced_by: Option<&str>) -> rusqlite::Result<usize> {
    let now = now_iso();
    conn.execute(
        "UPDATE memory_facts SET valid_to = ?2, closed_reason = ?3, replaced_by = ?4, updated_at = ?2 WHERE id = ?1 AND valid_to IS NULL",
        params![id, now, reason, replaced_by],
    )
}

pub fn reopen_fact(conn: &Connection, id: &str) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE memory_facts SET valid_to = NULL, closed_reason = NULL, replaced_by = NULL, updated_at = ?2 WHERE id = ?1",
        params![id, now_iso()],
    )
}

pub fn edit_fact(conn: &Connection, id: &str, text_: &str) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE memory_facts SET text = ?2, lang = ?3, updated_at = ?4 WHERE id = ?1",
        params![id, text_.trim(), text::lang_of(text_).as_str(), now_iso()],
    )
}

pub fn pin_fact(conn: &Connection, id: &str, pinned: bool) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE memory_facts SET pinned = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, pinned as i64, now_iso()],
    )
}

/// Erases a fact for good, and its words from the memory's log: «Стереть» must not leave the text behind.
pub fn erase_fact(conn: &Connection, id: &str) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM memory_visible_to WHERE record_id = ?1", [id])?;
    scrub_changes(conn, id)?;
    conn.execute("DELETE FROM memory_facts WHERE id = ?1", [id])
}

/// A fact came up again (found by recall): counts towards its importance later.
pub fn mention(conn: &Connection, ids: &[String]) -> rusqlite::Result<()> {
    for id in ids {
        conn.execute("UPDATE memory_facts SET mentions = mentions + 1 WHERE id = ?1", [id])?;
    }
    Ok(())
}

/// The wake-up went into a prompt: when each record was last used, for reflection later.
pub fn prompted(conn: &Connection, ids: &[String]) -> rusqlite::Result<()> {
    let now = now_iso();
    for id in ids {
        conn.execute("UPDATE memory_facts SET last_prompted_at = ?2 WHERE id = ?1", params![id, now])?;
    }
    Ok(())
}

// ───────── Episodes ─────────

pub fn insert_episode(conn: &Connection, new: &NewEpisode) -> rusqlite::Result<String> {
    let id = uuid();
    conn.execute(
        "INSERT INTO memory_episodes (id, at, station, character_id, question, answer, said_by_kind, said_by_id, channel, audience) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            id,
            now_iso(),
            new.station,
            new.character_id,
            new.question,
            new.answer,
            new.origin.said_by_kind.as_str(),
            new.origin.said_by_id,
            new.origin.channel.as_str(),
            new.origin.audience.as_str(),
        ],
    )?;
    Ok(id)
}

/// Drops the episodes older than `before` (an RFC 3339 time): word for word only for the last days.
pub fn prune_episodes(conn: &Connection, before: &str) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM memory_episodes WHERE at < ?1", [before])
}

// ───────── The log ─────────

pub struct NewChange<'a> {
    pub actor_kind: &'a str,
    pub actor_id: Option<&'a str>,
    pub action: &'a str,
    pub target: &'a str,
    pub target_id: &'a str,
    pub summary: &'a str,
    pub before: Option<String>,
    pub after: Option<String>,
    pub reason: Option<&'a str>,
    /// The night of the sleep that made it; None for the day's own changes.
    pub run_id: Option<&'a str>,
}

pub fn log_change(conn: &Connection, change: NewChange) -> rusqlite::Result<String> {
    let id = uuid();
    conn.execute(
        "INSERT INTO memory_changes (id, at, actor_kind, actor_id, action, target, target_id, summary, before, after, reason, run_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            id,
            now_iso(),
            change.actor_kind,
            change.actor_id,
            change.action,
            change.target,
            change.target_id,
            change.summary,
            change.before,
            change.after,
            change.reason,
            change.run_id,
        ],
    )?;
    Ok(id)
}

fn change_from(row: &Row) -> rusqlite::Result<Change> {
    Ok(Change {
        id: row.get(0)?,
        at: row.get(1)?,
        actor_kind: row.get(2)?,
        actor_id: row.get(3)?,
        action: row.get(4)?,
        target: row.get(5)?,
        target_id: row.get(6)?,
        summary: row.get(7)?,
        before: row.get(8)?,
        after: row.get(9)?,
        reason: row.get(10)?,
        undone_at: row.get(11)?,
        run_id: row.get(12)?,
    })
}

const CHANGE_COLUMNS: &str =
    "id, at, actor_kind, actor_id, action, target, target_id, summary, before, after, reason, undone_at, run_id";

pub fn recent_changes(conn: &Connection, limit: i64) -> rusqlite::Result<Vec<Change>> {
    let mut statement =
        conn.prepare(&format!("SELECT {CHANGE_COLUMNS} FROM memory_changes ORDER BY at DESC, rowid DESC LIMIT ?1"))?;
    let rows = statement.query_map([limit.clamp(1, 500)], change_from)?;
    rows.collect()
}

pub fn change(conn: &Connection, id: &str) -> rusqlite::Result<Option<Change>> {
    conn.query_row(&format!("SELECT {CHANGE_COLUMNS} FROM memory_changes WHERE id = ?1"), [id], change_from).optional()
}

pub fn mark_undone(conn: &Connection, id: &str) -> rusqlite::Result<usize> {
    conn.execute("UPDATE memory_changes SET undone_at = ?2 WHERE id = ?1 AND undone_at IS NULL", params![id, now_iso()])
}

/// Removes a record's words from the log once it is erased; the line itself stays, without the text.
pub fn scrub_changes(conn: &Connection, target_id: &str) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE memory_changes SET summary = '', before = NULL, after = NULL WHERE target_id = ?1",
        [target_id],
    )
}

/// How many changes a night made, undone ones too.
pub fn count_changes_of_run(conn: &Connection, run_id: &str) -> rusqlite::Result<i64> {
    conn.query_row("SELECT COUNT(*) FROM memory_changes WHERE run_id = ?1", [run_id], |r| r.get(0))
}

/// A night's changes, in the order they were made.
pub fn changes_of_run(conn: &Connection, run_id: &str) -> rusqlite::Result<Vec<Change>> {
    let mut statement =
        conn.prepare(&format!("SELECT {CHANGE_COLUMNS} FROM memory_changes WHERE run_id = ?1 ORDER BY rowid"))?;
    let rows = statement.query_map([run_id], change_from)?;
    rows.collect()
}

pub fn set_importance(conn: &Connection, id: &str, importance: i64) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE memory_facts SET importance = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, importance, now_iso()],
    )
}

// ───────── The reflection: conversations to think over, notes ─────────

/// A conversation's exchange as the reflection reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct Episode {
    pub id: String,
    pub at: String,
    pub station: Option<String>,
    pub character_id: Option<String>,
    pub question: String,
    pub answer: String,
}

/// The exchanges no night has thought over yet, oldest first.
pub fn unreflected_episodes(conn: &Connection, limit: i64) -> rusqlite::Result<Vec<Episode>> {
    let mut statement = conn.prepare(
        "SELECT id, at, station, character_id, question, answer FROM memory_episodes \
         WHERE reflected_run IS NULL ORDER BY at, rowid LIMIT ?1",
    )?;
    let rows = statement.query_map([limit.max(1)], |row| {
        Ok(Episode {
            id: row.get(0)?,
            at: row.get(1)?,
            station: row.get(2)?,
            character_id: row.get(3)?,
            question: row.get(4)?,
            answer: row.get(5)?,
        })
    })?;
    rows.collect()
}

pub fn unreflected_count(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("SELECT COUNT(*) FROM memory_episodes WHERE reflected_run IS NULL", [], |r| r.get(0))
}

pub fn mark_reflected(conn: &Connection, ids: &[String], run_id: &str) -> rusqlite::Result<()> {
    let mut statement = conn.prepare("UPDATE memory_episodes SET reflected_run = ?2 WHERE id = ?1")?;
    for id in ids {
        statement.execute(params![id, run_id])?;
    }
    Ok(())
}

/// How many conversations a night thought over.
pub fn reflected_count(conn: &Connection, run_id: &str) -> rusqlite::Result<i64> {
    conn.query_row("SELECT COUNT(*) FROM memory_episodes WHERE reflected_run = ?1", [run_id], |r| r.get(0))
}

/// A night rolled back: what it thought over waits for the next one.
pub fn unmark_reflected(conn: &Connection, run_id: &str) -> rusqlite::Result<usize> {
    conn.execute("UPDATE memory_episodes SET reflected_run = NULL WHERE reflected_run = ?1", [run_id])
}

/// The household's summary of a day, a character's diary entry.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub id: String,
    /// day | diary | week | month
    pub kind: String,
    pub day: String,
    pub character_id: Option<String>,
    pub text: String,
    pub lang: String,
    pub source: String,
    pub run_id: Option<String>,
    pub visibility: String,
    pub created_at: String,
    pub updated_at: String,
}

const NOTE_COLUMNS: &str =
    "id, kind, day, character_id, text, lang, source, run_id, visibility, created_at, updated_at";

fn note_from(row: &Row) -> rusqlite::Result<Note> {
    Ok(Note {
        id: row.get(0)?,
        kind: row.get(1)?,
        day: row.get(2)?,
        character_id: row.get(3)?,
        text: row.get(4)?,
        lang: row.get(5)?,
        source: row.get(6)?,
        run_id: row.get(7)?,
        visibility: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

pub fn note(conn: &Connection, id: &str) -> rusqlite::Result<Option<Note>> {
    conn.query_row(&format!("SELECT {NOTE_COLUMNS} FROM memory_notes WHERE id = ?1"), [id], note_from).optional()
}

/// The note of a kind for a day (and a character's, for a diary), if there is one.
pub fn note_of(conn: &Connection, kind: &str, day: &str, character: Option<&str>) -> rusqlite::Result<Option<Note>> {
    conn.query_row(
        &format!(
            "SELECT {NOTE_COLUMNS} FROM memory_notes WHERE kind = ?1 AND day = ?2 AND character_id IS ?3 \
             ORDER BY created_at DESC LIMIT 1"
        ),
        params![kind, day, character],
        note_from,
    )
    .optional()
}

/// The newest note of a kind from `since` (a day, YYYY-MM-DD) on.
pub fn latest_note(
    conn: &Connection,
    kind: &str,
    character: Option<&str>,
    since: &str,
) -> rusqlite::Result<Option<Note>> {
    conn.query_row(
        &format!(
            "SELECT {NOTE_COLUMNS} FROM memory_notes WHERE kind = ?1 AND character_id IS ?2 AND day >= ?3 \
             ORDER BY day DESC, updated_at DESC LIMIT 1"
        ),
        params![kind, character, since],
        note_from,
    )
    .optional()
}

/// A night's notes, by day: the household's summary first, then the diaries.
pub fn notes_of_run(conn: &Connection, run_id: &str) -> rusqlite::Result<Vec<Note>> {
    let mut statement = conn.prepare(&format!(
        "SELECT {NOTE_COLUMNS} FROM memory_notes WHERE run_id = ?1 ORDER BY day, character_id IS NOT NULL, character_id"
    ))?;
    let rows = statement.query_map([run_id], note_from)?;
    rows.collect()
}

/// A new note; a diary is its character's alone.
pub fn insert_note(
    conn: &Connection,
    kind: &str,
    day: &str,
    character: Option<&str>,
    text_: &str,
    source: &str,
    run_id: Option<&str>,
) -> rusqlite::Result<Note> {
    let id = uuid();
    let now = now_iso();
    let (visibility, owner) = match (kind, character) {
        ("diary", Some(character)) => ("character", Some(character)),
        _ => ("household", None),
    };
    conn.execute(
        "INSERT INTO memory_notes (id, kind, day, character_id, text, lang, source, run_id, visibility, visibility_owner, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)",
        params![id, kind, day, character, text_.trim(), text::lang_of(text_).as_str(), source, run_id, visibility, owner, now],
    )?;
    Ok(note(conn, &id)?.expect("the note was just written"))
}

/// A note's new words from a night: that night is its writer now.
pub fn update_note(conn: &Connection, id: &str, text_: &str, run_id: Option<&str>) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE memory_notes SET text = ?2, lang = ?3, run_id = ?4, updated_at = ?5 WHERE id = ?1",
        params![id, text_.trim(), text::lang_of(text_).as_str(), run_id, now_iso()],
    )
}

pub fn delete_note(conn: &Connection, id: &str) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM memory_notes WHERE id = ?1", [id])
}

// ───────── Sizes and counts ─────────

/// How many records of what, for the overview.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Counts {
    /// Current household facts.
    pub home: i64,
    /// Characters' personal records (opinions, impressions; the diary comes in stage 2).
    pub personal: i64,
    pub entities: i64,
    pub episodes: i64,
    /// Closed facts kept as history.
    pub closed: i64,
}

pub fn counts(conn: &Connection, viewer: &Viewer) -> rusqlite::Result<Counts> {
    let count = |sql: &str| -> rusqlite::Result<i64> {
        let params = viewer_params(viewer);
        conn.query_row(
            sql,
            &[
                (params[0].0, &params[0].1 as &dyn rusqlite::ToSql),
                (params[1].0, &params[1].1),
                (params[2].0, &params[2].1),
            ][..],
            |r| r.get(0),
        )
    };
    Ok(Counts {
        home: count(&format!(
            "SELECT COUNT(*) FROM memory_facts f WHERE {VISIBLE} AND f.visibility != 'character' AND f.valid_to IS NULL AND f.rejected = 0"
        ))?,
        personal: count(&format!(
            "SELECT COUNT(*) FROM memory_facts f WHERE {VISIBLE} AND f.visibility = 'character' AND f.valid_to IS NULL"
        ))?,
        entities: conn.query_row("SELECT COUNT(*) FROM memory_entities WHERE kind != 'household'", [], |r| r.get(0))?,
        episodes: conn.query_row("SELECT COUNT(*) FROM memory_episodes", [], |r| r.get(0))?,
        closed: count(&format!("SELECT COUNT(*) FROM memory_facts f WHERE {VISIBLE} AND f.valid_to IS NOT NULL"))?,
    })
}

/// About how many bytes the memory takes: its texts with a margin for the rows and indexes.
pub fn approx_bytes(conn: &Connection) -> rusqlite::Result<i64> {
    let texts: i64 = conn.query_row(
        "SELECT \
           (SELECT COALESCE(SUM(LENGTH(CAST(text AS BLOB)) + 300), 0) FROM memory_facts) + \
           (SELECT COALESCE(SUM(LENGTH(CAST(question AS BLOB)) + LENGTH(CAST(answer AS BLOB)) + 200), 0) FROM memory_episodes) + \
           (SELECT COALESCE(SUM(LENGTH(CAST(summary AS BLOB)) + COALESCE(LENGTH(before), 0) + COALESCE(LENGTH(after), 0) + 150), 0) FROM memory_changes) + \
           (SELECT COALESCE(SUM(LENGTH(CAST(name AS BLOB)) + 200), 0) FROM memory_entities) + \
           (SELECT COALESCE(SUM(LENGTH(CAST(text AS BLOB)) + 200), 0) FROM memory_notes)",
        [],
        |r| r.get(0),
    )?;
    Ok(texts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::model::{Channel, SpeakerKind};
    use crate::storage::Db;

    fn new_fact(subject: &str, said: &str) -> NewFact {
        NewFact {
            subject_id: subject.into(),
            relation: None,
            object_id: None,
            value: None,
            text: said.into(),
            kind: FactKind::Fact,
            viewpoint: None,
            valid_from: None,
            confidence: 1.0,
            importance: 0,
            pinned: false,
            source: Source::Voice,
            episode_id: None,
            origin: Origin::station_voice(Some("kitchen"), Some("alice")),
            visibility: Visibility::Household,
            visibility_owner: None,
            owner_id: None,
        }
    }

    #[test]
    fn keeps_who_said_it_and_finds_names_in_any_form() {
        let db = Db::memory();
        db.with(|c| {
            let home = household(c, "Дом")?;
            assert_eq!(household(c, "Дом")?, home, "one household");
            let masha = insert_entity(c, EntityKind::Person, "Маша")?;
            add_alias(c, &masha.id, "Мария")?;
            let fact = insert_fact(c, &new_fact(&masha.id, "У Маши аллергия на орехи"))?;
            assert_eq!(fact.lang, "ru");
            assert_eq!(fact.origin.said_by_kind, SpeakerKind::Household);
            assert_eq!(fact.origin.channel, Channel::Station);
            assert_eq!(fact.origin.station.as_deref(), Some("kitchen"));
            assert_eq!(find_entity(c, "машу")?.map(|e| e.id), Some(masha.id.clone()));
            assert_eq!(find_entity(c, "Марии")?.map(|e| e.id), Some(masha.id.clone()));
            assert_eq!(mentioned_entities(c, "что можно маше на ужин")?.len(), 1);
            assert!(find_entity(c, "Петя")?.is_none());
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn private_records_stay_with_their_people() {
        let db = Db::memory();
        db.with(|c| {
            let petya = insert_entity(c, EntityKind::Person, "Петя")?;
            let masha = insert_entity(c, EntityKind::Person, "Маша")?;
            insert_fact(c, &new_fact(&masha.id, "У Маши аллергия на орехи"))?;
            let mut gift = new_fact(&masha.id, "Купить Маше подарок к 20.11");
            gift.visibility = Visibility::Person;
            gift.visibility_owner = Some(petya.id.clone());
            insert_fact(c, &gift)?;
            let mut opinion = new_fact(&masha.id, "Маша слишком торопится");
            opinion.kind = FactKind::Opinion;
            opinion.visibility = Visibility::Character;
            opinion.visibility_owner = Some("arthur".into());
            insert_fact(c, &opinion)?;

            // A voice at a station: nobody known, only the household's.
            let room = Viewer::default();
            assert_eq!(visible_facts(c, &room, true)?.len(), 1);
            // Петя sees his own; Маша does not see the gift; Артур sees his opinion.
            let as_petya = Viewer { person: Some(petya.id.clone()), ..Viewer::default() };
            assert_eq!(visible_facts(c, &as_petya, true)?.len(), 2);
            let as_masha = Viewer { person: Some(masha.id.clone()), ..Viewer::default() };
            assert!(visible_facts(c, &as_masha, true)?.iter().all(|f| !f.text.contains("подарок")));
            let arthur = Viewer { character: Some("arthur".into()), ..Viewer::default() };
            assert_eq!(visible_facts(c, &arthur, true)?.len(), 2);
            assert_eq!(facts_of(c, &room, &masha.id)?.len(), 1);
            let counts = counts(c, &arthur)?;
            assert_eq!((counts.home, counts.personal), (1, 1));
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn closes_reopens_merges_and_erases() {
        let db = Db::memory();
        db.with(|c| {
            let masha = insert_entity(c, EntityKind::Person, "Маша")?;
            let maria = insert_entity(c, EntityKind::Person, "Мария")?;
            let cafe = insert_fact(c, &new_fact(&maria.id, "Маша работает в кафе"))?;
            assert_eq!(close_fact(c, &cafe.id, "перешла в библиотеку", None)?, 1);
            assert_eq!(close_fact(c, &cafe.id, "again", None)?, 0, "closed once");
            assert!(visible_facts(c, &Viewer::default(), true)?.is_empty());
            assert_eq!(visible_facts(c, &Viewer::default(), false)?.len(), 1);
            reopen_fact(c, &cafe.id)?;
            assert_eq!(visible_facts(c, &Viewer::default(), true)?.len(), 1);

            merge_entities(c, &maria.id, &masha.id)?;
            let merged = entity(c, &masha.id)?.unwrap();
            assert_eq!(merged.aliases, vec!["Мария".to_string()]);
            assert_eq!(facts_of(c, &Viewer::default(), &masha.id)?.len(), 1);

            log_change(
                c,
                NewChange {
                    actor_kind: "household",
                    actor_id: None,
                    action: "add",
                    target: "fact",
                    target_id: &cafe.id,
                    summary: &cafe.text,
                    before: None,
                    after: Some("{}".into()),
                    reason: None,
                    run_id: None,
                },
            )?;
            erase_fact(c, &cafe.id)?;
            assert!(fact(c, &cafe.id)?.is_none());
            let log = recent_changes(c, 10)?;
            assert_eq!(log.len(), 1);
            assert_eq!(log[0].summary, "", "the words are gone from the log too");
            assert!(log[0].after.is_none());
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn prunes_old_episodes_and_measures_itself() {
        let db = Db::memory();
        db.with(|c| {
            let episode = NewEpisode {
                station: Some("kitchen".into()),
                character_id: Some("alice".into()),
                question: "Какая погода завтра?".into(),
                answer: "Солнечно.".into(),
                origin: Origin::station_voice(Some("kitchen"), Some("alice")),
            };
            insert_episode(c, &episode)?;
            assert_eq!(prune_episodes(c, "2000-01-01T00:00:00.000Z")?, 0);
            assert!(approx_bytes(c)? > 0);
            assert_eq!(prune_episodes(c, "2999-01-01T00:00:00.000Z")?, 1);
            Ok(())
        })
        .unwrap();
    }
}
