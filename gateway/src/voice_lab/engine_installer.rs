//! An engine installed by one button, the same way on macOS, Linux and Windows: uv makes the environment and installs
//! the pinned packages (clone-server/requirements), PyTorch comes from the index that suits the machine, the weights
//! from HuggingFace. A machine with the bench (../self-voice-tests) gets its installation copied instead: on a Mac an
//! APFS clone — no download and no disk spent. ready.json, written last, is what makes an engine «installed».

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::clone_engines::{flavour, verdict, EngineId, EngineSpec, Flavour};
use super::jobs::{JobContext, JobError, RunOptions};
use super::js::{self, gb};
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

/// A long download, seen by the size of its folder against the size it will have: the weights are gigabytes, and a
/// bar standing still for minutes looks like a hang. «Скачиваю веса модели · 1,2 из ~3,0 ГБ».
async fn sized_download<T>(
    context: &JobContext,
    dir: &Path,
    expected_mb: u64,
    label: &str,
    (from, to): (f64, f64),
    run: impl std::future::Future<Output = T>,
) -> T {
    let show = || {
        let mb = folder_bytes(dir) as f64 / 1_048_576.0;
        let done = (mb / expected_mb as f64).min(1.0);
        context.stage(
            "weights",
            &format!("{label} · {} из ~{} ГБ", gb(mb), gb(expected_mb as f64)),
            Some(from + (to - from) * done),
        );
    };
    show();
    let period = Duration::from_secs(2);
    let mut timer = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
    tokio::pin!(run);
    loop {
        tokio::select! {
            result = &mut run => return result,
            _ = timer.tick() => show(),
        }
    }
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
            return Err("Остановлено.".into());
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
    Err(format!("не скачался {}", tail.join("/")).into())
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
    context.stage("check", "Проверяю компьютер", Some(2.0));
    let fits = verdict(engine, platform);
    if !fits.supported {
        return Err(fits.reason.unwrap_or_else(|| "не поддерживается".into()).into());
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
            "Нужен uv — менеджер Python. Установите: powershell -c \"irm https://astral.sh/uv/install.ps1 | iex\" — и \
             нажмите ещё раз."
        } else {
            "Нужен uv — менеджер Python. Установите: curl -LsSf https://astral.sh/uv/install.sh | sh (или brew install \
             uv) — и нажмите ещё раз."
        }
        .into());
    }
    if copy_from.is_none() && engine.id == EngineId::Cosyvoice3 && !platform.tools.git {
        return Err("Для CosyVoice нужен git.".into());
    }
    let need_mb = if copy_from.is_some() && platform.os == OsName::Darwin {
        0
    } else {
        engine.disk_mb.of(flavour(platform)) + 1_024
    };
    if platform.disk_free_mb < need_mb {
        return Err(format!(
            "На диске свободно {} ГБ, а нужно около {} ГБ.",
            js::round_i64(platform.disk_free_mb as f64 / 1024.0),
            js::round_i64(need_mb as f64 / 1024.0)
        )
        .into());
    }

    // A failed install leaves a folder without ready.json: it starts over.
    match std::fs::remove_dir_all(&target) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string().into()),
    }
    std::fs::create_dir_all(&target).map_err(|error| error.to_string())?;
    let mut env = plan.env.clone();
    env.insert("HF_HOME".into(), arg(&target.join("hf-cache")));
    env.insert("HF_HUB_DISABLE_TELEMETRY".into(), "1".into());
    env.insert("PYTHONUNBUFFERED".into(), "1".into());
    let with_env = || RunOptions { env: Some(env.clone()), ..RunOptions::default() };

    let source = if let Some((bench, parts)) = &copy_from {
        context.stage("copy", "Копирую готовую установку со стенда", Some(10.0));
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
        context.stage("python", &format!("Ставлю Python {}", engine.python), Some(5.0));
        let venv = vec!["venv".into(), "--python".into(), engine.python.into(), arg(&target.join("env"))];
        context.run(&uv, &venv, with_env()).await?;
        context.stage("torch", "Ставлю PyTorch", Some(10.0));
        let mut torch = strings(&["pip", "install", "-p", &python]);
        torch.extend(torch_packages(engine, platform));
        context.run(&uv, &torch, with_env()).await?;
        if engine.id == EngineId::Cosyvoice3 {
            context.stage("repo", "Скачиваю CosyVoice", Some(30.0));
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
        context.stage("packages", "Ставлю библиотеки", Some(40.0));
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
            context.stage("wetext", "Скачиваю словари нормализации текста", Some(55.0));
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
        sized_download(
            context,
            &watched,
            engine.weights_mb,
            "Скачиваю веса модели",
            (60.0, 88.0),
            context.run(&python, &args, with_env()),
        )
        .await?;
        InstallSource::Download
    };

    context.stage("verify", "Проверяю установку", Some(90.0));
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
        for _ in 0..400 {
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
            tools: Tools { uv, git: true, taskpolicy: false, systemd_run: false },
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
        };
        let state = install(plan).await;
        assert_eq!(state.state, JobRunState::Done, "{state:?}");
        let target = root.path().join("runtime").join("omnivoice");
        assert!(target.join("hf-cache").join("weights.bin").exists());
        let ready: ReadyInfo =
            serde_json::from_str(&std::fs::read_to_string(target.join("ready.json")).unwrap()).unwrap();
        assert_eq!(ready.source, InstallSource::Bench);
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
        };
        let without_uv = install(plan(mac(None))).await;
        assert_eq!(without_uv.state, JobRunState::Error);
        assert!(without_uv.error.unwrap().starts_with("Нужен uv"));
        let full = install(plan(PlatformInfo { disk_free_mb: 2_048, ..mac(Some("uv".into())) })).await;
        assert_eq!(full.error.as_deref(), Some("На диске свободно 2 ГБ, а нужно около 6 ГБ."));
        assert!(!root.path().join("runtime").join("omnivoice").exists());
    }
}
