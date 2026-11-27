//! The works in folders end to end inside the crate, with the stub `claude` (`duowave-test-stub`, src/bin/test_stub.rs):
//! stage 3's conditions of readiness (docs/plans/router-and-works.md, «Этапы», 3).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::folders::{self, Asked, Guarded};
use super::harness::claude_code::KEY_SECRET;
use super::runs;
use super::*;
use crate::router::Billing;
use crate::settings::definitions::{HARNESS_CLAUDE_AUTH, HARNESS_CLAUDE_PATH};
use crate::talk::folder::{FolderEnd, FolderStart, PortEvent, WorksPort};

const RESUME: &str = "Продолжай работу с того места, где остановился.";

/// The folder's own commands, as the household writes them for its system: sh, and cmd on Windows (runner.rs).
const PREPARE: &str = if cfg!(windows) { "exit /b 0" } else { "true" };
const CHECK: &str =
    if cfg!(windows) { "if exist contacts.txt (exit /b 0) else (exit /b 1)" } else { "test -f contacts.txt" };

struct Bench {
    dir: tempfile::TempDir,
    works: Arc<FolderWorks>,
    settings: Arc<SettingsService>,
    secrets: Arc<SecretsService>,
    db: Arc<Db>,
    heard: Arc<Mutex<Vec<PortEvent>>>,
    /// The stub's folder: its mode and what it was asked.
    stub: PathBuf,
    folder: PathBuf,
    remote: PathBuf,
}

async fn git(dir: &Path, args: &[&str]) -> String {
    super::git::git(dir, args).await.unwrap_or_else(|why| panic!("git {args:?}: {why}"))
}

impl Bench {
    async fn new() -> Bench {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let db = Arc::new(Db::open(Some(&state)).unwrap());
        let settings = Arc::new(SettingsService::new(db.clone(), EventBus::new()));
        let secrets = Arc::new(SecretsService::new(db.clone(), Some(&state)).unwrap());
        let stub = dir.path().join("stub");
        let claude = crate::utils::test_stub::put(&stub, "claude");
        settings.set(&HARNESS_CLAUDE_PATH, &claude.display().to_string(), None).unwrap();
        // The person's folder with a remote of its own (a bare repository), on its branch `main`.
        let remote = dir.path().join("remote.git");
        std::fs::create_dir_all(&remote).unwrap();
        git(&remote, &["init", "--quiet", "--bare", "-b", "main"]).await;
        let folder = dir.path().join("site");
        std::fs::create_dir_all(&folder).unwrap();
        git(&folder, &["init", "--quiet", "-b", "main"]).await;
        git(&folder, &["config", "user.name", "Owner"]).await;
        git(&folder, &["config", "user.email", "owner@example.com"]).await;
        std::fs::write(folder.join("app.txt"), "app\n").unwrap();
        git(&folder, &["add", "-A"]).await;
        git(&folder, &["commit", "--quiet", "-m", "first"]).await;
        git(&folder, &["remote", "add", "origin", &remote.display().to_string()]).await;
        let works = FolderWorks::new(
            db.clone(),
            dir.path().join("data/works"),
            vec![dir.path().join("data")],
            settings.clone(),
            secrets.clone(),
            EventBus::new(),
        );
        let heard = Arc::new(Mutex::new(Vec::new()));
        let kept = heard.clone();
        works.listen(Arc::new(move |event| kept.lock().unwrap().push(event)));
        let guarded = Guarded { data: vec![dir.path().join("data")], home: None };
        let asked = Asked {
            name: Some("Сайт".into()),
            path: Some(folder.display().to_string()),
            prepare: Some(vec![PREPARE.into()]),
            checks: Some(vec![CHECK.into()]),
            ..Asked::default()
        };
        folders::add(&settings, &guarded, asked, None).await.unwrap();
        Bench { dir, works, settings, secrets, db, heard, stub, folder, remote }
    }

    fn mode(&self, mode: &str) {
        std::fs::write(self.stub.join("mode"), mode).unwrap();
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.stub.join("calls.log")).unwrap_or_default().lines().map(str::to_string).collect()
    }

    fn start(&self, task: &str) -> String {
        self.works
            .start(FolderStart {
                folder: "sayt".into(),
                task: task.into(),
                title: "Страница контактов".into(),
                agent: None,
                model: None,
                resume: RESUME.into(),
                set: None,
                way: None,
            })
            .unwrap()
    }

    /// Until the work ended (or another of its events came): its end.
    async fn ended(&self, id: &str) -> FolderEnd {
        for _ in 0..300 {
            let end = self.heard.lock().unwrap().iter().rev().find_map(|event| match event {
                PortEvent::Ended { id: ended, end } if ended == id => Some(*end),
                _ => None,
            });
            if let Some(end) = end {
                self.heard.lock().unwrap().retain(|event| !matches!(event, PortEvent::Ended { .. }));
                return end;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the work {id} never ended");
    }

    async fn until(&self, what: &str, check: impl Fn(&Bench) -> bool) {
        for _ in 0..300 {
            if check(self) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("never: {what}");
    }

    fn spent(&self) -> Vec<crate::router::Spent> {
        self.heard
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                PortEvent::Ran { spent, .. } => Some(spent.clone()),
                _ => None,
            })
            .collect()
    }
}

/// A run in a test folder: a branch of the work with a commit and a deal with the dollars of the result; the folder's
/// chosen branch unchanged; the copy has the folder's push addresses; the checks ran after it; the journal is read in
/// common steps with the subagent's nested, and its raw file is there; no key reached a run «как в терминале».
#[tokio::test]
async fn a_work_makes_a_branch_a_commit_a_deal_and_a_journal() {
    let bench = Bench::new().await;
    let id = bench.start("Сделай страницу контактов с формой");
    assert_eq!(bench.ended(&id).await, FolderEnd::Ready);
    let record = bench.works.record(&id).unwrap();
    assert_eq!(record.branch, "duowave/stranitsa-kontaktov");
    assert_eq!(record.base.as_deref(), Some("main"));
    assert!(record.fetched && record.committed && record.changed, "{record:?}");
    assert_eq!(record.usd_micro, Some(1_200_000));
    assert_eq!(record.billing.as_deref(), Some("plan"));
    assert_eq!(record.last.as_deref(), Some("Done: the contacts page."));
    let checks = record.checks.clone().unwrap();
    assert!(checks.len() == 1 && checks[0].ok, "{checks:?}");
    // The folder: the branch with the commit; its own branch and files as they were.
    assert_eq!(git(&bench.folder, &["symbolic-ref", "--short", "HEAD"]).await, "main");
    assert!(!bench.folder.join("contacts.txt").exists());
    let log = git(&bench.folder, &["log", "--format=%s|%an", "duowave/stranitsa-kontaktov", "-1"]).await;
    assert_eq!(log, "Страница контактов|Owner", "committed by the work, as the folder's git names who commits");
    // The copy goes to the folder's remotes.
    let copy = bench.works.copy_of(&id);
    assert_eq!(git(&copy, &["remote", "get-url", "--push", "origin"]).await, bench.remote.display().to_string());
    // The deal: the seat «Исполнитель», Claude Code's way, its dollars, by a plan.
    let spent = bench.spent();
    assert_eq!(spent.len(), 1);
    let bill = spent[0].bill.clone().unwrap();
    assert_eq!(
        (bill.via.as_str(), bill.billing, bill.usd_micro),
        ("harness:claude-code", Billing::Plan, Some(1_200_000))
    );
    assert_eq!(spent[0].key, format!("work:folder:{id}"));
    assert_eq!(spent[0].by.seat, crate::router::Seat::Worker);
    assert_eq!(spent[0].tokens.cache_read, 1000);
    // How it was run: never bypassPermissions nor --bare, edits accepted, nothing asked, no key.
    let calls = bench.calls();
    assert_eq!(calls.len(), 1);
    for word in [
        "--permission-mode acceptEdits",
        "--permission-prompts none",
        "--output-format stream-json",
        "--forward-subagent-text",
        "--session-id",
    ] {
        assert!(calls[0].contains(word), "{word}: {}", calls[0]);
    }
    assert!(!calls[0].contains("bypassPermissions") && !calls[0].contains("--bare"));
    assert!(calls[0].ends_with("key=no"), "«как в терминале» passes no key: {}", calls[0]);
    // The journal: common steps, the subagent's nested under its call, the gateway's own steps; the raw file.
    let runs = runs::list(&bench.db, &id).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].outcome.as_deref(), Some("ok"));
    let read = runs::read(&bench.works.dir, &id, 1, 0, 1000, false, None);
    let kinds: Vec<String> =
        read.steps.iter().map(|step| step["type"].as_str().unwrap_or_default().to_string()).collect();
    for kind in ["chore", "begin", "thinks", "tool", "plan", "says", "end"] {
        assert!(kinds.iter().any(|known| known == kind), "{kind} in {kinds:?}");
    }
    assert!(read.steps.iter().any(|step| step["parent"] == "task1"), "the subagent's steps carry its call");
    let from_middle = runs::read(&bench.works.dir, &id, 1, 3, 1000, false, None);
    assert_eq!(from_middle.steps.len(), read.steps.len() - 3);
    assert!(std::fs::read_to_string(runs::raw_path(&bench.works.dir, &id, 1))
        .unwrap()
        .contains("\"subtype\":\"init\""));
    drop(bench.dir);
}

/// «По ключу API» passes the gateway's key and the deal says `key`; no program, or not signed in — «не готов».
#[tokio::test]
async fn the_key_goes_only_by_key_and_an_agent_not_ready_says_why() {
    let bench = Bench::new().await;
    bench.settings.set(&HARNESS_CLAUDE_AUTH, &"key".to_string(), None).unwrap();
    let id = bench.start("Сделай страницу");
    assert_eq!(bench.ended(&id).await, FolderEnd::Failed, "no key yet");
    assert_eq!(bench.works.record(&id).unwrap().why.as_deref(), Some("auth"));
    bench.secrets.set(KEY_SECRET, Some("sk-test-key")).unwrap();
    let id = bench.start("Сделай страницу");
    assert_eq!(bench.ended(&id).await, FolderEnd::Ready);
    assert!(bench.calls().last().unwrap().ends_with("key=yes"));
    assert_eq!(bench.works.record(&id).unwrap().billing.as_deref(), Some("key"));
    assert_eq!(bench.spent().last().unwrap().bill.as_ref().unwrap().billing, Billing::Key);

    bench.settings.set(&HARNESS_CLAUDE_AUTH, &"terminal".to_string(), None).unwrap();
    std::fs::write(bench.stub.join("signed-out"), "").unwrap();
    let ready = bench.works.claude.ready().await;
    assert_eq!((ready.signed_in, ready.ok()), (Some(false), false));
    let id = bench.start("Сделай страницу");
    assert_eq!(bench.ended(&id).await, FolderEnd::Failed);
    assert_eq!(bench.works.record(&id).unwrap().why.as_deref(), Some("auth"));
    std::fs::remove_file(bench.stub.join("signed-out")).unwrap();
    let ready = bench.works.claude.ready().await;
    assert_eq!((ready.version.as_deref(), ready.pays.as_deref(), ready.ok()), (Some("2.1.302"), Some("plan"), true));

    bench
        .settings
        .set(&HARNESS_CLAUDE_PATH, &bench.dir.path().join("nothing/claude").display().to_string(), None)
        .unwrap();
    let ready = bench.works.claude.ready().await;
    assert!(matches!(ready.not, Some(super::harness::NotReady::Missing { .. })));
    let id = bench.start("Сделай страницу");
    assert_eq!(bench.ended(&id).await, FolderEnd::Failed);
    assert_eq!(bench.works.record(&id).unwrap().why.as_deref(), Some("no-agent"));
}

/// `git push` is an ordinary command (decision 21): refused unless the folder lists it; listed — it goes to the
/// folder's remote.
#[tokio::test]
async fn git_push_goes_only_when_the_folder_allows_it() {
    let bench = Bench::new().await;
    bench.mode("push");
    let id = bench.start("Сделай и отправь");
    assert_eq!(bench.ended(&id).await, FolderEnd::Ready);
    let branch = bench.works.record(&id).unwrap().branch;
    assert!(super::git::git(&bench.remote, &["rev-parse", "--verify", &format!("refs/heads/{branch}")]).await.is_err());
    let read = runs::read(&bench.works.dir, &id, 1, 0, 1000, true, None);
    assert!(read.steps.iter().any(|step| step["type"] == "denied"), "the refusal is in the journal");
    folders::update(
        &bench.settings,
        "sayt",
        Asked { allow: Some(vec!["Bash(git push:*)".into()]), ..Asked::default() },
        None,
    )
    .unwrap();
    let id = bench.start("Сделай и отправь");
    assert_eq!(bench.ended(&id).await, FolderEnd::Ready);
    let branch = bench.works.record(&id).unwrap().branch;
    assert_eq!(branch, "duowave/stranitsa-kontaktov-2", "a branch nobody has");
    assert!(bench.calls().last().unwrap().contains("--allowedTools Bash(git push:*)"));
    git(&bench.remote, &["rev-parse", "--verify", &format!("refs/heads/{branch}")]).await;
}

/// A note while it runs waits for the run's end and goes to the agent in the same session; a stop ends the run; a
/// note after the end is a new run in the same session too.
#[tokio::test]
async fn a_note_goes_on_in_the_same_session_and_a_stop_ends_the_run() {
    let bench = Bench::new().await;
    bench.mode("slow");
    let id = bench.start("Сделай страницу");
    bench.until("the agent runs", |bench| bench.works.doing(&id) == Some(FolderDoing::Runs("npm test".into()))).await;
    assert!(bench.works.add(&id, "и проверку почты"));
    std::fs::write(bench.stub.join("go-on"), "").unwrap();
    assert_eq!(bench.ended(&id).await, FolderEnd::Ready);
    assert!(bench.heard.lock().unwrap().iter().any(|event| matches!(event, PortEvent::Taken { .. })));
    let calls = bench.calls();
    assert_eq!(calls.len(), 2);
    let session = bench.works.record(&id).unwrap().session.unwrap();
    assert!(calls[0].contains(&format!("--session-id {session}")));
    assert!(calls[1].contains(&format!("--resume {session}")), "{}", calls[1]);
    let prompts = std::fs::read_to_string(bench.stub.join("prompts.log")).unwrap();
    assert!(prompts.contains("и проверку почты"));
    let runs = runs::list(&bench.db, &id).unwrap();
    assert_eq!(runs.iter().map(|run| run.what).collect::<Vec<_>>(), [runs::What::Work, runs::What::Note]);

    // After the end: a new run, the same session.
    bench.mode("ok");
    bench.works.again(&id, "и кнопку", RESUME).unwrap();
    assert_eq!(bench.ended(&id).await, FolderEnd::Ready);
    assert!(bench.calls().last().unwrap().contains(&format!("--resume {session}")));
    assert_eq!(runs::list(&bench.db, &id).unwrap().len(), 3);

    // A stop: the agent ends its turn, the work is stopped, what it did is handed over.
    std::fs::remove_file(bench.stub.join("go-on")).unwrap();
    bench.mode("slow");
    let id = bench.start("Сделай другую страницу");
    bench
        .until("the agent runs", |bench| {
            bench.works.doing(&id).is_some_and(|doing| matches!(doing, FolderDoing::Runs(_)))
        })
        .await;
    assert!(bench.works.stop(&id));
    assert_eq!(bench.ended(&id).await, FolderEnd::Stopped);
    assert_eq!(runs::list(&bench.db, &id).unwrap()[0].outcome.as_deref(), Some("stopped"));
}

/// A plan's limit: the question, the wait until it resets, then the same session goes on by itself.
#[tokio::test]
async fn a_plans_limit_is_waited_out_and_the_session_goes_on() {
    let bench = Bench::new().await;
    bench.mode("limit");
    let id = bench.start("Сделай страницу");
    bench
        .until("the question", |bench| {
            bench.heard.lock().unwrap().iter().any(|event| matches!(event, PortEvent::Waits { until: Some(_), .. }))
        })
        .await;
    assert_eq!(bench.works.record(&id).unwrap().state, State::Waiting);
    assert!(matches!(bench.works.doing(&id), Some(FolderDoing::Waits(Some(_)))));
    assert_eq!(bench.ended(&id).await, FolderEnd::Ready);
    assert!(bench.heard.lock().unwrap().iter().any(|event| matches!(event, PortEvent::Goes { .. })));
    let calls = bench.calls();
    assert_eq!(calls.len(), 2);
    assert!(calls[1].contains("--resume"));
    let prompts = std::fs::read_to_string(bench.stub.join("prompts.log")).unwrap();
    assert!(prompts.trim_end().ends_with(RESUME));
    let outcomes: Vec<Option<String>> =
        runs::list(&bench.db, &id).unwrap().into_iter().map(|run| run.outcome).collect();
    assert_eq!(outcomes, [Some("limit".to_string()), Some("ok".to_string())]);
}

/// Stage 5 of docs/plans/work-systems.md: a set chosen for a work gives its model; on a plan's limit the cheaper set is
/// offered, and the switch goes on at once on the set's model, in a new session told what was done.
#[tokio::test]
async fn a_set_gives_the_model_and_a_switch_on_a_limit_goes_on_in_a_new_session() {
    let bench = Bench::new().await;
    let quality = FolderStart {
        folder: "sayt".into(),
        task: "Сделай страницу".into(),
        title: "Страница".into(),
        agent: None,
        model: None,
        resume: RESUME.into(),
        set: Some(crate::router::sets::QUALITY.into()),
        way: None,
    };
    let id = bench.works.start(quality).unwrap();
    assert_eq!(bench.ended(&id).await, FolderEnd::Ready);
    assert!(bench.calls()[0].contains("--model opus"), "{:?}", bench.calls());
    let record = bench.works.record(&id).unwrap();
    assert_eq!((record.model.as_deref(), record.model_set.as_deref()), (Some("opus"), Some("quality")));

    std::fs::remove_file(bench.stub.join("calls.log")).unwrap();
    bench.mode("limit");
    let id = bench.start("Сделай ещё страницу");
    bench
        .until("the question", |bench| {
            bench.heard.lock().unwrap().iter().any(|event| matches!(event, PortEvent::Waits { .. }))
        })
        .await;
    assert_eq!(bench.works.cheaper(&id).as_deref(), Some(crate::router::sets::ECONOMY));
    bench.works.switch_set(&id, crate::router::sets::ECONOMY).unwrap();
    assert_eq!(bench.ended(&id).await, FolderEnd::Ready);
    let calls = bench.calls();
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert!(calls[1].contains("--model haiku") && !calls[1].contains("--resume"), "{calls:?}");
    assert_eq!(bench.works.record(&id).unwrap().model_set.as_deref(), Some(crate::router::sets::ECONOMY));
    assert_eq!(bench.works.cheaper(&id), None, "nothing cheaper than «Экономно»");
}

fn start_with(way: &str, title: &str) -> FolderStart {
    FolderStart {
        folder: "sayt".into(),
        task: "Сделай страницу контактов".into(),
        title: title.into(),
        agent: None,
        model: None,
        resume: RESUME.into(),
        set: None,
        way: Some(way.into()),
    }
}

/// Stage 6 of docs/plans/work-systems.md, «с планом»: the plan first, in a session that changes nothing (Claude Code
/// in its mode `plan`); the work waits for «да», goes by the plan, and is reviewed after, read only, on the set's
/// strongest model; its «Готово» says the review found nothing.
#[tokio::test]
async fn with_a_plan_it_waits_for_yes_goes_by_the_plan_and_is_reviewed() {
    let bench = Bench::new().await;
    let id = bench.works.start(start_with("plan", "Контакты")).unwrap();
    bench
        .until("the plan's question", |bench| {
            bench.heard.lock().unwrap().iter().any(|event| matches!(event, PortEvent::Plan { .. }))
        })
        .await;
    let plan = bench.heard.lock().unwrap().iter().find_map(|event| match event {
        PortEvent::Plan { plan, .. } => Some(plan.clone()),
        _ => None,
    });
    assert!(plan.unwrap().contains("Прочитать README"));
    assert_eq!(bench.works.record(&id).unwrap().state, State::Waiting);
    assert!(matches!(bench.works.doing(&id), Some(FolderDoing::WaitsPlan)));
    let calls = bench.calls();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].contains("--permission-mode plan"), "{calls:?}");
    assert!(bench.works.nudge(&id), "«да»");
    assert_eq!(bench.ended(&id).await, FolderEnd::Ready);
    let calls = bench.calls();
    assert_eq!(calls.len(), 3, "the plan, the work, the review: {calls:?}");
    assert!(calls[1].contains("--permission-mode acceptEdits"), "{calls:?}");
    assert!(calls[2].contains("--permission-mode plan") && calls[2].contains("--model opus"), "{calls:?}");
    let prompts = std::fs::read_to_string(bench.stub.join("prompts.log")).unwrap();
    assert!(prompts.contains("The plan agreed with the person:\n1. Прочитать README"), "{prompts}");
    let record = bench.works.record(&id).unwrap();
    assert_eq!((record.review.as_deref(), record.plan.is_some()), (Some("clean"), true));
    let told = WorksPort::told(bench.works.as_ref(), &id).unwrap();
    let words = &crate::talk::pack::of(crate::memory::text::Lang::Ru).work.folder;
    assert!(crate::talk::folder::summary_of(words, &told).contains("ревью без замечаний"));
    let kinds: Vec<String> =
        runs::list(&bench.db, &id).unwrap().into_iter().map(|run| run.what.as_str().to_string()).collect();
    assert_eq!(kinds, ["plan", "work", "review"]);
}

/// «С проверкой»: the review's blocking remarks go back to the writer's own session, and a clean review after says
/// how many were fixed; a review that never clears leaves the work unfinished after its two rounds, with the remarks.
#[tokio::test]
async fn with_a_review_its_remarks_go_back_and_one_that_never_clears_leaves_it_unfinished() {
    let bench = Bench::new().await;
    let fix = r#"{"verdict": "fix", "blocking": ["нет проверки email"], "notes": []}"#;
    std::fs::write(bench.stub.join("review"), fix).unwrap();
    let id = bench.works.start(start_with("review", "Контакты")).unwrap();
    assert_eq!(bench.ended(&id).await, FolderEnd::Ready);
    let calls = bench.calls();
    assert_eq!(calls.len(), 4, "the work, a review, the fix, a review: {calls:?}");
    assert!(calls[2].contains("--resume") && calls[2].contains("acceptEdits"), "the writer's session: {calls:?}");
    let prompts = std::fs::read_to_string(bench.stub.join("prompts.log")).unwrap();
    assert!(prompts.contains("The review found what must be fixed:\n- нет проверки email"), "{prompts}");
    assert_eq!(bench.works.record(&id).unwrap().review.as_deref(), Some("fixed:1"));

    std::fs::remove_file(bench.stub.join("reviewed.log")).unwrap();
    std::fs::write(bench.stub.join("review-again"), fix).unwrap();
    let id = bench.works.start(start_with("review", "Контакты ещё")).unwrap();
    assert_eq!(bench.ended(&id).await, FolderEnd::Unfinished);
    let record = bench.works.record(&id).unwrap();
    assert_eq!((record.why.as_deref(), record.review.as_deref()), (Some("review"), Some("нет проверки email")));
}

/// A restart in the middle: the work is interrupted; «продолжай» goes on in the same session and the same copy. A key
/// without money ends it «не доделано»; a run that reports bypassPermissions is ended at once.
#[tokio::test]
async fn a_restart_interrupts_and_go_on_takes_the_same_session() {
    let bench = Bench::new().await;
    bench.mode("slow");
    let id = bench.start("Сделай страницу");
    bench
        .until("the agent runs", |bench| {
            bench.works.doing(&id).is_some_and(|doing| matches!(doing, FolderDoing::Runs(_)))
        })
        .await;
    let session = bench.works.record(&id).unwrap().session.clone();
    // Another gateway on the same base: what went on is interrupted.
    let again = FolderWorks::new(
        bench.db.clone(),
        bench.works.dir.clone(),
        Vec::new(),
        bench.settings.clone(),
        bench.secrets.clone(),
        EventBus::new(),
    );
    bench.works.stop_all();
    assert_eq!(again.record(&id).unwrap().state, State::Interrupted);
    assert_eq!(runs::list(&bench.db, &id).unwrap()[0].outcome.as_deref(), Some("interrupted"));
    bench.until("the old job hands over and ends", |bench| bench.works.doing(&id).is_none()).await;
    assert_eq!(again.record(&id).unwrap().state, State::Interrupted, "cut by the gateway's stop: interrupted");
    assert!(!bench.heard.lock().unwrap().iter().any(|event| matches!(event, PortEvent::Ended { .. })), "nothing told");
    let heard = Arc::new(Mutex::new(Vec::new()));
    let kept = heard.clone();
    again.listen(Arc::new(move |event| kept.lock().unwrap().push(event)));
    bench.mode("ok");
    let session = session.or_else(|| again.record(&id).unwrap().session).unwrap();
    again.again(&id, RESUME, RESUME).unwrap();
    for _ in 0..300 {
        if heard.lock().unwrap().iter().any(|event| matches!(event, PortEvent::Ended { .. })) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(again.record(&id).unwrap().state, State::Ready);
    assert!(bench.calls().last().unwrap().contains(&format!("--resume {session}")));
    let runs = runs::list(&bench.db, &id).unwrap();
    assert_eq!(runs.last().unwrap().what, runs::What::Resume);

    bench.mode("billing");
    let id = bench.start("Ещё страницу");
    assert_eq!(bench.ended(&id).await, FolderEnd::Unfinished);
    assert_eq!(bench.works.record(&id).unwrap().why.as_deref(), Some("billing"));
    bench.mode("bypass");
    let id = bench.start("Ещё одну");
    assert_eq!(bench.ended(&id).await, FolderEnd::Unfinished);
    let read = runs::read(&bench.works.dir, &id, 1, 0, 1000, false, None);
    assert!(read
        .steps
        .iter()
        .any(|step| step["type"] == "end" && step["error"].as_str().is_some_and(|e| e.contains("bypassPermissions"))));
}

// ─── Stage 3б: agents of the Agent Client Protocol, with the stub agent (`duowave-test-stub` as `acp-stub`) ───

use crate::plugins::manifest::{AgentAcp, AgentCard, AgentModel, AgentProgram, AgentSignin};
use crate::settings::definitions::HARNESS_PATHS;

const AGENT: &str = "stub-agent";

/// The stub's card, written for this version of the protocol.
fn card_on(protocol: u64) -> AgentCard {
    AgentCard {
        site: Some("https://example.com".into()),
        terms: Some("https://example.com/terms".into()),
        program: AgentProgram {
            name: "acp-stub".into(),
            install: Some("install acp-stub".into()),
            check: Vec::new(),
            min_version: None,
        },
        acp: AgentAcp { command: Vec::new(), protocol },
        signin: AgentSignin { check: Vec::new(), hint: Some("sign in to the stub".into()) },
        model: AgentModel { choose: true, via: "agent".into() },
        resume: Some("acp-stub --resume {session}".into()),
        asks: Some(true),
        system: None,
    }
}

impl Bench {
    /// The stub agent as an installed card, its program where the panel set it; the folder's agent.
    fn with_agent(&self, allow: &[&str]) -> PathBuf {
        self.with_agent_on(1, allow)
    }

    /// The same, its card written for this version of the protocol.
    fn with_agent_on(&self, protocol: u64, allow: &[&str]) -> PathBuf {
        let program = crate::utils::test_stub::put(&self.stub, "acp-stub");
        self.works.agents.set_cards(Arc::new(move || {
            let card = card_on(protocol);
            vec![super::agents::Card { id: AGENT.into(), title: "Stub Agent".into(), description: None, card }]
        }));
        let paths = std::collections::BTreeMap::from([(AGENT.to_string(), program.display().to_string())]);
        self.settings.set(&HARNESS_PATHS, &paths, None).unwrap();
        let asked = Asked {
            agent: Some(AGENT.into()),
            model: Some("stub-large".into()),
            allow: Some(allow.iter().map(|rule| rule.to_string()).collect()),
            ..Asked::default()
        };
        folders::update(&self.settings, "sayt", asked, None).unwrap();
        program
    }

    fn agent_calls(&self) -> String {
        std::fs::read_to_string(self.stub.join("calls.log")).unwrap_or_default()
    }
}

/// A work with an agent of ACP goes the same way as with Claude Code: steps in the journal, the end, the branch, the
/// deal — the agent's own dollars, «оценка агента». Its questions of rights are answered by the folder's rules:
/// `allow_once` or `reject_once`, never `allow_always`; files outside the copy — no; `git push` — no unless listed.
#[tokio::test]
async fn an_acp_agent_works_like_claude_code_and_its_rights_follow_the_folder() {
    let bench = Bench::new().await;
    bench.with_agent(&["Bash(npm test *)"]);
    let id = bench.start("Сделай страницу контактов");
    assert_eq!(bench.ended(&id).await, FolderEnd::Ready);
    let record = bench.works.record(&id).unwrap();
    assert_eq!(record.agent, AGENT);
    assert!(record.fetched && record.committed && record.changed, "{record:?}");
    assert_eq!(record.last.as_deref(), Some("Done: the contacts page."));
    assert_eq!(record.billing.as_deref(), Some("agent"));
    let calls = bench.agent_calls();
    assert!(
        calls.contains(r#"initialize {"fs": {"readTextFile": false, "writeTextFile": false}, "terminal": false}"#),
        "{calls}"
    );
    assert!(calls.contains("model stub-large"), "the model asked for is set: {calls}");
    assert!(calls.contains("permission npm test -> once"), "{calls}");
    assert!(calls.contains("permission git push origin HEAD -> no"), "{calls}");
    assert!(calls.contains("permission Read a secret -> no"), "outside the copy: {calls}");
    assert!(!calls.contains("always"), "allow_always is never given");
    // The deal: the agent's own estimate, its way, its paying.
    let spent = bench.spent();
    let bill = spent.last().unwrap().bill.clone().unwrap();
    assert_eq!((bill.via.as_str(), bill.billing, bill.usd_micro), ("agent:stub-agent", Billing::Agent, Some(420_000)));
    assert_eq!(bill.from, crate::router::pool::PriceSource::Agent);
    assert_eq!(spent.last().unwrap().by.model, "stub-large");
    // The journal: thoughts and words joined from their pieces, the plan, the edit with its diff, the questions.
    let read = runs::read(&bench.works.dir, &id, 1, 0, 1000, false, None);
    let of = |kind: &str| read.steps.iter().filter(|step| step["type"] == kind).cloned().collect::<Vec<_>>();
    assert_eq!(of("thinks")[0]["text"], "A contacts page with a form.");
    assert_eq!(of("says")[0]["text"], "Done: the contacts page.");
    assert_eq!(of("plan").len(), 1);
    assert!(of("tool").iter().any(|step| step["diff"][0]["new"] == "form"), "{:?}", of("tool"));
    let asks = of("asks");
    assert_eq!(asks.len(), 3);
    assert_eq!(asks.iter().filter(|step| step["allowed"] == true).count(), 1);
    assert!(of("usage").iter().any(|step| step["usdMicro"] == 420_000));
    let resume = bench.works.harness(AGENT).unwrap().resume_command("s-1", Path::new("/data/copy")).unwrap();
    assert_eq!(resume, "cd /data/copy && acp-stub --resume s-1");
}

/// Without a cost the deal has tokens and no price; words after the end go on in the same session (`session/load`,
/// its replayed history not journaled again); a stop cancels the turn; «authentication required» — not signed in.
#[tokio::test]
async fn an_acp_agent_without_a_cost_goes_on_in_its_session_and_stops() {
    let bench = Bench::new().await;
    bench.with_agent(&[]);
    bench.mode("nocost");
    let id = bench.start("Сделай страницу");
    assert_eq!(bench.ended(&id).await, FolderEnd::Ready);
    assert_eq!(bench.spent().last().unwrap().bill.as_ref().unwrap().usd_micro, None, "price unknown");
    assert_eq!(bench.works.record(&id).unwrap().usd_micro, None);
    let session = bench.works.record(&id).unwrap().session.unwrap();
    bench.works.again(&id, "и кнопку", RESUME).unwrap();
    assert_eq!(bench.ended(&id).await, FolderEnd::Ready);
    let calls = bench.agent_calls();
    assert!(calls.contains(&format!("load {session}")), "{calls}");
    assert!(calls.contains(&format!("prompt {session}: и кнопку")), "the same session: {calls}");
    let second = runs::read(&bench.works.dir, &id, 2, 0, 1000, false, None);
    assert!(!second.steps.iter().any(|step| step["text"] == "replayed history"), "the replay is not journaled");

    bench.mode("slow");
    let id = bench.start("Ещё страницу");
    bench
        .until("the agent runs", |bench| {
            bench.works.doing(&id).is_some_and(|doing| matches!(doing, FolderDoing::Runs(_)))
        })
        .await;
    assert!(bench.works.stop(&id));
    assert_eq!(bench.ended(&id).await, FolderEnd::Stopped);

    bench.mode("auth");
    let id = bench.start("Ещё одну");
    assert_eq!(bench.ended(&id).await, FolderEnd::Unfinished);
    assert_eq!(bench.works.record(&id).unwrap().why.as_deref(), Some("auth"));
}

/// Junie of JetBrains (`junie --acp true`; the card `packages/<id>` of the catalog) over ACP version 1, as it spoke in a
/// real run on 27.11.2026: its questions of rights have only «Yes» (`allow_once`) and «No» (`reject_once`) — the folder's
/// rules choose between them; the title and the command list arrive as updates; a command comes with its folder, its
/// output with an exit code; a read starts as «other»; the turn's tokens come in the prompt's answer and no price.
#[tokio::test]
async fn an_agent_that_speaks_as_junie_does_gets_its_questions_answered_by_the_folder() {
    let bench = Bench::new().await;
    bench.with_agent(&["Bash(echo *)"]);
    bench.mode("junie");
    let id = bench.start("Запиши hello в probe.txt");
    assert_eq!(bench.ended(&id).await, FolderEnd::Ready);
    let record = bench.works.record(&id).unwrap();
    assert!(record.changed, "the file the allowed command wrote: {record:?}");
    assert_eq!(record.last.as_deref(), Some("**Summary**\n- probe.txt holds `hello`."), "trimmed");
    let calls = bench.agent_calls();
    assert!(calls.contains("permission echo hello > probe.txt -> Yes"), "{calls}");
    assert!(calls.contains("permission rm -rf node_modules -> No"), "{calls}");
    let read = runs::read(&bench.works.dir, &id, 1, 0, 1000, false, None);
    let asks: Vec<_> = read.steps.iter().filter(|step| step["type"] == "asks").collect();
    assert_eq!(asks.len(), 2, "{:?}", read.steps);
    assert_eq!(asks.iter().filter(|step| step["allowed"] == true).count(), 1);
    assert_eq!(bench.spent().last().unwrap().bill.as_ref().unwrap().usd_micro, None, "tokens, no price");
}

/// An unknown version of the protocol — «не готов», no run on a guess; a card without its program — words with how
/// to install it.
#[tokio::test]
async fn an_unknown_protocol_or_a_missing_program_is_not_ready() {
    use super::harness::NotReady;
    let bench = Bench::new().await;
    let program = bench.with_agent(&[]);
    std::fs::write(bench.stub.join("protocol"), "3").unwrap();
    let id = bench.start("Сделай страницу");
    assert_eq!(bench.ended(&id).await, FolderEnd::Failed);
    let record = bench.works.record(&id).unwrap();
    assert_eq!(record.why.as_deref(), Some("no-agent"));
    assert!(record.error.unwrap_or_default().contains("3"), "the version it said");
    assert!(!bench.agent_calls().contains("prompt"), "no run on a guess");
    let ready = bench.works.harness(AGENT).unwrap().ready().await;
    assert!(matches!(ready.not, Some(NotReady::Unknown { detail: Some(_) })), "{ready:?}");

    std::fs::remove_file(&program).unwrap();
    bench.works.agents.forget(AGENT);
    let ready = bench.works.harness(AGENT).unwrap().ready().await;
    assert_eq!(ready.not, Some(NotReady::Missing { install: Some("install acp-stub".into()) }));
}

/// Version 2 of the protocol (docs/plans/work-systems.md, stage 2): a card written for it meets an agent that speaks
/// it, and a work goes as in version 1. The prompt is answered at once and its turn ends with the idle state; the
/// journal names the version, and the update of a kind no client knows is counted at the end. A question of rights
/// names a command and the folder it runs in: only the copy; a whole message after its pieces adds nothing.
#[tokio::test]
async fn an_acp_agent_of_version_2_works_as_one_of_version_1() {
    let bench = Bench::new().await;
    bench.with_agent_on(2, &["Bash(npm test *)"]);
    std::fs::write(bench.stub.join("protocol"), "2").unwrap();
    let id = bench.start("Сделай страницу контактов");
    assert_eq!(bench.ended(&id).await, FolderEnd::Ready);
    let record = bench.works.record(&id).unwrap();
    assert!(record.fetched && record.committed && record.changed, "{record:?}");
    assert_eq!(record.last.as_deref(), Some("Done: the contacts page."));
    let calls = bench.agent_calls();
    assert!(calls.contains("asked 2"), "{calls}");
    assert!(calls.contains("model stub-large id"), "version 2's option and its kind of value: {calls}");
    assert!(calls.contains("permission npm test -> once"), "{calls}");
    assert!(calls.contains("permission npm test in the root -> no"), "a command runs only in the copy: {calls}");
    assert!(calls.contains("permission git push origin HEAD -> no"), "{calls}");
    assert!(calls.contains("permission Read a secret -> no"), "outside the copy: {calls}");
    assert!(!calls.contains("always"), "allow_always is never given");
    let bill = bench.spent().last().unwrap().bill.clone().unwrap();
    assert_eq!((bill.via.as_str(), bill.billing, bill.usd_micro), ("agent:stub-agent", Billing::Agent, Some(420_000)));
    let read = runs::read(&bench.works.dir, &id, 1, 0, 1000, false, None);
    let of = |kind: &str| read.steps.iter().filter(|step| step["type"] == kind).cloned().collect::<Vec<_>>();
    assert_eq!(of("begin")[0]["protocol"], "ACP 2");
    assert_eq!(of("begin")[0]["version"], "0.2.0", "the agent's version as it named itself");
    assert_eq!(of("thinks")[0]["text"], "A contacts page with a form.");
    assert_eq!(of("says")[0]["text"], "Done: the contacts page.");
    assert_eq!(of("says").len(), 1, "{:?}", of("says"));
    assert_eq!(of("plan")[0]["entries"][0]["text"], "Form");
    let tools = of("tool");
    assert!(
        tools.iter().any(|step| step["diff"][0]["new"] == "+form" && step["diff"][0]["old"].is_null()),
        "{tools:?}"
    );
    assert!(tools.iter().any(|step| step["output"] == "Tests: 3 passed"), "{tools:?}");
    assert_eq!(of("asks").len(), 4);
    assert_eq!(of("asks").iter().filter(|step| step["allowed"] == true).count(), 1);
    assert_eq!(of("end")[0]["outcome"], "ok");
    assert_eq!(of("end")[0]["unknown"], 1, "the update of a later version is counted");
}

/// Version 2 goes on in the same session by `session/resume`, with nothing replayed, and what the session spent before
/// is not counted again; a stop cancels the turn, which ends idle; «authentication required» — not signed in.
#[tokio::test]
async fn an_acp_agent_of_version_2_goes_on_by_resume_and_stops() {
    let bench = Bench::new().await;
    bench.with_agent_on(2, &[]);
    std::fs::write(bench.stub.join("protocol"), "2").unwrap();
    let id = bench.start("Сделай страницу");
    assert_eq!(bench.ended(&id).await, FolderEnd::Ready);
    let session = bench.works.record(&id).unwrap().session.unwrap();
    bench.works.again(&id, "и кнопку", RESUME).unwrap();
    assert_eq!(bench.ended(&id).await, FolderEnd::Ready);
    let calls = bench.agent_calls();
    assert!(calls.contains(&format!("resume {session}")), "{calls}");
    assert!(!calls.contains("load "), "no session/load in version 2: {calls}");
    assert!(calls.contains(&format!("prompt {session}: и кнопку")), "the same session: {calls}");
    let bill = bench.spent().last().unwrap().bill.clone().unwrap();
    assert_eq!(bill.usd_micro, Some(420_000), "0.84 said at the end, 0.42 of it before this run");

    bench.mode("slow");
    let id = bench.start("Ещё страницу");
    bench
        .until("the agent runs", |bench| {
            bench.works.doing(&id).is_some_and(|doing| matches!(doing, FolderDoing::Runs(_)))
        })
        .await;
    assert!(bench.works.stop(&id));
    assert_eq!(bench.ended(&id).await, FolderEnd::Stopped);

    bench.mode("auth");
    let id = bench.start("Ещё одну");
    assert_eq!(bench.ended(&id).await, FolderEnd::Unfinished);
    assert_eq!(bench.works.record(&id).unwrap().why.as_deref(), Some("auth"));
}

/// A card written for version 2 meets an agent of version 1: the agent answers 1, and the work goes in version 1. An
/// agent that knows neither version it is asked refuses `initialize`: «не готов», in words.
#[tokio::test]
async fn a_card_of_version_2_meets_an_agent_of_version_1_or_none() {
    let bench = Bench::new().await;
    bench.with_agent_on(2, &["Bash(npm test *)"]);
    let id = bench.start("Сделай страницу контактов");
    assert_eq!(bench.ended(&id).await, FolderEnd::Ready);
    let calls = bench.agent_calls();
    assert!(calls.contains("asked 2"), "{calls}");
    assert!(calls.contains("model stub-large") && !calls.contains("model stub-large id"), "{calls}");
    assert!(calls.contains("permission npm test -> once"), "{calls}");
    let read = runs::read(&bench.works.dir, &id, 1, 0, 1000, false, None);
    let begin = read.steps.iter().find(|step| step["type"] == "begin").cloned().unwrap();
    assert_eq!(begin["protocol"], "ACP 1");

    let bench = Bench::new().await;
    bench.with_agent_on(2, &[]);
    std::fs::write(bench.stub.join("protocol"), "reject").unwrap();
    let id = bench.start("Сделай страницу");
    assert_eq!(bench.ended(&id).await, FolderEnd::Failed);
    let record = bench.works.record(&id).unwrap();
    assert_eq!(record.why.as_deref(), Some("no-agent"));
    assert!(record.error.unwrap_or_default().contains("1 и 2"), "the versions DuoWave knows");
    assert!(!bench.agent_calls().contains("prompt"), "no run on a guess");
}

/// A door like app.rs's over the bench: what the work's runs spent, as the port heard them; a work's own budget by its
/// id at the helper.
fn door(bench: &Bench) -> (Arc<crate::router::Router>, crate::talk::folder::WorkerDoor) {
    let router = crate::router::Router::new(
        bench.db.clone(),
        bench.settings.clone(),
        EventBus::new(),
        crate::router::pool::EnvModel { kind: crate::config::DialogProvider::None, base_url: None },
    );
    let (heard, of) = (bench.heard.clone(), router.clone());
    let door: crate::talk::folder::WorkerDoor = Arc::new(move |key: &str, mut ask: crate::router::WorkerAsk| {
        let id = key.strip_prefix("work:folder:").unwrap_or(key).to_string();
        let usd: u64 = heard
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                PortEvent::Ran { id: ran, spent, .. } if *ran == id => spent.bill.as_ref()?.usd_micro,
                _ => None,
            })
            .sum();
        ask.spent.usd_micro = usd;
        ask.work = of.budgets.get(&id, 0);
        of.worker(&ask)
    });
    (router, door)
}

/// Router stage 2 (docs/plans/router-and-works.md, «Этапы», 2): the folder's budget holds — stage 3's dollars of a
/// folder are its budget now —, a run may spend what is left to the question; at the step `cheaper` (0.8) the next run
/// goes on a cheaper model through the table — another model, another session, its deal by `budget:cheaper`; at the
/// step `ask` the work waits on the question, and goes on once its own budget grew.
#[tokio::test]
async fn the_budget_sends_the_next_run_cheaper_then_asks_and_goes_on_when_it_grew() {
    let bench = Bench::new().await;
    // Each run of the stub costs $1.20: after the first, 1.20 of 1.40 — past 0.8.
    folders::update(&bench.settings, "sayt", Asked { max_usd: Some(1.4), ..Asked::default() }, None).unwrap();
    let (router, door) = door(&bench);
    bench.works.attach_door(door);
    let id = bench.start("Сделай страницу");
    assert_eq!(bench.ended(&id).await, FolderEnd::Ready);
    let calls = bench.calls();
    assert!(calls[0].contains("--max-budget-usd 1.40") && !calls[0].contains("--model"), "{}", calls[0]);
    let first = bench.works.record(&id).unwrap().session.unwrap();

    // «И кнопку» after the end: the step `cheaper` — a new session on haiku, told the task and the last result.
    bench.works.again(&id, "и кнопку", RESUME).unwrap();
    assert_eq!(bench.ended(&id).await, FolderEnd::Ready);
    let calls = bench.calls();
    assert!(calls[1].contains("--model haiku") && calls[1].contains("--session-id"), "{}", calls[1]);
    assert!(!calls[1].contains(&first), "another model is another session: {}", calls[1]);
    let prompts = std::fs::read_to_string(bench.stub.join("prompts.log")).unwrap();
    assert!(prompts.contains("Сделай страницу") && prompts.contains("Done: the contacts page."), "{prompts}");
    let spent = bench.spent();
    assert_eq!(spent[1].by.reason, crate::router::Reason::Budget(crate::router::budgets::Act::Cheaper));
    let record = bench.works.record(&id).unwrap();
    assert_eq!(record.session_model.as_deref(), Some("haiku"));

    // Spent 2.40 of 1.40: the question before the next run; the work waits.
    bench.works.again(&id, "и подвал", RESUME).unwrap();
    bench
        .until("the budget's question", |bench| {
            bench.heard.lock().unwrap().iter().any(|event| matches!(event, PortEvent::Budget { .. }))
        })
        .await;
    assert_eq!(bench.works.record(&id).unwrap().state, State::Waiting);
    assert_eq!(bench.works.doing(&id), Some(FolderDoing::WaitsBudget));
    assert_eq!(bench.calls().len(), 2, "nothing runs while it asks");
    // «Добавить»: the work's own budget, over the folder's — the door is asked again, the run goes, back on the
    // work's own model in a new session.
    let raised = crate::router::budgets::Budget::usd(10.0);
    router.budgets.set(&id, 0, Some(&raised), 1).unwrap();
    assert!(bench.works.nudge(&id));
    assert_eq!(bench.ended(&id).await, FolderEnd::Ready);
    let calls = bench.calls();
    assert_eq!(calls.len(), 3);
    assert!(!calls[2].contains("--model") && calls[2].contains("--max-budget-usd 7.60"), "{}", calls[2]);
}

/// A step `stop` ends the work «не доделано» before the run; the regulator keeps Claude Code's cloud in.
#[tokio::test]
async fn the_budgets_stop_and_the_regulator_hold_a_work() {
    let bench = Bench::new().await;
    let tokens = serde_json::json!({"measure": "tokens", "limit": 100, "steps": [{"at": 1, "do": "stop"}]});
    folders::update(&bench.settings, "sayt", Asked { budget: Some(tokens), ..Asked::default() }, None).unwrap();
    let (_, plain) = door(&bench);
    bench.works.attach_door(Arc::new(move |key: &str, mut ask: crate::router::WorkerAsk| {
        ask.spent.tokens.output = 500;
        plain(key, ask)
    }));
    let id = bench.start("Сделай страницу");
    assert_eq!(bench.ended(&id).await, FolderEnd::Failed, "stopped before its first run");
    assert_eq!(bench.works.record(&id).unwrap().why.as_deref(), Some("budget"));
    assert!(bench.calls().is_empty());

    let bench = Bench::new().await;
    let (_, door) = door(&bench);
    bench.works.attach_door(door);
    bench.settings.set(&crate::settings::definitions::AWAY, &crate::helpers::Away::Home, None).unwrap();
    let id = bench.start("Сделай страницу");
    assert_eq!(bench.ended(&id).await, FolderEnd::Failed);
    assert_eq!(bench.works.record(&id).unwrap().why.as_deref(), Some("closed"));
    assert!(bench.calls().is_empty());
}

// ─── Work systems, stage 1: our runner as the system `duowave` (docs/plans/work-systems.md) ───

/// Our runner stands behind the contract as the system `duowave`: it passes the kit, its folders are its places and
/// its agents its setups, and the end of a work is told as the contract's event.
#[tokio::test]
async fn our_runner_is_the_system_duowave_and_passes_the_kit() {
    use crate::talk::system::{SystemEvent, SystemState, WorkSystem};
    let bench = Bench::new().await;
    bench.with_agent(&[]);
    bench.mode("slow");
    let system = super::system::DuowaveSystem::new(bench.works.clone());
    let problems = crate::talk::system::check(system.as_ref(), Some("поправь опечатку")).await;
    assert!(problems.is_empty(), "{problems:?}");
    assert!(system.places().await.iter().any(|place| place.id == "sayt"), "{:?}", system.places().await);
    assert!(system.setups().await.iter().any(|setup| setup.id == "claude-code"));
    assert!(system.list().await.iter().all(|seen| !seen.outside), "our works are begun here");
    let heard = Arc::new(Mutex::new(Vec::new()));
    {
        let heard = heard.clone();
        system.listen(Arc::new(move |_: &str, event: SystemEvent| heard.lock().unwrap().push(event)));
    }
    bench.mode("ok");
    let id = bench.start("Сделай страницу");
    assert_eq!(bench.ended(&id).await, FolderEnd::Ready);
    let told = |heard: &Mutex<Vec<SystemEvent>>| {
        heard
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(event, SystemEvent::Ended { work, state: SystemState::Ready, .. } if *work == id))
    };
    bench.until("the end told as the contract's event", |_| told(&heard)).await;
    let outcome = system.result(&id).await.unwrap();
    assert!(outcome.artifacts.iter().any(|artifact| artifact.at.starts_with("duowave/")), "{outcome:?}");
}
