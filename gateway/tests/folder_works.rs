//! The works in folders end to end through the API (docs/plans/router-and-works.md, stage 3), with the stub
//! `claude` (`duowave-test-stub`, src/bin/test_stub.rs): the agent found and signed in, a folder added (one without git refused),
//! «Новая работа» — its «Готово» with the branch, the checks and the dollars, its deal on the seat «Исполнитель», its
//! runs, its journal read from a place and its raw file; one work in folders at a time; a plan's limit asked and
//! answered from the page; a stop and «Продолжить» in the same session. Stage 5: a phrase said to the panel begins a
//! work in a folder — read first, or by the brain's `work_start` — and its «Готово» reads what the agent wrote. Each
//! test starts its own gateway.

mod fakes;
mod support;

use std::path::{Path, PathBuf};
use std::time::Duration;

use fakes::{ChatStub, Scripted};
use reqwest::StatusCode;
use serde_json::{json, Value};
use support::{bearer, message, send, Gateway};

/// The stub's name is its role (src/bin/test_stub.rs).
const CLAUDE: &str = if cfg!(windows) { "claude.exe" } else { "claude" };
/// The folder's own commands, as the household writes them for its system: sh, and cmd on Windows.
const PREPARE: &str = if cfg!(windows) { "exit /b 0" } else { "true" };
const CHECK: &str =
    if cfg!(windows) { "if exist contacts.txt (exit /b 0) else (exit /b 1)" } else { "test -f contacts.txt" };

trait Signed {
    fn signed(self, token: &str) -> Self;
}

impl Signed for reqwest::RequestBuilder {
    fn signed(self, token: &str) -> Self {
        let (name, value) = bearer(token);
        self.header(name, value)
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().expect("git runs");
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// A gateway with its admin, the stub `claude` set for Claude Code, and a folder «Сайт» with git.
struct Bench {
    gateway: Gateway,
    admin: String,
    stub: PathBuf,
    folder: PathBuf,
    _outside: tempfile::TempDir,
}

impl Bench {
    async fn new() -> Bench {
        Bench::with(&[]).await
    }

    async fn with(vars: &[(&str, &str)]) -> Bench {
        let gateway = Gateway::start(vars).await;
        let setup = message(
            gateway.post("/api/v1/auth/setup").json(&json!({ "password": "admin-password-1" })),
            StatusCode::CREATED,
        );
        let admin = setup.await["token"].as_str().expect("a token").to_string();
        let outside = tempfile::tempdir().unwrap();
        let stub = outside.path().join("stub");
        std::fs::create_dir_all(&stub).unwrap();
        std::fs::copy(env!("CARGO_BIN_EXE_duowave-test-stub"), stub.join(CLAUDE)).unwrap();
        let folder = outside.path().join("site");
        std::fs::create_dir_all(&folder).unwrap();
        git(&folder, &["init", "--quiet", "-b", "main"]);
        git(&folder, &["config", "user.name", "Owner"]);
        git(&folder, &["config", "user.email", "owner@example.com"]);
        std::fs::write(folder.join("app.txt"), "app\n").unwrap();
        git(&folder, &["add", "-A"]);
        git(&folder, &["commit", "--quiet", "-m", "first"]);
        Bench { gateway, admin, stub, folder, _outside: outside }
    }

    fn mode(&self, mode: &str) {
        std::fs::write(self.stub.join("mode"), mode).unwrap();
    }

    async fn get(&self, path: &str) -> Value {
        message(self.gateway.get(path).signed(&self.admin), StatusCode::OK).await
    }

    async fn post(&self, path: &str, body: Value) -> Value {
        message(self.gateway.post(path).signed(&self.admin).json(&body), StatusCode::OK).await
    }

    /// The talk's work once `check` holds for it.
    async fn work_until(&self, id: &str, what: &str, check: impl Fn(&Value) -> bool) -> Value {
        let mut last = Value::Null;
        for _ in 0..400 {
            last = self.get(&format!("/api/v1/works/{id}")).await;
            if check(&last) {
                return last;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the work {id} never: {what}; it is {last}");
    }

    async fn agent_and_folder(&self) {
        let claude = message(
            self.gateway
                .put("/api/v1/harnesses/claude-code")
                .signed(&self.admin)
                .json(&json!({ "path": self.stub.join(CLAUDE).display().to_string() })),
            StatusCode::OK,
        )
        .await;
        assert_eq!(claude["ready"]["version"], "2.1.302");
        assert_eq!(claude["ready"]["signedIn"], true);
        assert_eq!(claude["ready"]["pays"], "plan");
        assert!(claude["ready"]["not"].is_null(), "{claude}");
        let folder = self
            .post(
                "/api/v1/work-folders",
                json!({
                    "name": "Сайт",
                    "path": self.folder.display().to_string(),
                    "prepare": [PREPARE],
                    "checks": [CHECK],
                }),
            )
            .await;
        assert_eq!(
            (folder["id"].as_str(), folder["branch"].as_str(), folder["git"].as_bool()),
            (Some("sayt"), Some("main"), Some(true))
        );
    }

    async fn start(&self, task: &str) -> (String, String) {
        let begun = self.post("/api/v1/work-folders/sayt/start", json!({ "task": task })).await;
        let id = begun["work"]["id"].as_str().expect("a work").to_string();
        assert_eq!(begun["work"]["kind"], "folder");
        (id, begun["said"].as_str().unwrap_or_default().to_string())
    }
}

/// The whole way: an agent ready, a folder, a work, its «Готово», its deal, its runs and journal.
#[tokio::test]
async fn a_work_in_a_folder_goes_from_the_panel_to_its_branch_deal_and_journal() {
    let bench = Bench::new().await;
    let (status, body) = send(bench.gateway.get("/api/v1/work-folders")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    bench.agent_and_folder().await;
    // A folder without git is refused with words why.
    let plain = bench._outside.path().join("notes");
    std::fs::create_dir_all(&plain).unwrap();
    let (status, body) = send(
        bench
            .gateway
            .post("/api/v1/work-folders")
            .signed(&bench.admin)
            .json(&json!({ "path": plain.display().to_string() })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.to_string().contains("git"), "{body}");

    let (id, said) = bench.start("сделай страницу контактов с формой").await;
    assert_eq!(said, "Взяла в работу в папке «Сайт»: «Сделай страницу контактов с формой». Скажу, когда будет готово.");
    let work = bench.work_until(&id, "done", |work| work["state"] == "done").await;
    let line = work["result"]["line"].as_str().unwrap_or_default();
    assert_eq!(
        line,
        "Сделай страницу контактов с формой: готово. Ветка duowave/sdelay-stranitsu-kontaktov, проверки прошли, ≈ $1.20 по подписке. Рассказать, что написал агент?"
    );
    assert_eq!(git(&bench.folder, &["symbolic-ref", "--short", "HEAD"]), "main");
    git(&bench.folder, &["rev-parse", "--verify", "refs/heads/duowave/sdelay-stranitsu-kontaktov"]);

    // Its deal: «Исполнитель», Claude Code's way, its own dollars, by the plan.
    let deals = bench.get(&format!("/api/v1/works/{id}/deals")).await;
    let deal = &deals["deals"][0];
    assert_eq!(deal["seat"], "worker");
    assert_eq!(deal["via"], "harness:claude-code");
    assert_eq!(deal["provider"], "claude-code");
    assert_eq!(deal["model"], "claude-sonnet-5-5");
    assert_eq!(deal["usdMicro"], 1_200_000);
    assert_eq!(deal["billing"], "plan");
    assert_eq!(deal["price"]["from"], "harness");
    let listed = bench.get("/api/v1/works?group=recent").await;
    let item = listed.as_array().unwrap().iter().find(|item| item["id"] == id.as_str()).cloned().unwrap();
    assert_eq!(item["spent"]["usdMicro"], 1_200_000);
    assert_eq!(item["model"]["seat"], "worker");

    // Its runs: one, with the session and the command to open it in a terminal.
    let runs = bench.get(&format!("/api/v1/works/{id}/runs")).await;
    assert_eq!(runs["runs"].as_array().unwrap().len(), 1);
    assert_eq!(runs["work"]["branch"], "duowave/sdelay-stranitsu-kontaktov");
    assert_eq!(runs["work"]["agentTitle"], "Claude Code");
    let session = runs["work"]["session"].as_str().unwrap().to_string();
    assert!(runs["work"]["resumeCommand"].as_str().unwrap().ends_with(&format!("--resume {session}")));
    // Its journal from a place, only the troubles, its raw file.
    let log = bench.get(&format!("/api/v1/works/{id}/runs/1/log?after=0")).await;
    let steps = log["steps"].as_array().unwrap();
    assert!(steps.len() > 8, "{log}");
    assert_eq!(log["going"], false);
    assert!(steps.iter().any(|step| step["parent"] == "task1"));
    let next = log["next"].as_u64().unwrap();
    let rest = bench.get(&format!("/api/v1/works/{id}/runs/1/log?after={next}")).await;
    assert_eq!(rest["steps"].as_array().unwrap().len(), 0);
    let troubles = bench.get(&format!("/api/v1/works/{id}/runs/1/log?trouble=1")).await;
    assert_eq!(troubles["steps"].as_array().unwrap().len(), 0, "{troubles}");
    let found = bench.get(&format!("/api/v1/works/{id}/runs/1/log?find=contacts")).await;
    assert!(!found["steps"].as_array().unwrap().is_empty());
    let raw = bench.gateway.get(&format!("/api/v1/works/{id}/runs/1/raw")).signed(&bench.admin).send().await.unwrap();
    assert_eq!(raw.status(), StatusCode::OK);
    assert_eq!(raw.headers()["content-type"], "application/x-ndjson");
    assert!(raw.text().await.unwrap().contains("\"subtype\":\"init\""));
}

/// One work in folders at a time: the next waits in the queue and starts by itself; a plan's limit asks from the
/// page, «подождать» waits and it goes on in the same session.
#[tokio::test]
async fn one_at_a_time_and_a_plans_limit_asks_from_the_page() {
    let bench = Bench::new().await;
    bench.agent_and_folder().await;
    bench.mode("slow");
    let (first, _) = bench.start("первая страница").await;
    let (second, said) = bench.start("вторая страница").await;
    assert!(said.contains("в очереди"), "{said}");
    let queued = bench.get("/api/v1/works?group=queued").await;
    let item = queued.as_array().unwrap().iter().find(|item| item["id"] == second.as_str()).cloned().unwrap();
    assert_eq!(item["wait"]["why"], "busy");
    std::fs::write(bench.stub.join("go-on"), "").unwrap();
    bench.work_until(&first, "done", |work| work["state"] == "done").await;
    bench.work_until(&second, "done", |work| work["state"] == "done").await;

    std::fs::remove_file(bench.stub.join("go-on")).unwrap();
    bench.mode("limit");
    let (id, _) = bench.start("третья страница").await;
    let asking = bench.work_until(&id, "asks", |work| work["group"] == "asking").await;
    let question = asking["question"]["text"].as_str().unwrap_or_default();
    // Only what is possible (docs/plans/work-systems.md, stage 5): no other system is here, the cheaper set is.
    assert!(question.starts_with("Лимит Claude Code кончился до "), "{question}");
    assert!(question.ends_with(". Подождать, остановить или перейти на «Экономно»?"), "{question}");
    let offers = &asking["question"]["offers"];
    assert_eq!(offers, &json!([{ "kind": "switch", "id": "economy", "title": "Экономно" }]), "{asking}");
    let waiting = bench.get("/api/v1/works?waiting=1").await;
    assert!(waiting.as_array().unwrap().iter().any(|work| work["id"] == id.as_str()));
    let said = bench.post(&format!("/api/v1/works/{id}/do"), json!({ "action": "answer", "answer": "yes" })).await;
    assert_eq!(said["said"], "Жду. Продолжу сам, когда лимит сбросится.");
    let done = bench.work_until(&id, "done", |work| work["state"] == "done").await;
    // The limit's question went; the one of «Готово» asks whether to read what the agent wrote (stage 5).
    let asks = done["question"]["text"].as_str().unwrap_or_default();
    assert!(asks.ends_with(" Рассказать, что написал агент?"), "{done}");
    let runs = bench.get(&format!("/api/v1/works/{id}/runs")).await;
    let outcomes: Vec<&str> =
        runs["runs"].as_array().unwrap().iter().map(|run| run["outcome"].as_str().unwrap()).collect();
    assert_eq!(outcomes, ["limit", "ok"]);
}

/// «Остановить», then «Продолжить»: the agent goes on in its session; «как дела» says what it is at.
#[tokio::test]
async fn a_stopped_work_goes_on_in_its_session() {
    let bench = Bench::new().await;
    bench.agent_and_folder().await;
    bench.mode("slow");
    let (id, _) = bench.start("страница контактов").await;
    let going =
        bench.work_until(&id, "runs", |work| work["step"].as_str().is_some_and(|step| step.contains("npm test"))).await;
    assert_eq!(going["step"], "запускает npm test");
    let status = bench.post(&format!("/api/v1/works/{id}/do"), json!({ "action": "status" })).await;
    assert!(status["said"].as_str().unwrap().contains(": запускает npm test."), "{status}");
    let stopped = bench.post(&format!("/api/v1/works/{id}/do"), json!({ "action": "stop" })).await;
    assert!(stopped["said"].as_str().unwrap().starts_with("Остановлено"), "{stopped}");
    bench.work_until(&id, "stopped", |work| work["state"] == "stopped").await;
    bench.mode("ok");
    let went_on = bench.post(&format!("/api/v1/works/{id}/do"), json!({ "action": "continue" })).await;
    assert!(went_on["said"].as_str().unwrap().starts_with("Передаю агенту"), "{went_on}");
    let works = bench.get("/api/v1/works").await;
    let again =
        works.as_array().unwrap().iter().find(|work| work["again"] == true).cloned().expect("a record of going on");
    let again_id = again["id"].as_str().unwrap().to_string();
    bench.work_until(&again_id, "done", |work| work["state"] == "done").await;
    let runs = bench.get(&format!("/api/v1/works/{again_id}/runs")).await;
    let whats: Vec<&str> = runs["runs"].as_array().unwrap().iter().map(|run| run["what"].as_str().unwrap()).collect();
    assert_eq!(whats, ["work", "resume"]);
    let sessions: Vec<&str> =
        runs["runs"].as_array().unwrap().iter().filter_map(|run| run["session"].as_str()).collect();
    assert!(sessions.len() == 2 && sessions[0] == sessions[1], "{sessions:?}");
}

/// The tools a request of the model offers, by name.
fn offered(request: &Value) -> Vec<String> {
    let tools = request["tools"].as_array().cloned().unwrap_or_default();
    tools.iter().filter_map(|tool| tool["function"]["name"].as_str().map(str::to_string)).collect()
}

/// Stage 5: a phrase said to the panel begins a work in a folder — read first when it names a folder, or by the
/// brain's `work_start` with a folder; «Готово» says the result and asks to read what the agent wrote, «подробнее»
/// reads it.
#[tokio::test]
async fn a_phrase_to_the_panel_begins_a_work_in_a_folder_and_more_reads_what_the_agent_wrote() {
    let llm = ChatStub::start().await;
    let bench =
        Bench::with(&[("DIALOG_PROVIDER", "openai"), ("LLM_BASE_URL", llm.url.as_str()), ("LLM_MODEL", "test-llm")])
            .await;
    // The reader reads «в папке …» as a work in a folder; later it reads a phrase as conversation, and the brain calls
    // `work_start` with a folder.
    let reads_folder = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    {
        let reads_folder = reads_folder.clone();
        llm.set_responder(move |request| {
            let tools = offered(request);
            if tools.iter().any(|tool| tool == "read") {
                return Some(if reads_folder.load(std::sync::atomic::Ordering::Relaxed) {
                    Scripted::call(
                        "read",
                        json!({ "act": "folder", "text": "сделай страницу контактов с формой", "folder": "Сайт" }),
                    )
                } else {
                    Scripted::call("read", json!({ "act": "talk" }))
                });
            }
            let answered = request["messages"].to_string().contains("\"role\":\"tool\"");
            if tools.iter().any(|tool| tool == "work_start") && !answered {
                return Some(Scripted::call(
                    "work_start",
                    json!({ "kind": "folder", "brief": "добавь ссылку на контакты в меню", "folder": "сайт" }),
                ));
            }
            None
        });
    }
    // No folder yet: the computer says so.
    let said = bench
        .post("/api/v1/talk", json!({ "text": "В папке Сайт сделай страницу контактов с формой", "via": "button" }))
        .await;
    assert_eq!(
        said["said"], "Рабочих папок пока нет. Добавьте папку в пульте, в «Рабочих папках» на странице «Работы».",
        "{said}"
    );
    bench.agent_and_folder().await;
    let said = bench
        .post("/api/v1/talk", json!({ "text": "В папке Сайт сделай страницу контактов с формой", "via": "button" }))
        .await;
    assert_eq!(
        said["said"], "Взяла в работу в папке «Сайт»: «Сделай страницу контактов с формой». Скажу, когда будет готово.",
        "{said}"
    );
    let id = said["work"].as_str().expect("the work it began").to_string();
    let work = bench.work_until(&id, "done", |work| work["state"] == "done").await;
    assert_eq!(work["kind"], "folder");
    assert_eq!(work["origin"]["via"], "voice");
    assert_eq!(
        work["result"]["line"],
        "Сделай страницу контактов с формой: готово. Ветка duowave/sdelay-stranitsu-kontaktov, проверки прошли, ≈ $1.20 по подписке. Рассказать, что написал агент?"
    );
    // The panel said its line: the question opens; «подробнее» reads the agent's last message.
    bench.post(&format!("/api/v1/works/{id}/said"), json!({})).await;
    let more = bench.post("/api/v1/talk", json!({ "text": "Подробнее", "via": "answer" })).await;
    assert_eq!(more["said"], "Done: the contacts page.", "{more}");

    // Read as conversation: the brain's `work_start` with a folder begins it — typed, so no wait between works by voice.
    reads_folder.store(false, std::sync::atomic::Ordering::Relaxed);
    let said = bench.post("/api/v1/talk", json!({ "text": "Пусть агент добавит ссылку на контакты в меню" })).await;
    assert_eq!(
        said["said"], "Взяла в работу в папке «Сайт»: «Добавь ссылку на контакты в меню». Скажу, когда будет готово.",
        "{said}"
    );
    let asked = llm.requests().into_iter().find(|request| offered(request).iter().any(|tool| tool == "work_start"));
    let start = asked.expect("the brain was offered work_start")["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["function"]["name"] == "work_start")
        .cloned()
        .unwrap();
    assert_eq!(start["function"]["parameters"]["properties"]["kind"]["enum"], json!(["thing", "folder"]));
}
