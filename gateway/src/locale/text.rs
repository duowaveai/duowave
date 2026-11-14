//! The gateway's own words in the language of whoever reads them (docs/plans/i18n.md, «Тексты шлюза»).
//!
//! A text is written once, in Russian — the source language — through `tr!`:
//!
//! ```ignore
//! ApiError::bad_request(tr!("Нет текста."))
//! ApiError::bad_request(tr!("Фраза длиннее {max} знаков.", max = MAX_PHRASE))
//! ```
//!
//! Its translations are catalogs by the source text: `gateway/locales/<code>/<area>.json`, `{"Нет текста.": "No
//! text."}`, the same `{names}` in both. The language is the request's (`Accept-Language`), else the household's.
//! Russian — the source itself; a language without a catalog or a text without a translation — English; a request
//! with no language on a gateway with no household language (a script without the header) — the source, as before.
//! A test checks that every `tr!` of the code has its English.
//!
//! A new language: a folder `locales/<code>/` with the same files, and its line in `CATALOGS`.

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

/// The catalogs built in, by language: the files of each, parsed once.
const CATALOGS: &[(&str, &[&str])] = &[(
    "en",
    &[
        include_str!("../../locales/en/placement.json"),
        include_str!("../../locales/en/plugins.json"),
        include_str!("../../locales/en/services.json"),
    ],
)];

static HOUSEHOLD: RwLock<Option<String>> = RwLock::new(None);

/// The household's language as the texts read it: told at the start and on every change (`locale::routes`).
pub fn remember_household(language: Option<String>) {
    *HOUSEHOLD.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = language;
}

/// The household's language as last told; none set — none.
pub fn household_now() -> Option<String> {
    HOUSEHOLD.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
}

fn catalogs() -> &'static HashMap<&'static str, HashMap<String, String>> {
    static PARSED: OnceLock<HashMap<&'static str, HashMap<String, String>>> = OnceLock::new();
    PARSED.get_or_init(|| {
        CATALOGS
            .iter()
            .map(|(code, files)| {
                let mut all = HashMap::new();
                for file in *files {
                    match serde_json::from_str::<HashMap<String, String>>(file) {
                        Ok(texts) => all.extend(texts),
                        Err(error) => tracing::error!(target: "locale", "the {code} catalog does not read: {error}"),
                    }
                }
                (*code, all)
            })
            .collect()
    })
}

/// The language the gateway's words go out in now: the request's, else the household's; none known — the source.
pub fn language_now() -> Option<String> {
    super::of_request().or_else(household_now)
}

/// The text in `language`: the source for Russian, its catalog's translation, else the English one, else the source.
pub fn translated(source: &'static str, language: Option<&str>) -> &'static str {
    let Some(language) = language.filter(|code| *code != super::SOURCE) else { return source };
    let catalogs = catalogs();
    catalogs
        .get(language)
        .and_then(|catalog| catalog.get(source))
        .or_else(|| catalogs.get(super::FALLBACK).and_then(|catalog| catalog.get(source)))
        .map(|text| text.as_str())
        .unwrap_or(source)
}

/// A text that is not a literal (a field of a table): its translation in the language of now, else itself. Nothing is
/// kept: a text the catalogs do not know costs one lookup.
pub fn translate_str(text: &str) -> String {
    let Some(language) = language_now().filter(|code| code != super::SOURCE) else { return text.to_string() };
    let catalogs = catalogs();
    catalogs
        .get(language.as_str())
        .and_then(|catalog| catalog.get(text))
        .or_else(|| catalogs.get(super::FALLBACK).and_then(|catalog| catalog.get(text)))
        .cloned()
        .unwrap_or_else(|| text.to_string())
}

/// `{name}` in `text` given its value.
pub fn fill(text: &str, params: &[(&str, String)]) -> String {
    let mut out = text.to_string();
    for (name, value) in params {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

/// What `tr!` does: the text in the language of now, its `{names}` filled.
pub fn translate(source: &'static str, params: &[(&str, String)]) -> String {
    fill(translated(source, language_now().as_deref()), params)
}

/// The gateway's words in the reader's language: `tr!("Источник {name} не найден.", name = id)`.
#[macro_export]
macro_rules! tr {
    ($text:literal $(,)?) => {
        $crate::locale::text::translate($text, &[])
    };
    ($text:literal, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::locale::text::translate($text, &[$((stringify!($name), ($value).to_string())),+])
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `tr!` of the code has its English, with the same `{names}`.
    #[test]
    fn every_text_has_its_english_with_the_same_names() {
        let english = &catalogs()["en"];
        let call = regex::Regex::new(r#"(?:^|[^A-Za-z0-9_])tr!\(\s*"((?:[^"\\]|\\.)*)""#).unwrap();
        let names = regex::Regex::new(r"\{(\w+)\}").unwrap();
        let set =
            |text: &str| names.captures_iter(text).map(|c| c[1].to_string()).collect::<std::collections::BTreeSet<_>>();
        let mut missing = Vec::new();
        let mut stack = vec![std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/src"))];
        while let Some(path) = stack.pop() {
            for entry in std::fs::read_dir(&path).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|ext| ext != "rs") || path.ends_with("locale/text.rs") {
                    continue;
                }
                let code = std::fs::read_to_string(&path).unwrap();
                for found in call.captures_iter(&code) {
                    let source = found[1].replace("\\\"", "\"").replace("\\\\", "\\").replace("\\n", "\n");
                    match english.get(&source) {
                        None => missing.push(format!("{}: {source}", path.display())),
                        Some(text) if set(text) != set(&source) => {
                            missing.push(format!("{}: names differ: {source} → {text}", path.display()))
                        }
                        Some(_) => {}
                    }
                }
            }
        }
        assert!(missing.is_empty(), "texts without English:\n{}", missing.join("\n"));
    }

    #[test]
    fn russian_is_the_source_other_languages_fall_back_to_english() {
        assert_eq!(translated("нет такого текста", Some("en")), "нет такого текста");
        assert_eq!(translated("нет такого текста", None), "нет такого текста");
        assert_eq!(
            fill("Не больше {n} — {what}.", &[("n", "3".into()), ("what", "фраз".into())]),
            "Не больше 3 — фраз."
        );
    }
}
