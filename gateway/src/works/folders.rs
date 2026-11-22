//! The work folders (docs/plans/router-and-works.md, block A, 6; decisions 9 and 13): folders on this computer the
//! owner added in the panel for the helper to work in. Kept in the setting `work_folders`, local — a path stays on its
//! computer. Added only in the panel, by the admin: a folder with git (its top), not the gateway's data folder, not
//! the home folder itself. The rest of the gateway knows a folder only by its id and its name.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::git;
use super::workspace::inside;
use crate::core::events::Actor;
use crate::platform::http::ApiError;
use crate::settings::definitions::{WorkFolder, WORK_FOLDERS};
use crate::settings::SettingsService;

/// What a folder may say of itself at most: names, commands, rules.
const NAME: usize = 60;
const COMMANDS: usize = 20;
const COMMAND: usize = 500;

pub fn list(settings: &SettingsService) -> Vec<WorkFolder> {
    settings.get(&WORK_FOLDERS, None).unwrap_or_default()
}

pub fn get(settings: &SettingsService, id: &str) -> Option<WorkFolder> {
    list(settings).into_iter().find(|folder| folder.id == id)
}

/// A folder as the panel sends it: everything but its id.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Asked {
    pub name: Option<String>,
    pub path: Option<String>,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub max_usd: Option<f64>,
    pub prepare: Option<Vec<String>>,
    pub checks: Option<Vec<String>>,
    pub allow: Option<Vec<String>>,
}

/// Where the gateway itself keeps things: a folder there is never one to work in.
pub struct Guarded {
    pub data: Vec<PathBuf>,
    pub home: Option<PathBuf>,
}

/// A new folder: its path checked — there, with git at its top, not the gateway's, not the home folder itself.
pub async fn add(
    settings: &SettingsService,
    guarded: &Guarded,
    asked: Asked,
    actor: Option<Actor>,
) -> Result<WorkFolder, ApiError> {
    let path = checked_path(asked.path.as_deref().unwrap_or_default(), guarded).await?;
    let mut folders = list(settings);
    if folders.iter().any(|folder| Path::new(&folder.path) == path) {
        return Err(ApiError::field("path", crate::tr!("Эта папка уже добавлена.")));
    }
    let name = asked
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .or_else(|| path.file_name().map(|name| name.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "folder".into());
    let base = git::slug(&name);
    let mut id = base.clone();
    let mut n = 2;
    while folders.iter().any(|folder| folder.id == id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    let mut folder = WorkFolder {
        id,
        name: String::new(),
        path: path.display().to_string(),
        kind: "git".into(),
        agent: None,
        model: None,
        max_usd: None,
        prepare: Vec::new(),
        checks: Vec::new(),
        allow: Vec::new(),
    };
    apply(&mut folder, Asked { name: Some(name), path: None, ..asked })?;
    folders.push(folder.clone());
    settings.set(&WORK_FOLDERS, &folders, actor)?;
    Ok(folder)
}

/// A folder changed: its name, agent, model, ceiling, commands. Its path does not change: another folder is added anew.
pub fn update(
    settings: &SettingsService,
    id: &str,
    asked: Asked,
    actor: Option<Actor>,
) -> Result<WorkFolder, ApiError> {
    if asked.path.is_some() {
        return Err(ApiError::field("path", crate::tr!("Путь папки не меняется: добавьте её заново.")));
    }
    let mut folders = list(settings);
    let folder = folders
        .iter_mut()
        .find(|folder| folder.id == id)
        .ok_or_else(|| ApiError::not_found(crate::tr!("Такой рабочей папки нет.")))?;
    apply(folder, asked)?;
    let changed = folder.clone();
    settings.set(&WORK_FOLDERS, &folders, actor)?;
    Ok(changed)
}

pub fn remove(settings: &SettingsService, id: &str, actor: Option<Actor>) -> Result<(), ApiError> {
    let mut folders = list(settings);
    let before = folders.len();
    folders.retain(|folder| folder.id != id);
    if folders.len() == before {
        return Err(ApiError::not_found(crate::tr!("Такой рабочей папки нет.")));
    }
    settings.set(&WORK_FOLDERS, &folders, actor)
}

/// The agent's card is gone: the folders that had it go back to the seat «Исполнитель». Their names, for the words.
pub fn forget_agent(settings: &SettingsService, agent: &str) -> Vec<String> {
    let mut folders = list(settings);
    let mut named = Vec::new();
    for folder in folders.iter_mut().filter(|folder| folder.agent.as_deref() == Some(agent)) {
        folder.agent = None;
        folder.model = None;
        named.push(folder.name.clone());
    }
    if !named.is_empty() {
        if let Err(error) = settings.set(&WORK_FOLDERS, &folders, None) {
            tracing::warn!(target: "works", "the folders still name the agent {agent}: {error:?}");
        }
    }
    named
}

fn apply(folder: &mut WorkFolder, asked: Asked) -> Result<(), ApiError> {
    if let Some(name) = asked.name {
        let name = name.trim();
        if name.is_empty() {
            return Err(ApiError::field("name", crate::tr!("У папки должно быть название.")));
        }
        folder.name = name.chars().take(NAME).collect();
    }
    if let Some(agent) = asked.agent {
        folder.agent = Some(agent.trim().to_string()).filter(|agent| !agent.is_empty());
    }
    if let Some(model) = asked.model {
        folder.model = Some(model.trim().to_string()).filter(|model| !model.is_empty());
    }
    if let Some(usd) = asked.max_usd {
        if !usd.is_finite() || usd < 0.0 || usd > 1000.0 {
            return Err(ApiError::field("maxUsd", crate::tr!("Потолок запуска — от 0 до 1000 долларов.")));
        }
        folder.max_usd = (usd > 0.0).then_some(usd);
    }
    if let Some(prepare) = asked.prepare {
        folder.prepare = commands("prepare", prepare)?;
    }
    if let Some(checks) = asked.checks {
        folder.checks = commands("checks", checks)?;
    }
    if let Some(allow) = asked.allow {
        folder.allow = commands("allow", allow)?;
    }
    Ok(())
}

fn commands(field: &str, list: Vec<String>) -> Result<Vec<String>, ApiError> {
    let list: Vec<String> =
        list.into_iter().map(|line| line.trim().to_string()).filter(|line| !line.is_empty()).collect();
    if list.len() > COMMANDS || list.iter().any(|line| line.len() > COMMAND || line.contains('\n')) {
        return Err(ApiError::field(field, crate::tr!("Не больше 20 строк, каждая — одна команда до 500 знаков.")));
    }
    Ok(list)
}

/// The folder's path as it is on disk: there, a folder, the top of a git work tree, none of the gateway's own.
async fn checked_path(asked: &str, guarded: &Guarded) -> Result<PathBuf, ApiError> {
    let asked = asked.trim();
    if asked.is_empty() {
        return Err(ApiError::field("path", crate::tr!("Укажите путь к папке.")));
    }
    let expanded = match (asked.strip_prefix("~/"), &guarded.home) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(asked),
    };
    if !expanded.is_absolute() {
        return Err(ApiError::field("path", crate::tr!("Нужен полный путь к папке, от корня диска.")));
    }
    let path = crate::utils::files::canonical(&expanded)
        .map_err(|_| ApiError::field("path", crate::tr!("Такой папки нет на этом компьютере.")))?;
    if !path.is_dir() {
        return Err(ApiError::field("path", crate::tr!("Это файл, а нужна папка.")));
    }
    let home = guarded.home.as_ref().and_then(|home| crate::utils::files::canonical(home).ok());
    if path.parent().is_none() || home.as_deref() == Some(path.as_path()) {
        return Err(ApiError::field(
            "path",
            crate::tr!("Домашнюю папку и корень диска добавить нельзя: выберите папку одного проекта."),
        ));
    }
    if guarded.data.iter().any(|data| inside(&path, data) || inside(data, &path)) {
        return Err(ApiError::field("path", crate::tr!("Это папка самого DuoWave: в ней работать нельзя.")));
    }
    // Only with git (decision 9): the work's result is a branch.
    let Some(top) = git::toplevel(&path).await else {
        return Err(ApiError::field(
            "path",
            crate::tr!("В этой папке нет git. Работа в папке — это своя ветка, поэтому нужна папка с git: выполните в ней «git init» и сделайте первый коммит."),
        ));
    };
    let top = crate::utils::files::canonical(&top).unwrap_or(top);
    if top != path {
        return Err(ApiError::field(
            "path",
            crate::tr!("Эта папка — часть репозитория {top}: добавьте его корень.", top = top.display()),
        ));
    }
    if git::head_commit(&path).await.is_none() {
        return Err(ApiError::field("path", crate::tr!("В репозитории ещё нет ни одного коммита: сделайте первый.")));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::events::EventBus;
    use crate::storage::Db;
    use std::sync::Arc;

    #[tokio::test]
    async fn only_a_folder_with_git_and_none_of_the_gateways_is_added() {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Db::open(Some(&dir.path().join("state"))).unwrap());
        let settings = SettingsService::new(db, EventBus::new());
        let home = dir.path().join("home");
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        let guarded = Guarded { data: vec![data.clone()], home: Some(home.clone()) };
        let plain = home.join("notes");
        std::fs::create_dir_all(&plain).unwrap();
        let asked = |path: &Path| Asked { path: Some(path.display().to_string()), ..Asked::default() };
        let refused = add(&settings, &guarded, asked(&plain), None).await.unwrap_err();
        assert!(format!("{refused:?}").contains("git"), "{refused:?}");
        assert!(add(&settings, &guarded, asked(&home), None).await.is_err(), "the home folder itself");
        assert!(add(&settings, &guarded, asked(&data), None).await.is_err(), "the gateway's own");
        assert!(add(&settings, &guarded, asked(Path::new("relative/x")), None).await.is_err());
        let site = home.join("Сайт");
        std::fs::create_dir_all(site.join("src")).unwrap();
        git::git(&site, &["init", "--quiet", "-b", "main"]).await.unwrap();
        assert!(add(&settings, &guarded, asked(&site), None).await.is_err(), "no commit yet");
        std::fs::write(site.join("a.txt"), "a").unwrap();
        git::git(&site, &["add", "-A"]).await.unwrap();
        git::git(&site, &["-c", "user.name=T", "-c", "user.email=t@example.com", "commit", "-qm", "a"]).await.unwrap();
        assert!(add(&settings, &guarded, asked(&site.join("src")), None).await.is_err(), "a part of the repository");
        let added =
            add(&settings, &guarded, Asked { checks: Some(vec!["npm test".into(), " ".into()]), ..asked(&site) }, None)
                .await
                .unwrap();
        assert_eq!((added.id.as_str(), added.name.as_str(), added.kind.as_str()), ("sayt", "Сайт", "git"));
        assert_eq!(added.checks, vec!["npm test".to_string()]);
        assert!(add(&settings, &guarded, asked(&site), None).await.is_err(), "added already");
        let changed = update(
            &settings,
            "sayt",
            Asked { allow: Some(vec!["Bash(git push:*)".into()]), max_usd: Some(3.0), ..Asked::default() },
            None,
        )
        .unwrap();
        assert_eq!((changed.allow.len(), changed.max_usd), (1, Some(3.0)));
        assert_eq!(list(&settings).len(), 1);
        remove(&settings, "sayt", None).unwrap();
        assert!(list(&settings).is_empty());
    }
}
