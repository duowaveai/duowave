//! The memory's voice commands as our skill (docs/plans/memory.md, «Голос»): «Запомни …», «Забудь …», «Что ты
//! помнишь про …?», «Что ты обо мне знаешь?», «Не запоминай ничего до утра» and back — answered at once, without the
//! model. Wiping the memory, switching it off or reflection on are refused by voice: only the panel, with the right.
//! The phrases sit beside the built-in rules (`Router::add_rules`); a built-in rule of another capability wins.

use std::sync::Arc;

use futures::future::BoxFuture;
use serde_json::json;

use crate::core::events::Actor;
use crate::core::identity::principal::PrincipalKind;
use crate::memory::commands::{self, Command, PauseSpan};
use crate::memory::model::{Origin, Source};
use crate::memory::replies::{self, Speaker};
use crate::memory::text::{self, Lang};
use crate::memory::{MemoryError, MemoryService, Remember, Remembered};
use crate::skills::capabilities::{CapabilityName, Intent};
use crate::skills::quick_commands::QuickMatch;
use crate::skills::router::ExtraRules;
use crate::skills::skill::{intent, text_value, ExecutionContext, Outcome, OwnSkill};

/// How many facts «что ты помнишь про …» says.
const RECALL_SAID: usize = 5;

/// The phrases: whole commands only, from their first word.
pub struct MemoryRules;

impl ExtraRules for MemoryRules {
    fn matched(&self, normalized: &str) -> Option<QuickMatch> {
        let memory = CapabilityName::Memory;
        let intent = match commands::parse(normalized)? {
            Command::Remember(said) => intent(memory, "remember", vec![("text", text_value(Some(&said)))]),
            Command::Forget(what) => intent(memory, "forget", vec![("text", text_value(Some(&what)))]),
            Command::Recall(about) => intent(memory, "recall", vec![("text", text_value(Some(&about)))]),
            Command::AboutMe => Intent::new(memory, "about_me"),
            Command::Pause(span) => intent(memory, "pause", vec![("span", Some(json!(span_name(span))))]),
            Command::Resume => Intent::new(memory, "resume"),
            Command::PanelOnly => Intent::new(memory, "panel_only"),
        };
        Some(QuickMatch { intent, rule: "memory" })
    }
}

fn span_name(span: PauseSpan) -> &'static str {
    match span {
        PauseSpan::Hour => "hour",
        PauseSpan::Morning => "morning",
        PauseSpan::UntilResumed => "manual",
    }
}

fn span_of(name: Option<&str>) -> PauseSpan {
    match name {
        Some("hour") => PauseSpan::Hour,
        Some("manual") => PauseSpan::UntilResumed,
        _ => PauseSpan::Morning,
    }
}

pub struct MemorySkill {
    memory: Arc<MemoryService>,
}

impl MemorySkill {
    pub fn new(memory: Arc<MemoryService>) -> MemorySkill {
        MemorySkill { memory }
    }

    fn speaker(&self, said: &str) -> Speaker {
        let lang = match text::lang_of(said) {
            Lang::En => Lang::En,
            _ => Lang::Ru,
        };
        Speaker { lang, feminine: self.memory.answering().feminine }
    }

    /// The command's words as said: from the request's own text when it still holds them, with its capitals.
    fn said(intent: &Intent, context: &ExecutionContext) -> String {
        let tail = intent.params.get("text").and_then(|value| value.as_str()).unwrap_or_default();
        let original = text::original_tail(&context.text, tail);
        if text::normalize(&original) == text::normalize(tail) {
            original
        } else {
            tail.to_string()
        }
    }

    fn run(&self, intent: &Intent, context: &ExecutionContext) -> anyhow::Result<Outcome> {
        let speaker = self.speaker(&context.text);
        let memory = &self.memory;
        let household = Actor { kind: PrincipalKind::Household, id: "household".into(), name: "Дом".into() };
        let refused = |error: MemoryError| match error {
            MemoryError::Off => Ok(Outcome::speech(replies::memory_off(speaker))),
            MemoryError::Paused => Ok(Outcome::speech(replies::paused_now(
                speaker,
                memory.paused_until_on_clock(context.timezone.as_deref()),
            ))),
            MemoryError::Full | MemoryError::Empty | MemoryError::TooLong => Ok(Outcome::speech(error.to_string())),
            other => Err(anyhow::anyhow!("{other}")),
        };
        match intent.action.as_str() {
            "remember" => {
                let said = Self::said(intent, context);
                let character = memory.answering().id;
                let origin = Origin::station_voice(Some(&context.device_tag), Some(&character));
                match memory.remember(Remember { fact: said, ..Remember::default() }, origin, Source::Voice, None) {
                    Ok(Remembered::Added { fact, .. } | Remembered::Known(fact)) => Ok(Outcome {
                        speech: Some(replies::remembered(speaker)),
                        done: Some(format!("запомнено: {}", fact.text)),
                        ..Outcome::default()
                    }),
                    Err(error) => refused(error),
                }
            }
            "forget" => {
                let what = Self::said(intent, context);
                match memory.forget(&what, &memory.viewer(), None) {
                    Ok(Some(fact)) => Ok(Outcome {
                        speech: Some(replies::forgot(speaker, &fact.text)),
                        done: Some(format!("забыто: {}", fact.text)),
                        ..Outcome::default()
                    }),
                    Ok(None) => Ok(Outcome::speech(replies::nothing_to_forget(speaker))),
                    Err(error) => refused(error),
                }
            }
            "recall" => {
                let about = Self::said(intent, context);
                if !memory.enabled() {
                    return refused(MemoryError::Off);
                }
                let found = memory.about(&about, &memory.viewer(), RECALL_SAID).map_err(|e| anyhow::anyhow!("{e}"))?;
                let facts: Vec<String> = found.into_iter().map(|view| view.fact.text).collect();
                Ok(Outcome::speech(replies::recall(speaker, &facts)))
            }
            // A voice at a station is anyone in the room: the memory does not know who asks yet.
            "about_me" => Ok(Outcome::speech(replies::who_are_you(speaker))),
            "pause" => {
                let span = span_of(intent.params.get("span").and_then(|value| value.as_str()));
                let (_, until) = memory
                    .pause(span, context.timezone.as_deref(), Some(household))
                    .map_err(|e| anyhow::anyhow!("{}", e.message))?;
                Ok(Outcome::speech(replies::paused(speaker, span, until)))
            }
            "resume" => {
                memory.resume(Some(household)).map_err(|e| anyhow::anyhow!("{}", e.message))?;
                Ok(Outcome::speech(replies::resumed(speaker)))
            }
            "panel_only" => Ok(Outcome::speech(replies::panel_only(speaker))),
            other => anyhow::bail!("memory: unknown action {other}"),
        }
    }
}

impl OwnSkill for MemorySkill {
    fn capability(&self) -> CapabilityName {
        CapabilityName::Memory
    }

    fn verified(&self, _intent: &Intent) -> bool {
        // Nothing of the station's is needed: the memory is ours.
        true
    }

    fn execute<'a>(
        &'a self,
        intent: &'a Intent,
        context: &'a ExecutionContext,
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async move { self.run(intent, context) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Env};
    use crate::core::events::EventBus;
    use crate::dialog::PersonaService;
    use crate::memory::model::Viewer;
    use crate::settings::SettingsService;
    use crate::storage::Db;
    use chrono::Utc;

    fn skill() -> MemorySkill {
        let config = Config::for_tests(&Env::default()).dialog;
        let db = Arc::new(Db::memory());
        let events = EventBus::new();
        let settings = Arc::new(SettingsService::new(db.clone(), events.clone()));
        let personas = Arc::new(PersonaService::new(&config, db.clone(), settings.clone(), events.clone()).unwrap());
        MemorySkill::new(Arc::new(MemoryService::new(db, settings, events, personas)))
    }

    async fn say(skill: &MemorySkill, said: &str) -> Outcome {
        let normalized = text::normalize(said);
        let matched = MemoryRules.matched(&normalized).unwrap_or_else(|| panic!("a memory command: {said}"));
        let mut context = ExecutionContext::new("kitchen", Utc::now());
        context.text = said.to_string();
        context.timezone = Some("Europe/Moscow".into());
        skill.execute(&matched.intent, &context).await.unwrap()
    }

    #[tokio::test]
    async fn remembers_recalls_and_forgets_by_voice() {
        let skill = skill();
        let said = say(&skill, "Запомни: у Маши аллергия на орехи").await;
        assert_eq!(said.speech.as_deref(), Some("Запомнила."));
        assert_eq!(said.done.as_deref(), Some("запомнено: У Маши аллергия на орехи"));
        let recalled = say(&skill, "Что ты помнишь про орехи?").await;
        assert_eq!(recalled.speech.as_deref(), Some("Вот что я помню. У Маши аллергия на орехи."));
        let forgot = say(&skill, "Забудь про орехи").await;
        assert_eq!(forgot.speech.as_deref(), Some("Хорошо, забыла: У Маши аллергия на орехи."));
        assert_eq!(say(&skill, "Забудь про орехи").await.speech.as_deref(), Some("Такого я не помню."));
        let stored = skill.memory.recall("орехи", &Viewer::default(), 5).unwrap();
        assert!(stored.is_empty());
    }

    #[tokio::test]
    async fn pauses_until_morning_and_refuses_the_dangerous() {
        let skill = skill();
        assert_eq!(
            say(&skill, "Не запоминай ничего до утра").await.speech.as_deref(),
            Some("Хорошо. До 7:00 ничего не запоминаю.")
        );
        assert_eq!(
            say(&skill, "Запомни, что Петя ходит на шахматы").await.speech.as_deref(),
            Some("Сейчас я ничего не запоминаю: память на паузе до 7:00.")
        );
        assert_eq!(say(&skill, "Можешь снова запоминать").await.speech.as_deref(), Some("Хорошо, снова запоминаю."));
        assert_eq!(say(&skill, "Запомни, что Петя ходит на шахматы").await.speech.as_deref(), Some("Запомнила."));
        assert_eq!(
            say(&skill, "Сотри всю память").await.speech.as_deref(),
            Some("Это можно сделать только в пульте: Память → Настройки.")
        );
        assert!(skill.memory.enabled(), "nothing switched by voice");
        assert_eq!(
            say(&skill, "Что ты обо мне знаешь?").await.speech.as_deref(),
            Some("Я пока не различаю людей по голосу. Спросите про кого-нибудь по имени.")
        );
    }

    #[tokio::test]
    async fn answers_in_english_to_english() {
        let skill = skill();
        assert_eq!(
            say(&skill, "Remember that Masha is allergic to nuts").await.speech.as_deref(),
            Some("Got it, I'll remember.")
        );
        assert_eq!(
            say(&skill, "Forget about the nuts").await.speech.as_deref(),
            Some("Okay, I've forgotten: Masha is allergic to nuts.")
        );
    }
}
