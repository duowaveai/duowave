//! The works of the talk end to end (docs/plans/audio-interface.md): the episodes of 12 November — A, B, C, D, E — and
//! the stage's own way («Как это будет»), with a fake brain that answers by what it is asked (the conversation's
//! model apart from the workshop's agent) and, where a station speaks, a fake station. Each test starts its own
//! gateway on a free port and signs in from scratch.

mod fakes;
mod support;

use fakes::{agent_made, agent_made_after, read_as, reads, ChatStub, Scripted};
use reqwest::StatusCode;
use serde_json::{json, Value};
use support::{bearer, message, until, Gateway};

trait Signed {
    fn signed(self, token: &str) -> Self;
}

impl Signed for reqwest::RequestBuilder {
    fn signed(self, token: &str) -> Self {
        let (name, value) = bearer(token);
        self.header(name, value)
    }
}

/// The instructions a request of the stub carries: its system message.
fn system_of(request: &Value) -> String {
    request["messages"][0]["content"].as_str().unwrap_or_default().to_string()
}

/// The workshop's agent asks with the workshop's skill; the conversation's brain with the household's character.
fn by_agent(request: &Value) -> bool {
    system_of(request).starts_with("# Мастерская")
}

/// A gateway whose brain is the stub, signed in as its admin.
async fn with_brain(llm: &ChatStub) -> (Gateway, String) {
    let vars = [("DIALOG_PROVIDER", "openai"), ("LLM_BASE_URL", llm.url.as_str()), ("LLM_MODEL", "test-llm")];
    let gateway = Gateway::start(&vars).await;
    let setup = message(
        gateway.post("/api/v1/auth/setup").json(&json!({ "password": "admin-password-1" })),
        StatusCode::CREATED,
    );
    let admin = setup.await["token"].as_str().expect("a token").to_string();
    (gateway, admin)
}

async fn works(gateway: &Gateway, admin: &str, query: &str) -> Vec<Value> {
    let works = message(gateway.get(&format!("/api/v1/works{query}")).signed(admin), StatusCode::OK).await;
    works.as_array().cloned().unwrap_or_default()
}

/// Stage 1: a run of the workshop's agent is a work; while it goes on, the brain's request carries the block
/// «Сейчас» with it, and the person reads the same block.
#[tokio::test]
async fn the_brain_knows_the_work_going_on() {
    let llm = ChatStub::start().await;
    // The agent takes its time; the brain answers at once.
    llm.set_responder(|request| by_agent(request).then(|| agent_made_after(request, "Сделано: слова дня.", 2500)));
    let (gateway, admin) = with_brain(&llm).await;

    let made = message(
        gateway
            .post("/api/v1/workshop/make")
            .signed(&admin)
            .json(&json!({ "words": "Сделай виджет: испанские слова дня" })),
        StatusCode::CREATED,
    )
    .await;
    let draft = made["draft"].as_str().unwrap().to_string();
    let going = works(&gateway, &admin, "?state=working").await;
    assert_eq!(going.len(), 1, "{going:?}");
    let work = &going[0];
    assert_eq!((work["id"].clone(), work["target"].clone()), (json!("1"), json!(draft)));
    assert_eq!(
        (work["kind"].clone(), work["face"].clone(), work["executor"].clone()),
        (json!("app"), json!("widget"), json!("workshop"))
    );
    assert_eq!(work["origin"]["device"], "computer", "the panel's work is the computer's");

    // The brain asked on the computer gets the block with the work going on («как там виджет?» is the layer's rule
    // now: the brain is asked in other words).
    llm.set_reply("Ещё делаю.");
    let said = message(
        gateway.post("/api/v1/talk").signed(&admin).json(&json!({ "text": "Расскажи, что у нас сейчас происходит" })),
        StatusCode::OK,
    )
    .await;
    assert_eq!(said["said"], "Ещё делаю.");
    let asked = llm.requests().into_iter().rev().find(|request| !by_agent(request)).expect("the brain was asked");
    let system = system_of(&asked);
    assert!(system.contains("Сейчас (знай, не пересказывай без вопроса):"), "{system}");
    assert!(system.contains("- В работе [р1]: виджет «Испанские слова дня»"), "{system}");
    assert!(system.contains("Просили: «Сделай виджет: испанские слова дня»"), "{system}");

    // The person reads what the brain knows.
    let now = message(gateway.get("/api/v1/talk/now").signed(&admin), StatusCode::OK).await;
    assert_eq!(now["device"], "computer");
    assert!(now["text"].as_str().unwrap().contains("[р1]"), "{now}");

    // The agent finished: the work is ready, and the block says it waits for an answer.
    assert!(until(10_000, || gateway.app.talk.works().get("1").is_some_and(|work| !work.going())).await);
    let work = message(gateway.get("/api/v1/works/1").signed(&admin), StatusCode::OK).await;
    assert_eq!(work["state"], "ready");
    let now = message(gateway.get("/api/v1/talk/now").signed(&admin), StatusCode::OK).await;
    assert!(now["text"].as_str().unwrap().contains("- Ждёт ответа [р1]"), "{now}");
    let (status, _) = support::send(gateway.get("/api/v1/works/99").signed(&admin)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    gateway.stop().await;
}

/// The tools a request of the stub offers.
fn tools_of(request: &Value) -> Vec<String> {
    let tools = request["tools"].as_array().cloned().unwrap_or_default();
    tools.iter().map(|tool| tool["function"]["name"].as_str().unwrap_or("").to_string()).collect()
}

/// The last thing the person said in a request of the stub.
fn last_said(request: &Value) -> String {
    let messages = request["messages"].as_array().cloned().unwrap_or_default();
    let user = messages.iter().rev().find(|message| message["role"] == "user");
    user.and_then(|message| message["content"].as_str()).unwrap_or_default().to_string()
}

/// Whether a request carries a tool's answer already (the round after a call).
fn answered(request: &Value) -> bool {
    request["messages"].as_array().is_some_and(|messages| messages.iter().any(|message| message["role"] == "tool"))
}

async fn talk(gateway: &Gateway, admin: &str, text: &str) -> String {
    let said = message(gateway.post("/api/v1/talk").signed(admin).json(&json!({ "text": text })), StatusCode::OK).await;
    said["said"].as_str().unwrap_or_default().to_string()
}

/// Episodes A and C of 12 November: a wish in one's own words — no «сделай виджет», no word of things, nothing going
/// on: not the reader's — starts one work through the brain's tool, and the person hears the gateway's ready phrase,
/// not a phrase dictated to repeat: the model is asked once, never to retell the tool's answer.
#[tokio::test]
async fn a_wish_in_ones_own_words_starts_one_work_and_nothing_is_dictated() {
    let llm = ChatStub::start().await;
    llm.set_responder(|request| {
        if by_agent(request) {
            return Some(agent_made_after(request, "Сделано: слова дня.", 300));
        }
        let wish = last_said(request).contains("испанский") && !answered(request);
        (wish && tools_of(request).contains(&"work_start".to_string())).then(|| {
            Scripted::call(
                "work_start",
                json!({ "brief": "я учу испанский; виджет со словами дня, знакомые подсвечиваются", "title": "Испанский: слова дня" }),
            )
        })
    });
    let (gateway, admin) = with_brain(&llm).await;

    let said =
        talk(&gateway, &admin, "Я учу испанский. Хочу каждый день новые слова, и чтобы знакомые подсвечивались.").await;
    assert_eq!(
        said, "Делаю: испанский: слова дня. Около минуты. Можно добавлять детали по ходу. Что-то ещё добавить?",
        "the first today: how to add on the way"
    );
    let brain: Vec<Value> = llm.requests().into_iter().filter(|request| !by_agent(request)).collect();
    assert_eq!(brain.len(), 1, "the ready phrase is said as it is: no second round");
    let system = system_of(&brain[0]);
    assert!(system.contains("никогда не диктуй человеку, какую фразу сказать"), "{system}");
    assert!(system.contains("отвечай на том языке, на котором тебя спросили"), "{system}");
    let tools = tools_of(&brain[0]);
    assert!(tools.contains(&"work_start".to_string()) && tools.contains(&"work_do".to_string()), "{tools:?}");

    // One work, one draft — of a widget, with the whole wish.
    let all = works(&gateway, &admin, "").await;
    assert_eq!(all.len(), 1, "{all:?}");
    let work = &all[0];
    assert_eq!(work["brief"], "я учу испанский; виджет со словами дня, знакомые подсвечиваются");
    assert_eq!((work["face"].clone(), work["title"].clone()), (json!("widget"), json!("Испанский: слова дня")));
    assert_eq!(
        (work["origin"]["activator"].clone(), work["origin"]["device"].clone()),
        (json!("text"), json!("computer"))
    );
    let drafts = gateway.app.workshop.drafts();
    assert_eq!(drafts.len(), 1);
    assert_eq!(Some(drafts[0].id.clone()), work["target"].as_str().map(str::to_string));
    assert_eq!(drafts[0].meta.work.as_deref(), Some("1"), "the card names its work");

    // The next request remembers the exchange — as the work's line, not as the ready phrase: a phrase of ours in the
    // history is a pattern the model said without doing anything (live 14.11).
    llm.set_reply("Пожалуйста.");
    assert_eq!(talk(&gateway, &admin, "Спасибо!").await, "Пожалуйста.");
    let last = llm.requests().into_iter().rev().find(|request| !by_agent(request)).unwrap();
    let remembered = last["messages"].to_string();
    assert!(remembered.contains("(В работе [р1]: виджет «Испанский: слова дня»"), "{remembered}");
    assert!(!remembered.contains("Делаю: испанский"), "{remembered}");
    gateway.stop().await;
}

/// A wish like the thing of the last half hour is a question — change it, or make a new one — not a second draft; the
/// wishes name a widget, so the reader reads them; «как там виджет?» is said from the work's progress.
#[tokio::test]
async fn a_wish_like_a_recent_thing_is_a_question_and_the_status_is_the_works_own() {
    let llm = ChatStub::start().await;
    llm.set_responder(|request| {
        if by_agent(request) {
            return Some(agent_made_after(request, "Сделано.", 3000));
        }
        if answered(request) {
            return None;
        }
        let said = last_said(request);
        if reads(request) {
            return Some(read_as(if said.contains("испан") {
                json!({ "act": "new", "text": said, "title": "Испанский" })
            } else {
                json!({ "act": "talk" })
            }));
        }
        if said.to_lowercase().contains("как там") {
            return Some(Scripted::call("work_do", json!({ "action": "status" })));
        }
        said.contains("испан").then(|| Scripted::call("work_start", json!({ "brief": said, "title": "Испанский" })))
    });
    let (gateway, admin) = with_brain(&llm).await;
    assert!(talk(&gateway, &admin, "Хочу виджет со словами по-испански").await.starts_with("Делаю: испанский."));

    // The same wish again: the layer's question, said as it is — the model gets nothing to retell.
    let before = llm.requests().len();
    assert_eq!(
        talk(&gateway, &admin, "Хочу ещё виджет с испанскими словами на каждый день").await,
        "Поправить «Испанский» — или сделать новую?"
    );
    let rounds = llm.requests().into_iter().skip(before).filter(|request| !by_agent(request)).count();
    assert_eq!(rounds, 1, "the call's round only: none to retell the question");
    assert_eq!(works(&gateway, &admin, "").await.len(), 1, "no second work");
    assert_eq!(gateway.app.workshop.drafts().len(), 1, "no second draft");

    // The template's words of the same wish: the layer's own question, without the model.
    let asked = llm.requests().len();
    assert_eq!(
        talk(&gateway, &admin, "Сделай виджет с испанскими словами на каждый день").await,
        "Поправить «Испанский» — или сделать новую?"
    );
    assert_eq!(llm.requests().len(), asked);

    // The answer is the layer's too: «новую» makes the wish that waited for it a new thing.
    let brain = || llm.requests().iter().filter(|request| !by_agent(request)).count();
    let asked = brain();
    let new = message(
        gateway.post("/api/v1/talk").signed(&admin).json(&json!({ "text": "Новую", "via": "answer" })),
        StatusCode::OK,
    )
    .await;
    assert!(new["said"].as_str().is_some_and(|said| said.starts_with("Делаю: ")), "{new}");
    assert_eq!(brain(), asked, "without the model");
    let all = works(&gateway, &admin, "").await;
    assert_eq!(all.len(), 2, "a second thing now: {all:?}");
    assert_eq!(all.iter().filter(|work| work["brief"] == "с испанскими словами на каждый день").count(), 1);

    // How it goes: two go on now, so the layer asks which; named, from the work's progress, said as it is.
    assert_eq!(talk(&gateway, &admin, "Как там виджет?").await, "Какую: «Новый виджет» или «Испанский»?");
    let status = talk(&gateway, &admin, "Как там испанский?").await;
    assert!(status.starts_with("В работе ") && status.ends_with('.'), "{status}");
    gateway.stop().await;
}

/// Episodes E and D of 12 November. E: what is added while the thing is made goes into the same work — the agent
/// reads «Вдогонку: …» at its next step; one draft, one work, one «Готово». D: a change of the finished thing is a new
/// version of it — the same draft, a new record with the same thing — not a second draft.
#[tokio::test]
async fn an_addition_goes_into_the_work_going_on_and_a_change_of_a_ready_thing_is_its_new_version() {
    let llm = ChatStub::start().await;
    llm.set_responder(|request| {
        if by_agent(request) {
            // Writes for a while, then finishes: the person's words come in between.
            return Some(agent_made_after(request, "Готово: слова дня, кнопка произношения.", 1500));
        }
        if answered(request) {
            return None;
        }
        let said = last_said(request).to_lowercase();
        if said.contains("кнопк") {
            return Some(Scripted::call("work_add", json!({ "text": "кнопку, чтобы слушать произношение" })));
        }
        if said.contains("покрупнее") {
            return Some(Scripted::call("work_add", json!({ "text": "слова покрупнее" })));
        }
        said.contains("испанский").then(|| {
            Scripted::call("work_start", json!({ "brief": "испанские слова дня", "title": "Испанский: слова дня" }))
        })
    });
    let (gateway, admin) = with_brain(&llm).await;
    let mut heard = gateway.app.events.subscribe();

    assert!(talk(&gateway, &admin, "Хочу виджет: испанский, слова дня").await.starts_with("Делаю: "));
    // E: while the agent works.
    // In words the layer's rules do not take («и ещё…» is their own): the brain's tool adds them.
    let added = talk(&gateway, &admin, "Ещё нужна кнопка, чтобы слушать произношение.").await;
    assert_eq!(added, "Добавлю: кнопку, чтобы слушать произношение.");
    // Ready, its line made by the gateway («Готово» is told once the line is there).
    assert!(until(10_000, || gateway.app.talk.works().get("1").is_some_and(|work| work.result.is_some())).await);
    let work = message(gateway.get("/api/v1/works/1").signed(&admin), StatusCode::OK).await;
    assert_eq!(work["state"], "ready");
    assert_eq!(work["notes"][0]["text"], "кнопку, чтобы слушать произношение");
    assert!(work["notes"][0]["readAt"].is_u64(), "the agent read it: {work}");
    let agent: Vec<Value> = llm.requests().into_iter().filter(by_agent).collect();
    assert_eq!(agent.len(), 2, "one run: the addition did not start it over");
    let after = agent[1]["messages"].to_string();
    assert!(after.contains("Вдогонку: кнопку, чтобы слушать произношение"), "{after}");
    assert_eq!(gateway.app.workshop.drafts().len(), 1, "one draft");
    let mut readies = 0;
    while let Ok(event) = heard.try_recv() {
        if let duowave::core::events::GatewayEvent::WorkChanged { change, .. } = event.as_ref() {
            readies += usize::from(change == "ready");
        }
    }
    assert_eq!(readies, 1, "one «Готово»");

    // D: the finished thing changed — its new version, the same draft.
    let draft = gateway.app.workshop.drafts()[0].id.clone();
    let changed = talk(&gateway, &admin, "А слова сделай покрупнее.").await;
    assert_eq!(changed, "Поправляю «Испанский: слова дня»: версия 2. Скажу, когда будет готово.");
    assert!(until(10_000, || gateway.app.talk.works().get("2").is_some_and(|work| !work.going())).await);
    let all = works(&gateway, &admin, "").await;
    assert_eq!(all.len(), 2);
    assert_eq!(
        (all[0]["target"].as_str(), all[0]["again"].clone(), all[0]["brief"].clone()),
        (Some(draft.as_str()), json!(true), json!("слова покрупнее"))
    );
    assert_eq!(all[1]["target"].as_str(), Some(draft.as_str()));
    assert_eq!(gateway.app.workshop.drafts().len(), 1, "still one draft");
    // The draft keeps the new words with the first wish it was made for (8c504303: «Переделай» carries it).
    assert_eq!(
        gateway.app.workshop.card(&draft).asked,
        "слова покрупнее\n\nРаньше об этой вещи просили: «испанские слова дня»"
    );
    gateway.stop().await;
}

/// Steps 7 and 6: what the model spent is in the journal — the request's tokens, every round summed — and in the
/// work's record; a spoken answer asks for at most 600 tokens; the request says where its phrase came from.
#[tokio::test]
async fn the_journal_and_the_work_tell_what_the_model_spent_and_a_spoken_answer_is_short() {
    let llm = ChatStub::start().await;
    llm.set_responder(|request| {
        if by_agent(request) {
            return Some(agent_made(request, "Сделано."));
        }
        (!answered(request) && last_said(request).contains("курс"))
            .then(|| Scripted::call("work_start", json!({ "brief": "курс доллара", "title": "Курс доллара" })))
    });
    let (gateway, admin) = with_brain(&llm).await;
    llm.set_reply("Сейчас полдень.");
    assert_eq!(talk(&gateway, &admin, "Который час?").await, "Сейчас полдень.");
    let brain = llm.requests().into_iter().find(|request| !by_agent(request)).unwrap();
    assert_eq!(brain["max_tokens"], 600, "a spoken answer: a few sentences");

    assert!(until(2000, || !gateway.app.requests.recent_views(5).is_empty()).await);
    let journal = message(gateway.get("/api/v1/requests?device=computer").signed(&admin), StatusCode::OK).await;
    let request = &journal["requests"][0];
    assert_eq!(
        request["tokens"],
        json!({
            "input": 100, "output": 20, "cacheRead": 80, "cacheWrite": 0, "rounds": 1, "model": "test-llm",
            "seat": "fast", "billing": "local", "usdMicro": 0
        }),
        "{request}"
    );
    // Step 6: where the phrase came from — typed on the computer, by the person signed in.
    assert_eq!(
        (request["origin"]["activator"].clone(), request["origin"]["device"].clone()),
        (json!("text"), json!("computer"))
    );
    assert_eq!(request["origin"]["via"], "text");
    assert!(request["origin"]["who"].is_string(), "{request}");

    assert!(talk(&gateway, &admin, "Хочу виджет: курс доллара").await.starts_with("Делаю: "));
    assert!(until(10_000, || gateway.app.talk.works().get("1").is_some_and(|work| !work.going())).await);
    let work = message(gateway.get("/api/v1/works/1").signed(&admin), StatusCode::OK).await;
    assert_eq!(
        work["tokens"],
        json!({ "input": 200, "output": 40, "cacheRead": 160, "cacheWrite": 0, "rounds": 2 }),
        "{work}"
    );
    // The router's deals (src/router): the agent's run is one deal on the workshop's seat, with its work and its
    // rounds summed (the input without the cache); the conversation's answers are on «Быстрая», with their requests.
    let listed = message(gateway.get("/api/v1/router/deals").signed(&admin), StatusCode::OK).await;
    let deals = listed["deals"].as_array().unwrap();
    let agent: Vec<&Value> = deals.iter().filter(|deal| deal["seat"] == "workshop").collect();
    assert_eq!(agent.len(), 1, "{listed}");
    assert_eq!((agent[0]["work"].as_str(), agent[0]["rounds"].as_u64()), (Some("1"), Some(2)), "{listed}");
    assert_eq!(agent[0]["tokens"], json!({ "input": 40, "output": 40, "cacheRead": 160, "cacheWrite": 0 }));
    assert!(deals.iter().filter(|deal| deal["seat"] == "fast").all(|deal| deal["request"].is_string()), "{listed}");
    gateway.stop().await;
}

/// Step 8 on the gateway's side: the panel's phrase begins a work and the answer names it; «который час» stays the
/// brain's; the computer's «Готово» is the work's line, said by the panel — once it says it to its end the question
/// opens, and «да» said after it (`via: answer`) puts the thing on the desk without the model; the old template's
/// words make a work without the model too.
#[tokio::test]
async fn the_panels_phrase_begins_a_work_and_its_yes_puts_it_on_the_desk() {
    let llm = ChatStub::start().await;
    llm.set_responder(|request| {
        if by_agent(request) {
            return Some(agent_made(request, "Сделано."));
        }
        // The wish names a widget: the reader reads it; «который час» it reads as conversation.
        reads(request).then(|| {
            read_as(if last_said(request).contains("курсом доллара") {
                json!({ "act": "new", "text": "курс доллара к рублю", "title": "Курс доллара" })
            } else {
                json!({ "act": "talk" })
            })
        })
    });
    let (gateway, admin) = with_brain(&llm).await;
    let said = message(
        gateway
            .post("/api/v1/talk")
            .signed(&admin)
            .json(&json!({ "text": "Я хочу виджет с курсом доллара", "via": "button", "heard": "home" })),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        (said["said"].clone(), said["work"].clone()),
        (json!("Делаю: курс доллара. Около минуты. Можно добавлять детали по ходу. Что-то ещё добавить?"), json!("1"))
    );
    llm.set_reply("Сейчас полдень.");
    let said =
        message(gateway.post("/api/v1/talk").signed(&admin).json(&json!({ "text": "Который час?" })), StatusCode::OK)
            .await;
    assert_eq!(said["said"], "Сейчас полдень.");
    assert!(said.get("work").is_none(), "no work: {said}");

    // The computer's «Готово»: the work's line; the panel says it, then listens for the answer.
    assert!(until(10_000, || gateway.app.talk.works().get("1").is_some_and(|work| work.result.is_some())).await);
    let work = message(gateway.get("/api/v1/works/1").signed(&admin), StatusCode::OK).await;
    assert_eq!(work["result"]["line"], "Курс доллара: готово. На стол или примерить?");
    let asked = llm.requests().len();
    message(gateway.post("/api/v1/works/1/said").signed(&admin), StatusCode::OK).await;
    let yes = message(
        gateway.post("/api/v1/talk").signed(&admin).json(&json!({ "text": "Да", "via": "answer", "heard": "home" })),
        StatusCode::OK,
    )
    .await;
    assert_eq!(yes["said"], "На столе.");
    assert_eq!(llm.requests().len(), asked, "the answer was the layer's, not the model's");
    let draft = gateway.app.talk.works().get("1").unwrap().target.unwrap();
    assert!(gateway.app.workshop.host().installed(&draft).is_some());

    // The old template's words: a work, without the model.
    let said = message(
        gateway.post("/api/v1/talk").signed(&admin).json(&json!({ "text": "Сделай виджет: часы с секундами" })),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        (said["said"].clone(), said["work"].clone()),
        (json!("Делаю: часы с секундами. Около минуты. Что-то ещё добавить?"), json!("2"))
    );
    let model_heard =
        |words: &str| llm.requests().iter().any(|request| !by_agent(request) && last_said(request).contains(words));
    assert!(!model_heard("часы с секундами") && !model_heard("Да"), "the template and the answer were the layer's");

    // A work's actions from the panel.
    let done =
        message(gateway.post("/api/v1/works/2/do").signed(&admin).json(&json!({ "action": "status" })), StatusCode::OK)
            .await;
    assert!(done["said"].as_str().is_some_and(|said| !said.is_empty()), "{done}");
    gateway.stop().await;
}

/// Change 4 of the hand check of 14.11: a run that changed nothing is no «готово» — a template no agent touched once
/// went on the desk as one; «на стол» while the thing is being made puts it there once it is ready, not the template
/// now.
#[tokio::test]
async fn a_run_that_changed_nothing_is_no_ready_and_the_desk_said_on_the_way_waits_for_it() {
    let llm = ChatStub::start().await;
    llm.set_responder(|request| {
        if !by_agent(request) {
            return None;
        }
        // The agent reads the wish and writes nothing for «пустышка»; the clock it writes, taking its time.
        Some(if request["messages"].to_string().contains("пустышка") {
            Scripted::reply("Не понял, что сделать.")
        } else {
            agent_made_after(request, "Сделано: часы.", 1500)
        })
    });
    let (gateway, admin) = with_brain(&llm).await;

    assert!(talk(&gateway, &admin, "Сделай виджет: пустышка").await.starts_with("Делаю: пустышка."));
    assert!(until(10_000, || gateway.app.talk.works().get("1").is_some_and(|work| work.result.is_some())).await);
    let work = message(gateway.get("/api/v1/works/1").signed(&admin), StatusCode::OK).await;
    assert_eq!(work["state"], "unfinished", "{work}");
    assert_eq!(work["result"]["line"], "Пустышка: ничего не поменялось. Что сделать?");
    let draft = work["target"].as_str().unwrap().to_string();
    assert!(gateway.app.workshop.card(&draft).unchanged, "the card says so too");

    assert!(talk(&gateway, &admin, "Сделай виджет: часы").await.starts_with("Делаю: часы."));
    assert_eq!(talk(&gateway, &admin, "На стол").await, "Ещё делаю — поставлю на стол, когда будет готово.");
    let clock = gateway.app.talk.works().get("2").and_then(|work| work.target).expect("the clock's draft");
    assert!(gateway.app.workshop.host().installed(&clock).is_none(), "not the template now");
    assert!(until(10_000, || gateway.app.talk.works().get("2").is_some_and(|work| work.result.is_some())).await);
    let work = message(gateway.get("/api/v1/works/2").signed(&admin), StatusCode::OK).await;
    assert_eq!(work["result"]["line"], "Часы: готово. На столе.", "{work}");
    assert_eq!(work["state"], "done");
    assert!(gateway.app.workshop.host().installed(&clock).is_some(), "on the desk once ready");
    gateway.stop().await;
}

/// Change 5 of the hand check of 14.11: the limits are a queue, not a refusal — «not now» retold by a model once
/// came out as «Делаю…» with nothing begun. Two at once (here one, for the test): the next wish waits and starts by
/// itself once the first is ready.
#[tokio::test]
async fn a_wish_over_the_limit_waits_in_the_queue_and_starts_by_itself() {
    let llm = ChatStub::start().await;
    llm.set_responder(|request| by_agent(request).then(|| agent_made_after(request, "Сделано.", 800)));
    let (gateway, admin) = with_brain(&llm).await;
    gateway.app.talk.works().set_at_once(1);

    assert!(talk(&gateway, &admin, "Сделай виджет: часы").await.starts_with("Делаю: часы."));
    assert_eq!(talk(&gateway, &admin, "Сделай виджет: погода").await, "Сейчас делается другое — начну следом: погода.");
    let waiting = message(gateway.get("/api/v1/works/2").signed(&admin), StatusCode::OK).await;
    assert_eq!(waiting["state"], "queued", "{waiting}");
    let now = message(gateway.get("/api/v1/talk/now").signed(&admin), StatusCode::OK).await;
    assert!(now["text"].as_str().unwrap().contains("- В очереди [р2]: виджет «Погода»"), "{now}");

    // The first is ready: the second starts by itself, and is made.
    assert!(until(10_000, || gateway.app.talk.works().get("2").is_some_and(|work| work.going())).await);
    assert!(until(10_000, || gateway.app.talk.works().get("2").is_some_and(|work| work.result.is_some())).await);
    assert_eq!(gateway.app.workshop.drafts().len(), 2, "two things, one after the other");
    gateway.stop().await;
}

/// A phrase of the panel's microphone: `via` — `button` (said after «Сказать») or `answer` (in the window after a
/// question).
async fn spoken(gateway: &Gateway, admin: &str, text: &str, via: &str) -> String {
    let said = message(
        gateway.post("/api/v1/talk").signed(admin).json(&json!({ "text": text, "via": via, "heard": "home" })),
        StatusCode::OK,
    )
    .await;
    said["said"].as_str().unwrap_or_default().to_string()
}

/// The owner's hand check of 14 November, phrase by phrase (docs/plans/audio-interface.md, «Доводка после проверки
/// руками 14.11»). The brain, asked in words, claims what it never did — «Готово: слова теперь крупнее», as it did
/// that evening; the reader reads every phrase about the works first, so that claim is never heard and the works are
/// done: the addition, the new version, the scrap of speech, the exit in other words, the wish over the limit.
#[tokio::test]
async fn the_hand_check_of_14_november_again() {
    const LIE: &str = "Готово: слова теперь крупнее.";
    let llm = ChatStub::start().await;
    llm.set_responder(|request| {
        if by_agent(request) {
            return Some(agent_made_after(request, "Сделано.", 1500));
        }
        let said = last_said(request);
        if reads(request) {
            let reading = if said.contains("Дай мне виджет") {
                json!({ "act": "new", "text": "я учу испанский; виджет со словами дня", "title": "Испанский: слова дня" })
            } else if said.contains("кнопку") {
                json!({ "act": "add", "text": "кнопку, чтобы слушать произношение" })
            } else if said.contains("покрупнее") {
                json!({ "act": "add", "thing": "Испанский", "text": "слова покрупнее" })
            } else if said.starts_with("Ос") {
                json!({ "act": "noise" })
            } else if said.contains("курсом доллара") {
                json!({ "act": "new", "text": "виджет с курсом доллара к рублю", "title": "Курс доллара" })
            } else {
                json!({ "act": "talk" })
            };
            return Some(read_as(reading));
        }
        Some(Scripted::reply(LIE))
    });
    let (gateway, admin) = with_brain(&llm).await;
    let mut heard: Vec<String> = Vec::new();
    let all = |gateway: &Gateway| gateway.app.talk.works().all(None, None);

    // 2–3: the wish, and what is added while it is made.
    let said = spoken(&gateway, &admin, "Я учу испанский. Дай мне виджет со словами дня.", "button").await;
    assert!(said.starts_with("Делаю: испанский: слова дня. Около минуты. Можно добавлять детали по ходу"), "{said}");
    heard.push(said);
    let said = spoken(&gateway, &admin, "Ещё кнопку, чтобы слушать произношение, добавь.", "button").await;
    assert_eq!(said, "Добавлю: кнопку, чтобы слушать произношение.");
    heard.push(said);

    // 5: «Готово», and «на стол» in the window after it.
    assert!(until(10_000, || gateway.app.talk.works().get("1").is_some_and(|work| work.result.is_some())).await);
    message(gateway.post("/api/v1/works/1/said").signed(&admin), StatusCode::OK).await;
    let said = spoken(&gateway, &admin, "На стол.", "answer").await;
    assert_eq!(said, "На столе.");
    heard.push(said);

    // 6: «покрупнее» — a new version of that thing, not «Готово» out of nothing.
    let said = spoken(&gateway, &admin, "Сделай в испанском слова покрупнее.", "button").await;
    assert_eq!(said, "Поправляю «Испанский: слова дня»: версия 2. Скажу, когда будет готово.");
    heard.push(said);
    let again = gateway.app.talk.works().get("2").expect("the new version's work");
    assert!(again.again && again.target == gateway.app.talk.works().get("1").unwrap().target, "{again:?}");

    // 7: the workshop's door — a scrap of speech is asked again, the exit in other words is the exit. The scrap came
    // that evening in the window after an answer, without the button: since stage 2 the closed window does not take
    // it at all (docs/plans/audio-interface-2.md, «К2») — no answer, no work; after the button it is asked again.
    heard.push(spoken(&gateway, &admin, "Мастерская.", "button").await);
    assert_eq!(spoken(&gateway, &admin, "Ос.", "answer").await, "", "the window does not take a scrap");
    let said = spoken(&gateway, &admin, "Ос.", "button").await;
    assert_eq!(said, "Не разобрать. Что сделать?");
    heard.push(said);
    let said = spoken(&gateway, &admin, "Выйди из режима мастерской.", "button").await;
    assert_eq!(said, "Мастерская закрыта.");
    heard.push(said);
    assert_eq!(all(&gateway).len(), 2, "no work out of a scrap or of the exit: {:?}", all(&gateway));

    // 8: a new wish by voice too soon after the first — the queue, and how it goes is said so.
    let said = spoken(&gateway, &admin, "Виджет с курсом доллара.", "button").await;
    assert!(said.starts_with("Начну через ") && said.ends_with(": курс доллара."), "{said}");
    heard.push(said);
    assert_eq!(gateway.app.talk.works().get("3").map(|work| work.state.as_str()), Some("queued"));
    let said = spoken(&gateway, &admin, "Как там курс доллара?", "button").await;
    assert_eq!(said, "Курс доллара: в очереди, начнётся сама.");
    heard.push(said);

    assert!(!heard.iter().any(|said| said == LIE), "the brain's claim was never said: {heard:?}");
    let titles: Vec<String> = all(&gateway).into_iter().map(|work| work.title).collect();
    assert!(!titles.iter().any(|title| title.contains("Выйди") || title == "Ос"), "{titles:?}");
    gateway.stop().await;
}

/// «Какую: … или …?» is the layer's question (live 15.11: «Первая.» answered it three times and it was asked again):
/// the action waits, and «первая» does it on the first thing named.
#[tokio::test]
async fn which_one_is_the_layers_question_and_the_first_is_the_first() {
    let llm = ChatStub::start().await;
    llm.set_responder(|request| by_agent(request).then(|| agent_made(request, "Сделано.")));
    let (gateway, admin) = with_brain(&llm).await;
    let ready = |id: &str| {
        let id = id.to_string();
        let works = gateway.app.talk.works();
        move || works.get(&id).is_some_and(|work| work.result.is_some())
    };

    assert!(talk(&gateway, &admin, "Сделай виджет: часы").await.starts_with("Делаю: часы."));
    assert!(until(10_000, ready("1")).await);
    assert_eq!(
        talk(&gateway, &admin, "Сделай виджет: часы с будильником").await,
        "Поправить «Часы» — или сделать новую?"
    );
    assert!(spoken(&gateway, &admin, "Новую", "answer").await.starts_with("Делаю: часы с будильником."));
    assert!(until(10_000, ready("2")).await);

    assert_eq!(talk(&gateway, &admin, "Как там часы?").await, "Какую: «Часы с будильником» или «Часы»?");
    assert_eq!(spoken(&gateway, &admin, "Первая", "answer").await, "Часы с будильником: готово.");
    gateway.stop().await;
}

/// A thing removed in the panel reaches its works by its id (live 15.11: a deleted widget stayed «на столе» for the
/// brain and was still found by its name): taken off the desk — in the drafts; its draft deleted too — gone, and
/// named it asks to be made again.
#[tokio::test]
async fn a_thing_removed_in_the_panel_is_gone_for_the_talk_by_its_id() {
    let llm = ChatStub::start().await;
    llm.set_responder(|request| by_agent(request).then(|| agent_made(request, "Сделано.")));
    let (gateway, admin) = with_brain(&llm).await;

    assert!(talk(&gateway, &admin, "Сделай виджет: часы").await.starts_with("Делаю: часы."));
    assert!(until(10_000, || gateway.app.talk.works().get("1").is_some_and(|work| work.result.is_some())).await);
    message(gateway.post("/api/v1/works/1/said").signed(&admin), StatusCode::OK).await;
    assert_eq!(spoken(&gateway, &admin, "Да", "answer").await, "На столе.");
    let id = gateway.app.talk.works().get("1").and_then(|work| work.target).expect("its id");
    let end = || gateway.app.talk.works().get("1").and_then(|work| work.result).and_then(|result| result.end);

    // Off the desk: its draft stays.
    message(gateway.delete(&format!("/api/v1/packages/{id}")).signed(&admin), StatusCode::OK).await;
    assert!(until(5_000, || end() == Some(duowave::talk::works::End::Parked)).await, "{:?}", end());
    // The draft deleted too: gone.
    let (status, _) = support::send(gateway.delete(&format!("/api/v1/workshop/drafts/{id}")).signed(&admin)).await;
    assert!(status.is_success(), "{status}");
    assert!(until(5_000, || end() == Some(duowave::talk::works::End::Removed)).await, "{:?}", end());
    let now = message(gateway.get("/api/v1/talk/now").signed(&admin), StatusCode::OK).await;
    assert!(now["text"].as_str().unwrap().contains("«Часы» — убрано"), "{now}");

    assert_eq!(talk(&gateway, &admin, "Как там часы?").await, "«Часы» больше нет — его убрали. Сделать заново?");
    assert!(spoken(&gateway, &admin, "Да", "answer").await.starts_with("Делаю: часы."));
    gateway.stop().await;
}

/// A phrase the panel's microphone heard by itself, after `why` it opened: the closed window's.
async fn in_window(gateway: &Gateway, admin: &str, text: &str, why: &str) -> Value {
    let body = json!({ "text": text, "via": "answer", "why": why, "heard": "home", "voice": true });
    message(gateway.post("/api/v1/talk").signed(admin).json(&body), StatusCode::OK).await
}

/// Stage 2 (docs/plans/audio-interface-2.md, «Как это будет», «К2», «К3»), with a brain that behaves badly: asked in
/// words, it says «Готово: сделала.» without a single call, and its reader reads a wish into a new thing whenever it
/// can. The closed window takes «и ещё…» to the work going on and the answers to the questions — the layer's and the
/// brain's own — and nothing else: talk in the room, a new wish, a question to the brain without the button never
/// reach the journal, the history or a work. «Хватит» declines the question; nobody's answer leaves it waiting.
#[tokio::test]
async fn the_closed_window_takes_an_answer_and_more_and_lets_go_of_the_rest() {
    const LIE: &str = "Готово: сделала.";
    let llm = ChatStub::start().await;
    llm.set_responder(|request| {
        if by_agent(request) {
            return Some(agent_made_after(request, "Сделано.", 1500));
        }
        let said = last_said(request);
        if reads(request) {
            let reading = if said.contains("курсом валют") {
                json!({ "act": "new", "text": "виджет с курсом валют", "title": "Курс валют" })
            } else if said.contains("с погодой") {
                json!({ "act": "new", "text": "виджет с погодой", "title": "Погода" })
            } else if said.contains("юань") {
                json!({ "act": "add", "text": "пусть показывает юань" })
            } else {
                json!({ "act": "talk" })
            };
            return Some(read_as(reading));
        }
        Some(Scripted::reply(if said.contains("Покажи погоду") {
            "Какой город?"
        } else if said.contains("Казань") {
            "В Казани солнечно."
        } else {
            LIE
        }))
    });
    let (gateway, admin) = with_brain(&llm).await;
    let journal = |gateway: &Gateway| gateway.app.requests.recent_views(100).len();
    let brain_heard = |words: &str| {
        llm.requests().iter().any(|request| !by_agent(request) && !reads(request) && last_said(request).contains(words))
    };

    // The wish — after the button: a work.
    assert_eq!(
        spoken(&gateway, &admin, "Я хочу виджет с курсом валют", "button").await,
        "Делаю: курс валют. Около минуты. Можно добавлять детали по ходу. Что-то ещё добавить?"
    );
    // «И ещё…» in the window after «Делаю…»: the work's, said by the layer; the brain is not asked.
    let added = in_window(&gateway, &admin, "И ещё пусть показывает юань", "work").await;
    assert_eq!(added["said"], "Добавлю: пусть показывает юань.");
    assert!(!brain_heard("юань"));
    let work = gateway.app.talk.works().get("1").unwrap();
    assert_eq!(work.notes.len(), 1, "{work:?}");

    // Talk in the room and a new wish, said in the window: not taken — no request, no work, no word of the brain.
    let before = journal(&gateway);
    for text in ["А что на ужин?", "Сделай ещё виджет с погодой"] {
        let dropped = in_window(&gateway, &admin, text, "work").await;
        assert_eq!((dropped["said"].clone(), dropped["dropped"].clone()), (json!(""), json!(true)), "{text}");
        assert!(!brain_heard(text), "{text}");
    }
    assert_eq!(journal(&gateway), before, "nothing the window did not take is in the journal");
    // «Нет» to «Что-то ещё добавить?»: the window closes, nothing is said, nothing is dropped.
    let no = in_window(&gateway, &admin, "Нет", "question").await;
    assert_eq!((no["said"].clone(), no.get("dropped").cloned()), (json!(""), None), "{no}");
    assert_eq!(gateway.app.talk.works().all(None, None).len(), 1, "no new work without the button");

    // «Готово…»: the panel said it; in the window «какая погода» is no answer — not taken, the lie never heard.
    assert!(until(10_000, || gateway.app.talk.works().get("1").is_some_and(|work| work.result.is_some())).await);
    message(gateway.post("/api/v1/works/1/said").signed(&admin), StatusCode::OK).await;
    let weather = in_window(&gateway, &admin, "Какая погода?", "ready").await;
    assert_eq!(weather["dropped"], true, "{weather}");
    assert!(!brain_heard("Какая погода"));
    // «Хватит» closes the window: the question is declined, nothing said; it does not wait for the person.
    let enough = in_window(&gateway, &admin, "Хватит", "ready").await;
    assert_eq!((enough["said"].clone(), enough.get("dropped").cloned()), (json!(""), None), "{enough}");
    let question = gateway.app.talk.works().get("1").unwrap().question.expect("the question stays in the record");
    assert_eq!(question.outcome, Some(duowave::talk::works::Outcome::Declined));
    assert!(works(&gateway, &admin, "?waiting=1").await.is_empty(), "declined: nothing waits");

    // The brain's own question: the answer said in the window is the brain's.
    let asked = message(
        gateway
            .post("/api/v1/talk")
            .signed(&admin)
            .json(&json!({ "text": "Покажи погоду", "via": "button", "heard": "home", "voice": true })),
        StatusCode::OK,
    )
    .await;
    assert_eq!((asked["said"].clone(), asked["asks"].clone()), (json!("Какой город?"), json!(true)));
    let answer = in_window(&gateway, &admin, "Казань", "question").await;
    assert_eq!(answer["said"], "В Казани солнечно.");
    assert!(brain_heard("Казань"));
    // Answered: the next phrase without the button is not the brain's any more.
    let after = in_window(&gateway, &admin, "А завтра?", "question").await;
    assert_eq!(after["dropped"], true, "{after}");
    gateway.stop().await;
}

/// The page «Работы» (docs/plans/router-and-works.md, decision 17, stage 6а): the list says where each work stands —
/// going, waiting in the queue and why, asking, recent — a question is answered by a button as by voice, and a work
/// has its deals and the model that answered it.
#[tokio::test]
async fn the_page_of_works_groups_them_says_why_one_waits_and_answers_a_question_by_a_button() {
    let llm = ChatStub::start().await;
    llm.set_responder(|request| by_agent(request).then(|| agent_made_after(request, "Сделано.", 1200)));
    let (gateway, admin) = with_brain(&llm).await;
    gateway.app.talk.works().set_at_once(1);

    assert!(talk(&gateway, &admin, "Сделай виджет: часы").await.starts_with("Делаю: часы."));
    assert_eq!(talk(&gateway, &admin, "Сделай виджет: погода").await, "Сейчас делается другое — начну следом: погода.");
    let going = works(&gateway, &admin, "?group=going").await;
    assert_eq!(going.len(), 1, "{going:?}");
    assert_eq!((going[0]["id"].clone(), going[0]["group"].clone()), (json!("1"), json!("going")));
    let queued = works(&gateway, &admin, "?group=queued").await;
    assert_eq!(queued.len(), 1, "{queued:?}");
    assert_eq!(queued[0]["id"], "2");
    assert_eq!(queued[0]["wait"], json!({ "why": "busy", "ahead": 0 }), "one goes on at once: it waits for its place");
    assert_eq!(works(&gateway, &admin, "?group=going,queued").await.len(), 2);
    assert_eq!(works(&gateway, &admin, "").await.len(), 2, "no group — all, as before");
    let (status, _) = support::send(gateway.get("/api/v1/works?group=soon").signed(&admin)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // The clock is ready and asks; the weather left the queue by itself.
    assert!(until(10_000, || gateway.app.talk.works().get("1").is_some_and(|work| work.result.is_some())).await);
    let asking = works(&gateway, &admin, "?group=asking").await;
    assert_eq!(asking.iter().map(|work| work["id"].clone()).collect::<Vec<_>>(), [json!("1")], "{asking:?}");
    assert_eq!(asking[0]["result"]["line"], "Часы: готово. На стол или примерить?");
    let (status, _) = support::send(
        gateway.post("/api/v1/works/2/do").signed(&admin).json(&json!({ "action": "answer", "answer": "yes" })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "a work going on asks nothing");
    let (status, _) = support::send(
        gateway.post("/api/v1/works/1/do").signed(&admin).json(&json!({ "action": "answer", "answer": "maybe" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // «На стол» on the page: the same as «да» said aloud, without the model.
    let asked = llm.requests().len();
    let yes = message(
        gateway.post("/api/v1/works/1/do").signed(&admin).json(&json!({ "action": "answer", "answer": "yes" })),
        StatusCode::OK,
    )
    .await;
    assert_eq!(yes["said"], "На столе.");
    let draft = gateway.app.talk.works().get("1").unwrap().target.unwrap();
    assert!(gateway.app.workshop.host().installed(&draft).is_some(), "the thing is on the desk");
    assert!(
        llm.requests()[asked..].iter().all(by_agent),
        "the answer was the layer's: only the second work's agent asked the model"
    );
    let clock = message(gateway.get("/api/v1/works/1").signed(&admin), StatusCode::OK).await;
    assert_eq!(clock["group"], "recent", "{clock}");
    assert!(works(&gateway, &admin, "?group=asking").await.is_empty());

    // The clock's deals: the agent's answer on the workshop's seat, by the stub's model; the work says the same.
    let deals = message(gateway.get("/api/v1/works/1/deals").signed(&admin), StatusCode::OK).await;
    let deals = deals["deals"].as_array().cloned().unwrap_or_default();
    assert!(!deals.is_empty(), "the agent's answer is a deal of the work");
    assert!(deals.iter().all(|deal| deal["work"] == "1" && deal["seat"] == "workshop"), "{deals:?}");
    assert_eq!(clock["model"]["model"], "test-llm", "{clock}");
    assert_eq!(clock["spent"]["deals"], json!(deals.len()), "{clock}");
    let recent = works(&gateway, &admin, "?group=recent").await;
    assert_eq!(recent[0]["id"], "1");
    let (status, _) = support::send(gateway.get("/api/v1/works/99/deals").signed(&admin)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    gateway.stop().await;
}

/// 27.11, a widget on GLM 5.3: the model's thinking took its whole answer four times running, six and a half minutes,
/// and the work said «ничего не поменялось» as if the agent had chosen to do nothing. Now the agent asks again once,
/// and the work says why it is not finished.
#[tokio::test]
async fn a_run_the_thinking_took_whole_is_asked_again_once_and_says_why_it_is_not_finished() {
    let llm = ChatStub::start().await;
    llm.set_responder(|request| by_agent(request).then(Scripted::thought_away));
    let (gateway, admin) = with_brain(&llm).await;

    assert!(talk(&gateway, &admin, "Сделай виджет: цитата").await.starts_with("Делаю: цитата."));
    assert!(until(10_000, || gateway.app.talk.works().get("1").is_some_and(|work| work.result.is_some())).await);
    let work = message(gateway.get("/api/v1/works/1").signed(&admin), StatusCode::OK).await;
    assert_eq!(work["state"], "unfinished", "{work}");
    assert_eq!(
        work["result"]["line"],
        "Цитата: не доделано. Модель думала слишком долго и не успела написать код. Продолжить?"
    );
    let draft = work["target"].as_str().unwrap().to_string();
    let card = gateway.app.workshop.card(&draft);
    assert!(card.overthought && !card.unchanged, "the card says why, not «ничего не поменялось»");
    assert_eq!(llm.requests().iter().filter(|request| by_agent(request)).count(), 2, "asked again once");
    gateway.stop().await;
}
