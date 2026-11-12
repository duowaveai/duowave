//! One road for «сделай…» (docs/plans/voice-creation.md, «Одна дорога для «сделай…»»): the panel's words and the
//! station's go the same way. One rule reads the request — «сделай виджет / навык / игру / тему …» — a draft of its
//! face is made from its template, its card says who asked and what, and the workshop's agent works on it in the
//! background: the panel and the station are free meanwhile, the card says when it is ready. Words for a draft that is
//! there already («переделай: крупнее») are new work on it.

use std::sync::{Arc, LazyLock};

use axum::http::StatusCode;
use futures::StreamExt;
use regex::Regex;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use super::activity::By;
use super::meta::{DraftMeta, Face, Stage};
use super::{agent, Workshop};
use crate::platform::http::ApiError;
use crate::skills::text::normalize;

/// «Сделай виджет: мои дела на сегодня», «давай сделаем игру на время», «нарисуй тему потемнее». «Гиджет» is how the
/// recognizer often hears the word.
static MAKE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        "^(?:сделай|создай|нарисуй|напиши|добавь|давай (?:сделаем|создадим|нарисуем|напишем)) (?:мне |нам )?(?:нов(?:ый|ую|ое) )?(?P<what>[вг]иджет|навык|плагин|игру|викторину|тему|приложение|учителя)(?: (?P<ask>.+))?$",
    )
    .expect("a valid pattern")
});

/// The workshop asked for by name, without what to make: the station asks what it should be.
static WORKSHOP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new("^(?:режим мастерской|открой мастерскую|включи мастерскую|мастерская)$").expect("a valid pattern")
});

/// A request to make something: its face and what it should do (empty — not said yet).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub face: Face,
    pub ask: String,
}

/// The words as the rules read them: normalized, a colon that only separates («сделай виджет: …») gone — a time
/// («в 10:30») keeps its own.
pub fn words_of(text: &str) -> String {
    let normalized = normalize(text).replace(": ", " ");
    normalized.trim_end_matches(':').trim().to_string()
}

/// The request in the words, if they are one: the same rule for the station and the panel.
pub fn request_of(words: &str) -> Option<Request> {
    let normalized = words_of(words);
    if WORKSHOP.is_match(&normalized) {
        return Some(Request { face: Face::Skill, ask: String::new() });
    }
    let found = MAKE.captures(&normalized)?;
    let face = match found.name("what").map(|what| what.as_str()).unwrap_or_default() {
        "виджет" | "гиджет" | "приложение" | "учителя" => Face::Widget,
        "игру" | "викторину" => Face::Game,
        "тему" => Face::Theme,
        _ => Face::Skill,
    };
    let mut ask = found.name("ask").map(|ask| ask.as_str().trim().to_string()).unwrap_or_default();
    // «Сделай учителя для MIDI-клавиатуры»: what to make is in the words, the teacher is a part of the ask.
    if found.name("what").is_some_and(|what| what.as_str() == "учителя") && !ask.is_empty() {
        ask = format!("учителя {ask}");
    }
    Some(Request { face, ask })
}

/// Words about a MIDI keyboard (docs/plans/midi.md, «Создание голосом»): the draft starts from the teacher's template.
/// «Миди» alone is also a Station model, so it counts only with «клавиатура»; the Latin «midi» and a piano count by
/// themselves.
static MIDI_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new("(?:midi|миди[- ]?клавиатур|клавиатур|пианино|фортепиано|синтезатор|рояль|рояле)")
        .expect("a valid pattern")
});

/// Whether the words ask for something of a MIDI keyboard.
pub fn about_midi(words: &str) -> bool {
    MIDI_WORDS.is_match(&words_of(words))
}

/// The template a request starts from: the teacher for a MIDI keyboard when the words are about one, the face's own
/// otherwise.
pub fn template_of(request: &Request, words: &str) -> &'static str {
    if request.face == Face::Widget && about_midi(words) {
        "midi"
    } else {
        request.face.template()
    }
}

/// Said when a gateway that hears no keyboard is asked for a teacher (the station's own, a build without MIDI).
pub const MIDI_ELSEWHERE: &str =
    "Учителю нужен компьютер с DuoWave: клавиатура подключается к нему. Попросите в пульте на компьютере.";

/// What `make` started: the draft, its face and its title for now (the agent may rename it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Made {
    pub draft: String,
    pub kind: Face,
    pub title: String,
}

/// A draft's id starts so: the panel's by its face, the station's as the voice workshop's always were.
fn prefix(by: By, face: Face) -> &'static str {
    match (by, face) {
        (By::Panel, Face::Widget) => "widget",
        (By::Panel, Face::Skill) => "skill",
        (By::Panel, Face::Game) => "game",
        (By::Panel, Face::Theme) => "theme",
        (By::Voice, Face::Widget) => "golos-vidzhet",
        (By::Voice, Face::Skill) => "golos",
        (By::Voice, Face::Game) => "golos-igra",
        (By::Voice, Face::Theme) => "golos-tema",
    }
}

/// The title until the agent gives its own: what was asked, from a capital, up to some 40 letters.
fn title_of(face: Face, ask: &str) -> String {
    let ask = ask.trim().trim_start_matches(|c: char| !c.is_alphanumeric());
    if ask.is_empty() {
        let word = face.word();
        let mut chars = word.chars();
        return chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default();
    }
    let mut title = String::new();
    for word in ask.split_whitespace() {
        if !title.is_empty() && title.chars().count() + word.chars().count() > 40 {
            break;
        }
        if !title.is_empty() {
            title.push(' ');
        }
        title.push_str(word);
    }
    let title: String = title.chars().take(60).collect();
    let mut chars = title.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// What the agent is told besides the skill when it works in the background (docs/plugins/skill/SKILL.md, «Работа в
/// фоне»): its last words go with «Готово» — said by the station, written on the panel.
pub const BACKGROUND_RULES: &str = "## Работа в фоне

Ты работаешь в фоне: человек попросил и занят своим, по ходу он не отвечает. Не задавай вопросов по ходу: чего не \
хватает — реши разумно сам. Когда закончишь, скажи одной-двумя короткими фразами, что сделал и что решил сам; вопрос, \
если без него никак, — один и в самом конце. Без списков, кода, разметки и ссылок: последние слова скажет колонка \
вместе с «Готово». Ставить сам не пытайся: на стол ставит человек, нажатием. Ходит в сеть — сразу напиши пример \
(examples.json) и проверь run_function с example: true: до «да» плитка показывает пример. Если черновик — виджет стола \
пульта (в манифесте слот desk.widgets): что он показывает, рисует panel.js, данные даёт panelData в backend.js.";

impl Workshop {
    /// Whether this gateway hears MIDI keyboards (src/midi).
    pub fn set_midi(&self, here: bool) {
        self.midi.store(here, std::sync::atomic::Ordering::Relaxed);
    }

    /// The model the agent answers with: «Мозг», once the gateway gives it.
    pub fn set_dialog(&self, dialog: Arc<crate::dialog::DialogService>) {
        let _ = self.dialog.set(dialog);
    }

    /// A free id for a new draft: `widget-1`, `golos-2`, …
    pub fn free_id(&self, prefix: &str) -> String {
        (1..)
            .map(|n| format!("{prefix}-{n}"))
            .find(|id| !self.dir().join(id).exists() && self.host().installed(id).is_none())
            .unwrap_or_else(|| prefix.to_string())
    }

    /// «Сделай…»: a new draft of the request's face — or, with `draft`, new work on that one — its card written, the
    /// agent at work in the background. The station's work has no `install`: a voice can be faked. Refused when the
    /// words are not a request (422) or no model is chosen (409).
    pub fn make(
        self: &Arc<Self>,
        words: &str,
        draft: Option<&str>,
        by: By,
        who: Option<String>,
    ) -> Result<Made, ApiError> {
        let words: String = words.trim().chars().take(4000).collect();
        let dialog = self
            .dialog
            .get()
            .cloned()
            .filter(|dialog| dialog.enabled())
            .ok_or_else(|| ApiError::conflict("Модель не выбрана: добавьте провайдера в «Персонаж» → «Мозг»."))?;
        if words.is_empty() {
            return Err(ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "Скажите, что сделать."));
        }
        let (id, kind, title) = match draft.map(str::trim).filter(|draft| !draft.is_empty()) {
            Some(id) => {
                let view = self.draft(id)?;
                (id.to_string(), view.meta.kind, view.title.unwrap_or_else(|| id.to_string()))
            }
            None => {
                let request = request_of(&words).ok_or_else(|| {
                    ApiError::new(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "Это не просьба «сделай…». Скажите, например: «Сделай виджет: мои дела на сегодня».",
                    )
                })?;
                if request.ask.is_empty() {
                    return Err(ApiError::new(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        format!("Скажите, что должен делать {}.", request.face.word()),
                    ));
                }
                let template = template_of(&request, &words);
                if template == "midi" && !self.midi.load(std::sync::atomic::Ordering::Relaxed) {
                    return Err(ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, MIDI_ELSEWHERE));
                }
                let id = self.free_id(prefix(by, request.face));
                let title = if template == "midi" {
                    "Учитель на MIDI-клавиатуре".to_string()
                } else {
                    title_of(request.face, &request.ask)
                };
                self.create(&id, Some(template), Some(&title), None)?;
                (id, request.face, title)
            }
        };
        let card = DraftMeta {
            by,
            who,
            asked: words.clone(),
            kind,
            stage: Stage::Working,
            updated: 0,
            note: None,
            kept: true,
        };
        self.keep_card(&id, card, Some("made"))?;
        tracing::info!(target: "plugins", "workshop: {} asked to make {id} ({})", match by { By::Voice => "a station", By::Panel => "the panel" }, kind.word());
        // The agent in the background, the panel's limits; the conversation is the draft's, so «переделай» goes on
        // from what it did. Nothing waits for the phrases: the card hears the work end.
        let key = format!("workshop:make:{id}");
        let mut phrases = agent::work(
            &dialog,
            self.clone(),
            &key,
            &id,
            &words,
            agent::Work { by, rules: Some(BACKGROUND_RULES), install: by == By::Panel, told: None },
            CancellationToken::new(),
        );
        tokio::spawn(async move {
            while let Some(phrase) = phrases.next().await {
                if phrase.is_err() {
                    break;
                }
            }
        });
        Ok(Made { draft: id, kind, title })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_one_request_for_the_station_and_the_panel() {
        let request = request_of("Алиса, сделай виджет: мои дела на сегодня из Тудуиста").unwrap();
        assert_eq!(request, Request { face: Face::Widget, ask: "мои дела на сегодня из тудуиста".into() });
        assert_eq!(request_of("давай сделаем гиджет").unwrap(), Request { face: Face::Widget, ask: String::new() });
        assert_eq!(request_of("Создай мне новую игру на время").unwrap().face, Face::Game);
        assert_eq!(request_of("нарисуй тему потемнее").unwrap(), Request { face: Face::Theme, ask: "потемнее".into() });
        assert_eq!(request_of("давай напишем новый плагин").unwrap().face, Face::Skill);
        assert_eq!(request_of("режим мастерской").unwrap(), Request { face: Face::Skill, ask: String::new() });
        // A teacher for a MIDI keyboard (docs/plans/midi.md): from the teacher's template, a Station «Миди» is not one.
        let teacher = request_of("Сделай учителя для моей MIDI-клавиатуры").unwrap();
        assert_eq!(teacher.face, Face::Widget);
        assert!(teacher.ask.starts_with("учителя "));
        assert_eq!(template_of(&teacher, "Сделай учителя для моей MIDI-клавиатуры"), "midi");
        let app = request_of("сделай приложение: учитель для миди клавиатуры").unwrap();
        assert_eq!(template_of(&app, "сделай приложение: учитель для миди клавиатуры"), "midi");
        let station = request_of("сделай виджет громкости миди").unwrap();
        assert_eq!(template_of(&station, "сделай виджет громкости миди"), "widget", "«Миди» alone is a station");
        let game = request_of("сделай игру на пианино").unwrap();
        assert_eq!(template_of(&game, "сделай игру на пианино"), "quiz", "a game keeps its own face");
        for not in ["сделай погромче", "сделай виджеты", "давай сделаем уроки", "добавь молоко в список"]
        {
            assert!(request_of(not).is_none(), "{not}");
        }
    }

    #[test]
    fn names_a_new_draft_by_who_asked_and_what() {
        assert_eq!(prefix(By::Panel, Face::Widget), "widget");
        assert_eq!(prefix(By::Voice, Face::Widget), "golos-vidzhet");
        assert_eq!(title_of(Face::Widget, "мои дела на сегодня из тудуиста"), "Мои дела на сегодня из тудуиста");
        assert_eq!(title_of(Face::Game, ""), "Игра");
        let long = title_of(Face::Skill, "напоминай мне каждое утро про таблетки и воду и прогулку с собакой в парке");
        assert!(long.chars().count() <= 45 && long.starts_with("Напоминай"), "{long}");
    }
}
