//! The tests of `memory/controller.rs` that need the whole gateway (`App`): its crate does not have it, the program
//! does.

use axum::extract::{Path, Query};
use axum::http::StatusCode;

use crate::app::App;
use crate::config::{Config, Env};
use crate::core::identity::principal::user_principal;
use crate::core::identity::Role;
use crate::memory::controller::*;
use crate::memory::model::{Origin, Source};
use crate::memory::Remember;
use crate::platform::http::{Body, Caller, Uses};

#[tokio::test]
async fn shows_memory_to_who_may_see_it_and_changes_it_only_with_the_right() {
    let app = App::build(Config::for_tests(&Env::from_pairs([("STATE_DIR", "")]))).unwrap();
    let member = || Caller::local(app.clone(), Some(user_principal("u2", "Аня", Role::Member, None)));
    let admin = || Caller::local(app.clone(), Some(user_principal("u1", "Админ", Role::Admin, None)));

    // The admin-only rights: a member sees nothing by default.
    let refused = overview(Uses::of(&app), member()).await.err().unwrap();
    assert_eq!(refused.status, StatusCode::FORBIDDEN);

    app.memory
        .remember(
            Remember {
                fact: "У Маши аллергия на орехи".into(), about: Some("Маша".into()), ..Remember::default()
            },
            Origin::station_voice(Some("kitchen"), Some("alice")),
            Source::Model,
            None,
        )
        .unwrap();
    let shown = overview(Uses::of(&app), admin()).await.unwrap().1;
    assert_eq!(shown.counts.home, 1);
    assert_eq!(shown.recent.len(), 1);
    let masha =
        entities(Uses::of(&app), admin()).await.unwrap().1.into_iter().find(|e| e.entity.name == "Маша").unwrap();
    let card = entity(Uses::of(&app), admin(), Path(masha.entity.id.clone())).await.unwrap().1;
    assert_eq!(card.current.len(), 1);

    // «Больше не так», then «Отменить» it.
    let fact = card.current[0].fact.id.clone();
    close_fact(Uses::of(&app), admin(), Path(fact.clone()), Body(CloseBody::default())).await.unwrap();
    assert_eq!(overview(Uses::of(&app), admin()).await.unwrap().1.counts.home, 0);
    let log = changes(Uses::of(&app), admin(), Query(ChangesQuery { limit: Some(10) })).await.unwrap().1;
    let close = log.iter().find(|c| c.action == "close").unwrap();
    undo(Uses::of(&app), admin(), Path(close.id.clone())).await.unwrap();
    assert_eq!(overview(Uses::of(&app), admin()).await.unwrap().1.counts.home, 1);

    // Off by the switch: the panel's only, with the right.
    let off =
        set_state(Uses::of(&app), admin(), Body(StateBody { enabled: Some(false), in_passing: None })).await.unwrap();
    assert!(!off.1.enabled);
    let paused =
        pause(Uses::of(&app), admin(), Body(PauseBody { span: Some("hour".into()), timezone: None })).await.unwrap();
    assert!(paused.1.paused);
    let erased = erase_fact(Uses::of(&app), admin(), Path(fact)).await.unwrap();
    assert!(erased.1.done);
    assert_eq!(overview(Uses::of(&app), admin()).await.unwrap().1.counts.home, 0);
}
