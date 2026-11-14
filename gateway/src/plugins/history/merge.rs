//! Merging two lines of an app (docs/plans/package-history.md, «Объединение»): the owner's decisions 2 and 3 — a simple
//! merge goes by itself, a hard one (or one asked for so) by the agent, and when the agent cannot tell what to keep it
//! stops and asks: «а вот что из этого?».
//!
//! - **The base** is the nearest record both lines come from, across apps too (a copy of one's own → the catalog's).
//! - **By itself**, file by file: changed on one side — that side; the same on both — it; `manifest.json` on both —
//!   field by field (the version: the higher); text on both — line by line (diff3); else, or a stretch changed on both
//!   differently, a conflict. The result is the draft, checked in the sandbox at once.
//! - **By the agent**: conflicts, a merge that does not pass the check, or «через агента». The draft keeps our side of
//!   a conflicting file; the agent reads the three versions (`merge_state`), the requests of both sides since the base,
//!   writes the result, checks it and ends the merge (`finish_merge`) — or asks (`ask_owner`) and waits.
//! - **The state** of a merge under way: `<drafts>/.merge/<id>.json`. «Отменить» puts the draft back as it was.
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::diff::{self, Conflict};
use super::store::{id_of, Kind};
use super::{now_at, Author, History, RecordKind, StepMeta, TreeEntry, Who, CATALOG, DRAFT, INSTALLED};
use crate::platform::http::ApiError;
use crate::plugins::workshop::Workshop;

/// A newer version of the app a copy of one's own was made from: to merge into the copy.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Upstream {
    /// The app the copy came from.
    pub app: String,
    pub title: Option<String>,
    /// Its newer version's record; None — a built-in one the gateway's update changed, recorded when merged.
    pub record: Option<String>,
    pub version: Option<String>,
}

/// A file both sides changed so that it does not merge by itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileConflict {
    pub path: String,
    /// In words: «правки в одном месте», «поля манифеста: …», «удалён с одной стороны», «двоичный файл».
    pub why: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stretches: Vec<Conflict>,
}

/// The agent's question to the owner, and its choices.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Question {
    pub text: String,
    pub options: Vec<String>,
    pub at: String,
}

/// A merge under way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeState {
    pub app: String,
    pub base: Option<String>,
    /// The draft's step before the merge, and the record merged in.
    pub ours: String,
    pub theirs: String,
    pub conflicts: Vec<FileConflict>,
    /// Why the agent: conflicts, the check, or asked.
    pub reason: String,
    pub question: Option<Question>,
    /// What the owner said when asking for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked: Option<String>,
    pub by: Author,
    pub started: String,
}

/// What a merge did.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeReport {
    /// `merged` (by itself), `agent` (the agent took it), `nothing` (already there).
    pub state: &'static str,
    pub message: String,
    pub record: Option<String>,
    pub merge: Option<MergeState>,
}

/// What the mechanical merge gave.
struct Mechanical {
    entries: Vec<TreeEntry>,
    conflicts: Vec<FileConflict>,
}

fn state_path(history: &History, app: &str) -> PathBuf {
    history.dev_dir().join(".merge").join(format!("{app}.json"))
}

/// The merge under way on a draft, if any.
pub fn state(history: &History, app: &str) -> Option<MergeState> {
    serde_json::from_str(&std::fs::read_to_string(state_path(history, app)).ok()?).ok()
}

fn save(history: &History, state: &MergeState) -> Result<(), String> {
    let text = serde_json::to_string_pretty(state).map_err(|error| error.to_string())?;
    super::store::write_whole(&state_path(history, &state.app), text.as_bytes()).map_err(|error| error.to_string())
}

fn forget(history: &History, app: &str) {
    let _ = std::fs::remove_file(state_path(history, app));
}

/// The records open merges stand on: the cleaning keeps them.
pub fn pinned(history: &History) -> HashSet<String> {
    let mut ids = HashSet::new();
    for entry in std::fs::read_dir(history.dev_dir().join(".merge")).into_iter().flatten().flatten() {
        let Some(app) = entry.file_name().to_str().and_then(|name| name.strip_suffix(".json")).map(str::to_string)
        else {
            continue;
        };
        if let Some(state) = state(history, &app) {
            ids.extend(state.base);
            ids.insert(state.ours);
            ids.insert(state.theirs);
        }
    }
    ids
}

/// Three JSON values merged: ours where only ours changed, theirs where only theirs did; objects key by key; the
/// version the higher one. The paths that changed differently on both sides go into `conflicts`.
pub fn merge_json(
    base: Option<&Value>,
    ours: Option<&Value>,
    theirs: Option<&Value>,
    path: &str,
    conflicts: &mut Vec<String>,
) -> Option<Value> {
    if ours == theirs || theirs == base {
        return ours.cloned();
    }
    if ours == base {
        return theirs.cloned();
    }
    if path == "version" {
        let parse =
            |value: Option<&Value>| value.and_then(Value::as_str).and_then(|text| semver::Version::parse(text).ok());
        if let (Some(a), Some(b)) = (parse(ours), parse(theirs)) {
            return Some(Value::String(a.max(b).to_string()));
        }
    }
    match (ours, theirs) {
        (Some(Value::Object(a)), Some(Value::Object(b))) if base.is_none_or(Value::is_object) => {
            let empty = Map::new();
            let base = base.and_then(Value::as_object).unwrap_or(&empty);
            let mut keys: Vec<&String> = a.keys().collect();
            keys.extend(b.keys().filter(|key| !a.contains_key(*key)));
            let mut merged = Map::new();
            for key in keys {
                let inner = if path.is_empty() { key.clone() } else { format!("{path}.{key}") };
                if let Some(value) = merge_json(base.get(key), a.get(key), b.get(key), &inner, conflicts) {
                    merged.insert(key.clone(), value);
                }
            }
            Some(Value::Object(merged))
        }
        _ => {
            conflicts.push(if path.is_empty() { crate::tr!("весь манифест") } else { path.to_string() });
            ours.cloned()
        }
    }
}

impl History {
    /// The nearest record both come from; None — no common history.
    pub fn merge_base(&self, a: &str, b: &str) -> Option<String> {
        let mut ours = HashSet::new();
        let mut queue = VecDeque::from([a.to_string()]);
        while let Some(id) = queue.pop_front() {
            if ours.len() > 20_000 || !ours.insert(id.clone()) {
                continue;
            }
            if let Some(record) = self.record(&id) {
                queue.extend(record.parents);
            }
        }
        let mut seen = HashSet::new();
        let mut queue = VecDeque::from([b.to_string()]);
        while let Some(id) = queue.pop_front() {
            if ours.contains(&id) {
                return Some(id);
            }
            if seen.len() > 20_000 || !seen.insert(id.clone()) {
                continue;
            }
            if let Some(record) = self.record(&id) {
                queue.extend(record.parents);
            }
        }
        None
    }

    fn entries(&self, tree: Option<&str>) -> HashMap<String, TreeEntry> {
        tree.and_then(|tree| self.tree(tree))
            .unwrap_or_default()
            .into_iter()
            .map(|entry| (entry.path.clone(), entry))
            .collect()
    }

    fn text_of(&self, entry: Option<&TreeEntry>) -> Option<Option<String>> {
        match entry {
            None => Some(None),
            Some(entry) => String::from_utf8(self.store().get_kind(&entry.file, Kind::File)?).ok().map(Some),
        }
    }

    /// Three trees merged file by file.
    fn mechanical(&self, base: Option<&str>, ours: &str, theirs: &str) -> Result<Mechanical, String> {
        let (b, o, t) = (self.entries(base), self.entries(Some(ours)), self.entries(Some(theirs)));
        let paths: BTreeSet<&String> = b.keys().chain(o.keys()).chain(t.keys()).collect();
        let mut entries = Vec::new();
        let mut conflicts = Vec::new();
        let file = |entry: Option<&TreeEntry>| entry.map(|entry| entry.file.clone());
        for path in paths {
            let (be, oe, te) = (b.get(path), o.get(path), t.get(path));
            let (bf, of, tf) = (file(be), file(oe), file(te));
            let take = |entry: Option<&TreeEntry>, entries: &mut Vec<TreeEntry>| {
                if let Some(entry) = entry {
                    entries.push(entry.clone());
                }
            };
            if of == tf || tf == bf {
                take(oe, &mut entries);
                continue;
            }
            if of == bf {
                take(te, &mut entries);
                continue;
            }
            // Changed on both sides, differently.
            let (base_text, ours_text, theirs_text) = (self.text_of(be), self.text_of(oe), self.text_of(te));
            let texts = match (base_text, ours_text, theirs_text) {
                (Some(base), Some(Some(ours)), Some(Some(theirs))) => Some((base.unwrap_or_default(), ours, theirs)),
                _ => None,
            };
            let mut conflict = |why: String, stretches: Vec<Conflict>| {
                conflicts.push(FileConflict { path: path.clone(), why, stretches });
            };
            let Some((base, ours, theirs)) = texts else {
                if oe.is_none() || te.is_none() {
                    conflict(crate::tr!("удалён с одной стороны, изменён с другой"), Vec::new());
                } else {
                    conflict(crate::tr!("двоичный файл изменён с обеих сторон"), Vec::new());
                }
                take(oe, &mut entries);
                continue;
            };
            let merged = if path == "manifest.json" {
                let parse = |text: &str| serde_json::from_str::<Value>(text).ok();
                match (parse(&ours), parse(&theirs)) {
                    (Some(ours_json), Some(theirs_json)) => {
                        let mut paths = Vec::new();
                        let merged =
                            merge_json(parse(&base).as_ref(), Some(&ours_json), Some(&theirs_json), "", &mut paths);
                        if paths.is_empty() {
                            let pretty = serde_json::to_string_pretty(&merged.unwrap_or(Value::Null))
                                .map_err(|error| error.to_string())?;
                            Ok(format!("{pretty}\n"))
                        } else {
                            Err((
                                crate::tr!("поля манифеста изменены с обеих сторон: {paths}", paths = paths.join(", ")),
                                Vec::new(),
                            ))
                        }
                    }
                    _ => Err((crate::tr!("манифест не читается как JSON"), Vec::new())),
                }
            } else {
                diff::merge3(&base, &ours, &theirs).map_err(|stretches| (crate::tr!("правки в одном месте"), stretches))
            };
            match merged {
                Ok(text) => {
                    let id = self.store().put(Kind::File, text.as_bytes()).map_err(|error| error.to_string())?;
                    entries.push(TreeEntry { path: path.clone(), file: id, size: text.len() as u64 });
                }
                Err((why, stretches)) => {
                    conflict(why, stretches);
                    take(oe, &mut entries);
                }
            }
        }
        Ok(Mechanical { entries, conflicts })
    }

    /// The requests of a line since `base`: the newest first, at most `max`.
    fn requests_since(&self, from: &str, base: Option<&str>, max: usize) -> Vec<String> {
        let mut found = Vec::new();
        let mut seen = HashSet::new();
        let mut queue = VecDeque::from([from.to_string()]);
        while let Some(id) = queue.pop_front() {
            if Some(id.as_str()) == base || !seen.insert(id.clone()) || found.len() >= max || seen.len() > 2000 {
                continue;
            }
            let Some(record) = self.record(&id) else { continue };
            let words = match (&record.asked, &record.note) {
                (Some(asked), Some(note)) => format!("«{asked}» — {note}"),
                (Some(asked), None) => format!("«{asked}»"),
                (None, Some(note)) => note.clone(),
                (None, None) => String::new(),
            };
            if !words.is_empty() {
                found.push(format!("{} ({}): {words}", super::restore::local_time(&record.at), record.app));
            }
            queue.extend(record.parents);
        }
        found
    }

    /// For a copy of one's own: a newer version of the app it was made from.
    pub fn upstream_of(&self, app: &str) -> Option<Upstream> {
        let line = self.line(app);
        let (_, copy) = line.iter().find(|(_, record)| record.kind == RecordKind::Copy)?;
        let source_record = copy.parents.iter().find_map(|parent| self.record(parent).map(|r| (parent.clone(), r)))?;
        let (base_id, base) = source_record;
        let source = base.app.clone();
        if source == app {
            return None;
        }
        let ours: Vec<String> = [DRAFT, INSTALLED].iter().filter_map(|mark| self.store().ref_get(app, mark)).collect();
        let merged_in = |candidate: &str| ours.iter().any(|ours| self.is_ancestor(candidate, ours));
        let host = self.host();
        let title =
            host.as_ref().and_then(|host| host.installed(&source)).map(|package| package.manifest.title.clone());
        // A built-in source: its folder as the gateway's update left it.
        if let Some(host) = host.as_ref().filter(|host| !host.origin_of(&source).removable()) {
            let package = host.installed(&source)?;
            let tree = self.tree_id_of(&package.dir).ok()?;
            if tree == base.tree || self.line(&source).iter().any(|(id, r)| r.tree == tree && merged_in(id)) {
                return None;
            }
            let record = self.line(&source).into_iter().rev().find(|(_, r)| r.tree == tree).map(|(id, _)| id);
            return Some(Upstream { app: source, title, record, version: Some(package.version()) });
        }
        let head = [CATALOG, INSTALLED].iter().find_map(|mark| self.store().ref_get(&source, mark))?;
        if head == base_id || merged_in(&head) || !self.is_ancestor(&base_id, &head) {
            return None;
        }
        let version = self.record(&head).and_then(|record| record.version);
        Some(Upstream { app: source, title, record: Some(head), version })
    }

    /// A folder's tree id without writing anything.
    pub fn tree_id_of(&self, dir: &std::path::Path) -> Result<String, String> {
        let mut files = Vec::new();
        super::walk(dir, dir, &mut files).map_err(|error| error.to_string())?;
        let mut entries = Vec::new();
        for (path, full) in files {
            let bytes = std::fs::read(&full).map_err(|error| error.to_string())?;
            entries.push(TreeEntry { path, file: id_of(Kind::File, &bytes), size: bytes.len() as u64 });
        }
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        let bytes = serde_json::to_vec(&entries).map_err(|error| error.to_string())?;
        Ok(id_of(Kind::Tree, &bytes))
    }

    /// The upstream's record, written now when it is a built-in's folder not recorded yet.
    pub fn upstream_record(&self, app: &str) -> Option<String> {
        let upstream = self.upstream_of(app)?;
        if let Some(record) = upstream.record {
            return Some(record);
        }
        let host = self.host()?;
        let package = host.installed(&upstream.app)?;
        self.ensure_recorded(&upstream.app, &package.dir, &package.version())
    }
}

/// The feed's offer for a copy of one's own.
pub fn upstream(history: &History, app: &str) -> Option<Upstream> {
    history.upstream_of(app)
}

impl Workshop {
    /// `record` merged into the draft: by itself when it can and the result passes the check, else the agent's (the
    /// route starts its work with `brief`). `agent` — the owner asked for the agent.
    pub async fn merge(
        &self,
        id: &str,
        record: &str,
        agent: bool,
        by: Author,
        asked: Option<String>,
    ) -> Result<MergeReport, ApiError> {
        let history = self.history().ok_or_else(|| ApiError::conflict(crate::tr!("Истории здесь нет.")))?.clone();
        self.draft(id)?;
        if state(&history, id).is_some() {
            return Err(ApiError::conflict(crate::tr!("Объединение уже идёт: закончите его или отмените.")));
        }
        if by.who != Who::Agent && self.activity().jobs().iter().any(|job| job.draft == id && job.busy) {
            return Err(ApiError::conflict(crate::tr!(
                "Агент сейчас работает над черновиком: дождитесь или остановите его."
            )));
        }
        history.before_work(id);
        let ours = history
            .store()
            .ref_get(id, DRAFT)
            .ok_or_else(|| ApiError::conflict(crate::tr!("У черновика ещё нет истории.")))?;
        let theirs_record =
            history.record(record).ok_or_else(|| ApiError::not_found(crate::tr!("Такой записи нет.")))?;
        if history.is_ancestor(record, &ours) {
            // The draft's own past: nothing merges by itself — the agent takes from it what was asked.
            if !agent && asked.is_none() && by.who != Who::Agent {
                return Ok(MergeReport {
                    state: "nothing",
                    message: crate::tr!("Эта версия — прошлое черновика: всё из неё уже было. Чтобы взять из неё что-то, скажите агенту, что именно."),
                    record: None,
                    merge: None,
                });
            }
            let ours_tree = history.record(&ours).map(|record| record.tree).unwrap_or_default();
            let (now, then) = (history.entries(Some(&ours_tree)), history.entries(Some(&theirs_record.tree)));
            let paths: BTreeSet<&String> = now.keys().chain(then.keys()).collect();
            let conflicts = paths
                .into_iter()
                .filter(|path| now.get(*path).map(|e| &e.file) != then.get(*path).map(|e| &e.file))
                .map(|path| FileConflict {
                    path: path.clone(),
                    why: crate::tr!("в той версии иначе"),
                    stretches: Vec::new(),
                })
                .collect();
            let merge = MergeState {
                app: id.to_string(),
                base: None,
                ours,
                theirs: record.to_string(),
                conflicts,
                reason: crate::tr!(
                    "взять из версии от {at} то, что просили",
                    at = super::restore::local_time(&theirs_record.at)
                ),
                question: None,
                asked,
                by,
                started: now_at(),
            };
            save(&history, &merge).map_err(ApiError::internal)?;
            history.tell(id, "merge");
            return Ok(MergeReport {
                state: "agent",
                message: crate::tr!("Агент берёт из прошлой версии то, что просили: {reason}.", reason = merge.reason),
                record: None,
                merge: Some(merge),
            });
        }
        let base = history.merge_base(&ours, record);
        let ours_tree = history.record(&ours).map(|record| record.tree).unwrap_or_default();
        let base_tree = base.as_deref().and_then(|base| history.record(base)).map(|record| record.tree);
        let mechanical =
            history.mechanical(base_tree.as_deref(), &ours_tree, &theirs_record.tree).map_err(ApiError::internal)?;
        let tree = history.put_tree(mechanical.entries).map_err(ApiError::internal)?;
        let dir = history.dev_dir().join(id);
        history.materialize(&tree, &dir, true).map_err(ApiError::conflict)?;
        let checked = if mechanical.conflicts.is_empty() { self.check(id).await.ok() } else { None };
        let passed = checked.as_ref().is_some_and(|report| report.ok);
        if passed && !agent {
            let mut meta = StepMeta::new(RecordKind::Merge, by);
            meta.other = Some(record.to_string());
            meta.asked = asked;
            meta.note = Some(crate::tr!(
                "объединено само с записью от {at}",
                at = super::restore::local_time(&theirs_record.at)
            ));
            let step = history.step(id, meta);
            return Ok(MergeReport {
                state: "merged",
                message: crate::tr!("Объединено само, проверка прошла. Поставить можно кнопкой «Поставить»."),
                record: step,
                merge: None,
            });
        }
        let reason = if agent {
            crate::tr!("вы попросили объединить через агента")
        } else if !mechanical.conflicts.is_empty() {
            crate::tr!(
                "правки в одних и тех же местах: {list}",
                list = mechanical.conflicts.iter().map(|c| c.path.as_str()).collect::<Vec<_>>().join(", ")
            )
        } else {
            crate::tr!("само объединилось, но проверка не прошла")
        };
        let merge = MergeState {
            app: id.to_string(),
            base,
            ours,
            theirs: record.to_string(),
            conflicts: mechanical.conflicts,
            reason: reason.clone(),
            question: None,
            asked,
            by,
            started: now_at(),
        };
        save(&history, &merge).map_err(ApiError::internal)?;
        history.tell(id, "merge");
        Ok(MergeReport {
            state: "agent",
            message: crate::tr!(
                "Объединяет агент: {reason}. Если ему будет непонятно, что оставить, он спросит.",
                reason = reason
            ),
            record: None,
            merge: Some(merge),
        })
    }

    /// What the agent is told when it takes a merge.
    pub fn merge_brief(&self, merge: &MergeState) -> String {
        let Some(history) = self.history() else { return String::new() };
        let ours = history.requests_since(&merge.ours, merge.base.as_deref(), 12);
        let theirs = history.requests_since(&merge.theirs, merge.base.as_deref(), 12);
        let list = |items: &[String]| if items.is_empty() { "—".to_string() } else { items.join("\n- ") };
        let conflicts = if merge.conflicts.is_empty() {
            "Спорных файлов нет: само объединилось, но проверка (check) не прошла — исправь.".to_string()
        } else {
            merge.conflicts.iter().map(|c| format!("{} — {}", c.path, c.why)).collect::<Vec<_>>().join("; ")
        };
        format!(
            "Объедини две версии черновика ({}). Почему ты: {}.\n\nНаша сторона (черновик) — что просили:\n- {}\n\n\
Другая сторона — что просили:\n- {}\n\nСпорные файлы: {conflicts}. В черновике сейчас всё, что объединилось само, а в \
спорных файлах — наша сторона. Три версии спорного файла (основа, наша, другая) — инструмент merge_state. Объединяй \
намерения, а не строки: сохрани то, что просили с обеих сторон. Если по смыслу непонятно, что оставить, не угадывай — \
вызови ask_owner с вопросом и вариантами и закончи ответ этим вопросом. Когда готово: check, потом finish_merge.{}",
            merge.app,
            merge.reason,
            list(&ours),
            list(&theirs),
            merge.asked.as_deref().map(|asked| format!("\n\nВладелец сказал: «{asked}».")).unwrap_or_default()
        )
    }

    /// The merge under way, for the agent: the three versions of its conflicting files (or of `path`).
    pub fn merge_files(&self, id: &str, path: Option<&str>) -> Result<Value, ApiError> {
        let history = self.history().ok_or_else(|| ApiError::conflict(crate::tr!("Истории здесь нет.")))?;
        let merge = state(history, id).ok_or_else(|| ApiError::conflict(crate::tr!("Объединения сейчас нет.")))?;
        let tree_of = |record: Option<&str>| record.and_then(|record| history.record(record)).map(|record| record.tree);
        let (base, ours, theirs) =
            (tree_of(merge.base.as_deref()), tree_of(Some(&merge.ours)), tree_of(Some(&merge.theirs)));
        let text = |tree: &Option<String>, path: &str| -> Value {
            match tree.as_deref().and_then(|tree| history.file_at(tree, path)) {
                Some(bytes) => match String::from_utf8(bytes) {
                    Ok(text) if text.len() <= 64 * 1024 => Value::String(text),
                    Ok(text) => Value::String(format!("{}…", text.chars().take(64 * 1024).collect::<String>())),
                    Err(_) => Value::String("(двоичный файл)".into()),
                },
                None => Value::Null,
            }
        };
        let paths: Vec<String> = match path {
            Some(path) => vec![path.to_string()],
            None => merge.conflicts.iter().map(|conflict| conflict.path.clone()).collect(),
        };
        let files: Vec<Value> = paths
            .iter()
            .map(|path| {
                serde_json::json!({
                    "path": path,
                    "base": text(&base, path),
                    "ours": text(&ours, path),
                    "theirs": text(&theirs, path),
                })
            })
            .collect();
        Ok(
            serde_json::json!({ "reason": merge.reason, "conflicts": merge.conflicts, "files": files, "question": merge.question }),
        )
    }

    /// The agent's end of a merge: the draft checked, a step with both parents, the merge gone.
    pub async fn finish_merge(&self, id: &str) -> Result<String, ApiError> {
        let history = self.history().ok_or_else(|| ApiError::conflict(crate::tr!("Истории здесь нет.")))?.clone();
        let merge = state(&history, id).ok_or_else(|| ApiError::conflict(crate::tr!("Объединения сейчас нет.")))?;
        let report = self.check(id).await?;
        if !report.ok {
            return Err(ApiError::bad_request("Проверка не прошла: исправь, потом finish_merge."));
        }
        let mut meta = StepMeta::new(RecordKind::Merge, Author { who: Who::Agent, name: None, via: None });
        meta.other = Some(merge.theirs.clone());
        meta.asked = merge.asked.clone();
        meta.note = Some(crate::tr!("объединено агентом: {reason}", reason = merge.reason));
        history.step(id, meta);
        forget(&history, id);
        history.tell(id, "merge");
        Ok("Объединение закончено и записано в историю. Поставить — кнопкой «Поставить» в пульте.".into())
    }

    /// The agent's question: the merge waits for the owner's answer.
    pub fn ask_owner(&self, id: &str, text: &str, options: Vec<String>) -> Result<String, ApiError> {
        let history = self.history().ok_or_else(|| ApiError::conflict(crate::tr!("Истории здесь нет.")))?;
        let Some(mut merge) = state(history, id) else {
            return Ok("Объединения нет: просто задай вопрос в ответе.".into());
        };
        let options: Vec<String> = options
            .into_iter()
            .map(|option| option.trim().to_string())
            .filter(|option| !option.is_empty())
            .take(4)
            .collect();
        merge.question = Some(Question { text: text.trim().to_string(), options, at: now_at() });
        save(history, &merge).map_err(ApiError::internal)?;
        history.tell(id, "question");
        Ok("Вопрос записан, владелец увидит его с вариантами. Закончи ответ этим вопросом и жди: ничего больше не меняй.".into())
    }

    /// The owner's answer: the message the agent goes on with.
    pub fn answer_merge(&self, id: &str, choice: &str) -> Result<String, ApiError> {
        let history = self.history().ok_or_else(|| ApiError::conflict(crate::tr!("Истории здесь нет.")))?;
        let mut merge = state(history, id).ok_or_else(|| ApiError::conflict(crate::tr!("Объединения сейчас нет.")))?;
        let question = merge.question.take().ok_or_else(|| ApiError::conflict(crate::tr!("Вопроса нет.")))?;
        save(history, &merge).map_err(ApiError::internal)?;
        history.tell(id, "merge");
        Ok(format!(
            "Владелец ответил на твой вопрос «{}»: «{}». Продолжай объединение так, потом check и finish_merge.",
            question.text,
            choice.trim()
        ))
    }

    /// «Отменить»: the draft back to its step before the merge.
    pub fn cancel_merge(&self, id: &str) -> Result<(), ApiError> {
        let history = self.history().ok_or_else(|| ApiError::conflict(crate::tr!("Истории здесь нет.")))?.clone();
        let merge = state(&history, id).ok_or_else(|| ApiError::conflict(crate::tr!("Объединения сейчас нет.")))?;
        self.activity().stop(id);
        let ours = history
            .record(&merge.ours)
            .ok_or_else(|| ApiError::conflict(crate::tr!("Шага до объединения уже нет.")))?;
        history.materialize(&ours.tree, &history.dev_dir().join(id), true).map_err(ApiError::conflict)?;
        forget(&history, id);
        let mut meta = StepMeta::new(RecordKind::Restore, Author::new(Who::Hand));
        meta.from = Some(merge.ours.clone());
        meta.note = Some(crate::tr!("объединение отменено"));
        history.step(id, meta);
        history.tell(id, "merge");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{history, write_draft};
    use super::*;

    #[test]
    fn merges_a_manifest_field_by_field_and_takes_the_higher_version() {
        let base = serde_json::json!({ "version": "1.0.0", "title": "Погода", "permissions": [] });
        let ours = serde_json::json!({ "version": "1.0.1", "title": "Погода коротко", "permissions": [] });
        let theirs = serde_json::json!({ "version": "1.1.0", "title": "Погода", "permissions": ["net:x.org"] });
        let mut conflicts = Vec::new();
        let merged = merge_json(Some(&base), Some(&ours), Some(&theirs), "", &mut conflicts).unwrap();
        assert!(conflicts.is_empty(), "{conflicts:?}");
        assert_eq!(
            merged,
            serde_json::json!({ "version": "1.1.0", "title": "Погода коротко", "permissions": ["net:x.org"] })
        );
        let other = serde_json::json!({ "version": "1.0.0", "title": "Погода длинно", "permissions": [] });
        merge_json(Some(&base), Some(&ours), Some(&other), "", &mut conflicts);
        assert_eq!(conflicts, vec!["title".to_string()]);
    }

    #[test]
    fn finds_the_base_and_merges_files_changed_on_one_side_by_themselves() {
        let dir = tempfile::tempdir().unwrap();
        let history = history(dir.path());
        write_draft(dir.path(), "app", &[("manifest.json", "{}\n"), ("backend.js", "a\nb\nc\n"), ("panel.css", "x\n")]);
        let base = history.step("app", StepMeta::new(RecordKind::Found, Author::new(Who::System))).unwrap();
        write_draft(dir.path(), "app", &[("backend.js", "A\nb\nc\n")]);
        let ours = history.step("app", StepMeta::new(RecordKind::Step, Author::new(Who::Agent))).unwrap();
        // The other side, from the base: another file and another line (as the station would have it).
        let other_side = |files: &[(&str, &str)]| {
            let folder = tempfile::tempdir().unwrap();
            for (path, text) in [("manifest.json", "{}\n"), ("backend.js", "a\nb\nc\n"), ("panel.css", "x\n")] {
                std::fs::write(folder.path().join(path), text).unwrap();
            }
            for (path, text) in files {
                std::fs::write(folder.path().join(path), text).unwrap();
            }
            let tree = history.snapshot_dir(folder.path()).unwrap();
            let mut record = history.new_record("app", RecordKind::Step, tree, Author::new(Who::Agent));
            record.parents = vec![base.clone()];
            history.write(&record).unwrap()
        };
        let theirs = other_side(&[("backend.js", "a\nb\nC\n"), ("panel.css", "y\n")]);
        assert_eq!(history.merge_base(&ours, &theirs), Some(base.clone()));
        let tree = |id: &str| history.record(id).unwrap().tree;
        let merged = history.mechanical(Some(&tree(&base)), &tree(&ours), &tree(&theirs)).unwrap();
        assert!(merged.conflicts.is_empty());
        let result = history.put_tree(merged.entries).unwrap();
        assert_eq!(String::from_utf8(history.file_at(&result, "backend.js").unwrap()).unwrap(), "A\nb\nC\n");
        assert_eq!(String::from_utf8(history.file_at(&result, "panel.css").unwrap()).unwrap(), "y\n");
        // The same line both ways: a conflict, our side kept meanwhile.
        let other = other_side(&[("backend.js", "Z\nb\nC\n")]);
        let clash = history.mechanical(Some(&tree(&base)), &tree(&ours), &tree(&other)).unwrap();
        assert_eq!(clash.conflicts.len(), 1);
        assert_eq!(clash.conflicts[0].path, "backend.js");
    }
}
