//! The tests of `gateway/uniproxy/session_registry.rs` that need the whole gateway (`App`): its crate does not have it,
//! the program does.

use crate::app::App;
use crate::config::{Config, Env};

#[tokio::test]
async fn counts_a_socket_from_its_upgrade_on_until_its_session_is_served_or_its_slot_goes() {
    let app = App::build(Config::for_tests(&Env::from_pairs([("STATE_DIR", "")]))).unwrap();
    let sessions = &app.sessions;
    let first = sessions.reserve("192.168.1.50", 2, 3).unwrap();
    let _second = sessions.reserve("192.168.1.50", 2, 3).unwrap();
    // Neither is served yet, and a third from that address is already one too many.
    assert_eq!(sessions.reserve("192.168.1.50", 2, 3).err(), Some((2, 2)));
    let _other = sessions.reserve("192.168.1.51", 2, 3).unwrap();
    assert_eq!(sessions.reserve("192.168.1.52", 2, 3).err(), Some((3, 0)), "in all");
    // An upgrade that never completed gives its place back.
    drop(first);
    assert!(sessions.reserve("192.168.1.50", 2, 3).is_ok());
}

#[tokio::test]
async fn lets_through_no_more_than_the_limit_of_upgrades_that_come_at_once() {
    let app = App::build(Config::for_tests(&Env::from_pairs([("STATE_DIR", "")]))).unwrap();
    let sessions = &app.sessions;
    let slots: Vec<_> = std::thread::scope(|scope| {
        let burst: Vec<_> = (0..40).map(|_| scope.spawn(|| sessions.reserve("192.168.1.60", 16, 64).ok())).collect();
        burst.into_iter().map(|upgrade| upgrade.join().unwrap()).collect()
    });
    assert_eq!(slots.iter().flatten().count(), 16);
}
