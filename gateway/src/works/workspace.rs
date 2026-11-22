//! Where a work is done (docs/plans/router-and-works.md, block A, 8; decision 9): the run of a work knows only this
//! contract — open a place for the work, hand its result over, clean up after it. The first version has one kind of
//! folder, `git`, and one place, `GitWorkspace`: a copy of the folder on the work's own branch. Another kind of folder
//! later is a new value of `kind` and a new implementation here, not a change of the run.

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use super::git;

/// What a work's place is made of: the folder, the copy's path, the branch, and whether the branch is in the folder
/// already (a copy made again for a later run).
#[derive(Debug, Clone)]
pub struct Place {
    pub folder: PathBuf,
    pub copy: PathBuf,
    pub branch: String,
    /// The branch the work began from and its commit, as the folder had them.
    pub base_commit: Option<String>,
}

/// What the place was opened from: a new copy (prepare it), or the one there already.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Opened {
    New,
    Kept,
}

/// What handing over did: committed what the agent left, the branch fetched into the folder (or why not).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Handed {
    pub committed: bool,
    pub fetched: bool,
    pub changed: bool,
    pub error: Option<String>,
}

#[async_trait]
pub trait Workspace: Send + Sync {
    /// The place ready for a run: the copy there already, else made — from the folder's chosen branch for a new work,
    /// from the work's branch in the folder for a later run.
    async fn open(&self, place: &Place) -> Result<Opened, String>;
    /// The result to the person: what the agent left is committed with `message`, the branch goes into the folder.
    async fn hand_over(&self, place: &Place, message: &str) -> Handed;
    /// The copy goes; the branch stays in the folder.
    async fn clean(&self, place: &Place);
}

pub struct GitWorkspace;

#[async_trait]
impl Workspace for GitWorkspace {
    async fn open(&self, place: &Place) -> Result<Opened, String> {
        if place.copy.join(".git").exists() && git::current_branch(&place.copy).await.as_deref() == Some(&place.branch)
        {
            return Ok(Opened::Kept);
        }
        let from = git::branch_exists(&place.folder, &place.branch).await.then_some(place.branch.as_str());
        git::make_copy(&place.folder, &place.copy, &place.branch, from).await?;
        Ok(Opened::New)
    }

    async fn hand_over(&self, place: &Place, message: &str) -> Handed {
        let mut handed = Handed::default();
        if !place.copy.exists() {
            handed.error = Some("the work copy is gone".into());
            return handed;
        }
        match git::commit_all(&place.copy, message).await {
            Ok(committed) => handed.committed = committed,
            Err(why) => handed.error = Some(why),
        }
        handed.changed = match &place.base_commit {
            Some(base) => git::has_commits(&place.copy, base).await,
            None => true,
        };
        match git::fetch_into(&place.folder, &place.copy, &place.branch).await {
            Ok(()) => handed.fetched = true,
            Err(why) => handed.error = Some(handed.error.map_or(why.clone(), |first| format!("{first}; {why}"))),
        }
        handed
    }

    async fn clean(&self, place: &Place) {
        if let Err(error) = tokio::fs::remove_dir_all(&place.copy).await {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(target: "works", "the copy {} is not cleaned: {error}", place.copy.display());
            }
        }
    }
}

/// Whether a path is inside another (both as they are on disk).
pub fn inside(path: &Path, of: &Path) -> bool {
    let path = crate::utils::files::canonical(path).unwrap_or_else(|_| path.to_path_buf());
    let of = crate::utils::files::canonical(of).unwrap_or_else(|_| of.to_path_buf());
    path.starts_with(&of)
}
