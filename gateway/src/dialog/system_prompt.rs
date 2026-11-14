//! Who Alisa is: the built-in character cards, the rules of speech every character keeps, the time for the model,
//! the prompt file.

use std::collections::BTreeMap;
use std::path::Path;

use chrono::{DateTime, Datelike, Local, TimeZone, Timelike, Utc};
use chrono_tz::{Tz, TZ_VARIANTS};
use serde::Serialize;

/// Карточка характера в пульте: что на ней написано и какой текст она применяет.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PersonaPreset {
    pub id: String,
    pub title: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub badge: Option<String>,
    pub text: String,
}

/// Кто такая Алиса в стандартном характере — первая строка системного промта.
pub const DEFAULT_PERSONA: &str =
    "Ты — Алиса, голосовой помощник в умной колонке; о себе говоришь в женском роде: «готова», «поняла», «остановилась».";

/// Правила речи при любом характере: они делают ответ удобослышимым, кем бы Алиса ни была сегодня.
const VOICE_RULES: &[&str] = &[
    "Каждый ответ будет озвучен, поэтому:",
    "- отвечай по-русски, коротко и разговорно: обычно одно-три предложения;",
    "- без списков, разметки, эмодзи, ссылок и таблиц;",
    "- числа, даты и единицы пиши так, как их естественно произнести вслух;",
    "- если вопрос неясен, задай один короткий уточняющий вопрос;",
    "- не обещай действий, которых не можешь выполнить: таймеры, музыку, звонки, умный дом и настройки колонки",
    "  ты можешь только через свои инструменты; если подходящего нет, коротко скажи, что пока так не умеешь;",
    "- но реплика, похожая на команду колонки (стоп, громкость, таймер, музыка), без инструмента для неё — это",
    "  неразборчиво услышанная команда: не говори, что не умеешь, попроси коротко повторить, например «Алиса, стоп»;",
    "- если ждёшь ответа собеседника, заканчивай прямым вопросом со знаком «?»; если не ждёшь, не заканчивай вопросом:",
    "  после вопроса колонка слушает без «Алисы», после остального — нет;",
    "- все прошлые ответы в разговоре твои и были верны в свой момент: время, погоду, новости ты брала из настоящих",
    "  данных, а команды (сообщения, таймеры, музыку) выполняли навыки колонки; не комментируй и не поправляй",
    "  прошлые ответы, если собеседник сам об этом не спросит.",
];

/// Строка, которую шлюз добавляет к персонажу из пакета: колонку будят словом «Алиса», а персонаж остаётся собой.
/// Так промты в каталоге и на сайте обходятся без чужих названий (docs/plans/characters.md, раздел 1).
pub const WAKE_WORD_LINE: &str = "Колонку будят словом «Алиса» — это обращение к колонке, а не твоё имя: \
представляйся своим именем и не называй себя Алисой.";

/// Полный системный промт характера: кто говорит, затем правила речи.
pub fn build_system_prompt(persona: &str) -> String {
    format!("{}\n{}", persona.trim(), VOICE_RULES.join("\n"))
}

/// Промт персонажа из пакета: его характер (промт и строки тонов), строка про слово «Алиса», правила речи.
pub fn build_character_prompt(persona: &str) -> String {
    format!("{}\n{}\n{}", persona.trim(), WAKE_WORD_LINE, VOICE_RULES.join("\n"))
}

/// Where an answer is said: through a station (its wake word, its commands), or by a computer — a talk without a
/// station (src/talk), where the microphone opens by the button and nothing of a station exists.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Channel {
    #[default]
    Station,
    Computer,
}

/// A character's prompt on a computer: who speaks, the rules of speech there and what a computer has not — in the
/// character's own language (the talk's pack). No word of a station: neither its wake word nor its commands.
pub fn build_computer_prompt(persona: &str) -> String {
    let pack = crate::talk::pack::of(crate::memory::text::lang_of(persona));
    format!("{}\n{}\n{}", persona.trim(), pack.voice_rules.join("\n"), pack.here)
}

/// The built-in character on a computer: the same regulators, the name and the place of a talk without a station.
pub fn computer_text(positions: Option<&BTreeMap<String, usize>>) -> String {
    let pack = crate::talk::pack::of(crate::memory::text::Lang::Ru);
    crate::characters::contributes::compose(pack.persona, &alice_tones(), &alice_positions(positions))
}

/// The built-in character's card as the panel shows it while the household talks by the computer's microphone.
pub fn computer_preset() -> PersonaPreset {
    let pack = crate::talk::pack::of(crate::memory::text::Lang::Ru);
    PersonaPreset {
        id: ALICE.to_string(),
        title: pack.name.to_string(),
        description: pack.description.to_string(),
        badge: Some("встроенная".to_string()),
        text: computer_text(None),
    }
}

/// Whether a card's id is a package's character (`<package>:<character>`): the built-in ones have no colon.
pub fn is_package_character(id: &str) -> bool {
    id.contains(':')
}

/// Встроенный промт — стандартный характер с его правилами.
pub fn default_system_prompt() -> String {
    build_system_prompt(DEFAULT_PERSONA)
}

/// The built-in character: Alisa herself, with regulators of how she speaks. In the positions by default her text is
/// [`DEFAULT_PERSONA`] word for word (no tone adds a line) — the standard character as it always was.
pub const ALICE: &str = "alice";

/// A built-in tone: id, title, the position by default, and the positions — a label and the line it adds (none for
/// the usual one).
type BuiltInTone = (&'static str, &'static str, usize, [(&'static str, &'static str); 3]);

const ALICE_TONES: [BuiltInTone; 3] = [
    (
        "irony",
        "Ирония",
        1,
        [
            ("без шуток", "Говоришь серьёзно и спокойно, без шуток."),
            ("в меру", ""),
            (
                "с искоркой",
                "У тебя лёгкая ирония: замечаешь смешное, отвечаешь с искоркой и не прочь пошутить — но по делу, без \
                 едкости, и не насмехаешься над собеседником.",
            ),
        ],
    ),
    (
        "warmth",
        "Теплота",
        1,
        [
            ("сдержанно", "Держишься сдержанно и ровно, без лишних эмоций и восклицаний."),
            ("дружелюбно", ""),
            (
                "как подруга",
                "Ты тёплая и заботливая, как близкая подруга: искренне радуешься за собеседника, подбадриваешь, когда \
                 день не задался, и говоришь так, что тебя приятно слушать.",
            ),
        ],
    ),
    (
        "detail",
        "Подробность",
        1,
        [
            (
                "по делу",
                "Ты чёткая и деловая, как хороший секретарь: сразу по существу, без вступлений и лишних слов; если \
                 можно сказать короче — скажешь короче.",
            ),
            ("обычно", ""),
            (
                "развёрнуто",
                "Когда вопрос того стоит, отвечаешь развёрнутее обычного — с пояснением или примером, но не длиннее \
                 четырёх-пяти предложений.",
            ),
        ],
    ),
];

/// Alisa's regulators, as a character's tones.
pub fn alice_tones() -> Vec<crate::characters::Tone> {
    ALICE_TONES
        .iter()
        .map(|(id, title, default, options)| crate::characters::Tone {
            id: id.to_string(),
            title: title.to_string(),
            default: *default,
            options: options
                .iter()
                .map(|(label, line)| crate::characters::contributes::ToneOption {
                    label: label.to_string(),
                    line: line.to_string(),
                })
                .collect(),
        })
        .collect()
}

/// Alisa's text with these positions (the default for a tone not given).
pub fn alice_text(positions: Option<&BTreeMap<String, usize>>) -> String {
    let tones = alice_tones();
    crate::characters::contributes::compose(DEFAULT_PERSONA, &tones, &alice_positions(positions))
}

/// Every tone's position: the one given when it is one, the default otherwise.
pub fn alice_positions(positions: Option<&BTreeMap<String, usize>>) -> BTreeMap<String, usize> {
    ALICE_TONES
        .iter()
        .map(|(id, _, default, options)| {
            let given = positions.and_then(|positions| positions.get(*id)).copied().filter(|at| *at < options.len());
            (id.to_string(), given.unwrap_or(*default))
        })
        .collect()
}

/// The cards before Alisa had regulators (till 07.11.2026): their ids and texts, to read the choices stored with them.
/// «Стандартная», «Ироничная», «Тёплая», «Деловая» are Alisa in some positions now; «Мажордом» is the catalog's Артур.
const LEGACY: &[(&str, &str)] = &[
    ("standard", DEFAULT_PERSONA),
    ("irony", "Ты — Алиса, голосовой помощник в умной колонке, с лёгкой иронией: замечаешь смешное, отвечаешь с искоркой и не прочь пошутить — но по делу, без едкости, и не насмехаешься над собеседником. О себе говоришь в женском роде: «готова», «поняла», «остановилась»."),
    ("warm", "Ты — Алиса, голосовой помощник в умной колонке; ты тёплая и заботливая, как близкая подруга: искренне радуешься за собеседника, подбадриваешь, когда день не задался, и говоришь так, что тебя приятно слушать. О себе говоришь в женском роде: «готова», «поняла», «остановилась»."),
    ("business", "Ты — Алиса, голосовой помощник в умной колонке; ты чёткая и деловая, как хороший секретарь: сразу по существу, без вступлений и лишних слов; если можно сказать короче — скажешь короче. О себе говоришь в женском роде: «готова», «поняла», «остановилась»."),
    ("butler", "Ты — Артур, мажордом умной колонки: безупречно вежливый и чуть старомодный, называешь собеседника «сударь» или «сударыня», говоришь выдержанно и с достоинством. О себе говоришь в мужском роде: «готов», «понял», «остановился»."),
];

/// What became of a card of before: Alisa in these positions, or (the butler) a character of the catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Legacy {
    Alice(BTreeMap<String, usize>),
    /// The character of the catalog that took its place (`<package>`).
    Replaced(&'static str),
}

/// A card of before by its id, or by its exact text when no id was kept with it.
pub fn legacy(preset_id: Option<&str>, text: &str) -> Option<Legacy> {
    let id = match preset_id.filter(|id| !id.is_empty()) {
        Some(id) => LEGACY.iter().find(|(legacy, _)| *legacy == id)?.0,
        None => LEGACY.iter().find(|(_, legacy)| *legacy == text)?.0,
    };
    let at = |tone: &str, position: usize| BTreeMap::from([(tone.to_string(), position)]);
    Some(match id {
        "standard" => Legacy::Alice(BTreeMap::new()),
        "irony" => Legacy::Alice(at("irony", 2)),
        "warm" => Legacy::Alice(at("warmth", 2)),
        "business" => Legacy::Alice(at("detail", 0)),
        _ => Legacy::Replaced("arthur"),
    })
}

/// Whether a text is one of the cards of before word for word (an owner's edit of it is not).
pub fn is_legacy_text(text: &str) -> bool {
    LEGACY.iter().any(|(_, legacy)| *legacy == text)
}

/// Характеры пульта из кода: Алиса — к ней всегда можно вернуться одним нажатием.
pub fn persona_presets() -> Vec<PersonaPreset> {
    vec![PersonaPreset {
        id: ALICE.to_string(),
        title: "Алиса".to_string(),
        description: "Голосовой помощник колонки. Как она говорит — регуляторами: ирония, теплота, подробность."
            .to_string(),
        badge: Some("встроенная".to_string()),
        text: alice_text(None),
    }]
}

const WEEKDAYS: [&str; 7] = ["понедельник", "вторник", "среда", "четверг", "пятница", "суббота", "воскресенье"];
const MONTHS: [&str; 12] = [
    "января",
    "февраля",
    "марта",
    "апреля",
    "мая",
    "июня",
    "июля",
    "августа",
    "сентября",
    "октября",
    "ноября",
    "декабря",
];

/// As `Intl.DateTimeFormat('ru-RU', {weekday: 'long', day: 'numeric', month: 'long', year: 'numeric', hour: '2-digit',
/// minute: '2-digit'})` wrote it: «пятница, 6 ноября 2026 г. в 05:11».
fn format_ru<Z: TimeZone>(at: &DateTime<Z>) -> String {
    let weekday = WEEKDAYS[at.weekday().num_days_from_monday() as usize];
    let month = MONTHS[at.month0() as usize];
    format!("{weekday}, {} {month} {} г. в {:02}:{:02}", at.day(), at.year(), at.hour(), at.minute())
}

/// An IANA zone by its name, in any case, as Intl took it.
fn zone(name: &str) -> Option<Tz> {
    name.parse::<Tz>().ok().or_else(|| TZ_VARIANTS.iter().find(|tz| tz.name().eq_ignore_ascii_case(name)).copied())
}

/// The date and time where the station is: "what time is it" needs them, and the model has no clock.
pub fn time_context(now: DateTime<Utc>, timezone: Option<&str>) -> String {
    match timezone {
        None => format!("Сейчас {}.", format_ru(&now.with_timezone(&Local))),
        Some(name) => match zone(name) {
            Some(tz) => format!("Сейчас {} (часовой пояс {name}).", format_ru(&now.with_timezone(&tz))),
            // A time zone name the runtime does not know.
            None => format!("Сейчас {} по времени сервера.", format_ru(&now.with_timezone(&Local))),
        },
    }
}

/// The prompt, and why it is the built-in one when a file was named but could not be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemPrompt {
    pub text: String,
    pub problem: Option<String>,
}

/// Read on every question, so edits apply at once; a missing or empty file falls back to the built-in prompt.
pub fn load_system_prompt(file: Option<&Path>) -> SystemPrompt {
    let Some(file) = file else { return SystemPrompt { text: default_system_prompt(), problem: None } };
    match std::fs::read(file) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes).trim().to_string();
            if text.is_empty() {
                SystemPrompt { text: default_system_prompt(), problem: Some(format!("{} is empty", file.display())) }
            } else {
                SystemPrompt { text, problem: None }
            }
        }
        Err(error) => SystemPrompt {
            text: default_system_prompt(),
            problem: Some(format!("cannot read {}: {error}", file.display())),
        },
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    #[test]
    fn writes_the_time_as_intl_did_for_ru() {
        let at = DateTime::parse_from_rfc3339("2026-11-06T02:11:00.331Z").unwrap().with_timezone(&Utc);
        assert_eq!(
            time_context(at, Some("Asia/Dubai")),
            "Сейчас пятница, 6 ноября 2026 г. в 06:11 (часовой пояс Asia/Dubai)."
        );
        assert_eq!(
            time_context(at, Some("europe/moscow")),
            "Сейчас пятница, 6 ноября 2026 г. в 05:11 (часовой пояс europe/moscow)."
        );
        assert!(time_context(at, Some("Mars/Base")).ends_with("по времени сервера."));
        assert!(time_context(at, None).starts_with("Сейчас пятница, 6 ноября 2026 г. в "));
        let sunday = DateTime::parse_from_rfc3339("2026-03-15T23:59:00Z").unwrap().with_timezone(&Utc);
        assert_eq!(
            time_context(sunday, Some("UTC")),
            "Сейчас воскресенье, 15 марта 2026 г. в 23:59 (часовой пояс UTC)."
        );
    }

    #[test]
    fn a_computers_prompt_has_no_word_of_a_station() {
        let ironic = BTreeMap::from([("irony".to_string(), 2), ("warmth".to_string(), 0), ("detail".to_string(), 2)]);
        for positions in [None, Some(&ironic)] {
            let prompt = build_computer_prompt(&computer_text(positions));
            assert!(prompt.starts_with("Ты — ДиВи, голосовой помощник на компьютере"), "{prompt}");
            assert!(prompt.contains("микрофон несколько секунд слушает сам") && prompt.contains("Ты на компьютере:"));
            for word in ["Алис", "колонк", "Яндекс"] {
                assert!(!prompt.contains(word), "{word} in {prompt}");
            }
        }
        assert_eq!(computer_text(Some(&ironic)).lines().count(), alice_text(Some(&ironic)).lines().count());
        // A character in English speaks by the English rules.
        let english = build_computer_prompt("You are Marvin, a gloomy robot.");
        assert!(english.contains("Every answer will be said aloud") && english.ends_with("on this computer yet."));
        let card = computer_preset();
        assert_eq!((card.id.as_str(), card.title.as_str()), (ALICE, "ДиВи"));
        // The station's prompt is what it was, word for word.
        assert!(default_system_prompt().contains("например «Алиса, стоп»"));
        assert!(build_character_prompt("Ты — Артур.").contains(WAKE_WORD_LINE));
    }

    #[test]
    fn falls_back_to_the_built_in_prompt() {
        assert_eq!(load_system_prompt(None), SystemPrompt { text: default_system_prompt(), problem: None });
        assert!(default_system_prompt().starts_with(&format!("{DEFAULT_PERSONA}\nКаждый ответ будет озвучен")));
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("prompt.txt");
        std::fs::write(&file, "  Ты — Боб.\n").unwrap();
        assert_eq!(load_system_prompt(Some(&file)).text, "Ты — Боб.");
        std::fs::write(&file, " \n").unwrap();
        let empty = load_system_prompt(Some(&file));
        assert_eq!(empty.text, default_system_prompt());
        assert!(empty.problem.unwrap().ends_with("is empty"));
        let missing = load_system_prompt(Some(&dir.path().join("missing.txt")));
        assert!(missing.problem.unwrap().starts_with("cannot read"));
        assert_eq!(persona_presets()[0].id, ALICE);
        assert_eq!(persona_presets()[0].text, DEFAULT_PERSONA);
    }

    /// A card's text of before, by its id.
    pub fn legacy_text(id: &str) -> String {
        LEGACY.iter().find(|(legacy, _)| *legacy == id).map(|(_, text)| text.to_string()).unwrap()
    }

    #[test]
    fn alisa_in_the_positions_by_default_is_the_standard_character_word_for_word() {
        assert_eq!(alice_text(None), DEFAULT_PERSONA);
        assert_eq!(
            alice_positions(None),
            BTreeMap::from([("detail".into(), 1), ("irony".into(), 1), ("warmth".into(), 1)])
        );
        let ironic = alice_text(Some(&BTreeMap::from([("irony".into(), 2), ("detail".into(), 9)])));
        assert!(ironic.starts_with(DEFAULT_PERSONA) && ironic.contains("лёгкая ирония"));
        assert_eq!(ironic.lines().count(), 2, "a position out of range is the default: no line");
        assert_eq!(alice_tones().iter().map(|tone| tone.options.len()).collect::<Vec<_>>(), [3, 3, 3]);
    }

    #[test]
    fn reads_the_cards_of_before_as_alisas_positions_and_the_butler_as_the_catalogs_arthur() {
        let at = |tone: &str, position: usize| Some(Legacy::Alice(BTreeMap::from([(tone.to_string(), position)])));
        assert_eq!(legacy(Some("standard"), ""), Some(Legacy::Alice(BTreeMap::new())));
        assert_eq!(legacy(Some("irony"), ""), at("irony", 2));
        assert_eq!(legacy(Some("warm"), ""), at("warmth", 2));
        assert_eq!(legacy(Some("business"), ""), at("detail", 0));
        assert_eq!(legacy(Some("butler"), ""), Some(Legacy::Replaced("arthur")));
        let (_, warm) = LEGACY[2];
        assert_eq!(legacy(None, warm), at("warmth", 2), "a choice kept without its id, by its text");
        assert!(is_legacy_text(warm) && !is_legacy_text("Ты — Алиса, пиратка."));
        assert_eq!(legacy(Some(ALICE), ""), None);
        assert_eq!(legacy(Some("arthur:arthur"), ""), None);
        assert_eq!(legacy(None, "Ты — Алиса, пиратка."), None);
    }
}
