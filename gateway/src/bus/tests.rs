//! The bus with a made-up provider: letters, questions, answers, news, lists and fuses as a whole.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

use futures::future::BoxFuture;
use serde_json::json;

use super::*;
use crate::core::identity::principal::{household_principal, user_principal};
use crate::core::identity::roles::Role;

/// Apps `app/a` … with a few handles: `list` (call), `wipe` (call, dangerous), `notes` (call, personal), `smart`
/// (call, needs `use:smart_home`), `edit` (edit). `app/a` asked for `call:app/b` in its manifest.
pub(crate) struct Toy {
    pub calls: AtomicU32,
    pub heard: Mutex<Vec<String>>,
}

impl Toy {
    pub fn new() -> Arc<Toy> {
        Arc::new(Toy { calls: AtomicU32::new(0), heard: Mutex::new(Vec::new()) })
    }
}

impl Provider for Toy {
    fn kind(&self) -> &'static str {
        "app"
    }

    fn participants(&self) -> Vec<Participant> {
        ["a", "b", "c", "lock"]
            .iter()
            .map(|id| Participant::new(Address::app(id), &format!("Приложение {id}")))
            .collect()
    }

    fn handles(&self, _address: &Address) -> Vec<Handle> {
        vec![
            Handle::new("list", Level::Call).titled("список"),
            Handle::new("wipe", Level::Call).dangerous(Some(Danger::Delete)),
            Handle::new("notes", Level::Call).private(true),
            Handle::new("smart", Level::Call).needs(Some("use:smart_home".into())),
            Handle::new("edit", Level::Edit),
        ]
    }

    fn declared(&self, from: &Address, to: &Address) -> Level {
        if from.as_str() == "app/a" && to.as_str() == "app/b" {
            Level::Call
        } else if from.as_str() == "app/c" && to.as_str() == "app/a" {
            Level::Listen
        } else if from.as_str() == "app/a" && Pattern::parse("files/home/Документы/*").unwrap().matches(to) {
            Level::Edit
        } else {
            Level::None
        }
    }

    fn call(&self, letter: Letter) -> BoxFuture<'static, Result<Value, BusError>> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let root = letter.trail.root.label();
        Box::pin(
            async move { Ok(json!({ "handle": letter.handle, "root": root, "trail": trail::current().is_some() })) },
        )
    }

    fn listeners(&self, from: &Address, event: &str) -> Vec<Address> {
        if from.as_str() == "app/a" && event == "bought" {
            vec![Address::app("b"), Address::app("c")]
        } else {
            Vec::new()
        }
    }

    fn deliver(&self, to: &Address, news: News) -> BoxFuture<'static, Result<(), BusError>> {
        self.heard.lock().unwrap().push(format!("{to}:{}", news.event));
        Box::pin(async { Ok(()) })
    }
}

pub(crate) fn bus() -> (Arc<Bus>, Arc<Toy>) {
    let db = Arc::new(Db::open(None).unwrap());
    let events = EventBus::new();
    let settings = Arc::new(SettingsService::new(db.clone(), events.clone()));
    let bus = Bus::new(BusDeps { db, settings, events, keep_lines: 1000 });
    let toy = Toy::new();
    bus.register(toy.clone());
    (bus, toy)
}

fn a() -> Address {
    Address::app("a")
}

fn b() -> Address {
    Address::app("b")
}

#[tokio::test]
async fn a_letter_without_a_link_meets_a_closed_door_and_with_one_goes_through_in_its_trail() {
    let (bus, toy) = bus();
    let trail = Trail::app("c");
    let shut = bus.call(trail.clone(), Address::app("c"), b(), "list", json!({}), vec![]).await;
    assert!(matches!(shut, Err(BusError::NotFound(_))), "{shut:?}");
    bus.links
        .grant(
            Link::new(
                Pattern::exact(&Address::app("c")),
                Pattern::parse("app/*").unwrap(),
                Level::Call,
                LinkSource::Owner,
            ),
            None,
        )
        .unwrap();
    let answer = bus.call(trail, Address::app("c"), b(), "list", json!({}), vec![]).await.unwrap();
    assert_eq!(answer, json!({"handle": "list", "root": "app:c", "trail": true}));
    assert_eq!(toy.calls.load(Ordering::Relaxed), 1);
    let lines = bus.journal.read(None, None, 10);
    assert_eq!(lines[0].outcome, "ok");
    assert_eq!(lines[1].outcome, "notFound");
}

#[tokio::test]
async fn what_the_manifest_asked_for_waits_for_a_yes_once_then_goes_through() {
    let (bus, toy) = bus();
    let first = bus.call(Trail::app("a"), a(), b(), "list", json!({}), vec![]).await;
    let Err(BusError::NeedsYes { id, why }) = first else { panic!("{first:?}") };
    assert_eq!(why, "«Приложение a» хочет звать «Приложение b»");
    // Asked again while it waits: the same question, no second one.
    let _ = bus.call(Trail::app("a"), a(), b(), "list", json!({}), vec![]).await;
    assert_eq!(bus.consent.waiting().len(), 1);
    bus.answer(&id, true, None).unwrap().unwrap();
    assert_eq!(bus.links.level(&a(), &b()), Level::Call);
    bus.call(Trail::app("a"), a(), b(), "list", json!({}), vec![]).await.unwrap();
    assert_eq!(toy.calls.load(Ordering::Relaxed), 1);
    // The answer «нет» is remembered as a phone remembers it: the door stays shut, and the app is not asked about
    // again — until the owner opens it in «Связи».
    let (bus, _) = super::tests::bus();
    let Err(BusError::NeedsYes { id, .. }) = bus.call(Trail::app("a"), a(), b(), "list", json!({}), vec![]).await
    else {
        panic!()
    };
    bus.answer(&id, false, None).unwrap();
    assert_eq!(
        bus.links.link(&a(), &b()).map(|link| (link.level, link.source)),
        Some((Level::None, LinkSource::Refused))
    );
    let again = bus.call(Trail::app("a"), a(), b(), "list", json!({}), vec![]).await;
    assert!(matches!(again, Err(BusError::Denied(_))), "{again:?}");
    assert!(bus.consent.waiting().is_empty());
    bus.links
        .grant(Link::new(Pattern::exact(&a()), Pattern::exact(&b()), Level::Call, LinkSource::Owner), None)
        .unwrap();
    bus.call(Trail::app("a"), a(), b(), "list", json!({}), vec![]).await.unwrap();
}

#[tokio::test]
async fn a_yes_for_a_folder_covers_all_in_it_and_what_was_not_asked_for_is_not_there() {
    let (bus, _) = bus();
    let dir = tempfile::tempdir().unwrap();
    let places = providers::files::Places::new(&dir.path().join("data"), &dir.path().join("content"), false);
    let files = Arc::new(providers::files::Files::new(places, Arc::new(|_| 64)));
    files.set_bus(Arc::downgrade(&bus));
    bus.register(files);
    let doc = |name: &str| Address::parse(&format!("files/home/Документы/{name}")).unwrap();
    let first = bus.call(Trail::app("a"), a(), doc("a.txt"), "write", json!({"text": "x"}), vec![]).await;
    let Err(BusError::NeedsYes { id, why }) = first else { panic!("{first:?}") };
    assert_eq!(why, "«Приложение a» хочет менять файлы в папке «Документы»");
    let question = bus.consent.waiting().remove(0);
    assert_eq!((question.grant.as_str(), question.level), ("files/home/Документы/*", Level::Edit));
    // Reading first, then writing: still the one question.
    let _ = bus.call(Trail::app("a"), a(), doc("b.txt"), "read", json!({}), vec![]).await;
    assert_eq!(bus.consent.waiting().len(), 1);
    bus.answer(&id, true, None).unwrap();
    bus.call(Trail::app("a"), a(), doc("a.txt"), "write", json!({"text": "x"}), vec![]).await.unwrap();
    bus.call(Trail::app("a"), a(), doc("b.txt"), "write", json!({"text": "y"}), vec![]).await.unwrap();
    let read = bus.call(Trail::app("a"), a(), doc("b.txt"), "read", json!({"as": "text"}), vec![]).await.unwrap();
    assert_eq!(read["text"], "y");
    // «Картинки» it never asked for: not there for it at all.
    let pictures = Address::parse("files/home/Картинки/c.png").unwrap();
    let unseen = bus.call(Trail::app("a"), a(), pictures, "read", json!({}), vec![]).await;
    assert!(matches!(unseen, Err(BusError::NotFound(_))), "{unseen:?}");
}

#[tokio::test]
async fn a_chain_a_guest_started_cannot_reach_through_an_app_what_the_guest_cannot() {
    let (bus, toy) = bus();
    bus.links
        .grant(Link::new(Pattern::exact(&a()), Pattern::exact(&b()), Level::Call, LinkSource::Owner), None)
        .unwrap();
    let guest = Trail::principal(&household_principal(Role::Guest, None)).entering("a").unwrap();
    let refused = bus.call(guest, a(), b(), "smart", json!({}), vec![]).await;
    assert!(matches!(refused, Err(BusError::Denied(ref why)) if why.contains("Дом")), "{refused:?}");
    let member = Trail::principal(&user_principal("u1", "Мама", Role::Member, None)).entering("a").unwrap();
    bus.call(member, a(), b(), "smart", json!({}), vec![]).await.unwrap();
    // An app on its own answers for itself with its links.
    bus.call(Trail::app("a"), a(), b(), "smart", json!({}), vec![]).await.unwrap();
    assert_eq!(toy.calls.load(Ordering::Relaxed), 2);
}

#[tokio::test]
async fn danger_without_a_person_asks_each_time_and_a_yes_buys_one_action() {
    let (bus, toy) = bus();
    bus.links
        .grant(Link::new(Pattern::exact(&a()), Pattern::exact(&b()), Level::Call, LinkSource::Owner), None)
        .unwrap();
    let person = Trail::principal(&user_principal("u1", "Мама", Role::Admin, None));
    bus.call(person, a(), b(), "wipe", json!({}), vec![]).await.unwrap();
    let Err(BusError::NeedsYes { id, why }) = bus.call(Trail::app("a"), a(), b(), "wipe", json!({}), vec![]).await
    else {
        panic!()
    };
    assert!(why.contains("удаляет"), "{why}");
    bus.answer(&id, true, None).unwrap();
    // «Да» to a dangerous action is no link: one action.
    assert_eq!(bus.links.level(&a(), &b()), Level::Call);
    bus.call(Trail::app("a"), a(), b(), "wipe", json!({}), vec![]).await.unwrap();
    assert!(matches!(
        bus.call(Trail::app("a"), a(), b(), "wipe", json!({}), vec![]).await,
        Err(BusError::NeedsYes { .. })
    ));
    assert_eq!(toy.calls.load(Ordering::Relaxed), 2);
}

#[tokio::test]
async fn the_personal_and_the_isolated_are_not_there_and_free_opens_only_the_open() {
    let (bus, _) = bus();
    bus.links.set_mode(Some(&a()), Some(links::Mode::Free), None).unwrap();
    bus.call(Trail::app("a"), a(), b(), "list", json!({}), vec![]).await.unwrap();
    assert!(matches!(
        bus.call(Trail::app("a"), a(), b(), "notes", json!({}), vec![]).await,
        Err(BusError::NotFound(_))
    ));
    assert!(matches!(bus.call(Trail::app("a"), a(), b(), "edit", json!({}), vec![]).await, Err(BusError::Denied(_))));
    bus.links.set_mode(Some(&b()), Some(links::Mode::Isolated), None).unwrap();
    assert!(matches!(bus.call(Trail::app("a"), a(), b(), "list", json!({}), vec![]).await, Err(BusError::NotFound(_))));
    // A person still reaches the isolated app in the panel.
    let person = Trail::principal(&user_principal("u1", "Мама", Role::Admin, None));
    bus.call(person, Address::parse("person/u1").unwrap(), b(), "list", json!({}), vec![]).await.unwrap();
}

#[tokio::test]
async fn news_reaches_the_listeners_the_rule_lets_hear_and_asks_for_the_declared() {
    let (bus, toy) = bus();
    bus.links
        .grant(Link::new(Pattern::exact(&b()), Pattern::exact(&a()), Level::Listen, LinkSource::Owner), None)
        .unwrap();
    bus.tell(Trail::app("a"), a(), "bought", json!({"item": "молоко"}));
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(*toy.heard.lock().unwrap(), vec!["app/b:bought".to_string()]);
    // `app/c` asked to listen in its manifest: a question waits for it.
    let waiting = bus.consent.waiting();
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0].from.as_str(), "app/c");
}

#[tokio::test]
async fn a_list_shows_only_what_the_caller_may_see_or_ask_for() {
    let (bus, _) = bus();
    let seen = bus.list(&Trail::app("a"), &a());
    // `app/b`: declared, so its handles show as askable; the rest are not there.
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].participant.address, b());
    assert!(seen[0].handles.iter().any(|handle| handle.handle.name == "list" && handle.asks));
    assert!(!seen[0].handles.iter().any(|handle| handle.handle.name == "notes"));
}

#[tokio::test]
async fn two_apps_answering_each_other_stop_at_the_days_budget() {
    let (bus, _) = bus();
    bus.links
        .grant(Link::new(Pattern::exact(&a()), Pattern::exact(&b()), Level::Call, LinkSource::Owner), None)
        .unwrap();
    for _ in 0..PAIR_BUDGET {
        bus.call(Trail::app("a"), a(), b(), "list", json!({}), vec![]).await.unwrap();
    }
    let stopped = bus.call(Trail::app("a"), a(), b(), "list", json!({}), vec![]).await;
    assert!(matches!(stopped, Err(BusError::Fuse(_))), "{stopped:?}");
    // A person's chain is not counted.
    let person = Trail::principal(&user_principal("u1", "Мама", Role::Admin, None));
    bus.call(person, a(), b(), "list", json!({}), vec![]).await.unwrap();
}
