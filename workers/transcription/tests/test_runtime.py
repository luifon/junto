from __future__ import annotations

import ast
import importlib
import os
from importlib import metadata
from importlib.util import find_spec
from pathlib import Path

import pytest

import junto_transcribe
from junto_transcribe import runtime
from junto_transcribe.runtime import (
    PINNED,
    TELEMETRY_SWITCHES,
    StartError,
    apply_telemetry_switches,
    available_cpus,
    choose_device,
    library_versions,
    require_pinned_libraries,
)

PACKAGE = Path(junto_transcribe.__file__).parent
NETWORK_MODULES = {"socket", "ssl", "http", "urllib", "requests", "httpx", "huggingface_hub", "ftplib", "smtplib"}
TELEMETRY_MARKERS = (b"OneCollector", b"ORT_DISABLE_TELEMETRY")


def test_the_switches_are_set_even_when_the_environment_says_otherwise(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("ORT_DISABLE_TELEMETRY", "0")
    monkeypatch.delenv("HF_HUB_OFFLINE", raising=False)
    apply_telemetry_switches()
    assert {name: os.environ[name] for name in TELEMETRY_SWITCHES} == {
        "ORT_DISABLE_TELEMETRY": "1",
        "HF_HUB_DISABLE_TELEMETRY": "1",
        "HF_HUB_OFFLINE": "1",
        "DO_NOT_TRACK": "1",
    }


def test_importing_the_package_sets_the_switches(monkeypatch: pytest.MonkeyPatch) -> None:
    for name in TELEMETRY_SWITCHES:
        monkeypatch.delenv(name, raising=False)
    importlib.reload(junto_transcribe)
    assert {name: os.environ.get(name) for name in TELEMETRY_SWITCHES} == TELEMETRY_SWITCHES


def test_the_installed_libraries_are_the_pinned_ones() -> None:
    require_pinned_libraries()
    assert PINNED["onnxruntime"] == "1.28.0"


def test_another_library_version_refuses_the_start(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setitem(runtime.PINNED, "onnxruntime", "1.27.0")
    with pytest.raises(StartError, match=r"library_versions_differ: onnxruntime is 1\.28\.0, expected 1\.27\.0"):
        require_pinned_libraries()


def native_libraries(package: str) -> list[Path]:
    spec = find_spec(package)
    assert spec is not None and spec.origin is not None
    root = Path(spec.origin).parent
    return [path for path in root.rglob("*") if path.suffix in {".so", ".dylib", ".pyd"} or ".so." in path.name]


@pytest.mark.parametrize("package", ["onnxruntime", "sherpa_onnx", "ctranslate2"])
def test_no_native_library_contains_the_telemetry_client(package: str) -> None:
    libraries = native_libraries(package)
    assert libraries
    for library in libraries:
        content = library.read_bytes()
        for marker in TELEMETRY_MARKERS:
            assert marker not in content, f"{library.name} contains {marker.decode()}"


def imported_modules(path: Path) -> set[str]:
    modules: set[str] = set()
    for node in ast.walk(ast.parse(path.read_text("utf-8"))):
        if isinstance(node, ast.Import):
            modules.update(alias.name.split(".")[0] for alias in node.names)
        elif isinstance(node, ast.ImportFrom) and node.module and node.level == 0:
            modules.add(node.module.split(".")[0])
    return modules


def test_only_the_fetch_module_imports_a_network_module() -> None:
    users = {path.name for path in PACKAGE.glob("*.py") if imported_modules(path) & NETWORK_MODULES}
    assert users == {"fetch.py"}


def test_the_fetch_module_is_imported_only_by_the_models_fetch_command() -> None:
    importers = set()
    for path in PACKAGE.glob("*.py"):
        tree = ast.parse(path.read_text("utf-8"))
        for node in ast.walk(tree):
            if isinstance(node, ast.ImportFrom) and node.module == "junto_transcribe.fetch":
                importers.add(path.name)
    assert importers == {"cli.py"}
    source = (PACKAGE / "cli.py").read_text("utf-8")
    assert source.count("junto_transcribe.fetch") == 1
    assert 'if arguments.action == "fetch":\n            from junto_transcribe.fetch import fetch' in source


def test_library_versions_name_every_pinned_library() -> None:
    versions = library_versions("cpu")
    for name, version in PINNED.items():
        assert versions[name] == version
    assert versions["av"] == metadata.version("av")
    assert {"python", "numpy", "tokenizers", "libavcodec"} <= set(versions)


def test_cuda_is_refused_when_no_device_exists(monkeypatch: pytest.MonkeyPatch) -> None:
    import ctranslate2

    monkeypatch.setattr(ctranslate2, "get_cuda_device_count", lambda: 0)
    with pytest.raises(StartError, match="cuda_not_available"):
        choose_device("cuda", None, None)
    device = choose_device("auto", None, 3)
    assert (device.name, device.compute_type, device.threads) == ("cpu", "int8", 3)
    with pytest.raises(StartError, match="compute_type_not_available on cpu"):
        choose_device("cpu", "float16", None)


def test_a_gpu_that_the_user_cannot_open_refuses_the_start_instead_of_using_the_cpu(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    import ctranslate2

    node = tmp_path / "nvidiactl"
    node.write_bytes(b"")
    node.chmod(0)
    monkeypatch.setattr(ctranslate2, "get_cuda_device_count", lambda: 0)
    monkeypatch.setattr(runtime, "GPU_CONTROL_DEVICE", node)
    for requested in ("auto", "cuda"):
        with pytest.raises(StartError, match=r"gpu_device_not_permitted.*--group-add"):
            choose_device(requested, None, None)
    assert choose_device("cpu", None, None).name == "cpu"
    node.chmod(0o600)
    assert choose_device("auto", None, None).name == "cpu"


def test_an_unknown_device_name_is_refused() -> None:
    with pytest.raises(StartError, match="device_not_known"):
        choose_device("gpu", None, None)


def test_cuda_is_used_when_a_device_exists(monkeypatch: pytest.MonkeyPatch) -> None:
    import ctranslate2

    monkeypatch.setattr(ctranslate2, "get_cuda_device_count", lambda: 1)
    monkeypatch.setattr(ctranslate2, "get_supported_compute_types", lambda _: {"float16", "int8"})
    assert choose_device("auto", None, None).name == "cuda"
    assert choose_device("auto", None, None).compute_type == "float16"
    assert choose_device("cpu", None, None).name == "cpu"


def test_threads_follow_the_cpus_the_process_may_use() -> None:
    assert 1 <= available_cpus() <= (os.cpu_count() or 1)
    assert 1 <= choose_device("cpu", None, None).threads <= 8
