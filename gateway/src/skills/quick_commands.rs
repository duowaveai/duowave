//! Quick rules: frequent commands recognised without the model, at once and the same way every time. Patterns
//! are anchored on the whole command, so "включи свет на кухне" is not taken for "включи" anything else; what
//! they do not cover goes to the model with tools.
//!
//! The patterns are the TS ones on normalized text (lower case, single spaces). The `regex` crate has no
//! look-around: a look-ahead for a word's end is `( |$)` where only a test needs it, and the radio rule's
//! `(?!станц)` is checked in its build.

use std::sync::LazyLock;

use regex::{Captures, Regex};
use serde_json::Value;

use super::capabilities::{CapabilityName, Intent};
use super::js;
use super::skill::{intent, text_value};
use super::text::{find_number, normalize, parse_clock, parse_duration, read_number};
use crate::config::ModelProfile;

#[derive(Debug, Clone, PartialEq)]
pub struct QuickMatch {
    pub intent: Intent,
    /// The rule's name, for the journal.
    pub rule: &'static str,
}

type Build = fn(&Captures, &str) -> Option<Intent>;

struct Rule {
    name: &'static str,
    pattern: Regex,
    build: Build,
}

fn rule(name: &'static str, pattern: &str, build: Build) -> Rule {
    Rule { name, pattern: Regex::new(pattern).unwrap_or_else(|error| panic!("rule {name}: {error}")), build }
}

fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("a valid pattern")
}

fn fixed(capability: CapabilityName, action: &str) -> Option<Intent> {
    Some(Intent::new(capability, action))
}

/// A captured part trimmed, or none when empty.
fn rest(value: Option<&str>) -> Option<Value> {
    value.map(js::trim).filter(|text| !text.is_empty()).map(|text| Value::String(text.to_string()))
}

/// Words said with a timer that are not what it is for: «поставь таймер ещё на пять минут», «таймер мне на чай».
const NOT_TIMER_NAMES: &str = "еще снова опять обратно тоже также сразу заново теперь мне нам полтора полторы";

/// What a timer is for, when the part says it: not a number said without its unit («таймер на пять», «на 5»), not a
/// word of the request itself.
fn timer_name(part: &str) -> Option<String> {
    let part = js::trim(part);
    let words: Vec<&str> = part.split(' ').collect();
    let first = words.first().copied().unwrap_or("");
    let number = first.starts_with(|c: char| c.is_ascii_digit()) || read_number(&words, 0).is_some();
    let request = NOT_TIMER_NAMES.split(' ').any(|word| word == first);
    (!first.is_empty() && !number && !request).then(|| part.to_string())
}

/// A timer's length and its name from what follows «таймер на»: «чай на пять минут», «пять минут на чай», «пять минут
/// для стирки», «чай» (the length asked next), «пять минут». The duration parser skips words it does not know, so a part
/// is the length only when its other part says none. A number without its unit is no length and no name: «на пять»
/// asks how long, «чай на пять» asks it for «чай».
fn timer_parts(text: &str) -> (Option<u64>, Option<String>) {
    let text = js::trim(text);
    let seconds = |part: &str| parse_duration(part).filter(|seconds| *seconds > 0);
    for separator in [" для ", " на "] {
        if let Some((left, right)) = text.split_once(separator) {
            match (seconds(left), seconds(right)) {
                (None, Some(length)) => return (Some(length), timer_name(left)),
                (Some(length), None) => return (Some(length), timer_name(right)),
                _ => {}
            }
        }
    }
    if let Some(length) = seconds(text) {
        return (Some(length), None);
    }
    for separator in [" для ", " на "] {
        if let Some((left, right)) = text.split_once(separator) {
            match (timer_name(left), timer_name(right)) {
                (Some(name), None) | (None, Some(name)) => return (None, Some(name)),
                _ => {}
            }
        }
    }
    (None, timer_name(text))
}

fn group<'a>(m: &'a Captures, index: usize) -> Option<&'a str> {
    m.get(index).map(|g| g.as_str())
}

fn named<'a>(m: &'a Captures, name: &str) -> Option<&'a str> {
    m.name(name).map(|g| g.as_str())
}

fn number(value: f64) -> Option<Value> {
    Some(js::number_value(value))
}

/// «Громче», «чуть погромче», «ещё тише», «сделай тише на два», «убавь звук»; a repeated word is a step each:
/// «тише, тише, тише». Live, «чуть погромче» went to the model, which said it was louder and changed nothing.
fn relative_volume(word: &str, verbs: &str) -> String {
    format!(
        "^((сделай|можно|давай) )?((еще|чуть|чуть-чуть|немного|немножко|слегка|капельку) )*(по)?{word}\
         (?<again>( (по)?{word})*)( (на|в) (?<by>.+))?$|^({verbs}) (звук|громкость)( на (?<byVerb>.+))?$"
    )
}

fn volume_steps(m: &Captures) -> f64 {
    let again = named(m, "again").map(js::trim).filter(|again| !again.is_empty());
    find_number(named(m, "by").or(named(m, "byVerb")).unwrap_or(""))
        .unwrap_or_else(|| 1.0 + again.map_or(0.0, |again| again.split(' ').count() as f64))
}

static RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    use CapabilityName::*;
    vec![
        // Time and date: the most frequent question, answered without the model.
        rule(
            "time",
            "^(сколько (сейчас )?(времени|время)|который (сейчас )?час|скажи (который час|время|точное время)|какое (сейчас )?время)$",
            |_, _| fixed(Chat, "time"),
        ),
        rule(
            "date",
            "^(какое (сегодня |сейчас )?число|какая (сегодня )?дата|какой (сегодня )?день( недели)?|какой сегодня день)$",
            |_, _| fixed(Chat, "date"),
        ),
        // Volume.
        rule("louder", &relative_volume("громче", "прибавь|увеличь"), |m, _| {
            Some(intent(Volume, "louder", vec![("steps", number(volume_steps(m)))]))
        }),
        rule("quieter", &relative_volume("тише", "убавь|уменьши"), |m, _| {
            Some(intent(Volume, "quieter", vec![("steps", number(volume_steps(m)))]))
        }),
        rule(
            "volumeMax",
            "^((поставь|сделай|установи) )?(громкость )?(на )?(максимум|максимальную|полную)( громкость)?$",
            |_, _| fixed(Volume, "max"),
        ),
        rule("volumeSet", "^((поставь|сделай|установи) )?громкость (на )?(.+)$", |m, _| {
            let level = find_number(group(m, 4).unwrap_or(""))?;
            Some(intent(Volume, "set", vec![("level", number(level))]))
        }),
        // «Мьют»: live (02.11) it went to the conversation, where the model said it could not mute the speaker;
        // Yandex played a song called «мьют» for it, so it gets the plain words.
        rule("mute", "^(выключи|отключи) звук$|^без звука$", |_, _| fixed(Volume, "mute")),
        rule("muteSlang", "^(за|са)?мьют(ь|ни|и)?$|^mute$", |_, _| {
            Some(intent(Volume, "mute", vec![("yandexText", text_value(Some("Выключи звук.")))]))
        }),
        rule("unmute", "^(включи|верни) звук$", |_, _| fixed(Volume, "unmute")),
        rule("unmuteSlang", "^(раз|ан)мьют(ь|ни|и)?$|^unmute$", |_, _| {
            Some(intent(Volume, "unmute", vec![("yandexText", text_value(Some("Включи звук.")))]))
        }),
        rule("volumeStatus", "^какая (сейчас )?громкость$", |_, _| fixed(Volume, "status")),
        // Player. A stop said again before the station fell silent is one stop: «Стоп. Стоп.» (live, 12.11).
        rule(
            "stop",
            "^(стоп|stop|хватит|пауза|поставь на паузу|на паузу|останови(сь)?|перестань|прекрати|выключись|замолчи|выключи( музыку| радио| песню| это)?)( (стоп|stop|хватит))*$",
            |_, _| fixed(Playback, "stop"),
        ),
        // «Продолжу»: recognition's «продолжи» (live, 02.11).
        rule(
            "resume",
            "^(продолжи|продолжай|продолжим|продолжу|играй дальше|сними с паузы|включи обратно|продолжи (играть|воспроизведение))$",
            |_, _| fixed(Playback, "resume"),
        ),
        rule(
            "next",
            r"^(дальше|следующ(ий|ая|ую|ее|его)( трек| песня| песню| станция| станцию)?|(включи|давай) следующ\p{L}*( трек| песню| станцию)?|переключи|пропусти)$",
            |_, _| fixed(Playback, "next"),
        ),
        // «Проиграй ещё раз этот трек сначала» (live, 02.11): the model answered itself, the player was Yandex's.
        rule(
            "replay",
            "^((проиграй|включи|поставь|начни|сыграй|повтори|запусти|играй) )?((этот|эту|трек|песню|еще|раз|заново|сначала|с|начала)( |$))+$",
            |m, text| {
                let again = AGAIN.is_match(text) || REPEAT_TRACK.is_match(text);
                // «Ещё раз» or «сначала» alone may be about the answer, not the track.
                let about_track = group(m, 2).is_some_and(|verb| !verb.is_empty()) || TRACK.is_match(text);
                (again && about_track).then(|| Intent::new(Playback, "replay"))
            },
        ),
        rule(
            "previous",
            r"^(назад|предыдущ(ий|ая|ую|ее|его)( трек| песня| песню| станция| станцию)?|(включи|верни) предыдущ\p{L}*( трек| песню| станцию)?)$",
            |_, _| fixed(Playback, "previous"),
        ),
        // Bluetooth.
        rule(
            "bluetoothOn",
            "^(включи|активируй|запусти) (блютуз|блютус|bluetooth)$|^подключи (телефон|блютуз)$",
            |_, _| fixed(Bluetooth, "on"),
        ),
        rule("bluetoothOff", "^(выключи|отключи) (блютуз|блютус|bluetooth)$", |_, _| fixed(Bluetooth, "off")),
        // Timers. Pause before set: live, «поставь таймер на паузу» was taken for «поставь таймер на …» and asked
        // how long.
        rule(
            "timerPause",
            "^(поставь )?таймер на паузу$|^поставь на паузу таймер$|^приостанови таймер$",
            |_, _| fixed(Timers, "pause"),
        ),
        rule("timerResume", "^(продолжи|продолжай|возобнови) таймер$|^сними таймер с паузы$", |_, _| {
            fixed(Timers, "resume")
        }),
        rule("timerCancel", "^(отмени|выключи|отключи|удали|сбрось|останови) (все )?таймер(ы)?$", |_, _| {
            fixed(Timers, "cancel")
        }),
        rule("timerSet", "^((поставь|заведи|установи|запусти|включи) )?таймер на (.+)$|^засеки (.+)$", |m, _| {
            let (seconds, name) = timer_parts(group(m, 3).or(group(m, 4)).unwrap_or(""));
            Some(intent(Timers, "set", vec![("seconds", seconds.map(Value::from)), ("name", name.map(Value::from))]))
        }),
        // «Таймер стирка на сорок минут»: the name before «на». «Таймер ещё на пять минут» names nothing: it stays the
        // model's, as it was before timers had names.
        rule("timerNamed", "^((поставь|заведи|установи|запусти|включи) )?таймер ([а-яё-]+) на (.+)$", |m, _| {
            let seconds = parse_duration(group(m, 4).unwrap_or("")).filter(|seconds| *seconds > 0);
            let name = timer_name(group(m, 3).unwrap_or(""))?;
            Some(intent(Timers, "set", vec![("seconds", seconds.map(Value::from)), ("name", Some(Value::from(name)))]))
        }),
        rule(
            "timerStatus",
            "^сколько (осталось|времени осталось)( на таймере| до конца таймера| таймеру)?$|^сколько на таймере$",
            |_, _| fixed(Timers, "status"),
        ),
        // Alarms. Live, «поставь будильник через 2 минуты» went through the model: 1,6 s before Yandex got it.
        rule("alarmIn", "^((поставь|заведи|установи) )?будильник через (.+)$|^разбуди (меня )?через (.+)$", |m, _| {
            let seconds = parse_duration(group(m, 3).or(group(m, 5)).unwrap_or("")).filter(|seconds| *seconds > 0)?;
            Some(intent(Alarms, "set", vec![("inSeconds", Some(Value::from(seconds)))]))
        }),
        rule("alarmSet", "^((поставь|заведи|установи) )?будильник (на|в) (.+)$|^разбуди (меня )?(в|на) (.+)$", |m, _| {
            Some(match parse_clock(group(m, 4).or(group(m, 7)).unwrap_or("")) {
                Some(time) => intent(
                    Alarms,
                    "set",
                    vec![("hours", Some(Value::from(time.hours))), ("minutes", Some(Value::from(time.minutes)))],
                ),
                None => Intent::new(Alarms, "set"),
            })
        }),
        // «Отключи будильник на 8 утра»: live, the model took it for a question about the alarms.
        rule("alarmCancel", "^(отмени|выключи|отключи|удали|сними) (все )?будильник(и)?( (на|в) (.+))?$", |m, _| {
            let Some(when) = group(m, 6) else { return fixed(Alarms, "cancel") };
            let time = parse_clock(when)?;
            Some(intent(
                Alarms,
                "cancel",
                vec![("hours", Some(Value::from(time.hours))), ("minutes", Some(Value::from(time.minutes)))],
            ))
        }),
        // A lone «отложи» too: live 03.11, said while the alarm rang, it went to the model, which asked to say it
        // again.
        rule(
            "alarmSnooze",
            "^(отложи|отложить)( будильник( на (.+))?)?$|^(дай )?(еще|ещ) (пять|5|десять|10) минут( поспать)?$",
            |_, _| fixed(Alarms, "snooze"),
        ),
        rule(
            "alarmStatus",
            "^(какие|сколько) (у меня )?(сейчас )?будильник(и|ов)( (сейчас )?(установлен[оы]?|поставлен[оы]?|заведен[оы]?|стоят|есть))?$|^на (какое время|когда|сколько) (стоит|поставлен|заведен) будильник$",
            |_, _| fixed(Alarms, "status"),
        ),
        // Do not disturb.
        rule("dndOn", "^((включи|активируй) )?(режим )?не беспокоить$|^не беспокой( меня)?$", |_, _| {
            fixed(DoNotDisturb, "on")
        }),
        rule("dndOff", "^(выключи|отключи) (режим )?не беспокоить$", |_, _| fixed(DoNotDisturb, "off")),
        // Content.
        // «Включи радиостудия 21»: live (02.11) recognition glued the word to the station's name. The third
        // alternative is `радио(?!станц)…` in the TS: a name glued on that starts with «станц» is not taken.
        rule(
            "radio",
            r"^(включи|поставь|запусти|вруби) радио( (.+))?$|^(включи|поставь) (радио)?станцию (.+)$|^(включи|поставь|запусти|вруби) радио(\p{L}+( .+)?)$",
            |m, _| {
                if group(m, 8).is_some_and(|glued| glued.starts_with("станц")) {
                    return None;
                }
                Some(intent(Radio, "play", vec![("station", rest(group(m, 3).or(group(m, 6)).or(group(m, 8))))]))
            },
        ),
        // Ours plays what was liked here; Yandex, on its route, plays its own «Мне нравится».
        rule(
            "favorites",
            "^(включи|поставь|запусти|вруби) (мою |мои )?(любимое|любимую музыку|любимые песни|любимые треки|понравившееся|понравившиеся( песни| треки)?|избранное)$",
            |_, _| Some(intent(Music, "play", vec![("favorites", Some(Value::Bool(true)))])),
        ),
        rule("music", "^(включи|поставь|запусти|вруби) (музыку|песню|песни|трек|плейлист)( (.+))?$", |m, _| {
            Some(intent(Music, "play", vec![("query", rest(group(m, 4)))]))
        }),
        rule("tale", "^(расскажи|прочитай|включи|поставь) (мне )?сказку( (про|о|об) (.+))?$", |m, _| {
            Some(intent(Tales, "tell", vec![("topic", rest(group(m, 5)))]))
        }),
        rule("audiobook", "^(включи|поставь) аудиокнигу( (.+))?$", |m, _| {
            Some(intent(Tales, "audiobook", vec![("query", rest(group(m, 2)))]))
        }),
        // Lookups.
        // «Какая сейчас погода» too: live (02.11) it went to the model, and the temperature missed the display.
        rule(
            "weather",
            "^((какая|что с|расскажи|скажи)( (?<day>сейчас|сегодня|завтра|послезавтра))? )?погод(а|у|ой)( (?<asked>.+))?$|^(будет ли|ожидается ли|пойдет ли) (дождь|снег)( (?<rain>.+))?$|^(сколько|какая) (сейчас )?(градусов|температура)( на улице)?$",
            |m, _| {
                let parts: Vec<&str> =
                    [named(m, "day"), named(m, "asked").or(named(m, "rain"))].into_iter().flatten().collect();
                let when = parts.join(" ");
                Some(intent(Weather, "forecast", vec![("when", rest(Some(&when)))]))
            },
        ),
        // «Расскажи новость» too: live (02.11) it went to the model first, a second more.
        rule("news", "^((расскажи|включи|какие|скажи) )?новост(ь|и|ях|ей)( (.+))?$", |m, _| {
            Some(intent(News, "headlines", vec![("topic", rest(group(m, 5)))]))
        }),
        // Reminders.
        rule("reminderList", "^(какие у меня|мои) напоминания$", |_, _| fixed(Reminders, "list")),
        rule("reminderCancel", "^(отмени|удали) (все )?напоминани(е|я)$", |_, _| fixed(Reminders, "cancel")),
        // «через 10 минут выключи свет», «в 18:30 включи радио» (skills/own/later_skill.rs): the rest put off.
        rule("later", "^(через|завтра в|сегодня в|в) .+$", |_, text| crate::skills::own::later_skill::intent_of(text)),
        rule("laterCancelAll", "^(отмени|удали) все отложенн(ые|ое)( команды)?$", |_, _| fixed(Later, "cancelAll")),
        rule("laterCancel", "^(отмени|удали) отложенн(ое|ую)( команду)?$", |_, _| fixed(Later, "cancel")),
        rule("laterList", "^(что|какие команды) (у меня )?отложен(о|ы)$", |_, _| fixed(Later, "list")),
        rule("reminder", "^напомни (мне )?(.+)$", |m, _| {
            Some(intent(Reminders, "set", vec![("request", text_value(group(m, 2)))]))
        }),
        // Calls and voice messages.
        rule("callAnswer", "^(ответь|ответить|прими (звонок|вызов)|возьми трубку)$", |_, _| fixed(Calls, "answer")),
        rule(
            "callHangUp",
            "^(сбрось|отклони|положи трубку|заверши звонок|сбрось звонок|закончи звонок|отбой)$",
            |_, _| fixed(Calls, "hangup"),
        ),
        // «позвони маме ещё раз»: the contact without the «ещё раз» (live 02.11 it went to Yandex as «маме ещё
        // раз»); «перезвони»: live 02.11 it went to the model, which said it cannot call.
        rule(
            "call",
            "^(позвони|перезвони|набери|вызови|звони)( на телефон| по телефону)? (.+?)( ещё раз| еще раз| снова| опять| обратно)?$",
            |m, _| Some(intent(Calls, "call", vec![("contact", text_value(group(m, 3)))])),
        ),
        // «передай маме, что я задержусь»: the contact, and the message without «что» («Я задержусь»).
        rule("message", r"^(передай|отправь сообщение|напиши) (\p{L}+) (что )?(.+)$", |m, _| {
            let text = group(m, 4).unwrap_or("");
            let mut chars = text.chars();
            let capital: String = chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default();
            Some(intent(Calls, "message", vec![("contact", text_value(group(m, 2))), ("text", Some(Value::String(capital)))]))
        }),
        // Yandex's own.
        // Live 02.11: «Кто сейчас играет?», «Какая музыка сейчас играет?» went to the model, which said it could not.
        rule(
            "musicRecognition",
            "^(что|кто) (сейчас |это |там )?(играет|звучит|поет|исполняет)$|^что (это |сейчас )?за (песня|музыка|трек|мелодия)( сейчас)?( играет| звучит)?$|^как(ая|ой) (сейчас |это )?(песня|музыка|трек|мелодия)( сейчас)?( играет| звучит)?$|^(узнай|угадай|определи) (песню|музыку|мелодию|трек)$",
            |_, _| fixed(MusicRecognition, "recognize"),
        ),
        rule("morningShow", "^((включи|запусти|давай) )?(утреннее шоу|шоу алисы|утреннюю программу)$", |_, _| {
            fixed(MorningShow, "start")
        }),
        rule("skill", "^(запусти|открой|включи|вызови) навык (.+)$", |m, _| {
            Some(intent(Skills, "launch", vec![("name", text_value(group(m, 2)))]))
        }),
        rule("findPhone", "^(найди|где) (мой )?телефон$", |_, _| fixed(Account, "find_phone")),
        rule("shopping", "^(добавь|запиши) (.+) в (список )?покупок$|^(что|какой) (в )?списке покупок$", |_, _| {
            fixed(Account, "shopping_list")
        }),
        // Games.
        rule("game", "^(давай )?(поиграем|сыграем|играть)( в (.+))?$|^(запусти|начни|давай) игру( (.+))?$", |m, _| {
            Some(intent(Games, "start", vec![("game", rest(group(m, 4).or(group(m, 7))))]))
        }),
    ]
});

static AGAIN: LazyLock<Regex> = LazyLock::new(|| regex("(еще раз|заново|сначала|с начала)"));
static REPEAT_TRACK: LazyLock<Regex> = LazyLock::new(|| regex("^повтори (этот |эту )?(трек|песню)$"));
static TRACK: LazyLock<Regex> = LazyLock::new(|| regex("(трек|песню)"));
static THREE_LETTERS: LazyLock<Regex> = LazyLock::new(|| regex(r"^\p{L}{3}"));

/// What recognition sometimes leaves of the wake word before the command: «Алиса, стоп» heard as «состоп», «Алиса,
/// что играет» as «за что играет». Tried off only when the phrase as heard matches no rule.
const WAKE_REMNANTS: &[&str] = &["за ", "со ", "с ", "so ", "за", "со", "с"];

/// Words before the command that are not part of it: «ты продолжи» (live, 02.11), «ну стоп», «а сколько время».
const LEAD_WORDS: &[&str] = &["ты ", "ну ", "а ", "и ", "давай ", "слушай ", "ай ", "ой "];

/// A quick rule for the command, if one matches the whole of it.
pub fn match_quick_command(text: &str) -> Option<QuickMatch> {
    match_heard(text, |normalized| match_rules(normalized, &RULES))
}

/// A stop and nothing else, as recognition heard it («Стоп.», «Хватит», «Состоп», «Стоп. Стоп.»): interim recognition
/// ends the command on it without waiting for the end of speech (src/speech/interim_recognition.rs).
pub fn is_stop(text: &str) -> bool {
    match_quick_command(text).is_some_and(|found| found.rule == "stop")
}

/// A command as recognition heard it, through `matcher` (rules that take a whole normalized command): as heard,
/// then without what is left of the wake word, then without a word before the command. The plugins' phrases are
/// heard the same way as the built-in rules.
pub fn match_heard<T>(text: &str, matcher: impl Fn(&str) -> Option<T>) -> Option<T> {
    let normalized = normalize(text);
    if normalized.is_empty() {
        return None;
    }
    if let Some(heard) = matcher(&normalized) {
        return Some(heard);
    }
    for remnant in WAKE_REMNANTS {
        // A remnant glued to the word leaves a word of its own: «состоп» → «стоп», not «с» + «ол».
        let Some(rest) = normalized.strip_prefix(remnant) else { continue };
        if !THREE_LETTERS.is_match(rest) {
            continue;
        }
        if let Some(found) = matcher(rest) {
            return Some(found);
        }
    }
    for word in LEAD_WORDS {
        let Some(rest) = normalized.strip_prefix(word) else { continue };
        if let Some(found) = matcher(rest) {
            return Some(found);
        }
    }
    None
}

/// Under music recognition hears more than was said: «Алиса, дальше» came as «Что дальше?» twice (01.11). While
/// the player plays such a phrase is the player's; otherwise it stays the model's.
static WHILE_PLAYING: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    use CapabilityName::*;
    vec![
        // Yandex gets the command itself: «Что дальше?» it answered with «Не поняла».
        rule("nextWhilePlaying", "^(что|ну|а|и|давай) дальше$", |_, _| {
            Some(intent(Playback, "next", vec![("yandexText", text_value(Some("Дальше.")))]))
        }),
        // Only while it plays: otherwise «мне нравится» is a remark for the model (live 03.11 under the radio it
        // was taken for one, and nothing was liked).
        rule(
            "like",
            "^(мне (это |очень )?нравится|(поставь |ставлю )?лайк|лайкни( это)?|запомни (эту песню|этот трек))$",
            |_, _| fixed(Playback, "like"),
        ),
        rule(
            "dislike",
            "^((мне )?(это )?не нравится|(поставь )?дизлайк|дизлайкни( это)?)$",
            |_, _| fixed(Playback, "dislike"),
        ),
    ]
});

pub fn match_while_playing(text: &str) -> Option<QuickMatch> {
    match_rules(&normalize(text), &WHILE_PLAYING)
}

/// Under loud music or a ringing timer «Алиса, стоп» comes mangled: «Со стол?», «Со стола», «Олесоп», «Состо.»,
/// «Ай. Stop.» (live, 31.10 and 02.11). While something plays or rings, such a word alone is a stop (the timer's or
/// the alarm's while one rings: the router's interceptor).
static WHILE_BUSY: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    vec![rule(
        "stopWhileBusy",
        "^((ай|ой|а|ну|со|с|за|о) ?)?(стоп|stop|сто|соп|стоб|стол|стола)$|^[ао]л[еи]с[ао]?(с|ст)?оп$",
        |_, _| Some(intent(CapabilityName::Playback, "stop", vec![("yandexText", text_value(Some("Стоп.")))])),
    )]
});

pub fn match_while_busy(text: &str) -> Option<QuickMatch> {
    match_rules(&normalize(text), &WHILE_BUSY)
}

/// Not music after «включи»: the home, the station's settings, our other skills. These stay the model's.
static NOT_MUSIC: LazyLock<Regex> = LazyLock::new(|| {
    regex(
        "(^| )(свет|ламп|люстр|торшер|розетк|кондиционер|обогревател|батаре|увлажнител|очистител|пылесос|телевизор|чайник|вентилятор|сценари|устройств|подсветк|ночник|будильник|таймер|напомина|режим|звук|громк|тише|блютуз|bluetooth|радио|сказк|аудиокниг|новост|погод|навык|игр|шоу|следующ|предыдущ|заново|сначала|обратно|дальше)",
    )
});

/// «Включи Miyagi», «поставь что-нибудь спокойное»: something to play, by its name alone. Only while Yandex serves
/// music first (the router asks): it gets the request itself and knows artists, stations and the rest; without
/// the rule the model chose first, 1–1,7 s more (live, 02.11).
static PLAY_ANYTHING: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    vec![rule(
        "playAnything",
        "^(включи|включите|поставь|поставьте|вруби|хочу послушать|давай послушаем) (.+)$",
        |m, _| {
            let query = group(m, 2)?;
            if NOT_MUSIC.is_match(query) {
                return None;
            }
            Some(intent(CapabilityName::Music, "play", vec![("query", text_value(Some(query)))]))
        },
    )]
});

pub fn match_play_anything(text: &str) -> Option<QuickMatch> {
    match_rules(&normalize(text), &PLAY_ANYTHING)
}

fn match_rules(normalized: &str, rules: &[Rule]) -> Option<QuickMatch> {
    rules.iter().find_map(|rule| {
        let found = rule.pattern.captures(normalized)?;
        (rule.build)(&found, normalized).map(|intent| QuickMatch { intent, rule: rule.name })
    })
}

static COMMAND_START: LazyLock<Regex> = LazyLock::new(|| {
    regex(
        "^(включи|включите|выключи|выключите|отключи|поставь|поставьте|сделай|убавь|прибавь|увеличь|уменьши|напомни|позвони|перезвони|набери|отмени|запусти|открой|закрой|найди|разбуди|установи|заведи|останови|продолжи|переключи|добавь|удали|закажи|вызови|отправь|передай|запиши|убери|сбрось|вруби|выруби|смени|опусти|подними|зажги|погаси)( |$)",
    )
});
static WANTS_ACTION: LazyLock<Regex> = LazyLock::new(|| {
    regex(
        "^(хочу|можно|можешь|могла бы|давай)( ли)? (послушать|включить|поставить|выключить|услышать|запустить|сделать)",
    )
});
static ACTION_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    regex(
        "(таймер|будильник|громкост|громче|тише|радио|музык|песн|трек|свет|ламп|розетк|кондиционер|обогревател|увлажнител|пылесос|телевизор|блютуз|bluetooth|напомина|сказк|аудиокниг)",
    )
});
static QUESTION_START: LazyLock<Regex> =
    LazyLock::new(|| regex("^(что|как|почему|зачем|кто|когда|сколько|какой|какая|какое|какие|где|чем|чей|ли)( |$)"));

/// Whether a command for the station is likely (an imperative, a wish to hear something, words of station
/// capabilities outside a question). Then the model's choice of tool comes before any answer; otherwise the
/// answer is a conversation and starts at once with the jingle.
pub fn looks_like_command(text: &str) -> bool {
    let normalized = normalize(text);
    if COMMAND_START.is_match(&normalized) || WANTS_ACTION.is_match(&normalized) {
        return true;
    }
    ACTION_WORDS.is_match(&normalized) && !QUESTION_START.is_match(&normalized)
}

static HARD_QUESTION: LazyLock<Regex> = LazyLock::new(|| {
    regex(
        "^(почему|зачем|объясни|расскажи (подробно|про|о|об)|сравни|в чем разница|чем отличается|как (работает|устроен|сделать|приготовить|решить)|посоветуй|придумай|сочини|напиши|переведи|посчитай|реши|составь)( |$)",
    )
});

/// The model profile: smart for tools and hard questions, fast for short replies.
pub fn choose_profile(text: &str, command: bool) -> ModelProfile {
    if command {
        return ModelProfile::Smart;
    }
    let normalized = normalize(text);
    if HARD_QUESTION.is_match(&normalized) || normalized.split(' ').count() >= 14 {
        return ModelProfile::Smart;
    }
    ModelProfile::Fast
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn quick(text: &str) -> Option<Intent> {
        match_quick_command(text).map(|found| found.intent)
    }

    fn exact(capability: CapabilityName, action: &str, params: Value) -> Option<Intent> {
        Some(Intent { capability, action: action.into(), params: params.as_object().cloned().unwrap() })
    }

    fn is(text: &str, capability: CapabilityName, action: &str) {
        let found = quick(text).unwrap_or_else(|| panic!("{text}: no rule"));
        assert_eq!((found.capability, found.action.as_str()), (capability, action), "{text}");
    }

    fn param(text: &str, key: &str) -> Value {
        quick(text).unwrap_or_else(|| panic!("{text}: no rule")).params.get(key).cloned().unwrap_or(Value::Null)
    }

    use CapabilityName::*;

    #[test]
    fn knows_volume_commands() {
        assert_eq!(quick("Громче"), exact(Volume, "louder", json!({"steps": 1})));
        assert_eq!(quick("сделай громче на два"), exact(Volume, "louder", json!({"steps": 2})));
        is("потише", Volume, "quieter");
        assert_eq!(quick("Чуть погромче."), exact(Volume, "louder", json!({"steps": 1})));
        assert_eq!(quick("Ещё чуть-чуть тише"), exact(Volume, "quieter", json!({"steps": 1})));
        assert_eq!(quick("Тише, тише, тише."), exact(Volume, "quieter", json!({"steps": 3})));
        assert_eq!(quick("убавь звук на три"), exact(Volume, "quieter", json!({"steps": 3})));
        assert_eq!(quick("Громкость 7."), exact(Volume, "set", json!({"level": 7})));
        is("громкость на максимум", Volume, "max");
        is("выключи звук", Volume, "mute");
        // Live (02.11): «Мьют» went to the conversation.
        is("Мьют.", Volume, "mute");
        is("замьють", Volume, "mute");
        is("Самьют.", Volume, "mute");
        assert_eq!(param("Самьют.", "yandexText"), json!("Выключи звук."));
        is("размьють", Volume, "unmute");
    }

    #[test]
    fn knows_the_player() {
        is("Стоп.", Playback, "stop");
        is("Алиса, хватит", Playback, "stop");
        // «Алиса, стоп» said quickly: the wake word's «с» heard with the command.
        is("Состоп.", Playback, "stop");
        is("So stop.", Playback, "stop");
        is("Продолжу.", Playback, "resume");
        // Live (02.11): «ты» before the command; the track again from the start.
        is("Ты продолжи.", Playback, "resume");
        is("Ну стоп.", Playback, "stop");
        // Said again before the station fell silent (live, 12.11).
        is("Стоп. Стоп.", Playback, "stop");
        is("Хватит, хватит!", Playback, "stop");
        is("Проиграй ещё раз этот трек сначала.", Playback, "replay");
        is("включи эту песню заново", Playback, "replay");
        is("повтори трек", Playback, "replay");
        is("включи песню сначала", Playback, "replay");
        // About the answer, not the track.
        assert_eq!(quick("Ещё раз"), None);
        assert_eq!(quick("сначала"), None);
        assert_eq!(quick("Ты кто?"), None);
    }

    #[test]
    fn says_which_text_is_a_stop_and_nothing_else() {
        for text in ["Стоп.", "Хватит!", "Состоп.", "Стоп. Стоп.", "Замолчи.", "Ну стоп.", "Выключи музыку."]
        {
            assert!(is_stop(text), "{text}");
        }
        // More words after it, before it, or another command.
        for text in ["Стоп, а какая погода?", "Отлично. Стоп. Стоп.", "Стоп-кран", "Продолжи.", "Сто.", ""]
        {
            assert!(!is_stop(text), "{text}");
        }
    }

    #[test]
    fn likes_what_plays_only_while_it_plays_and_plays_what_was_liked_live_03_11() {
        let playing =
            |text: &str| match_while_playing(text).map(|found| (found.intent.capability, found.intent.action));
        for like in ["Мне нравится.", "мне очень нравится", "Поставь лайк", "лайк", "лайкни", "запомни эту песню"]
        {
            assert_eq!(playing(like), Some((Playback, "like".to_string())), "{like}");
        }
        for dislike in ["Мне не нравится.", "не нравится", "дизлайк", "поставь дизлайк"]
        {
            assert_eq!(playing(dislike), Some((Playback, "dislike".to_string())), "{dislike}");
        }
        // With nothing playing it is a remark for the model.
        assert_eq!(quick("Мне нравится."), None);
        assert_eq!(playing("Мне нравится твой голос"), None);
        for favorites in
            ["Включи любимое", "включи мою любимую музыку", "поставь понравившиеся песни", "включи избранное"]
        {
            assert_eq!(quick(favorites), exact(Music, "play", json!({"favorites": true})), "{favorites}");
        }
    }

    #[test]
    fn takes_off_what_is_left_of_the_wake_word_only_when_the_phrase_as_heard_matches_nothing() {
        is("За что сейчас играет?", MusicRecognition, "recognize");
        is("Сдальше", Playback, "next");
        assert_eq!(quick("За что?"), None);
        assert_eq!(quick("Сохрани это"), None);
        assert_eq!(quick("С днём рождения"), None);
        is("Сколько времени", Chat, "time");
        is("продолжи", Playback, "resume");
        is("следующий трек", Playback, "next");
        is("назад", Playback, "previous");
    }

    #[test]
    fn knows_timers_alarms_and_do_not_disturb() {
        assert_eq!(quick("Поставь таймер на 5 минут"), exact(Timers, "set", json!({"seconds": 300})));
        assert_eq!(quick("таймер на полчаса"), exact(Timers, "set", json!({"seconds": 1800})));
        is("отмени таймер", Timers, "cancel");
        is("через 10 минут выключи свет", Later, "set");
        is("в 18:30 включи радио", Later, "set");
        is("отмени отложенное", Later, "cancel");
        is("отмени все отложенные", Later, "cancelAll");
        is("что отложено", Later, "list");
        is("сколько осталось", Timers, "status");
        assert_eq!(quick("Поставь будильник на 7:30"), exact(Alarms, "set", json!({"hours": 7, "minutes": 30})));
        assert_eq!(quick("разбуди меня через 2 минуты"), exact(Alarms, "set", json!({"inSeconds": 120})));
        is("не беспокоить", DoNotDisturb, "on");
    }

    #[test]
    fn names_a_timer_said_with_what_it_is_for() {
        let set = |seconds: Option<u64>, name: &str| exact(Timers, "set", json!({"seconds": seconds, "name": name}));
        assert_eq!(quick("Поставь таймер на чай на пять минут"), set(Some(300), "чай"));
        assert_eq!(quick("таймер на пять минут на чай"), set(Some(300), "чай"));
        assert_eq!(quick("таймер на сорок минут для стирки"), set(Some(2400), "стирки"));
        assert_eq!(quick("поставь таймер стирка на сорок минут"), set(Some(2400), "стирка"));
        // Only the name: the length is asked next, the name waits for it.
        assert_eq!(quick("поставь таймер на чай"), exact(Timers, "set", json!({"name": "чай"})));
        // No name: as before.
        assert_eq!(quick("Поставь таймер на 5 минут"), exact(Timers, "set", json!({"seconds": 300})));
    }

    /// Review G10: a number said without its unit is no name — «На сколько поставить таймер?», not «…таймер «пять»?»;
    /// nor is a word of the request itself.
    #[test]
    fn names_a_timer_neither_by_a_bare_number_nor_by_a_word_of_the_request() {
        assert_eq!(quick("Поставь таймер на пять"), exact(Timers, "set", json!({})));
        assert_eq!(quick("таймер на 5"), exact(Timers, "set", json!({})));
        assert_eq!(quick("засеки двадцать пять"), exact(Timers, "set", json!({})));
        // The name stays, the length is asked again.
        assert_eq!(quick("поставь таймер на чай на пять"), exact(Timers, "set", json!({"name": "чай"})));
        assert_eq!(quick("таймер на пять для стирки"), exact(Timers, "set", json!({"name": "стирки"})));
        // «Ещё», «снова», «мне»: the model's, as before timers had names.
        for text in ["Поставь таймер ещё на пять минут", "таймер снова на 10 минут", "поставь таймер мне на пять минут"]
        {
            assert_eq!(quick(text), None, "{text}");
        }
    }

    #[test]
    fn takes_the_timer_and_alarm_phrases_of_the_live_check_02_11() {
        // «Поставь таймер на паузу» was a timer to set, «на сколько?» in a loop.
        assert_eq!(quick("Поставь таймер на паузу."), exact(Timers, "pause", json!({})));
        is("Таймер на паузу.", Timers, "pause");
        is("Продолжи таймер.", Timers, "resume");
        is("Отключи таймер.", Timers, "cancel");
        assert_eq!(quick("Поставь будильник через 2 минуты."), exact(Alarms, "set", json!({"inSeconds": 120})));
        assert_eq!(
            quick("Отключи будильник на 8:00 утра."),
            exact(Alarms, "cancel", json!({"hours": 8, "minutes": 0}))
        );
        assert_eq!(quick("Выключи будильник"), exact(Alarms, "cancel", json!({})));
        assert_eq!(quick("Отмени будильник на завтра"), None);
        is("Какие будильники сейчас установлены?", Alarms, "status");
        is("Отложи будильник.", Alarms, "snooze");
        is("Отложи.", Alarms, "snooze");
        assert_eq!(quick("Отложи на завтра"), None);
    }

    #[test]
    fn knows_content_lookups_calls_and_yandex_services() {
        assert_eq!(quick("Включи радио Студия 21"), exact(Radio, "play", json!({"station": "студия 21"})));
        assert_eq!(quick("включи радио"), exact(Radio, "play", json!({})));
        // Recognition glued the words (live, 02.11).
        assert_eq!(quick("Включи радиостудия 21."), exact(Radio, "play", json!({"station": "студия 21"})));
        assert_eq!(quick("включи радиостанцию европа плюс"), exact(Radio, "play", json!({"station": "европа плюс"})));
        assert_eq!(quick("расскажи сказку про ежика"), exact(Tales, "tell", json!({"topic": "ежика"})));
        assert_eq!(quick("какая погода завтра"), exact(Weather, "forecast", json!({"when": "завтра"})));
        assert_eq!(quick("Какая сейчас погода?"), exact(Weather, "forecast", json!({"when": "сейчас"})));
        assert_eq!(
            quick("какая завтра погода в москве"),
            exact(Weather, "forecast", json!({"when": "завтра в москве"}))
        );
        assert_eq!(quick("какая погода"), exact(Weather, "forecast", json!({})));
        assert_eq!(quick("будет ли дождь завтра"), exact(Weather, "forecast", json!({"when": "завтра"})));
        // Live (02.11): «новость» went to the model first.
        assert_eq!(quick("Расскажи новость."), exact(News, "headlines", json!({})));
        assert_eq!(quick("какие новости про футбол"), exact(News, "headlines", json!({"topic": "про футбол"})));
        assert_eq!(
            quick("напомни через минуту выключить чайник"),
            exact(Reminders, "set", json!({"request": "через минуту выключить чайник"}))
        );
        is("ответь", Calls, "answer");
        assert_eq!(quick("позвони маме"), exact(Calls, "call", json!({"contact": "маме"})));
        let mother = Regex::new("^мам[еу]$").unwrap();
        for again in [
            "позвони маме ещё раз",
            "позвони маме еще раз",
            "набери маму снова",
            "позвони маме обратно",
            "перезвони маме ещё раз",
        ] {
            let contact = param(again, "contact");
            assert!(mother.is_match(contact.as_str().unwrap()), "{again}: {contact}");
        }
        assert_eq!(
            quick("Передай маме, что я задержусь."),
            exact(Calls, "message", json!({"contact": "маме", "text": "Я задержусь"}))
        );
        assert_eq!(param("напиши папе ужин готов", "contact"), json!("папе"));
        assert_eq!(param("напиши папе ужин готов", "text"), json!("Ужин готов"));
        is("что сейчас играет", MusicRecognition, "recognize");
        is("запусти навык угадай мелодию", Skills, "launch");
        is("который час", Chat, "time");
    }

    #[test]
    fn asks_yandex_what_plays_however_it_is_asked_live_02_11() {
        for text in [
            "Кто сейчас играет?",
            "Какая музыка сейчас играет?",
            "Какая это песня?",
            "Что за песня сейчас играет?",
            "Что звучит?",
            "Кто это поёт?",
            "Какой трек играет?",
            "Определи песню.",
        ] {
            is(text, MusicRecognition, "recognize");
        }
        for text in ["Какая песня у тебя любимая?", "Во что сейчас играем?", "Кто играет в финале?"]
        {
            assert_eq!(quick(text), None, "{text}");
        }
    }

    #[test]
    fn leaves_the_rest_to_the_model() {
        assert_eq!(quick("включи свет на кухне"), None);
        assert_eq!(quick("почему небо голубое"), None);
        assert_eq!(quick("как дела"), None);
    }

    #[test]
    fn tells_commands_from_conversation_and_picks_the_model_profile() {
        assert!(looks_like_command("включи свет на кухне"));
        assert!(looks_like_command("хочу послушать что-нибудь спокойное"));
        assert!(!looks_like_command("почему небо голубое"));
        assert!(!looks_like_command("сколько стоит таймер на кухню"));
        assert!(looks_like_command("говори потише"));
        assert_eq!(choose_profile("как дела", false), ModelProfile::Fast);
        assert_eq!(choose_profile("почему небо голубое", false), ModelProfile::Smart);
        assert_eq!(choose_profile("включи свет", true), ModelProfile::Smart);
    }

    #[test]
    fn checks_the_glued_radio_name_as_the_look_ahead_did() {
        // «радиостанция …» without «включи радиостанцию»: neither a station's name nor the station rule.
        assert_eq!(quick("включи радиостанция"), None);
        assert_eq!(quick("вруби радиорекорд"), exact(Radio, "play", json!({"station": "рекорд"})));
        assert_eq!(rule_of("Включи радиостудия 21."), Some("radio"));
    }

    fn rule_of(text: &str) -> Option<&'static str> {
        match_quick_command(text).map(|found| found.rule)
    }
}
