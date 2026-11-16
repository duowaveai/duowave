//! The model never silent end to end (docs/plans/home-and-helpers.md, block 1, stage 4): «помощник не на связи —
//! отвечает «Запасная» без ожидания соединения; без «Запасной» слова приходят меньше чем за секунду». The station is a
//! computer's build playing it (DUOWAVE_ROLE=station); its helper and the backup's service are stand-ins on 127.0.0.1.
//! The tests assert the cause — the helper's model route not hit once it is away — with a bound that only guards
//! against the old ten-second connect: one test binary runs many gateways at once (the second's own bound lives in the
//! unit tests, without I/O).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use duowave::router::deals::DealsQuery;
use reqwest::header::HeaderName;
use reqwest::StatusCode;
use serde_json::{json, Value};

use crate::stand::{contract, stand, Stand, ALL, KEY};
use crate::support::{bearer, message, until, Gateway};

const PEER: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
/// What only the old connection's wait would pass.
const AT_ONCE: Duration = Duration::from_secs(5);

/// A station's gateway and its first admin's header.
async fn station() -> (Gateway, (HeaderName, String)) {
    let station = Gateway::start(&[("DUOWAVE_ROLE", "station"), ("PEER_KEY", PEER)]).await;
    let setup = station.post("/api/v1/auth/setup").json(&json!({"password": "helpers-admin-password"}));
    let auth = bearer(message(setup, StatusCode::CREATED).await["token"].as_str().expect("a token"));
    (station, auth)
}

/// An OpenAI-compatible service at home for the backup: its answer, and the questions it got.
async fn spare() -> (String, Arc<AtomicUsize>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let counted = hits.clone();
    let router = axum::Router::new().route(
        "/v1/chat/completions",
        axum::routing::post(move || {
            let hits = counted.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                let chunk = json!({ "choices": [{ "delta": { "content": "Ответ запасной." } }] });
                (
                    [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                    format!("data: {chunk}\n\ndata: [DONE]\n\n"),
                )
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (url, hits)
}

/// The computer in the station's list, and the id of its model's row in «Мозг», made with its first look.
async fn computer_in_the_list(station: &Gateway, auth: &(HeaderName, String), computer: &Stand) -> (String, String) {
    let body = json!({"url": computer.url, "key": KEY, "name": "Компьютер дома"});
    let added = message(
        station.post("/api/v1/helpers").header(auth.0.clone(), auth.1.clone()).json(&body),
        StatusCode::CREATED,
    )
    .await;
    let id = added["helpers"][0]["id"].as_str().unwrap().to_string();
    let row = || {
        let view = station.app.brain.view();
        view.providers
            .into_iter()
            .find(|row| row.helper.as_ref().is_some_and(|helper| helper.id == id))
            .map(|row| row.id)
    };
    assert!(until(5000, || row().is_some()).await, "its model's row comes with its first look");
    let row = row().unwrap();
    (id, row)
}

/// The computer away: two looks that miss.
async fn away(station: &Gateway, auth: &(HeaderName, String), computer: &Stand, id: &str) {
    computer.set_away(true);
    let check = || station.post(&format!("/api/v1/helpers/{id}/check")).header(auth.0.clone(), auth.1.clone());
    message(check(), StatusCode::OK).await;
    let view = message(check(), StatusCode::OK).await;
    assert_eq!(view["helpers"][0]["state"], "down", "{view}");
}

/// «Спросить мозг»: the events of its answer, as the panel reads them, and how long the whole answer took.
async fn ask(station: &Gateway, auth: &(HeaderName, String), question: &str) -> (Vec<Value>, Duration) {
    let started = Instant::now();
    let response = station
        .post("/api/v1/brain/ask")
        .header(auth.0.clone(), auth.1.clone())
        .json(&json!({"question": question}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.unwrap();
    let events =
        body.lines().filter_map(|line| line.strip_prefix("data: ")).filter_map(|data| serde_json::from_str(data).ok());
    (events.collect(), started.elapsed())
}

fn of_kind<'a>(events: &'a [Value], kind: &str) -> Vec<&'a Value> {
    events.iter().filter(|event| event["type"] == kind).collect()
}

#[tokio::test]
async fn a_helper_away_the_backup_answers_at_once() {
    let computer = stand(contract("pc", ALL)).await;
    let (spare_url, spare_hits) = spare().await;
    let (station, auth) = station().await;
    let (id, row) = computer_in_the_list(&station, &auth, &computer).await;
    let provider = json!({"preset": "custom-openai", "title": "Запас", "baseUrl": spare_url});
    let spare = message(
        station.post("/api/v1/brain/providers").header(auth.0.clone(), auth.1.clone()).json(&provider),
        StatusCode::CREATED,
    )
    .await;
    let choice = json!({
        "fast": {"provider": row, "model": "home"},
        "backup": {"provider": spare["id"], "model": "spare-model"},
    });
    message(station.put("/api/v1/brain/choice").header(auth.0.clone(), auth.1.clone()).json(&choice), StatusCode::OK)
        .await;

    // On the line: the computer's model answers.
    let (events, _) = ask(&station, &auth, "Который час?").await;
    assert_eq!(of_kind(&events, "text")[0]["text"], "Ответ Турбо.", "{events:?}");
    assert_eq!(computer.llm_hits.load(Ordering::SeqCst), 1);

    // Away: the backup at once — no request to the computer, no connection waited for.
    away(&station, &auth, &computer, &id).await;
    let (events, took) = ask(&station, &auth, "А погода?").await;
    let start = of_kind(&events, "start")[0];
    assert_eq!((start["model"].as_str(), start["provider"].as_str()), (Some("spare-model"), Some("Запас")), "{start}");
    assert_eq!(of_kind(&events, "text")[0]["text"], "Ответ запасной.", "{events:?}");
    assert!(of_kind(&events, "error").is_empty() && of_kind(&events, "done").len() == 1, "{events:?}");
    assert_eq!(computer.llm_hits.load(Ordering::SeqCst), 1, "the computer away is not asked");
    assert_eq!(spare_hits.load(Ordering::SeqCst), 1);
    assert!(took < AT_ONCE, "{took:?}");

    // «Где сейчас» says who answers and why not the first.
    let view = message(station.get("/api/v1/helpers").header(auth.0.clone(), auth.1.clone()), StatusCode::OK).await;
    let model = view["now"].as_array().unwrap().iter().find(|row| row["ability"] == "model").unwrap();
    assert_eq!(
        (model["by"].as_str(), model["name"].as_str(), model["first"].as_bool(), model["why"].as_str()),
        (Some("here"), Some("Запас"), Some(false), Some("Компьютер дома не на связи")),
        "{model}"
    );

    // The router's journal (src/router): each answer is a deal of the model that gave it — the computer's row at home,
    // free, then the backup that stood in for it at once; newest first.
    let deals = || station.app.router.deals.list(&DealsQuery::default()).unwrap();
    assert!(until(3000, || deals().len() == 2).await, "{:?}", deals());
    let deals: Vec<[String; 4]> =
        deals().into_iter().map(|deal| [deal.provider, deal.model, deal.place, deal.billing]).collect();
    let at_home = |provider: &str, model: &str| [provider, model, "home", "local"].map(String::from);
    assert_eq!(deals, [at_home(spare["id"].as_str().unwrap(), "spare-model"), at_home(&row, "home")]);
    station.stop().await;
}

#[tokio::test]
async fn a_helper_away_without_a_backup_the_words_come_at_once() {
    let computer = stand(contract("pc", ALL)).await;
    let (station, auth) = station().await;
    let (id, row) = computer_in_the_list(&station, &auth, &computer).await;
    let choice = json!({"fast": {"provider": row, "model": "home"}});
    let brain = message(
        station.put("/api/v1/brain/choice").header(auth.0.clone(), auth.1.clone()).json(&choice),
        StatusCode::OK,
    )
    .await;
    // Nothing the gateway could take as the backup by itself: «Мозг» asks for one.
    assert_eq!(brain["note"]["kind"], "backupNeeded", "{}", brain["note"]);

    away(&station, &auth, &computer, &id).await;
    let (events, took) = ask(&station, &auth, "Который час?").await;
    let errors = of_kind(&events, "error");
    assert_eq!(errors.len(), 1, "{events:?}");
    assert_eq!(errors[0]["message"], "Компьютер дома не на связи, а запасной модели нет.");
    assert!(of_kind(&events, "text").is_empty());
    assert_eq!(computer.llm_hits.load(Ordering::SeqCst), 0, "the computer away is not asked");
    assert!(took < AT_ONCE, "{took:?}");
    // No model answered: the words leave no deal in the router's journal.
    let deals = station.app.router.deals.list(&DealsQuery::default()).unwrap();
    assert!(deals.is_empty(), "{deals:?}");
    station.stop().await;
}
