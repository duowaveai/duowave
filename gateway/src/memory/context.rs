//! What the memory adds to the prompt (docs/plans/memory.md, «Что идёт в подсказку»): the «wake-up» — the most
//! important records the speaking character may see, capped by count and tokens — and the instructions for the
//! memory tools. The wake-up sits in the stable part of the prompt so a provider can cache it; the panel shows the
//! same set as «Глазами персонажа».

use chrono::{DateTime, Utc};

use super::text::{tokens, Lang};

/// About 30 records and 1000 tokens, as the plan says.
pub const WAKE_UP_ITEMS: usize = 30;
pub const WAKE_UP_TOKENS: usize = 1000;

/// Which layer a prompt line comes from: the panel colours them, the prompt does not care.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PromptLayer {
    Home,
    Personal,
}

/// One candidate for the wake-up.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptItem {
    pub id: String,
    pub text: String,
    pub pinned: bool,
    pub importance: i64,
    pub mentions: i64,
    pub updated_at: DateTime<Utc>,
    pub layer: PromptLayer,
}

/// The wake-up: pinned first, then by importance, freshness and how often it came up; stops at either cap.
pub fn pick_wake_up(mut items: Vec<PromptItem>, max_items: usize, max_tokens: usize) -> Vec<PromptItem> {
    items.sort_by(|a, b| {
        b.pinned
            .cmp(&a.pinned)
            .then(b.importance.cmp(&a.importance))
            .then(b.updated_at.cmp(&a.updated_at))
            .then(b.mentions.cmp(&a.mentions))
    });
    let mut picked = Vec::new();
    let mut spent = 0usize;
    for item in items {
        if picked.len() >= max_items {
            break;
        }
        let cost = tokens(&item.text) + 2;
        if spent + cost > max_tokens {
            continue;
        }
        spent += cost;
        picked.push(item);
    }
    picked
}

/// The tokens a wake-up takes in the prompt.
pub fn wake_up_tokens(items: &[PromptItem]) -> usize {
    items.iter().map(|item| tokens(&item.text) + 2).sum()
}

/// Which memory tools this answer offers: decides the instructions the model gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Writing {
    /// `remember` is offered (memory on, not paused, no untrusted text read).
    pub remember: bool,
    /// «Запоминать мимоходом»: the model may remember without being asked.
    pub in_passing: bool,
    /// `recall` and `forget` are offered.
    pub recall: bool,
}

/// The memory's block for the system prompt, in the household's language. Empty when there is nothing to say.
pub fn render(lang: Lang, items: &[PromptItem], writing: Writing) -> String {
    let mut out = String::new();
    if !items.is_empty() {
        out.push_str(match lang {
            Lang::En => "What you know for sure about this home (use it when it fits, don't recite it):\n",
            _ => "Что ты знаешь о доме наверняка (используй, когда к месту, не пересказывай без просьбы):\n",
        });
        for item in items {
            out.push_str("- ");
            out.push_str(item.text.trim().trim_end_matches('.'));
            out.push_str(".\n");
        }
    }
    let rules = match (lang, writing.remember, writing.in_passing) {
        (Lang::En, false, _) if writing.recall => Some(
            "Memory tools: recall looks deeper when the lines above are not enough; forget closes a fact when asked. \
             Memory is paused now: don't remember anything new.",
        ),
        (_, false, _) if writing.recall => Some(
            "Инструменты памяти: recall ищет глубже, когда строк выше мало; forget закрывает факт по просьбе. Сейчас \
             память на паузе: ничего нового не запоминай.",
        ),
        (_, false, _) => None,
        (Lang::En, true, true) => Some(
            "Memory tools: when someone tells you something worth keeping about the home and its people (who is who, \
             where, habits, dates, likes), call remember, even if not asked; if it replaces an old fact, say which in \
             `replaces`. Sensitive things (health, money, relationships, politics, faith) only when asked to remember. \
             forget closes a fact when asked; recall looks deeper when the lines above are not enough. Don't announce \
             that you remembered something unless you were asked to remember it.",
        ),
        (Lang::En, true, false) => Some(
            "Memory tools: call remember only when someone asks you to remember something; forget closes a fact when \
             asked; recall looks deeper when the lines above are not enough.",
        ),
        (_, true, true) => Some(
            "Инструменты памяти: если человек сообщает что-то, что стоит помнить о доме и людях (кто кем кому \
             приходится, кто где, привычки, даты, вкусы), вызови remember, даже без просьбы; если это заменяет старый \
             факт, укажи его в replaces. Чувствительное (здоровье, деньги, отношения, политика, вера) — только когда \
             просят запомнить. forget закрывает факт по просьбе; recall ищет глубже, когда строк выше мало. Не говори, \
             что запомнила, если тебя об этом не просили.",
        ),
        (_, true, false) => Some(
            "Инструменты памяти: remember — только когда человек просит что-то запомнить; forget закрывает факт по \
             просьбе; recall ищет глубже, когда строк выше мало.",
        ),
    };
    if let Some(rules) = rules {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(rules);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn item(id: &str, text: &str, pinned: bool, importance: i64, day: u32) -> PromptItem {
        PromptItem {
            id: id.into(),
            text: text.into(),
            pinned,
            importance,
            mentions: 0,
            updated_at: Utc.with_ymd_and_hms(2026, 11, day, 12, 0, 0).unwrap(),
            layer: PromptLayer::Home,
        }
    }

    #[test]
    fn picks_pinned_then_important_then_fresh() {
        let picked = pick_wake_up(
            vec![
                item("old", "Старый факт", false, 0, 1),
                item("fresh", "Свежий факт", false, 0, 12),
                item("pin", "У Маши аллергия на орехи", true, 0, 1),
                item("big", "Важный факт", false, 5, 2),
            ],
            WAKE_UP_ITEMS,
            WAKE_UP_TOKENS,
        );
        let ids: Vec<&str> = picked.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, vec!["pin", "big", "fresh", "old"]);
    }

    #[test]
    fn stops_at_the_caps() {
        let many: Vec<PromptItem> = (1..=28).map(|d| item(&d.to_string(), "Факт о доме", false, 0, d)).collect();
        assert_eq!(pick_wake_up(many.clone(), 5, WAKE_UP_TOKENS).len(), 5);
        let picked = pick_wake_up(many, WAKE_UP_ITEMS, 20);
        assert!(wake_up_tokens(&picked) <= 20);
        assert!(!picked.is_empty());
    }

    #[test]
    fn renders_in_the_home_language() {
        let items = vec![item("a", "У Маши аллергия на орехи.", true, 0, 1)];
        let ru = render(Lang::Ru, &items, Writing { remember: true, in_passing: true, recall: true });
        assert!(ru.starts_with("Что ты знаешь о доме наверняка"));
        assert!(ru.contains("- У Маши аллергия на орехи.\n"));
        assert!(ru.contains("даже без просьбы"));
        let en = render(Lang::En, &items, Writing { remember: true, in_passing: false, recall: true });
        assert!(en.starts_with("What you know for sure"));
        assert!(en.contains("only when someone asks"));
    }

    #[test]
    fn says_nothing_without_records_or_tools() {
        assert_eq!(render(Lang::Ru, &[], Writing { remember: false, in_passing: true, recall: false }), "");
        let only_rules = render(Lang::Ru, &[], Writing { remember: true, in_passing: false, recall: true });
        assert!(only_rules.starts_with("Инструменты памяти"));
        let paused = render(Lang::Ru, &[], Writing { remember: false, in_passing: false, recall: true });
        assert!(paused.contains("на паузе"));
    }
}
