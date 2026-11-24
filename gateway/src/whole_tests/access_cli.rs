//! The tests of `access/cli.rs` that need the whole gateway (`App`): its crate does not have it, the program does.

use std::sync::Arc;

use crate::access::auth_service::LoginResult;
use crate::access::cli::*;
use crate::app::App;
use crate::config::{Config, Env};

/// A gateway nobody has set up yet, served on loopback as the station's own is: it and its port.
async fn served() -> (Arc<App>, u16) {
    let app = App::build(Config::for_tests(&Env::from_pairs([("STATE_DIR", "")]))).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let serving = app.clone();
    tokio::spawn(async move { crate::app::serve(serving, listener, std::future::pending()).await });
    (app, port)
}

#[tokio::test]
async fn panel_setup_on_a_gateway_with_nobody_makes_the_admin_over_loopback_without_the_code() {
    let (app, port) = served().await;
    assert_eq!(configured(port).await, Some(false));
    assert_eq!(set_up(port, "", "admin-password-1").await, Outcome::Made("admin".into()));
    assert!(app.access.auth.configured().unwrap());
    assert_eq!(configured(port).await, Some(true));
    // The panel signs in with that very password.
    let signed_in = app.access.auth.login(None, "admin-password-1", "127.0.0.1").await.unwrap();
    assert!(matches!(signed_in, LoginResult::Ok(_)), "{signed_in:?}");
}

#[tokio::test]
async fn panel_setup_on_a_gateway_that_has_its_password_refuses_and_changes_nothing() {
    let (app, port) = served().await;
    app.access.auth.setup("admin-password-1", "Мама", "setup").await.unwrap().unwrap();
    assert_eq!(configured(port).await, Some(true));
    assert_eq!(set_up(port, "", "another-password").await, Outcome::AlreadySetUp);
    assert_eq!(app.access.auth.accounts().unwrap(), 1);
    let old = app.access.auth.login(None, "admin-password-1", "127.0.0.1").await.unwrap();
    assert!(matches!(old, LoginResult::Ok(_)), "the first password stands: {old:?}");
}

#[tokio::test]
async fn panel_setup_names_the_admin_as_told_and_says_what_the_gateway_refuses() {
    let (_, port) = served().await;
    assert_eq!(
        set_up(port, "x", "short").await,
        Outcome::Refused("Пароль — хотя бы 8 символов.".into()),
        "the gateway's own words"
    );
    assert_eq!(set_up(port, "Мама", "admin-password-1").await, Outcome::Made("Мама".into()));
}

#[tokio::test]
async fn a_gateway_that_is_not_there_does_not_answer() {
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    assert_eq!(configured(port).await, None);
    assert_eq!(set_up(port, "", "admin-password-1").await, Outcome::NoAnswer);
}
