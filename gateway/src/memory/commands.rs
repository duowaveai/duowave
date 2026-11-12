//! The memory's explicit voice commands, recognised without the model (docs/plans/memory.md, «Голос»): «Запомни …»,
//! «Забудь …», «Что ты помнишь про …?», «Что ты обо мне знаешь?», «Не запоминай ничего до утра» and back. Russian and
//! English. Anything else goes to the model as usual.

use super::text::normalize;

/// How long «не запоминай» lasts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseSpan {
    /// An hour from now.
    Hour,
    /// Until the next morning, 7:00 by the station's clock.
    Morning,
    /// Until someone turns it back on.
    UntilResumed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// «Запомни: у Маши аллергия на орехи» — the text as said, without the command word.
    Remember(String),
    /// «Забудь про орехи» — what to close.
    Forget(String),
    /// «Что ты помнишь про Машу?» — about whom or what.
    Recall(String),
    /// «Что ты обо мне знаешь?»
    AboutMe,
    /// «Не запоминай ничего до утра»
    Pause(PauseSpan),
    /// «Можешь снова запоминать»
    Resume,
    /// «Сотри всю память», «Выключи память», «Включи рефлексию»: only in the panel, with the right to change memory —
    /// so nobody in the room can wipe or switch it by voice.
    PanelOnly,
}

/// The command in an utterance, if it is one. The utterance is matched whole, from its start: «я сказала ему,
/// запомни» is not a command.
pub fn parse(utterance: &str) -> Option<Command> {
    let text = normalize(utterance);
    let text = strip_lead(&text);
    parse_ru(text).or_else(|| parse_en(text))
}

/// Polite or calling words before the command: «пожалуйста», «слушай», «алиса» are not part of it.
fn strip_lead(text: &str) -> &str {
    let mut rest = text;
    loop {
        let before = rest;
        for lead in ["пожалуйста ", "слушай ", "алиса ", "please ", "hey "] {
            if let Some(after) = rest.strip_prefix(lead) {
                rest = after;
            }
        }
        if rest == before {
            return rest;
        }
    }
}

fn parse_ru(text: &str) -> Option<Command> {
    if is_panel_only_ru(text) {
        return Some(Command::PanelOnly);
    }
    if let Some(span) = pause_ru(text) {
        return Some(Command::Pause(span));
    }
    if is_resume_ru(text) {
        return Some(Command::Resume);
    }
    if matches!(
        text,
        "что ты обо мне знаешь"
            | "что ты знаешь обо мне"
            | "что ты обо мне помнишь"
            | "что ты помнишь обо мне"
            | "что ты знаешь про меня"
            | "что ты про меня знаешь"
            | "что ты помнишь про меня"
            | "что ты про меня помнишь"
    ) {
        return Some(Command::AboutMe);
    }
    for head in [
        "что ты помнишь про ",
        "что ты помнишь о ",
        "что ты помнишь об ",
        "что ты запомнила про ",
        "что ты запомнил про ",
    ] {
        if let Some(rest) = text.strip_prefix(head) {
            return non_empty(rest).map(Command::Recall);
        }
    }
    for head in ["запомни что ", "запомни ", "запомните что ", "запомните "] {
        if let Some(rest) = text.strip_prefix(head) {
            // «Запомни это» says nothing to remember.
            return non_empty(rest)
                .filter(|r| !matches!(r.as_str(), "это" | "все" | "пожалуйста"))
                .map(Command::Remember);
        }
    }
    for head in ["забудь про ", "забудь о ", "забудь об ", "забудь что ", "забудь "]
    {
        if let Some(rest) = text.strip_prefix(head) {
            return non_empty(rest).filter(|r| !matches!(r.as_str(), "это" | "все")).map(Command::Forget);
        }
    }
    None
}

fn pause_ru(text: &str) -> Option<PauseSpan> {
    let rest = text.strip_prefix("не запоминай ")?;
    let rest = rest.strip_prefix("ничего").map(str::trim_start).unwrap_or(rest);
    let span = match rest {
        "" if text.contains("ничего") => PauseSpan::Morning,
        "до утра" | "до завтра" | "сегодня" => PauseSpan::Morning,
        "час" | "в течение часа" | "ближайший час" | "пока час" => PauseSpan::Hour,
        "пока не скажу" | "пока я не скажу" | "пока не попрошу" | "пока я не попрошу" | "пока не разрешу" => {
            PauseSpan::UntilResumed
        }
        _ => return None,
    };
    Some(span)
}

fn is_panel_only_ru(text: &str) -> bool {
    let wipe = ["сотри", "удали", "очисти", "стереть", "удалить", "очистить"]
        .iter()
        .any(|verb| text.starts_with(&format!("{verb} всю память")) || text == format!("{verb} память"));
    let switch = ["выключи память", "отключи память", "выключи свою память", "включи рефлексию", "выключи рефлексию"]
        .contains(&text);
    // The text is normalized: «всё» arrives as «все».
    wipe || switch || matches!(text, "забудь все" | "забудь вообще все")
}

fn is_resume_ru(text: &str) -> bool {
    matches!(
        text,
        "можешь запоминать"
            | "можешь снова запоминать"
            | "снова запоминай"
            | "запоминай снова"
            | "можно запоминать"
            | "можно снова запоминать"
            | "начни запоминать"
            | "запоминай"
    )
}

fn parse_en(text: &str) -> Option<Command> {
    if matches!(
        text,
        "erase your memory"
            | "erase all your memory"
            | "delete your memory"
            | "wipe your memory"
            | "clear your memory"
            | "turn off your memory"
            | "turn off memory"
            | "forget everything"
            | "turn on reflection"
    ) {
        return Some(Command::PanelOnly);
    }
    if let Some(span) = pause_en(text) {
        return Some(Command::Pause(span));
    }
    if matches!(
        text,
        "you can remember again" | "start remembering" | "start remembering again" | "remember again" | "resume memory"
    ) {
        return Some(Command::Resume);
    }
    if matches!(text, "what do you know about me" | "what do you remember about me") {
        return Some(Command::AboutMe);
    }
    for head in ["what do you remember about ", "what do you know about my "] {
        if let Some(rest) = text.strip_prefix(head) {
            let rest = if head.ends_with("my ") { format!("my {rest}") } else { rest.to_string() };
            return non_empty(&rest).map(Command::Recall);
        }
    }
    for head in ["remember that ", "remember "] {
        if let Some(rest) = text.strip_prefix(head) {
            return non_empty(rest).filter(|r| !matches!(r.as_str(), "this" | "that" | "it")).map(Command::Remember);
        }
    }
    for head in ["forget about ", "forget that ", "forget "] {
        if let Some(rest) = text.strip_prefix(head) {
            return non_empty(rest)
                .filter(|r| !matches!(r.as_str(), "it" | "this" | "that" | "everything"))
                .map(Command::Forget);
        }
    }
    None
}

fn pause_en(text: &str) -> Option<PauseSpan> {
    let rest = ["don't remember anything", "do not remember anything", "stop remembering"]
        .iter()
        .find_map(|head| text.strip_prefix(head))?
        .trim_start();
    Some(match rest {
        "" | "until morning" | "until tomorrow" | "tonight" => PauseSpan::Morning,
        "for an hour" | "for the next hour" => PauseSpan::Hour,
        "until i say so" | "until i tell you" => PauseSpan::UntilResumed,
        _ => return None,
    })
}

fn non_empty(rest: &str) -> Option<String> {
    let rest = rest.trim();
    (!rest.is_empty()).then(|| rest.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_and_forgets_in_russian() {
        assert_eq!(
            parse("Запомни: у Маши аллергия на орехи"),
            Some(Command::Remember("у маши аллергия на орехи".into()))
        );
        assert_eq!(
            parse("Алиса, запомни, что Петя ходит на шахматы"),
            Some(Command::Remember("петя ходит на шахматы".into()))
        );
        assert_eq!(parse("Запомни это"), None);
        assert_eq!(parse("Забудь про орехи"), Some(Command::Forget("орехи".into())));
        assert_eq!(parse("забудь что у маши аллергия"), Some(Command::Forget("у маши аллергия".into())));
        assert_eq!(parse("Забудь"), None);
    }

    #[test]
    fn recalls_and_answers_about_me() {
        assert_eq!(parse("Что ты помнишь про Машу?"), Some(Command::Recall("машу".into())));
        assert_eq!(parse("что ты помнишь о коте"), Some(Command::Recall("коте".into())));
        assert_eq!(parse("Что ты обо мне знаешь?"), Some(Command::AboutMe));
        // A question about the world is the model's: «что ты знаешь про чёрные дыры».
        assert_eq!(parse("Что ты знаешь про чёрные дыры?"), None);
    }

    #[test]
    fn pauses_and_resumes() {
        assert_eq!(parse("Не запоминай ничего до утра"), Some(Command::Pause(PauseSpan::Morning)));
        assert_eq!(parse("не запоминай ничего"), Some(Command::Pause(PauseSpan::Morning)));
        assert_eq!(parse("не запоминай ничего час"), Some(Command::Pause(PauseSpan::Hour)));
        assert_eq!(parse("Не запоминай, пока я не скажу"), Some(Command::Pause(PauseSpan::UntilResumed)));
        assert_eq!(parse("не запоминай это"), None);
        assert_eq!(parse("Можешь снова запоминать"), Some(Command::Resume));
    }

    #[test]
    fn works_in_english() {
        assert_eq!(
            parse("Remember that Masha is allergic to nuts"),
            Some(Command::Remember("masha is allergic to nuts".into()))
        );
        assert_eq!(parse("Forget about the nuts"), Some(Command::Forget("the nuts".into())));
        assert_eq!(parse("What do you remember about Masha?"), Some(Command::Recall("masha".into())));
        assert_eq!(parse("What do you know about me?"), Some(Command::AboutMe));
        assert_eq!(parse("Don't remember anything until morning"), Some(Command::Pause(PauseSpan::Morning)));
        assert_eq!(parse("You can remember again"), Some(Command::Resume));
        assert_eq!(parse("Remember this"), None);
    }

    #[test]
    fn leaves_wiping_and_switching_to_the_panel() {
        assert_eq!(parse("Сотри всю память"), Some(Command::PanelOnly));
        assert_eq!(parse("Выключи память"), Some(Command::PanelOnly));
        assert_eq!(parse("Забудь всё"), Some(Command::PanelOnly));
        assert_eq!(parse("Включи рефлексию"), Some(Command::PanelOnly));
        assert_eq!(parse("Forget everything"), Some(Command::PanelOnly));
        // «Забудь про всё, что я говорил про орехи» is a normal forget.
        assert_eq!(parse("Забудь про орехи"), Some(Command::Forget("орехи".into())));
    }

    #[test]
    fn ignores_other_talk() {
        assert_eq!(parse("Какая погода завтра?"), None);
        assert_eq!(parse("я сказала ему запомни"), None);
        assert_eq!(parse("What's the weather?"), None);
    }
}
