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
use crate::plugins::manifest;
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

/// Words about the home folder's photos (docs/plans/shared-folders.md, «Заготовка «Фотоальбом»»): «фото дня из папки
/// Фото», «фотоальбом», «мои фотографии».
static PHOTO_WORDS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("(?:фото|фотк|фотограф|снимк|альбом|из папки|в папке)").expect("a valid pattern"));

/// Whether the words ask for something of the home folder's photos.
pub fn about_photos(words: &str) -> bool {
    PHOTO_WORDS.is_match(&words_of(words))
}

/// The template a request starts from: the teacher for a MIDI keyboard when the words are about one, the photo album
/// when they are about the home folder's photos, the face's own otherwise.
pub fn template_of(request: &Request, words: &str) -> &'static str {
    if request.face == Face::Widget && about_midi(words) {
        "midi"
    } else if request.face == Face::Widget && about_photos(words) {
        "folders"
    } else {
        request.face.template()
    }
}

/// Said when the station's own gateway is asked for an album of the home folder and no Турбо gives the home its folders.
pub const FOLDERS_ELSEWHERE: &str =
    "Фотоальбому нужен компьютер с DuoWave: домашняя папка с фото — на нём. Подключите компьютер к дому как Турбо.";

/// Said when a teacher is asked where no keyboard can ever be heard: no MIDI here and no Турбо in the home
/// (docs/plans/turbo-devices.md — a Турбо's keyboard is the home's).
pub const MIDI_ELSEWHERE: &str =
    "Учителю нужен компьютер с DuoWave рядом с клавиатурой: подключите компьютер к дому как Турбо.";

/// Whether a template's device is in the home, and what is said of it (docs/plans/turbo-devices.md, «Мастерская»): made
/// where the device is here or at a Турбо — a sleeping one, one away, one with nothing plugged in said so; refused only
/// where it can never be: no MIDI here and no Турбо in the home, no folders here and none at a Турбо.
pub fn reach_of(
    template: &str,
    midi_here: bool,
    folders_here: bool,
    midi: Option<crate::home::turbo_devices::Whereabouts>,
    folders_at_turbo: Option<String>,
) -> Result<Option<String>, ApiError> {
    use crate::home::turbo_devices::Whereabouts;
    match template {
        "midi" if !midi_here => match midi {
            Some(Whereabouts::Here | Whereabouts::Turbo(_)) => Ok(None),
            Some(Whereabouts::Asleep(name)) => Ok(Some(crate::tr!(
                "Клавиатура у {name}, а он сейчас спит: учитель заработает, когда {name} проснётся.",
                name = name
            ))),
            Some(Whereabouts::Away(name)) => Ok(Some(crate::tr!(
                "Клавиатура у {name}, а он сейчас не дома: учитель заработает, когда {name} вернётся.",
                name = name
            ))),
            Some(Whereabouts::Unplugged(name)) => Ok(Some(crate::tr!(
                "Клавиатуру сейчас не вижу: подключите её к {name}, и учитель её услышит.",
                name = name
            ))),
            Some(Whereabouts::Never) => Ok(Some(crate::tr!(
                "Клавиатуры пока не вижу: подключите её к компьютеру с DuoWave, и учитель её услышит."
            ))),
            Some(Whereabouts::NoTurbo) | None => Err(ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, MIDI_ELSEWHERE)),
        },
        "folders" if !folders_here => match folders_at_turbo {
            Some(_) => Ok(None),
            None => Err(ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, FOLDERS_ELSEWHERE)),
        },
        _ => Ok(None),
    }
}

/// How an earlier wish rides with new words on the same draft.
const EARLIER: &str = "\n\nРаньше об этой вещи просили: ";

/// New words on a draft with the wish it was made for: «Переделай» alone says nothing of what the thing is, and the
/// agent's conversation is not always there to say it (19.11: a run that ended with no words left none, and «Переделай»
/// of a «Portal 2D» game made a quiz of the template). The first wish is kept, not every «переделай» before.
fn with_earlier(words: &str, before: &str) -> String {
    let before = before.trim();
    if before.is_empty() || before == words.trim() || words.contains(EARLIER.trim()) {
        return words.to_string();
    }
    let earlier = match before.split_once(EARLIER.trim()) {
        Some((_, earlier)) => earlier.trim().to_string(),
        None => format!("«{before}»"),
    };
    format!("{words}{EARLIER}{earlier}")
}

/// What `make` started: the draft, its face and its title for now (the agent may rename it). `noted`: the agent
/// already worked on the draft — the words went to that work («вдогонку»), nothing new began.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Made {
    pub draft: String,
    pub kind: Face,
    pub title: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub noted: bool,
    /// What is said with «Делаю»: where the thing's device is when it is not at hand now («Клавиатура у Mac, а он
    /// сейчас спит…», docs/plans/turbo-devices.md).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
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
/// фоне»): its last words go on the tile and to «подробнее»; «Готово» itself is the gateway's line
/// (docs/plans/audio-interface.md, «Честное «Готово»»).
pub const BACKGROUND_RULES: &str = "## Работа в фоне

Ты работаешь в фоне: человек попросил и занят своим, по ходу он не отвечает. Не задавай вопросов по ходу: чего не \
хватает — реши разумно сам. Когда закончишь, скажи одной-двумя короткими фразами, что сделал и что решил сам; вопрос, \
если без него никак, — один и в самом конце. Без списков, кода, разметки и ссылок: последние слова покажет плитка и \
скажет колонка по слову «подробнее»; «Готово» шлюз скажет сам, после своей проверки. По ходу могут прийти слова \
человека «Вдогонку: …» — это дополнение к этой же работе: учти их в ней, заново не начинай. Ставить сам не пытайся: на \
стол ставит человек — нажатием или своим «да». Ходит в сеть — сразу напиши пример \
(examples.json) и проверь run_function с example: true: до «да» плитка показывает пример. Виджет стола — карточка \
(contributes.panel.cards): данные из деталей пульта отдаёт её функция в backend.js, рисует пульт (docs: cards). \
Своё окошко (panel.js) — только когда карточки мало: своё рисование, игра, клавиши на экране, MIDI, звук, движение.";

impl Workshop {
    /// Whether this gateway hears MIDI keyboards (src/midi).
    pub fn set_midi(&self, here: bool) {
        self.midi.store(here, std::sync::atomic::Ordering::Relaxed);
    }

    /// Whether the home folder is here: a computer's gateway, not the station's own.
    pub fn set_folders(&self, here: bool) {
        self.folders.store(here, std::sync::atomic::Ordering::Relaxed);
    }

    /// The home's devices: a keyboard or folders at a Турбо are the home's too (docs/plans/turbo-devices.md).
    pub fn set_house(&self, house: &Arc<crate::home::turbo_devices::HouseDevices>) {
        let _ = self.house.set(Arc::downgrade(house));
    }

    /// Whether a template's device is in the home, and what is said of it (`reach_of`).
    fn reach(&self, template: &str) -> Result<Option<String>, ApiError> {
        let house = self.house.get().and_then(std::sync::Weak::upgrade);
        let midi_here = self.midi.load(std::sync::atomic::Ordering::Relaxed);
        let midi =
            (template == "midi" && !midi_here).then(|| house.as_ref().map(|house| house.whereabouts("midi"))).flatten();
        let folders = (template == "folders" && !self.folders_here())
            .then(|| house.as_ref().and_then(|house| house.folders_at_turbo()))
            .flatten();
        reach_of(template, midi_here, self.folders_here(), midi, folders)
    }

    fn folders_here(&self) -> bool {
        self.folders.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The model the agent answers with: «Мозг», once the gateway gives it.
    pub fn set_dialog(&self, dialog: Arc<crate::dialog::DialogService>) {
        let _ = self.dialog.set(dialog);
    }

    /// A free id for a new draft: a random one (`manifest::new_package_id`) no draft and no package has. Who asked and
    /// the face are the draft's card, not its id.
    pub fn new_id(&self) -> String {
        loop {
            let id = manifest::new_package_id();
            if !self.dir().join(&id).exists() && self.host().installed(&id).is_none() {
                return id;
            }
        }
    }

    /// «Сделай…»: a new draft of the request's face — or, with `draft`, new work on that one — its card written, the
    /// agent at work in the background. Words for a draft the agent works on now are added to that work («вдогонку»):
    /// nothing is cancelled. The station's work has no `install`: a voice can be faked. Refused when the words are not
    /// a request (422) or no model is chosen (409).
    pub fn make(
        self: &Arc<Self>,
        words: &str,
        draft: Option<&str>,
        by: By,
        who: Option<String>,
    ) -> Result<Made, ApiError> {
        self.make_for(words, draft, by, who, None)
    }

    /// `make` for the talk's work `work` (src/talk): the card names it.
    pub fn make_for(
        self: &Arc<Self>,
        words: &str,
        draft: Option<&str>,
        by: By,
        who: Option<String>,
        work: Option<&str>,
    ) -> Result<Made, ApiError> {
        let mut words: String = words.trim().chars().take(4000).collect();
        let dialog = self.dialog.get().cloned().filter(|dialog| dialog.enabled()).ok_or_else(|| {
            ApiError::conflict(crate::tr!("Модель не выбрана: добавьте провайдера в «Персонаж» → «Мышление»."))
        })?;
        if words.is_empty() {
            return Err(ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "Скажите, что сделать."));
        }
        let mut note = None;
        let (id, kind, title) = match draft.map(str::trim).filter(|draft| !draft.is_empty()) {
            Some(id) => {
                let view = self.draft(id)?;
                let title = view.title.unwrap_or_else(|| id.to_string());
                if self.note(id, &words) {
                    tracing::info!(target: "plugins", "workshop: words added to the work on {id}");
                    return Ok(Made { draft: id.to_string(), kind: view.meta.kind, title, noted: true, note: None });
                }
                words = with_earlier(&words, &self.card(id).asked);
                (id.to_string(), view.meta.kind, title)
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
                note = self.reach(template)?;
                let id = self.new_id();
                let title = if template == "midi" {
                    "Учитель на MIDI-клавиатуре".to_string()
                } else {
                    title_of(request.face, &request.ask)
                };
                self.create(&id, Some(template), Some(&title), None)?;
                (id, request.face, title)
            }
        };
        let install = by == By::Panel;
        let work = work.map(str::to_string);
        self.launch(&dialog, Launch { id, words, by, who, kind, title, work, install })
            .map(|made| Made { note, ..made })
    }

    /// A new thing asked for in a conversation (src/talk, `work_start`): its face, its title (none — from the wish),
    /// the whole wish in the person's words — no words of a template needed. The card names the talk's work the run
    /// belongs to. Never installs by itself: the thing goes on the desk after a «да». Refused as `make` is.
    pub fn begin(
        self: &Arc<Self>,
        face: Face,
        title: Option<&str>,
        brief: &str,
        by: By,
        who: Option<String>,
        work: Option<&str>,
    ) -> Result<Made, ApiError> {
        let brief: String = brief.trim().chars().take(4000).collect();
        let dialog = self.dialog.get().cloned().filter(|dialog| dialog.enabled()).ok_or_else(|| {
            ApiError::conflict(crate::tr!("Модель не выбрана: добавьте провайдера в «Персонаж» → «Мышление»."))
        })?;
        if brief.is_empty() {
            return Err(ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "Скажите, что сделать."));
        }
        let request = Request { face, ask: brief.clone() };
        let template = template_of(&request, &brief);
        let note = self.reach(template)?;
        let id = self.new_id();
        let title = match title.map(str::trim).filter(|title| !title.is_empty()) {
            _ if template == "midi" => super::templates::MIDI_TITLE.to_string(),
            Some(title) => title.chars().take(60).collect(),
            None => title_of(face, &brief),
        };
        self.create(&id, Some(template), Some(&title), None)?;
        let work = work.map(str::to_string);
        self.launch(&dialog, Launch { id, words: brief, by, who, kind: face, title, work, install: false })
            .map(|made| Made { note, ..made })
    }

    /// The card written and the agent at work on the draft in the background: the panel's limits, the conversation
    /// the draft's own, so «переделай» goes on from what it did. Nothing waits for the phrases: the card hears the
    /// work end.
    fn launch(self: &Arc<Self>, dialog: &Arc<crate::dialog::DialogService>, launch: Launch) -> Result<Made, ApiError> {
        let Launch { id, words, by, who, kind, title, work, install } = launch;
        let card = DraftMeta {
            by,
            who,
            asked: words.clone(),
            kind,
            stage: Stage::Working,
            updated: 0,
            note: None,
            kept: true,
            work,
            unchanged: false,
        };
        self.keep_card(&id, card, Some("made"))?;
        tracing::info!(target: "plugins", "workshop: {} asked to make {id} ({})", match by { By::Voice => "a station", By::Panel => "the panel" }, kind.word());
        let key = format!("workshop:make:{id}");
        let mut phrases = agent::work(
            dialog,
            self.clone(),
            &key,
            &id,
            &words,
            agent::Work { by, rules: Some(BACKGROUND_RULES), install, told: None },
            CancellationToken::new(),
        );
        tokio::spawn(async move {
            while let Some(phrase) = phrases.next().await {
                if phrase.is_err() {
                    break;
                }
            }
        });
        Ok(Made { draft: id, kind, title, noted: false, note: None })
    }
}

/// What a run of the agent on a draft starts with: the draft, the words it gets, who asked, the face, the title, the
/// talk's work, whether the agent may install.
struct Launch {
    id: String,
    words: String,
    by: By,
    who: Option<String>,
    kind: Face,
    title: String,
    work: Option<String>,
    install: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The request of 19.11 («Сделай виджет MIDI-клавиатуры…») on the station: refused then by the station's own build;
    /// now made wherever a keyboard is the home's, and said where it is (docs/plans/turbo-devices.md).
    #[test]
    fn a_teacher_is_made_where_the_homes_keyboard_is_and_said_where() {
        use crate::home::turbo_devices::Whereabouts;
        let reach = |midi: Option<Whereabouts>| reach_of("midi", false, false, midi, None);
        assert_eq!(reach_of("midi", true, false, None, None).unwrap(), None, "a computer's own keyboard");
        assert_eq!(reach(Some(Whereabouts::Turbo("Mac".into()))).unwrap(), None);
        let asleep = reach(Some(Whereabouts::Asleep("Mac".into()))).unwrap().unwrap();
        assert!(asleep.contains("Mac") && asleep.contains("спит"), "{asleep}");
        assert!(reach(Some(Whereabouts::Away("Mac".into()))).unwrap().unwrap().contains("не дома"));
        assert!(reach(Some(Whereabouts::Unplugged("Mac".into()))).unwrap().unwrap().contains("подключите"));
        assert!(reach(Some(Whereabouts::Never)).unwrap().unwrap().contains("компьютеру с DuoWave"));
        assert_eq!(reach(Some(Whereabouts::NoTurbo)).unwrap_err().message, MIDI_ELSEWHERE, "no Турбо at all");
        assert_eq!(reach_of("folders", false, false, None, Some("Mac".into())).unwrap(), None, "a Турбо's folders");
        assert_eq!(reach_of("folders", false, false, None, None).unwrap_err().message, FOLDERS_ELSEWHERE);
        assert_eq!(reach_of("widget", false, false, None, None).unwrap(), None);
    }

    #[test]
    fn new_words_on_a_draft_carry_the_first_wish_once() {
        let once = with_earlier("Переделай", "сделай игру portal 2d, пару уровней");
        assert_eq!(once, "Переделай\n\nРаньше об этой вещи просили: «сделай игру portal 2d, пару уровней»");
        // Another «переделай» keeps the first wish, not the chain.
        assert_eq!(
            with_earlier("Крупнее", &once),
            "Крупнее\n\nРаньше об этой вещи просили: «сделай игру portal 2d, пару уровней»"
        );
        assert_eq!(with_earlier("Переделай", ""), "Переделай");
        assert_eq!(with_earlier("Переделай", "Переделай"), "Переделай");
    }

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
        assert_eq!(template_of(&game, "сделай игру на пианино"), "game", "a game starts blank, from no quiz");
        // The home folder's photos (docs/plans/shared-folders.md): from the album's template.
        let album = request_of("Сделай виджет: фото дня из папки Фото").unwrap();
        assert_eq!(template_of(&album, "Сделай виджет: фото дня из папки Фото"), "folders");
        let plain = request_of("сделай виджет: мои дела на сегодня").unwrap();
        assert_eq!(template_of(&plain, "сделай виджет: мои дела на сегодня"), "widget");
        for not in ["сделай погромче", "сделай виджеты", "давай сделаем уроки", "добавь молоко в список"]
        {
            assert!(request_of(not).is_none(), "{not}");
        }
    }

    #[test]
    fn titles_a_new_draft_by_what_was_asked() {
        assert_eq!(title_of(Face::Widget, "мои дела на сегодня из тудуиста"), "Мои дела на сегодня из тудуиста");
        assert_eq!(title_of(Face::Game, ""), "Игра");
        let long = title_of(Face::Skill, "напоминай мне каждое утро про таблетки и воду и прогулку с собакой в парке");
        assert!(long.chars().count() <= 45 && long.starts_with("Напоминай"), "{long}");
    }
}
