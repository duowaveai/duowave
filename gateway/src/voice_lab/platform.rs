//! What this computer is, for the own voices: whose engines can run here and on what. Asked once at start and again
//! on the panel's «Проверить снова»; every command goes through a Probe, so tests feed the outputs of three systems.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use futures::future::BoxFuture;
use serde::Serialize;

use super::js;
use crate::utils::time::now_iso;

/// How long a probe's command may take.
pub const PROBE_TIMEOUT_MS: u64 = 5_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum OsName {
    #[serde(rename = "darwin")]
    Darwin,
    #[serde(rename = "linux")]
    Linux,
    #[serde(rename = "win32")]
    Win32,
    #[serde(rename = "other")]
    Other,
}

impl OsName {
    pub fn as_str(self) -> &'static str {
        match self {
            OsName::Darwin => "darwin",
            OsName::Linux => "linux",
            OsName::Win32 => "win32",
            OsName::Other => "other",
        }
    }
}

/// Node's process.platform names: darwin, linux, win32 — anything else is `other`.
pub fn os_name(platform: &str) -> OsName {
    match platform {
        "darwin" => OsName::Darwin,
        "linux" => OsName::Linux,
        "win32" => OsName::Win32,
        _ => OsName::Other,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GpuKind {
    /// Apple Silicon (MPS).
    Apple,
    /// CUDA through nvidia-smi.
    Nvidia,
    /// The processor only.
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuInfo {
    pub kind: GpuKind,
    pub name: Option<String>,
    pub vram_mb: Option<u64>,
    pub driver: Option<String>,
}

impl GpuInfo {
    pub fn none() -> GpuInfo {
        GpuInfo { kind: GpuKind::None, name: None, vram_mb: None, driver: None }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tools {
    /// uv's path: it installs Python and the engines' packages on all three systems.
    pub uv: Option<String>,
    pub git: bool,
    /// macOS: per-process memory limits through the kernel (taskpolicy -m).
    pub taskpolicy: bool,
    /// Linux: memory limits through a systemd user scope.
    pub systemd_run: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlatformInfo {
    pub os: OsName,
    /// Node's names: arm64, x64.
    pub arch: String,
    pub ram_mb: u64,
    pub cpus: usize,
    pub cpu_name: Option<String>,
    /// The gateway runs in a container: no Python engines beside it.
    pub docker: bool,
    pub gpu: GpuInfo,
    /// Free disk where the engines are installed.
    pub disk_free_mb: u64,
    pub tools: Tools,
    pub checked_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandResult {
    /// The exit code; none when the program did not start, was killed or ran out of time.
    pub code: Option<i32>,
    pub stdout: String,
}

impl CommandResult {
    pub fn ok(&self) -> bool {
        self.code == Some(0)
    }
}

/// The outside world detection and the memory sampling ask: commands, files, the OS. Tests give their own.
pub trait Probe: Send + Sync {
    /// Node's process.platform: darwin, linux, win32, …
    fn platform(&self) -> String;
    /// Node's process.arch: arm64, x64, …
    fn arch(&self) -> String;
    fn env(&self, name: &str) -> Option<String>;
    fn run(&self, command: &str, args: &[&str], timeout_ms: u64) -> BoxFuture<'static, CommandResult>;
    fn exists(&self, path: &Path) -> bool;
    fn read(&self, path: &Path) -> Option<String>;
    fn total_mb(&self) -> u64;
    /// The physical memory free and in all, bytes (os.freemem(), os.totalmem()).
    fn memory_bytes(&self) -> (u64, u64);
    /// How many processors, and the first one's name.
    fn cpus(&self) -> (usize, Option<String>);
    fn free_disk_mb(&self, path: &Path) -> u64;
    fn home(&self) -> PathBuf;
}

/// The probe the voice lab asks, swappable: a test or a tool gives its own (Nest's overrideProvider did this).
#[derive(Clone)]
pub struct ProbeCell(Arc<RwLock<Arc<dyn Probe>>>);

impl ProbeCell {
    pub fn new(probe: Arc<dyn Probe>) -> ProbeCell {
        ProbeCell(Arc::new(RwLock::new(probe)))
    }

    pub fn get(&self) -> Arc<dyn Probe> {
        self.0.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    pub fn set(&self, probe: Arc<dyn Probe>) {
        *self.0.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = probe;
    }
}

const MB: f64 = 1_048_576.0;

/// Windows: no console window for a child.
#[cfg(windows)]
pub(crate) const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// This computer.
pub struct SystemProbe;

impl Probe for SystemProbe {
    fn platform(&self) -> String {
        match std::env::consts::OS {
            "macos" => "darwin".into(),
            "windows" => "win32".into(),
            other => other.into(),
        }
    }

    fn arch(&self) -> String {
        match std::env::consts::ARCH {
            "aarch64" => "arm64".into(),
            "x86_64" => "x64".into(),
            "x86" => "ia32".into(),
            other => other.into(),
        }
    }

    fn env(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }

    fn run(&self, command: &str, args: &[&str], timeout_ms: u64) -> BoxFuture<'static, CommandResult> {
        let mut child = tokio::process::Command::new(command);
        child.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
        #[cfg(windows)]
        child.creation_flags(CREATE_NO_WINDOW);
        Box::pin(async move {
            let failed = CommandResult { code: None, stdout: String::new() };
            let Ok(child) = child.spawn() else { return failed };
            match tokio::time::timeout(Duration::from_millis(timeout_ms), child.wait_with_output()).await {
                Ok(Ok(output)) => {
                    let stdout = &output.stdout[..output.stdout.len().min(1 << 20)];
                    CommandResult { code: output.status.code(), stdout: String::from_utf8_lossy(stdout).into_owned() }
                }
                // Out of time: dropped, so killed.
                _ => failed,
            }
        })
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn read(&self, path: &Path) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    fn total_mb(&self) -> u64 {
        js::round(self.memory_bytes().1 as f64 / MB) as u64
    }

    fn memory_bytes(&self) -> (u64, u64) {
        native::memory_bytes()
    }

    fn cpus(&self) -> (usize, Option<String>) {
        let count = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
        let model = std::fs::read_to_string("/proc/cpuinfo").ok().and_then(|info| {
            info.lines()
                .find(|line| line.starts_with("model name"))
                .and_then(|line| line.split_once(':'))
                .map(|(_, name)| name.trim().to_string())
        });
        (count, model)
    }

    fn free_disk_mb(&self, path: &Path) -> u64 {
        let mut place = path.to_path_buf();
        while !place.exists() {
            match place.parent() {
                Some(parent) if parent != place => place = parent.to_path_buf(),
                _ => break,
            }
        }
        js::round(native::free_disk_bytes(&place) as f64 / MB) as u64
    }

    fn home(&self) -> PathBuf {
        let name = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
        std::env::var_os(name).map(PathBuf::from).unwrap_or_default()
    }
}

/// The few numbers only the system itself gives: memory and free disk.
#[cfg(unix)]
mod native {
    use std::ffi::CString;
    use std::path::Path;

    pub fn memory_bytes() -> (u64, u64) {
        // SAFETY: sysconf only reads system constants.
        let (pages, page) = unsafe { (libc::sysconf(libc::_SC_PHYS_PAGES), libc::sysconf(libc::_SC_PAGESIZE)) };
        let total = (pages.max(0) as u64).saturating_mul(page.max(0) as u64);
        (free_bytes(page.max(0) as u64), total)
    }

    #[cfg(target_os = "macos")]
    fn free_bytes(page: u64) -> u64 {
        let mut free: u32 = 0;
        let mut size = std::mem::size_of::<u32>();
        let name = c"vm.page_free_count";
        // SAFETY: the name is a valid C string and the buffer is a u32 of the size given.
        let status = unsafe {
            libc::sysctlbyname(name.as_ptr(), (&mut free as *mut u32).cast(), &mut size, std::ptr::null_mut(), 0)
        };
        if status == 0 {
            free as u64 * page
        } else {
            0
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn free_bytes(page: u64) -> u64 {
        // What libuv took: MemAvailable, else the free pages.
        if let Ok(meminfo) = std::fs::read_to_string("/proc/meminfo") {
            let available = meminfo.lines().find_map(|line| {
                let rest = line.strip_prefix("MemAvailable:")?;
                rest.trim().trim_end_matches("kB").trim().parse::<u64>().ok()
            });
            if let Some(kb) = available {
                return kb * 1024;
            }
        }
        #[cfg(target_os = "linux")]
        {
            // SAFETY: sysconf only reads system counters.
            let pages = unsafe { libc::sysconf(libc::_SC_AVPHYS_PAGES) };
            pages.max(0) as u64 * page
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = page;
            0
        }
    }

    pub fn free_disk_bytes(path: &Path) -> u64 {
        use std::os::unix::ffi::OsStrExt;
        let Ok(name) = CString::new(path.as_os_str().as_bytes()) else { return 0 };
        #[cfg(target_os = "macos")]
        {
            let mut stats: libc::statfs = unsafe { std::mem::zeroed() };
            // SAFETY: a valid C string and a statfs to fill.
            if unsafe { libc::statfs(name.as_ptr(), &mut stats) } != 0 {
                return 0;
            }
            stats.f_bavail.saturating_mul(stats.f_bsize as u64)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
            // SAFETY: a valid C string and a statvfs to fill.
            if unsafe { libc::statvfs(name.as_ptr(), &mut stats) } != 0 {
                return 0;
            }
            (stats.f_bavail as u64).saturating_mul(stats.f_frsize as u64)
        }
    }
}

#[cfg(windows)]
mod native {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    pub fn memory_bytes() -> (u64, u64) {
        // SAFETY: a zeroed MEMORYSTATUSEX with its length set, as the call wants it.
        unsafe {
            let mut status: MEMORYSTATUSEX = std::mem::zeroed();
            status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
            if GlobalMemoryStatusEx(&mut status) == 0 {
                return (0, 0);
            }
            (status.ullAvailPhys, status.ullTotalPhys)
        }
    }

    pub fn free_disk_bytes(path: &Path) -> u64 {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut available: u64 = 0;
        // SAFETY: a NUL-terminated wide path and a u64 to fill; the totals are not asked.
        let ok =
            unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut available, std::ptr::null_mut(), std::ptr::null_mut()) };
        if ok == 0 {
            0
        } else {
            available
        }
    }
}

#[cfg(not(any(unix, windows)))]
mod native {
    pub fn memory_bytes() -> (u64, u64) {
        (0, 0)
    }

    pub fn free_disk_bytes(_path: &std::path::Path) -> u64 {
        0
    }
}

/// uv on PATH, or where its installers put it.
async fn find_uv(probe: &dyn Probe, os: OsName) -> Option<String> {
    let exe = if os == OsName::Win32 { "uv.exe" } else { "uv" };
    if probe.run("uv", &["--version"], PROBE_TIMEOUT_MS).await.ok() {
        return Some("uv".into());
    }
    let home = probe.home();
    let candidates: Vec<PathBuf> = if os == OsName::Win32 {
        vec![
            home.join(".local").join("bin").join(exe),
            PathBuf::from(probe.env("LOCALAPPDATA").unwrap_or_default()).join("uv").join(exe),
            home.join(".cargo").join("bin").join(exe),
        ]
    } else {
        vec![
            home.join(".local").join("bin").join(exe),
            home.join(".cargo").join("bin").join(exe),
            PathBuf::from("/opt/homebrew/bin/uv"),
            PathBuf::from("/usr/local/bin/uv"),
        ]
    };
    candidates.into_iter().find(|path| probe.exists(path)).map(|path| path.to_string_lossy().into_owned())
}

/// The first NVIDIA card as nvidia-smi names it: `NVIDIA GeForce RTX 4070, 12282, 550.54.14`.
pub fn parse_nvidia_smi(stdout: &str) -> Option<GpuInfo> {
    let line = stdout.split('\n').find(|row| !js::trim(row).is_empty())?;
    let mut parts = line.split(',').map(js::trim);
    let name = parts.next().filter(|name| !name.is_empty())?;
    // Number(''): 0.
    let vram_mb = parts
        .next()
        .and_then(|memory| if memory.is_empty() { Some(0.0) } else { memory.parse::<f64>().ok() })
        .filter(|mb| mb.is_finite() && *mb >= 0.0);
    let driver = parts.next().filter(|driver| !driver.is_empty());
    Some(GpuInfo {
        kind: GpuKind::Nvidia,
        name: Some(name.to_string()),
        vram_mb: vram_mb.map(|mb| mb as u64),
        driver: driver.map(str::to_string),
    })
}

fn in_docker(probe: &dyn Probe, os: OsName) -> bool {
    if os != OsName::Linux {
        return false;
    }
    if probe.exists(Path::new("/.dockerenv")) || probe.exists(Path::new("/run/.containerenv")) {
        return true;
    }
    let cgroup = probe.read(Path::new("/proc/1/cgroup")).unwrap_or_default();
    ["docker", "containerd", "kubepods", "libpod"].iter().any(|name| cgroup.contains(name))
}

pub async fn detect_platform(probe: &dyn Probe, runtime_dir: &Path) -> PlatformInfo {
    let os = os_name(&probe.platform());
    let arch = probe.arch();
    let (cpus, model) = probe.cpus();
    let mut gpu = GpuInfo::none();
    let mut cpu_name = model.filter(|name| !name.is_empty());
    match os {
        OsName::Darwin => {
            let brand = probe.run("sysctl", &["-n", "machdep.cpu.brand_string"], PROBE_TIMEOUT_MS).await;
            let brand = js::trim(&brand.stdout).to_string();
            if !brand.is_empty() {
                cpu_name = Some(brand.clone());
            }
            if arch == "arm64" {
                let name = if brand.is_empty() { "Apple Silicon".to_string() } else { brand };
                gpu = GpuInfo { kind: GpuKind::Apple, name: Some(name), vram_mb: None, driver: None };
            }
        }
        OsName::Linux | OsName::Win32 => {
            let query = ["--query-gpu=name,memory.total,driver_version", "--format=csv,noheader,nounits"];
            let smi = probe.run("nvidia-smi", &query, PROBE_TIMEOUT_MS).await;
            if smi.ok() {
                gpu = parse_nvidia_smi(&smi.stdout).unwrap_or(gpu);
            }
        }
        OsName::Other => {}
    }
    let (uv, git, systemd_run) = tokio::join!(
        find_uv(probe, os),
        async { probe.run("git", &["--version"], PROBE_TIMEOUT_MS).await.ok() },
        async {
            os == OsName::Linux
                && probe.run("systemd-run", &["--user", "--scope", "--quiet", "true"], PROBE_TIMEOUT_MS).await.ok()
        },
    );
    PlatformInfo {
        os,
        arch,
        ram_mb: probe.total_mb(),
        cpus,
        cpu_name,
        docker: in_docker(probe, os),
        gpu,
        disk_free_mb: probe.free_disk_mb(runtime_dir),
        tools: Tools {
            uv,
            git,
            taskpolicy: os == OsName::Darwin && probe.exists(Path::new("/usr/sbin/taskpolicy")),
            systemd_run,
        },
        checked_at: now_iso(),
    }
}

/// «Мак, Apple M4 Max, 36 ГБ» — the panel's line about this computer.
pub fn describe_platform(platform: &PlatformInfo) -> String {
    let os = match platform.os {
        OsName::Darwin => "Мак",
        OsName::Linux => "Linux",
        OsName::Win32 => "Windows",
        OsName::Other => "other",
    };
    let gpu = match platform.gpu.kind {
        GpuKind::Apple => platform.gpu.name.clone().unwrap_or_else(|| "Apple Silicon".into()),
        GpuKind::Nvidia => {
            let name = platform.gpu.name.clone().unwrap_or_default();
            match platform.gpu.vram_mb.filter(|mb| *mb > 0) {
                Some(mb) => format!("{name}, {} ГБ видеопамяти", js::round_i64(mb as f64 / 1024.0)),
                None => name,
            }
        }
        GpuKind::None => match &platform.cpu_name {
            Some(cpu) => format!("{cpu}, без видеокарты NVIDIA"),
            None => "без видеокарты NVIDIA".into(),
        },
    };
    let docker = if platform.docker { " (Docker)" } else { "" };
    format!("{os}{docker}, {gpu}, {} ГБ памяти", js::round_i64(platform.ram_mb as f64 / 1024.0))
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::voice_lab::clone_engines::{flavour, spec, verdict, Device, EngineId, Flavour, Verdict};
    use crate::voice_lab::engine_installer::torch_packages;

    /// A computer as detection sees it: which commands answer what, which files exist.
    pub(crate) struct Computer {
        pub platform: &'static str,
        pub arch: &'static str,
        pub ram_mb: u64,
        /// By the command line's start; None: the command fails.
        pub answers: Vec<(&'static str, Option<&'static str>)>,
        pub files: HashMap<String, String>,
    }

    impl Computer {
        pub fn new(platform: &'static str) -> Computer {
            Computer { platform, arch: "x64", ram_mb: 32_768, answers: Vec::new(), files: HashMap::new() }
        }

        pub fn arch(mut self, arch: &'static str) -> Computer {
            self.arch = arch;
            self
        }

        pub fn ram(mut self, mb: u64) -> Computer {
            self.ram_mb = mb;
            self
        }

        pub fn answer(mut self, prefix: &'static str, stdout: Option<&'static str>) -> Computer {
            self.answers.push((prefix, stdout));
            self
        }

        pub fn file(mut self, path: &str, text: &str) -> Computer {
            self.files.insert(path.to_string(), text.to_string());
            self
        }
    }

    impl Probe for Computer {
        fn platform(&self) -> String {
            self.platform.into()
        }

        fn arch(&self) -> String {
            self.arch.into()
        }

        fn env(&self, _name: &str) -> Option<String> {
            None
        }

        fn run(&self, command: &str, args: &[&str], _timeout_ms: u64) -> BoxFuture<'static, CommandResult> {
            let key = std::iter::once(command).chain(args.iter().copied()).collect::<Vec<_>>().join(" ");
            let found = self.answers.iter().find(|(prefix, _)| key.starts_with(prefix));
            let result = match found {
                Some((_, Some(stdout))) => CommandResult { code: Some(0), stdout: stdout.to_string() },
                _ => CommandResult { code: None, stdout: String::new() },
            };
            Box::pin(async move { result })
        }

        fn exists(&self, path: &Path) -> bool {
            self.files.contains_key(&path.to_string_lossy().into_owned())
        }

        fn read(&self, path: &Path) -> Option<String> {
            self.files.get(&path.to_string_lossy().into_owned()).cloned()
        }

        fn total_mb(&self) -> u64 {
            self.ram_mb
        }

        fn memory_bytes(&self) -> (u64, u64) {
            (self.ram_mb * 1_048_576 / 2, self.ram_mb * 1_048_576)
        }

        fn cpus(&self) -> (usize, Option<String>) {
            (8, Some("Some CPU".into()))
        }

        fn free_disk_mb(&self, _path: &Path) -> u64 {
            80_000
        }

        fn home(&self) -> PathBuf {
            PathBuf::from("/home/owner")
        }
    }

    pub(crate) fn mac() -> Computer {
        Computer::new("darwin")
            .arch("arm64")
            .ram(36_864)
            .answer("sysctl -n machdep.cpu.brand_string", Some("Apple M4 Max\n"))
            .answer("uv --version", Some("uv 0.9.0"))
            .answer("git --version", Some("git version 2.50"))
            .file("/usr/sbin/taskpolicy", "")
    }

    fn linux_nvidia() -> Computer {
        Computer::new("linux")
            .answer("nvidia-smi", Some("NVIDIA GeForce RTX 4070, 12282, 550.54.14\n"))
            .answer("git --version", Some("git version 2.43"))
            .answer("systemd-run --user --scope", Some(""))
            .file("/home/owner/.local/bin/uv", "")
    }

    async fn detect(probe: &Computer) -> PlatformInfo {
        detect_platform(probe, Path::new("/runtime")).await
    }

    #[tokio::test]
    async fn sees_an_apple_silicon_mac_its_chip_the_kernel_memory_limit_uv_and_git() {
        let info = detect(&mac()).await;
        assert_eq!(info.os, OsName::Darwin);
        assert_eq!(info.arch, "arm64");
        assert_eq!(info.ram_mb, 36_864);
        assert!(!info.docker);
        assert_eq!(info.gpu.kind, GpuKind::Apple);
        assert_eq!(info.gpu.name.as_deref(), Some("Apple M4 Max"));
        assert_eq!(info.disk_free_mb, 80_000);
        assert_eq!(info.tools, Tools { uv: Some("uv".into()), git: true, taskpolicy: true, systemd_run: false });
        assert_eq!(describe_platform(&info), "Мак, Apple M4 Max, 36 ГБ памяти");
    }

    #[tokio::test]
    async fn sees_linux_with_an_nvidia_card_uv_where_its_installer_puts_it_and_a_systemd_user_scope() {
        let info = detect(&linux_nvidia()).await;
        assert_eq!(
            info.gpu,
            GpuInfo {
                kind: GpuKind::Nvidia,
                name: Some("NVIDIA GeForce RTX 4070".into()),
                vram_mb: Some(12_282),
                driver: Some("550.54.14".into()),
            }
        );
        assert_eq!(
            info.tools,
            Tools { uv: Some("/home/owner/.local/bin/uv".into()), git: true, taskpolicy: false, systemd_run: true }
        );
        assert_eq!(describe_platform(&info), "Linux, NVIDIA GeForce RTX 4070, 12 ГБ видеопамяти, 32 ГБ памяти");
    }

    #[tokio::test]
    async fn knows_a_container_by_dockerenv_or_its_cgroup() {
        assert!(detect(&Computer::new("linux").file("/.dockerenv", "")).await.docker);
        assert!(detect(&Computer::new("linux").file("/proc/1/cgroup", "0::/docker/abc")).await.docker);
        assert!(!detect(&Computer::new("linux").file("/proc/1/cgroup", "0::/")).await.docker);
    }

    #[test]
    fn reads_nvidia_smi_and_refuses_an_empty_answer() {
        assert_eq!(parse_nvidia_smi(""), None);
        let two = "NVIDIA RTX A4000, 16376, 535.1\nNVIDIA RTX A4000, 16376, 535.1\n";
        assert_eq!(parse_nvidia_smi(two).and_then(|gpu| gpu.vram_mb), Some(16_376));
    }

    async fn at(probe: &Computer) -> (Verdict, Verdict, PlatformInfo) {
        let info = detect(probe).await;
        (verdict(spec(EngineId::Omnivoice), &info), verdict(spec(EngineId::Cosyvoice3), &info), info)
    }

    fn runs(device: Device, imprint: Device, slow: bool) -> Verdict {
        Verdict { supported: true, device: Some(device), imprint_device: Some(imprint), slow, reason: None }
    }

    #[tokio::test]
    async fn apple_silicon_omnivoice_speaks_on_mps_imprints_on_the_cpu_cosyvoice_on_the_cpu_slowly() {
        let (omnivoice, cosyvoice3, info) = at(&mac()).await;
        assert_eq!(omnivoice, runs(Device::Mps, Device::Cpu, false));
        assert_eq!(cosyvoice3, runs(Device::Cpu, Device::Cpu, true));
        // VoxCPM speaks on the card like OmniVoice — in float32, which its code insists on for MPS.
        assert_eq!(verdict(spec(EngineId::Voxcpm), &info), runs(Device::Mps, Device::Cpu, false));
    }

    #[tokio::test]
    async fn an_intel_mac_neither_no_pytorch_past_2_2_for_it() {
        let (omnivoice, cosyvoice3, _) = at(&Computer::new("darwin").arch("x64")).await;
        assert!(!omnivoice.supported);
        assert!(omnivoice.reason.unwrap().contains("Intel"));
        assert!(!cosyvoice3.supported);
    }

    #[tokio::test]
    async fn linux_cuda_with_nvidia_the_cpu_slow_without() {
        assert_eq!(at(&linux_nvidia()).await.1, runs(Device::Cuda, Device::Cuda, false));
        assert_eq!(at(&Computer::new("linux").arch("arm64")).await.0, runs(Device::Cpu, Device::Cpu, true));
    }

    #[tokio::test]
    async fn windows_omnivoice_with_nvidia_only_cosyvoice_not_at_all() {
        let windows = Computer::new("win32").answer("nvidia-smi", Some("NVIDIA GeForce RTX 3060, 12288, 560.0\n"));
        let (omnivoice, cosyvoice3, _) = at(&windows).await;
        assert_eq!(omnivoice, runs(Device::Cuda, Device::Cuda, false));
        assert!(!cosyvoice3.supported);
        assert_eq!(at(&Computer::new("win32")).await.0.reason.as_deref(), Some("На Windows нужна видеокарта NVIDIA."));
    }

    #[tokio::test]
    async fn a_container_or_a_small_computer_no() {
        let docker = at(&Computer::new("linux").file("/.dockerenv", "")).await.0;
        assert!(docker.reason.unwrap().contains("Docker"));
        let small = at(&Computer::new("linux").ram(4_096)).await.0;
        assert!(small.reason.unwrap().contains("от 8 ГБ"));
    }

    #[tokio::test]
    async fn takes_pytorch_from_pypi_on_a_mac_the_cuda_index_with_nvidia_the_small_cpu_wheels_otherwise() {
        let mac = detect(&mac()).await;
        let nvidia = detect(&linux_nvidia()).await;
        let cpu = PlatformInfo { gpu: GpuInfo::none(), ..nvidia.clone() };
        assert_eq!(flavour(&mac), Flavour::Mac);
        assert_eq!(torch_packages(spec(EngineId::Omnivoice), &mac), vec!["torch==2.8.0", "torchaudio==2.8.0"]);
        assert_eq!(
            torch_packages(spec(EngineId::Omnivoice), &nvidia),
            vec!["torch==2.8.0", "torchaudio==2.8.0", "--index-url", "https://download.pytorch.org/whl/cu126"]
        );
        let cosy = torch_packages(spec(EngineId::Cosyvoice3), &nvidia);
        assert_eq!(cosy.last().map(String::as_str), Some("https://download.pytorch.org/whl/cu121"));
        let small = torch_packages(spec(EngineId::Omnivoice), &cpu);
        assert_eq!(small.last().map(String::as_str), Some("https://download.pytorch.org/whl/cpu"));
    }
}
