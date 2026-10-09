"""The model files this worker accepts: the manifest and the checksum check."""

from __future__ import annotations

import hashlib
import json
import os
import stat
from dataclasses import dataclass
from functools import cache
from importlib import resources
from importlib.util import find_spec
from pathlib import Path
from typing import Any, Literal

Role = Literal["voice-detector", "whisper", "speaker-embedding"]

DEFAULT_WHISPER = "large-v3-turbo"
_CHUNK = 4 * 1024 * 1024


class ModelError(Exception):
    pass


@dataclass(frozen=True)
class ModelFile:
    name: str
    url: str | None
    sha256: str
    size: int


@dataclass(frozen=True)
class Model:
    role: Role
    name: str
    default: bool
    purpose: str
    version: str
    licence: str
    source: str
    attribution: str
    package: str | None
    directory: str
    weights: str
    settings: dict[str, Any]
    files: tuple[ModelFile, ...]

    def root(self, models_dir: Path) -> Path:
        if self.package is None:
            return models_dir / self.directory
        spec = find_spec(self.package)
        if spec is None or spec.origin is None:
            raise ModelError(f"model_package_not_installed: {self.role} {self.name}")
        return Path(spec.origin).parent / self.directory

    def weights_path(self, models_dir: Path) -> Path:
        return self.root(models_dir) / self.weights

    @property
    def weights_sha256(self) -> str:
        return next(file.sha256 for file in self.files if file.name == self.weights)


@cache
def manifest() -> tuple[Model, ...]:
    data = json.loads(resources.files("junto_transcribe").joinpath("manifest.json").read_text("utf-8"))
    return tuple(
        Model(
            role=item["role"],
            name=item["name"],
            default=item["default"],
            purpose=item["purpose"],
            version=item["version"],
            licence=item["licence"],
            source=item["source"],
            attribution=item["attribution"],
            package=item.get("package"),
            directory=item["directory"],
            weights=item["weights"],
            settings=item["settings"],
            files=tuple(ModelFile(**file) for file in item["files"]),
        )
        for item in data["models"]
    )


def whisper_names() -> list[str]:
    return [model.name for model in manifest() if model.role == "whisper"]


def select(whisper: str = DEFAULT_WHISPER) -> dict[Role, Model]:
    """The voice detector, the named Whisper model and the speaker-embedding model."""
    chosen: dict[Role, Model] = {}
    for model in manifest():
        if model.role == "whisper":
            if model.name == whisper:
                chosen["whisper"] = model
        elif model.default:
            chosen[model.role] = model
    if "whisper" not in chosen:
        raise ModelError(f"whisper_model_not_in_manifest (the manifest has: {', '.join(whisper_names())})")
    return chosen


def sha256_of(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as file:
        while block := file.read(_CHUNK):
            digest.update(block)
    return digest.hexdigest()


def file_problem(path: Path, expected: ModelFile) -> str | None:
    """A code for what is wrong with the file, or `None`."""
    try:
        status = path.lstat()
    except FileNotFoundError:
        return "model_file_missing"
    if not stat.S_ISREG(status.st_mode):
        return "model_file_not_regular"
    if status.st_size != expected.size:
        return "model_file_size_differs"
    if sha256_of(path) != expected.sha256:
        return "model_file_checksum_differs"
    return None


def problems(models: dict[Role, Model], models_dir: Path) -> list[str]:
    """Every difference from the manifest, as a code and names from the manifest. No path of the host.

    A model directory may hold the listed files and nothing else, because a loader also reads
    optional files that it finds there.
    """
    found = []
    for model in models.values():
        root = model.root(models_dir)
        label = f"{model.role} {model.name}"
        if model.package is None and root.is_symlink():
            found.append(f"model_directory_is_a_link: {label}")
            continue
        if model.package is None and root.is_dir():
            listed = {file.name for file in model.files}
            if any(entry.name not in listed for entry in root.iterdir()):
                found.append(f"model_directory_has_unlisted_entry: {label}")
        for file in model.files:
            problem = file_problem(root / file.name, file)
            if problem is not None:
                found.append(f"{problem}: {label} {file.name}")
    return found


def require(models: dict[Role, Model], models_dir: Path) -> None:
    found = problems(models, models_dir)
    if found:
        raise ModelError("; ".join(found))


def require_read_only(models: dict[Role, Model], models_dir: Path) -> None:
    """Refuses a model directory that this process can change.

    The worker verifies the files and then loads them by path. Between the two steps the files
    must not change, so nothing that runs as the worker may be able to write to them.
    """
    writable = ["the model directory"] if os.access(models_dir, os.W_OK) else []
    for model in models.values():
        if model.package is None:
            root = model.root(models_dir)
            paths = [root, *(root / file.name for file in model.files)]
            if any(path.exists() and os.access(path, os.W_OK) for path in paths):
                writable.append(f"{model.role} {model.name}")
    if writable:
        raise ModelError("model_files_writable: " + ", ".join(writable))
