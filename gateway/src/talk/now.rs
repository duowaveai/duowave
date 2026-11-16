//! The block «Сейчас» (docs/plans/audio-interface.md, «Блок «Сейчас»»): what the brain knows of what goes on — the
//! works going on and what was asked of them, what waits for an answer, what ended lately, the apps on the desk, the
//! workshop's door. Made from the records of works at every request to the model, in the character's language;
//! empty when nothing goes on. It goes into the part of the prompt that changes (beside the time), so the cache of
//! the stable part stays whole. The person sees the same text: `GET /api/v1/talk/now`, «Что сейчас знает Мозг».
//!
//! It is cut by tokens, not by the number of works (the fifth rule of «Задел на несколько дел»): the lines that matter
//! least go first.

use super::executor::Doing;
use super::pack::{fill, Pack};
use super::works::{End, Work, WorkState};

/// The block at most this long, in tokens.
pub const BUDGET: usize = 230;
/// What ended this long ago is still «недавно», and an answer still awaited.
const RECENT_MS: u64 = 24 * 60 * 60_000;
/// Ended works the block names at most.
const RECENT: usize = 3;
/// What an ask keeps of its words.
const ASK: usize = 40;

/// Tokens of a text, roughly: some three letters a token.
pub fn tokens(text: &str) -> usize {
    text.chars().count().div_ceil(3)
}

/// What the block is made of.
pub struct Sources<'a> {
    /// The device's works, any order.
    pub works: &'a [Work],
    /// What the executor is at on a work going on.
    pub doing: &'a dyn Fn(&Work) -> Option<Doing>,
    /// The titles of the apps on the desk.
    pub desk: &'a [String],
    /// The workshop's door is open on this device.
    pub door: bool,
    pub now: u64,
}

fn duration(pack: &Pack, ms: u64) -> String {
    let words = pack.work;
    let seconds = ms / 1000;
    let (template, n) = match seconds {
        0..=59 => (words.seconds, seconds.max(1)),
        60..=3599 => (words.minutes, seconds / 60),
        3600..=86_399 => (words.hours, seconds / 3600),
        _ => (words.days, seconds / 86_400),
    };
    fill(template, &[("n", &n.to_string())])
}

fn short(text: &str, max: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= max {
        return text;
    }
    let mut kept: String = text.chars().take(max - 1).collect();
    kept.push('…');
    kept
}

pub(super) fn doing_words(pack: &Pack, doing: Doing) -> &'static str {
    let words = &pack.work.doing;
    match doing {
        Doing::Thinking => words.thinking,
        Doing::Writing => words.writing,
        Doing::Reading => words.reading,
        Doing::Checking => words.checking,
        Doing::Trying => words.trying,
        Doing::Phrase => words.phrase,
        Doing::Docs => words.docs,
        Doing::Journal => words.journal,
    }
}

/// How a work that ended is said after «—».
fn end_words(pack: &Pack, work: &Work) -> &'static str {
    let ends = &pack.work.ends;
    match (work.result.as_ref().and_then(|result| result.end), work.state) {
        (Some(End::Desk), _) => ends.desk,
        (Some(End::Fitting), _) => ends.fitting,
        (Some(End::Parked), _) => ends.parked,
        (Some(End::Declined), _) => ends.declined,
        (Some(End::Removed), _) => ends.removed,
        (None, WorkState::Stopped) => ends.stopped,
        (None, WorkState::Failed) => ends.failed,
        (None, WorkState::Interrupted) => ends.interrupted,
        (None, WorkState::Unfinished) => ends.unfinished,
        (None, _) => ends.done,
    }
}

/// One work's line of the block, in brackets: what the history keeps in place of a ready phrase of the layer
/// («(В работе [р6]: виджет «Курс доллара» — 2 с, думаю.)», not «Делаю: курс доллара…»), so the model knows what was
/// done and has no phrase of ours to say next time without doing anything.
pub fn fact(pack: &Pack, work: &Work, now: u64, doing: Option<Doing>) -> String {
    let words = pack.work;
    let label = format!("{}{}", words.label, work.id);
    let face = face_of(work, words);
    let base = |template: &str, extra: &[(&str, &str)]| {
        let mut values = vec![("label", label.as_str()), ("face", face), ("title", work.title.as_str())];
        values.extend_from_slice(extra);
        fill(template, &values)
    };
    let ago = duration(pack, now.saturating_sub(work.ended_at.unwrap_or(work.updated_at)));
    let said = if work.said_at.is_some() { words.said } else { words.unsaid };
    let line = if work.state.queued() {
        base(words.queued, &[])
    } else if work.going() {
        let since = duration(pack, now.saturating_sub(work.started_at));
        base(words.working, &[("since", &since), ("doing", doing_words(pack, doing.unwrap_or(Doing::Thinking)))])
    } else if work.awaiting() {
        let template = if work.state == WorkState::Unfinished { words.unfinished } else { words.awaiting };
        base(template, &[("ago", &ago), ("said", said)])
    } else if work.state == WorkState::Interrupted {
        base(words.interrupted, &[("ago", &ago)])
    } else {
        base(words.recent, &[("end", end_words(pack, work))])
    };
    format!("({})", line.trim_start_matches('-').trim())
}

/// A line and how much it matters: the higher, the sooner it goes when the block is too long.
struct Line {
    text: String,
    rank: u8,
}

/// The block for one device; empty when nothing goes on there.
pub fn block(pack: &Pack, sources: &Sources) -> String {
    let words = pack.work;
    let face = |work: &Work| face_of(work, words);
    let label = |work: &Work| format!("{}{}", words.label, work.id);
    let mut works: Vec<&Work> = sources.works.iter().collect();
    works.sort_by_key(|work| std::cmp::Reverse(work.updated_at));
    // One thing, one line: an older record of the same thing says nothing more.
    let mut seen = std::collections::HashSet::new();
    works.retain(|work| match &work.target {
        Some(target) => seen.insert((work.executor.clone(), target.clone())),
        None => true,
    });
    let mut lines: Vec<Line> = Vec::new();
    let mut recent = 0;
    for work in works {
        let ago = || duration(pack, sources.now.saturating_sub(work.ended_at.unwrap_or(work.updated_at)));
        let said = || if work.said_at.is_some() { words.said } else { words.unsaid };
        let base = |template: &str, extra: &[(&str, &str)]| {
            let (label, face) = (label(work), face(work));
            let mut values = vec![("label", label.as_str()), ("face", face), ("title", work.title.as_str())];
            values.extend_from_slice(extra);
            fill(template, &values)
        };
        let fresh = sources.now.saturating_sub(work.ended_at.unwrap_or(work.updated_at)) < RECENT_MS;
        if work.state.queued() {
            lines.push(Line { text: base(words.queued, &[]), rank: 1 });
        } else if work.going() {
            let since = duration(pack, sources.now.saturating_sub(work.started_at));
            let doing = doing_words(pack, (sources.doing)(work).unwrap_or(Doing::Thinking));
            let mut text = base(words.working, &[("since", &since), ("doing", doing)]);
            let mut asks = vec![fill(words.ask, &[("text", &short(&work.brief, ASK))])];
            for note in &work.notes {
                asks.push(fill(words.ask, &[("text", &format!("{}{}", words.added, short(&note.text, ASK)))]));
            }
            if !work.brief.trim().is_empty() {
                text.push_str(&fill(words.asked, &[("asks", &asks.join("; "))]));
            }
            lines.push(Line { text, rank: 0 });
        } else if work.awaiting() && fresh {
            let template = if work.state == WorkState::Unfinished { words.unfinished } else { words.awaiting };
            lines.push(Line { text: base(template, &[("ago", &ago()), ("said", said())]), rank: 1 });
        } else if work.state == WorkState::Interrupted && fresh {
            lines.push(Line { text: base(words.interrupted, &[("ago", &ago())]), rank: 3 });
        } else if fresh && recent < RECENT {
            recent += 1;
            lines.push(Line { text: base(words.recent, &[("end", end_words(pack, work))]), rank: 4 + recent as u8 });
        }
    }
    if lines.is_empty() && !sources.door {
        return String::new();
    }
    if sources.door {
        lines.push(Line { text: words.door.to_string(), rank: 2 });
    }
    // Drops what matters least until the block fits, the desk's list last of all made to fit what is left.
    let used = |lines: &[Line]| tokens(words.head) + lines.iter().map(|line| tokens(&line.text) + 1).sum::<usize>();
    while used(&lines) > BUDGET {
        let Some(worst) = lines.iter().enumerate().max_by_key(|(_, line)| line.rank).map(|(at, _)| at) else { break };
        if lines[worst].rank == 0 {
            // Only works going on are left: each keeps its line, without what was asked.
            for line in lines.iter_mut() {
                if let Some(cut) =
                    line.text.find(words.asked.split('{').next().unwrap_or_default()).filter(|at| *at > 0)
                {
                    line.text.truncate(cut);
                }
            }
            break;
        }
        lines.remove(worst);
    }
    let room = BUDGET.saturating_sub(used(&lines));
    if let Some(desk) = desk_line(pack, sources.desk, room) {
        // Before the door: the order a person reads it in.
        let at = lines.iter().position(|line| line.text == words.door).unwrap_or(lines.len());
        lines.insert(at, Line { text: desk, rank: 9 });
    }
    let mut text = words.head.to_string();
    for line in lines {
        text.push('\n');
        text.push_str(&line.text);
    }
    text
}

/// «На столе: Погода, Дела, Курс валют… (12)» within `room` tokens; None when nothing is there or nothing fits.
fn desk_line(pack: &Pack, desk: &[String], room: usize) -> Option<String> {
    let words = pack.work;
    if desk.is_empty() {
        return None;
    }
    let mut shown = desk.len();
    loop {
        let mut list = desk[..shown].join(", ");
        if shown < desk.len() {
            list.push_str(&fill(words.more, &[("n", &desk.len().to_string())]));
        }
        let line = fill(words.desk, &[("list", &list)]);
        if tokens(&line) < room {
            return Some(line);
        }
        if shown <= 1 {
            return None;
        }
        shown -= 1;
    }
}

/// What a work is called in the block: its face, «фоновое дело» for a tool's call and an app's task, «работа в
/// папке» for an agent in a work folder.
fn face_of(work: &Work, words: &super::pack::WorkWords) -> &'static str {
    if matches!(work.kind.as_str(), super::works::TOOL | super::works::TASK) {
        return words.background.face;
    }
    if work.kind == super::works::FOLDER {
        return words.folder.face;
    }
    work.face.map(|face| words.faces[face.index()]).unwrap_or(words.faces[0])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::text::Lang;
    use crate::talk::origin::Origin;
    use crate::talk::pack;
    use crate::talk::works::{note_of, Face, Tokens, WorkResult};

    const MIN: u64 = 60_000;

    fn work(id: &str, title: &str, state: WorkState, started: u64, ended: Option<u64>) -> Work {
        Work {
            id: id.into(),
            kind: "app".into(),
            face: Some(Face::Widget),
            executor: "workshop".into(),
            target: Some(format!("golos-{id}")),
            title: title.into(),
            state,
            brief: "я учу испанский, дай мне виджет со словами дня, и чтобы знакомые подсвечивались".into(),
            again: false,
            notes: Vec::new(),
            result: None,
            question: None,
            origin: Origin::station("kitchen", true),
            said_at: None,
            tokens: Tokens::default(),
            started_at: started,
            updated_at: ended.unwrap_or(started),
            ended_at: ended,
        }
    }

    /// The history keeps a work's line in place of the layer's phrase: the state, not «Делаю: …».
    #[test]
    fn a_fact_is_the_works_line_in_brackets_whatever_its_state() {
        let now = 100 * MIN;
        let pack = pack::of(Lang::Ru);
        let going = work("6", "Курс доллара", WorkState::Working, now - 2_000, None);
        assert_eq!(fact(pack, &going, now, None), "(В работе [р6]: виджет «Курс доллара» — 2 с, думаю.)");
        let mut ready = work("6", "Курс доллара", WorkState::Ready, now - 9 * MIN, Some(now - MIN));
        assert_eq!(
            fact(pack, &ready, now, None),
            "(Ждёт ответа [р6]: виджет «Курс доллара» — готово 1 мин назад, ещё не сказано.)"
        );
        ready.state = WorkState::Done;
        ready.result = Some(WorkResult { line: String::new(), end: Some(End::Desk), ..WorkResult::default() });
        assert_eq!(fact(pack, &ready, now, None), "(Недавно [р6]: виджет «Курс доллара» — на столе.)");
    }

    #[test]
    fn tells_what_goes_on_what_waits_and_what_ended_and_is_empty_when_nothing_does() {
        let now = 100 * MIN;
        let mut spanish = work("3", "Испанский: слова дня", WorkState::Working, now - 40_000, None);
        spanish.notes.push(note_of("и ещё кнопку, чтобы слушать произношение", Origin::station("kitchen", true), now));
        let mut teacher =
            work("2", "Учитель на MIDI-клавиатуре", WorkState::Ready, now - 20 * MIN, Some(now - 9 * MIN));
        teacher.said_at = Some(now - 9 * MIN);
        teacher.face = Some(Face::Skill);
        let mut watering = work("1", "Полив", WorkState::Done, now - 60 * MIN, Some(now - 50 * MIN));
        watering.result = Some(WorkResult { line: String::new(), end: Some(End::Desk), ..WorkResult::default() });
        let works = vec![spanish, teacher, watering];
        let doing = |_: &Work| Some(Doing::Writing);
        let desk = vec!["Погода".to_string(), "Дела".to_string(), "Курс валют".to_string()];
        let sources = Sources { works: &works, doing: &doing, desk: &desk, door: true, now };
        let text = block(pack::of(Lang::Ru), &sources);
        assert_eq!(
            text,
            "Сейчас (знай, не пересказывай без вопроса):\n\
             - В работе [р3]: виджет «Испанский: слова дня» — 40 с, пишу код. Просили: «я учу испанский, дай мне виджет со \
             слов…»; «+ и ещё кнопку, чтобы слушать произношение».\n\
             - Ждёт ответа [р2]: навык «Учитель на MIDI-клавиатуре» — готово 9 мин назад, сказано.\n\
             - Недавно [р1]: виджет «Полив» — на столе.\n\
             - На столе: Погода, Дела, Курс валют.\n\
             - Мастерская открыта."
        );
        assert!(tokens(&text) <= BUDGET);
        let english = block(pack::of(Lang::En), &sources);
        assert!(english.starts_with("Now (keep it in mind") && english.contains("[w3]"), "{english}");

        // Nothing goes on, the door is shut: no block — the desk alone is not news.
        let old = vec![work("1", "Полив", WorkState::Done, 0, Some(MIN))];
        let quiet = Sources { works: &old, doing: &doing, desk: &desk, door: false, now: now + 2 * RECENT_MS };
        assert_eq!(block(pack::of(Lang::Ru), &quiet), "");
        assert_eq!(block(pack::of(Lang::Ru), &Sources { works: &[], doing: &doing, desk: &[], door: false, now }), "");
    }

    #[test]
    fn keeps_to_its_budget_by_tokens_whatever_the_number_of_works() {
        let now = 100 * MIN;
        let works: Vec<Work> = (1..=40)
            .map(|n| {
                work(
                    &n.to_string(),
                    &format!("Вещь номер {n} с длинным названием"),
                    WorkState::Done,
                    now - MIN,
                    Some(now - MIN),
                )
            })
            .chain((41..=45).map(|n| work(&n.to_string(), "Идёт", WorkState::Working, now - MIN, None)))
            .collect();
        let doing = |_: &Work| None;
        let desk: Vec<String> = (1..=30).map(|n| format!("Приложение {n}")).collect();
        let text = block(pack::of(Lang::Ru), &Sources { works: &works, doing: &doing, desk: &desk, door: true, now });
        assert!(tokens(&text) <= BUDGET, "{} tokens: {text}", tokens(&text));
        assert!(text.contains("[р45]") && text.contains("[р41]"), "the works going on stay: {text}");
    }
}
