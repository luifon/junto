"""Download of the model files. This is the only module of the worker that opens a network connection."""

from __future__ import annotations

import contextlib
import fcntl
import hashlib
import os
import secrets
import stat
import urllib.request
from collections.abc import Callable
from pathlib import Path
from typing import BinaryIO
from urllib.error import HTTPError, URLError

from junto_transcribe import __version__
from junto_transcribe.models import Model, ModelError, ModelFile, Role

Opener = Callable[[str], BinaryIO]

_CHUNK = 1024 * 1024
_TIMEOUT_SECONDS = 60
_TEMPORARY_PREFIX = ".fetch-"
_DIRECTORY = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC
_SEALED_DIRECTORY = 0o555
_SEALED_FILE = 0o444


class _HttpsOnly(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):  # type: ignore[no-untyped-def]
        if not newurl.lower().startswith("https://"):
            raise HTTPError(newurl, code, "redirect to an address that is not HTTPS", headers, fp)
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def https_opener(url: str) -> BinaryIO:
    if not url.lower().startswith("https://"):
        raise ModelError(f"{url} is not an HTTPS address")
    request = urllib.request.Request(url, headers={"User-Agent": f"junto-transcribe/{__version__}"})
    opener = urllib.request.build_opener(_HttpsOnly)
    response: BinaryIO = opener.open(request, timeout=_TIMEOUT_SECONDS)
    return response


def _open_checked_directory(path: Path) -> int:
    """A descriptor of a directory that is not a link, belongs to this user and is closed to other users.

    The directory is created when it is absent. It is made writable for this user for the time of the fetch.
    """
    try:
        path.mkdir(mode=0o755, exist_ok=True)
    except OSError as error:
        raise ModelError(f"{path} cannot be created: {type(error).__name__}") from error
    try:
        descriptor = os.open(path, _DIRECTORY)
    except OSError as error:
        raise ModelError(
            f"{path} is a link or is not a directory; a model directory must be a real directory"
        ) from error
    status = os.fstat(descriptor)
    problem = None
    if status.st_uid != os.geteuid():
        problem = f"{path} belongs to another user"
    elif status.st_mode & (stat.S_IWGRP | stat.S_IWOTH):
        problem = f"other users can write to {path}"
    if problem is not None:
        os.close(descriptor)
        raise ModelError(problem)
    os.fchmod(descriptor, 0o755)
    return descriptor


def _present(name: str, file: ModelFile, directory: int, shown: Path) -> bool:
    """Whether the file is there with the right content. A link at its place is refused."""
    try:
        status = os.stat(name, dir_fd=directory, follow_symlinks=False)
    except FileNotFoundError:
        return False
    if stat.S_ISLNK(status.st_mode):
        raise ModelError(f"{shown} is a link; remove it and run the command again")
    if not stat.S_ISREG(status.st_mode) or status.st_size != file.size:
        return False
    digest = hashlib.sha256()
    descriptor = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=directory)
    with os.fdopen(descriptor, "rb") as content:
        while block := content.read(_CHUNK):
            digest.update(block)
        if digest.hexdigest() != file.sha256:
            return False
        os.fchmod(content.fileno(), _SEALED_FILE)
    return True


def _download(file: ModelFile, directory: int, opener: Opener) -> None:
    """Writes to a new file with a random name inside the checked directory, and renames it when it is right."""
    if file.url is None:
        raise ModelError(f"{file.name} has no download address")
    temporary = _TEMPORARY_PREFIX + secrets.token_hex(16)
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC
    try:
        descriptor = os.open(temporary, flags, 0o600, dir_fd=directory)
    except FileExistsError as error:
        raise ModelError(f"the temporary file for {file.name} exists already; nothing was written") from error
    digest = hashlib.sha256()
    size = 0
    try:
        with os.fdopen(descriptor, "wb") as output, opener(file.url) as response:
            while block := response.read(_CHUNK):
                size += len(block)
                if size > file.size:
                    raise ModelError(f"{file.url} is larger than the {file.size} bytes in the manifest")
                digest.update(block)
                output.write(block)
            output.flush()
            os.fsync(output.fileno())
            if size != file.size:
                raise ModelError(f"{file.url} has {size} bytes, expected {file.size}")
            if digest.hexdigest() != file.sha256:
                raise ModelError(f"{file.url} has a different SHA-256 checksum than the manifest")
            os.fchmod(output.fileno(), _SEALED_FILE)
        os.rename(temporary, file.name, src_dir_fd=directory, dst_dir_fd=directory)
    except (HTTPError, URLError, TimeoutError, ConnectionError) as error:
        raise ModelError(f"download of {file.url} failed: {error}") from error
    finally:
        with contextlib.suppress(FileNotFoundError):
            os.unlink(temporary, dir_fd=directory)


def _lock(directory: int, report: Callable[[str], None]) -> None:
    """One fetch at a time per model directory. The lock ends when the descriptor is closed."""
    try:
        fcntl.flock(directory, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        report("another models fetch uses this directory; waiting for it to end")
        fcntl.flock(directory, fcntl.LOCK_EX)


def _fetch_model(model: Model, root: Path, opener: Opener, report: Callable[[str], None]) -> None:
    directory = _open_checked_directory(root)
    try:
        for name in os.listdir(directory):  # noqa: PTH208 - the checked descriptor, not a path
            if name.startswith(_TEMPORARY_PREFIX):
                os.unlink(name, dir_fd=directory)
        for file in model.files:
            if _present(file.name, file, directory, root / file.name):
                report(f"present   {root / file.name}")
                continue
            report(f"download  {file.url} ({file.size} bytes)")
            _download(file, directory, opener)
            report(f"verified  {root / file.name}")
    finally:
        os.fchmod(directory, _SEALED_DIRECTORY)
        os.close(directory)


def fetch(
    models: dict[Role, Model],
    models_dir: Path,
    opener: Opener = https_opener,
    report: Callable[[str], None] = print,
) -> None:
    """Downloads every file that is missing or wrong, and keeps a file only when its checksum matches.

    Every directory is checked before it is written to, and is left without write permission.
    The model directory is locked for the whole fetch, so a second fetch waits, and what an
    interrupted fetch left is removed only by the fetch that holds the lock.
    """
    top = _open_checked_directory(models_dir)
    try:
        _lock(top, report)
        for model in models.values():
            if model.package is None:
                _fetch_model(model, model.root(models_dir), opener, report)
    finally:
        os.fchmod(top, _SEALED_DIRECTORY)
        os.close(top)
