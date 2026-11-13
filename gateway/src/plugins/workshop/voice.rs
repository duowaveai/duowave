//! The workshop by voice (docs/plans/voice-creation.md, «Колонка: «Готово» и ответ без «Алиса»»). «Алиса, сделай
//! виджет: мои дела на сегодня» goes the panel's road (make.rs): a draft, its card, the agent in the background. The
//! station answers «Делаю виджет. Скажу, когда будет готово.» and is free: a new «Алиса» no longer cuts the work. The
//! draft shows on the panel's desk as a tile «попросили у колонки».
//!
//! When the agent is done, the station that asked says so by itself — the turn timer's way («время вышло»): our skill
//! answers a push (`TurnStation::ask`) — and listens some 5 s without the wake word:
//!
//! | Said after «Готово» | What happens |
//! |---|---|
//! | «примерить», «покажи» | The panel opens «Примерка» (`open:fitting`). |
//! | «на стол», «поставь» | The station never installs: «Одобрите в пульте» and `open:consent`. |
//! | «в черновики» | Parked. |
//! | «не надо» | «Удалить черновик?» → «да» — deleted. |
//! | «переделай: …» | New work in the background with these words. |
//! | silence | Nothing: the tile waits on the desk. |
//!
//! While the agent works: «Алиса, что с виджетом?» — its current step; «Алиса, отмени виджет» — stopped. A busy
//! station (playing, ringing, talking) has «Готово» wait up to 10 minutes; then it is not said, the tile waits.
//! Installing and new rights are never voice's: a voice can be faked.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use regex::Regex;
use serde_json::json;

use super::activity::{By, StepKind};
use super::meta::{DraftMeta, Face, Stage};
use super::{request_of, Workshop};
use crate::dialog::DialogService;
use crate::plugins::skill::TurnStation;
use crate::skills::capabilities::{CapabilityName, Intent};
use crate::skills::quick_commands::QuickMatch;
use crate::skills::router::{ExtraRules, EXECUTOR};
use crate::skills::skill::{ContinuationRequest, ExecutionContext, Outcome};
use crate::skills::text::normalize;

/// The rule's name in the journal.
const RULE: &str = "workshop:voice";
/// The answer to «Готово» or to «что на нём показать?» is awaited this long: about as long as the station listens
/// without «Алиса», and a little more for «Алиса, на стол».
const ANSWER_MS: u64 = 8_000;
/// «Что на нём показать?» waits a little longer: the person thinks.
const WHAT_MS: u64 = 20_000;
/// A busy station has «Готово» wait this long at most, asked again every so often.
const READY_WAITS: Duration = Duration::from_secs(10 * 60);
const BUSY_POLL: Duration = Duration::from_secs(2);

/// «Алиса, что с виджетом?», «как там игра».
static STATUS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new("^(?:что|как) (?:там )?(?:дела )?(?:с )?(?:моим |моей |мой |моя )?(?:[вг]иджет(?:ом)?|навык(?:ом)?|игр(?:ой|а)|тем(?:ой|а)|приложени(?:ем|е))$")
        .expect("a valid pattern")
});
/// «Алиса, отмени виджет», «останови игру».
static CANCEL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        "^(?:отмени|останови|прекрати|не делай|хватит делать) (?:делать )?(?:[вг]иджет|навык|игру|тему|приложение)$",
    )
    .expect("a valid pattern")
});

/// An answer after «Готово».
#[derive(Debug, Clone, PartialEq, Eq)]
enum Answer {
    Fit,
    Desk,
    Park,
    Drop,
    Redo(String),
    Other,
}

fn answer_of(words: &str) -> Answer {
    let words = super::make::words_of(words);
    let first = words.split(' ').next().unwrap_or_default();
    let starts = |prefixes: &[&str]| prefixes.iter().any(|prefix| words.starts_with(prefix));
    if starts(&["переделай", "исправь", "поменяй", "измени", "сделай ", "добавь", "убери "])
    {
        return Answer::Redo(words);
    }
    if starts(&["в черновик", "отложи", "потом", "пусть подождет", "оставь"]) {
        return Answer::Park;
    }
    if starts(&["не надо", "не нужно", "удали", "выброси", "не хочу"]) {
        return Answer::Drop;
    }
    if starts(&["на стол", "поставь", "ставь", "включи", "сразу"]) || first == "давай" && words.contains("стол")
    {
        return Answer::Desk;
    }
    if starts(&["пример", "покажи", "посмотр", "открой", "давай", "да"]) {
        return Answer::Fit;
    }
    Answer::Other
}

/// A device's answer awaited: to «Готово», to «Удалить черновик?», or what to make.
#[derive(Debug, Clone)]
enum Awaited {
    Ready { draft: String },
    Delete { draft: String },
    What { face: Face },
}

/// What the agent's step is, in a few words.
fn step_words(tool: Option<&str>, text: &str) -> String {
    let about = text.split_whitespace().next().unwrap_or_default();
    match tool.unwrap_or_default() {
        "write_file" | "edit_file" => format!("пишу {about}"),
        "read_file" => format!("читаю {about}"),
        "delete_file" => format!("убираю {about}"),
        "check" => "проверяю".into(),
        "run_function" => "пробую, что получилось".into(),
        "try_phrase" => "пробую фразу".into(),
        "docs" => "читаю справку".into(),
        "plugin_journal" => "читаю журнал".into(),
        _ => "думаю".into(),
    }
}

pub struct VoiceWorkshop {
    workshop: Arc<Workshop>,
    awaited: Mutex<HashMap<String, (Awaited, Instant)>>,
    /// Where «Готово» is said: the session registry, once it stands.
    turns: OnceLock<Arc<dyn TurnStation>>,
    /// Whether a panel is open now: «Открыто в пульте» or «Плитка ждёт на столе».
    panels: OnceLock<Arc<dyn Fn() -> bool + Send + Sync>>,
}

impl VoiceWorkshop {
    pub fn new(workshop: Arc<Workshop>, dialog: Arc<DialogService>) -> Arc<VoiceWorkshop> {
        workshop.set_dialog(dialog);
        let voice = Arc::new(VoiceWorkshop {
            workshop: workshop.clone(),
            awaited: Mutex::new(HashMap::new()),
            turns: OnceLock::new(),
            panels: OnceLock::new(),
        });
        // The agent finished a draft a station asked for: that station says so.
        let me: Weak<VoiceWorkshop> = Arc::downgrade(&voice);
        workshop.follow_cards(Arc::new(move |draft: &str, card: &DraftMeta| {
            if let Some(voice) = me.upgrade() {
                voice.card_changed(draft, card);
            }
        }));
        voice
    }

    /// The stations, for «Готово», once the session registry stands.
    pub fn set_turns(&self, turns: Arc<dyn TurnStation>) {
        let _ = self.turns.set(turns);
    }

    /// Whether a panel is open now (the live channel's listeners).
    pub fn set_panels(&self, panels: Arc<dyn Fn() -> bool + Send + Sync>) {
        let _ = self.panels.set(panels);
    }

    fn awaited(&self) -> std::sync::MutexGuard<'_, HashMap<String, (Awaited, Instant)>> {
        self.awaited.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The device's awaited answer, while it is still awaited.
    fn awaiting(&self, device: &str) -> Option<Awaited> {
        let mut awaited = self.awaited();
        match awaited.get(device) {
            Some((what, until)) if *until > Instant::now() => Some(what.clone()),
            Some(_) => {
                awaited.remove(device);
                None
            }
            None => None,
        }
    }

    fn wait_for(&self, device: &str, what: Awaited, ms: u64) -> ContinuationRequest {
        self.awaited().insert(device.to_string(), (what, Instant::now() + Duration::from_millis(ms)));
        ContinuationRequest { capability: CapabilityName::Plugins, ttl_ms: ms }
    }

    fn panel_open(&self) -> bool {
        self.panels.get().is_some_and(|open| open())
    }

    /// Whether the intent is the workshop's: a request, «Готово», a question about the work, or an awaited answer.
    pub fn handles(&self, intent: &Intent, device: &str) -> bool {
        intent.action.starts_with("workshop") || (intent.action == "continue" && self.awaiting(device).is_some())
    }

    pub fn execute<'a>(
        &'a self,
        intent: &'a Intent,
        context: &'a ExecutionContext,
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async move {
            let device = context.device_tag.as_str();
            let text = |key: &str| intent.params.get(key).and_then(|value| value.as_str()).unwrap_or_default();
            Ok(match intent.action.as_str() {
                "workshop" => {
                    let face = Face::parse(text("kind")).unwrap_or(Face::Skill);
                    self.start(device, face, text("ask"), &context.text)
                }
                "workshop.ready" => self.ready(device, text("draft")),
                "workshop.status" => Outcome::speech(self.status()),
                "workshop.cancel" => Outcome::speech(self.cancel()),
                _ => self.answer(device, text("text")),
            })
        })
    }

    /// «Сделай виджет…»: with what to make, the work starts in the background and the station is free; without it,
    /// the station asks what.
    fn start(&self, device: &str, face: Face, ask: &str, words: &str) -> Outcome {
        if ask.trim().is_empty() {
            let question = match face {
                Face::Widget => "Делаем виджет для стола пульта. Что на нём показать?",
                Face::Game => "Делаем игру. Какую?",
                Face::Theme => "Делаем тему. Какой она будет: тёмная или светлая, какие цвета?",
                Face::Skill => "Делаем навык. Что он должен уметь?",
            };
            return Outcome {
                speech: Some(question.into()),
                listen: true,
                continuation: Some(self.wait_for(device, Awaited::What { face }, WHAT_MS)),
                ..Outcome::default()
            };
        }
        self.make(device, words, None, face)
    }

    fn make(&self, device: &str, words: &str, draft: Option<&str>, face: Face) -> Outcome {
        match self.workshop.make(words, draft, By::Voice, Some(device.to_string())) {
            Ok(made) => {
                tracing::info!(target: "plugins", "workshop by voice on {device}: {} in the background", made.draft);
                let again = if draft.is_some() { "Переделываю" } else { "Делаю" };
                Outcome::speech(format!("{again} {}. Скажу, когда будет готово.", made.kind.accusative()))
            }
            // A teacher for a keyboard on a gateway that hears none: the line says where to ask, nothing failed.
            Err(error) if error.message == super::make::MIDI_ELSEWHERE => Outcome::speech(error.message),
            Err(error) => Outcome::speech(format!("Не получилось начать {}: {}", face.accusative(), error.message)),
        }
    }

    /// The card changed: a draft a station asked for is ready or failed — that station says so, once it is free.
    fn card_changed(self: Arc<Self>, draft: &str, card: &DraftMeta) {
        if card.by != By::Voice || !matches!(card.stage, Stage::Ready | Stage::Failed) {
            return;
        }
        let (Some(device), Some(turns)) = (card.who.clone(), self.turns.get().cloned()) else { return };
        let Ok(handle) = tokio::runtime::Handle::try_current() else { return };
        let (draft, updated) = (draft.to_string(), card.updated);
        handle.spawn(async move {
            let began = Instant::now();
            while turns.busy(&device) {
                if began.elapsed() > READY_WAITS {
                    tracing::info!(target: "plugins", "workshop: {draft} is ready, {device} was busy for 10 minutes — the tile waits");
                    return;
                }
                tokio::time::sleep(BUSY_POLL).await;
            }
            // Still the same card when the station calls back: not installed, parked or redone meanwhile.
            let still = {
                let workshop = self.workshop.clone();
                let draft = draft.clone();
                Arc::new(move || workshop.card(&draft).updated == updated) as Arc<dyn Fn() -> bool + Send + Sync>
            };
            let mut intent = Intent::new(CapabilityName::Plugins, "workshop.ready");
            intent.params.insert("draft".into(), json!(draft));
            if !turns.ask(&device, intent, format!("«готово» · {draft}"), still).await {
                tracing::info!(target: "plugins", "workshop: «готово» of {draft} was not said on {device}");
            }
        });
    }

    /// «Готово…» on the device that asked: the line from the manifest with the agent's last words, then the answer is
    /// listened for without the wake word.
    fn ready(&self, device: &str, draft: &str) -> Outcome {
        let Ok(view) = self.workshop.draft(draft) else { return Outcome::default() };
        let face = view.meta.kind;
        if view.meta.stage == Stage::Failed {
            return Outcome::speech(format!(
                "Не получилось сделать {}. Что случилось — в пульте, в мастерской.",
                face.accusative()
            ));
        }
        let line = view.ready_line.clone().unwrap_or_else(|| format!("Готово: {}.", face.word()));
        // The agent's words go after «Готово: …», before what it needs and the question.
        let speech = match view.meta.note.as_deref().filter(|note| !note.trim().is_empty()) {
            Some(note) => match line.split_once(". ") {
                Some((head, tail)) => format!("{head}. {} {tail}", note.trim()),
                None => format!("{line} {}", note.trim()),
            },
            None => line,
        };
        Outcome {
            speech: Some(speech),
            listen: true,
            continuation: Some(self.wait_for(device, Awaited::Ready { draft: draft.to_string() }, ANSWER_MS)),
            ..Outcome::default()
        }
    }

    /// The words that answer the station's question.
    fn answer(&self, device: &str, words: &str) -> Outcome {
        let Some(awaited) = self.awaiting(device) else { return Outcome::default() };
        self.awaited().remove(device);
        match awaited {
            Awaited::What { face } => {
                if words.trim().is_empty() {
                    return Outcome::default();
                }
                let words = format!("Сделай {}: {}", face.accusative(), words.trim());
                self.make(device, &words, None, face)
            }
            Awaited::Delete { draft } => {
                if normalize(words).starts_with("да") {
                    match self.workshop.delete(&draft) {
                        Ok(()) => Outcome::speech("Удалила черновик."),
                        Err(error) => Outcome::speech(format!("Не удалила: {}", error.message)),
                    }
                } else {
                    Outcome::speech("Хорошо, черновик ждёт на столе.")
                }
            }
            Awaited::Ready { draft } => self.after_ready(device, &draft, words),
        }
    }

    fn after_ready(&self, device: &str, draft: &str, words: &str) -> Outcome {
        let face = self.workshop.card(draft).kind;
        match answer_of(words) {
            Answer::Fit => {
                self.workshop.tell(draft, "open:fitting");
                Outcome::speech(if self.panel_open() {
                    "Открыто в пульте."
                } else {
                    "Плитка ждёт на столе."
                })
            }
            Answer::Desk => {
                // The station never installs: the «да» is a press in the panel.
                self.workshop.tell(draft, "open:consent");
                Outcome::speech(if self.panel_open() {
                    "Одобрите в пульте — окно открыто."
                } else {
                    "Одобрите в пульте: плитка ждёт на столе."
                })
            }
            Answer::Park => match self.workshop.park(draft) {
                Ok(_) => Outcome::speech("Черновик ждёт в мастерской."),
                Err(error) => Outcome::speech(format!("Не получилось: {}", error.message)),
            },
            Answer::Drop => Outcome {
                speech: Some("Удалить черновик?".into()),
                listen: true,
                continuation: Some(self.wait_for(device, Awaited::Delete { draft: draft.to_string() }, ANSWER_MS)),
                ..Outcome::default()
            },
            Answer::Redo(words) => self.make(device, &words, Some(draft), face),
            Answer::Other => {
                Outcome::speech("Не поняла. Можно сказать: примерить, на стол, в черновики или переделай.")
            }
        }
    }

    /// «Что с виджетом?»: the newest work going on, its current step.
    fn status(&self) -> String {
        let jobs = self.workshop.activity().jobs();
        let Some(job) = jobs.iter().find(|job| job.busy) else {
            return match jobs.first() {
                Some(job) => match self.workshop.draft(&job.draft) {
                    Ok(view) if view.meta.stage == Stage::Ready => {
                        view.ready_line.unwrap_or_else(|| "Готово — плитка на столе.".into())
                    }
                    _ => "Сейчас ничего не делаю.".into(),
                },
                None => "Сейчас ничего не делаю.".into(),
            };
        };
        let view = self.workshop.draft(&job.draft).ok();
        let face = view.as_ref().map(|view| view.meta.kind).unwrap_or(Face::Skill);
        let title = view.and_then(|view| view.title).unwrap_or_else(|| job.draft.clone());
        let step = match &job.last {
            Some(step) if step.kind == StepKind::Tool => step_words(step.tool.as_deref(), &step.text),
            Some(step) if step.kind == StepKind::Result => "смотрю, что получилось".into(),
            _ => "думаю".into(),
        };
        format!("Делаю {} «{title}»: {step}.", face.accusative())
    }

    /// «Отмени виджет»: the newest work going on stops.
    fn cancel(&self) -> String {
        let jobs = self.workshop.activity().jobs();
        match jobs.iter().find(|job| job.busy) {
            Some(job) if self.workshop.activity().stop(&job.draft) => {
                let face = self.workshop.card(&job.draft).kind;
                format!("Остановила. Черновик — {} — ждёт в мастерской.", face.word())
            }
            _ => "Сейчас ничего не делаю.".into(),
        }
    }
}

/// «Сделай виджет…», «что с виджетом?», «отмени виджет»: the router's rules, beside the plugins' phrases.
impl ExtraRules for VoiceWorkshop {
    fn matched(&self, normalized: &str) -> Option<QuickMatch> {
        let mut intent = Intent::new(CapabilityName::Plugins, "workshop");
        intent.params.insert(EXECUTOR.into(), json!(CapabilityName::Plugins.as_str()));
        if STATUS.is_match(normalized) {
            intent.action = "workshop.status".into();
            return Some(QuickMatch { intent, rule: RULE });
        }
        if CANCEL.is_match(normalized) {
            intent.action = "workshop.cancel".into();
            return Some(QuickMatch { intent, rule: RULE });
        }
        let request = request_of(normalized)?;
        let kind = match request.face {
            Face::Widget => "widget",
            Face::Skill => "skill",
            Face::Game => "game",
            Face::Theme => "theme",
        };
        intent.params.insert("kind".into(), json!(kind));
        intent.params.insert("ask".into(), json!(request.ask));
        Some(QuickMatch { intent, rule: RULE })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Env};
    use crate::core::events::GatewayEvent;
    use crate::skills::router::{Decision, ModelState, Resolution, RouteInput};
    use crate::utils::time::now_ms;

    /// A station that answers every push and keeps what it was asked.
    #[derive(Default)]
    struct Station {
        asked: Mutex<Vec<(String, Intent)>>,
        busy: std::sync::atomic::AtomicBool,
    }

    impl TurnStation for Station {
        fn ask(
            &self,
            device: &str,
            intent: Intent,
            _label: String,
            still: Arc<dyn Fn() -> bool + Send + Sync>,
        ) -> BoxFuture<'static, bool> {
            if still() {
                self.asked.lock().unwrap().push((device.to_string(), intent));
            }
            Box::pin(async { true })
        }
        fn hearing(&self, _device: &str) -> bool {
            false
        }
        fn ringing(&self, _device: &str) -> bool {
            false
        }
        fn busy(&self, _device: &str) -> bool {
            self.busy.load(std::sync::atomic::Ordering::Relaxed)
        }
    }

    async fn pushed(station: &Station) -> (String, Intent) {
        for _ in 0..200 {
            if let Some(found) = station.asked.lock().unwrap().pop() {
                return found;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("nothing was pushed to the station");
    }

    async fn stage_of(workshop: &Workshop, draft: &str, wanted: Stage) {
        for _ in 0..200 {
            if workshop.card(draft).stage == wanted {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("{draft} never became {wanted:?}: {:?}", workshop.card(draft));
    }

    /// Stage 5 of docs/plans/voice-creation.md: «Алиса, сделай виджет…» does not hold the station; the agent works in
    /// the background; the station that asked says «Готово…» by itself and hears the answer without the wake word —
    /// «примерить» opens «Примерка» in the panel, «переделай» starts new work, «не надо» → «да» deletes the draft.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn makes_a_widget_by_voice_in_the_background_and_says_ready_on_the_station_that_asked() {
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
        let mut heard = app.events.subscribe();
        let station = Arc::new(Station::default());
        let voice = VoiceWorkshop::new(app.workshop.clone(), app.dialog.clone());
        voice.set_turns(station.clone());
        let context = ExecutionContext::new("kitchen", chrono::Utc::now());
        let say = |outcome: &Outcome| outcome.speech.clone().unwrap_or_default();

        // The router hears the request by the shared rule.
        let input = RouteInput {
            text: "Алиса, сделай виджет мои дела на сегодня".into(),
            device_tag: "kitchen".into(),
            wake_word: true,
            model: ModelState { enabled: true, tools: true },
            now: now_ms(),
            ..RouteInput::default()
        };
        let Decision::Intent { intent, resolution: Resolution::Own(_), rule, .. } = app.skills.router.decide(&input)
        else {
            panic!("not ours");
        };
        assert_eq!((intent.action.as_str(), rule), ("workshop", Some(RULE)));
        assert_eq!(
            (intent.params["kind"].clone(), intent.params["ask"].clone()),
            (json!("widget"), json!("мои дела на сегодня"))
        );

        // The station is free at once.
        let mut asked = intent.clone();
        asked.params.insert("ask".into(), json!("мои дела на сегодня"));
        let context_asked = ExecutionContext {
            text: "сделай виджет мои дела на сегодня".into(), ..context.clone()
        };
        let started = voice.execute(&asked, &context_asked).await.unwrap();
        assert_eq!(say(&started), "Делаю виджет. Скажу, когда будет готово.");
        assert!(!started.listen && started.continuation.is_none(), "the station is not held");
        let draft = "golos-vidzhet-1";
        let card = app.workshop.card(draft);
        assert_eq!((card.by, card.who.as_deref(), card.kind), (By::Voice, Some("kitchen"), Face::Widget));
        let template: serde_json::Value =
            serde_json::from_str(&app.workshop.read(draft, "manifest.json").unwrap()).unwrap();
        assert_eq!(
            template["contributes"]["panel"]["cards"][0]["function"], "panelData",
            "the widget's template: a card"
        );

        // The agent finishes: «Готово…» is pushed to the station that asked.
        stage_of(&app.workshop, draft, Stage::Ready).await;
        let (device, ready) = pushed(&station).await;
        assert_eq!((device.as_str(), ready.action.as_str()), ("kitchen", "workshop.ready"));
        let said = voice.execute(&ready, &context).await.unwrap();
        assert!(say(&said).starts_with("Готово: виджет «Мои дела на сегодня»."), "{}", say(&said));
        assert!(say(&said).ends_with("Примерите — или сразу на стол?"), "{}", say(&said));
        assert!(said.listen && said.continuation.as_ref().is_some_and(|next| next.ttl_ms == ANSWER_MS));

        // «Примерить», without the wake word: the panel opens «Примерка».
        let mut answer = Intent::new(CapabilityName::Plugins, "continue");
        answer.params.insert("text".into(), json!("примерить"));
        assert!(voice.handles(&answer, "kitchen") && !voice.handles(&answer, "hall"));
        let opened = voice.execute(&answer, &context).await.unwrap();
        assert_eq!(say(&opened), "Плитка ждёт на столе.", "no panel is open in the test");
        assert!(!voice.handles(&answer, "kitchen"), "one answer");
        let mut changes = Vec::new();
        while let Ok(event) = heard.try_recv() {
            if let GatewayEvent::WorkshopChanged { draft, change } = event.as_ref() {
                // The card's changes; the agent's steps, its edits and the apps' history ride the same event.
                if change != "agent" && change != "edited" && change != "history" {
                    changes.push(format!("{draft}:{change}"));
                }
            }
        }
        assert_eq!(changes, [format!("{draft}:made"), format!("{draft}:ready"), format!("{draft}:open:fitting")]);

        // «Что с виджетом?» while nothing goes on: the ready line.
        assert!(voice.status().starts_with("Готово: виджет"), "{}", voice.status());

        // «Переделай: крупнее» after the next «Готово»: new work on the same draft; then «не надо» → «да».
        voice.clone().card_changed(draft, &app.workshop.card(draft));
        let (_, ready) = pushed(&station).await;
        voice.execute(&ready, &context).await.unwrap();
        answer.params.insert("text".into(), json!("переделай: крупнее"));
        let redo = voice.execute(&answer, &context).await.unwrap();
        assert_eq!(say(&redo), "Переделываю виджет. Скажу, когда будет готово.");
        assert!(app.workshop.card(draft).asked.contains("крупнее"));
        stage_of(&app.workshop, draft, Stage::Ready).await;
        let (_, ready) = pushed(&station).await;
        voice.execute(&ready, &context).await.unwrap();
        answer.params.insert("text".into(), json!("не надо"));
        let asked_again = voice.execute(&answer, &context).await.unwrap();
        assert_eq!(say(&asked_again), "Удалить черновик?");
        answer.params.insert("text".into(), json!("да"));
        assert_eq!(say(&voice.execute(&answer, &context).await.unwrap()), "Удалила черновик.");
        assert!(app.workshop.draft(draft).is_err());

        // A busy station: «Готово» waits (and is said once it is free).
        station.busy.store(true, std::sync::atomic::Ordering::Relaxed);
        let mut card = DraftMeta {
            by: By::Voice,
            who: Some("kitchen".into()),
            asked: "x".into(),
            kind: Face::Skill,
            stage: Stage::Ready,
            updated: 1,
            note: None,
            kept: true,
        };
        app.workshop.create("golos-9", Some("plugin"), Some("Навык"), None).unwrap();
        card = app.workshop.keep_card("golos-9", card, None).unwrap();
        voice.clone().card_changed("golos-9", &card);
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(station.asked.lock().unwrap().is_empty(), "nothing said while the station is busy");
        station.busy.store(false, std::sync::atomic::Ordering::Relaxed);
        let (_, ready) = pushed(&station).await;
        assert_eq!(ready.params["draft"], json!("golos-9"));
    }

    #[test]
    fn hears_the_answers_to_ready_and_the_questions_about_the_work() {
        assert_eq!(answer_of("примерить"), Answer::Fit);
        assert_eq!(answer_of("Покажи!"), Answer::Fit);
        assert_eq!(answer_of("на стол"), Answer::Desk);
        assert_eq!(answer_of("поставь"), Answer::Desk);
        assert_eq!(answer_of("в черновики"), Answer::Park);
        assert_eq!(answer_of("не надо"), Answer::Drop);
        assert_eq!(answer_of("переделай: крупнее"), Answer::Redo("переделай крупнее".into()));
        assert_eq!(answer_of("какая погода"), Answer::Other);
        assert!(
            STATUS.is_match("что с виджетом")
                && STATUS.is_match("как там игра")
                && STATUS.is_match("что с моим навыком")
        );
        assert!(CANCEL.is_match("отмени виджет") && CANCEL.is_match("останови игру"));
        assert!(!STATUS.is_match("что с погодой") && !CANCEL.is_match("отмени будильник"));
    }
}
