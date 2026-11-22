//! The helpers and the choice of road end to end (docs/plans/home-and-helpers.md, block 1, stage 3): «с подставным
//! помощником (тестовый сервер `/home/v1`) слух, голос, модель и приложения идут к первому, кто на связи; выключенный —
//! к следующему; регулятор «Только дома» не пускает облако; старые `/api/v1/home-server*` работают поверх списка».
//! The station is a computer's build playing it (DUOWAVE_ROLE=station); its helpers are stand-ins on 127.0.0.1.

use duowave::config::TtsProvider;
use duowave::helpers::contract::Ability;
use duowave::helpers::Giver;
use reqwest::header::HeaderName;
use reqwest::StatusCode;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::stand::{contract, stand, status_0_11, ALL, KEY};
use crate::support::{bearer, message, send, until, Gateway};

const PEER: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// The first admin's header.
async fn admin(gateway: &Gateway) -> (HeaderName, String) {
    let setup = gateway.post("/api/v1/auth/setup").json(&json!({"password": "helpers-admin-password"}));
    bearer(message(setup, StatusCode::CREATED).await["token"].as_str().expect("a token"))
}

/// A station's gateway with these more variables, and its admin.
async fn station(vars: &[(&str, &str)]) -> (Gateway, (HeaderName, String)) {
    let mut all = vec![("DUOWAVE_ROLE", "station"), ("PEER_KEY", PEER)];
    all.extend_from_slice(vars);
    let station = Gateway::start(&all).await;
    let auth = admin(&station).await;
    (station, auth)
}

/// Who gives the ability on this gateway now, by entry id.
fn by(gateway: &Gateway, ability: Ability) -> Option<String> {
    match gateway.app.helpers.pick(ability)? {
        Giver::Helper(remote) => Some(remote.id().to_string()),
        Giver::Itself => Some("self".into()),
    }
}

/// The station's recognition now: what it hears.
async fn hears(gateway: &Gateway) -> String {
    let recognizer = gateway.app.speech.recognizer().expect("the station's chain");
    recognizer
        .recognize(&[0i16; 1600], 16000, &CancellationToken::new())
        .await
        .unwrap_or_else(|error| format!("{error}"))
}

/// The id and the base URL of the row of a helper's model in «Мозг», when it has one (stage 4: one per helper).
fn model_row(gateway: &Gateway, helper: &str) -> Option<(String, Option<String>)> {
    let view = gateway.app.brain.view();
    let row = view.providers.into_iter().find(|row| row.helper.as_ref().is_some_and(|whose| whose.id == helper))?;
    Some((row.id, row.base_url))
}

fn now<'a>(view: &'a Value, ability: &str) -> &'a Value {
    view["now"].as_array().unwrap().iter().find(|row| row["ability"] == ability).expect("a row")
}

/// The app «Тяжёлое», which prefers home (`prefer: home`), in the gateway's DATA_DIR as an install leaves it: here it
/// would answer «здесь».
fn install_heavy(gateway: &Gateway) {
    let dir = gateway.dir.path().join("packages").join("heavy").join("1.0.0");
    std::fs::create_dir_all(&dir).unwrap();
    let manifest = json!({
        "schema": 1, "id": "heavy", "version": "1.0.0", "title": "Тяжёлое", "api": "^2",
        "runs": ["pc", "station", "stationServer"], "prefer": "home",
        "contributes": { "panel": { "calls": ["work"] } }
    });
    std::fs::write(dir.join("manifest.json"), manifest.to_string()).unwrap();
    std::fs::write(dir.join("backend.js"), "export function work() { return \"здесь\"; }").unwrap();
}

/// The panel's call of «Тяжёлое»: what it answered, and where its calls go now (`runsAt`).
async fn work(gateway: &Gateway, (header, auth): &(HeaderName, String)) -> (Value, Value) {
    let call = gateway.post("/api/v1/packages/heavy/call/work").header(header.clone(), auth.clone()).json(&json!({}));
    let answer = message(call, StatusCode::OK).await["result"].clone();
    let view =
        message(gateway.get("/api/v1/packages/heavy").header(header.clone(), auth.clone()), StatusCode::OK).await;
    (answer, view["runsAt"].clone())
}

#[tokio::test]
async fn a_station_takes_hearing_voice_model_and_apps_from_the_first_helper_up_and_goes_down_the_list() {
    let (a, b) = (stand(contract("a", ALL)).await, stand(contract("b", ALL)).await);
    *a.heard.lock().unwrap() = "раз".into();
    *b.heard.lock().unwrap() = "два".into();
    let (station, (header, auth)) = station(&[]).await;
    let add = |url: &str| {
        station.post("/api/v1/helpers").header(header.clone(), auth.clone()).json(&json!({"url": url, "key": KEY}))
    };
    let added = message(add(&a.url), StatusCode::CREATED).await;
    let a_id = added["helpers"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(added["helpers"][0]["abilities"].as_array().unwrap().len(), 7, "what it gives, shown: {added}");
    let added = message(add(&b.url), StatusCode::CREATED).await;
    let b_id = added["helpers"][1]["id"].as_str().unwrap().to_string();
    assert_eq!(
        (added["helpers"][0]["state"].as_str(), added["helpers"][1]["state"].as_str()),
        (Some("up"), Some("up"))
    );
    // The «Голос» window's choice: the helpers' voice (Alice's only when none gives one).
    station.app.voice.use_home().unwrap();
    let admin = (header.clone(), auth.clone());
    install_heavy(&station);
    let enable = station.post("/api/v1/packages/heavy/enable").header(header.clone(), auth.clone()).json(&json!({}));
    message(enable, StatusCode::OK).await;

    // Hearing, voice, apps and the model: the first helper on the line. The placement stays the plain station's
    // (no `home-computer`): the app goes to the helper all the same.
    assert_eq!(hears(&station).await, "раз");
    assert_eq!(station.app.voice.speaker().unwrap().base_url(), format!("{}/home/v1/tts", a.url));
    let apps = station.app.helpers.link_for(Ability::Apps);
    assert_eq!(apps.up().map(|(url, _)| url).as_deref(), Some(a.url.as_str()));
    assert_eq!(work(&station, &admin).await, (json!("heavy: раз"), json!("home")));
    assert_eq!((a.app_calls(), b.app_calls()), (1, 0));
    let at = |id: &str, url: &str| model_row(&station, id).and_then(|(_, at)| at) == Some(format!("{url}/home/v1/llm"));
    assert!(until(5000, || at(&a_id, &a.url) && at(&b_id, &b.url)).await, "each helper's model has its row");

    // Switched off: the next one for all of them.
    let put = |id: &str, body: Value| {
        station.put(&format!("/api/v1/helpers/{id}")).header(header.clone(), auth.clone()).json(&body)
    };
    message(put(&a_id, json!({"enabled": false})), StatusCode::OK).await;
    assert_eq!(hears(&station).await, "два");
    assert_eq!(station.app.voice.speaker().unwrap().base_url(), format!("{}/home/v1/tts", b.url));
    assert_eq!(apps.up().map(|(url, _)| url).as_deref(), Some(b.url.as_str()));
    assert_eq!(work(&station, &admin).await, (json!("heavy: два"), json!("home")));
    assert_eq!((a.app_calls(), b.app_calls()), (1, 1));
    assert!(until(5000, || model_row(&station, &a_id).is_none()).await, "its model goes with it: nobody chose it");
    // «Мозг» answers with the second helper's model: «Где сейчас» says whose.
    let (row, _) = model_row(&station, &b_id).unwrap();
    let fast = serde_json::Map::from_iter([("fast".to_string(), json!({"provider": row, "model": "home"}))]);
    station.app.brain.choose(&fast, None).unwrap();
    let view = message(station.get("/api/v1/helpers").header(header.clone(), auth.clone()), StatusCode::OK).await;
    let model = now(&view, "model");
    assert_eq!((model["by"].as_str(), model["helper"].as_str()), (Some("helper"), Some(b_id.as_str())), "{model}");

    // On again, then away: two looks — hearing and voice go down the list, and «Где сейчас» says why.
    message(put(&a_id, json!({"enabled": true})), StatusCode::OK).await;
    a.set_away(true);
    let check = || station.post(&format!("/api/v1/helpers/{a_id}/check")).header(header.clone(), auth.clone());
    message(check(), StatusCode::OK).await;
    let view = message(check(), StatusCode::OK).await;
    assert_eq!(view["helpers"][0]["state"], "down");
    assert_eq!(
        (by(&station, Ability::Hearing), by(&station, Ability::Voice)),
        (Some(b_id.clone()), Some(b_id.clone()))
    );
    assert_eq!(hears(&station).await, "два");
    let hearing = now(&view, "hearing");
    assert_eq!((hearing["by"].as_str(), hearing["helper"].as_str()), (Some("helper"), Some(b_id.as_str())));
    assert_eq!((hearing["first"].as_bool(), hearing["why"].as_str()), (Some(false), Some("Турбо a не на связи")));
    let voice = now(&view, "voice");
    assert_eq!(
        (voice["by"].as_str(), voice["helper"].as_str(), voice["first"].as_bool(), voice["why"].as_str()),
        (Some("helper"), Some(b_id.as_str()), Some(false), Some("Турбо a не на связи")),
        "{voice}"
    );
    // The model chosen is the second helper's, on the line: it answers, first.
    let model = now(&view, "model");
    assert_eq!(
        (model["by"].as_str(), model["helper"].as_str(), model["first"].as_bool()),
        (Some("helper"), Some(b_id.as_str()), Some(true)),
        "{model}"
    );

    // Both away: our voice has nobody, Alice's speaks — not by choice, and «Где сейчас» says why.
    b.set_away(true);
    let check = || station.post(&format!("/api/v1/helpers/{b_id}/check")).header(header.clone(), auth.clone());
    message(check(), StatusCode::OK).await;
    let view = message(check(), StatusCode::OK).await;
    assert_eq!(station.app.voice.provider(true), TtsProvider::Yandex);
    let voice = now(&view, "voice");
    assert_eq!(
        (voice["by"].as_str(), voice["first"].as_bool(), voice["why"].as_str()),
        (Some("yandex"), Some(false), Some("Турбо a не на связи")),
        "{voice}"
    );
    // No model may be asked: Alice takes the question (stage 4), and the row says why.
    let model = now(&view, "model");
    assert_eq!(
        (model["by"].as_str(), model["why"].as_str()),
        (Some("yandex"), Some("Турбо b не на связи, а запасной модели нет.")),
        "{model}"
    );
    station.stop().await;
}

#[tokio::test]
async fn only_at_home_keeps_the_clouds_and_my_server_out() {
    let (station, (header, auth)) = station(&[
        ("ASR_PROVIDER", "openai"),
        ("ASR_BASE_URL", "https://asr.example.invalid/v1"),
        ("TTS_OWN_URL", "https://tts.example.invalid/v1"),
        ("TTS_PROVIDER", "mixed"),
    ])
    .await;
    // «Любые облака»: the station's cloud hears and speaks.
    assert!(station.app.speech.recognizes());
    assert_eq!(station.app.voice.speaker().unwrap().base_url(), "https://tts.example.invalid/v1");
    assert_eq!(station.app.voice.provider(true), TtsProvider::Mixed);

    let away =
        |to: &str| station.put("/api/v1/helpers/away").header(header.clone(), auth.clone()).json(&json!({"away": to}));
    let view = message(away("home"), StatusCode::OK).await;
    assert_eq!(view["away"], "home");
    assert!(!station.app.speech.recognizes(), "the cloud does not hear: Yandex does");
    assert!(station.app.voice.speaker().is_none(), "the cloud voice is kept in");
    assert_eq!(station.app.voice.provider(true), TtsProvider::Yandex, "Alice's, while the mode lets Yandex");
    let hearing = now(&view, "hearing");
    assert_eq!(
        (hearing["by"].as_str(), hearing["why"].as_str()),
        (Some("yandex"), Some("Регулятор не пускает облако"))
    );
    let voice = now(&view, "voice");
    assert_eq!(
        (voice["by"].as_str(), voice["first"].as_bool(), voice["why"].as_str()),
        (Some("yandex"), Some(false), Some("Регулятор не пускает облако")),
        "Alice's because the cloud voice is kept in, not by choice: {voice}"
    );
    let refused = send(away("nowhere")).await;
    assert_eq!(refused.0, StatusCode::BAD_REQUEST);
    let read = message(station.get("/api/v1/helpers/away").header(header.clone(), auth.clone()), StatusCode::OK).await;
    assert_eq!(read, json!({"away": "home"}));

    // One's own server: kept out under «Только дома», let in under «Дом и моё облако».
    let server = stand(contract("srv", ALL)).await;
    let own = json!({"url": server.url, "key": KEY, "kind": "own"});
    let added =
        message(station.post("/api/v1/helpers").header(header.clone(), auth.clone()).json(&own), StatusCode::CREATED)
            .await;
    let id = added["helpers"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(
        (added["helpers"][0]["kind"].as_str(), added["helpers"][0]["allowed"].as_bool()),
        (Some("own"), Some(false))
    );
    assert_eq!(by(&station, Ability::Hearing), None, "not picked");
    message(away("mine"), StatusCode::OK).await;
    // Let out, it gives after a look of its own, not on the add's (`check` or the tick).
    let check = station.post(&format!("/api/v1/helpers/{id}/check")).header(header.clone(), auth.clone());
    message(check, StatusCode::OK).await;
    assert_eq!(by(&station, Ability::Hearing), Some(id));
    assert_eq!(hears(&station).await, "слышу");
    station.stop().await;
}

#[tokio::test]
async fn the_old_home_server_routes_work_on_top_of_the_list() {
    let computer = stand(contract("pc", ALL)).await;
    let (station, (header, auth)) = station(&[]).await;
    let as_admin = |request: reqwest::RequestBuilder| request.header(header.clone(), auth.clone());
    let old = json!({"url": computer.url, "key": KEY});
    let status = message(as_admin(station.put("/api/v1/home-server")).json(&old), StatusCode::CREATED).await;
    assert_eq!((status["configured"].as_bool(), status["state"].as_str()), (Some(true), Some("up")), "{status}");
    let list = message(as_admin(station.get("/api/v1/helpers")), StatusCode::OK).await;
    assert_eq!(
        (list["helpers"][0]["id"].as_str(), list["helpers"][0]["url"].as_str()),
        (Some("home-computer"), Some(computer.url.as_str()))
    );

    // The computer's «Что даю колонке», with the pair's key, in the old names.
    let pair = bearer(PEER);
    let link =
        |body: Value| station.post("/api/v1/home-server/link").header(pair.0.clone(), pair.1.clone()).json(&body);
    message(link(json!({"without": ["tts"]})), StatusCode::OK).await;
    let list = message(as_admin(station.get("/api/v1/helpers")), StatusCode::OK).await;
    assert_eq!(list["helpers"][0]["without"], json!(["voice"]));

    // The new page's «Не брать у него…»: the old route says the old part, and keeps the new one away from the old.
    let parts = json!({"without": ["voice", "apps"]});
    message(as_admin(station.put("/api/v1/helpers/home-computer")).json(&parts), StatusCode::OK).await;
    let old = message(as_admin(station.get("/api/v1/home-server")), StatusCode::OK).await;
    assert_eq!(old["without"], json!(["tts"]));
    let health = message(station.get("/health"), StatusCode::OK).await;
    assert_eq!(health["placement"]["homeServer"]["without"], json!(["tts"]));
    assert_eq!(health["placement"]["placement"], "stationServer");

    // The switch off: the station works alone.
    message(link(json!({"enabled": false})), StatusCode::OK).await;
    let health = message(station.get("/health"), StatusCode::OK).await;
    assert_eq!(
        (health["placement"]["placement"].as_str(), health["placement"]["homeServer"]["state"].as_str()),
        (Some("station"), Some("none"))
    );

    // Removed by the old route: the list and the old setting both.
    message(as_admin(station.delete("/api/v1/home-server")), StatusCode::OK).await;
    let list = message(as_admin(station.get("/api/v1/helpers")), StatusCode::OK).await;
    assert_eq!(list["helpers"], json!([]));
    assert_eq!(station.app.settings.raw("connector_home_server"), None);
    station.stop().await;
}

/// The pair's door says why the computer is away in the reader's language, as the list says it: the English panel's
/// «Подключить заново» (`POST /api/v1/home-server/link`) and its row of «Компьютер дома» read the same words.
#[tokio::test]
async fn the_old_door_says_why_the_computer_is_away_in_the_readers_language() {
    // An address at home where nothing listens.
    let gone = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", gone.local_addr().unwrap());
    drop(gone);
    let (station, (header, auth)) = station(&[]).await;
    let english =
        |request: reqwest::RequestBuilder| request.header(header.clone(), auth.clone()).header("accept-language", "en");
    let old = json!({"url": url, "key": KEY});
    message(english(station.put("/api/v1/home-server")).json(&old), StatusCode::CREATED).await;
    let checked = message(english(station.post("/api/v1/home-server/check")), StatusCode::CREATED).await;
    assert_eq!((checked["state"].as_str(), checked["error"].as_str()), (Some("down"), Some("Turbo is offline")));
    let pair = bearer(PEER);
    let again = station.post("/api/v1/home-server/link").header(pair.0, pair.1).header("accept-language", "en");
    let linked = message(again.json(&json!({"url": url})), StatusCode::OK).await;
    assert_eq!(linked["error"], "Turbo is offline", "{linked}");
    let list = message(english(station.get("/api/v1/helpers")), StatusCode::OK).await;
    assert_eq!(list["helpers"][0]["error"], linked["error"], "the row of «Компьютер дома» in the same words");
    let health = message(station.get("/health").header("accept-language", "en"), StatusCode::OK).await;
    assert_eq!(health["placement"]["homeServer"]["error"], "Turbo is offline");
    // In Russian, as it always was.
    let russian = message(station.get("/api/v1/home-server").header(header, auth), StatusCode::OK).await;
    assert_eq!(russian["error"], "Турбо не на связи");
    station.stop().await;
}

#[tokio::test]
async fn a_0_11_helper_without_a_contract_reads_as_version_0() {
    let old = stand(status_0_11()).await;
    let (station, (header, auth)) = station(&[]).await;
    let body = json!({"url": old.url, "key": KEY});
    let added =
        message(station.post("/api/v1/helpers").header(header.clone(), auth.clone()).json(&body), StatusCode::CREATED)
            .await;
    let helper = &added["helpers"][0];
    assert_eq!((helper["contract"].as_u64(), helper["version"].as_str()), (Some(0), Some("0.11.1")));
    assert!(helper.get("helper").is_none(), "a computer of 0.11 does not name itself");
    let abilities: Vec<&str> =
        helper["abilities"].as_array().unwrap().iter().filter_map(|ability| ability["id"].as_str()).collect();
    assert_eq!(abilities, ["hearing", "voice", "model", "messages", "apps", "history", "catalog"], "the seven parts");
    // Its bot is known by its contacts: none set up (503) — messages declared, not given.
    let messages = helper["abilities"].as_array().unwrap().iter().find(|ability| ability["id"] == "messages").unwrap();
    assert_eq!(messages["ready"], false);
    let id = helper["id"].as_str().unwrap();
    // A look reads its bot's contacts: none set up — «Где сейчас» says so of this helper by its name.
    let view = message(station.post(&format!("/api/v1/helpers/{id}/check")).header(header, auth), StatusCode::OK).await;
    assert_eq!(by(&station, Ability::Messages), None);
    let row = now(&view, "messages");
    assert_eq!((row["by"].as_str(), row["why"].as_str()), (Some("none"), Some("Турбо: не настроен бот Telegram")));
    assert_eq!(by(&station, Ability::Hearing).as_deref(), Some(id));
    station.stop().await;
}

/// The plan's stage 1 seen from a station: a computer of this build answers with its contract.
#[cfg(feature = "pc")]
#[tokio::test]
async fn a_new_computer_answers_its_contract_to_a_station() {
    let pc = Gateway::start(&[]).await;
    let (name, pc_admin) = admin(&pc).await;
    let key = json!({"name": "Колонка: домашний сервер", "grants": ["home.models"]});
    let created = message(pc.post("/api/v1/api-keys").header(name, pc_admin).json(&key), StatusCode::CREATED).await;
    let (station, (header, auth)) = station(&[]).await;
    let body = json!({"url": pc.base, "key": created["secret"]});
    let added = message(station.post("/api/v1/helpers").header(header, auth).json(&body), StatusCode::CREATED).await;
    let helper = &added["helpers"][0];
    let me = pc.app.identity.gateway().unwrap();
    assert_eq!(helper["contract"].as_u64(), Some(u64::from(duowave::helpers::contract::VERSION)));
    assert_eq!(
        (helper["helper"]["id"].as_str(), helper["helper"]["name"].as_str()),
        (Some(me.id.as_str()), Some(me.name.as_str()))
    );
    assert_eq!(helper["name"].as_str(), Some(me.name.as_str()), "named as it names itself");
    let abilities: Vec<&str> =
        helper["abilities"].as_array().unwrap().iter().filter_map(|ability| ability["id"].as_str()).collect();
    assert!(abilities.contains(&"apps") && abilities.contains(&"messages"), "{abilities:?}");
    assert_eq!(station.app.helpers.entry(helper["id"].as_str().unwrap()).unwrap().helper_id, Some(me.id), "kept");
    station.stop().await;
    pc.stop().await;
}

/// «ПК помогает колонке» from the computer's panel goes through the old door into the station's list.
#[cfg(feature = "pc")]
#[tokio::test]
async fn the_pc_switch_puts_the_computer_into_the_stations_list_through_the_old_door() {
    use duowave::core::availability::Placement;
    use duowave::settings::definitions::{SettingsHome, StationPlacement};

    let pc = Gateway::start(&[]).await;
    let (name, pc_admin) = admin(&pc).await;
    let key = json!({"name": "Колонка: домашний сервер", "grants": ["home.models"]});
    let created =
        message(pc.post("/api/v1/api-keys").header(name.clone(), pc_admin.clone()).json(&key), StatusCode::CREATED)
            .await;
    let (station, _) = station(&[]).await;
    // A move brought the computer's address and key; its help is off.
    station.app.home_server.set(Some(&pc.base), created["secret"].as_str(), None).unwrap();
    station.app.home_server.link(None, None, Some(false), None, None).unwrap();
    assert!(!station.app.helpers.entry("home-computer").unwrap().enabled);

    pc.app.secrets.set("placement:peer", Some(PEER)).unwrap();
    let record = StationPlacement {
        placement: Placement::Station,
        address: "127.0.0.1".into(),
        port: station.port,
        device: None,
        settings_home: SettingsHome::Station,
        auto_update: false,
        managed: true,
        home_key_id: Some(created["key"]["id"].as_str().unwrap().into()),
        moved_at: None,
    };
    let watch = pc.app.station_gateway.watch("at:127.0.0.1");
    watch.set_record(Some(&record), None).unwrap();
    watch.poll().await;
    // The computer took the home's copy (docs/plans/home-and-devices.md, stage 3): its people are the home's now —
    // the station's admin signs in here with the home's password; its own access keys stayed.
    let _ = pc_admin;
    let login = pc.post("/api/v1/auth/login").json(&json!({"password": "helpers-admin-password"}));
    let (name, pc_admin) = bearer(message(login, StatusCode::OK).await["token"].as_str().expect("a token"));
    let switch = pc.post("/api/v1/placement/helper").header(name, pc_admin).json(&json!({"on": true}));
    message(switch, StatusCode::CREATED).await;
    // The computer's address as it tells the station (its home network's, when it has one; this one else).
    let told = watch.home_url().unwrap_or_else(|| pc.base.clone());
    let door = station.app.helpers.entry("home-computer").unwrap();
    assert!(door.enabled && door.url == told, "{door:?}");
    let health = message(station.get("/health"), StatusCode::OK).await;
    assert_eq!(health["placement"]["placement"], "stationServer");
    assert_eq!(watch.record().unwrap().placement, Placement::StationServer);
    station.stop().await;
    pc.stop().await;
}

#[tokio::test]
async fn the_panel_reads_its_helpers_where_now_and_the_regulator_in_one_answer() {
    let computer = stand(contract("pc", ALL)).await;
    let (station, (header, auth)) = station(&[]).await;
    let body = json!({"url": computer.url, "key": KEY, "name": "Кабинет"});
    message(station.post("/api/v1/helpers").header(header.clone(), auth.clone()).json(&body), StatusCode::CREATED)
        .await;
    let view = message(station.get("/api/v1/helpers").header(header.clone(), auth.clone()), StatusCode::OK).await;
    let helper = &view["helpers"][0];
    assert_eq!(
        (helper["name"].as_str(), helper["kind"].as_str(), helper["state"].as_str()),
        (Some("Кабинет"), Some("home"), Some("up"))
    );
    assert!(helper["since"].is_string() && helper["key"]["set"] == true && helper["allowed"] == true, "{helper}");
    assert_eq!(helper["abilities"][0], json!({"id": "hearing", "ready": true, "details": {}}));
    let rows: Vec<&str> = view["now"].as_array().unwrap().iter().filter_map(|row| row["ability"].as_str()).collect();
    assert_eq!(rows, ["hearing", "model", "voice", "messages", "apps", "history"]);
    let hearing = now(&view, "hearing");
    assert_eq!(
        (hearing["by"].as_str(), hearing["name"].as_str(), hearing["first"].as_bool()),
        (Some("helper"), Some("Кабинет"), Some(true))
    );
    assert_eq!(hearing["reach"], "home");
    assert_eq!(view["away"], "any");
    let health = message(station.get("/health"), StatusCode::OK).await;
    assert_eq!(view["gateway"]["id"], health["gateway"]["id"]);
    // Seeing needs the right; an unknown helper is none.
    assert_eq!(send(station.get("/api/v1/helpers")).await.0, StatusCode::UNAUTHORIZED);
    let missing = station.post("/api/v1/helpers/nobody/check").header(header.clone(), auth.clone());
    assert_eq!(send(missing).await.0, StatusCode::NOT_FOUND);
    let wrong = station.put("/api/v1/helpers/order").header(header, auth).json(&json!({"ids": ["nobody"]}));
    assert_eq!(send(wrong).await.0, StatusCode::BAD_REQUEST);
    station.stop().await;
}

#[tokio::test]
async fn a_dry_run_shows_what_the_helper_gives_and_stores_nothing() {
    let computer = stand(contract("pc", ALL)).await;
    let (station, (header, auth)) = station(&[]).await;
    let body = json!({"url": computer.url, "key": KEY, "dryRun": true});
    let checked =
        message(station.post("/api/v1/helpers").header(header.clone(), auth.clone()).json(&body), StatusCode::OK).await;
    let helper = &checked["helper"];
    assert_eq!(
        (helper["state"].as_str(), helper["contract"].as_u64(), helper["name"].as_str()),
        (Some("up"), Some(1), Some("Турбо pc"))
    );
    assert_eq!(helper["abilities"].as_array().unwrap().len(), 7);
    let view = message(station.get("/api/v1/helpers").header(header.clone(), auth.clone()), StatusCode::OK).await;
    assert_eq!(view["helpers"], json!([]), "nothing stored");
    assert!(station.app.secrets.ids("helper:").is_empty(), "nor its key");
    // A wrong key is said in the helper's words, before anything is stored.
    let wrong = json!({"url": computer.url, "key": "vgw_wrong", "dryRun": true});
    let (status, body) = send(station.post("/api/v1/helpers").header(header, auth).json(&wrong)).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert!(body["message"].as_str().unwrap_or_default().contains("не узнал ключ"), "{body}");
    station.stop().await;
}

#[tokio::test]
async fn the_order_the_name_and_the_regulator_are_stored_and_survive_a_restart() {
    let (a, b) = (stand(contract("a", ALL)).await, stand(contract("b", ALL)).await);
    let (station, (header, auth)) = station(&[]).await;
    let mut ids = Vec::new();
    for url in [&a.url, &b.url] {
        let body = json!({"url": url, "key": KEY});
        let view = message(
            station.post("/api/v1/helpers").header(header.clone(), auth.clone()).json(&body),
            StatusCode::CREATED,
        )
        .await;
        ids.push(view["helpers"].as_array().unwrap().last().unwrap()["id"].as_str().unwrap().to_string());
    }
    let order = json!({"ids": [ids[1], ids[0]]});
    message(station.put("/api/v1/helpers/order").header(header.clone(), auth.clone()).json(&order), StatusCode::OK)
        .await;
    let change = json!({"name": "Кабинет", "without": ["history"]});
    let put = station.put(&format!("/api/v1/helpers/{}", ids[0])).header(header.clone(), auth.clone()).json(&change);
    message(put, StatusCode::OK).await;
    message(
        station.put("/api/v1/helpers/away").header(header.clone(), auth.clone()).json(&json!({"away": "mine"})),
        StatusCode::OK,
    )
    .await;

    let station = station.restart().await;
    let view = message(station.get("/api/v1/helpers").header(header, auth), StatusCode::OK).await;
    let listed: Vec<&str> =
        view["helpers"].as_array().unwrap().iter().filter_map(|helper| helper["id"].as_str()).collect();
    assert_eq!(listed, [ids[1].as_str(), ids[0].as_str()]);
    assert_eq!(
        (view["helpers"][1]["name"].as_str(), &view["helpers"][1]["without"]),
        (Some("Кабинет"), &json!(["history"]))
    );
    assert_eq!(view["away"], "mine");
    assert!(station.app.helpers.key_mask(&ids[0]).set, "the key kept too");
    station.stop().await;
}

#[tokio::test]
async fn the_health_carries_the_helpers_and_still_the_home_server_for_older_panels() {
    let computer = stand(contract("pc", ALL)).await;
    let (station, (header, auth)) = station(&[]).await;
    let old = json!({"url": computer.url, "key": KEY});
    message(station.put("/api/v1/home-server").header(header, auth).json(&old), StatusCode::CREATED).await;
    let health = message(station.get("/health"), StatusCode::OK).await;
    let helpers = &health["placement"]["helpers"];
    assert_eq!((helpers["away"].as_str(), helpers["list"][0]["id"].as_str()), (Some("any"), Some("home-computer")));
    assert_eq!(helpers["list"][0]["abilities"].as_array().unwrap().len(), 7);
    assert!(helpers["list"][0].get("url").is_none(), "no address in the brief");
    assert_eq!(health["placement"]["homeServer"]["url"].as_str(), Some(computer.url.as_str()));
    assert!(!health.to_string().contains(KEY), "never the key");
    // A computer says its helpers too: itself first, its built-in helper (stage 6), and none more yet.
    let pc = Gateway::start(&[]).await;
    let health = message(pc.get("/health"), StatusCode::OK).await;
    let helpers = &health["placement"]["helpers"];
    let ids: Vec<&str> = helpers["list"].as_array().unwrap().iter().filter_map(|entry| entry["id"].as_str()).collect();
    assert_eq!(
        (helpers["away"].as_str(), ids),
        (Some("any"), if cfg!(feature = "pc") { vec!["self"] } else { vec![] })
    );
    if cfg!(feature = "pc") {
        assert_eq!((&helpers["list"][0]["builtin"], &helpers["list"][0]["state"]), (&json!(true), &json!("up")));
    }
    assert!(health["placement"].get("homeServer").is_none());
    pc.stop().await;
    station.stop().await;
}
