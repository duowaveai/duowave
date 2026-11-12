//! An installed app brought back to a version of its history (docs/plans/package-history.md, «Возврат поставленного
//! приложения»): its code, its data or both — the owner's decision 6; the code alone by default (decision 22).
//!
//! - **The code.** The version's files go into `PACKAGES_DIR/<id>/<its version>/` (a folder already there with the same
//!   files is taken as it is; one with other files under the same number makes the next free patch), the newer
//!   folders go — they are in the history — and the plugin starts on it. The draft is not touched.
//! - **The data.** What the plugin has now is kept first, so the step back can be undone; then its store, jobs and
//!   pictures are replaced, its code out of memory meanwhile.
//! - **Which data.** For a version: the data as it was when that version gave way to the next (the snapshot taken
//!   then), else its own from its start. For a snapshot of the data (a record `data`): that one.
//! - **Rights.** A version that asks for rights or hosts the running one does not have waits for the admin's «ок».
//! - **The kind of data.** A manifest may say `dataVersion`: a step back of the code alone across a change of it is
//!   marked risky — the old code may not read today's data — and «Код и данные» is offered first.
//! - Built-in apps go with the gateway's version: their code is not stepped back, their data is.

use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::Value;

use super::{Author, History, RecordKind, RecordView, What, Who, INSTALLED};
use crate::core::events::Actor;
use crate::platform::http::ApiError;
use crate::plugins::controller::PackageView;
use crate::plugins::manifest::{self, Permission};

/// What a step back would do, shown before it does.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestorePlan {
    pub app: String,
    pub record: RecordView,
    pub what: What,
    /// The version that runs now, and the one that would.
    pub current: Option<String>,
    pub version: Option<String>,
    /// The code may be stepped back (one's own and a catalog's app; a version with a manifest).
    pub can_code: bool,
    /// The data that would come back: the record whose snapshot it is, and what is in it.
    pub data_from: Option<RecordView>,
    pub data_summary: Option<String>,
    pub rights_added: Vec<String>,
    pub hosts_added: Vec<String>,
    pub needs_approval: bool,
    /// The manifests' `dataVersion` differs: the old code may not read today's data.
    pub data_version_changed: bool,
    /// The code alone across such a change.
    pub risky: bool,
    /// In words, one line each: what will happen.
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreReport {
    pub restored: bool,
    /// Set when it waits for the admin's «ок»: the plan with the rights.
    pub needs_approval: Option<RestorePlan>,
    pub message: String,
    pub record: Option<String>,
    pub package: Option<PackageView>,
}

fn names(manifest: &manifest::Manifest) -> BTreeSet<String> {
    manifest.permissions.iter().map(Permission::name).collect()
}

impl History {
    /// The snapshot of the data that goes with a record: its own for a snapshot; for a version, the one taken when
    /// it gave way to the next, else its own; else the nearest before it. The record of it and the data's id.
    pub fn data_for(&self, app: &str, record: &str) -> Option<(String, String)> {
        let line = self.line(app);
        let at = line.iter().position(|(id, _)| id == record)?;
        let target = &line[at].1;
        if target.kind == RecordKind::Data {
            return target.data.clone().map(|data| (record.to_string(), data));
        }
        let mut found = target.data.clone().map(|data| (record.to_string(), data));
        for (id, later) in &line[at + 1..] {
            if let Some(data) = &later.data {
                found = Some((id.clone(), data.clone()));
            }
            let changes_code = later.kind.installs()
                || later.kind == RecordKind::Removed
                || (later.kind == RecordKind::Restore && later.what.is_some_and(What::code));
            if changes_code {
                break;
            }
        }
        found.or_else(|| {
            line[..at].iter().rev().find_map(|(id, earlier)| earlier.data.clone().map(|data| (id.clone(), data)))
        })
    }

    /// What a step back of the installed app to `record` would do.
    pub fn restore_plan(&self, app: &str, record: &str, what: What) -> Result<RestorePlan, ApiError> {
        let host = self.host().ok_or_else(|| ApiError::conflict("Приложений здесь нет."))?;
        let view =
            self.record_view(app, record).ok_or_else(|| ApiError::not_found("Такой записи у приложения нет."))?;
        let target = view.record.clone();
        let installed = host.installed(app);
        let origin = host.origin_of(app);
        let text = self.file_at(&target.tree, "manifest.json").and_then(|bytes| String::from_utf8(bytes).ok());
        let parsed = text.as_deref().and_then(|text| manifest::parse(text).ok());
        let can_code = origin.removable() && parsed.as_ref().is_some_and(|parsed| parsed.id == app);
        if what.code() && !can_code {
            return Err(ApiError::conflict(if origin.removable() {
                "У этой записи нет целой версии приложения: вернуть можно только данные.".to_string()
            } else {
                format!(
                    "«{app}» — {} приложение: его версия — версия шлюза; вернуть можно только данные.",
                    origin.title()
                )
            }));
        }
        let mut lines = Vec::new();
        let (mut rights_added, mut hosts_added) = (Vec::new(), Vec::new());
        let mut data_version_changed = false;
        let version = if what.code() { parsed.as_ref().map(|parsed| parsed.version.to_string()) } else { None };
        if let Some(parsed) = parsed.as_ref().filter(|_| what.code()) {
            let before = installed.as_ref().map(|package| names(&package.manifest)).unwrap_or_default();
            for name in names(parsed).difference(&before) {
                match name.strip_prefix("net:") {
                    Some(host) => hosts_added.push(host.to_string()),
                    None => rights_added.push(name.clone()),
                }
            }
            data_version_changed =
                installed.as_ref().is_some_and(|package| package.manifest.data_version != parsed.data_version);
            lines.push(match &installed {
                Some(package) => format!("Код: версия {} вместо {}.", parsed.version, package.version()),
                None => format!("Код: версия {} снова поставлена.", parsed.version),
            });
        }
        let data = if what.data() { self.data_for(app, record) } else { None };
        let data_from = data.as_ref().and_then(|(id, _)| self.record_view(app, id));
        let data_summary = data
            .as_ref()
            .and_then(|(_, data)| super::data::DataParts::read(self.store(), data))
            .map(|snapshot| snapshot.summary());
        if what.data() {
            match (&data_from, &data_summary) {
                (Some(from), Some(summary)) => {
                    lines.push(format!("Данные: как были {} ({summary}).", local_time(&from.record.at)));
                    lines.push("Что есть сейчас, сохранится в истории: возврат можно отменить.".into());
                }
                _ if what == What::Data => {
                    return Err(ApiError::conflict("Снимка данных той поры в истории нет: возвращать нечего."));
                }
                _ => lines.push("Данные: снимка той поры нет — данные останутся как есть.".into()),
            }
        } else if data_version_changed {
            lines.push("Вид данных менялся: старая версия может не понять сегодняшние данные.".into());
        }
        if !rights_added.is_empty() || !hosts_added.is_empty() {
            let mut asks = rights_added.clone();
            asks.extend(hosts_added.iter().map(|host| format!("адрес {host}")));
            lines.push(format!("Старой версии нужно то, чего нет у работающей: {}.", asks.join(", ")));
        }
        Ok(RestorePlan {
            app: app.to_string(),
            what,
            current: installed.as_ref().map(|package| package.version()),
            version,
            can_code,
            data_from,
            data_summary,
            needs_approval: !rights_added.is_empty() || !hosts_added.is_empty(),
            rights_added,
            hosts_added,
            data_version_changed,
            risky: what == What::Code && data_version_changed,
            lines,
            record: view,
        })
    }

    /// The installed app brought back to `record`: its code, its data or both. A new record says so.
    pub async fn restore_installed(
        &self,
        app: &str,
        record: &str,
        what: What,
        approve: bool,
        by: Author,
        actor: Option<Actor>,
    ) -> Result<RestoreReport, ApiError> {
        let plan = self.restore_plan(app, record, what)?;
        if plan.needs_approval && !approve {
            return Ok(RestoreReport {
                restored: false,
                message: "Старой версии нужны права, которых нет у работающей: их одобряет администратор в пульте."
                    .into(),
                needs_approval: Some(plan),
                record: None,
                package: None,
            });
        }
        let host = self.host().ok_or_else(|| ApiError::conflict("Приложений здесь нет."))?;
        let target = plan.record.record.clone();
        // What there is now, kept before anything changes: the step back can be undone.
        let data_before = self.capture(app);
        let mut version = plan.current.clone();
        let mut tree = self.head(app, INSTALLED).map(|(_, head)| head.tree);
        if what.code() {
            let placed = self.place_version(&host, app, &target.tree)?;
            version = Some(placed.0.clone());
            tree = Some(placed.1);
            host.take_version(app, &placed.0, actor.clone()).await?;
        }
        let mut message = Vec::new();
        if what.code() {
            message.push(format!("Вернулась версия {}.", version.clone().unwrap_or_default()));
        }
        let data = if what.data() { self.data_for(app, record).map(|(_, data)| data) } else { None };
        if what.data() && data.is_none() {
            message.push("Снимка данных той поры нет: данные остались как были.".into());
        }
        if let Some(data) = data {
            host.unload_now(app).await;
            let parts = self.data_parts().ok_or_else(|| ApiError::conflict("Данных приложений здесь нет."))?;
            match parts.apply(self.store(), app, Some(&data)) {
                Ok(()) => message.push("Данные вернулись.".into()),
                Err(why) if what.code() => message.push(format!("Данные не вернулись: {why}.")),
                Err(why) => return Err(ApiError::conflict(format!("Данные не вернулись: {why}."))),
            }
            if let Some(events) = self.events.get() {
                events.emit(crate::core::events::GatewayEvent::PackageDataChanged { package: app.to_string() });
            }
        }
        let tree = match tree {
            Some(tree) => tree,
            None => match host.installed(app) {
                Some(package) => self.snapshot_dir(&package.dir).map_err(ApiError::internal)?,
                None => target.tree.clone(),
            },
        };
        let id = self.note_restored(app, tree, version.clone(), record, what, by, data_before);
        self.tell_packages(app, "restored", version, actor);
        Ok(RestoreReport {
            restored: true,
            needs_approval: None,
            message: format!("{} Вернуть обратно — в истории.", message.join(" ")),
            record: id,
            package: host.view_one(app).await.ok(),
        })
    }

    /// The record of a step back that happened (here, or the host's own rollback).
    #[allow(clippy::too_many_arguments)]
    pub fn note_restored(
        &self,
        app: &str,
        tree: String,
        version: Option<String>,
        from: &str,
        what: What,
        by: Author,
        data: Option<String>,
    ) -> Option<String> {
        let _writing = self.writing();
        let lock = self.lock(app);
        let _held = super::held(&lock);
        let mut record = self.new_record(app, RecordKind::Restore, tree, by);
        record.version = version;
        record.from = self.store().has(from).then(|| from.to_string());
        record.what = Some(what);
        record.data = data;
        if let Some(head) = self.store().ref_get(app, INSTALLED) {
            record.parents.push(head);
        }
        match self.write(&record) {
            Ok(id) => {
                self.mark(app, INSTALLED, Some(&id));
                drop(_held);
                self.tell(app, "history");
                Some(id)
            }
            Err(error) => {
                tracing::warn!(target: "plugins", "history: {app}: the step back is not kept: {error}");
                None
            }
        }
    }

    /// The host's own rollback (the version before the newest; a trial taken back) as a record.
    pub fn note_rollback(&self, app: &str, dir: &std::path::Path, version: &str, actor: Option<&Actor>) {
        let data = self.capture(app);
        let from = self.ensure_recorded(app, dir, version);
        let Ok(tree) = self.snapshot_dir(dir) else { return };
        self.note_restored(
            app,
            tree,
            Some(version.to_string()),
            from.as_deref().unwrap_or_default(),
            What::Code,
            Author::of(actor, Who::System),
            data,
        );
    }

    /// The tree's files as a version folder of the app, the newest on disk: (its version, its tree).
    fn place_version(
        &self,
        host: &crate::plugins::PluginHost,
        app: &str,
        tree: &str,
    ) -> Result<(String, String), ApiError> {
        // Everything on disk is in the history before a folder goes.
        self.adopt_installed(host, app);
        let manifest = self.manifest_of(tree).ok_or_else(|| ApiError::conflict("У версии нет manifest.json."))?;
        let mut version = manifest
            .get("version")
            .and_then(Value::as_str)
            .and_then(|version| semver::Version::parse(version).ok())
            .ok_or_else(|| ApiError::conflict("У версии нет номера."))?;
        let root = host.packages_dir().join(app);
        let _writing = self.writing();
        let lock = self.lock(app);
        let _held = super::held(&lock);
        let same = root.join(version.to_string());
        let placed_tree = if same.is_dir() && self.snapshot_dir(&same).ok().as_deref() == Some(tree) {
            tree.to_string()
        } else {
            while root.join(version.to_string()).exists() {
                version.patch += 1;
            }
            let partial = host.packages_dir().join(format!(".partial-{app}-{version}"));
            let _ = std::fs::remove_dir_all(&partial);
            self.materialize(tree, &partial, false).map_err(ApiError::conflict)?;
            let file = partial.join("manifest.json");
            let mut value = manifest.clone();
            if value.get("version").and_then(Value::as_str) != Some(version.to_string().as_str()) {
                value["version"] = Value::String(version.to_string());
                let pretty = serde_json::to_string_pretty(&value).map_err(ApiError::internal)?;
                std::fs::write(&file, format!("{pretty}\n")).map_err(ApiError::internal)?;
            }
            std::fs::create_dir_all(&root).map_err(ApiError::internal)?;
            if let Err(error) = std::fs::rename(&partial, root.join(version.to_string())) {
                let _ = std::fs::remove_dir_all(&partial);
                return Err(ApiError::internal(error));
            }
            self.snapshot_dir(&root.join(version.to_string())).map_err(ApiError::internal)?
        };
        // The newer folders go: they are in the history.
        for entry in std::fs::read_dir(&root).into_iter().flatten().flatten() {
            let Some(other) = entry.file_name().to_str().and_then(|name| semver::Version::parse(name).ok()) else {
                continue;
            };
            if other > version && entry.path().is_dir() {
                std::fs::remove_dir_all(entry.path()).map_err(ApiError::internal)?;
            }
        }
        Ok((version.to_string(), placed_tree))
    }
}

/// A record's time as the station's people read it.
pub fn local_time(at: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(at)
        .map(|at| at.with_timezone(&chrono::Local).format("%d.%m %H:%M").to_string())
        .unwrap_or_else(|_| at.to_string())
}
