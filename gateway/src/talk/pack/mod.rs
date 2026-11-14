//! The language packs of the talk: the words that depend on the language, one file per language. The assistant of a
//! talk without a station — its name, who it is and how it speaks on a computer (docs/plans/mic-mode.md, «Подсказка
//! без колонки») — and the talk layer's words (docs/plans/audio-interface.md, «Правила работы и пакет языка»): the
//! block «Сейчас», the ready phrases of the works, the words its rules catch. The frame knows no words: whatever a
//! rule or a phrase says comes from here. The ready phrases are impersonal or in the future tense — the character may
//! be of any gender and have any name. A language without a pack takes the Russian one: the household's first
//! language.

mod en;
mod ru;

use crate::memory::text::Lang;

pub struct Pack {
    /// The built-in assistant's name where no station gives it one.
    pub name: &'static str,
    /// The built-in assistant's card in the panel.
    pub description: &'static str,
    /// Who the built-in assistant is: the first line of its prompt, before its regulators' lines.
    pub persona: &'static str,
    /// How any character speaks on a computer: every answer is said aloud, the microphone opens by the button.
    pub voice_rules: &'static [&'static str],
    /// What a computer has not: said after the rules.
    pub here: &'static str,
    /// The talk layer's words: the works.
    pub work: &'static WorkWords,
    /// MagicPlace by voice (talk/store.rs).
    pub store: &'static StoreWords,
}

/// MagicPlace by voice (talk/store.rs, docs/plans/magicplace.md, «Голос»): regular expressions over the normalized
/// phrase (as `RuleWords`), and what is said. The other names of a store — a phone's, the people's — are for the voice
/// only, never shown: they are other companies' marks; the screen says MagicPlace. A recognizer may write them in
/// Latin letters: those are a pack's words too.
pub struct StoreWords {
    /// «магазин», «открой плей маркет», «зайди в app store».
    pub open: &'static [&'static str],
    /// «поставь приложение лента», «установи ленту из магазина»: `name` — the store's, whatever the name.
    pub install: &'static [&'static str],
    /// «поставь персонажа артур»: `name`, a character — the store's, whatever the name.
    pub character: &'static [&'static str],
    /// «установи ленту», «скачай ленту»: `name` — the store's only when it names a package of the catalog
    /// («установи будильник» is the alarm's); one installed here is «уже стоит».
    pub install_maybe: &'static [&'static str],
    /// «поставь ленту»: `name` — the store's only when it names a package of the catalog not installed here: «поставь»
    /// also plays («поставь радио», «поставь сказку») and sets (a timer, an alarm).
    pub put_maybe: &'static [&'static str],
    /// «что обновилось», «есть обновления»: the new versions, and the question.
    pub updates: &'static [&'static str],
    /// «обнови всё»: updates at once, no question — it was asked.
    pub update_all: &'static [&'static str],
    /// «что нового в магазине».
    pub new: &'static [&'static str],
    pub say: StoreSay,
}

/// What MagicPlace says by voice. Impersonal: the character may be of any gender and name.
pub struct StoreSay {
    /// The store is open in the panel.
    pub opened: &'static str,
    /// No panel is open: where the store is.
    pub where_is: &'static str,
    /// `{title} {about} {downloads} {rights}`: the question before an install.
    pub ask_install: &'static str,
    /// `{list}`: what it will download, said after `about`.
    pub downloads: &'static str,
    /// `{title} {mb}`: one download, at once.
    pub download_now: &'static str,
    /// `{title} {mb}`: one download, at its first use.
    pub download_later: &'static str,
    /// Code asks its rights itself, in its window.
    pub rights: &'static str,
    /// `{title}`: installed.
    pub installed: &'static str,
    /// `{title} {why}`.
    pub install_failed: &'static str,
    /// The answer was «нет».
    pub declined: &'static str,
    /// `{title}`: it is here already.
    pub already: &'static str,
    /// `{title} {version}`: here already, and a newer version is out.
    pub already_newer: &'static str,
    /// After `already_newer`: the question.
    pub update_one: &'static str,
    /// `{title} {why}`: it does not go here.
    pub not_here: &'static str,
    /// `{name}`: the catalog has no such thing.
    pub not_found: &'static str,
    /// Said after `not_found` when the panel opened the store.
    pub look_in_panel: &'static str,
    /// `{title}`: installing by voice is off — the panel's window is open.
    pub in_panel: &'static str,
    /// `{title}`: installing by voice is off, and no panel is open.
    pub in_panel_closed: &'static str,
    /// No updates.
    pub fresh: &'static str,
    /// `{list}`: new versions here.
    pub updates: &'static str,
    /// After `updates`: the question.
    pub update_all: &'static str,
    /// After `updates` or `already_newer` when updating by voice is off: where to update.
    pub update_in_panel: &'static str,
    /// `{list}`: their new versions wait for a newer DuoWave.
    pub waits: &'static str,
    /// `{list}`: updated.
    pub updated: &'static str,
    /// `{list}`: not updated.
    pub update_failed: &'static str,
    /// Nothing new in a month.
    pub nothing_new: &'static str,
    /// `{list}`: what is new.
    pub new: &'static str,
    /// The catalog is off or not set up here.
    pub catalog_off: &'static str,
    /// Joins the last two of a list: « и ».
    pub and: &'static str,
    /// «мегабайт» in the forms a number takes (`SayWords::plural`): sizes are said in words, not «МБ».
    pub megabytes: [&'static str; 3],
    /// The quotes a title takes in a list: «Лента».
    pub quotes: (&'static str, &'static str),
}

/// The faces of a thing the workshop makes, in the order of `talk::works::Face`: a widget, a skill, a game, a theme.
pub type Faces = [&'static str; 4];

/// What the agent is doing now, in a few words.
pub struct DoingWords {
    pub thinking: &'static str,
    pub writing: &'static str,
    pub reading: &'static str,
    pub checking: &'static str,
    pub trying: &'static str,
    pub phrase: &'static str,
    pub docs: &'static str,
    pub journal: &'static str,
}

/// How a work ended, as the block says it after «—».
pub struct EndWords {
    /// The thing is gone: removed from the desk and the drafts.
    pub removed: &'static str,
    pub desk: &'static str,
    pub fitting: &'static str,
    pub parked: &'static str,
    pub declined: &'static str,
    pub done: &'static str,
    pub stopped: &'static str,
    pub failed: &'static str,
    pub interrupted: &'static str,
    pub unfinished: &'static str,
}

/// The works' words. Templates take `{name}` slots (`fill`).
pub struct WorkWords {
    /// The block's first line: what the brain is told to keep to itself unless asked.
    pub head: &'static str,
    /// A work's label in the block: `[р3]`.
    pub label: &'static str,
    /// «виджет», «навык», «игра», «тема».
    pub faces: Faces,
    /// A new thing's name until its maker names it: «Новый виджет» — never the request itself («Сделай часы
    /// секундами» became a widget's name, live 14.11).
    pub unnamed: Faces,
    /// `{label} {face} {title}`: it waits in the queue.
    pub queued: &'static str,
    /// `{label} {face} {title} {since} {doing}`.
    pub working: &'static str,
    /// `{asks}`: what was asked and added, after a working line.
    pub asked: &'static str,
    /// `{text}`: one ask in its quotes.
    pub ask: &'static str,
    /// Before a note among the asks: «+ кнопка произношения».
    pub added: &'static str,
    /// `{label} {face} {title} {ago} {said}`: ready, its question not answered yet.
    pub awaiting: &'static str,
    /// `{label} {face} {title} {ago} {said}`: not finished, «Продолжить?» not answered.
    pub unfinished: &'static str,
    /// `{label} {face} {title} {ago}`: cut by a restart.
    pub interrupted: &'static str,
    /// Whether the result was said to the end.
    pub said: &'static str,
    pub unsaid: &'static str,
    /// `{label} {face} {title} {end}`.
    pub recent: &'static str,
    pub ends: EndWords,
    /// `{list}`: the apps on the desk.
    pub desk: &'static str,
    /// `{n}`: how many are on the desk when the list is cut.
    pub more: &'static str,
    pub door: &'static str,
    /// `{n}`: a duration.
    pub seconds: &'static str,
    pub minutes: &'static str,
    pub hours: &'static str,
    pub days: &'static str,
    pub doing: DoingWords,
    /// The ready phrases of the works: said as they are, without a round of the model.
    pub say: SayWords,
    /// The words the layer's rules catch (talk/rules.rs): regular expressions over the normalized phrase — lower
    /// case, «ё» as «е», no punctuation. Named groups: `thing` (what it is about), `text` (the words added), `what`
    /// (the face asked for), `ask` (the wish).
    pub rules: RuleWords,
}

/// The rules' words.
pub struct RuleWords {
    /// «как там виджет», «что с испанским», «долго еще»: how the work goes; `thing` when named.
    pub status: &'static [&'static str],
    /// «и еще кнопку…», «а также…»: words added to the thing in focus; `text`.
    pub add: &'static [&'static str],
    /// «примерить», «покажи виджет»; `thing`.
    pub show: &'static [&'static str],
    /// «на стол», «поставь испанский»; `thing`.
    pub desk: &'static [&'static str],
    /// «подробнее», «что ты сделала».
    pub details: &'static [&'static str],
    /// «отмени виджет», «останови испанский»; `thing`.
    pub stop: &'static [&'static str],
    /// «мастерская», «открой мастерскую».
    pub door_open: &'static [&'static str],
    /// «выйти из мастерской», «закрой мастерскую».
    pub door_close: &'static [&'static str],
    /// The door's words in any order and with a word or two more («зайди в мастерскую»).
    pub door_open_any: Words,
    /// The exit in any order and with a word or two more: the open door took «выйди из режима мастерской» for a wish
    /// (live 14.11).
    pub door_close_any: Words,
    /// The old template «сделай виджет: …»: `what`, `ask`.
    pub start: &'static [&'static str],
    /// The words of things and the face each is (its place in `Faces`): «виджет», «игру», «приложение»…
    pub things: &'static [(&'static str, usize)],
    /// «мне», «нам»: whom a thing is made for, after «напиши».
    pub for_us: &'static [&'static str],
    /// The answers to the layer's question, said without the wake word.
    pub answers: AnswerWords,
}

/// A command in any order of its words: every group has a word that begins with one of its stems, and at most `extra`
/// other words come with them («выйди из режима мастерской»: «выйд» and «мастерск», two more).
pub struct Words {
    pub all: &'static [&'static [&'static str]],
    pub extra: usize,
}

impl Words {
    /// Whether the words (normalized, lower case) are this command.
    pub fn matches(&self, text: &str) -> bool {
        let words: Vec<&str> = text.split_whitespace().collect();
        if self.all.is_empty() || words.is_empty() {
            return false;
        }
        let mut used = vec![false; words.len()];
        for group in self.all {
            let found = (0..words.len()).find(|&at| !used[at] && group.iter().any(|stem| words[at].starts_with(stem)));
            let Some(at) = found else { return false };
            used[at] = true;
        }
        used.iter().filter(|used| !**used).count() <= self.extra
    }
}

/// The answers to «На стол или примерить?», «Продолжить?», «Применить?», «Удалить черновик?»; what none of them takes
/// is not the question's: the phrase is read as any other.
pub struct AnswerWords {
    /// «да», «на стол», «поставь».
    pub yes: &'static [&'static str],
    /// «примерить», «покажи».
    pub fit: &'static [&'static str],
    /// «нет», «потом».
    pub no: &'static [&'static str],
    /// «в черновики».
    pub park: &'static [&'static str],
    /// «подробнее».
    pub details: &'static [&'static str],
    /// «переделай…», «а слова сделай покрупнее»: `text`.
    pub redo: &'static [&'static str],
    /// «не надо», «удали».
    pub drop: &'static [&'static str],
    /// «новую», «другую»: to «Поправить … или новую?» — a new thing.
    pub new: &'static [&'static str],
    /// «поправь», «эту»: to «Поправить … или новую?» — a new version of that thing.
    pub edit: &'static [&'static str],
    /// To «Какую: … или …?»: the first, the second, the third, the last named.
    pub choices: [&'static str; 4],
}

/// A duration said aloud: the number, then the unit in its form for that number (`plural`).
pub struct SpokenUnits {
    pub seconds: [&'static str; 3],
    pub minutes: [&'static str; 3],
    pub hours: [&'static str; 3],
}

/// The works' ready phrases. Impersonal or in the future tense: the character may be of any gender and name.
pub struct SayWords {
    /// `{title}`: a new work began.
    pub started: &'static str,
    /// `{title}`: the same, the first time a day on a device — with how to add on the way, in the owner's words
    /// (docs/plans/audio-interface.md, «Доводка после проверки руками 14.11», 6).
    pub started_first: &'static str,
    /// `{text}`: words added to the work going on.
    pub noted: &'static str,
    /// `{title} {version}`: a new version of a finished thing began — which thing, which version: «непонятно, правит ли
    /// она предыдущую» (live 14.11).
    pub again: &'static str,
    /// `{since} {doing}`: how the work goes.
    pub working: &'static str,
    /// `{title}`: the thing is ready.
    pub ready: &'static str,
    /// Nothing in work and nothing talked about lately.
    pub nothing: &'static str,
    /// `{title}`: stopped.
    pub stopped: &'static str,
    /// The workshop's door: open (a question), closed.
    pub door_open: &'static str,
    pub door_closed: &'static str,
    /// `{title}`: a thing not in work any more was asked to stop.
    pub not_going: &'static str,
    /// The panel opened «Примерка»; no panel is open — the tile waits on the desk.
    pub shown: &'static str,
    pub tile_waits: &'static str,
    /// In the drafts.
    pub parked: &'static str,
    /// Put on the desk only in the panel: its window is open, or the tile waits.
    pub approve_open: &'static str,
    pub approve_tile: &'static str,
    /// Nothing more was said of it.
    pub no_details: &'static str,
    /// What to make, or what to add: the words were not heard.
    pub what: &'static str,
    /// A scrap of speech or a word that says nothing to do («Ос»): not a wish.
    pub noise: &'static str,
    /// «Сделать свою копию?» — no.
    pub not_copied: &'static str,
    /// `{title}`: the thing named is gone (deleted) — make it again?
    pub gone: &'static str,
    /// «Сделать заново?» — no.
    pub not_remade: &'static str,
    /// `{title}`: a run ended without changing the thing — not «готово».
    pub unchanged_line: &'static str,
    /// «На стол» while the thing is being made: it goes there once it is ready.
    pub desk_later: &'static str,
    /// The limit of a day: ten new things by voice.
    pub enough: &'static str,
    /// `{title}`: two go on already — this one waits in the queue and starts next.
    pub queued_next: &'static str,
    /// `{in} {title}`: a new one by voice once in two minutes — this one starts by itself in so long.
    pub queued_in: &'static str,
    /// `{title}`: how a work in the queue goes.
    pub waiting: &'static str,
    /// `{title}`: a thing alike of the last half hour — change it, or make a new one?
    pub similar: &'static str,
    /// `{list}`: which of these; `or` joins them.
    pub which: &'static str,
    pub or: &'static str,
    /// `{title}`: a built-in app is not changed — one's own copy?
    pub built_in: &'static str,
    /// `{why}`: it did not work out.
    pub failed: &'static str,
    /// The line of a finished work (docs/plans/audio-interface.md, «Честное «Готово»»): `{title}` and the parts
    /// after it — what it needs, where it will go, the question.
    pub ready_line: &'static str,
    /// What it needs to be filled in: the needs' titles («ключ Todoist») into a sentence.
    pub needs: fn(&[String]) -> String,
    /// `{hosts}`: the hosts it will reach that it did not before.
    pub hosts: &'static str,
    /// The question after a new thing is ready.
    pub desk_or_fit: &'static str,
    /// `{title}`: the run ended half-way.
    pub unfinished_line: &'static str,
    /// `{title}`: it did not work out.
    pub failed_line: &'static str,
    /// `{title} {text}`: a new version of a thing on the desk — then «На столе.» when it went there at once, or the
    /// question `apply` when it asks for more.
    pub changed: &'static str,
    pub apply: &'static str,
    /// On the desk; on the desk but it waits for `{needs}` to be filled in — the panel's window open, or not.
    pub on_desk: &'static str,
    pub on_desk_needs_open: &'static str,
    pub on_desk_needs_tile: &'static str,
    /// Rights only the panel approves: its window is open, or the tile waits.
    pub dangerous_open: &'static str,
    pub dangerous_tile: &'static str,
    /// «нет» after the question: it stays to be tried on.
    pub kept: &'static str,
    /// `{title} {ask}`: the question of a work nobody answered, asked once more after the device's next talk
    /// (docs/plans/audio-interface-2.md, «К3»).
    pub remind: &'static str,
    /// «удали»: asked first; deleted; kept.
    pub delete_q: &'static str,
    pub deleted: &'static str,
    pub not_deleted: &'static str,
    /// The wish of a run that finishes what the last one left: «доделай и проверь».
    pub finish_brief: &'static str,
    /// The constant phrases kept ahead in the household's own voice (src/speech/tts/phrase_cache.rs): a layer's line
    /// that begins with one says it at once.
    pub kept_ahead: &'static [&'static str],
    /// A duration aloud.
    pub units: SpokenUnits,
    /// Which form of a unit a number takes: 0 — one, 1 — few, 2 — many.
    pub plural: fn(u64) -> usize,
}

impl SayWords {
    /// «сорок секунд», «2 минуты», «1 hour».
    pub fn duration(&self, ms: u64) -> String {
        let seconds = ms / 1000;
        let (forms, n) = match seconds {
            0..=59 => (self.units.seconds, seconds.max(1)),
            60..=3599 => (self.units.minutes, seconds / 60),
            _ => (self.units.hours, seconds / 3600),
        };
        format!("{n} {}", forms[(self.plural)(n)])
    }
}

pub fn of(lang: Lang) -> &'static Pack {
    match lang {
        Lang::En => &en::PACK,
        Lang::Ru | Lang::Other => &ru::PACK,
    }
}

/// A template with its slots filled: `{name}` → its value; a slot without a value stays as it is.
pub fn fill(template: &str, values: &[(&str, &str)]) -> String {
    let mut text = template.to_string();
    for (name, value) in values {
        text = text.replace(&format!("{{{name}}}"), value);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pack_names_the_assistant_and_none_names_a_station() {
        assert_eq!((of(Lang::Ru).name, of(Lang::En).name, of(Lang::Other).name), ("ДиВи", "Deevi", "ДиВи"));
        for pack in [of(Lang::Ru), of(Lang::En)] {
            let all = format!("{}\n{}\n{}\n{}", pack.description, pack.persona, pack.voice_rules.join("\n"), pack.here);
            for word in ["Алис", "колонк", "Яндекс", "Alice", "speaker", "station", "Yandex"] {
                assert!(!all.contains(word), "{word} in the pack of {}", pack.name);
            }
            assert!(pack.persona.contains(pack.name));
        }
    }

    #[test]
    fn says_a_duration_in_the_form_its_number_takes() {
        let ru = &of(Lang::Ru).work.say;
        let said: Vec<String> = [1_000, 2_000, 5_000, 21_000, 40_000, 60_000, 180_000, 660_000, 3_600_000]
            .iter()
            .map(|ms| ru.duration(*ms))
            .collect();
        assert_eq!(
            said,
            [
                "1 секунду",
                "2 секунды",
                "5 секунд",
                "21 секунду",
                "40 секунд",
                "1 минуту",
                "3 минуты",
                "11 минут",
                "1 час"
            ]
        );
        let en = &of(Lang::En).work.say;
        assert_eq!(
            (en.duration(1_000), en.duration(40_000), en.duration(7_200_000)),
            ("1 second".into(), "40 seconds".into(), "2 hours".into())
        );
    }

    #[test]
    fn fills_the_slots_of_a_template() {
        assert_eq!(
            fill("- В работе [{label}]: {title}", &[("label", "р3"), ("title", "Испанский")]),
            "- В работе [р3]: Испанский"
        );
        assert_eq!(fill("{a} и {b}", &[("a", "раз")]), "раз и {b}", "a slot without a value stays");
    }

    /// Every template of every pack has the slots its frame fills, and no other: a translation that drops one
    /// loses a word, one that invents one shows its braces.
    #[test]
    fn every_pack_has_the_slots_of_every_template() {
        let slots = |text: &str| -> Vec<String> {
            let mut found = Vec::new();
            let mut rest = text;
            while let Some(start) = rest.find('{') {
                let Some(end) = rest[start..].find('}') else { break };
                found.push(rest[start + 1..start + end].to_string());
                rest = &rest[start + end + 1..];
            }
            found.sort();
            found
        };
        for pack in [of(Lang::Ru), of(Lang::En)] {
            let work = pack.work;
            let expected: &[(&str, &str, &[&str])] = &[
                ("working", work.working, &["doing", "face", "label", "since", "title"]),
                ("asked", work.asked, &["asks"]),
                ("ask", work.ask, &["text"]),
                ("awaiting", work.awaiting, &["ago", "face", "label", "said", "title"]),
                ("unfinished", work.unfinished, &["ago", "face", "label", "said", "title"]),
                ("interrupted", work.interrupted, &["ago", "face", "label", "title"]),
                ("recent", work.recent, &["end", "face", "label", "title"]),
                ("desk", work.desk, &["list"]),
                ("more", work.more, &["n"]),
                ("seconds", work.seconds, &["n"]),
                ("minutes", work.minutes, &["n"]),
                ("hours", work.hours, &["n"]),
                ("days", work.days, &["n"]),
                ("say.started", work.say.started, &["title"]),
                ("say.started_first", work.say.started_first, &["title"]),
                ("say.noted", work.say.noted, &["text"]),
                ("say.again", work.say.again, &["title", "version"]),
                ("say.similar", work.say.similar, &["title"]),
                ("say.which", work.say.which, &["list"]),
                ("say.built_in", work.say.built_in, &["title"]),
                ("say.failed", work.say.failed, &["why"]),
                ("say.ready_line", work.say.ready_line, &["title"]),
                ("say.hosts", work.say.hosts, &["hosts"]),
                ("say.unfinished_line", work.say.unfinished_line, &["title"]),
                ("say.unchanged_line", work.say.unchanged_line, &["title"]),
                ("say.queued_next", work.say.queued_next, &["title"]),
                ("say.gone", work.say.gone, &["title"]),
                ("say.queued_in", work.say.queued_in, &["in", "title"]),
                ("say.waiting", work.say.waiting, &["title"]),
                ("queued", work.queued, &["face", "label", "title"]),
                ("say.failed_line", work.say.failed_line, &["title"]),
                ("say.changed", work.say.changed, &["text", "title"]),
                ("say.on_desk_needs_open", work.say.on_desk_needs_open, &["needs"]),
                ("say.on_desk_needs_tile", work.say.on_desk_needs_tile, &["needs"]),
                ("say.working", work.say.working, &["doing", "since"]),
                ("say.ready", work.say.ready, &["title"]),
                ("say.remind", work.say.remind, &["ask", "title"]),
                ("say.stopped", work.say.stopped, &["title"]),
                ("say.not_going", work.say.not_going, &["title"]),
            ];
            for (name, template, wanted) in expected {
                assert_eq!(slots(template), *wanted, "{name} of {}", pack.name);
            }
        }
    }
}
