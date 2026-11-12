//! The history's room and its cleaning (docs/plans/package-history.md, «Место и чистка», «Где хранить историю»).
//!
//! - **Always kept:** the records the marks point to (ours and the station's on the computer), the last two versions
//!   of each app, an open merge's sides.
//! - **By the rules:** the draft's steps of the last `days`, at most `steps` an app; the versions, at most `versions`;
//!   the data snapshots, at most `data` an app (an older version keeps its record, without its data).
//! - **Over the budget** the allowances halve, round by round, the marks and the last two versions never.
//! - **A removed app** keeps its history 30 days, then it goes — unless its draft is still there.
//! - **Objects** nothing kept points to go; one younger than ten minutes stays (a step being written).
//! - On the station that sends its history to the computer (`home`), what the computer has is cleaned harder.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{data::DataParts, History, Record, RecordKind, TreeEntry, INSTALLED};
use crate::settings::definitions::PACKAGE_HISTORY;

/// A removed app's history lives this long.
const REMOVED_DAYS: i64 = 30;
/// An object younger than this is never cleaned: a step may be being written.
const GRACE_SECS: u64 = 600;
/// Versions of each app kept whatever the budget.
const LAST_VERSIONS: usize = 2;
/// Rounds of halving the allowances when the budget is short.
const ROUNDS: u32 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Place {
    /// Only here.
    Here,
    /// On the computer: the station keeps the least of it (the station's choice).
    Home,
    /// Here and on the computer: the station keeps its short tail, the computer all of it.
    Both,
}

impl Place {
    pub fn parse(text: &str) -> Option<Place> {
        match text {
            "here" => Some(Place::Here),
            "home" => Some(Place::Home),
            "both" => Some(Place::Both),
            _ => None,
        }
    }
}

/// How much history is kept here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Policy {
    pub place: Place,
    pub budget_mb: u64,
    pub days: u64,
    pub steps: usize,
    pub versions: usize,
    pub data: usize,
}

impl Policy {
    /// The station's: 2 MB (`/data` has some 45 MB free, measured 11.11.2026).
    pub fn station() -> Policy {
        Policy { place: Place::Both, budget_mb: 2, days: 30, steps: 50, versions: 10, data: 5 }
    }

    pub fn computer() -> Policy {
        Policy { place: Place::Here, budget_mb: 200, days: 90, steps: 500, versions: 50, data: 20 }
    }

    /// The device's own, with what the setting says over it.
    pub fn of(on_station: bool, stored: Option<&Value>) -> Policy {
        let mut policy = if on_station { Policy::station() } else { Policy::computer() };
        let Some(stored) = stored else { return policy };
        if let Some(place) = stored.get("place").and_then(Value::as_str).and_then(Place::parse) {
            // A computer keeps its own: there is no other gateway to send it to (a server — later).
            policy.place = if on_station { place } else { Place::Here };
        }
        let number = |key: &str, min: u64, max: u64| stored.get(key).and_then(Value::as_u64).map(|n| n.clamp(min, max));
        if let Some(mb) = number("budgetMb", 1, 10_000) {
            policy.budget_mb = mb;
        }
        if let Some(days) = number("days", 1, 3650) {
            policy.days = days;
        }
        if let Some(steps) = number("steps", 1, 10_000) {
            policy.steps = steps as usize;
        }
        policy
    }

    /// The allowances of a round: halved each one, never below one (the last two versions are kept apart).
    fn squeezed(&self, round: u32) -> (usize, usize, usize) {
        let half = |n: usize| (n >> round).max(1);
        (half(self.steps), half(self.versions), half(self.data))
    }
}

/// What a cleaning did.
#[derive(Debug, Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Cleaned {
    pub records: usize,
    pub objects: usize,
    pub bytes_before: u64,
    pub bytes_after: u64,
    pub apps_gone: Vec<String>,
}

/// The history's room for the panel («Память»).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Room {
    pub bytes: u64,
    pub budget_bytes: u64,
    pub policy: Policy,
    pub on_station: bool,
    pub apps: usize,
    /// The places this gateway may choose: the station has three, a computer one.
    pub places: Vec<Place>,
    /// The computer is reachable for the station's history.
    pub home: bool,
}

fn age_days(at: &str, now: i64) -> i64 {
    chrono::DateTime::parse_from_rfc3339(at).map(|at| (now - at.timestamp_millis()) / 86_400_000).unwrap_or(0)
}

/// Whether a record is a version of the installed app (counted as versions, not as steps).
fn is_version(record: &Record) -> bool {
    record.kind.installs()
        || record.kind == RecordKind::Removed
        || (record.kind == RecordKind::Restore && record.what.is_some())
}

impl History {
    pub fn policy(&self) -> Policy {
        let stored = self.settings().and_then(|settings| settings.get(&PACKAGE_HISTORY, None));
        Policy::of(self.on_station(), stored.as_ref())
    }

    pub fn room(&self) -> Room {
        let policy = self.policy();
        Room {
            bytes: self.store().bytes(),
            budget_bytes: policy.budget_mb * 1024 * 1024,
            on_station: self.on_station(),
            apps: self.store().apps().len(),
            places: if self.on_station() { vec![Place::Both, Place::Home, Place::Here] } else { vec![Place::Here] },
            home: self.sync.home_up(),
            policy,
        }
    }

    /// The records of an app this round keeps.
    fn kept_of(
        &self,
        app: &str,
        line: &[(String, Record)],
        policy: &Policy,
        round: u32,
        pinned: &HashSet<String>,
        now: i64,
    ) -> (HashSet<String>, HashSet<String>) {
        let (steps, versions, data) = policy.squeezed(round);
        let mut kept: HashSet<String> = HashSet::new();
        for (_, id) in self.store().refs(app) {
            kept.insert(id);
        }
        for peer in self.store().peers() {
            for (_, id) in self.store().remote_refs(Some(&peer), app) {
                kept.insert(id);
            }
        }
        kept.extend(line.iter().filter(|(id, _)| pinned.contains(id)).map(|(id, _)| id.clone()));
        // Not yet on the computer: kept by the rules of «both» even when the station keeps the least.
        let unsent = |id: &String| policy.place == Place::Home && !self.was_sent(id);
        let mut version_count = 0;
        let mut step_count = 0;
        for (id, record) in line.iter().rev() {
            if is_version(record) {
                version_count += 1;
                if version_count <= LAST_VERSIONS.max(versions) || unsent(id) {
                    kept.insert(id.clone());
                }
            } else if record.kind == RecordKind::Data {
                // Kept by the data allowance below.
            } else {
                step_count += 1;
                let fresh = age_days(&record.at, now) < policy.days as i64;
                if (fresh && step_count <= steps) || unsent(id) {
                    kept.insert(id.clone());
                }
            }
        }
        // The data: the newest snapshots, of the records kept and the snapshots of their own.
        let mut data_kept = HashSet::new();
        for (id, record) in line.iter().rev() {
            let Some(snapshot) = &record.data else { continue };
            let counted = kept.contains(id) || record.kind == RecordKind::Data;
            if counted && (data_kept.len() < data || unsent(id)) {
                data_kept.insert(snapshot.clone());
                kept.insert(id.clone());
            }
        }
        (kept, data_kept)
    }

    /// One cleaning: the rules, then rounds over the budget.
    pub fn clean(&self) -> Cleaned {
        let mut policy = self.policy();
        if policy.place == Place::Home {
            // The station keeps the least: the computer has the rest.
            policy.steps = 5;
            policy.versions = LAST_VERSIONS;
            policy.data = 1;
        }
        let budget = policy.budget_mb * 1024 * 1024;
        let _cleaning = self.cleaning();
        let mut cleaned = Cleaned { bytes_before: self.store().bytes(), ..Cleaned::default() };
        let now = chrono::Utc::now().timestamp_millis();
        let pinned = super::merge::pinned(self);
        for round in 0..ROUNDS {
            let mut live: HashSet<String> = HashSet::new();
            for app in self.store().apps() {
                let line = self.line(&app);
                let draft = self.dev_dir().join(&app).is_dir();
                let installed = self.store().ref_get(&app, INSTALLED).is_some();
                let gone = self
                    .store()
                    .removed_at(&app)
                    .is_some_and(|at| age_days(&at, now) >= REMOVED_DAYS && !draft && !installed);
                if gone {
                    cleaned.records += line.len();
                    self.store().forget_app(&app);
                    cleaned.apps_gone.push(app.clone());
                    continue;
                }
                let (kept, data_kept) = self.kept_of(&app, &line, &policy, round, &pinned, now);
                let ids: Vec<String> = line.iter().map(|(id, _)| id.clone()).filter(|id| kept.contains(id)).collect();
                if ids.len() != line.len() {
                    cleaned.records += line.len() - ids.len();
                    if let Err(error) = self.store().log_write(&app, &ids) {
                        tracing::warn!(target: "plugins", "history: {app}: the line is not cleaned: {error}");
                        live.extend(line.iter().map(|(id, _)| id.clone()));
                        continue;
                    }
                }
                for (id, record) in line.iter().filter(|(id, _)| kept.contains(id)) {
                    self.mark_live(id, record, &data_kept, &mut live);
                }
            }
            // The station's records on the computer whose app line this computer has not: their marks keep them.
            for peer in self.store().peers() {
                for app in self.store().peer_apps(&peer) {
                    for (_, id) in self.store().remote_refs(Some(&peer), &app) {
                        if let Some(record) = self.record(&id) {
                            let data: HashSet<String> = record.data.iter().cloned().collect();
                            self.mark_live(&id, &record, &data, &mut live);
                        }
                    }
                }
            }
            cleaned.objects += self.sweep(&live);
            if self.store().bytes() <= budget {
                break;
            }
            if round + 1 == ROUNDS {
                tracing::warn!(
                    target: "plugins",
                    "history: {} KB over its {} MB after cleaning: only the marks and the last versions are left",
                    self.store().bytes() / 1024,
                    policy.budget_mb
                );
            }
        }
        cleaned.bytes_after = self.store().bytes();
        if cleaned.records > 0 || cleaned.objects > 0 {
            tracing::info!(
                target: "plugins",
                "history: cleaned {} record(s), {} object(s): {} KB → {} KB",
                cleaned.records,
                cleaned.objects,
                cleaned.bytes_before / 1024,
                cleaned.bytes_after / 1024
            );
        }
        cleaned
    }

    /// A kept record and what it points to: its tree and files, its data (when kept) and its pictures.
    fn mark_live(&self, id: &str, record: &Record, data_kept: &HashSet<String>, live: &mut HashSet<String>) {
        live.insert(id.to_string());
        if live.insert(record.tree.clone()) {
            if let Some(entries) = self.tree(&record.tree) {
                live.extend(entries.into_iter().map(|entry: TreeEntry| entry.file));
            }
        }
        if let Some(data) = record.data.as_ref().filter(|data| data_kept.contains(*data)) {
            if live.insert(data.clone()) {
                if let Some(snapshot) = DataParts::read(self.store(), data) {
                    live.extend(snapshot.pictures.into_iter().map(|picture| picture.file));
                }
            }
        }
    }

    /// Objects nothing kept points to go, but the young ones. How many went.
    fn sweep(&self, live: &HashSet<String>) -> usize {
        let now = std::time::SystemTime::now();
        let mut gone = 0;
        for (id, _) in self.store().objects() {
            if live.contains(&id) {
                continue;
            }
            let young = std::fs::metadata(self.store().root().join("objects").join(&id[..2]).join(&id[2..]))
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|at| now.duration_since(at).ok())
                .is_none_or(|age| age.as_secs() < GRACE_SECS);
            if !young {
                self.store().remove(&id);
                gone += 1;
            }
        }
        gone
    }

    /// Once a day: the plugins' data kept when it changed since its last snapshot.
    pub fn keep_data(&self) -> usize {
        let Some(host) = self.host() else { return 0 };
        let mut kept = 0;
        for package in host.installed_all() {
            if package.manifest.kind != crate::plugins::manifest::PackageKind::Plugin {
                continue;
            }
            let app = package.manifest.id.clone();
            let _writing = self.writing();
            let Some(data) = self.capture(&app) else { continue };
            let last = self.line(&app).into_iter().rev().find_map(|(_, record)| record.data);
            if last.as_deref() == Some(data.as_str()) {
                continue;
            }
            let lock = self.lock(&app);
            let _held = super::held(&lock);
            let installed = self.store().ref_get(&app, INSTALLED);
            let tree = match installed.as_deref().and_then(|id| self.record(id)) {
                Some(record) => record.tree,
                None => match self.snapshot_dir(&package.dir) {
                    Ok(tree) => tree,
                    Err(_) => continue,
                },
            };
            let mut record = self.new_record(&app, RecordKind::Data, tree, super::Author::new(super::Who::System));
            record.data = Some(data);
            record.version = Some(package.version());
            record.parents = installed.into_iter().collect();
            if self.write(&record).is_ok() {
                kept += 1;
            }
        }
        kept
    }

    /// What every app's history takes, roughly: its objects' sizes as written (shared files counted once each).
    pub fn sizes(&self) -> HashMap<String, u64> {
        let sizes: HashMap<String, u64> = self.store().objects().into_iter().collect();
        let mut by_app = HashMap::new();
        for app in self.store().apps() {
            let mut seen = HashSet::new();
            for (id, record) in self.line(&app) {
                seen.insert(id);
                seen.insert(record.tree.clone());
                if let Some(entries) = self.tree(&record.tree) {
                    seen.extend(entries.into_iter().map(|entry| entry.file));
                }
                if let Some(data) = record.data {
                    seen.insert(data);
                }
            }
            by_app.insert(app, seen.iter().filter_map(|id| sizes.get(id)).sum());
        }
        by_app
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{history, write_draft};
    use super::super::{Author, StepMeta, Who};
    use super::*;

    #[test]
    fn the_settings_place_and_numbers_go_over_the_devices_own() {
        assert_eq!(Policy::of(true, None), Policy::station());
        let stored = serde_json::json!({ "place": "home", "budgetMb": 1, "steps": 0 });
        let policy = Policy::of(true, Some(&stored));
        assert_eq!((policy.place, policy.budget_mb, policy.steps, policy.days), (Place::Home, 1, 1, 30));
        assert_eq!(Policy::of(false, Some(&stored)).place, Place::Here, "a computer keeps its own");
        assert_eq!(Policy::station().squeezed(2), (12, 2, 1));
    }

    #[test]
    fn cleans_old_steps_and_their_files_and_keeps_the_marks() {
        let dir = tempfile::tempdir().unwrap();
        let history = history(dir.path());
        write_draft(dir.path(), "notes", &[("manifest.json", "{}")]);
        let mut ids = Vec::new();
        for n in 0..8 {
            write_draft(dir.path(), "notes", &[("backend.js", &format!("// {n}\n{}", "x".repeat(50)))]);
            ids.push(history.step("notes", StepMeta::new(RecordKind::Step, Author::new(Who::Agent))).unwrap());
        }
        // The objects are young: age them past the grace.
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        for (id, _) in history.store().objects() {
            let path = history.store().root().join("objects").join(&id[..2]).join(&id[2..]);
            std::fs::File::options().write(true).open(path).unwrap().set_modified(old).unwrap();
        }
        let line = history.line("notes");
        let policy = Policy { steps: 3, ..Policy::computer() };
        let (kept, _) =
            history.kept_of("notes", &line, &policy, 0, &HashSet::new(), chrono::Utc::now().timestamp_millis());
        assert_eq!(kept.len(), 3, "the last three steps (the newest is the draft's mark)");
        assert!(kept.contains(ids.last().unwrap()));
        let stored = serde_json::json!({ "steps": 3 });
        let policy = Policy::of(false, Some(&stored));
        assert_eq!(policy.steps, 3);
    }
}
