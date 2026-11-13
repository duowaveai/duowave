//! Cards (docs/plans/bus.md, «Пульт»): a desk widget as data. An app's function returns a card — a number with its
//! change, a caption, rows, a line, tiles, a picture, clocks, countdown rings, shares, buttons, a field — and the
//! panel draws it with its own parts. No code of the app runs in the panel, so a card is safe, light, and the same
//! card can be shown anywhere: the desk, a phone, a chat, or said aloud (`speech`).
//!
//! The parts are fixed: an app cannot invent one. A part a widget needs and the set lacks is either a window (its own
//! drawing) or a new part added here and in the panel (`panel/src/cards/`) once, for everyone.

use serde_json::{Map, Value};

/// The parts a card may have, with what each is for: the workshop's agent reads this list.
pub const PARTS: &[(&str, &str)] = &[
    ("big", "крупное число или слово: «92,40 ₽», «+7°»"),
    ("delta", "изменение: {text, tone: up|down|flat}"),
    ("cap", "подпись под числом"),
    ("line", "линия: {points: [числа], area?: true} — до 200 точек"),
    ("bars", "столбики: {points: [числа], labels?: [строки]} — до 60"),
    ("tiles", "плитки: [{label, value}] — до 8, как дни погоды"),
    ("picture", "картинка из своей папки: {name, caption?}"),
    ("clocks", "часы: [{label, offset: минуты от UTC}] — до 6, тикают в пульте"),
    ("rings", "кольца с отсчётом: [{label, endsAt: время ISO, total?: секунды}] — до 6"),
    ("shares", "полоса долей: {parts: [{label, value}]} — до 6"),
    ("rows", "строки: [{text, value?, time?, delta?, mark?, tap?, with?}] — до 50"),
    ("progress", "полоска: {value: 0…1, text?}"),
    ("input", "поле: {placeholder, submit: функция, max?}"),
    ("buttons", "кнопки: [{text, tap: функция, with?, ink?}] — до 4"),
    ("choice", "выбор: {options: [{id, text}], selected, tap: функция}"),
    ("toggle", "переключатель: {text, on, tap: функция}"),
    ("empty", "что сказать, когда показывать нечего"),
    ("say", "своя фраза для голоса"),
];

fn text_ok(value: &Value, max: usize) -> bool {
    value.as_str().is_some_and(|text| text.chars().count() <= max)
}

fn function_ok(value: &Value) -> bool {
    value.as_str().is_some_and(crate::plugins::manifest::identifier)
}

/// Checks a card part by part; every problem in words for the agent and the author. Empty: a card the panel draws.
pub fn check(card: &Value) -> Vec<String> {
    let mut problems = Vec::new();
    let Some(fields) = card.as_object() else {
        return vec!["карточка — объект".into()];
    };
    let mut problem = |text: String| problems.push(text);
    for (name, value) in fields {
        match name.as_str() {
            "big" | "cap" | "empty" => {
                if !text_ok(value, 300) {
                    problem(format!("{name}: строка до 300 знаков"));
                }
            }
            "say" => {
                if !text_ok(value, 1000) {
                    problem("say: строка до 1000 знаков".into());
                }
            }
            "delta" => {
                let tone = value.get("tone").and_then(Value::as_str).unwrap_or("flat");
                if !value.get("text").is_some_and(|text| text_ok(text, 80)) || !matches!(tone, "up" | "down" | "flat") {
                    problem("delta: {text, tone: up|down|flat}".into());
                }
            }
            "line" | "bars" => {
                let max = if name == "line" { 200 } else { 60 };
                let points = value.get("points").and_then(Value::as_array);
                if !points.is_some_and(|points| points.len() <= max && points.iter().all(Value::is_number)) {
                    problem(format!("{name}: points — до {max} чисел"));
                }
            }
            "tiles" | "clocks" | "rings" | "rows" | "buttons" => {
                let max = match name.as_str() {
                    "tiles" => 8,
                    "clocks" | "rings" => 6,
                    "rows" => 50,
                    _ => 4,
                };
                match value.as_array() {
                    Some(items) if items.len() <= max => {
                        for (index, item) in items.iter().enumerate() {
                            if let Some(why) = item_problem(name, item) {
                                problem(format!("{name}[{index}]: {why}"));
                            }
                        }
                    }
                    _ => problem(format!("{name}: список до {max}")),
                }
            }
            "picture" => {
                if !value.get("name").is_some_and(|name| text_ok(name, 200)) {
                    problem("picture: {name: имя файла в своей папке, caption?}".into());
                }
            }
            "shares" => {
                let parts = value.get("parts").and_then(Value::as_array);
                let fine = parts.is_some_and(|parts| {
                    parts.len() <= 6
                        && parts.iter().all(|part| {
                            part.get("label").is_some_and(|label| text_ok(label, 40))
                                && part.get("value").is_some_and(Value::is_number)
                        })
                });
                if !fine {
                    problem("shares: {parts: [{label, value}]} — до 6".into());
                }
            }
            "progress" => {
                let fine = value.get("value").and_then(Value::as_f64).is_some_and(|share| (0.0..=1.0).contains(&share));
                if !fine {
                    problem("progress: {value: 0…1, text?}".into());
                }
            }
            "input" => {
                if !value.get("submit").is_some_and(function_ok) {
                    problem("input: {placeholder, submit: имя функции}".into());
                }
            }
            "choice" => {
                let options = value.get("options").and_then(Value::as_array);
                let fine = options.is_some_and(|options| {
                    options.len() <= 8
                        && options.iter().all(|option| option.get("id").is_some() && option.get("text").is_some())
                });
                if !fine || !value.get("tap").is_some_and(function_ok) {
                    problem("choice: {options: [{id, text}] до 8, selected, tap: функция}".into());
                }
            }
            "toggle" => {
                if !value.get("on").is_some_and(Value::is_boolean) || !value.get("tap").is_some_and(function_ok) {
                    problem("toggle: {text, on: да/нет, tap: функция}".into());
                }
            }
            other => problem(format!(
                "нет такой детали: «{other}» — детали карточки: {}; своё рисование — окошко",
                PARTS.iter().map(|(name, _)| *name).collect::<Vec<_>>().join(", ")
            )),
        }
    }
    problems
}

fn item_problem(part: &str, item: &Value) -> Option<String> {
    let fields = item.as_object()?;
    let tap = fields.get("tap");
    if tap.is_some_and(|tap| !function_ok(tap)) {
        return Some("tap — имя функции".into());
    }
    match part {
        "tiles" => (!fields.get("label").is_some_and(|text| text_ok(text, 40))
            || !fields.get("value").is_some_and(|text| text_ok(text, 40)))
        .then(|| "{label, value}".into()),
        "clocks" => (!fields.get("label").is_some_and(|text| text_ok(text, 40))
            || !fields.get("offset").and_then(Value::as_i64).is_some_and(|offset| offset.abs() <= 14 * 60))
        .then(|| "{label, offset: минуты от UTC}".into()),
        "rings" => (!fields.get("label").is_some_and(|text| text_ok(text, 40))
            || !fields.get("endsAt").is_some_and(Value::is_string))
        .then(|| "{label, endsAt: время ISO}".into()),
        "rows" => (!fields.get("text").is_some_and(|text| text_ok(text, 200)))
            .then(|| "{text, value?, time?, mark?, tap?}".into()),
        "buttons" => (!fields.get("text").is_some_and(|text| text_ok(text, 40)) || tap.is_none())
            .then(|| "{text, tap: функция}".into()),
        _ => None,
    }
}

/// The functions a card's buttons, rows, field and switches call: each must be among the app's `panel.calls`.
pub fn functions(card: &Value) -> Vec<String> {
    let mut found = Vec::new();
    let mut take = |value: Option<&Value>| {
        if let Some(name) = value.and_then(Value::as_str) {
            if !found.iter().any(|known: &String| known == name) {
                found.push(name.to_string());
            }
        }
    };
    for list in ["rows", "buttons"] {
        for item in card.get(list).and_then(Value::as_array).into_iter().flatten() {
            take(item.get("tap"));
        }
    }
    take(card.get("input").and_then(|input| input.get("submit")));
    take(card.get("choice").and_then(|choice| choice.get("tap")));
    take(card.get("toggle").and_then(|toggle| toggle.get("tap")));
    found
}

/// A card said aloud: its own `say`, or what it shows in a few sentences (the station without a screen, «что на
/// карточке»). Never longer than a short answer.
pub fn speech(title: &str, card: &Value) -> String {
    if let Some(say) = card.get("say").and_then(Value::as_str).filter(|say| !say.trim().is_empty()) {
        return say.trim().to_string();
    }
    fn text(value: Option<&Value>) -> Option<&str> {
        value.and_then(Value::as_str).map(str::trim).filter(|text| !text.is_empty())
    }
    let mut parts: Vec<String> = Vec::new();
    let mut head = title.trim().to_string();
    if let Some(big) = text(card.get("big")) {
        head = format!("{head}: {big}");
    }
    if let Some(delta) = text(card.get("delta").and_then(|delta| delta.get("text"))) {
        head = format!("{head}, {delta}");
    }
    parts.push(head);
    if let Some(cap) = text(card.get("cap")) {
        parts.push(cap.to_string());
    }
    let rows: Vec<String> = card
        .get("rows")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(5)
        .filter_map(|row| {
            let words = text(row.get("text"))?;
            let mut line = match text(row.get("time")) {
                Some(time) => format!("{time} {words}"),
                None => words.to_string(),
            };
            if let Some(value) = text(row.get("value")) {
                line = format!("{line} — {value}");
            }
            if row.get("mark").and_then(Value::as_bool) == Some(true) {
                line = format!("{line}, готово");
            }
            Some(line)
        })
        .collect();
    if !rows.is_empty() {
        parts.push(rows.join("; "));
    } else if let Some(empty) = text(card.get("empty")) {
        parts.push(empty.to_string());
    }
    let tiles: Vec<String> = card
        .get("tiles")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(4)
        .filter_map(|tile| Some(format!("{} {}", text(tile.get("label"))?, text(tile.get("value"))?)))
        .collect();
    if !tiles.is_empty() {
        parts.push(tiles.join(", "));
    }
    let spoken = parts.join(". ");
    spoken.chars().take(600).collect()
}

/// A card as the panel takes it: an object, or `{card: …}` around one.
pub fn unwrap(answer: Value) -> Value {
    match answer {
        Value::Object(mut fields) if fields.len() == 1 && fields.contains_key("card") => {
            fields.remove("card").unwrap_or(Value::Object(Map::new()))
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_card_of_known_parts_passes_and_an_unknown_part_is_named() {
        let card = json!({
            "big": "92,40 ₽",
            "delta": {"text": "+2,4% за сутки", "tone": "up"},
            "cap": "Доллар",
            "line": {"points": [1, 2, 3], "area": true},
            "rows": [{"text": "Молоко", "mark": false, "tap": "remove", "with": {"id": 1}}],
            "input": {"placeholder": "Добавить…", "submit": "add"},
            "buttons": [{"text": "Обновить", "tap": "refresh"}],
            "clocks": [{"label": "Москва", "offset": 180}],
            "rings": [{"label": "Чай", "endsAt": "2026-11-13T10:00:00Z", "total": 180}],
            "shares": {"parts": [{"label": "своё", "value": 20}]},
            "toggle": {"text": "Звук", "on": true, "tap": "sound"}
        });
        assert_eq!(check(&card), Vec::<String>::new());
        let problems =
            check(&json!({"canvas": {}, "rows": [{"value": "x"}], "buttons": [{"text": "Да", "tap": "do it"}]}));
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert!(problems
            .iter()
            .any(|problem| problem.contains("нет такой детали: «canvas»") && problem.contains("окошко")));
        assert_eq!(functions(&card), vec!["remove", "refresh", "add", "sound"]);
    }

    #[test]
    fn a_card_reads_aloud_in_a_few_sentences() {
        let card = json!({
            "big": "+7°",
            "cap": "Сейчас облачно",
            "rows": [{"text": "Вечером", "value": "+5°"}, {"time": "10:00", "text": "Позвонить", "mark": true}],
            "tiles": [{"label": "завтра", "value": "+4°"}]
        });
        assert_eq!(
            speech("Погода", &card),
            "Погода: +7°. Сейчас облачно. Вечером — +5°; 10:00 Позвонить, готово. завтра +4°"
        );
        assert_eq!(speech("Список", &json!({"empty": "Пусто", "rows": []})), "Список. Пусто");
        assert_eq!(
            speech("Курс", &json!({"say": "Доллар стоит девяносто два рубля"})),
            "Доллар стоит девяносто два рубля"
        );
        assert_eq!(unwrap(json!({"card": {"big": "1"}})), json!({"big": "1"}));
    }
}
