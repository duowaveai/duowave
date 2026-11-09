"""Memory and lifetime guards for the clone server and the imprint job: the same code on macOS, Linux and Windows.

A process measures its own memory the way its system counts it — macOS phys_footprint (GPU memory included: RSS
misses it, 128 MB against 4 GB on our MPS recognition server), Linux VmRSS, Windows private bytes — and leaves with
a clear reason when it outgrows its limit. The allocator gets the same ceiling (MPS or CUDA fraction), so a model
asking for too much fails inside torch before the system starts to swap. Where the kernel can enforce a limit, the
gateway starts the process under it as well (taskpolicy -m on macOS, a systemd scope on Linux); on Windows the
process puts itself into a Job Object.
"""
from __future__ import annotations

import ctypes
import json
import os
import sys
import threading
import time
from typing import Callable, Optional

MB = 1024 * 1024


def _darwin_rusage() -> Optional[ctypes.Array]:
    """proc_pid_rusage(RUSAGE_INFO_V4): a 16-byte uuid, then uint64 fields."""
    libproc = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
    buffer = (ctypes.c_uint64 * 64)()
    if libproc.proc_pid_rusage(os.getpid(), 4, ctypes.byref(buffer)) != 0:
        return None
    return buffer


def _darwin_footprint() -> int:
    """ri_phys_footprint: what Activity Monitor calls Memory, GPU memory included."""
    usage = _darwin_rusage()
    return int(usage[2 + 7]) if usage is not None else 0


def _darwin_peak() -> int:
    """ri_lifetime_max_phys_footprint: the exact peak, spikes between our samples included."""
    usage = _darwin_rusage()
    return int(usage[2 + 28]) if usage is not None else 0


def _linux_status(field: str) -> int:
    try:
        with open("/proc/self/status", encoding="ascii") as status:
            for line in status:
                if line.startswith(field):
                    return int(line.split()[1]) * 1024
    except OSError:
        pass
    return 0


def _linux_rss() -> int:
    return _linux_status("VmRSS:")


class _ProcessMemoryCountersEx(ctypes.Structure):
    _fields_ = [
        ("cb", ctypes.c_uint32),
        ("PageFaultCount", ctypes.c_uint32),
        ("PeakWorkingSetSize", ctypes.c_size_t),
        ("WorkingSetSize", ctypes.c_size_t),
        ("QuotaPeakPagedPoolUsage", ctypes.c_size_t),
        ("QuotaPagedPoolUsage", ctypes.c_size_t),
        ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t),
        ("QuotaNonPagedPoolUsage", ctypes.c_size_t),
        ("PagefileUsage", ctypes.c_size_t),
        ("PeakPagefileUsage", ctypes.c_size_t),
        ("PrivateUsage", ctypes.c_size_t),
    ]


def _windows_counters() -> Optional[_ProcessMemoryCountersEx]:
    counters = _ProcessMemoryCountersEx()
    counters.cb = ctypes.sizeof(counters)
    kernel32 = ctypes.windll.kernel32  # type: ignore[attr-defined]
    kernel32.GetCurrentProcess.restype = ctypes.c_void_p
    psapi = ctypes.windll.psapi  # type: ignore[attr-defined]
    psapi.GetProcessMemoryInfo.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_uint32]
    if not psapi.GetProcessMemoryInfo(kernel32.GetCurrentProcess(), ctypes.byref(counters), counters.cb):
        return None
    return counters


def _windows_private() -> int:
    counters = _windows_counters()
    return int(counters.PrivateUsage) if counters else 0


def peak_memory() -> int:
    """The process's peak as its system kept it (macOS lifetime max footprint, Linux VmHWM, Windows peak private)."""
    try:
        if sys.platform == "darwin":
            return _darwin_peak()
        if sys.platform.startswith("linux"):
            return _linux_status("VmHWM:")
        if sys.platform == "win32":
            counters = _windows_counters()
            return int(counters.PeakPagefileUsage) if counters else 0
    except Exception:
        return 0
    return 0


def own_memory() -> int:
    """This process's memory in bytes, as its system counts it; 0 when it cannot tell."""
    try:
        if sys.platform == "darwin":
            return _darwin_footprint()
        if sys.platform.startswith("linux"):
            return _linux_rss()
        if sys.platform == "win32":
            return _windows_private()
    except Exception:  # a guard that cannot measure must not take the process down
        return 0
    return 0


def gpu_memory(torch_module) -> int:
    """CUDA memory torch holds (video memory is not in RSS); MPS memory is already in macOS phys_footprint."""
    try:
        if torch_module is not None and torch_module.cuda.is_available():
            return int(torch_module.cuda.memory_reserved())
    except Exception:
        pass
    return 0


def cap_allocator(torch_module, device: str, limit_bytes: Optional[int]) -> None:
    """The allocator's own ceiling: past it torch raises out-of-memory instead of growing into swap."""
    if not limit_bytes or torch_module is None:
        return
    try:
        if device == "mps" and hasattr(torch_module.mps, "recommended_max_memory"):
            fraction = limit_bytes / max(1, torch_module.mps.recommended_max_memory())
            torch_module.mps.set_per_process_memory_fraction(max(0.05, min(1.0, fraction)))
        elif device == "cuda":
            total = torch_module.cuda.get_device_properties(0).total_memory
            torch_module.cuda.set_per_process_memory_fraction(max(0.05, min(0.95, limit_bytes / max(1, total))))
    except Exception as error:  # an old torch without the call: the guard thread still watches
        print(json.dumps({"log": f"allocator cap not set: {error}"}), flush=True)


def windows_job(limit_bytes: Optional[int]) -> None:
    """Windows: the process joins a Job Object that refuses it memory past the limit, at below-normal priority."""
    if sys.platform != "win32":
        return
    kernel32 = ctypes.windll.kernel32  # type: ignore[attr-defined]
    kernel32.GetCurrentProcess.restype = ctypes.c_void_p
    kernel32.SetPriorityClass(kernel32.GetCurrentProcess(), 0x4000)  # BELOW_NORMAL_PRIORITY_CLASS
    if not limit_bytes:
        return

    class IoCounters(ctypes.Structure):
        _fields_ = [(name, ctypes.c_ulonglong) for name in ("r", "w", "o", "rb", "wb", "ob")]

    class BasicLimits(ctypes.Structure):
        _fields_ = [
            ("PerProcessUserTimeLimit", ctypes.c_int64),
            ("PerJobUserTimeLimit", ctypes.c_int64),
            ("LimitFlags", ctypes.c_uint32),
            ("MinimumWorkingSetSize", ctypes.c_size_t),
            ("MaximumWorkingSetSize", ctypes.c_size_t),
            ("ActiveProcessLimit", ctypes.c_uint32),
            ("Affinity", ctypes.c_size_t),
            ("PriorityClass", ctypes.c_uint32),
            ("SchedulingClass", ctypes.c_uint32),
        ]

    class ExtendedLimits(ctypes.Structure):
        _fields_ = [
            ("BasicLimitInformation", BasicLimits),
            ("IoInfo", IoCounters),
            ("ProcessMemoryLimit", ctypes.c_size_t),
            ("JobMemoryLimit", ctypes.c_size_t),
            ("PeakProcessMemoryUsed", ctypes.c_size_t),
            ("PeakJobMemoryUsed", ctypes.c_size_t),
        ]

    kernel32.CreateJobObjectW.restype = ctypes.c_void_p
    kernel32.SetInformationJobObject.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_void_p, ctypes.c_uint32]
    kernel32.AssignProcessToJobObject.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
    job = kernel32.CreateJobObjectW(None, None)
    limits = ExtendedLimits()
    limits.BasicLimitInformation.LimitFlags = 0x100  # JOB_OBJECT_LIMIT_PROCESS_MEMORY
    limits.ProcessMemoryLimit = limit_bytes
    ok = job and kernel32.SetInformationJobObject(job, 9, ctypes.byref(limits), ctypes.sizeof(limits))
    ok = ok and kernel32.AssignProcessToJobObject(job, kernel32.GetCurrentProcess())
    if not ok:
        print(json.dumps({"log": "windows: memory limit not set (Job Object refused)"}), flush=True)


def _gb(size: int) -> str:
    return f"{size / 1024 ** 3:.1f}".replace(".", ",")


class Guard:
    """Watches this process's memory once a second: the peak for the gateway's learning, a clear exit past the ceiling
    (the gateway sets it well above the need: only a runaway reaches it)."""

    def __init__(self, limit_bytes: Optional[int], report: Optional[Callable[[int], None]] = None, torch_module=None):
        self.limit = limit_bytes or 0
        self.report = report
        self.torch = torch_module
        self.peak = 0
        self.current = 0
        threading.Thread(target=self._watch, name="memory-guard", daemon=True).start()

    def attach_torch(self, torch_module) -> None:
        self.torch = torch_module

    def measure(self) -> int:
        gpu = gpu_memory(self.torch)
        self.current = own_memory() + gpu
        # The system's own peak catches spikes between two samples (a model load can spike within a second).
        self.peak = max(self.peak, self.current, peak_memory() + gpu)
        return self.current

    def _watch(self) -> None:
        while True:
            used = self.measure()
            if self.report:
                self.report(used)
            if self.limit and used > self.limit:
                print(
                    json.dumps(
                        {
                            "error": f"Остановлено: модель заняла больше {_gb(self.limit)} ГБ — намного больше обычного, похоже на сбой.",
                            "reason": "memory",
                            "peakMb": self.peak // MB,
                        },
                        ensure_ascii=False,
                    ),
                    flush=True,
                )
                os._exit(137)
            time.sleep(1.0)


def die_with_stdin() -> None:
    """
    The job lives only while the gateway that started it does: its stdin closing means the gateway is gone. The
    descriptor is read raw: a daemon thread blocked in the buffered sys.stdin aborts the interpreter's shutdown
    («Fatal Python error: could not acquire lock for <stdin>», exit 134) — a finished imprint looked failed (06.11).
    """

    def watch() -> None:
        try:
            while os.read(sys.stdin.fileno(), 1):
                pass
        except OSError:
            pass
        os._exit(3)

    threading.Thread(target=watch, name="stdin-watch", daemon=True).start()
