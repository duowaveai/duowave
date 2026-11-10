//! Access end to end (test/access.e2e-spec.ts): people, keys, the gate. Each test starts its own gateway on a free
//! port and signs in from scratch. What the Node test asked of routes not ported yet — the station's mode and the
//! debug API, the station socket — is asked of the access routes that check the same rights (the household for
//! station.view, the journal for journal.view); the station socket's step waits for the uniproxy.
//!
//! test/access.e2e-spec.ts, the Node original, is removed from the tree: git tag `node-gateway-last`.

mod support;

use reqwest::StatusCode;
use serde_json::{json, Value};
use support::{bearer, message, send, status, until, Gateway};

/// The Node test's environment, without what DATA_DIR already moves.
const ENV: &[(&str, &str)] = &[
    ("UNIPROXY_MODE", "gateway"),
    ("DEBUG_API_ENABLED", "true"),
    ("STATION_PAIRING", "tofu"),
    ("ALLOWED_HOSTS", "gateway.example.org"),
];

const TUNNEL: &[(&str, &str)] = &[("cf-connecting-ip", "203.0.113.7"), ("cf-ray", "8f00000000000000-WAW")];

fn through_tunnel(request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    TUNNEL.iter().fold(request, |request, (name, value)| request.header(*name, *value))
}

/// The first-run setup from this computer: the admin's token.
async fn setup(gateway: &Gateway, password: &str) -> String {
    let setup = message(gateway.post("/api/v1/auth/setup").json(&json!({ "password": password })), StatusCode::CREATED);
    setup.await["token"].as_str().expect("a token").to_string()
}

async fn person_id(gateway: &Gateway, admin: &str, name: &str) -> String {
    let people = message(gateway.get("/api/v1/users").signed(admin), StatusCode::OK).await;
    people.as_array().unwrap().iter().find(|person| person["name"] == name).expect("the person")["id"]
        .as_str()
        .unwrap()
        .to_string()
}

trait Signed {
    fn signed(self, token: &str) -> Self;
}

impl Signed for reqwest::RequestBuilder {
    fn signed(self, token: &str) -> Self {
        let (name, value) = bearer(token);
        self.header(name, value)
    }
}

#[tokio::test]
async fn makes_the_first_password_the_admin_once_from_this_computer_without_a_code() {
    let gateway = Gateway::start(ENV).await;
    assert_eq!(
        message(gateway.get("/api/v1/auth/state"), StatusCode::OK).await,
        json!({"configured": false, "accounts": 0, "needsCode": false})
    );
    assert_eq!(
        status(gateway.post("/api/v1/auth/setup").json(&json!({"password": "short"}))).await,
        StatusCode::BAD_REQUEST
    );
    let setup =
        message(gateway.post("/api/v1/auth/setup").json(&json!({"password": "admin-password-1"})), StatusCode::CREATED)
            .await;
    let admin = setup["token"].as_str().unwrap();
    assert_eq!(setup["user"]["name"], "admin");
    assert_eq!(setup["user"]["role"], "admin");
    assert_eq!(setup["user"]["canSignIn"], true);
    assert_eq!(
        status(gateway.post("/api/v1/auth/setup").json(&json!({"password": "another-password"}))).await,
        StatusCode::FORBIDDEN
    );
    let me = message(gateway.get("/api/v1/auth/me").signed(admin), StatusCode::OK).await;
    let permissions: Vec<&str> = me["permissions"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    for permission in ["users.manage", "models.manage", "stations.manage"] {
        assert!(permissions.contains(&permission), "{permission} in {permissions:?}");
    }
    assert_eq!((me["kind"].as_str(), me["role"].as_str()), (Some("user"), Some("admin")));
    assert_eq!(
        message(gateway.get("/api/v1/auth/state"), StatusCode::OK).await,
        json!({"configured": true, "accounts": 1, "needsCode": false})
    );
}

#[tokio::test]
async fn gives_a_station_the_scripts_for_sdelayu_sam_as_plain_sh_by_the_address_it_asked_connecting_stays_the_admins() {
    let gateway = Gateway::start(ENV).await;
    let admin = setup(&gateway, "admin-password-1").await;
    let script = |host: &str| gateway.get("/station/connect.sh").header(reqwest::header::HOST, host);
    let connect = script("192.0.2.10").send().await.unwrap();
    assert_eq!(connect.status(), StatusCode::OK);
    assert!(connect.headers()["content-type"].to_str().unwrap().contains("text/plain"));
    let text = connect.text().await.unwrap();
    assert!(text.starts_with("#!/bin/sh"), "{text}");
    assert!(text.contains("voice.sh point 'ws://192.0.2.10/uni.ws' ''"), "{text}");
    let named = script("gateway.local:8080").send().await.unwrap().text().await.unwrap();
    assert!(named.contains("voice.sh point 'ws://gateway.local:8080/uni.ws' ''"), "{named}");
    let restore = gateway.get("/station/restore.sh").send().await.unwrap();
    assert_eq!(restore.status(), StatusCode::OK);
    assert!(restore.text().await.unwrap().contains("voice.sh drop"));

    // Connecting one from the panel is a PC's (the station's own build has no SSH). Every request here stops at the
    // gate or at the check of what it asks: nothing reaches a station.
    if cfg!(feature = "pc") {
        assert_eq!(status(gateway.get("/api/v1/stations/connect/discover")).await, StatusCode::UNAUTHORIZED);
        let connect = json!({"address": "192.0.2.23", "gateway": "192.0.2.10"});
        assert_eq!(status(gateway.post("/api/v1/stations/connect").json(&connect)).await, StatusCode::UNAUTHORIZED);
        let check =
            gateway.post("/api/v1/stations/connect/check").signed(&admin).json(&json!({"address": "not-an-address"}));
        assert_eq!(status(check).await, StatusCode::BAD_REQUEST);
        let injected = json!({"address": "192.0.2.23", "gateway": "x;reboot"});
        assert_eq!(
            status(gateway.post("/api/v1/stations/connect").signed(&admin).json(&injected)).await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status(gateway.get("/api/v1/stations/connect/jobs/none").signed(&admin)).await,
            StatusCode::NOT_FOUND
        );
    }
}

#[tokio::test]
async fn closes_every_route_that_does_not_say_otherwise_and_opens_the_stations_own() {
    let gateway = Gateway::start(ENV).await;
    let content = gateway.dir.path().join("content");
    std::fs::create_dir_all(content.join("music")).unwrap();
    std::fs::create_dir_all(content.join("messages")).unwrap();
    std::fs::write(content.join("music/song.mp3"), [1u8; 1024]).unwrap();
    std::fs::write(content.join("messages/voice_мама.ogg"), [2u8; 1024]).unwrap();
    let admin = setup(&gateway, "admin-password-1").await;
    let mut closed = vec![
        "/api/v1/users",
        "/api/v1/auth/me",
        "/api/v1/access/catalog",
        "/api/v1/household",
        "/api/v1/api-keys",
        "/api/v1/audit",
        "/api/v1/requests",
        "/api/v1/live",
        "/api/v1/connectors",
        "/api/v1/stations",
    ];
    if cfg!(feature = "pc") {
        closed.push("/calls/status");
    }
    for path in closed {
        let (code, body) = send(gateway.get(path)).await;
        assert_eq!(code, StatusCode::UNAUTHORIZED, "{path}");
        assert_eq!(body["message"], "Нужно войти.");
        assert_eq!(body["path"], path);
    }
    // The station's own: its connectivity check and our content for its player.
    let check = gateway.get("/generate_204").send().await.unwrap();
    assert_eq!(check.status(), StatusCode::NO_CONTENT);
    let song = gateway.get("/media/music/song.mp3").send().await.unwrap();
    assert_eq!(song.status(), StatusCode::OK);
    assert_eq!(song.bytes().await.unwrap().len(), 1024);
    // The family's voice messages are not served, whoever asks.
    let message = gateway.get("/media/messages/voice_%D0%BC%D0%B0%D0%BC%D0%B0.ogg").signed(&admin);
    assert_eq!(status(message).await, StatusCode::NOT_FOUND);
    // Open to all: the panel's first question, health, the panel's files.
    assert_eq!(status(gateway.get("/api/v1/auth/state")).await, StatusCode::OK);
    assert_eq!(status(gateway.get("/health")).await, StatusCode::OK);
    // The bare address leads to the panel.
    let plain =
        duowave::utils::http::client_builder().redirect(reqwest::redirect::Policy::none()).no_proxy().build().unwrap();
    let root = plain.get(gateway.url("/")).send().await.unwrap();
    assert_eq!(root.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(root.headers()["location"], "/panel");
    let (panel, body) = send(gateway.get("/panel/people")).await;
    assert!(
        panel == StatusCode::OK
            || (panel == StatusCode::NOT_FOUND && body["message"].as_str().unwrap().contains("not built")),
        "{panel} {body}"
    );
    // A path that exists with another method is no route either, as Express said.
    let (code, body) = send(gateway.get("/api/v1/auth/login")).await;
    assert_eq!((code, body["message"].as_str()), (StatusCode::NOT_FOUND, Some("Cannot GET /api/v1/auth/login")));
}

#[tokio::test]
async fn gives_each_role_what_it_may_do_and_ends_a_persons_sessions_when_their_rights_change() {
    let gateway = Gateway::start(ENV).await;
    let admin = setup(&gateway, "admin-password-1").await;
    let add = |body: Value| gateway.post("/api/v1/users").signed(&admin).json(&body);
    assert_eq!(
        status(add(json!({"name": "Аня", "password": "anya-password", "role": "member"}))).await,
        StatusCode::CREATED
    );
    assert_eq!(status(add(json!({"name": "аня", "password": "other-password"}))).await, StatusCode::BAD_REQUEST);
    // With two people, the login asks who.
    assert_eq!(
        status(gateway.post("/api/v1/auth/login").json(&json!({"password": "anya-password"}))).await,
        StatusCode::UNAUTHORIZED
    );
    let anya = message(
        gateway.post("/api/v1/auth/login").json(&json!({"name": "Аня", "password": "anya-password"})),
        StatusCode::OK,
    )
    .await;
    assert_eq!(anya["user"]["role"], "member");
    let token = anya["token"].as_str().unwrap();
    // A member sees the station (station.view) but changes nothing of the house and reads no journal.
    assert_eq!(status(gateway.get("/api/v1/household").signed(token)).await, StatusCode::OK);
    assert_eq!(status(gateway.get("/api/v1/connectors").signed(token)).await, StatusCode::OK);
    let (code, body) = send(gateway.put("/api/v1/household").signed(token).json(&json!({"role": "guest"}))).await;
    assert_eq!(code, StatusCode::FORBIDDEN);
    assert_eq!(body["message"], "Нет права: добавлять людей, менять их роли, права и ключи доступа.");
    assert_eq!(status(gateway.get("/api/v1/users").signed(token)).await, StatusCode::FORBIDDEN);
    assert_eq!(status(gateway.get("/api/v1/requests").signed(token)).await, StatusCode::FORBIDDEN);

    let id = person_id(&gateway, &admin, "Аня").await;
    let change = json!({"role": "guest", "restrictions": {"deny": ["use:music", "use:nonsense"]}});
    assert_eq!(
        status(gateway.patch(&format!("/api/v1/users/{id}")).signed(&admin).json(&change)).await,
        StatusCode::OK
    );
    assert_eq!(status(gateway.get("/api/v1/auth/me").signed(token)).await, StatusCode::UNAUTHORIZED);
    let people = message(gateway.get("/api/v1/users").signed(&admin), StatusCode::OK).await;
    let updated = people.as_array().unwrap().iter().find(|person| person["id"] == id.as_str()).unwrap().clone();
    assert_eq!((updated["role"].as_str(), &updated["restrictions"]), (Some("guest"), &json!({"deny": ["use:music"]})));

    // A profile without a password is somebody the station knows, not an account.
    let kid = message(add(json!({"name": "Миша", "role": "guest"})), StatusCode::CREATED).await;
    assert_eq!(kid["canSignIn"], false);
    assert_eq!(status(add(json!({"name": "Без пароля", "role": "admin"}))).await, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn keeps_an_admin_the_last_one_can_be_neither_demoted_nor_removed() {
    let gateway = Gateway::start(ENV).await;
    let admin = setup(&gateway, "admin-password-1").await;
    let id = person_id(&gateway, &admin, "admin").await;
    let (code, body) =
        send(gateway.patch(&format!("/api/v1/users/{id}")).signed(&admin).json(&json!({"role": "member"}))).await;
    assert_eq!(
        (code, body["message"].as_str()),
        (StatusCode::BAD_REQUEST, Some("Это последний администратор: сначала назначьте другого."))
    );
    assert_eq!(status(gateway.delete(&format!("/api/v1/users/{id}")).signed(&admin)).await, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn changes_ones_own_password_the_old_sessions_end_the_caller_gets_a_new_token() {
    let gateway = Gateway::start(ENV).await;
    let admin = setup(&gateway, "admin-password-1").await;
    let change = |current: &str| {
        gateway
            .put("/api/v1/auth/password")
            .signed(&admin)
            .json(&json!({"current": current, "next": "admin-password-2"}))
    };
    assert_eq!(status(change("wrong")).await, StatusCode::BAD_REQUEST);
    let fresh = message(change("admin-password-1"), StatusCode::OK).await["token"].as_str().unwrap().to_string();
    assert_eq!(status(gateway.get("/api/v1/auth/me").signed(&admin)).await, StatusCode::UNAUTHORIZED);
    assert_eq!(status(gateway.get("/api/v1/auth/me").signed(&fresh)).await, StatusCode::OK);
    // «Выйти везде» ends this session too.
    let (code, body) = send(gateway.post("/api/v1/auth/logout-everywhere").signed(&fresh)).await;
    assert_eq!((code, body), (StatusCode::NO_CONTENT, Value::Null));
    assert_eq!(status(gateway.get("/api/v1/auth/me").signed(&fresh)).await, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn gives_a_script_or_an_agent_a_key_with_exactly_its_rights_shown_once_revocable() {
    let gateway = Gateway::start(ENV).await;
    let admin = setup(&gateway, "admin-password-1").await;
    let created = message(
        gateway.post("/api/v1/api-keys").signed(&admin).json(&json!({"name": "OpenClaw", "grants": ["station.view"]})),
        StatusCode::CREATED,
    )
    .await;
    let secret = created["secret"].as_str().unwrap();
    assert!(secret.starts_with("vgw_"), "{secret}");
    assert_eq!(status(gateway.get("/api/v1/household").signed(secret)).await, StatusCode::OK);
    assert_eq!(
        status(gateway.put("/api/v1/household").signed(secret).json(&json!({"role": "guest"}))).await,
        StatusCode::FORBIDDEN
    );
    let me = message(gateway.get("/api/v1/auth/me").signed(secret), StatusCode::OK).await;
    assert_eq!(
        me,
        json!({"id": created["key"]["id"], "kind": "apiKey", "name": "OpenClaw", "permissions": ["station.view"]})
    );
    assert_eq!(
        status(gateway.post("/api/v1/auth/logout-everywhere").signed(secret)).await,
        StatusCode::FORBIDDEN,
        "for people, not for keys"
    );
    let listed = message(gateway.get("/api/v1/api-keys").signed(&admin), StatusCode::OK).await;
    assert!(!listed.to_string().contains(secret));
    let id = created["key"]["id"].as_str().unwrap();
    assert_eq!(status(gateway.delete(&format!("/api/v1/api-keys/{id}")).signed(&admin)).await, StatusCode::NO_CONTENT);
    assert_eq!(status(gateway.get("/api/v1/household").signed(secret)).await, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn answers_nothing_that_needs_a_sign_in_from_outside_the_home_without_panel_remote() {
    let gateway = Gateway::start(ENV).await;
    let admin = setup(&gateway, "admin-password-1").await;
    let (code, body) = send(through_tunnel(gateway.get("/api/v1/auth/me").signed(&admin))).await;
    assert_eq!(
        (code, body["message"].as_str()),
        (StatusCode::FORBIDDEN, Some("Пульт вне дома выключен (PANEL_REMOTE)."))
    );
    let login = gateway.post("/api/v1/auth/login").json(&json!({"password": "admin-password-1"}));
    assert_eq!(status(through_tunnel(login)).await, StatusCode::FORBIDDEN);
    let setup = gateway.post("/api/v1/auth/setup").json(&json!({"password": "admin-password-1"}));
    assert_eq!(status(through_tunnel(setup)).await, StatusCode::FORBIDDEN, "the first run only from home");
    // Health from outside says only that the gateway is up.
    assert_eq!(message(through_tunnel(gateway.get("/health")), StatusCode::OK).await, json!({"status": "ok"}));
    assert_eq!(message(gateway.get("/health"), StatusCode::OK).await["mode"], "gateway");
}

#[tokio::test]
async fn answers_its_own_names_only_and_says_what_every_response_is() {
    let gateway = Gateway::start(ENV).await;
    let host = |name: &str| gateway.get("/health").header(reqwest::header::HOST, name);
    assert_eq!(status(host("evil.example.com")).await, StatusCode::MISDIRECTED_REQUEST);
    assert_eq!(status(host("gateway.example.org")).await, StatusCode::OK);
    assert_eq!(status(host("mac-mini.local:3000")).await, StatusCode::OK);
    let health = gateway.get("/health").send().await.unwrap();
    assert_eq!(health.headers()["x-content-type-options"], "nosniff");
    // Framed only by itself: the helper's windows of the station are the panel again (docs/plans/desk.md, stage 4).
    assert_eq!(health.headers()["x-frame-options"], "SAMEORIGIN");
}

#[tokio::test]
async fn keeps_who_changed_what() {
    let gateway = Gateway::start(ENV).await;
    let admin = setup(&gateway, "admin-password-1").await;
    let anya = json!({"name": "Аня", "password": "anya-password"});
    assert_eq!(status(gateway.post("/api/v1/users").signed(&admin).json(&anya)).await, StatusCode::CREATED);
    let id = person_id(&gateway, &admin, "Аня").await;
    let change =
        gateway.patch(&format!("/api/v1/users/{id}")).signed(&admin).json(&json!({"profile": {"color": "#112233"}}));
    assert_eq!(status(change).await, StatusCode::OK);
    let key = message(
        gateway.post("/api/v1/api-keys").signed(&admin).json(&json!({"name": "curl", "grants": ["station.view"]})),
        StatusCode::CREATED,
    )
    .await;
    let revoke = gateway.delete(&format!("/api/v1/api-keys/{}", key["key"]["id"].as_str().unwrap())).signed(&admin);
    assert_eq!(status(revoke).await, StatusCode::NO_CONTENT);
    let wrong = gateway.post("/api/v1/auth/login").json(&json!({"name": "Аня", "password": "wrong-password"}));
    assert_eq!(status(wrong).await, StatusCode::UNAUTHORIZED);

    let audit = &gateway.app.access.audit;
    let expected = ["user.created", "user.updated", "apiKey.created", "apiKey.revoked", "auth.login", "auth.failed"];
    let all_written = || {
        let actions: Vec<String> = audit.recent(100).unwrap().into_iter().map(|entry| entry.action).collect();
        expected.iter().all(|action| actions.iter().any(|written| written == action))
    };
    assert!(until(2000, all_written).await, "the audit log follows the bus");
    let entries = message(gateway.get("/api/v1/audit?limit=50").signed(&admin), StatusCode::OK).await;
    let entries = entries.as_array().unwrap();
    let updated = entries.iter().find(|entry| entry["action"] == "user.updated").unwrap();
    assert_eq!(updated["actor"]["name"], "admin");
    assert_eq!(updated["details"]["changes"], json!(["profile"]));
    let failed = entries.iter().find(|entry| entry["action"] == "auth.failed").unwrap();
    assert_eq!((failed["actor"].clone(), failed["details"]["name"].as_str()), (Value::Null, Some("Аня")));
}

#[tokio::test]
async fn makes_a_source_wait_after_five_wrong_passwords_even_with_the_right_one() {
    let gateway = Gateway::start(ENV).await;
    setup(&gateway, "admin-password-2").await;
    for attempt in 0..5 {
        let wrong =
            gateway.post("/api/v1/auth/login").json(&json!({"name": "admin", "password": format!("wrong-{attempt}")}));
        assert_eq!(status(wrong).await, StatusCode::UNAUTHORIZED);
    }
    let right = gateway.post("/api/v1/auth/login").json(&json!({"name": "admin", "password": "admin-password-2"}));
    let (code, body) = send(right).await;
    assert_eq!(
        (code, body["message"].as_str()),
        (StatusCode::TOO_MANY_REQUESTS, Some("Слишком много попыток — подождите 15 мин."))
    );
}

#[tokio::test]
async fn lists_the_catalog_for_the_editors_and_sets_the_households_voice() {
    let gateway = Gateway::start(ENV).await;
    let admin = setup(&gateway, "admin-password-1").await;
    let catalog = message(gateway.get("/api/v1/access/catalog").signed(&admin), StatusCode::OK).await;
    let roles: Vec<&str> = catalog["roles"].as_array().unwrap().iter().filter_map(|role| role["id"].as_str()).collect();
    assert_eq!(roles, ["admin", "member", "guest"]);
    assert_eq!(catalog["roles"][0]["grants"], json!(["*"]));
    assert_eq!(
        catalog["permissions"][0],
        json!({"id": "station.view", "title": "Смотреть колонку: состояние, сессии, маршруты, модели, характер"})
    );
    assert_eq!(
        catalog["capabilities"][0],
        json!({"id": "chat", "title": "Разговор, время, дата", "permission": "use:chat"})
    );
    let household = message(gateway.get("/api/v1/household").signed(&admin), StatusCode::OK).await;
    assert_eq!(household, json!({"role": "member", "restrictions": {}}));
    let guest = message(
        gateway
            .put("/api/v1/household")
            .signed(&admin)
            .json(&json!({"role": "guest", "restrictions": {"allow": ["use:calls"]}})),
        StatusCode::OK,
    )
    .await;
    assert_eq!(guest, json!({"role": "guest", "restrictions": {"allow": ["use:calls"]}}));
    assert_eq!(gateway.app.access.household.speaker(None).role, Some(duowave::core::identity::Role::Guest));
}
