"""Model files: only the listed files, read-only for the worker, checked before and after loading, safe downloads."""

from __future__ import annotations

import hashlib
import io
import stat
from pathlib import Path
from typing import Any, BinaryIO

import pytest

from junto_transcribe import worker
from junto_transcribe.fetch import fetch
from junto_transcribe.models import (
    Model,
    ModelError,
    ModelFile,
    Role,
    manifest,
    problems,
    require,
    require_read_only,
)

CONTENT: dict[Role, dict[str, bytes]] = {
    "voice-detector": {"detector.onnx": b"detector" * 100},
    "whisper": {"config.json": b'{"a": 1}', "model.bin": b"weights" * 1000},
    "speaker-embedding": {"speaker.onnx": b"speaker" * 300},
}
WEIGHTS = {"voice-detector": "detector.onnx", "whisper": "model.bin", "speaker-embedding": "speaker.onnx"}


def models(*roles: Role) -> dict[Role, Model]:
    return {
        role: Model(
            role=role,
            name=f"invented-{role}",
            default=True,
            purpose="test",
            version="1",
            licence="MIT",
            source="https://models.invalid",
            attribution="",
            package=None,
            directory=role,
            weights=WEIGHTS[role],
            settings={"batch_size": 8, "threshold": 0.6},
            files=tuple(
                ModelFile(name, f"https://models.invalid/{role}/{name}", hashlib.sha256(data).hexdigest(), len(data))
                for name, data in CONTENT[role].items()
            ),
        )
        for role in (roles or tuple(CONTENT))
    }


def opener(url: str) -> BinaryIO:
    role, name = url.rsplit("/", 2)[1:]
    return io.BytesIO(CONTENT[role][name])  # type: ignore[index]


def install(directory: Path, *roles: Role) -> dict[Role, Model]:
    chosen = models(*roles)
    directory.mkdir(mode=0o755, exist_ok=True)
    fetch(chosen, directory, opener, report=lambda _: None)
    return chosen


def writable(path: Path) -> None:
    path.chmod(path.stat().st_mode | stat.S_IWUSR)


@pytest.fixture(autouse=True)
def restore_write_access(tmp_path: Path) -> Any:
    yield
    for path in [tmp_path, *tmp_path.rglob("*")]:
        if not path.is_symlink():
            writable(path)


# Finding 7: every file a loader can read is listed, and nothing else may be there.


def test_a_file_that_the_manifest_does_not_list_is_refused(tmp_path: Path) -> None:
    chosen = install(tmp_path / "models", "whisper")
    root = tmp_path / "models" / "whisper"
    writable(root)
    (root / "preprocessor_config.json").write_text('{"sampling_rate": 8000}')
    found = problems(chosen, tmp_path / "models")
    assert found == ["model_directory_has_unlisted_entry: whisper invented-whisper"]
    with pytest.raises(ModelError):
        require(chosen, tmp_path / "models")


def test_a_directory_or_a_link_inside_a_model_directory_is_refused(tmp_path: Path) -> None:
    chosen = install(tmp_path / "models", "whisper")
    root = tmp_path / "models" / "whisper"
    writable(root)
    (root / "extra").mkdir()
    assert problems(chosen, tmp_path / "models") == ["model_directory_has_unlisted_entry: whisper invented-whisper"]
    (root / "extra").rmdir()

    (tmp_path / "elsewhere.bin").write_bytes(CONTENT["whisper"]["model.bin"])
    (root / "model.bin").unlink()
    (root / "model.bin").symlink_to(tmp_path / "elsewhere.bin")
    assert problems(chosen, tmp_path / "models") == ["model_file_not_regular: whisper invented-whisper model.bin"]


def test_a_model_directory_that_is_a_link_is_refused(tmp_path: Path) -> None:
    chosen = install(tmp_path / "real", "whisper")
    (tmp_path / "models").mkdir()
    (tmp_path / "models" / "whisper").symlink_to(tmp_path / "real" / "whisper")
    assert problems(chosen, tmp_path / "models") == ["model_directory_is_a_link: whisper invented-whisper"]


def test_the_manifest_lists_the_files_that_the_whisper_loader_reads() -> None:
    for entry in manifest():
        if entry.role == "whisper":
            names = {file.name for file in entry.files}
            assert {"config.json", "model.bin", "tokenizer.json"} <= names
            assert names & {"vocabulary.json", "vocabulary.txt"}
            assert names <= {
                "config.json",
                "model.bin",
                "tokenizer.json",
                "preprocessor_config.json",
                "vocabulary.json",
                "vocabulary.txt",
            }


# Finding 5: the worker cannot write to the models, and checks them before and after loading.


def test_a_model_directory_that_the_worker_can_write_to_refuses_the_start(tmp_path: Path) -> None:
    chosen = install(tmp_path / "models")
    require_read_only(chosen, tmp_path / "models")

    for path in (tmp_path / "models", tmp_path / "models" / "whisper", tmp_path / "models" / "whisper" / "model.bin"):
        writable(path)
        with pytest.raises(ModelError, match="model_files_writable"):
            require_read_only(chosen, tmp_path / "models")
        path.chmod(path.stat().st_mode & ~stat.S_IWUSR)
    require_read_only(chosen, tmp_path / "models")


class Engine:
    def __init__(self, *_: object, **__: object) -> None:
        pass


def patch_engines(monkeypatch: pytest.MonkeyPatch, chosen: dict[Role, Model], whisper: type = Engine) -> None:
    import junto_transcribe.detector
    import junto_transcribe.embedder
    import junto_transcribe.whisper

    monkeypatch.setattr(worker, "select", lambda _: chosen)
    monkeypatch.setattr(junto_transcribe.detector, "SileroDetector", Engine)
    monkeypatch.setattr(junto_transcribe.embedder, "SherpaEmbedder", Engine)
    monkeypatch.setattr(junto_transcribe.whisper, "WhisperRecognizer", whisper)


def test_models_are_verified_again_after_they_are_loaded(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    chosen = install(tmp_path / "models")
    weights = tmp_path / "models" / "whisper" / "model.bin"

    class ChangedWhileLoading(Engine):
        def __init__(self, *_: object, **__: object) -> None:
            writable(weights.parent)
            writable(weights)
            weights.write_bytes(b"WEIGHTS" * 1000)
            weights.chmod(0o444)
            weights.parent.chmod(0o555)

    patch_engines(monkeypatch, chosen, ChangedWhileLoading)
    options = worker.Options(models_dir=tmp_path / "models", whisper="invented", device="cpu")
    with pytest.raises(
        ModelError,
        match=r"model_files_changed_while_loading: model_file_checksum_differs: whisper invented-whisper model\.bin",
    ):
        worker.load(options, speakers=True)


def test_unchanged_models_load(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    chosen = install(tmp_path / "models")
    patch_engines(monkeypatch, chosen)
    loaded = worker.load(
        worker.Options(models_dir=tmp_path / "models", whisper="invented", device="cpu"), speakers=True
    )
    assert set(loaded.stages_ms) == {"verify_models", "load_models", "verify_models_again"}


def test_the_worker_refuses_writable_models_before_it_loads_anything(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    chosen = install(tmp_path / "models")
    writable(tmp_path / "models" / "speaker-embedding")

    class NotReached(Engine):
        def __init__(self, *_: object, **__: object) -> None:
            raise AssertionError("a model was loaded from a directory the worker can write to")

    patch_engines(monkeypatch, chosen, NotReached)
    with pytest.raises(ModelError, match="model_files_writable"):
        worker.load(worker.Options(models_dir=tmp_path / "models", whisper="invented", device="cpu"), speakers=True)


# Finding 13: downloads.


def test_fetch_does_not_write_through_a_link_at_a_guessable_temporary_name(tmp_path: Path) -> None:
    victim = tmp_path / "victim.txt"
    victim.write_text("keep me")
    root = tmp_path / "models" / "whisper"
    root.mkdir(parents=True)
    for name in ("model.bin.part", "model.bin.tmp", ".model.bin.part"):
        (root / name).symlink_to(victim)
    fetch(models("whisper"), tmp_path / "models", opener, report=lambda _: None)
    assert victim.read_text() == "keep me"
    assert (root / "model.bin").read_bytes() == CONTENT["whisper"]["model.bin"]


def test_fetch_refuses_a_destination_that_is_a_link(tmp_path: Path) -> None:
    victim = tmp_path / "victim.bin"
    victim.write_bytes(b"keep me")
    root = tmp_path / "models" / "whisper"
    root.mkdir(parents=True)
    (root / "model.bin").symlink_to(victim)
    with pytest.raises(ModelError, match="link"):
        fetch(models("whisper"), tmp_path / "models", opener, report=lambda _: None)
    assert victim.read_bytes() == b"keep me"
    assert (root / "model.bin").is_symlink()


def test_fetch_refuses_a_model_directory_that_is_a_link(tmp_path: Path) -> None:
    (tmp_path / "elsewhere").mkdir()
    (tmp_path / "models").mkdir()
    (tmp_path / "models" / "whisper").symlink_to(tmp_path / "elsewhere")
    with pytest.raises(ModelError, match="link"):
        fetch(models("whisper"), tmp_path / "models", opener, report=lambda _: None)
    assert list((tmp_path / "elsewhere").iterdir()) == []

    (tmp_path / "linked").symlink_to(tmp_path / "elsewhere")
    with pytest.raises(ModelError, match="link"):
        fetch(models("whisper"), tmp_path / "linked", opener, report=lambda _: None)


@pytest.mark.parametrize("mode", [0o777, 0o775, 0o757])
def test_fetch_refuses_a_directory_that_other_users_can_write_to(tmp_path: Path, mode: int) -> None:
    (tmp_path / "models").mkdir()
    (tmp_path / "models").chmod(mode)
    with pytest.raises(ModelError, match="other users can write"):
        fetch(models("whisper"), tmp_path / "models", opener, report=lambda _: None)
    assert list((tmp_path / "models").iterdir()) == []


def test_temporary_files_are_created_exclusively_with_names_that_cannot_be_guessed(tmp_path: Path) -> None:
    seen: list[set[str]] = []

    def watching(url: str) -> BinaryIO:
        root = tmp_path / "models" / "whisper"
        seen.append({path.name for path in root.iterdir()} - set(CONTENT["whisper"]))
        return opener(url)

    for _ in range(2):
        fetch(models("whisper"), tmp_path / "models", watching, report=lambda _: None)
        for path in [tmp_path / "models" / "whisper", *(tmp_path / "models" / "whisper").iterdir()]:
            writable(path)
        (tmp_path / "models" / "whisper" / "model.bin").unlink()
        (tmp_path / "models" / "whisper" / "config.json").unlink()
    names = [name for names in seen for name in names]
    assert len(names) == 4 and len(set(names)) == 4
    assert all(len(name) >= 30 for name in names)
    assert sorted(path.name for path in (tmp_path / "models" / "whisper").iterdir()) == []


def test_a_failed_download_leaves_no_file(tmp_path: Path) -> None:
    def broken(url: str) -> BinaryIO:
        return io.BytesIO(b"other content")

    with pytest.raises(ModelError):
        fetch(models("whisper"), tmp_path / "models", broken, report=lambda _: None)
    assert list((tmp_path / "models" / "whisper").iterdir()) == []


def test_fetched_models_are_read_only_and_fetch_can_still_repair_them(tmp_path: Path) -> None:
    chosen = install(tmp_path / "models")
    for role, files in CONTENT.items():
        assert stat.S_IMODE((tmp_path / "models" / role).stat().st_mode) == 0o555
        for name in files:
            assert stat.S_IMODE((tmp_path / "models" / role / name).stat().st_mode) == 0o444
    assert stat.S_IMODE((tmp_path / "models").stat().st_mode) == 0o555
    require_read_only(chosen, tmp_path / "models")
    assert problems(chosen, tmp_path / "models") == []

    weights = tmp_path / "models" / "whisper" / "model.bin"
    writable(weights.parent)
    weights.unlink()
    weights.parent.chmod(0o555)
    fetch(chosen, tmp_path / "models", opener, report=lambda _: None)
    assert problems(chosen, tmp_path / "models") == []
    require_read_only(chosen, tmp_path / "models")


def test_fetch_removes_what_an_interrupted_fetch_left(tmp_path: Path) -> None:
    root = tmp_path / "models" / "whisper"
    root.mkdir(parents=True)
    (root / ".fetch-0123456789abcdef0123456789abcdef").write_bytes(b"partial")
    chosen = install(tmp_path / "models", "whisper")
    assert problems(chosen, tmp_path / "models") == []


def test_the_acceptance_scripts_never_open_the_model_directory_to_everyone() -> None:
    scripts = Path(__file__).parent / "acceptance"
    for script in scripts.glob("*.sh"):
        text = script.read_text("utf-8")
        for line in text.splitlines():
            if "chmod" in line:
                assert "models" not in line, f"{script.name}: {line.strip()}"
    prepare = (scripts / "prepare.sh").read_text("utf-8")
    assert '--user "$me"' in prepare.split('"$prefix-models"')[1].splitlines()[0]


# Second review, B8: one fetch at a time per model directory.


def test_a_second_fetch_waits_for_the_first_and_does_not_remove_its_temporary_file(tmp_path: Path) -> None:
    import fcntl
    import os
    import threading

    root = tmp_path / "models" / "whisper"
    root.mkdir(parents=True)
    active = root / ".fetch-0123456789abcdef0123456789abcdef"
    active.write_bytes(b"the first fetch is writing this file")
    first = os.open(tmp_path / "models", os.O_RDONLY)
    fcntl.flock(first, fcntl.LOCK_EX)
    messages: list[str] = []
    second = threading.Thread(target=fetch, args=(models("whisper"), tmp_path / "models", opener, messages.append))
    try:
        second.start()
        second.join(timeout=0.5)
        assert second.is_alive(), "the second fetch did not wait"
        assert active.read_bytes() == b"the first fetch is writing this file"
        assert not (root / "model.bin").exists()
        assert any("another" in message for message in messages)
    finally:
        fcntl.flock(first, fcntl.LOCK_UN)
        os.close(first)
        second.join(timeout=10)
    assert not second.is_alive()
    assert problems(models("whisper"), tmp_path / "models") == []


def test_a_temporary_file_is_never_opened_when_it_already_exists(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import secrets

    root = tmp_path / "models" / "whisper"
    name = "cd" * 16

    def another_process_creates_the_same_name(size: int) -> str:
        (root / f".fetch-{name}").write_bytes(b"file of another process")
        return name

    monkeypatch.setattr(secrets, "token_hex", another_process_creates_the_same_name)
    with pytest.raises(ModelError, match="temporary"):
        fetch(models("whisper"), tmp_path / "models", opener, report=lambda _: None)
    assert (root / f".fetch-{name}").read_bytes() == b"file of another process"
    assert not (root / "config.json").exists()
