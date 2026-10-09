from __future__ import annotations

import hashlib
import io
from pathlib import Path
from typing import BinaryIO

import pytest

from junto_transcribe import fetch as fetch_module
from junto_transcribe.fetch import fetch, https_opener
from junto_transcribe.models import (
    DEFAULT_WHISPER,
    Model,
    ModelError,
    ModelFile,
    Role,
    manifest,
    problems,
    require,
    select,
)

CONTENT = {"config.json": b'{"a": 1}', "model.bin": b"weights" * 1000}


def model(content: dict[str, bytes] = CONTENT) -> dict[Role, Model]:
    files = tuple(
        ModelFile(name, f"https://models.invalid/{name}", hashlib.sha256(data).hexdigest(), len(data))
        for name, data in content.items()
    )
    return {
        "whisper": Model(
            role="whisper",
            name="invented",
            default=True,
            purpose="test",
            version="1",
            licence="MIT",
            source="https://models.invalid",
            attribution="",
            package=None,
            directory="invented",
            weights="model.bin",
            settings={},
            files=files,
        )
    }


def serve(content: dict[str, bytes], requested: list[str]) -> fetch_module.Opener:
    def opener(url: str) -> BinaryIO:
        requested.append(url)
        return io.BytesIO(content[url.rsplit("/", 1)[1]])

    return opener


def test_the_manifest_names_one_default_model_per_role_with_checksums() -> None:
    chosen = select()
    assert set(chosen) == {"voice-detector", "whisper", "speaker-embedding"}
    assert chosen["whisper"].name == DEFAULT_WHISPER == "large-v3-turbo"
    assert chosen["speaker-embedding"].licence == "CC-BY-4.0"
    assert "NVIDIA" in chosen["speaker-embedding"].attribution
    assert chosen["speaker-embedding"].settings == {"threshold": 0.6}
    for entry in manifest():
        assert entry.licence in {"MIT", "CC-BY-4.0"}
        assert entry.weights in {file.name for file in entry.files}
        for file in entry.files:
            assert len(file.sha256) == 64 and file.size > 0
            assert (file.url is None) == (entry.package is not None)
            assert file.url is None or file.url.startswith("https://")


def test_download_addresses_name_a_revision_and_never_an_alias() -> None:
    for entry in manifest():
        if entry.role == "whisper":
            assert len(entry.version) == 40
            assert all(f"/resolve/{entry.version}/" in (file.url or "") for file in entry.files)


def test_an_unknown_whisper_model_is_refused() -> None:
    with pytest.raises(ModelError, match="whisper_model_not_in_manifest"):
        select("large-v9")


def test_the_voice_detector_inside_the_package_has_the_checksum_of_the_manifest(tmp_path: Path) -> None:
    detector: dict[Role, Model] = {"voice-detector": select()["voice-detector"]}
    assert problems(detector, tmp_path) == []


def test_fetch_downloads_verifies_and_leaves_no_partial_file(tmp_path: Path) -> None:
    requested: list[str] = []
    fetch(model(), tmp_path, serve(CONTENT, requested), report=lambda _: None)
    assert sorted(path.name for path in (tmp_path / "invented").iterdir()) == ["config.json", "model.bin"]
    assert problems(model(), tmp_path) == []
    assert len(requested) == 2

    fetch(model(), tmp_path, serve(CONTENT, requested), report=lambda _: None)
    assert len(requested) == 2


def test_fetch_refuses_a_file_with_another_checksum(tmp_path: Path) -> None:
    tampered = {**CONTENT, "model.bin": b"WEIGHTS" * 1000}
    with pytest.raises(ModelError, match="different SHA-256"):
        fetch(model(), tmp_path, serve(tampered, []), report=lambda _: None)
    assert [path.name for path in (tmp_path / "invented").iterdir()] == ["config.json"]


@pytest.mark.parametrize("served", [b"weights" * 999, b"weights" * 1001])
def test_fetch_refuses_a_file_with_another_size(tmp_path: Path, served: bytes) -> None:
    with pytest.raises(ModelError, match="bytes"):
        fetch(model(), tmp_path, serve({**CONTENT, "model.bin": served}, []), report=lambda _: None)
    assert not (tmp_path / "invented" / "model.bin").exists()


def test_fetch_replaces_a_damaged_file(tmp_path: Path) -> None:
    (tmp_path / "invented").mkdir()
    (tmp_path / "invented" / "model.bin").write_bytes(b"damaged")
    fetch(model(), tmp_path, serve(CONTENT, []), report=lambda _: None)
    assert problems(model(), tmp_path) == []


def test_verification_reports_a_missing_a_short_and_a_changed_file(tmp_path: Path) -> None:
    assert len(problems(model(), tmp_path)) == 2
    fetch(model(), tmp_path, serve(CONTENT, []), report=lambda _: None)
    weights = tmp_path / "invented" / "model.bin"
    weights.parent.chmod(0o755)
    weights.chmod(0o644)

    weights.write_bytes(CONTENT["model.bin"][:-1])
    assert problems(model(), tmp_path) == ["model_file_size_differs: whisper invented model.bin"]

    weights.write_bytes(b"x" + CONTENT["model.bin"][1:])
    assert problems(model(), tmp_path) == ["model_file_checksum_differs: whisper invented model.bin"]
    with pytest.raises(ModelError, match="model_file_checksum_differs"):
        require(model(), tmp_path)

    weights.unlink()
    assert problems(model(), tmp_path) == ["model_file_missing: whisper invented model.bin"]


def test_only_https_addresses_are_opened() -> None:
    with pytest.raises(ModelError, match="not an HTTPS address"):
        https_opener("http://models.invalid/model.bin")
