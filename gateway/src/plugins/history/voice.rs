//! The history by voice (docs/plans/package-history.md, «Голосом»): the router's rule beside the plugins' phrases.
//!
//! - «верни погоду как было вчера / позавчера / утром / до последнего изменения», «откати погоду» — the code alone
//!   (decision 22); when the kind of the data changed on the way, the station asks «вернуть и данные?».
//! - «отмени последнее изменение в погоде» — to the version before the newest.
//! - «верни список покупок на вчера» — the data alone; the station asks first: what came after goes (and the step back
//!   can be undone — decision 21: a spoken «да» is enough).
//! - «верни как было» right after a step back — that step back undone.
//! - «что поменялось в погоде?» — the last records in a few words.
//!
//! Rights the running version does not have are never voice's: the admin's «ок» in the panel (decision 10).

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use futures::future::BoxFuture;
use regex::Regex;
use serde_json::json;

use super::restore::local_time;
use super::{Author, History, Record, RecordKind, What, Who, INSTALLED};
use crate::skills::capabilities::{CapabilityName, Intent};
use crate::skills::quick_commands::QuickMatch;
use crate::skills::router::{ExtraRules, EXECUTOR};
use crate::skills::skill::{ContinuationRequest, ExecutionContext, Outcome};
use crate::skills::text::normalize;

/// The rule's name in the journal.
const RULE: &str = "history:voice";
/// How long the station waits for «да» or «нет».
const CONFIRM_MS: u64 = 60_000;
/// «Верни как было» undoes a step back made this recently.
const UNDO_MS: i64 = 15 * 60_000;

static RESTORE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        "^(?:верни|откати|восстанови) (?P<app>.+?) (?:как (?:было|была|были|раньше)|к тому что было) (?P<when>вчера|позавчера|сегодня утром|утром|до последнего изменения|до последней правки|до обновления)$",
    )
    .expect("a valid pattern")
});
static ROLLBACK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new("^(?:откати (?P<a>.+)|верни прежнюю версию (?P<b>.+)|отмени последнее изменение (?:в|у) (?P<c>.+))$")
        .expect("a valid pattern")
});
static DATA: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new("^верни (?:данные )?(?P<app>.+?) на (?P<when>вчера|позавчера|утро|сегодняшнее утро)$")
        .expect("a valid pattern")
});
static CHANGED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new("^что (?:поменялось|изменилось|нового) (?:в|у) (?P<app>.+)$").expect("a valid pattern")
});
static UNDO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("^(?:верни как было|отмени возврат|верни обратно)$").expect("a valid pattern"));
static YES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("^(?:да|давай|верни|возвращай|конечно|ага|да верни)$").expect("a valid pattern"));
static NO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new("^(?:нет|не надо|не нужно|отмена|отмени|стоп|не возвращай)$").expect("a valid pattern")
});

/// A step back waiting for «да».
#[derive(Debug, Clone)]
struct Pending {
    app: String,
    title: String,
    record: String,
    /// «да» does this, «нет» that (None — nothing).
    yes: What,
    no: Option<What>,
}

pub struct VoiceHistory {
    history: Arc<History>,
    pending: Mutex<HashMap<String, Pending>>,
}

/// Two words alike enough for Russian endings: «погоду» and «погода».
fn alike(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let common = a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count();
    let longer = a.chars().count().max(b.chars().count());
    common >= 3 && common + 2 >= longer
}

/// The installed app named by these words: (id, title).
fn find_app(history: &History, words: &str) -> Option<(String, String)> {
    let host = history.host()?;
    let said: Vec<&str> = words.split(' ').filter(|word| !word.is_empty()).collect();
    let mut best: Option<(usize, String, String)> = None;
    for package in host.visible_all() {
        let title = package.manifest.title.clone();
        let normalized = normalize(&title);
        let title_words: Vec<&str> = normalized.split(' ').filter(|word| word.chars().count() > 1).collect();
        if title_words.is_empty() {
            continue;
        }
        let matched = title_words.iter().filter(|word| said.iter().any(|said| alike(said, word))).count();
        let whole = matched == title_words.len() || said.iter().any(|said| *said == package.manifest.id);
        if whole && best.as_ref().is_none_or(|(score, _, _)| title_words.len() > *score) {
            best = Some((title_words.len(), package.manifest.id.clone(), title));
        }
    }
    best.map(|(_, id, title)| (id, title))
}

/// The moment a word of time means, in ms: «вчера» — the start of today, and so on.
fn cutoff(when: &str) -> Option<i64> {
    use chrono::{Local, TimeZone};
    let today = Local::now().date_naive();
    let at = |date: chrono::NaiveDate, hour: u32| {
        Local.from_local_datetime(&date.and_hms_opt(hour, 0, 0)?).earliest().map(|at| at.timestamp_millis())
    };
    match when {
        "вчера" => at(today, 0),
        "позавчера" => at(today.pred_opt()?, 0),
        "утром" | "сегодня утром" | "утро" | "сегодняшнее утро" => at(today, 12),
        _ => None,
    }
}

fn ms(record: &Record) -> i64 {
    chrono::DateTime::parse_from_rfc3339(&record.at).map(|at| at.timestamp_millis()).unwrap_or(0)
}

fn is_version(record: &Record) -> bool {
    record.kind.installs() || (record.kind == RecordKind::Restore && record.what.is_some_and(What::code))
}

impl VoiceHistory {
    pub fn new(history: Arc<History>) -> Arc<VoiceHistory> {
        Arc::new(VoiceHistory { history, pending: Mutex::new(HashMap::new()) })
    }

    fn pending(&self) -> std::sync::MutexGuard<'_, HashMap<String, Pending>> {
        self.pending.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Whether the intent is the history's: its rule, or the answer of a device it asked.
    pub fn handles(&self, intent: &Intent, device: &str) -> bool {
        intent.action == "history" || (intent.action == "continue" && self.pending().contains_key(device))
    }

    pub fn execute<'a>(
        &'a self,
        intent: &'a Intent,
        context: &'a ExecutionContext,
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async move {
            let device = context.device_tag.clone();
            if intent.action == "continue" {
                let text = intent.params.get("text").and_then(|text| text.as_str()).unwrap_or_default();
                return Ok(self.answer(&device, &normalize(text)).await);
            }
            let param = |key: &str| intent.params.get(key).and_then(|value| value.as_str()).unwrap_or_default();
            Ok(match param("kind") {
                "restore" => self.restore_code(&device, param("app"), param("when")).await,
                "data" => self.restore_data(&device, param("app"), param("when")),
                "changed" => self.changed(param("app")),
                "undo" => self.undo().await,
                _ => Outcome::speech("Не поняла, что вернуть."),
            })
        })
    }

    /// The version to come back to: before the moment, or the one before the running one.
    fn version_before(&self, app: &str, when: &str) -> Option<(String, Record)> {
        let line = self.history.line(app);
        let installed = self.history.head(app, INSTALLED).map(|(_, record)| record.tree);
        let versions: Vec<&(String, Record)> = line.iter().filter(|(_, record)| is_version(record)).collect();
        match cutoff(when) {
            Some(cut) => versions.iter().rev().find(|(_, record)| ms(record) < cut).map(|found| (*found).clone()),
            // «До последнего изменения»: the newest version with other files than the running one.
            None => versions
                .iter()
                .rev()
                .find(|(_, record)| Some(&record.tree) != installed.as_ref())
                .map(|found| (*found).clone()),
        }
    }

    async fn restore_code(&self, device: &str, words: &str, when: &str) -> Outcome {
        let Some((app, title)) = find_app(&self.history, words) else {
            return Outcome::speech(format!("Не нашла приложение «{words}»."));
        };
        let Some((record, found)) = self.version_before(&app, when) else {
            return Outcome::speech(format!("В истории «{title}» нет версии раньше этой."));
        };
        if self.history.head(&app, INSTALLED).is_some_and(|(_, head)| head.tree == found.tree) {
            return Outcome::speech(format!("«{title}» и так такое, как было."));
        }
        let plan = match self.history.restore_plan(&app, &record, What::Code) {
            Ok(plan) => plan,
            Err(error) => return Outcome::speech(error.message),
        };
        if plan.needs_approval {
            return Outcome::speech(format!(
                "Той версии «{title}» нужны права, которых нет у этой. Подтвердите в пульте, в истории приложения."
            ));
        }
        if plan.data_version_changed {
            self.pending().insert(
                device.to_string(),
                Pending { app, title: title.clone(), record, yes: What::CodeData, no: Some(What::Code) },
            );
            return asking(format!(
                "Вид данных «{title}» с тех пор менялся. Вернуть и данные той поры? Скажите «да» или «нет»."
            ));
        }
        self.run(&app, &title, &record, What::Code).await
    }

    fn restore_data(&self, device: &str, words: &str, when: &str) -> Outcome {
        let Some((app, title)) = find_app(&self.history, words) else {
            return Outcome::speech(format!("Не нашла приложение «{words}»."));
        };
        let Some(cut) = cutoff(when) else {
            return Outcome::speech("Не поняла, на когда вернуть.");
        };
        let found =
            self.history.line(&app).into_iter().rev().find(|(_, record)| record.data.is_some() && ms(record) < cut);
        let Some((record, snapshot)) = found else {
            return Outcome::speech(format!("Снимка данных «{title}» той поры в истории нет."));
        };
        self.pending()
            .insert(device.to_string(), Pending { app, title: title.clone(), record, yes: What::Data, no: None });
        asking(format!(
            "Вернуть данные «{title}», как были {}? Что появилось потом, пропадёт — но этот возврат можно отменить. Скажите «да» или «нет».",
            local_time(&snapshot.at)
        ))
    }

    async fn answer(&self, device: &str, text: &str) -> Outcome {
        let Some(pending) = self.pending().remove(device) else {
            return Outcome::speech("Возвращать нечего.");
        };
        if YES.is_match(text) {
            return self.run(&pending.app, &pending.title, &pending.record, pending.yes).await;
        }
        if NO.is_match(text) {
            return match pending.no {
                Some(what) => self.run(&pending.app, &pending.title, &pending.record, what).await,
                None => Outcome::speech("Хорошо, не трогаю."),
            };
        }
        self.pending().insert(device.to_string(), pending);
        asking("Скажите «да» или «нет».".into())
    }

    async fn run(&self, app: &str, title: &str, record: &str, what: What) -> Outcome {
        let by = Author { who: Who::Voice, name: None, via: Some("voice".into()) };
        match self.history.restore_installed(app, record, what, false, by, None).await {
            Ok(report) if report.restored => Outcome::speech(match what {
                What::Data => format!("Данные «{title}» вернулись. Передумаете — скажите «верни как было»."),
                What::CodeData => {
                    format!("«{title}» вернулось вместе с данными. Передумаете — скажите «верни как было».")
                }
                What::Code => format!("Вернула прежнюю версию «{title}». Передумаете — скажите «верни как было»."),
            }),
            Ok(_) => Outcome::speech("Нужно подтверждение в пульте: старой версии нужны новые права."),
            Err(error) => Outcome::speech(format!("Не получилось: {}", error.message)),
        }
    }

    /// The newest step back of the last minutes: its app and record.
    fn recent_restore(&self) -> Option<(String, Record)> {
        let now = chrono::Utc::now().timestamp_millis();
        let mut newest: Option<(String, Record)> = None;
        for app in self.history.store().apps() {
            let Some((_, record)) = self.history.head(&app, INSTALLED) else { continue };
            if record.kind == RecordKind::Restore
                && now - ms(&record) < UNDO_MS
                && newest.as_ref().is_none_or(|(_, other)| ms(&record) > ms(other))
            {
                newest = Some((app, record));
            }
        }
        newest
    }

    /// «Верни как было»: the newest step back of the last minutes, undone.
    async fn undo(&self) -> Outcome {
        let Some((app, record)) = self.recent_restore() else {
            return Outcome::speech("Недавних возвратов нет — возвращать нечего.");
        };
        let Some(before) = record.parents.first().cloned() else {
            return Outcome::speech("Что было до возврата, в истории уже нет.");
        };
        let title = self
            .history
            .host()
            .and_then(|host| host.installed(&app))
            .map(|package| package.manifest.title.clone())
            .unwrap_or_else(|| app.clone());
        self.run(&app, &title, &before, record.what.unwrap_or(What::Code)).await
    }

    fn changed(&self, words: &str) -> Outcome {
        let Some((app, title)) = find_app(&self.history, words) else {
            return Outcome::speech(format!("Не нашла приложение «{words}»."));
        };
        let feed = self.history.feed(&app, None, 6);
        let lines: Vec<String> = feed
            .records
            .iter()
            .filter(|view| view.record.kind != RecordKind::Data)
            .take(3)
            .map(|view| {
                let record = &view.record;
                let what = match (&record.asked, record.kind) {
                    (Some(asked), _) => format!("просили «{asked}»"),
                    (None, RecordKind::Install) => {
                        format!("поставлена версия {}", record.version.clone().unwrap_or_default())
                    }
                    (None, RecordKind::Catalog) => "обновление из каталога".into(),
                    (None, RecordKind::Restore) => "возврат к прежней версии".into(),
                    (None, RecordKind::Merge) => "объединение версий".into(),
                    (None, _) => record.note.clone().unwrap_or_else(|| "правка".into()),
                };
                format!("{} — {what}", local_time(&record.at))
            })
            .collect();
        if lines.is_empty() {
            return Outcome::speech(format!("В истории «{title}» пока ничего нет."));
        }
        Outcome::speech(format!("В «{title}»: {}.", lines.join("; ")))
    }
}

fn asking(text: String) -> Outcome {
    Outcome {
        speech: Some(text),
        listen: true,
        continuation: Some(ContinuationRequest { capability: CapabilityName::Plugins, ttl_ms: CONFIRM_MS }),
        ..Outcome::default()
    }
}

/// The history's phrases for the router.
impl ExtraRules for VoiceHistory {
    fn matched(&self, normalized: &str) -> Option<QuickMatch> {
        let mut intent = Intent::new(CapabilityName::Plugins, "history");
        intent.params.insert(EXECUTOR.into(), json!(CapabilityName::Plugins.as_str()));
        let mut set = |kind: &str, app: Option<&str>, when: Option<&str>| {
            intent.params.insert("kind".into(), json!(kind));
            if let Some(app) = app {
                intent.params.insert("app".into(), json!(app));
            }
            if let Some(when) = when {
                intent.params.insert("when".into(), json!(when));
            }
        };
        // Only what names an installed app (or undoes a step back just made) is the history's: anything else goes on
        // to the other rules and to Alice.
        let known = |app: Option<&str>| app.is_some_and(|app| find_app(&self.history, app).is_some());
        if let Some(found) = RESTORE.captures(normalized) {
            let app = found.name("app").map(|m| m.as_str());
            if !known(app) {
                return None;
            }
            set("restore", app, found.name("when").map(|m| m.as_str()));
        } else if let Some(found) = ROLLBACK.captures(normalized) {
            let app = found.name("a").or(found.name("b")).or(found.name("c")).map(|m| m.as_str());
            if !known(app) {
                return None;
            }
            set("restore", app, Some("до последнего изменения"));
        } else if let Some(found) = DATA.captures(normalized) {
            let app = found.name("app").map(|m| m.as_str());
            if !known(app) {
                return None;
            }
            set("data", app, found.name("when").map(|m| m.as_str()));
        } else if let Some(found) = CHANGED.captures(normalized) {
            let app = found.name("app").map(|m| m.as_str());
            if !known(app) {
                return None;
            }
            set("changed", app, None);
        } else if UNDO.is_match(normalized) && self.recent_restore().is_some() {
            set("undo", None, None);
        } else {
            return None;
        }
        Some(QuickMatch { intent, rule: RULE })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hears_a_step_back_by_its_words_and_the_apps_name_with_its_ending() {
        let found = RESTORE.captures("верни погоду как было вчера").unwrap();
        assert_eq!((&found["app"], &found["when"]), ("погоду", "вчера"));
        let found = RESTORE.captures("откати список покупок как было до последнего изменения").unwrap();
        assert_eq!(&found["app"], "список покупок");
        let found = ROLLBACK.captures("отмени последнее изменение в погоде").unwrap();
        assert_eq!(found.name("c").map(|m| m.as_str()), Some("погоде"));
        let found = DATA.captures("верни список покупок на вчера").unwrap();
        assert_eq!((&found["app"], &found["when"]), ("список покупок", "вчера"));
        assert!(CHANGED.is_match("что поменялось в погоде") && UNDO.is_match("верни как было"));
        assert!(!RESTORE.is_match("верни погоду") && !DATA.is_match("верни мне музыку"));
        assert!(alike("погоду", "погода") && alike("погоде", "погода") && alike("покупок", "покупок"));
        assert!(!alike("погода", "пол") && !alike("радио", "рассказ"));
        assert!(YES.is_match("да") && NO.is_match("не надо"));
        assert!(cutoff("вчера").is_some() && cutoff("до последнего изменения").is_none());
    }
}
