//! The tests of `voice_lab/service.rs` that need the whole gateway (`App`): its crate does not have it, the program
//! does.

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use serde_json::json;

use crate::computer::tests::voice_lab_testing::lab;
use crate::config::{Config, Env};
use crate::core::events::EventBus;
use crate::voice_lab::clone_engines::EngineId;
use crate::voice_lab::jobs::JobRunState;
use crate::voice_lab::service::*;
use crate::voice_lab::voice_store::testing::reading;

async fn imprinted(lab: &Arc<VoiceLabService>) -> String {
    lab.add_take(reading(20.0, -20.0, -60.0), Some(1)).await.unwrap();
    lab.imprint("omnivoice", Some(&json!("Мой голос")), None, false).await.unwrap();
    for _ in 0..100 {
        if let Some(job) = lab.jobs.state().filter(|job| job.state != JobRunState::Running) {
            assert_eq!(job.state, JobRunState::Done, "{job:?}");
            return job.result.and_then(|result| result.get("voiceId").cloned()).unwrap().as_str().unwrap().into();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the imprint did not finish");
}

/// Clean-path check 18.11, item 38: recognition inside this program (the desktop app) — imprint.py hears the words
/// through this gateway's loopback route with its key, not at ASR_BASE_URL where nobody listens; before the gateway
/// listens there is no such route yet; a recognition server elsewhere is refused as before.
#[tokio::test]
async fn an_imprint_hears_through_the_gateway_when_recognition_runs_inside_it() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().to_string_lossy().into_owned();
    let env = Env::from_pairs([
        ("STATE_DIR", String::new()),
        ("DATA_DIR", data.clone()),
        ("ASR_BASE_URL", "http://127.0.0.1:8090/v1".into()),
    ]);
    let mut config = Config::for_tests(&env);
    config.speech.asr.local =
        Some(crate::config::LocalAsrConfig { dir: dir.path().join("speech"), mirror: None, gpu: false });
    let lab = VoiceLabService::new(&config, EventBus::new());
    assert_eq!(
        lab.imprint_asr_url().unwrap_err().status,
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        "not listening yet"
    );
    lab.set_local_port(7780);
    let url = lab.imprint_asr_url().unwrap();
    let key = url.strip_prefix("http://127.0.0.1:7780/api/v1/voice-lab/asr/").and_then(|rest| rest.strip_suffix("/v1"));
    assert!(key.is_some_and(|key| lab.asr_key_is(key) && key.len() >= 32), "{url}");
    assert!(!lab.asr_key_is("") && !lab.asr_key_is("guess"));

    config.speech.asr.local = None;
    config.speech.asr.base_url = "https://asr.example.com/v1".into();
    let elsewhere = VoiceLabService::new(&config, EventBus::new());
    assert_eq!(
        elsewhere.imprint_asr_url().unwrap_err().status,
        axum::http::StatusCode::BAD_REQUEST,
        "not into the cloud"
    );
    config.speech.asr.base_url = "http://127.0.0.1:8090/v1".into();
    let server = VoiceLabService::new(&config, EventBus::new());
    assert_eq!(server.imprint_asr_url().unwrap(), "http://127.0.0.1:8090/v1", "a recognition server of this computer");
}

#[tokio::test]
async fn gives_the_speech_module_a_route_to_the_voices_server_and_its_server_state_for_the_cards() {
    let lab = lab().await;
    let voice_lab = &lab.app.pc.voice_lab;
    let id = imprinted(voice_lab).await;
    let cards = voice_lab.voices_for_cards().await;
    assert_eq!(cards.len(), 1);
    assert!(cards[0].serving && cards[0].ready);
    assert_eq!(cards[0].voice.samples.len(), 1);
    let prepared = voice_lab.prepare(&id, false).await.unwrap();
    assert_eq!((prepared.engine, prepared.name.as_str()), (EngineId::Omnivoice, "Мой голос"));
    assert_eq!(prepared.route.base_url(), lab.clone_url);
    assert_eq!((prepared.route.max_concurrent(), prepared.route.timeout_ms()), (Some(1), 30_000));
    let again = voice_lab.prepare(&id, false).await.unwrap();
    assert!(Arc::ptr_eq(&prepared.route, &again.route), "one route an engine: one queue");
    assert!(voice_lab.serving(&id).await);
    let restored = voice_lab.restore(&id).expect("the voice is there");
    assert!(Arc::ptr_eq(&restored.route, &prepared.route));
    assert!(voice_lab.restore("v-gone").is_none());
    assert_eq!(voice_lab.prepare("v-gone", false).await.unwrap_err().status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn knows_the_chosen_voice_from_the_speech_module_and_keeps_it() {
    let lab = lab().await;
    let voice_lab = &lab.app.pc.voice_lab;
    let id = imprinted(voice_lab).await;
    assert!(!voice_lab.view().await.voices[0].chosen, "the env's pair is Silero's, not an own voice");
    // The gateway's own voice service, linked at the build (src/links): its choice is the lab's.
    lab.app.voice.use_own("omnivoice", &id, None, None).unwrap();
    assert!(voice_lab.view().await.voices[0].chosen);
    let refused = voice_lab.remove(&id, None).unwrap_err();
    assert_eq!(refused.status, StatusCode::CONFLICT);
    assert_eq!(refused.message, "Этот голос выбран — сначала выберите другой.");
}

#[tokio::test]
async fn starts_a_server_a_synthesis_found_gone_again_at_most_every_30_seconds() {
    let lab = lab().await;
    let voice_lab = &lab.app.pc.voice_lab;
    let id = imprinted(voice_lab).await;
    let route = voice_lab.prepare(&id, false).await.unwrap().route;
    route.unreachable();
    let first = *locked(&voice_lab.restarted_at).get(&EngineId::Omnivoice).expect("revived");
    route.unreachable();
    assert_eq!(locked(&voice_lab.restarted_at).get(&EngineId::Omnivoice), Some(&first), "not again so soon");
}

#[tokio::test]
async fn says_why_an_imprint_cannot_start() {
    let lab = lab().await;
    let voice_lab = &lab.app.pc.voice_lab;
    let no_takes = voice_lab.imprint("omnivoice", None, None, false).await.unwrap_err();
    assert_eq!(no_takes.message, "Нужна хотя бы одна запись от 15 секунд — лучше три по 40.");
    assert_eq!(voice_lab.imprint("chatterbox", None, None, false).await.unwrap_err().message, "нет модели chatterbox");
    let missing = voice_lab.imprint("cosyvoice3", None, None, false).await.unwrap_err();
    assert_eq!(missing.message, "Сначала установите CosyVoice 3.");
    let installed = voice_lab.install("omnivoice", None).await.unwrap_err();
    assert_eq!((installed.status, installed.message.as_str()), (StatusCode::CONFLICT, "OmniVoice уже установлена."));
}

// Unix only: the refusal is made with Unix rights (a folder 0o500); Windows refuses by a file held open, below.
#[cfg(unix)]
#[tokio::test]
async fn answers_500_for_a_take_that_could_not_be_removed_and_keeps_it_listed() {
    use std::os::unix::fs::PermissionsExt;
    // SAFETY: geteuid only reads the process's user id.
    if unsafe { libc::geteuid() } == 0 {
        return; // root removes from a read-only folder all the same
    }
    let lab = lab().await;
    let voice_lab = &lab.app.pc.voice_lab;
    voice_lab.add_take(reading(16.0, -20.0, -60.0), None).await.unwrap();
    let draft = voice_lab.store.draft_dir.clone();
    let writable = |mode: u32| std::fs::set_permissions(&draft, std::fs::Permissions::from_mode(mode));
    writable(0o500).unwrap();
    let refused = voice_lab.remove_take(Some(1));
    writable(0o700).unwrap();
    assert_eq!(refused.unwrap_err().status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(voice_lab.view().await.draft.takes.len(), 1);
    voice_lab.remove_take(Some(1)).unwrap();
    assert_eq!(voice_lab.remove_take(Some(1)).unwrap_err().status, StatusCode::NOT_FOUND);
}

/// Windows: a take another program holds open (a player, the voice server) cannot go — os error 32: a 500, and
/// it stays listed; once let go, it goes.
#[cfg(windows)]
#[tokio::test]
async fn answers_500_for_a_take_held_open_and_keeps_it_listed() {
    use std::os::windows::fs::OpenOptionsExt;
    let lab = lab().await;
    let voice_lab = &lab.app.pc.voice_lab;
    voice_lab.add_take(reading(16.0, -20.0, -60.0), None).await.unwrap();
    let take = std::fs::read_dir(&voice_lab.store.draft_dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|ext| ext == "wav"))
        .expect("the take's file");
    // Shared for reading and writing, not for deleting: as Python's open() holds a file.
    let held = std::fs::OpenOptions::new().read(true).share_mode(1 | 2).open(&take).unwrap();
    assert_eq!(voice_lab.remove_take(Some(1)).unwrap_err().status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(voice_lab.view().await.draft.takes.len(), 1);
    drop(held);
    voice_lab.remove_take(Some(1)).unwrap();
    assert_eq!(voice_lab.remove_take(Some(1)).unwrap_err().status, StatusCode::NOT_FOUND);
}

#[test]
fn hands_the_engines_the_gateways_paths_and_proxies_never_its_keys() {
    use crate::config::Env;
    use crate::models::model_servers::server_env_of;
    let kept =
        ["PATH", "HOME", "LANG", "lc_all", "HTTPS_PROXY", "HF_HOME", "HF_TOKEN", "DATA_DIR", "CUDA_VISIBLE_DEVICES"];
    let dropped = ["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "STATION_SSH_PASSWORD", "HA_TOKEN", "PATHX", "LC_"];
    let env = Env::from_pairs(kept.iter().chain(dropped.iter()).map(|name| (*name, "1")));
    let mut passed: Vec<String> = server_env_of(&env).into_iter().map(|(name, _)| name).collect();
    passed.sort();
    let mut expected: Vec<String> = kept.iter().map(|name| name.to_string()).collect();
    expected.sort();
    assert_eq!(passed, expected);
    assert_eq!(child_env().get("PYTHONIOENCODING").map(String::as_str), Some("utf-8"));
}
