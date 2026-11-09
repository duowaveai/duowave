//! The Python model servers next to the gateway: what they say they loaded (/health), and how a new recognition
//! model or voice reaches them. A server that says in /health's `features` it can take one over HTTP is asked to —
//! the recognition server loads the model beside the serving one and swaps (POST /v1/models/load), the voice server
//! reads its folder again (POST /v1/models/rescan) — in Docker, on Linux, on Windows and on a Mac alike. One without
//! them (an older server) is restarted the way this Mac runs them, on macOS only: launchd services
//! (deploy/macos/install.sh: KeepAlive brings a stopped one back) or started by hand (the run script again,
//! detached). Elsewhere there is no restart from here, and it says so. The recognition model also goes to the file
//! the run script reads, so on a Mac the choice outlives the server's restart, launchd's and the Mac's.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use regex::Regex;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;

use super::model_files::safe_name;
use crate::config::Env;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ServerName {
    Asr,
    Tts,
}

impl ServerName {
    pub fn parse(value: &str) -> Option<ServerName> {
        match value {
            "asr" => Some(ServerName::Asr),
            "tts" => Some(ServerName::Tts),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ServerName::Asr => "asr",
            ServerName::Tts => "tts",
        }
    }

    fn script(self) -> &'static str {
        match self {
            ServerName::Asr => "run-asr.sh",
            ServerName::Tts => "run-tts.sh",
        }
    }

    fn process(self) -> &'static str {
        match self {
            ServerName::Asr => "asr-server/server.py",
            ServerName::Tts => "tts-server/server.py",
        }
    }

    fn launchd(self) -> &'static str {
        match self {
            ServerName::Asr => "app.duowave.asr",
            ServerName::Tts => "app.duowave.tts",
        }
    }
}

/// What deploy/macos/run-asr.sh reads for the model (GIGAAM_MODEL in the environment still wins).
pub const ASR_MODEL_FILE: &str = "asr-server/chosen-model";

/// A new recognition model may download for minutes first (Parakeet is ~2.4 GB); the panel waits as long.
const LOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// How often a load under way is looked at, and how long a silent server may stay silent meanwhile.
const LOAD_POLL: Duration = Duration::from_secs(1);
const LOAD_SILENCE: Duration = Duration::from_secs(60);
/// A rescan reads a folder and loads a new Silero file at most: seconds.
const RESCAN_TIMEOUT: Duration = Duration::from_secs(120);
/// /health for the panel's view: quick, or the server counts as down. Before a restart: patient, so a busy server
/// is not taken for an old one and restarted.
pub const HEALTH: Duration = Duration::from_millis(1500);
pub const HEALTH_PATIENT: Duration = Duration::from_millis(5000);
/// A model loaded once by a throwaway process before the Mac's restart.
const WARMUP_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TtsLoaded {
    /// The Silero file it loaded (which generation), if any.
    pub silero_file: Option<String>,
    /// The Piper voices it loaded, by their short names.
    pub piper: Vec<String>,
    /// The Piper voices by their files' stems, when the server names them (short names repeat across qualities).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub piper_files: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct AsrStatus {
    /// The model it serves; None while it has none.
    pub model: Option<String>,
    /// The model it is loading now, and why the last load failed.
    pub loading: Option<String>,
    pub error: Option<String>,
    /// What it can do beyond OpenAI's API: `load` — a model switch without a restart.
    pub features: Vec<String>,
}

impl AsrStatus {
    pub fn can(&self, feature: &str) -> bool {
        self.features.iter().any(|entry| entry == feature)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct TtsStatus {
    pub loaded: TtsLoaded,
    /// `rescan` — a voice put into its folder speaks without a restart.
    pub features: Vec<String>,
}

impl TtsStatus {
    pub fn can(&self, feature: &str) -> bool {
        self.features.iter().any(|entry| entry == feature)
    }
}

/// A program run to its end: its exit code (None: ended by a signal) and the end of what it said on stderr.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finished {
    pub code: Option<i32>,
    pub stderr: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    /// It did not start.
    Start(String),
    /// It did not end in time and was killed.
    Timeout,
}

/// What starts programs for the Mac's restart: the system (a test records instead, and starts nothing).
pub trait Launcher: Send + Sync {
    /// Runs a program to its end (pkill, a model's warm-up); past `timeout` it is killed.
    fn run(
        &self,
        program: PathBuf,
        args: Vec<String>,
        timeout: Option<Duration>,
    ) -> BoxFuture<'static, Result<Finished, RunError>>;

    /// Starts a program detached from the gateway, its output nowhere (a run script again): it outlives the gateway.
    fn start_detached(&self, program: PathBuf, args: Vec<String>, env: Vec<(String, String)>) -> Result<(), String>;
}

/// The system's way.
pub struct SystemLauncher;

impl Launcher for SystemLauncher {
    fn run(
        &self,
        program: PathBuf,
        args: Vec<String>,
        timeout: Option<Duration>,
    ) -> BoxFuture<'static, Result<Finished, RunError>> {
        Box::pin(async move {
            let mut child = tokio::process::Command::new(&program)
                .args(&args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .map_err(|error| RunError::Start(format!("{}: {error}", program.display())))?;
            let mut stderr = child.stderr.take();
            let finish = async {
                // The end of what it said is enough to say why it failed.
                let tail = async {
                    let mut text = Vec::new();
                    if let Some(stderr) = stderr.as_mut() {
                        let mut buffer = [0u8; 4096];
                        while let Ok(read) = stderr.read(&mut buffer).await {
                            if read == 0 {
                                break;
                            }
                            text.extend_from_slice(&buffer[..read]);
                            if text.len() > 8000 {
                                text.drain(..text.len() - 4000);
                            }
                        }
                    }
                    let text = String::from_utf8_lossy(&text).into_owned();
                    let skip = text.chars().count().saturating_sub(2000);
                    text.chars().skip(skip).collect::<String>()
                };
                let (status, stderr) = tokio::join!(child.wait(), tail);
                Finished { code: status.ok().and_then(|status| status.code()), stderr }
            };
            match timeout {
                Some(timeout) => tokio::time::timeout(timeout, finish).await.map_err(|_| RunError::Timeout),
                None => Ok(finish.await),
            }
        })
    }

    fn start_detached(&self, program: PathBuf, args: Vec<String>, env: Vec<(String, String)>) -> Result<(), String> {
        let mut command = tokio::process::Command::new(&program);
        command.args(&args).env_clear().envs(env).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        // Its own process group: a Ctrl+C to the gateway does not reach it.
        #[cfg(unix)]
        command.process_group(0);
        // Dropped, the child runs on; tokio reaps it when it ends.
        command.spawn().map(drop).map_err(|error| format!("{}: {error}", program.display()))
    }
}

#[derive(Clone, Default)]
pub struct ModelServersOptions {
    /// What the servers may ask for: the gateway's own ASR_API_KEY and TTS_OWN_API_KEY.
    pub asr_api_key: Option<String>,
    pub tts_api_key: Option<String>,
    /// The macOS run scripts' folder: MAC_DEPLOY_DIR when set, for a gateway that runs from elsewhere than the
    /// working copy — the desktop app's bundle, whose working folder has no deploy/macos — else the working
    /// folder's own.
    pub mac_deploy: Option<PathBuf>,
    /// For tests: the OS (as std::env::consts::OS names it), launchd's folder, the recognition server's Python,
    /// how often a load under way is looked at, what starts programs.
    pub platform: Option<String>,
    pub launch_daemons: Option<PathBuf>,
    pub asr_python: Option<PathBuf>,
    pub load_poll: Option<Duration>,
    pub launcher: Option<Arc<dyn Launcher>>,
}

/// A base URL without its trailing slashes and `/v1`: where /health is.
fn root(url: &str) -> String {
    let url = url.trim_end_matches('/');
    url.strip_suffix("/v1").unwrap_or(url).to_string()
}

fn features(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items.iter().filter_map(|item| item.as_str().map(str::to_string)).collect(),
        _ => Vec::new(),
    }
}

fn strings(value: Option<&Value>) -> Option<Vec<String>> {
    match value {
        Some(Value::Array(items)) => Some(items.iter().filter_map(|item| item.as_str().map(str::to_string)).collect()),
        _ => None,
    }
}

fn tts_loaded_of(loaded: Option<&Value>) -> TtsLoaded {
    let field = |name: &str| loaded.and_then(|loaded| loaded.get(name));
    TtsLoaded {
        silero_file: field("silero").and_then(Value::as_str).map(str::to_string),
        piper: strings(field("piper")).unwrap_or_default(),
        piper_files: strings(field("piperFiles")),
    }
}

/// A JSON value as JavaScript's `if (body)` reads it.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0),
        Value::String(text) => !text.is_empty(),
        _ => true,
    }
}

/// The first `n` characters.
fn clip(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

pub struct ModelServers {
    asr_root: Option<String>,
    tts_root: Option<String>,
    asr_model_file: PathBuf,
    asr_api_key: Option<String>,
    tts_api_key: Option<String>,
    platform: String,
    mac_deploy: PathBuf,
    launch_daemons: PathBuf,
    asr_python: PathBuf,
    load_poll: Duration,
    launcher: Arc<dyn Launcher>,
}

impl ModelServers {
    pub fn new(
        asr_base_url: Option<&str>,
        tts_base_url: Option<&str>,
        asr_model_file: Option<PathBuf>,
        options: ModelServersOptions,
    ) -> ModelServers {
        let absolute = |path: PathBuf| std::path::absolute(&path).unwrap_or(path);
        ModelServers {
            asr_root: asr_base_url.map(root),
            tts_root: tts_base_url.map(root),
            asr_model_file: asr_model_file.unwrap_or_else(|| absolute(PathBuf::from(ASR_MODEL_FILE))),
            asr_api_key: options.asr_api_key,
            tts_api_key: options.tts_api_key,
            platform: options.platform.unwrap_or_else(|| std::env::consts::OS.to_string()),
            mac_deploy: options.mac_deploy.unwrap_or_else(|| absolute(PathBuf::from("deploy/macos"))),
            launch_daemons: options.launch_daemons.unwrap_or_else(|| PathBuf::from("/Library/LaunchDaemons")),
            asr_python: options.asr_python.unwrap_or_else(|| absolute(PathBuf::from("asr-server/.venv/bin/python"))),
            load_poll: options.load_poll.unwrap_or(LOAD_POLL),
            launcher: options.launcher.unwrap_or_else(|| Arc::new(SystemLauncher)),
        }
    }

    /// Where the recognition server answers (its base URL without `/v1`); None: no recognition of ours.
    pub fn asr_root(&self) -> Option<&str> {
        self.asr_root.as_deref()
    }

    /// Where our voice server answers; None: TTS_OWN_URL is not set.
    pub fn tts_root(&self) -> Option<&str> {
        self.tts_root.as_deref()
    }

    /// The Mac's restart is here: macOS and its run scripts. Nowhere else — no launchd, and `nohup` on Windows is
    /// not even there to spawn.
    pub fn mac_restart(&self) -> bool {
        self.platform == "macos" && self.mac_deploy.join(ServerName::Asr.script()).exists()
    }

    /// Whether a new model or voice can be put to work from here: by a server over HTTP, or by the Mac's restart.
    pub fn can_restart(&self, asr: Option<&AsrStatus>, tts: Option<&TtsStatus>) -> bool {
        self.mac_restart() || asr.is_some_and(|asr| asr.can("load")) || tts.is_some_and(|tts| tts.can("rescan"))
    }

    /// The recognition server's word about its model and abilities; None when it is down or not ours.
    pub async fn asr_status(&self, timeout: Duration) -> Option<AsrStatus> {
        let body = self.ask(self.asr_root.as_deref(), timeout).await?;
        let text = |name: &str| body.get(name).and_then(Value::as_str).map(str::to_string);
        Some(AsrStatus {
            model: text("model"),
            loading: text("loading"),
            error: text("error"),
            features: features(body.get("features")),
        })
    }

    /// The recognition server's word about its model; None when it is down.
    pub async fn asr_loaded(&self) -> Option<String> {
        self.asr_status(HEALTH).await.and_then(|status| status.model)
    }

    /// What the voice server really loaded and what it can do; None when it is down or not ours.
    pub async fn tts_status(&self, timeout: Duration) -> Option<TtsStatus> {
        let body = self.ask(self.tts_root.as_deref(), timeout).await?;
        Some(TtsStatus { loaded: tts_loaded_of(body.get("loaded")), features: features(body.get("features")) })
    }

    /// What the voice server really loaded; None when it is down or not ours.
    pub async fn tts_loaded(&self) -> Option<TtsLoaded> {
        self.tts_status(HEALTH).await.map(|status| status.loaded)
    }

    /// The voice server reads its folder again: a voice downloaded or deleted here speaks or is gone at once. What
    /// it has loaded then; None when it cannot (an older server, another one, none). An error when it tried and
    /// failed.
    pub async fn rescan_tts(&self) -> Result<Option<TtsLoaded>, String> {
        let status = self.tts_status(HEALTH_PATIENT).await;
        let Some(root) = &self.tts_root else {
            return Ok(None);
        };
        if !status.is_some_and(|status| status.can("rescan")) {
            return Ok(None);
        }
        let body = self
            .post(
                &format!("{root}/v1/models/rescan"),
                None,
                self.tts_api_key.as_deref(),
                RESCAN_TIMEOUT,
                "голосовой сервер не перечитал голоса",
            )
            .await?;
        Ok(Some(tts_loaded_of(body.get("loaded"))))
    }

    /// The recognition model for the server's next start.
    pub fn choose_asr(&self, model: &str) -> Result<(), String> {
        if !safe_name(model) {
            return Err(format!("not a model name: {model}"));
        }
        std::fs::write(&self.asr_model_file, format!("{model}\n"))
            .map_err(|error| format!("{}: {error}", self.asr_model_file.display()))
    }

    /// The model the run script will start with; None when the file is missing or empty.
    pub fn asr_on_disk(&self) -> Option<String> {
        let text = std::fs::read_to_string(&self.asr_model_file).ok()?;
        let first = text.split('\n').next().unwrap_or("").trim();
        (!first.is_empty()).then(|| first.to_string())
    }

    /// Puts the chosen model in the run script's file when the file says something else: a choice from before the
    /// file existed, or a checkout without it, would otherwise let launchd or the deploy scripts start the default
    /// model. Whether it wrote.
    pub fn sync_asr(&self, model: &str) -> Result<bool, String> {
        if self.asr_on_disk().as_deref() == Some(model) {
            return Ok(false);
        }
        self.choose_asr(model)?;
        Ok(true)
    }

    /// Puts a new recognition model or new voices to work. A server that can do it over HTTP does it with no
    /// restart, and a model that cannot load leaves the serving one at work. Otherwise, on a Mac only, the server is
    /// restarted — a recognition model loaded once by a throwaway process first, so one that cannot load (not
    /// cached, its storage down) leaves the running server alone instead of taking the household's recognition with
    /// it.
    pub async fn restart(&self, which: ServerName, asr_model: Option<&str>) -> Result<(), String> {
        if which == ServerName::Asr
            && self.asr_root.is_some()
            && self.asr_status(HEALTH_PATIENT).await.is_some_and(|status| status.can("load"))
        {
            // Nothing chosen: nothing to put to work.
            let Some(model) = asr_model else {
                return Ok(());
            };
            self.load_asr(model).await?;
            // The Mac's run script reads its own file: the server's next start keeps the model too.
            if self.mac_restart() {
                self.choose_asr(model)?;
            }
            return Ok(());
        }
        if which == ServerName::Tts && self.rescan_tts().await?.is_some() {
            return Ok(());
        }
        if !self.mac_restart() {
            return Err(match which {
                ServerName::Asr => "Сервер распознавания не отвечает или не умеет менять модель на ходу, а перезапуск отсюда есть только на macOS — перезапустите его сами.",
                ServerName::Tts => "Голосовой сервер не отвечает или не умеет перечитывать голоса на ходу, а перезапуск отсюда есть только на macOS — перезапустите его сами.",
            }
            .into());
        }
        let script = self.mac_deploy.join(which.script());
        if !script.exists() {
            return Err(format!("нет {} — этот сервер здесь не перезапустить", which.script()));
        }
        if let (ServerName::Asr, Some(model)) = (which, asr_model) {
            self.warmup_asr(model).await?;
            // The run script takes the model from the file: the restarted server starts with the choice.
            self.choose_asr(model)?;
        }
        let managed = self.launch_daemons.join(format!("{}.plist", which.launchd())).exists();
        let _ = self.launcher.run("pkill".into(), vec!["-f".into(), which.process().into()], None).await;
        // launchd brings its service back by itself (KeepAlive); a server started by hand is started again.
        if !managed {
            tokio::time::sleep(Duration::from_secs(1)).await;
            self.launcher.start_detached("nohup".into(), vec![script.to_string_lossy().into_owned()], server_env())?;
        }
        Ok(())
    }

    /// The recognition server loads the model in the background (a download may take longer than an HTTP answer is
    /// waited for) and /health says when it is over: the model serves, or the error says why not.
    async fn load_asr(&self, model: &str) -> Result<(), String> {
        let what = format!("модель {model} не загрузилась");
        let root = self.asr_root.as_deref().unwrap_or_default();
        self.post(
            &format!("{root}/v1/models/load"),
            Some(json!({"model": model, "wait": false})),
            self.asr_api_key.as_deref(),
            Duration::from_secs(30),
            &what,
        )
        .await?;
        let deadline = Instant::now() + LOAD_TIMEOUT;
        let mut heard = Instant::now();
        loop {
            let status = self.asr_status(HEALTH_PATIENT).await;
            if status.is_some() {
                heard = Instant::now();
            } else if heard.elapsed() > LOAD_SILENCE {
                return Err(format!("{what}: сервер распознавания перестал отвечать"));
            }
            if let Some(status) = status.filter(|status| status.loading.as_deref() != Some(model)) {
                if status.model.as_deref() == Some(model) {
                    return Ok(());
                }
                let why = status
                    .error
                    .map(|error| clip(&error, 300))
                    .unwrap_or_else(|| "причина — в журнале сервера распознавания".into());
                return Err(format!("{what}: {why}"));
            }
            if Instant::now() > deadline {
                return Err(format!(
                    "{what} за {} минут — сервер ещё может её догрузить, загляните позже",
                    LOAD_TIMEOUT.as_secs() / 60
                ));
            }
            tokio::time::sleep(self.load_poll).await;
        }
    }

    async fn warmup_asr(&self, model: &str) -> Result<(), String> {
        if !safe_name(model) {
            return Err(format!("not a model name: {model}"));
        }
        // Not this deploy: let the server try itself.
        if !self.asr_python.exists() {
            return Ok(());
        }
        let script = if model.starts_with("whisper-") {
            r#"import sys; from faster_whisper import WhisperModel; WhisperModel(sys.argv[1][8:], device="cpu", compute_type="int8")"#
        } else if model.starts_with("parakeet-") {
            r#"from transformers import ParakeetForTDT; ParakeetForTDT.from_pretrained("nvidia/parakeet-tdt-0.6b-v3")"#
        } else {
            "import sys, gigaam; gigaam.load_model(sys.argv[1])"
        };
        let args = vec!["-c".to_string(), script.to_string(), model.to_string()];
        match self.launcher.run(self.asr_python.clone(), args, Some(WARMUP_TIMEOUT)).await {
            Ok(Finished { code: Some(0), .. }) => Ok(()),
            Ok(Finished { stderr, .. }) => {
                let last = stderr.trim().split('\n').next_back().unwrap_or("");
                Err(format!("модель {model} не загрузилась: {}", clip(last, 160)))
            }
            Err(RunError::Timeout) => Err("модель не загрузилась за 5 минут — склад моделей недоступен?".into()),
            Err(RunError::Start(error)) => Err(error),
        }
    }

    async fn ask(&self, base: Option<&str>, timeout: Duration) -> Option<Value> {
        let base = base?;
        let response = crate::utils::http::client().get(format!("{base}/health")).timeout(timeout).send().await.ok()?;
        if !response.status().is_success() {
            return None;
        }
        response.json::<Value>().await.ok().filter(truthy)
    }

    /// A POST to one of our servers; its own words about a failure, after `what`.
    async fn post(
        &self,
        url: &str,
        body: Option<Value>,
        api_key: Option<&str>,
        timeout: Duration,
        what: &str,
    ) -> Result<Value, String> {
        let failed = |error: reqwest::Error| {
            if error.is_timeout() {
                format!("{what}: нет ответа за {} с", (timeout.as_millis() as f64 / 1000.0).round())
            } else {
                format!("{what}: {error}")
            }
        };
        let mut request = crate::utils::http::client().post(url).timeout(timeout);
        if let Some(body) = &body {
            request = request.json(body);
        }
        if let Some(key) = api_key {
            request = request.bearer_auth(key);
        }
        let response = request.send().await.map_err(failed)?;
        let status = response.status();
        let text = response.text().await.map_err(failed)?;
        let parsed: Option<Value> = if text.is_empty() { Some(json!({})) } else { serde_json::from_str(&text).ok() };
        if status.is_success() {
            return Ok(parsed.unwrap_or_else(|| json!({})));
        }
        // Ours say {"error": {"message": …}} (recognition, as OpenAI) or {"error": "…"} (voice).
        let message = match parsed.as_ref().and_then(|parsed| parsed.get("error")) {
            Some(Value::String(message)) => message.clone(),
            Some(error) => error.get("message").and_then(Value::as_str).unwrap_or("").to_string(),
            None => String::new(),
        };
        let detail = if status == reqwest::StatusCode::CONFLICT {
            "сервер уже загружает другую модель — дождитесь её".to_string()
        } else if !message.is_empty() {
            clip(&message, 300)
        } else {
            format!("HTTP {}", status.as_u16())
        };
        Err(format!("{what}: {detail}"))
    }
}

/// What a model server or an engine's installer needs of the gateway's environment — not the gateway's keys to the
/// LLM and the bots. Proxies, certificates and uv's settings (UV_INDEX_URL, UV_CACHE_DIR) let downloads go the way
/// the house's network needs; Windows needs SYSTEMROOT and its folders for Python to start at all; DATA_DIR lets the
/// voice server find the voices where the gateway downloads them ($DATA_DIR/voices). Case-blind: on Windows PATH is
/// often «Path». The environment is the gateway's own: the process's, with the .env files under it.
pub fn server_env() -> Vec<(String, String)> {
    server_env_of(&Env::from_process())
}

pub fn server_env_of(env: &Env) -> Vec<(String, String)> {
    static KEEP: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)^(PATH|PATHEXT|HOME|USER|LOGNAME|SHELL|LANG|TMPDIR|TEMP|TMP|LC_\w+|SYSTEMROOT|WINDIR|COMSPEC|APPDATA|LOCALAPPDATA|USERPROFILE|HOMEDRIVE|HOMEPATH|PROGRAMDATA|PROGRAMFILES|DATA_DIR|(HTTPS?|ALL|NO)_PROXY|SSL_CERT_(FILE|DIR)|REQUESTS_CA_BUNDLE|(GIGAAM|ASR|TTS|HF|HUGGINGFACE|TORCH|PYTORCH|OMP|MKL|CUDA|UV)_\w+)$",
        )
        .expect("the environment filter compiles")
    });
    let mut kept: Vec<(String, String)> = env
        .vars()
        .filter(|(name, _)| KEEP.is_match(name))
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
    kept.sort();
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::Mutex;

    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::{get, post};
    use axum::Router;

    /// What a server was asked: the path, the body (None: there was none), the Authorization header.
    #[derive(Debug, Clone, PartialEq)]
    struct Posted {
        path: String,
        body: Option<Value>,
        auth: Option<String>,
    }

    type Health = Box<dyn FnMut() -> Option<Value> + Send>;
    type Answer = Box<dyn FnMut(&str, Option<&Value>) -> (u16, Value) + Send>;

    /// A model server as the stub plays it: /health from `health`, every POST recorded and answered by `answer`.
    struct Stub {
        url: String,
        posts: Arc<Mutex<Vec<Posted>>>,
        health: Arc<Mutex<Health>>,
        answer: Arc<Mutex<Answer>>,
    }

    impl Stub {
        fn health(&self, health: impl FnMut() -> Option<Value> + Send + 'static) {
            *self.health.lock().unwrap() = Box::new(health);
        }

        fn answer(&self, answer: impl FnMut(&str, Option<&Value>) -> (u16, Value) + Send + 'static) {
            *self.answer.lock().unwrap() = Box::new(answer);
        }

        fn posts(&self) -> Vec<Posted> {
            self.posts.lock().unwrap().clone()
        }
    }

    #[derive(Clone)]
    struct StubState {
        posts: Arc<Mutex<Vec<Posted>>>,
        health: Arc<Mutex<Health>>,
        answer: Arc<Mutex<Answer>>,
    }

    async fn stub() -> Stub {
        let state = StubState {
            posts: Arc::default(),
            health: Arc::new(Mutex::new(Box::new(|| Some(json!({"status": "ok"}))))),
            answer: Arc::new(Mutex::new(Box::new(|_, _| (404, json!({"error": "not found"}))))),
        };
        async fn health(State(state): State<StubState>) -> axum::response::Response {
            let body = (state.health.lock().unwrap())();
            match body {
                Some(body) => axum::Json(body).into_response(),
                None => (StatusCode::SERVICE_UNAVAILABLE, axum::Json(json!({}))).into_response(),
            }
        }
        async fn posted(
            State(state): State<StubState>,
            uri: axum::http::Uri,
            headers: HeaderMap,
            body: axum::body::Bytes,
        ) -> axum::response::Response {
            let body: Option<Value> = if body.is_empty() { None } else { serde_json::from_slice(&body).ok() };
            let auth = headers.get("authorization").and_then(|value| value.to_str().ok()).map(str::to_string);
            state.posts.lock().unwrap().push(Posted { path: uri.path().to_string(), body: body.clone(), auth });
            let (status, answer) = (state.answer.lock().unwrap())(uri.path(), body.as_ref());
            (StatusCode::from_u16(status).unwrap(), axum::Json(answer)).into_response()
        }
        let router = Router::new()
            .route("/health", get(health))
            .route("/v1/models/{action}", post(posted))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Stub { url: format!("http://{address}/v1"), posts: state.posts, health: state.health, answer: state.answer }
    }

    /// Records what the Mac's restart would start, and starts nothing.
    #[derive(Default)]
    struct Recorder {
        started: Mutex<Vec<String>>,
    }

    impl Recorder {
        fn started(&self) -> Vec<String> {
            self.started.lock().unwrap().clone()
        }
    }

    impl Launcher for Recorder {
        fn run(
            &self,
            program: PathBuf,
            args: Vec<String>,
            _timeout: Option<Duration>,
        ) -> BoxFuture<'static, Result<Finished, RunError>> {
            self.started.lock().unwrap().push(format!("{} {}", program.display(), args.join(" ")));
            Box::pin(async { Ok(Finished { code: Some(0), stderr: String::new() }) })
        }

        fn start_detached(
            &self,
            program: PathBuf,
            args: Vec<String>,
            _env: Vec<(String, String)>,
        ) -> Result<(), String> {
            self.started.lock().unwrap().push(format!("{} {}", program.display(), args.join(" ")));
            Ok(())
        }
    }

    // The recognition model for the run script.

    fn choice_servers(file: &Path) -> ModelServers {
        ModelServers::new(None, None, Some(file.to_path_buf()), ModelServersOptions::default())
    }

    #[test]
    fn notes_a_restored_choice_the_file_does_not_have_yet() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("chosen-model");
        let servers = choice_servers(&file);
        assert_eq!(servers.asr_on_disk(), None);
        assert!(servers.sync_asr("v3_ctc").unwrap());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "v3_ctc\n");
        assert_eq!(servers.asr_on_disk().as_deref(), Some("v3_ctc"));
    }

    #[test]
    fn leaves_the_file_alone_when_it_already_says_the_choice() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("chosen-model");
        std::fs::write(&file, "v3_ctc\n").unwrap();
        assert!(!choice_servers(&file).sync_asr("v3_ctc").unwrap());
    }

    #[test]
    fn replaces_another_model_the_default_of_an_old_run_among_them() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("chosen-model");
        std::fs::write(&file, "v3_e2e_rnnt\n").unwrap();
        let servers = choice_servers(&file);
        assert!(servers.sync_asr("whisper-large-v3-turbo").unwrap());
        assert_eq!(servers.asr_on_disk().as_deref(), Some("whisper-large-v3-turbo"));
    }

    #[test]
    fn never_writes_what_is_not_a_model_name() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("chosen-model");
        std::fs::write(&file, "v3_ctc\n").unwrap();
        let servers = choice_servers(&file);
        assert!(servers.sync_asr("../../evil").unwrap_err().contains("not a model name"));
        assert_eq!(servers.asr_on_disk().as_deref(), Some("v3_ctc"));
    }

    #[test]
    fn reads_an_empty_file_as_no_choice() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("chosen-model");
        std::fs::write(&file, "\n").unwrap();
        assert_eq!(choice_servers(&file).asr_on_disk(), None);
    }

    // A new model or voice put to work.

    struct Bench {
        root: tempfile::TempDir,
        file: PathBuf,
        mac_deploy: PathBuf,
        asr: Stub,
        tts: Stub,
        recorder: Arc<Recorder>,
    }

    impl Bench {
        async fn new() -> Bench {
            let root = tempfile::tempdir().unwrap();
            let file = root.path().join("chosen-model");
            // The run scripts are in every checkout: only the OS decides whether they are used.
            let mac_deploy = root.path().join("deploy/macos");
            std::fs::create_dir_all(&mac_deploy).unwrap();
            std::fs::write(mac_deploy.join("run-asr.sh"), "#!/bin/sh\n").unwrap();
            std::fs::write(mac_deploy.join("run-tts.sh"), "#!/bin/sh\n").unwrap();
            let (asr, tts) = tokio::join!(stub(), stub());
            Bench { file, mac_deploy, asr, tts, recorder: Arc::default(), root }
        }

        fn make(&self, platform: &str, options: ModelServersOptions) -> ModelServers {
            ModelServers::new(
                Some(&self.asr.url),
                Some(&self.tts.url),
                Some(self.file.clone()),
                ModelServersOptions {
                    platform: Some(platform.into()),
                    mac_deploy: Some(options.mac_deploy.clone().unwrap_or_else(|| self.mac_deploy.clone())),
                    launch_daemons: Some(self.root.path().join("LaunchDaemons")),
                    asr_python: Some(self.root.path().join("no-python")),
                    load_poll: Some(Duration::from_millis(10)),
                    launcher: Some(self.recorder.clone()),
                    ..options
                },
            )
        }
    }

    #[tokio::test]
    async fn restarts_the_macos_way_on_a_mac_only_elsewhere_the_run_scripts_sit_unused() {
        let bench = Bench::new().await;
        assert!(bench.make("macos", Default::default()).mac_restart());
        assert!(!bench.make("linux", Default::default()).mac_restart());
        assert!(!bench.make("windows", Default::default()).mac_restart());
        let elsewhere =
            ModelServersOptions { mac_deploy: Some(bench.root.path().join("elsewhere")), ..Default::default() };
        assert!(!bench.make("macos", elsewhere).mac_restart());
    }

    #[tokio::test]
    async fn reads_what_the_servers_can_do_from_their_health() {
        let bench = Bench::new().await;
        bench.asr.health(|| {
            Some(json!({"status": "ok", "model": "v3_e2e_rnnt", "loading": "v3_ctc", "error": null, "features": ["load"]}))
        });
        bench.tts.health(|| {
            Some(json!({
                "status": "ok",
                "loaded": {"silero": "v5_ru.pt", "piper": ["denis"], "piperFiles": ["ru_RU-denis-medium"]},
                "features": ["rescan"],
            }))
        });
        let servers = bench.make("linux", Default::default());
        let (asr, tts) = tokio::join!(servers.asr_status(HEALTH), servers.tts_status(HEALTH));
        let (asr, tts) = (asr.unwrap(), tts.unwrap());
        assert_eq!(
            asr,
            AsrStatus {
                model: Some("v3_e2e_rnnt".into()),
                loading: Some("v3_ctc".into()),
                error: None,
                features: vec!["load".into()]
            }
        );
        assert_eq!(
            tts,
            TtsStatus {
                loaded: TtsLoaded {
                    silero_file: Some("v5_ru.pt".into()),
                    piper: vec!["denis".into()],
                    piper_files: Some(vec!["ru_RU-denis-medium".into()]),
                },
                features: vec!["rescan".into()],
            }
        );
        assert!(servers.can_restart(Some(&asr), None));
        assert!(servers.can_restart(None, Some(&tts)));
        let deaf_asr = AsrStatus { features: vec![], ..asr };
        let deaf_tts = TtsStatus { features: vec![], ..tts };
        assert!(!servers.can_restart(Some(&deaf_asr), Some(&deaf_tts)));
        assert!(bench.make("macos", Default::default()).can_restart(None, None));
    }

    #[tokio::test]
    async fn loads_a_recognition_model_over_http_follows_it_in_health_and_keeps_the_run_script_file_on_a_mac() {
        let bench = Bench::new().await;
        let state = Arc::new(Mutex::new((None::<String>, "v3_e2e_rnnt".to_string(), 0)));
        let seen = state.clone();
        bench.asr.health(move || {
            let mut state = seen.lock().unwrap();
            // Two looks while it loads, then the model serves.
            if state.0.is_some() {
                state.2 += 1;
                if state.2 > 2 {
                    state.1 = state.0.take().unwrap();
                }
            }
            Some(json!({"status": "ok", "model": state.1, "loading": state.0, "error": null, "features": ["load"]}))
        });
        let loading = state.clone();
        bench.asr.answer(move |_, body| {
            let model = body.and_then(|body| body["model"].as_str()).unwrap_or("").to_string();
            loading.lock().unwrap().0 = Some(model.clone());
            (202, json!({"status": "loading", "model": model}))
        });
        let servers =
            bench.make("macos", ModelServersOptions { asr_api_key: Some("asr-key".into()), ..Default::default() });
        servers.restart(ServerName::Asr, Some("v3_ctc")).await.unwrap();
        assert_eq!(
            bench.asr.posts(),
            vec![Posted {
                path: "/v1/models/load".into(),
                body: Some(json!({"model": "v3_ctc", "wait": false})),
                auth: Some("Bearer asr-key".into()),
            }]
        );
        assert_eq!(state.lock().unwrap().1, "v3_ctc");
        assert_eq!(std::fs::read_to_string(&bench.file).unwrap(), "v3_ctc\n");
        assert_eq!(bench.recorder.started(), Vec::<String>::new());
    }

    #[tokio::test]
    async fn writes_no_run_script_file_off_a_mac() {
        let bench = Bench::new().await;
        bench.asr.health(|| Some(json!({"status": "ok", "model": "v3_ctc", "loading": null, "features": ["load"]})));
        bench.asr.answer(|_, _| (200, json!({"status": "ok", "model": "v3_ctc", "changed": false})));
        bench.make("linux", Default::default()).restart(ServerName::Asr, Some("v3_ctc")).await.unwrap();
        assert_eq!(bench.asr.posts().len(), 1);
        assert!(!bench.file.exists());
    }

    #[tokio::test]
    async fn says_why_a_model_did_not_load_and_leaves_the_serving_one_and_the_file_as_they_were() {
        let bench = Bench::new().await;
        std::fs::write(&bench.file, "v3_e2e_rnnt\n").unwrap();
        let loading = Arc::new(Mutex::new(None::<String>));
        let seen = loading.clone();
        bench.asr.health(move || {
            let mut loading = seen.lock().unwrap();
            let error = if loading.is_some() { Value::Null } else { json!("whisper-tiny: no network") };
            let now = json!({"status": "ok", "model": "v3_e2e_rnnt", "loading": *loading, "error": error, "features": ["load"]});
            *loading = None;
            Some(now)
        });
        let set = loading.clone();
        bench.asr.answer(move |_, body| {
            *set.lock().unwrap() = body.and_then(|body| body["model"].as_str()).map(str::to_string);
            (202, json!({"status": "loading"}))
        });
        let error =
            bench.make("macos", Default::default()).restart(ServerName::Asr, Some("whisper-tiny")).await.unwrap_err();
        assert!(error.contains("модель whisper-tiny не загрузилась: whisper-tiny: no network"), "{error}");
        assert_eq!(std::fs::read_to_string(&bench.file).unwrap(), "v3_e2e_rnnt\n");
        assert_eq!(bench.recorder.started(), Vec::<String>::new());
    }

    #[tokio::test]
    async fn passes_on_a_refusal_another_model_loading_a_name_the_server_does_not_know() {
        let bench = Bench::new().await;
        bench.asr.health(|| {
            Some(json!({"status": "ok", "model": "v3_e2e_rnnt", "loading": "v3_ctc", "features": ["load"]}))
        });
        bench.asr.answer(|_, _| (409, json!({"error": {"message": "v3_ctc is loading now"}})));
        let servers = bench.make("linux", Default::default());
        let error = servers.restart(ServerName::Asr, Some("v2_ctc")).await.unwrap_err();
        assert!(error.contains("сервер уже загружает другую модель"), "{error}");
        bench.asr.answer(|_, _| (400, json!({"error": {"message": "bad request: no GigaAM model v9"}})));
        let error = servers.restart(ServerName::Asr, Some("v9")).await.unwrap_err();
        assert!(error.contains("модель v9 не загрузилась: bad request: no GigaAM model v9"), "{error}");
    }

    #[tokio::test]
    async fn has_nothing_to_load_when_nothing_is_chosen() {
        let bench = Bench::new().await;
        bench.asr.health(|| Some(json!({"status": "ok", "model": "v3_e2e_rnnt", "features": ["load"]})));
        bench.make("linux", Default::default()).restart(ServerName::Asr, None).await.unwrap();
        assert_eq!(bench.asr.posts(), Vec::<Posted>::new());
    }

    #[tokio::test]
    async fn has_the_voice_server_read_its_folder_again_instead_of_a_restart() {
        let bench = Bench::new().await;
        bench.tts.health(|| {
            Some(json!({"status": "ok", "loaded": {"piper": [], "piperFiles": []}, "features": ["rescan"]}))
        });
        bench.tts.answer(|_, _| {
            (
                200,
                json!({
                    "status": "ok",
                    "loaded": {"piper": ["denis"], "piperFiles": ["ru_RU-denis-medium"]},
                    "added": ["ru_RU-denis-medium"],
                }),
            )
        });
        let servers =
            bench.make("linux", ModelServersOptions { tts_api_key: Some("tts-key".into()), ..Default::default() });
        assert_eq!(
            servers.rescan_tts().await.unwrap(),
            Some(TtsLoaded {
                silero_file: None,
                piper: vec!["denis".into()],
                piper_files: Some(vec!["ru_RU-denis-medium".into()]),
            })
        );
        servers.restart(ServerName::Tts, None).await.unwrap();
        let rescan = Posted { path: "/v1/models/rescan".into(), body: None, auth: Some("Bearer tts-key".into()) };
        assert_eq!(bench.tts.posts(), vec![rescan.clone(), rescan]);
        assert_eq!(bench.recorder.started(), Vec::<String>::new());
    }

    #[tokio::test]
    async fn asks_nothing_of_a_server_that_does_not_say_it_can_an_older_one_or_somebody_elses() {
        let bench = Bench::new().await;
        bench.tts.health(|| Some(json!({"status": "ok", "loaded": {"piper": ["denis"]}})));
        assert_eq!(bench.make("linux", Default::default()).rescan_tts().await.unwrap(), None);
        assert_eq!(bench.tts.posts(), Vec::<Posted>::new());
    }

    #[tokio::test]
    async fn off_a_mac_an_older_or_silent_server_is_not_restarted_from_here_it_says_so_and_nothing_is_spawned() {
        let bench = Bench::new().await;
        bench.asr.health(|| Some(json!({"status": "ok", "model": "v3_e2e_rnnt"})));
        bench.tts.health(|| None);
        for platform in ["linux", "windows"] {
            let servers = bench.make(platform, Default::default());
            let error = servers.restart(ServerName::Asr, Some("v3_ctc")).await.unwrap_err();
            assert!(error.contains("перезапуск отсюда есть только на macOS"), "{error}");
            let error = servers.restart(ServerName::Tts, None).await.unwrap_err();
            assert!(error.contains("перезапуск отсюда есть только на macOS"), "{error}");
        }
        assert_eq!(bench.asr.posts(), Vec::<Posted>::new());
        assert_eq!(bench.recorder.started(), Vec::<String>::new());
    }

    // Beyond the TS tests: the Mac's restart itself, recorded instead of run.

    #[tokio::test]
    async fn on_a_mac_an_older_server_is_stopped_and_its_run_script_started_again_unless_launchd_keeps_it() {
        let bench = Bench::new().await;
        bench.tts.health(|| Some(json!({"status": "ok", "loaded": {"piper": ["denis"]}})));
        let servers = bench.make("macos", Default::default());
        servers.restart(ServerName::Tts, None).await.unwrap();
        let script = bench.mac_deploy.join("run-tts.sh");
        assert_eq!(
            bench.recorder.started(),
            vec!["pkill -f tts-server/server.py".to_string(), format!("nohup {}", script.display())]
        );
        // A launchd service comes back by itself: only stopped.
        let daemons = bench.root.path().join("LaunchDaemons");
        std::fs::create_dir_all(&daemons).unwrap();
        std::fs::write(daemons.join("app.duowave.asr.plist"), "").unwrap();
        bench.asr.health(|| Some(json!({"status": "ok", "model": "v3_e2e_rnnt"})));
        servers.restart(ServerName::Asr, Some("v3_ctc")).await.unwrap();
        assert_eq!(bench.recorder.started().last().map(String::as_str), Some("pkill -f asr-server/server.py"));
        assert_eq!(servers.asr_on_disk().as_deref(), Some("v3_ctc"), "the run script starts with the choice");
    }

    #[test]
    fn hands_the_servers_only_what_they_need_of_the_environment() {
        let env = Env::from_pairs([
            ("PATH", "/usr/bin"),
            ("Path", "C:\\Windows"),
            ("HF_HOME", "/cache"),
            ("TTS_OWN_API_KEY", "k"),
            ("https_proxy", "http://proxy"),
            ("LLM_API_KEY", "secret"),
            ("TELEGRAM_BOT_TOKEN", "secret"),
            ("DATA_DIR", "/data"),
        ]);
        let names: Vec<String> = server_env_of(&env).into_iter().map(|(name, _)| name).collect();
        assert_eq!(names, ["DATA_DIR", "HF_HOME", "PATH", "Path", "TTS_OWN_API_KEY", "https_proxy"]);
        assert_eq!(root("http://127.0.0.1:8090/v1/"), "http://127.0.0.1:8090");
        assert_eq!(root("http://x"), "http://x");
    }
}
