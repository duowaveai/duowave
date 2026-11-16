//! Git for the work copies (docs/plans/router-and-works.md, block A, 8): the person's own `git`, run as it is. A work
//! copy is a separate `git clone --local` in the gateway's data folder — not a `git worktree`, so its settings never
//! change the folder's — with the folder's remotes and their push addresses: whether the agent may push is the folder's
//! allowed commands' business (decision 21). The work's branch goes back into the folder by `git fetch` from the copy;
//! the folder's chosen branch and its files are never touched, and the gateway itself pushes nothing.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

/// A git command at most this long: a clone of a big folder is local, by hard links.
const GIT_WAIT: Duration = Duration::from_secs(300);

/// Runs `git` in `dir`: what it printed, or why it failed in its own words.
pub async fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = tokio::process::Command::new("git");
    command
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = match tokio::time::timeout(GIT_WAIT, command.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => return Err("git is not installed".into()),
        Ok(Err(error)) => return Err(error.to_string()),
        Err(_) => return Err(format!("git {} took too long", args.first().unwrap_or(&""))),
    };
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim_end().to_string())
    } else {
        let words = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if words.is_empty() { format!("git {} failed", args.first().unwrap_or(&"")) } else { words })
    }
}

/// The top of the git work tree `path` is in; None — not in one (or no git).
pub async fn toplevel(path: &Path) -> Option<PathBuf> {
    git(path, &["rev-parse", "--show-toplevel"]).await.ok().map(PathBuf::from)
}

/// The branch checked out now; None — a detached head or no commit yet.
pub async fn current_branch(path: &Path) -> Option<String> {
    git(path, &["symbolic-ref", "--quiet", "--short", "HEAD"]).await.ok().filter(|branch| !branch.is_empty())
}

pub async fn head_commit(path: &Path) -> Option<String> {
    git(path, &["rev-parse", "HEAD"]).await.ok()
}

pub async fn branch_exists(path: &Path, branch: &str) -> bool {
    git(path, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")]).await.is_ok()
}

/// A remote of the folder: its name, where it fetches from and where it pushes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    pub name: String,
    pub fetch: Vec<String>,
    pub push: Vec<String>,
}

pub async fn remotes(path: &Path) -> Vec<Remote> {
    let names = git(path, &["remote"]).await.unwrap_or_default();
    let mut remotes = Vec::new();
    for name in names.lines().map(str::trim).filter(|name| !name.is_empty()) {
        let fetch = git(path, &["remote", "get-url", "--all", name]).await.unwrap_or_default();
        let push = git(path, &["remote", "get-url", "--push", "--all", name]).await.unwrap_or_default();
        remotes.push(Remote {
            name: name.to_string(),
            fetch: fetch.lines().map(str::to_string).filter(|url| !url.is_empty()).collect(),
            push: push.lines().map(str::to_string).filter(|url| !url.is_empty()).collect(),
        });
    }
    remotes
}

/// The folder's own settings a copy needs too: who commits, how git signs in to its remotes.
const CARRIED: &[&str] = &["user.", "credential.", "core.sshcommand", "http."];

/// A copy of the folder for a work: cloned locally, on its own branch from what the folder has checked out now (or,
/// `from` given, from that branch of the folder — a copy made again for a work that has its branch), with the folder's
/// remotes and their push addresses, and the folder's own settings of who commits and how it signs in.
pub async fn make_copy(folder: &Path, copy: &Path, branch: &str, from: Option<&str>) -> Result<(), String> {
    if let Some(parent) = copy.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|error| error.to_string())?;
    }
    if copy.exists() {
        tokio::fs::remove_dir_all(copy).await.map_err(|error| error.to_string())?;
    }
    let parent = copy.parent().unwrap_or(copy);
    let folder_text = folder.display().to_string();
    let copy_text = copy.display().to_string();
    git(parent, &["clone", "--local", "--quiet", "--no-tags", &folder_text, &copy_text]).await?;
    match from {
        Some(from) => {
            git(copy, &["checkout", "--quiet", "-B", branch, &format!("refs/remotes/origin/{from}")]).await?;
        }
        None => {
            git(copy, &["checkout", "--quiet", "-b", branch]).await?;
        }
    }
    // The clone's `origin` is the folder itself: the copy gets the folder's own remotes instead.
    git(copy, &["remote", "remove", "origin"]).await?;
    for remote in remotes(folder).await {
        let Some(first) = remote.fetch.first() else { continue };
        git(copy, &["remote", "add", &remote.name, first]).await?;
        for url in remote.fetch.iter().skip(1) {
            git(copy, &["remote", "set-url", "--add", &remote.name, url]).await?;
        }
        if remote.push != remote.fetch {
            for url in &remote.push {
                git(copy, &["remote", "set-url", "--add", "--push", &remote.name, url]).await?;
            }
        }
    }
    let local = git(folder, &["config", "--local", "--list"]).await.unwrap_or_default();
    for line in local.lines() {
        let Some((key, value)) = line.split_once('=') else { continue };
        if CARRIED.iter().any(|prefix| key.to_lowercase().starts_with(prefix)) {
            let _ = git(copy, &["config", "--add", key, value]).await;
        }
    }
    Ok(())
}

/// Commits what the agent left uncommitted, as the folder's git has it who commits. True — something was committed.
pub async fn commit_all(copy: &Path, message: &str) -> Result<bool, String> {
    let status = git(copy, &["status", "--porcelain"]).await?;
    if status.trim().is_empty() {
        return Ok(false);
    }
    git(copy, &["add", "-A"]).await?;
    match git(copy, &["commit", "--quiet", "--no-verify", "-m", message]).await {
        Ok(_) => Ok(true),
        // Nobody named in git at all: the work's own name, so the branch still has the change.
        Err(why) if why.contains("user.email") || why.contains("Please tell me who you are") => {
            git(
                copy,
                &[
                    "-c",
                    "user.name=DuoWave",
                    "-c",
                    "user.email=duowave@localhost",
                    "commit",
                    "--quiet",
                    "--no-verify",
                    "-m",
                    message,
                ],
            )
            .await?;
            Ok(true)
        }
        Err(why) => Err(why),
    }
}

/// Whether the copy's branch has commits beyond `base`.
pub async fn has_commits(copy: &Path, base: &str) -> bool {
    git(copy, &["rev-list", "--count", &format!("{base}..HEAD")])
        .await
        .ok()
        .and_then(|n| n.parse::<u64>().ok())
        .unwrap_or(0)
        > 0
}

/// The work's branch into the folder, from the copy: the folder gets the branch, nothing else changes there.
pub async fn fetch_into(folder: &Path, copy: &Path, branch: &str) -> Result<(), String> {
    let refspec = format!("+refs/heads/{branch}:refs/heads/{branch}");
    git(folder, &["fetch", "--quiet", "--no-tags", &copy.display().to_string(), &refspec]).await.map(|_| ())
}

/// A branch's name from a title: Latin letters, digits and dashes, the first words that say something — «Страница
/// контактов с формой» → `stranitsa-kontaktov-formoy`.
pub fn slug(title: &str) -> String {
    let latin: String = title.to_lowercase().chars().map(latin_of).collect();
    let words: Vec<String> = latin
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| word.len() > 2 || word.chars().all(|c| c.is_ascii_digit()))
        .take(3)
        .map(str::to_string)
        .collect();
    let mut slug = words.join("-");
    slug.truncate(40);
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "work".into()
    } else {
        slug
    }
}

fn latin_of(c: char) -> String {
    let text = match c {
        'а' => "a",
        'б' => "b",
        'в' => "v",
        'г' => "g",
        'д' => "d",
        'е' | 'э' => "e",
        'ё' => "yo",
        'ж' => "zh",
        'з' => "z",
        'и' => "i",
        'й' | 'ы' => "y",
        'к' => "k",
        'л' => "l",
        'м' => "m",
        'н' => "n",
        'о' => "o",
        'п' => "p",
        'р' => "r",
        'с' => "s",
        'т' => "t",
        'у' => "u",
        'ф' => "f",
        'х' => "kh",
        'ц' => "ts",
        'ч' => "ch",
        'ш' => "sh",
        'щ' => "shch",
        'ъ' | 'ь' => "",
        'ю' => "yu",
        'я' => "ya",
        c if c.is_ascii_alphanumeric() => return c.to_string(),
        _ => " ",
    };
    text.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_branch_is_named_by_the_first_words_in_latin() {
        assert_eq!(slug("Страница контактов с формой"), "stranitsa-kontaktov-formoy");
        assert_eq!(slug("Fix the login bug"), "fix-the-login");
        assert_eq!(slug("!!!"), "work");
        assert_eq!(slug("Счётчик 2"), "schyotchik-2");
    }

    /// A folder with a remote: the copy has the same addresses, its own branch, and the branch comes back to the
    /// folder by fetch while the folder's chosen branch stays.
    #[tokio::test]
    async fn a_copy_keeps_the_folders_remotes_and_its_branch_comes_back_by_fetch() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("site");
        std::fs::create_dir_all(&folder).unwrap();
        git(&folder, &["init", "--quiet", "-b", "main"]).await.unwrap();
        git(&folder, &["config", "user.name", "Owner"]).await.unwrap();
        git(&folder, &["config", "user.email", "owner@example.com"]).await.unwrap();
        std::fs::write(folder.join("app.txt"), "one\n").unwrap();
        git(&folder, &["add", "-A"]).await.unwrap();
        git(&folder, &["commit", "--quiet", "-m", "first"]).await.unwrap();
        git(&folder, &["remote", "add", "origin", "git@example.com:me/site.git"]).await.unwrap();
        git(&folder, &["remote", "set-url", "--add", "--push", "origin", "git@example.com:me/site-push.git"])
            .await
            .unwrap();
        let copy = dir.path().join("works/1/copy");
        make_copy(&folder, &copy, "duowave/kontakty", None).await.unwrap();
        assert_eq!(current_branch(&copy).await.as_deref(), Some("duowave/kontakty"));
        let theirs = remotes(&copy).await;
        assert_eq!(theirs, remotes(&folder).await, "the same addresses, push ones too");
        std::fs::write(copy.join("contacts.txt"), "form\n").unwrap();
        assert!(commit_all(&copy, "Страница контактов").await.unwrap());
        let author = git(&copy, &["log", "-1", "--format=%an"]).await.unwrap();
        assert_eq!(author, "Owner", "who commits is the folder's own");
        assert!(!commit_all(&copy, "again").await.unwrap(), "nothing left to commit");
        fetch_into(&folder, &copy, "duowave/kontakty").await.unwrap();
        assert!(branch_exists(&folder, "duowave/kontakty").await);
        assert_eq!(current_branch(&folder).await.as_deref(), Some("main"), "the folder's branch is not changed");
        assert!(!folder.join("contacts.txt").exists(), "nor its files");
        // A copy made again from the work's branch in the folder.
        make_copy(&folder, &copy, "duowave/kontakty", Some("duowave/kontakty")).await.unwrap();
        assert!(copy.join("contacts.txt").exists());
    }
}
