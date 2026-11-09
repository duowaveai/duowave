//! The engines a voice imprint can be taken with, and where each can run. Zero-shot all: no training, a few
//! seconds of the owner's speech and their words become the voice. Measured on the bench and on an M4 Max
//! (clone-server/README.md).

use serde::{Deserialize, Serialize};

use super::platform::{GpuKind, OsName, PlatformInfo};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum EngineId {
    #[serde(rename = "omnivoice")]
    Omnivoice,
    #[serde(rename = "cosyvoice3")]
    Cosyvoice3,
    #[serde(rename = "voxcpm")]
    Voxcpm,
}

pub const ENGINE_IDS: [EngineId; 3] = [EngineId::Omnivoice, EngineId::Cosyvoice3, EngineId::Voxcpm];

impl EngineId {
    pub fn as_str(self) -> &'static str {
        match self {
            EngineId::Omnivoice => "omnivoice",
            EngineId::Cosyvoice3 => "cosyvoice3",
            EngineId::Voxcpm => "voxcpm",
        }
    }

    pub fn parse(value: &str) -> Option<EngineId> {
        ENGINE_IDS.into_iter().find(|engine| engine.as_str() == value)
    }
}

impl std::fmt::Display for EngineId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Device {
    Mps,
    Cuda,
    Cpu,
}

impl Device {
    pub fn as_str(self) -> &'static str {
        match self {
            Device::Mps => "mps",
            Device::Cuda => "cuda",
            Device::Cpu => "cpu",
        }
    }
}

/// Megabytes on disk after an install, by what the machine gets.
#[derive(Debug, Clone, Copy)]
pub struct DiskMb {
    pub mac: u64,
    pub cuda: u64,
    pub cpu: u64,
}

/// Memory to serve a voice, megabytes, by where it speaks.
#[derive(Debug, Clone, Copy)]
pub struct ServeMb {
    pub mps: u64,
    pub cuda: u64,
    pub cpu: u64,
}

impl ServeMb {
    pub fn on(&self, device: Device) -> u64 {
        match device {
            Device::Mps => self.mps,
            Device::Cuda => self.cuda,
            Device::Cpu => self.cpu,
        }
    }
}

/// Memory before the first measurement, megabytes; afterwards the measured peak decides.
#[derive(Debug, Clone, Copy)]
pub struct NeedMb {
    pub imprint: u64,
    pub serve: ServeMb,
}

#[derive(Debug, Clone)]
pub struct EngineSpec {
    pub id: EngineId,
    pub title: &'static str,
    pub about: &'static str,
    pub license: &'static str,
    pub recommended: bool,
    /// The Python its environment is made with.
    pub python: &'static str,
    pub torch: &'static str,
    /// PyTorch's CUDA wheel index for NVIDIA machines.
    pub cuda_index: &'static str,
    pub disk_mb: DiskMb,
    /// The weights' download, MB (measured on the bench, 06.11): the install shows its progress against it.
    pub weights_mb: u64,
    pub need_mb: NeedMb,
    /// The server's default port.
    pub port: u16,
    /// The weights' revision this gateway installs (clone-server/README.md, requirements): a voice snapshot taken
    /// with other weights is taken again from its reference.
    pub revision: &'static str,
}

/// The packages an install downloads beside the weights, MB (clone-server/README.md: about 1 GB).
pub const PACKAGES_MB: u64 = 1_024;

/// Whether two revisions name the same weights: one is the other's beginning (a short commit and a full one).
pub fn same_revision(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim().to_ascii_lowercase(), b.trim().to_ascii_lowercase());
    let shorter = a.len().min(b.len());
    shorter >= 7 && a[..shorter] == b[..shorter]
}

pub static ENGINES: [EngineSpec; 3] = [
    EngineSpec {
        id: EngineId::Omnivoice,
        title: "OmniVoice",
        about: "Быстрее и ближе к голосу. Говорит на видеокарте: Apple Silicon или NVIDIA.",
        // The code is Apache-2.0; the weights are CC BY-NC 4.0 (trained on Emilia): at home, never for money.
        license: "код Apache-2.0 · веса CC BY-NC 4.0, некоммерчески",
        recommended: true,
        python: "3.11",
        torch: "2.8.0",
        cuda_index: "cu126",
        disk_mb: DiskMb { mac: 5_600, cuda: 9_000, cpu: 5_000 },
        weights_mb: 3_110,
        // Imprint on the CPU: 3.0 GB peak against 6.5 GB on MPS (06.11, M4 Max). Serving on MPS: 4.1 GB loaded,
        // 5.4 GB after two phrases (06.11) — the default until the server reports its own peak (/health) and it is
        // learned.
        need_mb: NeedMb { imprint: 3_500, serve: ServeMb { mps: 6_500, cuda: 2_500, cpu: 1_500 } },
        port: 8093,
        // k2-fsa/OmniVoice as the bench runs it.
        revision: "c5fdb5cc",
    },
    EngineSpec {
        id: EngineId::Cosyvoice3,
        title: "CosyVoice 3",
        about: "Хорошо держит русский. На Маке считает процессором — медленнее; быстро — на NVIDIA.",
        // Apache-2.0, but the model's page says academic use only.
        license: "Apache-2.0 · авторы модели: для академических целей",
        recommended: false,
        python: "3.10",
        torch: "2.3.1",
        cuda_index: "cu121",
        disk_mb: DiskMb { mac: 10_500, cuda: 13_000, cpu: 10_000 },
        weights_mb: 9_300,
        // Loading alone passed 6.5 GB on the CPU (06.11): the first measured run corrects these.
        need_mb: NeedMb { imprint: 8_000, serve: ServeMb { mps: 8_000, cuda: 4_000, cpu: 8_000 } },
        port: 8094,
        // FunAudioLLM/CosyVoice at the commit the installer fetches (engine_installer::COSYVOICE_COMMIT).
        revision: "074ca6dc",
    },
    EngineSpec {
        id: EngineId::Voxcpm,
        title: "VoxCPM 2",
        about: "Новая модель OpenBMB: 30 языков, русский в их числе. Apache-2.0 целиком — код и веса.",
        // Apache-2.0 both: the only one of the three whose weights can be used for money too.
        license: "Apache-2.0 · код и веса, можно коммерчески",
        recommended: false,
        python: "3.11",
        torch: "2.8.0",
        cuda_index: "cu126",
        // The wheel declares gradio, funasr and modelscope: the environment is heavier than the weights suggest.
        disk_mb: DiskMb { mac: 9_500, cuda: 14_000, cpu: 9_000 },
        weights_mb: 5_200,
        // VoxCPM2 is 2.3 B; on MPS the model insists on float32 (half precision glitches), so speaking takes twice
        // OmniVoice. Measured on an M4 Max (09.11): the imprint on the CPU peaks at 11.9 GB — loading passes through
        // a float32 copy before settling at 7.5 GB, RTF 9.4; serving on MPS holds 11.8 GB with a 14.3 GB peak,
        // RTF 1.3. CUDA keeps the estimate (the card runs bf16, ~8 GB).
        need_mb: NeedMb { imprint: 12_500, serve: ServeMb { mps: 15_000, cuda: 8_000, cpu: 8_000 } },
        port: 8095,
        // openbmb/VoxCPM2 on HuggingFace (the model and its AudioVAE).
        revision: "7936080b",
    },
];

/// An engine's spec.
pub fn spec(id: EngineId) -> &'static EngineSpec {
    match id {
        EngineId::Omnivoice => &ENGINES[0],
        EngineId::Cosyvoice3 => &ENGINES[1],
        EngineId::Voxcpm => &ENGINES[2],
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Verdict {
    pub supported: bool,
    /// Where the voice speaks.
    pub device: Option<Device>,
    /// Where the imprint is taken: never MPS — on the CPU it takes half the memory; CUDA keeps it in video memory.
    pub imprint_device: Option<Device>,
    /// The processor only: phrases take several times their length.
    pub slow: bool,
    pub reason: Option<String>,
}

fn no(reason: &str) -> Verdict {
    Verdict { supported: false, reason: Some(reason.to_string()), ..Verdict::default() }
}

fn yes(device: Device, imprint_device: Device, slow: bool) -> Verdict {
    Verdict { supported: true, device: Some(device), imprint_device: Some(imprint_device), slow, reason: None }
}

/// Whether an engine runs on this computer, and on what (the table in clone-server/README.md).
pub fn verdict(engine: &EngineSpec, platform: &PlatformInfo) -> Verdict {
    if platform.docker {
        return no("Шлюз запущен в Docker: модели голоса нужно ставить рядом с ним — пока не поддерживается.");
    }
    if platform.ram_mb < 8_000 {
        return no("Мало памяти: нужно от 8 ГБ.");
    }
    let nvidia = platform.gpu.kind == GpuKind::Nvidia;
    match platform.os {
        OsName::Darwin => {
            if platform.arch != "arm64" {
                return no("Мак на Intel: PyTorch новее 2.2 для него не выходит, а модели нужен 2.4 и выше.");
            }
            if engine.id == EngineId::Cosyvoice3 {
                yes(Device::Cpu, Device::Cpu, true)
            } else {
                // OmniVoice and VoxCPM speak on the video card; the imprint is taken on the CPU, where it takes
                // half the memory (for VoxCPM also because MPS forces float32).
                yes(Device::Mps, Device::Cpu, false)
            }
        }
        OsName::Linux => {
            if nvidia {
                yes(Device::Cuda, Device::Cuda, false)
            } else {
                yes(Device::Cpu, Device::Cpu, true)
            }
        }
        OsName::Win32 => {
            if !nvidia {
                return no("На Windows нужна видеокарта NVIDIA.");
            }
            if engine.id == EngineId::Cosyvoice3 {
                return no("CosyVoice на Windows ставится ненадёжно — возьмите OmniVoice.");
            }
            yes(Device::Cuda, Device::Cuda, false)
        }
        OsName::Other => no("Эта система не поддерживается: нужен Мак на Apple Silicon, Linux или Windows с NVIDIA."),
    }
}

/// Which install the machine gets: the Mac's wheels, CUDA ones, or the CPU ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavour {
    Mac,
    Cuda,
    Cpu,
}

pub fn flavour(platform: &PlatformInfo) -> Flavour {
    if platform.os == OsName::Darwin {
        return Flavour::Mac;
    }
    if platform.gpu.kind == GpuKind::Nvidia {
        Flavour::Cuda
    } else {
        Flavour::Cpu
    }
}

impl DiskMb {
    pub fn of(&self, flavour: Flavour) -> u64 {
        match flavour {
            Flavour::Mac => self.mac,
            Flavour::Cuda => self.cuda,
            Flavour::Cpu => self.cpu,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_weights_revision_of_each_engine() {
        assert!(same_revision(spec(EngineId::Cosyvoice3).revision, super::super::engine_installer::COSYVOICE_COMMIT));
        assert!(same_revision("C5FDB5CC", spec(EngineId::Omnivoice).revision));
        assert!(same_revision("7936080B", spec(EngineId::Voxcpm).revision));
        assert!(!same_revision("c5fdb5cd", "c5fdb5cc"));
        assert!(!same_revision("c5f", "c5fdb5cc"), "too short to tell");
    }
}
