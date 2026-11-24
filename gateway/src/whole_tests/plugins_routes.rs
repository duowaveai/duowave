//! The tests of `plugins/routes.rs` that need the whole gateway (`App`): its crate does not have it, the program does.

use axum::extract::{Path, RawQuery};
use axum::http::{header, StatusCode};
use serde_json::json;

use crate::config::{Config, Env};
use crate::core::identity::principal::user_principal;
use crate::core::identity::Role;
use crate::platform::http::{ApiResult, Body, Caller, Uses};
use crate::plugins::catalog::tests::{arthur, site};
use crate::plugins::install::tests::{character_manifest, vgp};
use crate::plugins::routes::*;

fn status<T>(result: ApiResult<T>) -> StatusCode {
    match result {
        Ok(answer) => answer.0,
        Err(error) => error.status,
    }
}

/// The switch «Расписания плагинов»: whoever sees the connectors sees it, only the admin turns it, and it is kept;
/// a plugin's schedule is seen the same way, its jobs paused and run by the admin alone.
#[tokio::test]
async fn the_schedules_switch_and_a_plugins_schedule_are_the_admins_to_change() {
    let app = crate::app::App::build(Config::for_tests(&Env::from_pairs([("STATE_DIR", "")]))).unwrap();
    let admin = || Caller::local(app.clone(), Some(user_principal("u1", "Админ", Role::Admin, None)));
    let member = || Caller::local(app.clone(), Some(user_principal("u2", "Аня", Role::Member, None)));
    let seen = schedules_switch(Uses::of(&app), member()).await.unwrap().1;
    assert_eq!(seen, json!({ "on": true, "allowed": true }));
    let refused = schedules_set(Uses::of(&app), member(), Body(SwitchBody { on: false })).await;
    assert_eq!(status(refused), StatusCode::FORBIDDEN);
    let off = schedules_set(Uses::of(&app), admin(), Body(SwitchBody { on: false })).await.unwrap().1;
    assert_eq!(off, json!({ "on": false, "allowed": true }));
    assert!(!app.schedules.is_on());
    // A plugin with no schedule: nothing to show, nothing to run.
    let none = schedule(Uses::of(&app), member(), Path("nothing".into())).await.unwrap().1;
    assert_eq!(none, json!({ "on": false, "jobs": [] }));
    let pause = schedule_pause(
        Uses::of(&app),
        member(),
        Path(("nothing".into(), "x".into())),
        Body(PauseBody { paused: true }),
    )
    .await;
    assert_eq!(status(pause), StatusCode::FORBIDDEN);
    let run = schedule_run(Uses::of(&app), admin(), Path(("nothing".into(), "x".into()))).await;
    assert_eq!(status(run), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn installs_and_removes_by_the_packages_kind_and_never_code_from_an_unsigned_catalog() {
    let site = site().await;
    let mut theme = arthur("1.0.0");
    theme["id"] = json!("dusk");
    theme["kind"] = json!("theme");
    let mut plugin = arthur("1.0.0");
    // Not a built-in's id: «Радио» is laid out by the build itself.
    plugin["id"] = json!("clock");
    plugin["kind"] = json!("plugin");
    let mut dusk = character_manifest("dusk", "1.0.0");
    dusk["kind"] = json!("theme");
    dusk["contributes"] = json!({ "themes": [] });
    site.publish(&[
        (arthur("1.2.0"), vgp(&character_manifest("arthur", "1.2.0"))),
        (theme, vgp(&dusk)),
        (plugin, b"code".to_vec()),
    ]);
    let dir = tempfile::tempdir().unwrap();
    let text = |sub: &str| dir.path().join(sub).to_string_lossy().into_owned();
    let env = Env::from_pairs([
        ("STATE_DIR", String::new()),
        ("PACKAGES_DIR", text("packages")),
        ("VOICE_LAB_DIR", text("voice-lab")),
        ("TTS_MODELS_DIR", text("voices")),
        ("DATA_DIR", text("data")),
        ("PACKAGES_FEED_URL", site.url.clone()),
    ]);
    let app = crate::app::App::build(Config::for_tests(&env)).unwrap();
    let admin = || Caller::local(app.clone(), Some(user_principal("u1", "Админ", Role::Admin, None)));
    let member = || Caller::local(app.clone(), Some(user_principal("u2", "Аня", Role::Member, None)));
    let body = || Body(InstallBody::default());

    let listed = catalog(Uses::of(&app), member(), RawQuery(None)).await.unwrap().1;
    assert_eq!(listed.packages.len(), 3);
    let installing = install(
        Uses::of(&app),
        Uses::of(&app),
        Uses::of(&app),
        Uses::of(&app),
        member(),
        Path("arthur".into()),
        body(),
    )
    .await;
    assert_eq!(status(installing), StatusCode::FORBIDDEN, "a character is who may change the character's");
    let installed =
        install(Uses::of(&app), Uses::of(&app), Uses::of(&app), Uses::of(&app), admin(), Path("arthur".into()), body())
            .await
            .unwrap();
    assert_eq!((installed.0, installed.1.source.as_str()), (StatusCode::CREATED, "catalog"));
    assert!(app.characters.character("arthur:arthur").is_some(), "its characters at once");
    assert_eq!(
        status(
            install(
                Uses::of(&app),
                Uses::of(&app),
                Uses::of(&app),
                Uses::of(&app),
                member(),
                Path("dusk".into()),
                body()
            )
            .await
        ),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        status(
            install(
                Uses::of(&app),
                Uses::of(&app),
                Uses::of(&app),
                Uses::of(&app),
                admin(),
                Path("dusk".into()),
                body()
            )
            .await
        ),
        StatusCode::CREATED
    );
    let unsigned =
        install(Uses::of(&app), Uses::of(&app), Uses::of(&app), Uses::of(&app), admin(), Path("clock".into()), body())
            .await;
    assert_eq!(status(unsigned), StatusCode::CONFLICT);
    assert!(!dir.path().join("packages/clock").exists());
    // The built-in «Радио» is there from the build (under its random id), and it is not removed.
    assert!(dir.path().join("packages/p-a2fd1d7eb8a4").is_dir());
    assert_eq!(
        status(
            remove(
                Uses::of(&app),
                Uses::of(&app),
                Uses::of(&app),
                Uses::of(&app),
                Uses::of(&app),
                Uses::of(&app),
                Uses::of(&app),
                Uses::of(&app),
                Uses::of(&app),
                admin(),
                Path("p-a2fd1d7eb8a4".into())
            )
            .await
        ),
        StatusCode::CONFLICT
    );

    // The owner's switch: off, nothing installs.
    let off = catalog_switch(Uses::of(&app), admin(), Body(CatalogSwitch { enabled: Some(false), show_hidden: None }))
        .await
        .unwrap();
    assert!(!off.1.enabled);
    assert_eq!(
        status(update(Uses::of(&app), Uses::of(&app), admin(), Path("arthur".into())).await),
        StatusCode::CONFLICT
    );
    let switched =
        catalog_switch(Uses::of(&app), member(), Body(CatalogSwitch { enabled: Some(true), show_hidden: None })).await;
    assert_eq!(status(switched), StatusCode::FORBIDDEN);

    assert_eq!(
        status(
            remove(
                Uses::of(&app),
                Uses::of(&app),
                Uses::of(&app),
                Uses::of(&app),
                Uses::of(&app),
                Uses::of(&app),
                Uses::of(&app),
                Uses::of(&app),
                Uses::of(&app),
                member(),
                Path("arthur".into())
            )
            .await
        ),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        status(
            remove(
                Uses::of(&app),
                Uses::of(&app),
                Uses::of(&app),
                Uses::of(&app),
                Uses::of(&app),
                Uses::of(&app),
                Uses::of(&app),
                Uses::of(&app),
                Uses::of(&app),
                admin(),
                Path("arthur".into())
            )
            .await
        ),
        StatusCode::OK
    );
    assert!(app.characters.character("arthur:arthur").is_none());
    let image = preview(Uses::of(&app), member(), Path(("arthur".into(), "image".into()))).await.unwrap();
    assert_eq!(image.headers()[header::CONTENT_TYPE], "image/webp");
}

/// Clean-path check 18.11, item 23: the rights approved with an install become links although the package is not
/// switched on yet — the install approves them before it switches on.
#[tokio::test]
async fn the_rights_approved_with_an_install_are_given_before_the_package_is_switched_on() {
    let dir = tempfile::tempdir().unwrap();
    let packages = dir.path().join("packages");
    let mut manifest = crate::plugins::registry::tests::manifest("word", "1.0.0");
    manifest["permissions"] = json!(["call:system/voice"]);
    crate::plugins::registry::tests::write_package(&packages, &manifest, Some("export function f() { return 1; }"));
    let env = Env::from_pairs([
        ("STATE_DIR", String::new()),
        ("PACKAGES_DIR", packages.to_string_lossy().into_owned()),
        ("DATA_DIR", dir.path().join("data").to_string_lossy().into_owned()),
        ("PLUGINS_BUNDLED", "false".into()),
    ]);
    let app = crate::app::App::build(Config::for_tests(&env)).unwrap();
    app.plugins.rescan();
    assert!(app.plugins.installed("word").is_some() && app.plugins.package("word").is_none(), "in place, off");
    let admin = Caller::local(app.clone(), Some(user_principal("u1", "Админ", Role::Admin, None)));
    approve_rights(&app.plugins, &app.bus, &admin, "word", None);
    let links = app.bus.links.all();
    assert!(
        links.iter().any(|link| link.source == crate::bus::links::LinkSource::Install
            && link.to == crate::bus::address::Pattern::parse("system/voice").unwrap()),
        "{links:?}"
    );
}
