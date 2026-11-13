//! The bus end to end (docs/plans/bus.md): apps in QuickJS on a real gateway. «Покупки» asked for `call:app/notes`
//! in its manifest: its first letter to «Заметки» waits for a person's «да», like a phone; after the «да» it goes
//! through, in the chain of the person who pressed the button, and the journal tells who wrote to whom — never what.
//! An app that asked for nothing does not even see «Заметки». An app's own folder is its own, no link needed.

mod fakes;
mod support;

use std::path::Path;

use fakes::ChatStub;
use reqwest::StatusCode;
use serde_json::{json, Value};
use support::{bearer, message, send, Gateway};

const RUNS: [&str; 3] = ["pc", "station", "stationServer"];

/// A QuickJS app in the gateway's DATA_DIR, as an install leaves it.
fn install(data: &Path, manifest: Value, code: &str) {
    let id = manifest["id"].as_str().unwrap();
    let version = manifest["version"].as_str().unwrap();
    let dir = data.join("packages").join(id).join(version);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("manifest.json"), manifest.to_string()).unwrap();
    std::fs::write(dir.join("backend.js"), code).unwrap();
}

fn apps(data: &Path) {
    install(
        data,
        json!({
            "schema": 1, "id": "notes", "version": "1.0.0", "title": "Заметки", "api": "^2", "runs": RUNS,
            "permissions": [],
            "contributes": {
                "tools": [{
                    "name": "notes_add", "description": "Добавляет заметку", "handler": "add",
                    "input": { "type": "object", "properties": { "text": { "type": "string" } } }
                }, {
                    "name": "notes_list", "description": "Что в заметках", "handler": "all",
                    "input": { "type": "object", "properties": {} }
                }],
                "panel": { "calls": ["all"] }
            }
        }),
        r#"
        export function add({ text }) {
            const all = host.kv.get("all") ?? [];
            all.push(text);
            host.kv.set("all", all);
            return { count: all.length };
        }
        export function all() { return host.kv.get("all") ?? []; }
        "#,
    );
    install(
        data,
        json!({
            "schema": 1, "id": "shop", "version": "1.0.0", "title": "Покупки", "api": "^2", "runs": RUNS,
            "permissions": ["call:app/notes"],
            "contributes": { "panel": { "calls": ["buy", "keep", "question"] } }
        }),
        r#"
        export async function buy({ text }) {
            return await host.bus.call("app/notes", "notes_add", { text });
        }
        export async function question({ text }) {
            return await host.bus.ask("app/notes", text);
        }
        export async function keep({ text }) {
            await host.files.write("tmp/list.txt", text);
            const read = await host.files.read("tmp/list.txt", { as: "text" });
            const listed = await host.files.list("tmp");
            return { text: read.text, names: listed.items.map((item) => item.name) };
        }
        "#,
    );
    install(
        data,
        json!({
            "schema": 1, "id": "spy", "version": "1.0.0", "title": "Шпион", "api": "^2", "runs": RUNS,
            "permissions": [],
            "contributes": { "panel": { "calls": ["peek"] } }
        }),
        r#"
        export async function peek() {
            const seen = (await host.bus.list()).map((one) => one.address);
            let tried = "";
            try { await host.bus.call("app/notes", "notes_add", { text: "чужое" }); } catch (error) { tried = String(error.message); }
            let theirs = "";
            try { await host.files.read("files/app/shop/tmp/list.txt"); } catch (error) { theirs = String(error.message); }
            return { seen, tried, theirs };
        }
        "#,
    );
}

struct Env {
    gateway: Gateway,
    admin: String,
}

impl Env {
    async fn start() -> Env {
        Env::with(&[]).await
    }

    async fn with(vars: &[(&str, &str)]) -> Env {
        let gateway = Gateway::start(vars).await;
        apps(gateway.dir.path());
        let setup = message(
            gateway.post("/api/v1/auth/setup").json(&json!({"password": "bus-admin-password"})),
            StatusCode::CREATED,
        );
        let admin = setup.await["token"].as_str().unwrap().to_string();
        let env = Env { gateway, admin };
        for id in ["notes", "shop", "spy"] {
            let (code, answer) = env.send("POST", &format!("/api/v1/packages/{id}/enable"), json!({})).await;
            assert_eq!(code, StatusCode::OK, "{id}: {answer}");
        }
        env
    }

    async fn send(&self, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
        let (name, value) = bearer(&self.admin);
        let method = reqwest::Method::from_bytes(method.as_bytes()).unwrap();
        send(self.gateway.request(method, path).header(name, value).json(&body)).await
    }

    async fn call(&self, app: &str, function: &str, input: Value) -> (StatusCode, Value) {
        self.send("POST", &format!("/api/v1/packages/{app}/call/{function}"), input).await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_asks_once_like_a_phone_and_after_the_yes_reaches_the_other_in_the_persons_chain() {
    let env = Env::start().await;

    // The first letter waits for a «да»: the panel hears a question with words for people.
    let (code, refused) = env.call("shop", "buy", json!({"text": "молоко"})).await;
    assert_eq!(code, StatusCode::BAD_REQUEST, "{refused}");
    assert!(refused.to_string().contains("ждёт вашего «да»"), "{refused}");
    let (_, waiting) = env.send("GET", "/api/v1/bus/questions", json!({})).await;
    let questions = waiting["message"]["questions"].as_array().cloned().unwrap_or_default();
    assert_eq!(questions.len(), 1, "{waiting}");
    let question = &questions[0];
    assert_eq!((question["from"].as_str(), question["to"].as_str()), (Some("app/shop"), Some("app/notes")));
    assert_eq!(question["what"], "звать «Заметки»");
    let id = question["id"].as_str().unwrap();
    let (code, answered) = env.send("POST", &format!("/api/v1/bus/questions/{id}"), json!({"yes": true})).await;
    assert_eq!(code, StatusCode::OK, "{answered}");

    // After the «да»: a link of the answer's kind, and the letter goes through.
    let (_, links) = env.send("GET", "/api/v1/bus/links", json!({})).await;
    let link = links["message"]["links"].as_array().unwrap().iter().find(|link| link["from"] == "app/shop").cloned();
    let link = link.unwrap_or_else(|| panic!("{links}"));
    assert_eq!(
        (link["to"].as_str(), link["level"].as_str(), link["source"].as_str()),
        (Some("app/notes"), Some("call"), Some("answer"))
    );
    let (code, bought) = env.call("shop", "buy", json!({"text": "молоко"})).await;
    assert_eq!(code, StatusCode::OK, "{bought}");
    assert_eq!(bought["message"]["result"], json!({"count": 1}));
    let (_, all) = env.call("notes", "all", json!({})).await;
    assert_eq!(all["message"]["result"], json!(["молоко"]));

    // An app that asked for nothing: «Заметки» is not there for it, nor is another app's folder.
    let (code, peeked) = env.call("spy", "peek", json!({})).await;
    assert_eq!(code, StatusCode::OK, "{peeked}");
    let peeked = &peeked["message"]["result"];
    assert!(!peeked["seen"].as_array().unwrap().iter().any(|seen| seen == "app/notes"), "{peeked}");
    assert!(peeked["tried"].as_str().unwrap().contains("нет такого"), "{peeked}");
    assert!(peeked["theirs"].as_str().unwrap().contains("нет такого"), "{peeked}");

    // Its own folder needs no link: write, read, list.
    let (code, kept) = env.call("shop", "keep", json!({"text": "хлеб"})).await;
    assert_eq!(code, StatusCode::OK, "{kept}");
    assert_eq!(kept["message"]["result"], json!({"text": "хлеб", "names": ["list.txt"]}));

    // The journal: who wrote to whom and how it went — never what.
    let (code, journal) = env.send("GET", "/api/v1/bus/journal?about=app/notes", json!({})).await;
    assert_eq!(code, StatusCode::OK, "{journal}");
    let lines = journal["message"]["lines"].as_array().unwrap();
    assert!(
        lines.iter().any(|line| line["from"] == "app/shop" && line["to"] == "app/notes" && line["outcome"] == "ok"),
        "{journal}"
    );
    assert!(lines.iter().any(|line| line["outcome"] == "asked" || line["kind"] == "answer"), "{journal}");
    assert!(!journal.to_string().contains("молоко"), "the journal keeps no content: {journal}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_no_is_remembered_and_the_owner_opens_the_door_in_links() {
    let env = Env::start().await;
    let (code, _) = env.call("shop", "buy", json!({"text": "сыр"})).await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    let (_, waiting) = env.send("GET", "/api/v1/bus/questions", json!({})).await;
    let id = waiting["message"]["questions"][0]["id"].as_str().unwrap().to_string();
    let (code, _) = env.send("POST", &format!("/api/v1/bus/questions/{id}"), json!({"yes": false})).await;
    assert_eq!(code, StatusCode::OK);

    // «Нет» stands: refused, and no new question.
    let (code, refused) = env.call("shop", "buy", json!({"text": "сыр"})).await;
    assert_eq!(code, StatusCode::BAD_REQUEST, "{refused}");
    assert!(refused.to_string().contains("нет связи"), "{refused}");
    let (_, waiting) = env.send("GET", "/api/v1/bus/questions", json!({})).await;
    assert_eq!(waiting["message"]["questions"], json!([]));

    // The owner changes their mind in «Связи».
    let (code, put) =
        env.send("PUT", "/api/v1/bus/links", json!({"from": "app/shop", "to": "app/notes", "level": "call"})).await;
    assert_eq!(code, StatusCode::OK, "{put}");
    let (code, bought) = env.call("shop", "buy", json!({"text": "сыр"})).await;
    assert_eq!(code, StatusCode::OK, "{bought}");

    // «Отдельно» for «Заметки»: nobody reaches it, the person in the panel still does.
    let (code, _) = env.send("PUT", "/api/v1/bus/modes", json!({"address": "app/notes", "mode": "isolated"})).await;
    assert_eq!(code, StatusCode::OK);
    let (code, cut) = env.call("shop", "buy", json!({"text": "сыр"})).await;
    assert_eq!(code, StatusCode::BAD_REQUEST, "{cut}");
    let (code, mine) = env.call("notes", "all", json!({})).await;
    assert_eq!((code, &mine["message"]["result"]), (StatusCode::OK, &json!(["сыр"])));

    // The overview the «Связи» screen draws.
    let (code, overview) = env.send("GET", "/api/v1/bus", json!({})).await;
    assert_eq!(code, StatusCode::OK, "{overview}");
    let participants = overview["message"]["participants"].as_array().unwrap();
    for address in ["app/notes", "app/shop", "system/notify", "files/home/Документы"] {
        assert!(participants.iter().any(|one| one["address"] == address), "{address}: {overview}");
    }
    assert_eq!(overview["message"]["modes"]["own"]["app/notes"], "isolated");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_without_its_own_ask_answers_in_words_through_the_model_with_its_tools_in_the_askers_chain() {
    let llm = ChatStub::start().await;
    let env = Env::with(&[("DIALOG_PROVIDER", "openai"), ("LLM_BASE_URL", &llm.url), ("LLM_MODEL", "test-llm")]).await;
    let (code, _) =
        env.send("PUT", "/api/v1/bus/links", json!({"from": "app/shop", "to": "app/notes", "level": "call"})).await;
    assert_eq!(code, StatusCode::OK);
    let (code, _) = env.call("shop", "buy", json!({"text": "молоко"})).await;
    assert_eq!(code, StatusCode::OK);

    // The model calls «Заметки»' own tool, then says the answer; the call is the asker's letter.
    llm.set_tool_call(Some(("notes_list", json!({}))));
    llm.set_reply("В заметках: молоко.");
    let (code, answered) = env.call("shop", "question", json!({"text": "Что в заметках?"})).await;
    assert_eq!(code, StatusCode::OK, "{answered}");
    assert_eq!(answered["message"]["result"]["text"], "В заметках: молоко.");
    let requests = llm.requests();
    let offered: Vec<&str> =
        requests[0]["tools"].as_array().unwrap().iter().filter_map(|tool| tool["function"]["name"].as_str()).collect();
    assert_eq!(offered, ["notes_add", "notes_list"], "only the asked app's tools");
    assert!(requests.last().unwrap().to_string().contains("молоко"), "the tool's result went back to the model");
    let (_, journal) = env.send("GET", "/api/v1/bus/journal?about=app/notes", json!({})).await;
    let lines = journal["message"]["lines"].as_array().unwrap();
    assert!(lines.iter().any(|line| line["handle"] == "notes_list" && line["from"] == "app/shop"), "{journal}");
    assert!(lines.iter().any(|line| line["handle"] == "ask" && line["outcome"] == "ok"), "{journal}");
}
