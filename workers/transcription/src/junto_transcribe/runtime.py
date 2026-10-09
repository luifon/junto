"""Process-level rules: telemetry switches, pinned libraries, device, threads."""

from __future__ import annotations

import math
import os
import platform
import resource
import sys
from dataclasses import dataclass
from importlib import metadata
from pathlib import Path

TELEMETRY_SWITCHES = {
    "ORT_DISABLE_TELEMETRY": "1",
    "HF_HUB_DISABLE_TELEMETRY": "1",
    "HF_HUB_OFFLINE": "1",
    "DO_NOT_TRACK": "1",
}

# Versions that the measurements and the telemetry check were made with.
PINNED = {
    "faster-whisper": "1.2.1",
    "ctranslate2": "4.8.2",
    "onnxruntime": "1.28.0",
    "sherpa-onnx": "1.13.8",
    "sherpa-onnx-core": "1.13.8",
}

REPORTED = (*PINNED, "tokenizers", "av", "numpy", "huggingface-hub", "jsonschema")
GPU_LIBRARIES = ("nvidia-cublas-cu12", "nvidia-cudnn-cu12")
MAX_THREADS = 8
DEVICES = ("auto", "cpu", "cuda")
GPU_CONTROL_DEVICE = Path("/dev/nvidiactl")


class StartError(Exception):
    """The worker refuses to start."""


@dataclass(frozen=True)
class Device:
    name: str
    compute_type: str
    threads: int


def apply_telemetry_switches() -> None:
    """Runs when the package is imported, which is before a model library is imported."""
    os.environ.update(TELEMETRY_SWITCHES)


def require_pinned_libraries() -> None:
    wrong = []
    for name, expected in PINNED.items():
        try:
            installed = metadata.version(name)
        except metadata.PackageNotFoundError:
            installed = "not installed"
        if installed != expected:
            wrong.append(f"{name} is {installed}, expected {expected}")
    if wrong:
        raise StartError("library_versions_differ: " + "; ".join(wrong))


def library_versions(device: str) -> dict[str, str]:
    versions = {"python": platform.python_version()}
    names = REPORTED + (GPU_LIBRARIES if device == "cuda" else ())
    for name in names:
        try:
            versions[name] = metadata.version(name)
        except metadata.PackageNotFoundError:
            continue
    import av

    for library, numbers in sorted(av.library_versions.items()):
        if library in ("libavcodec", "libavformat", "libavutil", "libswresample"):
            versions[library] = ".".join(str(number) for number in numbers)
    return versions


def available_cpus() -> int:
    count = len(os.sched_getaffinity(0)) if hasattr(os, "sched_getaffinity") else (os.cpu_count() or 1)
    try:
        quota, period = Path("/sys/fs/cgroup/cpu.max").read_text().split()
        if quota != "max":
            count = min(count, max(1, math.ceil(int(quota) / int(period))))
    except (OSError, ValueError):
        pass
    return count


def _gpu_without_permission() -> str | None:
    if GPU_CONTROL_DEVICE.exists() and not os.access(GPU_CONTROL_DEVICE, os.R_OK | os.W_OK):
        return (
            f"{GPU_CONTROL_DEVICE} exists and the user of this process cannot open it; "
            "start the container with the group of that device (--group-add)"
        )
    return None


def choose_device(requested: str, compute_type: str | None, threads: int | None) -> Device:
    import ctranslate2

    if requested not in DEVICES:
        raise StartError(f"device_not_known (use one of: {', '.join(DEVICES)})")
    cuda = ctranslate2.get_cuda_device_count() > 0
    if requested != "cpu" and not cuda:
        blocked = _gpu_without_permission()
        if blocked is not None:
            raise StartError(f"gpu_device_not_permitted: {blocked}; or ask for --device cpu")
        if requested == "cuda":
            raise StartError("cuda_not_available: the device cuda was asked for and no CUDA device exists")
    name = "cuda" if cuda and requested in ("auto", "cuda") else "cpu"
    chosen = compute_type or ("float16" if name == "cuda" else "int8")
    if chosen not in ctranslate2.get_supported_compute_types(name):
        raise StartError(f"compute_type_not_available on {name}")
    return Device(name, chosen, threads or min(MAX_THREADS, available_cpus()))


def peak_memory_mb() -> int:
    peak = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    return round(peak / (1024 * 1024 if sys.platform == "darwin" else 1024))
