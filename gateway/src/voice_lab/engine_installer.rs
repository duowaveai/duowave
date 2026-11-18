//! An engine installed by one button, the same way on macOS, Linux and Windows: uv makes the environment and installs
//! the pinned packages (clone-server/requirements), PyTorch comes from the index that suits the machine, the weights
//! from HuggingFace. A machine with the bench (../self-voice-tests) gets its installation copied instead: on a Mac an
//! APFS clone — no download and no disk spent. ready.json, written last, is what makes an engine «installed».

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use super::clone_engines::{flavour, verdict, EngineId, EngineSpec, Flavour};
use super::jobs::{JobContext, JobError, JobReason, RunOptions};
use super::js;
use super::platform::{GpuKind, OsName, PlatformInfo};
use crate::utils::time::now_iso;

/// CosyVoice's repository at the commit the bench runs.
pub const COSYVOICE_REPO: &str = "https://github.com/FunAudioLLM/CosyVoice";
pub const COSYVOICE_COMMIT: &str = "074ca6dc9e80a2f424f1f74b48bdd7d3fea531cc";
const WETEXT_URL: &str = "https://www.modelscope.cn/models/pengzhendong/wetext/resolve/master";

fn weights(engine: EngineId) -> &'static str {
    match engine {
        EngineId::Omnivoice => "from huggingface_hub import snapshot_download; snapshot_download('k2-fsa/OmniVoice')",
        EngineId::Cosyvoice3 => {
            "import sys; from huggingface_hub import snapshot_download; \
             snapshot_download('FunAudioLLM/Fun-CosyVoice3-0.5B-2512', local_dir=sys.argv[1])"
        }
        EngineId::Voxcpm => "from huggingface_hub import snapshot_download; snapshot_download('openbmb/VoxCPM2')",
    }
}

/// Bytes of the regular files under a folder, links not followed: how much a download has brought so far.
pub fn folder_bytes(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let mut total = 0;
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            total += folder_bytes(&entry.path());
        } else if kind.is_file() {
            // A file the download just renamed is gone: nothing to count.
            total += std::fs::symlink_metadata(entry.path()).map(|meta| meta.len()).unwrap_or(0);
        }
    }
    total
}

/// How a download is watched: it stands still this long (no byte added to its folder) — it is taken down and started
/// again, so many times; then the job fails in words (п. 66: the CDN hung at 603 MB for minutes, and nothing noticed).
#[derive(Debug, Clone, Copy)]
pub struct Watch {
    pub stall_after: Duration,
    pub retries: u32,
    pub period: Duration,
}

pub const WATCH: Watch = Watch { stall_after: Duration::from_secs(60), retries: 3, period: Duration::from_secs(2) };

/// Partial files of a download that a new start will not continue: huggingface_hub 2.x writes each into
/// `<etag>.<8 hex>.incomplete`, its own per process, and a new process starts the file again. Older ones (0.3x,
/// CosyVoice's) write `<etag>.incomplete` and continue it — those stay. Taken away before a new start: gigabytes.
pub fn drop_dead_partials(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let mut freed = 0;
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else { continue };
        let path = entry.path();
        if kind.is_dir() {
            freed += drop_dead_partials(&path);
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(stem) = name.strip_suffix(".incomplete") else { continue };
        let own =
            stem.rsplit_once('.').is_some_and(|(_, tag)| tag.len() == 8 && tag.bytes().all(|b| b.is_ascii_hexdigit()));
        if own {
            let size = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
            if std::fs::remove_file(&path).is_ok() {
                freed += size;
            }
        }
    }
    freed
}

/// A whole number with its thousands apart, as Russian writes it: 2300 → «2 300».
fn thousands(value: i64) -> String {
    let digits = value.abs().to_string();
    let mut out = String::new();
    for (at, digit) in digits.chars().enumerate() {
        if at > 0 && (digits.len() - at).is_multiple_of(3) {
            out.push('\u{a0}');
        }
        out.push(digit);
    }
    if value < 0 {
        out.insert(0, '-');
    }
    out
}

/// Empties a folder but for the named entries in it; no folder — nothing to do.
fn clear_except(dir: &Path, keep: &[&str]) -> std::io::Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries.flatten() {
        if keep.iter().any(|name| entry.file_name() == *name) {
            continue;
        }
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            std::fs::remove_dir_all(&path)?;
        } else {
            std::fs::remove_file(&path)?;
        }
    }
    Ok(())
}

/// «603 из ~2 300 МБ · 1,2 МБ/с»: how much there is, of how much, and how fast it comes now.
pub fn download_line(done_bytes: u64, expected_mb: u64, bytes_per_second: f64) -> String {
    let done = js::round_i64(done_bytes as f64 / 1_048_576.0);
    let speed = if bytes_per_second >= 1_048_576.0 {
        let tenths = js::round(bytes_per_second / 1_048_576.0 * 10.0) / 10.0;
        crate::tr!("{speed} МБ/с", speed = format!("{tenths:.1}").replace('.', ","))
    } else {
        crate::tr!("{speed} КБ/с", speed = js::round_i64(bytes_per_second / 1024.0))
    };
    crate::tr!(
        "{done} из ~{total} МБ · {speed}",
        done = thousands(done),
        total = thousands(expected_mb as i64),
        speed = speed
    )
}

/// A long download, seen by the size of its folder against the size it will have: the weights are gigabytes, and a
/// bar standing still for minutes looks like a hang. «Скачиваю веса модели · 603 из ~2 300 МБ · 1,2 МБ/с». Standing
/// still `stall_after` — the program is taken down and started again (finished files stay; a 2.x partial is dropped,
/// it would not be continued); after `retries` — the job fails: «Hugging Face не отвечает — проверьте сеть или VPN».
async fn watched_download<F, Fut>(
    context: &JobContext,
    dir: &Path,
    expected_mb: u64,
    label: &str,
    (from, to): (f64, f64),
    watch: Watch,
    run: F,
) -> Result<Map<String, Value>, JobError>
where
    F: Fn(CancellationToken, u32) -> Fut,
    Fut: std::future::Future<Output = Result<Map<String, Value>, JobError>>,
{
    // Partials an earlier job left (stopped, failed): a new start would not continue them.
    drop_dead_partials(dir);
    for attempt in 0..=watch.retries {
        if attempt > 0 {
            let freed = drop_dead_partials(dir);
            context.log(&format!(
                "скачивание стояло {} с — повторяю ({attempt} из {}), убрано недокачанного {} МБ",
                watch.stall_after.as_secs(),
                watch.retries,
                freed / 1_048_576
            ));
        }
        let head = if attempt == 0 {
            label.to_string()
        } else {
            crate::tr!(
                "Скачивание стояло — повторяю ({attempt} из {retries})",
                attempt = attempt,
                retries = watch.retries
            )
        };
        let show = |bytes: u64, speed: f64| {
            let done = (bytes as f64 / 1_048_576.0 / expected_mb.max(1) as f64).min(1.0);
            context.stage(
                "weights",
                &format!("{head} · {}", download_line(bytes, expected_mb, speed)),
                Some(from + (to - from) * done),
            );
        };
        let token = CancellationToken::new();
        let program = run(token.clone(), attempt);
        tokio::pin!(program);
        let started = tokio::time::Instant::now();
        let mut top = folder_bytes(dir);
        let mut grown_at = started;
        // Speed over the last ten seconds: one slow tick does not read as a hang.
        let mut samples: std::collections::VecDeque<(tokio::time::Instant, u64)> = [(started, top)].into();
        show(top, 0.0);
        let mut timer = tokio::time::interval_at(started + watch.period, watch.period);
        let outcome = loop {
            tokio::select! {
                result = &mut program => break result,
                _ = timer.tick() => {
                    let now = tokio::time::Instant::now();
                    let bytes = folder_bytes(dir);
                    if bytes > top {
                        top = bytes;
                        grown_at = now;
                    }
                    samples.push_back((now, bytes));
                    while samples.len() > 2 && now - samples[0].0 > Duration::from_secs(10) {
                        samples.pop_front();
                    }
                    let (first_at, first) = samples[0];
                    let seconds = (now - first_at).as_secs_f64();
                    let speed = if seconds > 0.0 { bytes.saturating_sub(first) as f64 / seconds } else { 0.0 };
                    show(bytes, speed);
                    if now - grown_at >= watch.stall_after && !token.is_cancelled() {
                        token.cancel();
                    }
                }
            }
        };
        match outcome {
            Err(error) if error.reason == Some(JobReason::Stalled) => continue,
            // The program fell (the network, the CDN): words for the owner; its own last line («Fetching 13 files:
            // 92%…») stays in the log, the panel shows it under them (п. 68).
            Err(error) if matches!(error.reason, None | Some(JobReason::Error)) => {
                context.log(&error.message);
                return Err(crate::tr!("модель не скачалась — проверьте сеть или VPN и попробуйте снова.").into());
            }
            other => return other,
        }
    }
    Err(crate::tr!("Hugging Face не отвечает — проверьте сеть или VPN: скачивание модели стояло, повторы не помогли.")
        .into())
}

/// The environment's Python, where uv puts it on each system.
pub fn python_of(engine_dir: &Path, os: OsName) -> PathBuf {
    if os == OsName::Win32 {
        engine_dir.join("env").join("Scripts").join("python.exe")
    } else {
        engine_dir.join("env").join("bin").join("python")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InstallSource {
    Bench,
    Download,
}

impl InstallSource {
    pub fn as_str(self) -> &'static str {
        match self {
            InstallSource::Bench => "bench",
            InstallSource::Download => "download",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadyInfo {
    pub engine: String,
    pub device: String,
    pub torch: String,
    pub source: InstallSource,
    pub installed_at: String,
    /// The weights' revision (EngineSpec::revision): a voice snapshot of other weights is taken again. Older
    /// installs have none and count as the spec's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
}

/// What the bench must have for a copy to work (its environment and weights).
fn bench_parts(engine: &EngineSpec, bench_models: &Path) -> Option<Vec<&'static str>> {
    let mut parts = if engine.id == EngineId::Cosyvoice3 {
        vec!["env", "repo", "pretrained", "wetext-mirror"]
    } else {
        // OmniVoice and VoxCPM are PyPI packages: an environment and a HuggingFace cache are all there is.
        vec!["env", "hf-cache"]
    };
    let base = bench_models.join(engine.id.as_str());
    let have = |part: &str| base.join(part).exists();
    if !parts.iter().all(|part| have(part)) || !base.join("env").join("bin").join("python").exists() {
        return None;
    }
    if engine.id == EngineId::Cosyvoice3 && have("hf-cache") {
        parts.push("hf-cache");
    }
    Some(parts)
}

/// PyTorch from the right index: the Mac's wheels on PyPI, CUDA ones for NVIDIA, the small CPU ones otherwise.
pub fn torch_packages(engine: &EngineSpec, platform: &PlatformInfo) -> Vec<String> {
    let mut packages = vec![format!("torch=={}", engine.torch), format!("torchaudio=={}", engine.torch)];
    let kind = flavour(platform);
    if kind == Flavour::Mac {
        return packages;
    }
    let index = if kind == Flavour::Cuda { engine.cuda_index } else { "cpu" };
    packages.extend(["--index-url".to_string(), format!("https://download.pytorch.org/whl/{index}")]);
    packages
}

/// One line of Python that names where torch computes: what the engine will speak on.
fn check_script(engine: &EngineSpec) -> String {
    let imports = match engine.id {
        EngineId::Omnivoice => "import omnivoice",
        EngineId::Voxcpm => "from voxcpm import VoxCPM",
        EngineId::Cosyvoice3 => {
            "import sys; sys.path[:0] = [sys.argv[1], sys.argv[1] + \"/third_party/Matcha-TTS\"]; import \
             onnxruntime, whisper; from cosyvoice.cli.cosyvoice import CosyVoice3"
        }
    };
    format!(
        "{imports}; import json, torch; d = \"cuda\" if torch.cuda.is_available() else (\"mps\" if \
         torch.backends.mps.is_available() else \"cpu\"); print(json.dumps({{\"stage\": \"done\", \"device\": d, \
         \"torch\": torch.__version__.split(\"+\")[0]}}))"
    )
}

async fn download(url: &str, target: &Path, signal: &tokio_util::sync::CancellationToken) -> Result<(), JobError> {
    // modelscope refuses clients without a browser's agent and rate-limits: a pause and a few tries (as the bench
    // does).
    for _attempt in 1..=5 {
        if signal.is_cancelled() {
            return Err(crate::tr!("Остановлено.").into());
        }
        let fetch = async {
            let response = crate::utils::http::client()
                .get(url)
                .header("User-Agent", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36")
                .timeout(Duration::from_secs(60))
                .send()
                .await
                .ok()?;
            let ok = response.status().is_success();
            let body = response.bytes().await.ok()?;
            (ok && body.len() > 100_000).then_some(body)
        };
        let body = tokio::select! {
            body = fetch => body,
            _ = signal.cancelled() => None,
        };
        if let Some(body) = body {
            if std::fs::write(target, &body).is_ok() {
                return Ok(());
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(6)) => {}
            _ = signal.cancelled() => {}
        }
    }
    let tail: Vec<&str> = url.rsplit('/').take(3).collect();
    let tail: Vec<&str> = tail.into_iter().rev().collect();
    Err(crate::tr!("не скачался {file}", file = tail.join("/")).into())
}

pub struct InstallPlan {
    pub engine: &'static EngineSpec,
    pub platform: PlatformInfo,
    /// clone-server/runtime
    pub runtime_dir: PathBuf,
    /// clone-server (its requirements/)
    pub server_dir: PathBuf,
    /// The bench's models folder, when this computer has one.
    pub bench_models: Option<PathBuf>,
    pub env: HashMap<String, String>,
    /// How the weights' download is watched (WATCH; tests — seconds).
    pub watch: Watch,
}

/// A path as a program's argument.
fn arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| item.to_string()).collect()
}

pub async fn install_engine(context: &JobContext, plan: &InstallPlan) -> Result<ReadyInfo, JobError> {
    let engine = plan.engine;
    let platform = &plan.platform;
    context.stage("check", &crate::tr!("Проверяю компьютер"), Some(2.0));
    let fits = verdict(engine, platform);
    if !fits.supported {
        return Err(fits.reason.unwrap_or_else(|| crate::tr!("не поддерживается")).into());
    }
    let target = js::resolve(&plan.runtime_dir.join(engine.id.as_str()));
    let python = arg(&python_of(&target, platform.os));
    let copy_from = match &plan.bench_models {
        Some(bench) if platform.os != OsName::Win32 => bench_parts(engine, bench).map(|parts| (bench.clone(), parts)),
        _ => None,
    };
    let uv = platform.tools.uv.clone();
    if copy_from.is_none() && uv.is_none() {
        return Err(if platform.os == OsName::Win32 {
            crate::tr!("Нужен uv — менеджер Python. Установите: powershell -c \"irm https://astral.sh/uv/install.ps1 | iex\" — и нажмите ещё раз.")
        } else {
            crate::tr!("Нужен uv — менеджер Python. Установите: curl -LsSf https://astral.sh/uv/install.sh | sh (или brew install uv) — и нажмите ещё раз.")
        }
        .into());
    }
    if copy_from.is_none() && engine.id == EngineId::Cosyvoice3 && !platform.tools.git {
        return Err(crate::tr!("Для CosyVoice нужен git.").into());
    }
    let need_mb = if copy_from.is_some() && platform.os == OsName::Darwin {
        0
    } else {
        engine.disk_mb.of(flavour(platform)) + 1_024
    };
    if platform.disk_free_mb < need_mb {
        return Err(crate::tr!(
            "На диске свободно {free} ГБ, а нужно около {need} ГБ.",
            free = js::round_i64(platform.disk_free_mb as f64 / 1024.0),
            need = js::round_i64(need_mb as f64 / 1024.0)
        )
        .into());
    }

    // A failed or stopped install leaves a folder without ready.json: it starts over — but the weights fetched so far
    // stay (п. 68: «Отменить» и снова — and gigabytes from zero): the environment is made anew, the download skips
    // the files it has. A copy from the bench takes all anew (its folders go in whole).
    let keep: &[&str] = if copy_from.is_none() { &["hf-cache", "pretrained"] } else { &[] };
    clear_except(&target, keep).map_err(|error| error.to_string())?;
    std::fs::create_dir_all(&target).map_err(|error| error.to_string())?;
    let mut env = plan.env.clone();
    env.insert("HF_HOME".into(), arg(&target.join("hf-cache")));
    env.insert("HF_HUB_DISABLE_TELEMETRY".into(), "1".into());
    env.insert("PYTHONUNBUFFERED".into(), "1".into());
    let with_env = || RunOptions { env: Some(env.clone()), ..RunOptions::default() };

    let source = if let Some((bench, parts)) = &copy_from {
        context.stage("copy", &crate::tr!("Копирую готовую установку со стенда"), Some(10.0));
        let count = parts.len() as f64;
        for (index, part) in parts.iter().enumerate() {
            let mut args =
                if platform.os == OsName::Darwin { strings(&["-cR"]) } else { strings(&["-a", "--reflink=auto"]) };
            args.push(arg(&bench.join(engine.id.as_str()).join(part)));
            args.push(arg(&target.join(part)));
            let percent = (10.0 + index as f64 * 70.0 / count, 10.0 + (index as f64 + 1.0) * 70.0 / count);
            context.run("cp", &args, RunOptions { percent: Some(percent), ..RunOptions::default() }).await?;
        }
        InstallSource::Bench
    } else {
        let uv = uv.unwrap_or_default();
        context.stage("python", &crate::tr!("Ставлю Python {version}", version = engine.python), Some(5.0));
        let venv = vec!["venv".into(), "--python".into(), engine.python.into(), arg(&target.join("env"))];
        context.run(&uv, &venv, with_env()).await?;
        context.stage("torch", &crate::tr!("Ставлю PyTorch"), Some(10.0));
        let mut torch = strings(&["pip", "install", "-p", &python]);
        torch.extend(torch_packages(engine, platform));
        context.run(&uv, &torch, with_env()).await?;
        if engine.id == EngineId::Cosyvoice3 {
            context.stage("repo", &crate::tr!("Скачиваю CosyVoice"), Some(30.0));
            let repo = arg(&target.join("repo"));
            std::fs::create_dir_all(&repo).map_err(|error| error.to_string())?;
            for args in [
                strings(&["-C", &repo, "init", "-q"]),
                strings(&["-C", &repo, "remote", "add", "origin", COSYVOICE_REPO]),
                strings(&["-C", &repo, "fetch", "-q", "--depth", "1", "origin", COSYVOICE_COMMIT]),
                strings(&["-C", &repo, "checkout", "-q", "--detach", "FETCH_HEAD"]),
                strings(&["-C", &repo, "submodule", "update", "-q", "--init", "--recursive", "--depth", "1"]),
            ] {
                context.run("git", &args, with_env()).await?;
            }
        }
        context.stage("packages", &crate::tr!("Ставлю библиотеки"), Some(40.0));
        if engine.id == EngineId::Cosyvoice3 {
            context.run(&uv, &strings(&["pip", "install", "-p", &python, "setuptools<81"]), with_env()).await?;
        }
        let requirements = arg(&plan.server_dir.join("requirements").join(format!("{}.txt", engine.id)));
        context.run(&uv, &strings(&["pip", "install", "-p", &python, "-r", &requirements]), with_env()).await?;
        if engine.id == EngineId::Cosyvoice3 {
            let onnx =
                if platform.gpu.kind == GpuKind::Nvidia { "onnxruntime-gpu==1.18.0" } else { "onnxruntime==1.18.0" };
            context.run(&uv, &strings(&["pip", "install", "-p", &python, onnx]), with_env()).await?;
            // The frontend takes Whisper's mel filters (no Whisper model is loaded); it builds only without isolation.
            let whisper =
                strings(&["pip", "install", "-p", &python, "--no-build-isolation", "openai-whisper==20250625"]);
            context.run(&uv, &whisper, with_env()).await?;
            context.stage("wetext", &crate::tr!("Скачиваю словари нормализации текста"), Some(55.0));
            let mirror = target.join("wetext-mirror");
            for lang in ["en", "zh"] {
                std::fs::create_dir_all(mirror.join(lang).join("tn")).map_err(|error| error.to_string())?;
            }
            for file in ["tagger.fst", "verbalizer.fst"] {
                let english = mirror.join("en").join("tn").join(file);
                download(&format!("{WETEXT_URL}/en/tn/{file}"), &english, &context.signal()).await?;
                // The Chinese frontend is not used for Russian, but CosyVoice will not start without its files.
                let copy = vec![
                    "-c".to_string(),
                    "import shutil, sys; shutil.copy(sys.argv[1], sys.argv[2])".to_string(),
                    arg(&english),
                    arg(&mirror.join("zh").join("tn").join(file)),
                ];
                context.run(&python, &copy, with_env()).await?;
            }
        }
        let weights_dir = target.join("pretrained").join("Fun-CosyVoice3-0.5B-2512");
        let cosy = engine.id == EngineId::Cosyvoice3;
        let mut args = vec!["-c".to_string(), weights(engine.id).to_string()];
        if cosy {
            args.push(arg(&weights_dir));
        }
        let watched = if cosy { weights_dir.clone() } else { target.join("hf-cache") };
        watched_download(
            context,
            &watched,
            engine.weights_mb,
            &crate::tr!("Скачиваю веса модели"),
            (60.0, 88.0),
            plan.watch,
            |cancel, attempt| {
                let mut options = RunOptions { cancel: Some(cancel), ..with_env() };
                // Started again after standing still: over plain HTTPS, not Xet — there huggingface_hub itself gives
                // up on 10 s of silence and continues the file from where it was (Range), within the one program.
                if attempt > 0 {
                    if let Some(env) = options.env.as_mut() {
                        env.insert("HF_HUB_DISABLE_XET".into(), "1".into());
                    }
                }
                context.run(&python, &args, options)
            },
        )
        .await?;
        InstallSource::Download
    };

    context.stage("verify", &crate::tr!("Проверяю установку"), Some(90.0));
    let mut offline = env.clone();
    offline.insert("HF_HUB_OFFLINE".into(), "1".into());
    let check = vec!["-c".to_string(), check_script(engine), arg(&target.join("repo"))];
    let checked = context.run(&python, &check, RunOptions { env: Some(offline), ..RunOptions::default() }).await?;
    let fallback = fits.device.map(|device| device.as_str()).unwrap_or("cpu");
    let ready = ReadyInfo {
        engine: engine.id.as_str().into(),
        device: checked.get("device").and_then(Value::as_str).unwrap_or(fallback).to_string(),
        torch: checked.get("torch").and_then(Value::as_str).unwrap_or(engine.torch).to_string(),
        source,
        installed_at: now_iso(),
        revision: Some(engine.revision.to_string()),
    };
    let text = serde_json::to_string_pretty(&ready).map_err(|error| error.to_string())?;
    std::fs::write(target.join("ready.json"), text).map_err(|error| error.to_string())?;
    Ok(ready)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::voice_lab::clone_engines::spec;
    use crate::voice_lab::jobs::{JobDeps, JobKind, JobRunState, JobRunner, JobState};
    use crate::voice_lab::platform::{GpuInfo, Tools};
    use crate::voice_lab::system_memory::{MemorySnapshot, Pressure};

    #[cfg(unix)]
    #[test]
    fn measures_a_download_by_its_folder_nested_files_count_links_to_them_do_not_a_missing_folder_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let blobs = dir.path().join("blobs").join("de");
        std::fs::create_dir_all(&blobs).unwrap();
        std::fs::write(blobs.join("model.safetensors"), vec![0u8; 3000]).unwrap();
        std::fs::write(dir.path().join("config.json"), vec![0u8; 200]).unwrap();
        std::fs::create_dir(dir.path().join("snapshots")).unwrap();
        // HuggingFace's snapshots are links to the blobs: counted once.
        std::os::unix::fs::symlink(
            blobs.join("model.safetensors"),
            dir.path().join("snapshots").join("model.safetensors"),
        )
        .unwrap();
        assert_eq!(folder_bytes(dir.path()), 3200);
        assert_eq!(folder_bytes(&dir.path().join("nothing-here")), 0);
    }

    #[test]
    fn finds_the_environments_python_where_uv_puts_it_on_each_system() {
        let engine = Path::new("/r/omnivoice");
        assert_eq!(python_of(engine, OsName::Darwin), engine.join("env").join("bin").join("python"));
        assert_eq!(python_of(engine, OsName::Win32), engine.join("env").join("Scripts").join("python.exe"));
    }

    fn calm() -> MemorySnapshot {
        MemorySnapshot {
            total_mb: 36_864,
            pool_mb: 20_000,
            pressure: Pressure::Normal,
            swap_in_mbps: None,
            vram_free_mb: None,
            at: 0,
        }
    }

    fn runner() -> JobRunner {
        JobRunner::new(JobDeps::new(Arc::new(|| Box::pin(async { calm() }))))
    }

    async fn finished(jobs: &JobRunner) -> JobState {
        for _ in 0..1_200 {
            if !jobs.busy() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        jobs.state().expect("a job ran")
    }

    fn mac(uv: Option<String>) -> PlatformInfo {
        PlatformInfo {
            os: OsName::Darwin,
            arch: "arm64".into(),
            ram_mb: 36_864,
            cpus: 14,
            cpu_name: None,
            docker: false,
            gpu: GpuInfo { kind: GpuKind::Apple, name: Some("Apple M4 Max".into()), vram_mb: None, driver: None },
            disk_free_mb: 80_000,
            tools: Tools {
                uv,
                git: true,
                taskpolicy: crate::voice_lab::platform::Taskpolicy::None,
                systemd_run: false,
            },
            checked_at: String::new(),
        }
    }

    #[cfg(unix)]
    fn executable(path: &Path, script: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, script).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// A Python that answers the install's check and fetches no weights.
    #[cfg(unix)]
    const FAKE_PYTHON: &str = "#!/bin/sh\necho \"$*\" >> \"$(dirname \"$0\")/calls\"\n\
        case \"$2\" in *'import json, torch'*) echo '{\"stage\":\"done\",\"device\":\"mps\",\"torch\":\"2.8.0\"}';; esac\n";

    async fn install(plan: InstallPlan) -> JobState {
        let jobs = runner();
        jobs.start(
            JobKind::Install,
            "omnivoice",
            move |context| async move {
                let ready = install_engine(&context, &plan).await?;
                Ok(Some(serde_json::to_value(ready).ok().and_then(|v| v.as_object().cloned()).unwrap_or_default()))
            },
            None,
        )
        .unwrap();
        finished(&jobs).await
    }

    /// The install with a uv that only pretends: the environment it «makes» has a Python that only answers the check.
    #[cfg(unix)]
    #[tokio::test]
    async fn installs_an_engine_through_uv_and_its_python_and_writes_ready_json_last() {
        let root = tempfile::tempdir().unwrap();
        let uv = root.path().join("bin").join("uv");
        let python = FAKE_PYTHON.replace('\'', "'\\''");
        executable(
            &uv,
            &format!(
                "#!/bin/sh\necho \"$*\" >> \"{log}\"\nif [ \"$1\" = venv ]; then mkdir -p \"$4/bin\"; printf '%s' '{python}' > \
                 \"$4/bin/python\"; chmod 755 \"$4/bin/python\"; fi\n",
                log = root.path().join("uv-calls").display(),
            ),
        );
        let plan = InstallPlan {
            engine: spec(EngineId::Omnivoice),
            platform: mac(Some(uv.to_string_lossy().into_owned())),
            runtime_dir: root.path().join("runtime"),
            server_dir: root.path().join("clone-server"),
            bench_models: None,
            env: std::env::var("PATH").ok().map(|path| ("PATH".to_string(), path)).into_iter().collect(),
            watch: WATCH,
        };
        let state = install(plan).await;
        assert_eq!(state.state, JobRunState::Done, "{state:?}");
        let calls = std::fs::read_to_string(root.path().join("uv-calls")).unwrap();
        let target = root.path().join("runtime").join("omnivoice");
        assert!(calls.contains(&format!("venv --python 3.11 {}", target.join("env").display())), "{calls}");
        assert!(calls.contains("torch==2.8.0 torchaudio==2.8.0"), "{calls}");
        assert!(calls.contains("requirements/omnivoice.txt"), "{calls}");
        let ready: ReadyInfo =
            serde_json::from_str(&std::fs::read_to_string(target.join("ready.json")).unwrap()).unwrap();
        assert_eq!((ready.device.as_str(), ready.source), ("mps", InstallSource::Download));
        let result = state.result.unwrap();
        assert_eq!(result.get("source"), Some(&Value::from("download")));
    }

    /// The bench's copy: `cp` of its environment and weights, then the same check.
    #[cfg(unix)]
    #[tokio::test]
    async fn copies_the_benchs_installation_when_this_computer_has_one() {
        let root = tempfile::tempdir().unwrap();
        let bench = root.path().join("bench").join("models");
        executable(&bench.join("omnivoice").join("env").join("bin").join("python"), FAKE_PYTHON);
        std::fs::create_dir_all(bench.join("omnivoice").join("hf-cache")).unwrap();
        std::fs::write(bench.join("omnivoice").join("hf-cache").join("weights.bin"), "w").unwrap();
        let plan = InstallPlan {
            engine: spec(EngineId::Omnivoice),
            // The host's own cp: an APFS clone on a Mac, a reflink or a copy on Linux.
            platform: PlatformInfo {
                os: if cfg!(target_os = "macos") { OsName::Darwin } else { OsName::Linux },
                ..mac(None)
            },
            runtime_dir: root.path().join("runtime"),
            server_dir: root.path().join("clone-server"),
            bench_models: Some(bench),
            env: HashMap::new(),
            watch: WATCH,
        };
        let state = install(plan).await;
        assert_eq!(state.state, JobRunState::Done, "{state:?}");
        let target = root.path().join("runtime").join("omnivoice");
        assert!(target.join("hf-cache").join("weights.bin").exists());
        let ready: ReadyInfo =
            serde_json::from_str(&std::fs::read_to_string(target.join("ready.json")).unwrap()).unwrap();
        assert_eq!(ready.source, InstallSource::Bench);
    }

    /// A uv whose Python is `python`: it makes the environment, the Python answers the check and fetches the weights
    /// its own way.
    #[cfg(unix)]
    fn plan_with_python(root: &Path, python: &str, watch: Watch) -> InstallPlan {
        let uv = root.join("bin").join("uv");
        let python = python.replace('\'', "'\\''");
        executable(
            &uv,
            &format!(
                "#!/bin/sh\nif [ \"$1\" = venv ]; then mkdir -p \"$4/bin\"; printf '%s' '{python}' > \"$4/bin/python\"; chmod 755 \
                 \"$4/bin/python\"; fi\n"
            ),
        );
        InstallPlan {
            engine: spec(EngineId::Omnivoice),
            platform: mac(Some(uv.to_string_lossy().into_owned())),
            runtime_dir: root.join("runtime"),
            server_dir: root.join("clone-server"),
            bench_models: None,
            env: std::env::var("PATH").ok().map(|path| ("PATH".to_string(), path)).into_iter().collect(),
            watch,
        }
    }

    /// The weights' fetch of a hanging CDN: each start brings a little into its own `.<8 hex>.incomplete` (as
    /// huggingface_hub 2.x does) and then stands still; from the `good`-th start on it finishes. `starts` counts them.
    #[cfg(unix)]
    fn hanging_python(good: u32) -> String {
        format!(
            "#!/bin/sh\ncase \"$2\" in\n*snapshot_download*)\n  mkdir -p \"$HF_HOME\"; n=$(cat \"$HF_HOME/../starts\" 2>/dev/null || echo 0); n=$((n+1)); echo $n > \
             \"$HF_HOME/../starts\"\n  mkdir -p \"$HF_HOME/hub/blobs\"\n  if [ $n -ge {good} ]; then head -c 4096 /dev/zero > \"$HF_HOME/hub/blobs/w\"; \
             exit 0; fi\n  head -c 2048 /dev/zero > \"$HF_HOME/hub/blobs/w.0a1b2c3$n.incomplete\"\n  exec sleep 30;;\n\
             *'import json, torch'*) echo '{{\"stage\":\"done\",\"device\":\"mps\",\"torch\":\"2.8.0\"}}';;\nesac\n"
        )
    }

    // Long enough that a busy test run still starts the fake fetch before it counts as standing still.
    const QUICK: Watch =
        Watch { stall_after: Duration::from_millis(2_000), retries: 3, period: Duration::from_millis(100) };

    /// п. 66: the download stood still at 603 MB for minutes, the bar on 72 %, and nothing noticed. Now: standing still
    /// — taken down and started again; the dead partial is dropped (2.x would not continue it), what finished stays.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_download_standing_still_is_started_again_and_its_dead_partial_dropped() {
        let root = tempfile::tempdir().unwrap();
        let state = install(plan_with_python(root.path(), &hanging_python(2), QUICK)).await;
        assert_eq!(state.state, JobRunState::Done, "{state:?}");
        let target = root.path().join("runtime").join("omnivoice");
        assert_eq!(std::fs::read_to_string(target.join("starts")).unwrap().trim(), "2", "one stall, one start again");
        let blobs = target.join("hf-cache").join("hub").join("blobs");
        assert!(blobs.join("w").exists());
        assert!(!blobs.join("w.0a1b2c31.incomplete").exists(), "the dead partial of the first start is gone");
        assert!(state.log.iter().any(|line| line.contains("скачивание стояло")), "{:?}", state.log);
    }

    /// Standing still every time: after three starts again — in words, the job failed (the panel offers «Повторить»).
    #[cfg(unix)]
    #[tokio::test]
    async fn a_download_that_keeps_standing_still_fails_in_words_after_three_tries() {
        let root = tempfile::tempdir().unwrap();
        let state = install(plan_with_python(root.path(), &hanging_python(99), QUICK)).await;
        assert_eq!(state.state, JobRunState::Error, "{state:?}");
        assert!(state.error.as_deref().unwrap_or("").starts_with("Hugging Face не отвечает"), "{state:?}");
        let target = root.path().join("runtime").join("omnivoice");
        let starts: u32 = std::fs::read_to_string(target.join("starts")).unwrap().trim().parse().unwrap();
        assert!((2..=4).contains(&starts), "the first and three again (a busy run may not start one): {starts}");
        let again = state.log.iter().filter(|line| line.contains("повторяю")).count();
        assert_eq!(again, 3, "{:?}", state.log);
        assert_ne!(state.reason, Some(JobReason::Stalled), "the stall stays inside the install");
    }

    /// п. 68: «Отменить» left a hung download living (TERM did not take it, KILL did). Now the stop takes the job's
    /// program down — TERM, and KILL after the grace when TERM is ignored — and the job ends stopped.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_stop_takes_down_a_hung_download_that_ignores_term_and_the_job_ends_stopped() {
        let root = tempfile::tempdir().unwrap();
        let hung = "#!/bin/sh\ncase \"$2\" in\n*snapshot_download*) mkdir -p \"$HF_HOME\"; echo $$ > \"$HF_HOME/../pid\"; trap '' TERM; \
                    exec sleep 60;;\nesac\n";
        let slow = Watch { stall_after: Duration::from_secs(60), retries: 3, period: Duration::from_millis(100) };
        let plan = plan_with_python(root.path(), hung, slow);
        let jobs = runner();
        jobs.start(
            JobKind::Install,
            "omnivoice",
            move |context| async move {
                install_engine(&context, &plan).await?;
                Ok(None)
            },
            None,
        )
        .unwrap();
        let pid_file = root.path().join("runtime").join("omnivoice").join("pid");
        for _ in 0..400 {
            if pid_file.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let pid = std::fs::read_to_string(&pid_file).unwrap().trim().to_string();
        assert!(jobs.stop());
        let state = finished(&jobs).await;
        assert_eq!(state.state, JobRunState::Stopped, "{state:?}");
        let alive = std::process::Command::new("kill").args(["-0", &pid]).status().unwrap().success();
        assert!(!alive, "the download's program is gone");
    }

    #[test]
    fn a_new_start_keeps_the_weights_fetched_so_far_and_empties_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("hf-cache").join("hub")).unwrap();
        std::fs::write(dir.path().join("hf-cache").join("hub").join("w"), "w").unwrap();
        std::fs::create_dir_all(dir.path().join("env").join("bin")).unwrap();
        std::fs::write(dir.path().join("ready.json"), "{}").unwrap();
        clear_except(dir.path(), &["hf-cache", "pretrained"]).unwrap();
        assert!(dir.path().join("hf-cache").join("hub").join("w").exists());
        assert!(!dir.path().join("env").exists() && !dir.path().join("ready.json").exists());
        clear_except(&dir.path().join("nothing"), &[]).unwrap();
    }

    #[test]
    fn drops_only_the_partials_a_new_start_would_not_continue_and_says_the_download_in_megabytes() {
        let dir = tempfile::tempdir().unwrap();
        let blobs = dir.path().join("blobs");
        std::fs::create_dir_all(&blobs).unwrap();
        std::fs::write(blobs.join("e1.0a1b2c3d.incomplete"), vec![0u8; 1000]).unwrap();
        // 0.3x's own partial (CosyVoice): it is continued on the next start — stays.
        std::fs::write(blobs.join("e2.incomplete"), vec![0u8; 500]).unwrap();
        std::fs::write(blobs.join("model.safetensors"), vec![0u8; 300]).unwrap();
        assert_eq!(drop_dead_partials(dir.path()), 1000);
        assert!(blobs.join("e2.incomplete").exists() && blobs.join("model.safetensors").exists());
        assert_eq!(download_line(603 * 1_048_576, 2_300, 0.0), "603 из ~2\u{a0}300 МБ · 0 КБ/с");
        assert_eq!(download_line(1_048_576, 2_300, 1.25 * 1_048_576.0), "1 из ~2\u{a0}300 МБ · 1,3 МБ/с");
    }

    #[tokio::test]
    async fn asks_for_uv_and_for_room_before_it_touches_anything() {
        let root = tempfile::tempdir().unwrap();
        let plan = |platform: PlatformInfo| InstallPlan {
            engine: spec(EngineId::Omnivoice),
            platform,
            runtime_dir: root.path().join("runtime"),
            server_dir: root.path().join("clone-server"),
            bench_models: None,
            env: HashMap::new(),
            watch: WATCH,
        };
        let without_uv = install(plan(mac(None))).await;
        assert_eq!(without_uv.state, JobRunState::Error);
        assert!(without_uv.error.unwrap().starts_with("Нужен uv"));
        let full = install(plan(PlatformInfo { disk_free_mb: 2_048, ..mac(Some("uv".into())) })).await;
        assert_eq!(full.error.as_deref(), Some("На диске свободно 2 ГБ, а нужно около 6 ГБ."));
        assert!(!root.path().join("runtime").join("omnivoice").exists());
    }
}
