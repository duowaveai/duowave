//! DATA_DIR: one folder for everything the gateway writes — the database and its key, journals, content, voices, the
//! voice lab, logs. Docker mounts it as a volume, the app keeps it in the user's data folder, a service in
//! /var/lib, the station in /data. Unset, every path stays beside the working copy, as it always was.

use std::path::{Path, PathBuf};

use super::Env;

/// `DATA_DIR/<sub>`, or the path beside the working copy without DATA_DIR.
pub fn data_path(env: &Env, sub: &str, legacy: &str) -> PathBuf {
    match env.get("DATA_DIR").map(str::trim).filter(|root| !root.is_empty()) {
        Some(root) => Path::new(root).join(sub),
        None => PathBuf::from(legacy),
    }
}

/// The .env files to read, the first one winning: DATA_DIR/.env (the app's, the container's), then the working copy's.
pub fn env_files(env: &Env) -> Vec<PathBuf> {
    match env.get("DATA_DIR").map(str::trim).filter(|root| !root.is_empty()) {
        Some(root) => vec![Path::new(root).join(".env"), PathBuf::from(".env")],
        None => vec![PathBuf::from(".env")],
    }
}

/// What a new content folder starts with: the radio list and the folder's README, built into the program so the
/// station needs nothing beside it. Music, tales and messages are the household's.
const SEEDS: &[(&str, &str)] = &[
    ("radio.json", include_str!("../../../content/radio.json")),
    ("README.md", include_str!("../../../content/README.md")),
];

/// Makes DATA_DIR ready before the gateway starts: the folder itself and the shipped content in its content/, written
/// once (a radio list the household has edited stays theirs). Nothing happens without DATA_DIR.
pub fn prepare_data_dir(env: &Env) -> std::io::Result<Option<PathBuf>> {
    let Some(root) = env.get("DATA_DIR").map(str::trim).filter(|root| !root.is_empty()) else {
        return Ok(None);
    };
    let root = PathBuf::from(root);
    std::fs::create_dir_all(&root)?;
    let content = env.get("CONTENT_DIR").map(PathBuf::from).unwrap_or_else(|| data_path(env, "content", "content"));
    std::fs::create_dir_all(&content)?;
    for (name, text) in SEEDS {
        let target = content.join(name);
        if !target.exists() {
            std::fs::write(&target, text)?;
        }
    }
    Ok(Some(root))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_go_under_data_dir_only_when_it_is_set() {
        let plain = Env::from_pairs([("X", "1")]);
        assert_eq!(data_path(&plain, "state", "data/state"), PathBuf::from("data/state"));
        assert_eq!(env_files(&plain), vec![PathBuf::from(".env")]);
        let data = Env::from_pairs([("DATA_DIR", "/var/lib/duowave")]);
        assert_eq!(data_path(&data, "state", "data/state"), PathBuf::from("/var/lib/duowave/state"));
        assert_eq!(env_files(&data), vec![PathBuf::from("/var/lib/duowave/.env"), PathBuf::from(".env")]);
    }

    #[test]
    fn seeds_the_content_once_and_keeps_the_households_edits() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::from_pairs([("DATA_DIR", dir.path().to_str().unwrap())]);
        prepare_data_dir(&env).unwrap();
        let radio = dir.path().join("content/radio.json");
        assert!(radio.exists());
        std::fs::write(&radio, "[]").unwrap();
        prepare_data_dir(&env).unwrap();
        assert_eq!(std::fs::read_to_string(&radio).unwrap(), "[]");
        assert!(prepare_data_dir(&Env::default()).unwrap().is_none());
    }
}
