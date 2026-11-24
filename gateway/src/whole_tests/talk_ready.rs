//! The tests of `talk/ready.rs` that need the whole gateway (`App`): its crate does not have it, the program does.

use std::sync::Arc;

use crate::memory::text::Lang;
use crate::talk::asked::Kind;
use crate::talk::layer::TalkLayer;
use crate::talk::origin::Origin;
use crate::talk::pack::{self, fill};
use crate::talk::ready::*;

/// A Турбо that sleeps, a wake that brings it back (or not): docs/plans/turbo-devices.md, owner 20.11, 5.
struct Sleeper {
    wakes: bool,
}

impl crate::home::turbo_devices::TurboSource for Sleeper {
    fn turbos(&self) -> Vec<crate::home::turbo_devices::TurboAt> {
        vec![crate::home::turbo_devices::TurboAt {
            id: "mac".into(),
            name: "Mac".into(),
            macs: vec!["a4:83:e7:01:02:03".into()],
            ..Default::default()
        }]
    }
    fn call(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: serde_json::Value,
    ) -> futures::future::BoxFuture<'static, Result<serde_json::Value, String>> {
        Box::pin(async { Err("asleep".to_string()) })
    }
    fn look(&self, _: &str) -> futures::future::BoxFuture<'static, bool> {
        let wakes = self.wakes;
        Box::pin(async move { wakes })
    }
}

fn sleeping_house(wakes: bool) -> Arc<crate::home::turbo_devices::HouseDevices> {
    let house =
        crate::home::turbo_devices::HouseDevices::new(Arc::new(crate::bus::providers::devices::Devices::new()), None);
    house.set_source(Arc::new(Sleeper { wakes }));
    house.set_waker(Arc::new(|macs: Vec<String>| Box::pin(async move { Ok(macs.len()) })));
    house
}

/// «…Разбудить Mac?» is the device's question; its «да» wakes the Mac, its «нет» leaves it to the person.
#[tokio::test]
async fn a_sleeping_turbo_is_woken_on_a_yes_and_left_on_a_no() {
    let layer = TalkLayer::new(None, crate::core::events::EventBus::new());
    let house = sleeping_house(true);
    let wake: Arc<dyn crate::talk::layer::TurboWake> = house.clone();
    layer.set_house(&wake);
    let origin = Origin::station("kitchen", true);
    assert_eq!(layer.offer_wake("kitchen", "mac", "Mac"), "Разбудить Mac?");
    let question = layer.asked.open("kitchen").unwrap();
    assert_eq!((question.kind, question.work.as_str()), (Kind::Wake, ""));
    let said = layer.answer(origin.clone(), &question.work, Kind::Wake, false, Answer::Yes).await;
    assert_eq!(said, "Mac проснулся. Скажите ещё раз, что хотели.");
    assert!(layer.asked.open("kitchen").is_none(), "answered");

    layer.offer_wake("kitchen", "mac", "Mac");
    let said = layer.answer(origin.clone(), "", Kind::Wake, false, Answer::No).await;
    assert_eq!(said, "Хорошо. Тогда разбудите Mac сами и скажите ещё раз.");
}

/// 19.11: OpenAI's «no credits» made a game «не получилось. Подробности в пульте» — the cause is said now.
#[test]
fn names_a_failures_plain_cause_money_a_limit_a_key_and_nothing_else() {
    let say = &pack::of(Lang::Ru).work.say;
    let openai = r#"HTTP 429: { "error": { "message": "You have no credits remaining.", "type": "insufficient_quota", "code": "credit_balance_exhausted" } }"#;
    assert_eq!(failure_why(openai, say), Some(say.why_money));
    assert_eq!(
        failure_why("429 rate_limit_error: Number of request tokens has exceeded your limit", say),
        Some(say.why_rate)
    );
    assert_eq!(failure_why("401 invalid x-api-key", say), Some(say.why_key));
    assert_eq!(failure_why("no API key: add it to the provider in the panel", say), Some(say.why_key));
    assert_eq!(failure_why("Anthropic stream broke: 14029 tokens", say), None);
    assert_eq!(
        fill(say.failed_why_line, &[("title", "Новая игра"), ("why", say.why_money)]),
        "Новая игра: не получилось — на ключе «Мышления» кончились деньги. Подробности в пульте."
    );
}

#[test]
fn reads_the_answers_to_the_question_and_lets_go_of_the_rest() {
    let ru = pack::of(Lang::Ru);
    let read = |text: &str| answer_of(ru, text);
    assert_eq!(read("да"), Some(Answer::Yes));
    assert_eq!(read("на стол"), Some(Answer::Yes));
    assert_eq!(read("поставь"), Some(Answer::Yes));
    assert_eq!(read("примерить"), Some(Answer::Fit));
    assert_eq!(read("нет"), Some(Answer::No));
    assert_eq!(read("потом"), Some(Answer::No));
    assert_eq!(read("в черновики"), Some(Answer::Park));
    assert_eq!(read("подробнее"), Some(Answer::Details));
    assert_eq!(read("переделай покрупнее"), Some(Answer::Redo { text: "переделай покрупнее".into() }));
    assert_eq!(read("а слова сделай покрупнее"), Some(Answer::Redo { text: "слова сделай покрупнее".into() }));
    assert_eq!(read("не надо"), Some(Answer::Drop));
    assert_eq!(read("какая погода"), None, "not the question's");
    let en = pack::of(Lang::En);
    assert_eq!(answer_of(en, "yes"), Some(Answer::Yes));
    assert_eq!(answer_of(en, "try it on"), Some(Answer::Fit));
    assert_eq!(answer_of(en, "what is the weather"), None);
}

/// 20.11: to «Что-то ещё добавить?» a «нет» in any words is «nothing to add» — the work goes on; it stops only on
/// words of stopping; anything else is the addition.
#[test]
fn to_anything_else_a_no_in_any_words_lets_the_work_go_on_and_only_a_stop_stops_it() {
    let read = |lang: Lang, text: &str| extra_answer(pack::of(lang), &crate::skills::text::normalize(text));
    for no in [
        "Нет.",
        "Нет, не надо.",
        "Не надо",
        "Нет, не надо ничего больше добавлять",
        "Ничего",
        "Больше ничего не надо",
        "Ничего не добавляй",
        "Всё",
        "Всё, хватит",
        "Так оставь",
        "Пусть будет так",
        "Нет, спасибо",
        "Продолжай",
        "Нет, делай дальше",
        "Больше ничего не надо делать",
    ] {
        assert_eq!(read(Lang::Ru, no), Some(Answer::No), "{no}");
    }
    for stop in [
        "Останови",
        "Отмени",
        "Нет, отмени виджет",
        "Стоп",
        "Прекрати",
        "Не делай",
        "Нет, не делай виджет",
        "Не надо делать",
        "Хватит делать",
        "Я передумал",
    ] {
        assert_eq!(read(Lang::Ru, stop), Some(Answer::Drop), "{stop}");
    }
    assert_eq!(read(Lang::Ru, "Да"), Some(Answer::Yes));
    assert_eq!(read(Lang::Ru, "Добавь"), Some(Answer::Yes));
    assert_eq!(read(Lang::Ru, "Да, сделай фон синим"), Some(Answer::Redo { text: "сделай фон синим".into() }));
    assert_eq!(
        read(Lang::Ru, "Нет, но пусть будет секундная стрелка"),
        Some(Answer::Redo { text: "но пусть будет секундная стрелка".into() })
    );
    assert_eq!(read(Lang::Ru, "И ещё будильник"), Some(Answer::Redo { text: "и еще будильник".into() }));
    assert_eq!(read(Lang::Ru, ""), None);

    for no in [
        "No",
        "No, thanks",
        "Nothing",
        "Nothing else",
        "No, don't add anything",
        "That's all",
        "That's it",
        "No need",
        "Keep going",
        "It's fine",
    ] {
        assert_eq!(read(Lang::En, no), Some(Answer::No), "{no}");
    }
    for stop in ["Stop", "Cancel it", "No, stop it", "Don't make it", "Do not build it", "I changed my mind"] {
        assert_eq!(read(Lang::En, stop), Some(Answer::Drop), "{stop}");
    }
    assert_eq!(read(Lang::En, "Yes"), Some(Answer::Yes));
    assert_eq!(
        read(Lang::En, "Yes, make the background blue"),
        Some(Answer::Redo { text: "make the background blue".into() })
    );
}
