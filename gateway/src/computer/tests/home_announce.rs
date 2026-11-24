//! The tests of `home/announce.rs` that need the whole gateway (`App`): its crate does not have it, the program does.

use std::net::Ipv4Addr;

use crate::app::App;
use crate::config::{Config, Env};
use crate::home::announce::*;
use crate::stations::connect::discovery::{self, DnsRecord, DUOWAVE_SERVICE};

/// A computer that is its own home's centre says so, with its port, version and home; its answer reads back as
/// the search reads answers (the same parser), PTR to SRV, TXT and A.
#[tokio::test]
async fn a_computer_centre_announces_its_role_port_and_home_and_reads_back() {
    let app = App::build(Config::for_tests(&Env::from_pairs([("STATE_DIR", "")]))).unwrap();
    app.identity.ensure().unwrap();
    let records =
        records(&crate::platform::http::Provides::provide(&app), &app.pc.home, Some(Ipv4Addr::new(192, 168, 1, 20)));
    let packet = discovery::mdns_answer(&records, TTL);
    let read = discovery::parse_records(&packet);
    let found = discovery::stations_in(&read, "192.168.1.30", DUOWAVE_SERVICE).expect("a home in the answer");
    assert_eq!(found.txt.get("role").map(String::as_str), Some("centre"));
    assert_eq!(found.txt.get("port"), Some(&app.config.app.port.to_string()));
    assert_eq!(found.txt.get("version").map(String::as_str), Some(crate::VERSION));
    assert!(found.txt.contains_key("homeId"), "{:?}", found.txt);
    assert!(read.iter().any(|record| matches!(record, DnsRecord::Srv { port, .. } if *port == app.config.app.port)));
}

/// The announcer answers questions about its service only, not other services' or other answers.
#[test]
fn only_a_question_about_duowave_is_answered() {
    assert!(discovery::asks_for(&discovery::mdns_query(DUOWAVE_SERVICE), DUOWAVE_SERVICE));
    assert!(!discovery::asks_for(&discovery::mdns_query(discovery::YANDEX_SERVICE), DUOWAVE_SERVICE));
    let answer = discovery::mdns_answer(&[DnsRecord::Ptr { name: DUOWAVE_SERVICE.into(), target: "x".into() }], TTL);
    assert!(!discovery::asks_for(&answer, DUOWAVE_SERVICE), "an answer is no question");
    assert!(!discovery::asks_for(&[0u8; 3], DUOWAVE_SERVICE), "a broken packet");
}

#[test]
fn names_become_dns_labels() {
    assert_eq!(label("Mac mini «Кабинет»"), "mac-mini");
    assert_eq!(label("DESKTOP-7Q3"), "desktop-7q3");
}
