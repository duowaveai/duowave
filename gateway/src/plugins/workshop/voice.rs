//! The workshop by voice (docs/plans/plugins.md, «Голосом: режим мастерской», stage 6а). «Алиса, давай сделаем навык»
//! (or «…тему», «режим мастерской») starts a draft and the conversation goes on in the workshop: the station listens on
//! without the wake word (the router's continuation, as a game's), each phrase goes to the workshop's agent, and its
//! answer is said. «Хватит», «выйди из мастерской» or a few minutes of silence end it.
//!
//! Voice is good at saying what the plugin should do, answering the agent's questions and «скажи: включи радио джаз»;
//! code is neither dictated nor read aloud — the panel shows the draft, its checks and its journal live meanwhile.
//! Installing and new rights are never voice's: a voice can be faked, so the agent here has no `install` at all and
//! says to press the button in the panel. The model takes 5–30 s: the station says «Работаю над этим» first.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use futures::future::BoxFuture;
use regex::Regex;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::agent;
use super::Workshop;
use crate::config::ModelProfile;
use crate::core::events::{EventBus, GatewayEvent};
use crate::dialog::llm::ToolCall;
use crate::dialog::{DialogContext, DialogService, Filler, ToolBox, ToolOutcome};
use crate::skills::capabilities::{CapabilityName, Intent};
use crate::skills::quick_commands::QuickMatch;
use crate::skills::router::{ExtraRules, EXECUTOR};
use crate::skills::skill::{ContinuationRequest, ExecutionContext, Outcome};
use crate::skills::text::normalize;

/// The workshop by voice lasts this long after the last phrase.
const VOICE_MS: u64 = 5 * 60_000;
/// The agent's rounds in one spoken answer: fewer than in the panel, the person waits by the station.
const ROUNDS: usize = 8;
const BUDGET_MS: u64 = 90_000;
/// The rule's name in the journal.
const RULE: &str = "workshop:voice";

static START: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        "^(давай (сделаем|напишем|создадим) (новый )?(навык|плагин|тему)|режим мастерской|открой мастерскую|включи мастерскую|мастерская)$",
    )
    .expect("a valid pattern")
});
static STOP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new("^(хватит|стоп|выход|выйти|закончим|закончить|давай закончим|выйди из мастерской|закрой мастерскую|всё хватит)$")
        .expect("a valid pattern")
});

/// What the voice gives the agent besides the skill.
const VOICE_RULES: &str = "## Голосовой режим

Ты говоришь с человеком через колонку, всё, что ты пишешь, будет озвучено. Отвечай одной-двумя короткими фразами, без \
списков, кода, разметки и ссылок. Код не диктуй и не читай: он в черновике, человек видит его в пульте, в «Мастерской». \
Уточняй по одному вопросу за раз. Поставить плагин на колонку и дать ему права голосом нельзя — голос можно подделать: \
когда черновик готов и проверен, скажи «Готово, поставить можно в пульте, в мастерской». Попроси проверить фразой: \
«скажите: включи радио джаз» — и прогони её сам (try_phrase).";

/// A device in the workshop: its draft.
#[derive(Debug, Clone)]
struct Session {
    draft: String,
    /// The person's turns so far: every few, the way out is said again.
    turns: u32,
}

/// How often the way out («хватит») is said again: a long talk forgets it.
const REMIND_EVERY: u32 = 4;
const REMINDER: &str = "Напомню: чтобы выйти из мастерской, скажите «хватит».";

pub struct VoiceWorkshop {
    workshop: Arc<Workshop>,
    dialog: Arc<DialogService>,
    events: EventBus,
    sessions: Mutex<HashMap<String, Session>>,
}

impl VoiceWorkshop {
    pub fn new(workshop: Arc<Workshop>, dialog: Arc<DialogService>, events: EventBus) -> Arc<VoiceWorkshop> {
        Arc::new(VoiceWorkshop { workshop, dialog, events, sessions: Mutex::new(HashMap::new()) })
    }

    fn sessions(&self) -> std::sync::MutexGuard<'_, HashMap<String, Session>> {
        self.sessions.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Whether the intent is the workshop's: its start, or a follow-up of a device in it.
    pub fn handles(&self, intent: &Intent, device: &str) -> bool {
        intent.action == "workshop" || (intent.action == "continue" && self.sessions().contains_key(device))
    }

    pub fn execute<'a>(
        &'a self,
        intent: &'a Intent,
        context: &'a ExecutionContext,
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async move {
            if intent.action == "workshop" {
                return Ok(self.start(intent, context));
            }
            let text = intent.params.get("text").and_then(|text| text.as_str()).unwrap_or_default().to_string();
            Ok(self.follow(&context.device_tag, &text, context.cancel.clone()))
        })
    }

    /// A free id for a new draft: `golos-1`, `golos-2`, …
    fn free_id(&self, prefix: &str) -> String {
        (1..)
            .map(|n| format!("{prefix}-{n}"))
            .find(|id| !self.workshop.dir().join(id).exists() && self.workshop.host().installed(id).is_none())
            .unwrap_or_else(|| prefix.to_string())
    }

    fn start(&self, intent: &Intent, context: &ExecutionContext) -> Outcome {
        if !self.dialog.enabled() {
            return Outcome::speech("Мастерская не работает: в «Мозге» не выбрана модель.");
        }
        let theme = intent.params.get("kind").and_then(|kind| kind.as_str()) == Some("theme");
        let (prefix, kind, title, greeting) = if theme {
            ("golos-tema", "theme", "Тема голосом", "Делаем тему. Какой она будет: тёмная или светлая, какие цвета?")
        } else {
            ("golos", "plugin", "Навык голосом", "Делаем навык. Что он должен уметь?")
        };
        let id = self.free_id(prefix);
        if let Err(error) = self.workshop.create(&id, Some(kind), Some(title), None) {
            return Outcome::speech(format!("Не получилось начать: {}", error.message));
        }
        self.sessions().insert(context.device_tag.clone(), Session { draft: id.clone(), turns: 0 });
        self.dialog.forget(&key(&context.device_tag));
        tracing::info!(target: "plugins", "workshop by voice on {}: draft {id}", context.device_tag);
        self.events.emit(GatewayEvent::WorkshopChanged { draft: id.clone(), change: "voice.started".into() });
        Outcome {
            speech: Some(format!(
                "{greeting} Черновик «{id}» открыт в пульте, в мастерской. Чтобы выйти, скажите «хватит»."
            )),
            listen: true,
            continuation: Some(ContinuationRequest { capability: CapabilityName::Plugins, ttl_ms: VOICE_MS }),
            ..Outcome::default()
        }
    }

    fn follow(&self, device: &str, text: &str, cancel: CancellationToken) -> Outcome {
        let Some(session) = self.sessions().get(device).cloned() else {
            return Outcome::speech("Мастерская закрыта. Скажите: давай сделаем навык.");
        };
        if STOP.is_match(&normalize(text)) {
            self.sessions().remove(device);
            self.events
                .emit(GatewayEvent::WorkshopChanged { draft: session.draft.clone(), change: "voice.ended".into() });
            return Outcome::speech(format!("Выхожу из мастерской. Черновик «{}» остался в пульте.", session.draft));
        }
        let workshop = self.workshop.clone();
        let draft = session.draft.clone();
        let events = self.events.clone();
        let mut toolbox = ToolBox::new(
            agent::tools(false).into_iter().filter(|tool| tool.name != "install").collect(),
            move |call: ToolCall, _cancel: CancellationToken| {
                let (workshop, draft, events) = (workshop.clone(), draft.clone(), events.clone());
                async move {
                    let (content, failed) = agent::run_tool(&workshop, &draft, &call.name, &call.input).await;
                    if call.name == "write_file" || call.name == "delete_file" {
                        events.emit(GatewayEvent::WorkshopChanged {
                            draft: draft.clone(),
                            change: "voice.edited".into(),
                        });
                    }
                    Ok(if failed { ToolOutcome::error(content) } else { ToolOutcome::from(content) })
                }
            },
        );
        toolbox.rounds = Some(ROUNDS);
        toolbox.budget_ms = Some(BUDGET_MS);
        toolbox.filler = Some(Filler { phrase: "Работаю над этим.".into(), after_ms: 1500 });
        let context = DialogContext {
            profile: Some(ModelProfile::Smart),
            system: Some(format!("{}\n\n{VOICE_RULES}", agent::instructions(&self.workshop, &session.draft))),
            ..DialogContext::default()
        };
        let phrases = self.dialog.answer(&key(device), text, context, cancel, Some(toolbox));
        let turns = self.sessions().get_mut(device).map(|session| {
            session.turns += 1;
            session.turns
        });
        let phrases = match turns {
            Some(turns) if turns % REMIND_EVERY == 0 => {
                use futures::StreamExt;
                phrases.chain(futures::stream::once(async { Ok(REMINDER.to_string()) })).boxed()
            }
            _ => phrases,
        };
        Outcome {
            phrases: Some(phrases),
            listen: true,
            continuation: Some(ContinuationRequest { capability: CapabilityName::Plugins, ttl_ms: VOICE_MS }),
            ..Outcome::default()
        }
    }
}

fn key(device: &str) -> String {
    format!("workshop:voice:{device}")
}

/// «Давай сделаем навык»: the router's rule for the workshop's start, beside the plugins' phrases.
impl ExtraRules for VoiceWorkshop {
    fn matched(&self, normalized: &str) -> Option<QuickMatch> {
        let found = START.captures(normalized)?;
        let theme = found.get(4).is_some_and(|word| word.as_str() == "тему");
        let mut intent = Intent::new(CapabilityName::Plugins, "workshop");
        intent.params.insert(EXECUTOR.into(), json!(CapabilityName::Plugins.as_str()));
        intent.params.insert("kind".into(), json!(if theme { "theme" } else { "plugin" }));
        Some(QuickMatch { intent, rule: RULE })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hears_the_workshops_start_and_its_end() {
        assert!(START.is_match("давай сделаем навык"));
        assert!(START.is_match("давай напишем новый плагин"));
        assert!(START.is_match("режим мастерской"));
        assert!(!START.is_match("давай сделаем уроки"));
        let found = START.captures("давай сделаем тему").unwrap();
        assert_eq!(found.get(4).map(|word| word.as_str()), Some("тему"));
        assert!(STOP.is_match("выйди из мастерской") && STOP.is_match("хватит"));
        assert!(!STOP.is_match("хватит громко"));
    }

    #[tokio::test]
    async fn starts_a_draft_by_voice_listens_on_without_the_wake_word_and_leaves_on_enough() {
        use crate::config::{Config, Env};
        use crate::skills::router::{Continuation, Decision, ModelState, Resolution, RouteInput};
        use crate::skills::routes::RouteStep;
        use crate::utils::time::now_ms;

        let dir = tempfile::tempdir().unwrap();
        let text = |sub: &str| dir.path().join(sub).to_string_lossy().into_owned();
        let env = Env::from_pairs([
            ("STATE_DIR", String::new()),
            ("DIALOG_PROVIDER", "echo".into()),
            ("PACKAGES_DIR", text("packages")),
            ("DATA_DIR", text("data")),
            ("VOICE_LAB_DIR", text("voice-lab")),
            ("TTS_MODELS_DIR", text("voices")),
        ]);
        let app = crate::app::App::build(Config::for_tests(&env)).unwrap();
        let input = |text: &str, continuation: Option<Continuation>| RouteInput {
            text: text.into(),
            device_tag: "kitchen".into(),
            continuation,
            wake_word: continuation.is_none(),
            model: ModelState { enabled: true, tools: true },
            now: now_ms(),
            ..RouteInput::default()
        };
        let heard = |decision: Decision| match decision {
            Decision::Intent { intent, resolution: Resolution::Own(skill), rule, .. } => (intent, skill, rule),
            other => panic!("not ours: {other:?}"),
        };
        let (intent, skill, rule) = heard(app.skills.router.decide(&input("Алиса, давай сделаем навык", None)));
        assert_eq!(
            (intent.capability, intent.action.as_str(), rule),
            (CapabilityName::Plugins, "workshop", Some(RULE))
        );
        let context = ExecutionContext::new("kitchen", chrono::Utc::now());
        let started = skill.execute(&intent, &context).await.unwrap();
        assert!(started.speech.as_deref().unwrap().starts_with("Делаем навык"), "{:?}", started.speech);
        assert!(started.listen && started.continuation.is_some());
        assert!(app.workshop.draft("golos-1").is_ok(), "the draft is in the panel's workshop");

        // A follow-up without the wake word comes back here, and «хватит» leaves.
        let going =
            Continuation { capability: CapabilityName::Plugins, executor: RouteStep::Own, until: now_ms() + 60_000 };
        let (intent, skill, _) = heard(app.skills.router.decide(&input("хватит", Some(going))));
        assert_eq!(intent.action, "continue");
        let left = skill.execute(&intent, &context).await.unwrap();
        assert!(left.speech.as_deref().unwrap().starts_with("Выхожу из мастерской"), "{:?}", left.speech);
        assert!(!left.listen && left.continuation.is_none());
    }
}
