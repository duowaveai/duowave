//! The environment as the gateway reads it: a snapshot of the variables, after the `.env` files, so a test can build
//! any configuration without touching the process. Values are read the way the Node gateway read them: an empty
//! value is no value, a flag is 1/true/yes/on, a list is comma-separated.

use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct Env {
    vars: HashMap<String, String>,
}

impl Env {
    /// The process environment with the `.env` files under it: a variable already set wins over the files, and the
    /// first file that names a variable wins over the later ones (DATA_DIR/.env before the working copy's).
    pub fn from_process() -> Env {
        let mut vars: HashMap<String, String> = process_vars();
        let probe = Env { vars: vars.clone() };
        for file in super::data_dir::env_files(&probe) {
            for (key, value) in read_env_file(&file) {
                vars.entry(key).or_insert(value);
            }
        }
        Env { vars }
    }

    /// These variables with the given `.env` files under them, the first file winning: the desktop app passes its
    /// own environment and only DATA_DIR/.env — never a working copy's `.env`, which may be someone's keys.
    pub fn with_files<I, K, V>(pairs: I, files: &[std::path::PathBuf]) -> Env
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let mut vars: HashMap<String, String> = pairs.into_iter().map(|(k, v)| (k.into(), v.into())).collect();
        for file in files {
            for (key, value) in read_env_file(file) {
                vars.entry(key).or_insert(value);
            }
        }
        Env { vars }
    }

    /// Only these variables: tests and tools.
    pub fn from_pairs<I, K, V>(pairs: I) -> Env
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        Env { vars: pairs.into_iter().map(|(k, v)| (k.into(), v.into())).collect() }
    }

    pub fn with(mut self, key: &str, value: &str) -> Env {
        self.vars.insert(key.to_string(), value.to_string());
        self
    }

    /// The value as it is, set or not (an empty one too).
    pub fn raw(&self, key: &str) -> Option<&str> {
        self.vars.get(key).map(String::as_str)
    }

    /// The value when it is set and not empty.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.raw(key).filter(|value| !value.is_empty())
    }

    pub fn string(&self, key: &str, fallback: &str) -> String {
        self.get(key).unwrap_or(fallback).to_string()
    }

    pub fn optional(&self, key: &str) -> Option<String> {
        self.get(key).map(str::to_string)
    }

    pub fn flag(&self, key: &str, fallback: bool) -> bool {
        match self.raw(key).map(str::trim) {
            None | Some("") => fallback,
            Some(value) => matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"),
        }
    }

    pub fn number(&self, key: &str, fallback: f64) -> f64 {
        match self.raw(key).map(str::trim) {
            None | Some("") => fallback,
            Some(value) => value.parse::<f64>().ok().filter(|n| n.is_finite()).unwrap_or(fallback),
        }
    }

    pub fn integer(&self, key: &str, fallback: u64) -> u64 {
        let value = self.number(key, fallback as f64);
        if value < 0.0 {
            0
        } else {
            value.round() as u64
        }
    }

    /// Comma-separated, trimmed, empty items dropped; the fallback only when the variable is not set at all.
    pub fn list(&self, key: &str, fallback: &[&str]) -> Vec<String> {
        match self.raw(key) {
            None => fallback.iter().map(|item| item.to_string()).collect(),
            Some(value) => {
                value.split(',').map(str::trim).filter(|item| !item.is_empty()).map(str::to_string).collect()
            }
        }
    }

    pub fn is_set(&self, key: &str) -> bool {
        self.vars.contains_key(key)
    }

    /// Every variable, as a child process would inherit them (the model servers take a filtered copy).
    pub fn vars(&self) -> impl Iterator<Item = (&str, &str)> {
        self.vars.iter().map(|(key, value)| (key.as_str(), value.as_str()))
    }
}

/// KEY=value lines; `#` comments, `export `, quotes. A file that is not there is nothing.
fn read_env_file(file: &Path) -> Vec<(String, String)> {
    match dotenvy::from_path_iter(file) {
        Ok(iter) => iter.filter_map(Result::ok).collect(),
        Err(_) => Vec::new(),
    }
}

/// The process environment as text. std::env::vars() panics on a variable that is not Unicode (a stray byte in
/// some LANG or a path); such a variable is read with U+FFFD in its place, as Node's process.env did, and a name
/// that is not Unicode is skipped: no setting of ours has one.
fn process_vars() -> HashMap<String, String> {
    std::env::vars_os()
        .filter_map(|(name, value)| Some((name.into_string().ok()?, value.to_string_lossy().into_owned())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_values_as_the_node_gateway_did() {
        let env = Env::from_pairs([("A", "yes"), ("B", ""), ("N", "12.5"), ("BAD", "x"), ("L", " a, ,b ")]);
        assert!(env.flag("A", false));
        assert!(env.flag("B", true), "an empty flag keeps the default");
        assert!(!env.flag("MISSING", false));
        assert_eq!(env.number("N", 1.0), 12.5);
        assert_eq!(env.number("BAD", 3.0), 3.0);
        assert_eq!(env.list("L", &[]), vec!["a", "b"]);
        assert_eq!(env.list("MISSING", &["x"]), vec!["x"]);
        assert_eq!(env.list("B", &["x"]), Vec::<String>::new(), "set but empty is an empty list");
        assert_eq!(env.get("B"), None);
        assert_eq!(env.raw("B"), Some(""));
    }

    #[test]
    fn the_first_env_file_wins_and_the_process_wins_over_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "FROM_FILE=data\nBOTH=data\n").unwrap();
        let vars = read_env_file(&dir.path().join(".env"));
        assert!(vars.contains(&("FROM_FILE".to_string(), "data".to_string())));
        assert!(read_env_file(&dir.path().join("missing.env")).is_empty());
    }
}
