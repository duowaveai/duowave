//! The tests of `voice_lab/service.rs` that need the whole gateway (`App`): its crate does not have it, the program
//! does.
//!
//! A gateway with the voice lab on a Mac that only pretends: its memory as a test sets it, an engine «installed» whose
//! Python is `duowave-test-stub` speaking as clone-server/imprint.py does (synthetic audio only, in a temporary
//! folder; it runs on Windows too), and a voice server already up that answers /health and speaks a tone.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::routing::get;
use axum::{Json, Router};
use futures::future::BoxFuture;
use serde_json::json;

use crate::app::App;
use crate::config::{Config, Env};
use crate::speech::audio::pcm::encode_wav;
use crate::voice_lab::platform::{CommandResult, Probe};
use crate::voice_lab::voice_store::testing::{silence, voice};

/// What the Mac's memory looks like to the sampling: 16 GB free and calm, until a test says otherwise.
pub struct Memory {
    pub free_mb: AtomicU64,
    pub pressure_level: AtomicU64,
    /// Free room on its disk, MB: 80 GB until a test says otherwise.
    pub free_disk_mb: AtomicU64,
}

/// An Apple Silicon Mac, as detection and the memory sampling would see it.
pub struct FakeMac {
    pub memory: Arc<Memory>,
}

impl Probe for FakeMac {
    fn platform(&self) -> String {
        "darwin".into()
    }
    fn arch(&self) -> String {
        "arm64".into()
    }
    fn env(&self, _name: &str) -> Option<String> {
        None
    }
    fn run(&self, command: &str, args: &[&str], _timeout_ms: u64) -> BoxFuture<'static, CommandResult> {
        let answer = |stdout: String| CommandResult { code: Some(0), stdout };
        let result = if command == "sysctl" && args.get(1) == Some(&"machdep.cpu.brand_string") {
            answer("Apple M4 Max\n".into())
        } else if command == "sysctl" {
            // By name, as `sysctl DARWIN_SYSCTL` answers: the free pages, the pressure level, no swap-ins yet.
            let free = self.memory.free_mb.load(Ordering::SeqCst) * 64;
            let level = self.memory.pressure_level.load(Ordering::SeqCst);
            answer(format!(
                "hw.pagesize: 16384\nvm.page_free_count: {free}\nkern.memorystatus_vm_pressure_level: {level}\n\
                 vm.compressor.swapper.swapins_total: 0\n"
            ))
        } else if command == "uv" || command == "git" {
            answer("ok".into())
        } else {
            CommandResult { code: None, stdout: String::new() }
        };
        Box::pin(async move { result })
    }
    fn exists(&self, _path: &Path) -> bool {
        false
    }
    fn read(&self, _path: &Path) -> Option<String> {
        None
    }
    fn total_mb(&self) -> u64 {
        36_864
    }
    fn memory_bytes(&self) -> (u64, u64) {
        (16_384 * 1_048_576, 36_864 * 1_048_576)
    }
    fn cpus(&self) -> (usize, Option<String>) {
        (14, None)
    }
    fn free_disk_mb(&self, _path: &Path) -> u64 {
        self.memory.free_disk_mb.load(Ordering::SeqCst)
    }
    fn home(&self) -> PathBuf {
        PathBuf::from("/Users/owner")
    }
}

/// 20 s of reading: phrases and pauses, 16 kHz.
pub fn spoken_take() -> Vec<u8> {
    let mut pcm = Vec::new();
    for _ in 0..8 {
        pcm.extend(voice(2000, -20.0));
        pcm.extend(silence(500, -60.0));
    }
    encode_wav(&pcm, 16_000)
}

pub struct Lab {
    /// Kept for the lab's life: the folder goes with it.
    _dir: tempfile::TempDir,
    pub app: Arc<App>,
    pub memory: Arc<Memory>,
    /// The voice server's base URL, with /v1.
    pub clone_url: String,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Lab {
    fn drop(&mut self) {
        self.server.abort();
    }
}

/// The voice's server: already up (the gateway takes it over), speaking WAV.
async fn clone_server() -> (String, tokio::task::JoinHandle<()>) {
    let health = json!({
        "server": "clone-server",
        "engine": "omnivoice",
        "device": "mps",
        "pid": 999_999,
        "instance": "stub",
        "voices": [],
        "memoryMb": 2_600,
        "peakMb": 3_000,
    });
    let wav = encode_wav(&voice(300, -20.0), 24_000);
    let router = Router::new().route("/health", get(move || async move { Json(health) })).fallback(move || {
        let wav = wav.clone();
        async move { ([(axum::http::header::CONTENT_TYPE, "audio/wav")], wav) }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    (url, server)
}

pub async fn lab() -> Lab {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // The engine's Python: the stub as imprint.py (src/bin/test_stub.rs, role `python`).
    crate::utils::test_stub::put_at(&root.join("runtime/omnivoice/env/bin/python"));
    std::fs::write(
        root.join("runtime/omnivoice/ready.json"),
        r#"{"engine":"omnivoice","device":"mps","torch":"2.8.0","source":"bench","installedAt":"2026-11-06T00:00:00Z"}"#,
    )
    .unwrap();
    std::fs::create_dir_all(root.join("clone-server")).unwrap();
    let (clone_url, server) = clone_server().await;
    // Nothing answers for the other engine: a closed port of this computer.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let text = |sub: &str| root.join(sub).to_string_lossy().into_owned();
    let env = Env::from_pairs([
        ("DATA_DIR", text("data")),
        ("STATE_DIR", String::new()),
        ("VOICE_LAB_DIR", text("voice-lab")),
        ("VOICE_LAB_RUNTIME_DIR", text("runtime")),
        ("VOICE_LAB_BENCH_DIR", String::new()),
        ("VOICE_LAB_SERVER_URLS", format!("omnivoice={clone_url};cosyvoice3=http://127.0.0.1:{closed}/v1")),
        // Our voice server for Silero and Piper (the env's route); recognition on this computer.
        ("TTS_OWN_URL", clone_url.clone()),
        ("ASR_PROVIDER", "openai".into()),
        ("ASR_BASE_URL", "http://127.0.0.1:9/v1".into()),
    ]);
    let mut config = Config::load(&env).expect("the test's configuration");
    config.voice_lab.server_dir = root.join("clone-server");
    let app = App::build(config).unwrap();
    let memory = Arc::new(Memory {
        free_mb: AtomicU64::new(16_384),
        pressure_level: AtomicU64::new(1),
        free_disk_mb: AtomicU64::new(80_000),
    });
    app.pc.voice_lab.use_probe(Arc::new(FakeMac { memory: memory.clone() }));
    app.pc.voice_lab.start();
    Lab { _dir: dir, app, memory, clone_url, server }
}
