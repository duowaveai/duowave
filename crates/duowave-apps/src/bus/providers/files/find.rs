//! Finding by name in a folder (docs/plans/shared-folders.md, «Новые умения папки»): «найди рецепт борща» looks
//! through the folder's names — up to 8 levels, 50 000 entries and 20 seconds — and scores each as the music is
//! scored (`match_score`). The walk is kept for a while per folder, so a second «найди» answers at once; a change in
//! the folder (a file put, moved, removed through the bus) forgets it. Names are compared in one Unicode form.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use unicode_normalization::UnicodeNormalization;

use super::path;

/// How deep the walk goes.
pub const DEPTH: usize = 8;
/// How many entries it takes at most.
pub const MOST: usize = 50_000;
/// How long it may walk.
pub const WALK_FOR: Duration = Duration::from_secs(20);
/// How long a walk is kept.
pub const KEPT: Duration = Duration::from_secs(10 * 60);
/// What scores lower is not a match.
const ENOUGH: f64 = 0.5;

#[derive(Debug, Clone)]
struct Entry {
    /// The path below the folder, parts joined by `/`.
    path: String,
    name: String,
    folder: bool,
}

struct Walk {
    entries: Vec<Entry>,
    /// The walk stopped at its limits: not every name was seen.
    partial: bool,
    made: Instant,
}

/// A name found.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub path: String,
    pub name: String,
    pub folder: bool,
    pub score: f64,
}

#[derive(Default)]
pub struct Finder {
    walks: Mutex<HashMap<PathBuf, Walk>>,
}

/// A name as compared: one Unicode form, the extension off.
fn comparable(name: &str) -> String {
    let stem = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() && extension.len() <= 5 => stem,
        _ => name,
    };
    stem.nfc().collect::<String>().replace(['_', '-'], " ")
}

impl Finder {
    /// The best matches for `query` below `root` (the folder's own place, checked by `path::root`), and whether the
    /// folder was too big to see every name.
    pub fn find(&self, root: &Path, query: &str, limit: usize) -> Result<(Vec<Hit>, bool), path::Refused> {
        let base = path::root(root)?;
        let query: String = query.nfc().collect();
        let mut walks = self.walks.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let fresh = walks.get(&base).is_some_and(|walk| walk.made.elapsed() < KEPT);
        if !fresh {
            walks.insert(base.clone(), walk(&base));
        }
        let walk = walks.get(&base).expect("just walked");
        let mut hits: Vec<Hit> = walk
            .entries
            .iter()
            .map(|entry| Hit {
                path: entry.path.clone(),
                name: entry.name.clone(),
                folder: entry.folder,
                score: crate::skills::media::sources::match_score(&query, &comparable(&entry.name)),
            })
            .filter(|hit| hit.score >= ENOUGH)
            .collect();
        hits.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.path.len().cmp(&b.path.len())));
        hits.truncate(limit.clamp(1, 50));
        Ok((hits, walk.partial))
    }

    /// A folder changed: its walks — of it, of the folders in it and of those around it — are made again next time.
    pub fn forget(&self, root: &Path) {
        let Ok(base) = path::root(root) else { return };
        self.walks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain(|kept, _| !(kept.starts_with(&base) || base.starts_with(kept)));
    }
}

fn walk(base: &Path) -> Walk {
    let mut entries = Vec::new();
    let began = Instant::now();
    let mut partial = false;
    let mut queue: Vec<(PathBuf, String, usize)> = vec![(base.to_path_buf(), String::new(), 0)];
    while let Some((dir, prefix, depth)) = queue.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else { continue };
        for item in read.flatten() {
            if entries.len() >= MOST || began.elapsed() > WALK_FOR {
                partial = true;
                return Walk { entries, partial, made: Instant::now() };
            }
            let full = item.path();
            if !path::shown(base, &full) {
                continue;
            }
            let name = item.file_name().to_string_lossy().to_string();
            let folder = std::fs::metadata(&full).is_ok_and(|meta| meta.is_dir());
            let relative = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
            if folder {
                if depth + 1 < DEPTH {
                    queue.push((full, relative.clone(), depth + 1));
                } else {
                    partial = true;
                }
            }
            entries.push(Entry { path: relative, name, folder });
        }
    }
    Walk { entries, partial, made: Instant::now() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_by_name_deep_in_the_folder_and_forgets_after_a_change() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Супы")).unwrap();
        std::fs::write(dir.path().join("Супы/Борщ украинский.txt"), "свёкла").unwrap();
        std::fs::write(dir.path().join("Супы/Щи.txt"), "капуста").unwrap();
        std::fs::write(dir.path().join(".скрытый борщ.txt"), "x").unwrap();
        let finder = Finder::default();
        let (hits, partial) = finder.find(dir.path(), "борщ", 10).unwrap();
        assert!(!partial);
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].path, "Супы/Борщ украинский.txt");
        std::fs::write(dir.path().join("Борщ зелёный.txt"), "щавель").unwrap();
        assert_eq!(finder.find(dir.path(), "борщ", 10).unwrap().0.len(), 1, "the walk is kept");
        finder.forget(dir.path());
        assert_eq!(finder.find(dir.path(), "борщ", 10).unwrap().0.len(), 2);
        assert!(finder.find(&dir.path().join("нет"), "борщ", 10).is_err());
    }

    #[test]
    fn a_name_is_compared_without_its_extension_and_in_one_form() {
        assert_eq!(comparable("Отпуск_2025.JPG"), "Отпуск 2025");
        assert_eq!(comparable(".bashrc"), ".bashrc");
        let decomposed: String = "Йод".nfd().collect();
        assert_eq!(comparable(&decomposed), "Йод");
    }
}
