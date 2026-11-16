//! What works where reads the helpers, end to end (docs/plans/home-and-helpers.md, block 1, stage 5): «таблица реестра
//! для трёх размещений даёт прежние ответы, а причины — новыми словами; «Своё» на колонке без помощника со слухом —
//! отказ словами, как сейчас». The station is a computer's build playing it (DUOWAVE_ROLE=station); its helper is a
//! stand-in on 127.0.0.1 — not its computer, so the placement stays «на колонке». Where the registry meets «Мозг»
//! (stage 4): for a helper away, kept out, switched off or without its bot both give the same verdict, each in its
//! own words.

use std::sync::atomic::Ordering;

use duowave::config::ModelProfile;
use duowave::core::availability::CLOUDS_SHUT;

/// «Своё» on the station with no helper and no cloud hearing set up (docs/plans/speaker-alone.md): the cloud's way
/// says what is missing first.
const NO_CLOUD_HEARING: &str = "Нет облачного распознавания";
const REFUSED_NO_CLOUD: &str = "«Ваш ИИ» сейчас не включить: нет облачного распознавания.";
use duowave::skills::router::{Decision, ModelState, RouteInput};
use reqwest::header::HeaderName;
use reqwest::StatusCode;
use serde_json::{json, Value};

use crate::stand::{contract, stand, Stand, ALL, KEY};
use crate::support::{bearer, message, until, Gateway};

const PEER: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// A station's gateway and its first admin's header.
async fn station() -> (Gateway, (HeaderName, String)) {
    let station = Gateway::start(&[("DUOWAVE_ROLE", "station"), ("PEER_KEY", PEER)]).await;
    let setup = station.post("/api/v1/auth/setup").json(&json!({"password": "registry-admin-password"}));
    let auth = bearer(message(setup, StatusCode::CREATED).await["token"].as_str().expect("a token"));
    (station, auth)
}

/// What works where on this gateway now, as the panel reads it.
async fn availability(gateway: &Gateway, (header, auth): &(HeaderName, String)) -> Value {
    message(gateway.get("/api/v1/availability").header(header.clone(), auth.clone()), StatusCode::OK).await
}

fn feature<'a>(view: &'a Value, id: &str) -> &'a Value {
    view["features"].as_array().unwrap().iter().find(|feature| feature["id"] == id).expect("a feature")
}

/// A feature's state, where it runs and why, as the panel reads them.
fn said(view: &Value, id: &str) -> (String, Option<String>, String) {
    let feature = feature(view, id);
    let text = |key: &str| feature[key].as_str().map(str::to_string);
    (text("state").unwrap_or_default(), text("runs"), text("reason").unwrap_or_default())
}

fn said_as(state: &str, runs: Option<&str>, reason: &str) -> (String, Option<String>, String) {
    (state.to_string(), runs.map(str::to_string), reason.to_string())
}

#[tokio::test]
async fn svoi_on_a_station_whose_helper_does_not_hear_is_refused_with_words_and_the_panel_reads_why() {
    let deaf: Vec<(&str, bool)> = ALL.iter().copied().filter(|(id, _)| *id != "hearing").collect();
    let helper = stand(contract("deaf", &deaf)).await;
    let (station, admin) = station().await;
    let (header, auth) = admin.clone();
    let own =
        || station.post("/api/v1/station/mode").header(header.clone(), auth.clone()).json(&json!({"mode": "own"}));

    // No helper: the station alone, refused as it always was.
    assert_eq!(message(own(), StatusCode::CONFLICT).await, REFUSED_NO_CLOUD);
    let alone = availability(&station, &admin).await;
    assert_eq!(alone["helpers"], json!({"online": false, "configured": [], "up": [], "waiting": [], "builtin": []}));
    assert_eq!(said(&alone, "mode.own"), said_as("off", None, NO_CLOUD_HEARING));

    // A helper on the line that hears nothing, and no cloud hears for it: refused, saying why.
    let add = station.post("/api/v1/helpers").header(header.clone(), auth.clone());
    let added = message(add.json(&json!({"url": helper.url, "key": KEY})), StatusCode::CREATED).await;
    let id = added["helpers"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(message(own(), StatusCode::CONFLICT).await, "«Ваш ИИ» сейчас не включить: нет Турбо со слухом.");
    // A look reads its bot's contacts too: it has no bot (503).
    let check = || station.post(&format!("/api/v1/helpers/{id}/check")).header(header.clone(), auth.clone());
    message(check(), StatusCode::OK).await;
    let deaf = availability(&station, &admin).await;
    assert_eq!((deaf["placement"].as_str(), deaf["homeServer"].as_str()), (Some("station"), Some("none")));
    assert_eq!(said(&deaf, "mode.own"), said_as("off", None, "Нет Турбо со слухом"));
    assert_eq!(said(&deaf, "asr.own"), said_as("fallback", Some("yandex"), "нет Турбо со слухом — у Яндекса"));
    assert_eq!(said(&deaf, "voice.own"), said_as("ready", Some("homeServer"), "у Турбо"));
    assert_eq!(
        deaf["helpers"],
        json!({
            "online": true,
            "configured": ["voice", "model", "messages", "apps", "history", "catalog"],
            "up": ["voice", "model", "apps", "history", "catalog"],
            "waiting": ["messages"],
            "noBot": true,
            "builtin": []
        }),
        "its bot is not set up: messages are not given, the helper is on the line"
    );
    assert_eq!(said(&deaf, "messages"), said_as("off", None, "У Турбо не настроен бот Telegram"));

    // Its bot set up and its hearing ready: «Своё» may be chosen, «передай …» goes, both at the helper —
    // `homeServer` on the wire for the panels of 0.11.
    *helper.contacts.lock().unwrap() = Some(vec!["Мама".into()]);
    *helper.status.lock().unwrap() = contract("deaf", ALL);
    message(check(), StatusCode::OK).await;
    let hears = availability(&station, &admin).await;
    assert_eq!(said(&hears, "mode.own"), said_as("ready", Some("homeServer"), "у Турбо"));
    assert_eq!(said(&hears, "asr.own"), said_as("ready", Some("homeServer"), "у Турбо"));
    assert_eq!(said(&hears, "messages"), said_as("ready", Some("homeServer"), "у Турбо"));
    assert_eq!((&hears["helpers"]["waiting"], hears["helpers"].get("noBot")), (&json!([]), None));
    assert_eq!(message(own(), StatusCode::CREATED).await["mode"], "own");
}

/// The station's own server in its list, named «Мой сервер» — of the kind «own», so «Только дома» keeps it out —: its
/// id, and its model's row in «Мозг», made with its first look.
async fn my_server(station: &Gateway, (header, auth): &(HeaderName, String), helper: &Stand) -> (String, String) {
    let body = json!({"url": helper.url, "key": KEY, "name": "Мой сервер", "kind": "own"});
    let added =
        message(station.post("/api/v1/helpers").header(header.clone(), auth.clone()).json(&body), StatusCode::CREATED)
            .await;
    let id = added["helpers"][0]["id"].as_str().unwrap().to_string();
    let row = || {
        let providers = station.app.brain.view().providers;
        providers.into_iter().find(|row| row.helper.as_ref().is_some_and(|whose| whose.id == id)).map(|row| row.id)
    };
    assert!(until(5000, || row().is_some()).await, "its model's row comes with its first look");
    let row = row().unwrap();
    (id, row)
}

/// «Мозг»'s choice: `{fast, backup?}`.
async fn choose(station: &Gateway, (header, auth): &(HeaderName, String), choice: Value) {
    message(station.put("/api/v1/brain/choice").header(header.clone(), auth.clone()).json(&choice), StatusCode::OK)
        .await;
}

/// «Где сейчас»: who gives the ability now, by name, and why not the first.
async fn now(
    station: &Gateway,
    (header, auth): &(HeaderName, String),
    ability: &str,
) -> (String, Option<String>, Option<String>) {
    let view = message(station.get("/api/v1/helpers").header(header.clone(), auth.clone()), StatusCode::OK).await;
    let row = view["now"].as_array().unwrap().iter().find(|row| row["ability"] == ability).expect("a row").clone();
    let text = |key: &str| row[key].as_str().map(str::to_string);
    (text("by").unwrap_or_default(), text("name"), text("why"))
}

fn now_as(by: &str, name: Option<&str>, why: Option<&str>) -> (String, Option<String>, Option<String>) {
    (by.to_string(), name.map(str::to_string), why.map(str::to_string))
}

/// Where a chat question goes in «Вместе»: the router asked as the station's session asks it — the model's state from
/// «Мозг», Yandex's session up.
fn routed(station: &Gateway) -> Decision {
    let dialog = &station.app.dialog;
    let input = RouteInput {
        text: "Как дела у тебя?".into(),
        yandex_available: true,
        model: ModelState {
            enabled: dialog.enabled(),
            tools: dialog.tools_enabled(),
            unreachable_fast: !dialog.reachable(ModelProfile::Fast),
            unreachable_smart: !dialog.reachable(ModelProfile::Smart),
        },
        ..RouteInput::default()
    };
    station.app.skills.router.decide(&input)
}

/// «Спросить мозг»: the answer's text, or the words of its error.
async fn ask(station: &Gateway, (header, auth): &(HeaderName, String)) -> Result<String, String> {
    let question = json!({"question": "Который час?"});
    let asked = station.post("/api/v1/brain/ask").header(header.clone(), auth.clone()).json(&question);
    let body = asked.send().await.unwrap().text().await.unwrap();
    let events: Vec<Value> = body
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|data| serde_json::from_str(data).ok())
        .collect();
    let of = |kind: &str, key: &str| {
        events.iter().find(|event| event["type"] == kind).and_then(|event| event[key].as_str()).map(str::to_string)
    };
    match (of("text", "text"), of("error", "message")) {
        (Some(text), None) => Ok(text),
        (None, Some(words)) => Err(words),
        _ => panic!("{events:?}"),
    }
}

fn hits(helper: &Stand) -> usize {
    helper.llm_hits.load(Ordering::SeqCst)
}

/// Where the registry meets «Мозг» (stage 4): a helper that does not give the model now — away, kept out by «Только
/// дома», switched off — is off in the registry with its reason, «Своё» is refused with it, and «Мозг» never asks it:
/// without a backup its words come, «Где сейчас» gives the model's row to Alice, and the station's router gives her a
/// chat question in «Вместе» (`ModelAway`). Without its bot only `messages` is off — the registry and «Где сейчас» say
/// so by one rule —; «Своё» and brain.local stay ready, and its model still answers.
#[tokio::test]
async fn a_helper_away_kept_out_or_switched_off_is_off_in_the_registry_and_never_asked_by_the_brain() {
    let helper = stand(contract("srv", ALL)).await;
    *helper.contacts.lock().unwrap() = Some(vec!["Мама".into()]);
    let (station, admin) = station().await;
    let (header, auth) = admin.clone();
    let (id, row) = my_server(&station, &admin, &helper).await;
    choose(&station, &admin, json!({"fast": {"provider": row, "model": "home"}})).await;
    let check = || station.post(&format!("/api/v1/helpers/{id}/check")).header(header.clone(), auth.clone());
    let own =
        || station.post("/api/v1/station/mode").header(header.clone(), auth.clone()).json(&json!({"mode": "own"}));
    let away = |away: &str| {
        station.put("/api/v1/helpers/away").header(header.clone(), auth.clone()).json(&json!({"away": away}))
    };
    let switch = |on: bool| {
        station.put(&format!("/api/v1/helpers/{id}")).header(header.clone(), auth.clone()).json(&json!({"enabled": on}))
    };
    let model = || Decision::Model { command: false, profile: ModelProfile::Fast };
    message(check(), StatusCode::OK).await;

    // On the line: it gives hearing, voice, the model and its bot; its model answers.
    let at_helper = said_as("ready", Some("homeServer"), "у Турбо");
    let view = availability(&station, &admin).await;
    for id in ["mode.own", "brain.local", "messages"] {
        assert_eq!(said(&view, id), at_helper, "{id}");
    }
    assert_eq!(now(&station, &admin, "model").await, now_as("helper", Some("Мой сервер"), None));
    assert_eq!(routed(&station), model());
    assert_eq!(ask(&station, &admin).await, Ok("Ответ Турбо.".to_string()));
    assert_eq!(hits(&helper), 1);

    // Away: «помощник не на связи» in the registry and in «Мозг»'s words.
    let words = "Мой сервер не на связи, а запасной модели нет.";
    helper.set_away(true);
    message(check(), StatusCode::OK).await;
    message(check(), StatusCode::OK).await;
    let view = availability(&station, &admin).await;
    for id in ["mode.own", "brain.local"] {
        assert_eq!(said(&view, id), said_as("off", None, "Турбо не на связи"), "{id}");
    }
    assert_eq!(message(own(), StatusCode::CONFLICT).await, "«Ваш ИИ» сейчас не включить: Турбо не на связи.");
    assert_eq!(now(&station, &admin, "model").await, now_as("yandex", None, Some(words)));
    assert_eq!(routed(&station), Decision::ModelAway);
    assert_eq!(ask(&station, &admin).await, Err(words.to_string()));
    helper.set_away(false);
    message(check(), StatusCode::OK).await;
    assert_eq!(routed(&station), model(), "back on the line");

    // Kept out by «Только дома»: the registry names the regulator, «Мозг» says the clouds are closed — one's own server
    // is «моё облако» to the regulator.
    let words = "Модель в облаке, а облака сейчас закрыты.";
    message(away("home"), StatusCode::OK).await;
    let view = availability(&station, &admin).await;
    for id in ["mode.own", "brain.local"] {
        assert_eq!(said(&view, id), said_as("off", None, "Регулятор не пускает Турбо"), "{id}");
    }
    assert_eq!(said(&view, "brain.cloud"), said_as("off", None, "Облака закрыты регулятором"));
    assert_eq!(message(own(), StatusCode::CONFLICT).await, "«Ваш ИИ» сейчас не включить: регулятор не пускает Турбо.");
    assert_eq!(now(&station, &admin, "model").await, now_as("yandex", None, Some(words)));
    assert_eq!(routed(&station), Decision::ModelAway);
    assert_eq!(ask(&station, &admin).await, Err(words.to_string()));
    message(away("any"), StatusCode::OK).await;
    message(check(), StatusCode::OK).await;
    assert_eq!(routed(&station), model(), "let out and looked at");

    // Switched off: no helper to lean on in the registry — the station alone —; «Мозг» keeps the row its choice answers
    // with and takes it as away.
    let words = "Мой сервер не на связи, а запасной модели нет.";
    message(switch(false), StatusCode::OK).await;
    let view = availability(&station, &admin).await;
    assert_eq!(said(&view, "mode.own"), said_as("off", None, NO_CLOUD_HEARING));
    let alone = duowave::core::availability::feature("brain.local").unwrap().absent;
    assert_eq!(said(&view, "brain.local"), said_as("off", None, alone));
    assert_eq!(message(own(), StatusCode::CONFLICT).await, REFUSED_NO_CLOUD);
    assert_eq!(now(&station, &admin, "model").await, now_as("yandex", None, Some(words)));
    assert_eq!(routed(&station), Decision::ModelAway);
    assert_eq!(ask(&station, &admin).await, Err(words.to_string()));
    assert_eq!(hits(&helper), 1, "away, kept out or switched off, it is never asked");
    message(switch(true), StatusCode::OK).await;
    message(check(), StatusCode::OK).await;

    // Without its bot only its messages are off — the registry and «Где сейчас» by one rule —; «Своё» and brain.local
    // stay ready, its model answers as before.
    *helper.contacts.lock().unwrap() = None;
    message(check(), StatusCode::OK).await;
    let view = availability(&station, &admin).await;
    assert_eq!(said(&view, "messages"), said_as("off", None, "У Турбо не настроен бот Telegram"));
    assert_eq!(
        now(&station, &admin, "messages").await,
        now_as("none", None, Some("Мой сервер: не настроен бот Telegram"))
    );
    for id in ["mode.own", "brain.local"] {
        assert_eq!(said(&view, id), at_helper, "{id}");
    }
    assert_eq!(now(&station, &admin, "model").await, now_as("helper", Some("Мой сервер"), None));
    assert_eq!(routed(&station), model());
    assert_eq!(ask(&station, &admin).await, Ok("Ответ Турбо.".to_string()));
    assert_eq!(hits(&helper), 2);
    station.stop().await;
}

/// With a backup «Мозг» answers at once (stage 4) where the registry has its first choice off, the same verdict in each
/// one's words: the helper away — «Помощник не на связи» in the registry, «Мой сервер не на связи» in «Мозг»; switched
/// off — the station alone's words in the registry (no helper to lean on), «Мой сервер не на связи» in «Мозг»; a cloud
/// chosen while «Дом и моё облако» keeps the clouds in — «облака закрыты регулятором», the one word of the registry's
/// clouds and «Мозг»'s, and the helper's model behind it answers. The question stays the model's in «Вместе».
#[tokio::test]
async fn with_a_backup_the_brain_answers_at_once_for_the_reason_the_registry_gives() {
    let helper = stand(contract("srv", ALL)).await;
    let (station, admin) = station().await;
    let (header, auth) = admin.clone();
    let (id, row) = my_server(&station, &admin, &helper).await;
    let provider = |title: &str, url: &str| {
        let body = json!({"preset": "custom-openai", "title": title, "baseUrl": url});
        station.post("/api/v1/brain/providers").header(header.clone(), auth.clone()).json(&body)
    };
    // A backup at home, and a cloud: decided on here, never asked.
    let spare = message(provider("Запас", "http://127.0.0.1:9/v1"), StatusCode::CREATED).await;
    let cloud = message(provider("Облако", "https://llm.example.invalid/v1"), StatusCode::CREATED).await;
    let mine = json!({"provider": row, "model": "home"});
    choose(&station, &admin, json!({"fast": mine, "backup": {"provider": spare["id"], "model": "spare-model"}})).await;
    let check = || station.post(&format!("/api/v1/helpers/{id}/check")).header(header.clone(), auth.clone());
    let switch = |on: bool| {
        station.put(&format!("/api/v1/helpers/{id}")).header(header.clone(), auth.clone()).json(&json!({"enabled": on}))
    };
    let model = || Decision::Model { command: false, profile: ModelProfile::Fast };
    message(check(), StatusCode::OK).await;
    assert_eq!(now(&station, &admin, "model").await, now_as("helper", Some("Мой сервер"), None));

    // Away: the backup at once, «Мой сервер не на связи» — the helper off in the registry.
    helper.set_away(true);
    message(check(), StatusCode::OK).await;
    message(check(), StatusCode::OK).await;
    let view = availability(&station, &admin).await;
    assert_eq!(said(&view, "brain.local"), said_as("off", None, "Турбо не на связи"));
    assert_eq!(now(&station, &admin, "model").await, now_as("here", Some("Запас"), Some("Мой сервер не на связи")));
    assert_eq!(routed(&station), model(), "the backup answers: the question stays the model's");
    helper.set_away(false);
    message(check(), StatusCode::OK).await;

    // Switched off: the backup at once as well; the registry has no helper to lean on — the station alone.
    message(switch(false), StatusCode::OK).await;
    let view = availability(&station, &admin).await;
    let alone = duowave::core::availability::feature("brain.local").unwrap().absent;
    assert_eq!(said(&view, "brain.local"), said_as("off", None, alone));
    assert_eq!(now(&station, &admin, "model").await, now_as("here", Some("Запас"), Some("Мой сервер не на связи")));
    assert_eq!(routed(&station), model());
    message(switch(true), StatusCode::OK).await;
    message(check(), StatusCode::OK).await;

    // A cloud chosen, the helper's model behind it, «Дом и моё облако»: the registry closes the clouds' models, «Мозг»
    // takes the helper's at once for the same word.
    choose(&station, &admin, json!({"fast": {"provider": cloud["id"], "model": "cloud-model"}, "backup": mine})).await;
    let to = station.put("/api/v1/helpers/away").header(header.clone(), auth.clone()).json(&json!({"away": "mine"}));
    message(to, StatusCode::OK).await;
    let view = availability(&station, &admin).await;
    let shut = said(&view, "brain.cloud");
    assert_eq!(shut, said_as("off", None, "Облака закрыты регулятором"));
    assert_eq!(said(&view, "brain.local"), said_as("ready", Some("homeServer"), "у Турбо"));
    let (by, name, why) = now(&station, &admin, "model").await;
    assert_eq!((by.as_str(), name.as_deref(), why.as_deref()), ("helper", Some("Мой сервер"), Some(CLOUDS_SHUT)));
    assert_eq!(shut.2.to_lowercase(), CLOUDS_SHUT, "one word");
    assert_eq!(routed(&station), model());
    assert_eq!(ask(&station, &admin).await, Ok("Ответ Турбо.".to_string()));
    assert_eq!(hits(&helper), 1, "the helper's model only once it stands for the cloud");
    station.stop().await;
}
