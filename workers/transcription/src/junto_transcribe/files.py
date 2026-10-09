"""Directories of a job, held by descriptors that are opened once."""

from __future__ import annotations

import contextlib
import os
import secrets
import stat
from collections.abc import Sequence
from pathlib import Path
from types import TracebackType
from typing import BinaryIO

from junto_transcribe.engines import AudioMissingError, AudioRefusedError

_DIRECTORY = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC
_FILE = os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC


class NotRegularFileError(Exception):
    pass


class FileTooLargeError(Exception):
    pass


class PinnedDirectory:
    """A directory that is opened once, without following a link at its own name.

    Every file is reached from the descriptor, so a later change of a path cannot change what is
    read or where a file is written. A file is accepted from its open descriptor.
    """

    def __init__(self, directory: Path) -> None:
        self._descriptor = os.open(directory, _DIRECTORY)

    def __enter__(self) -> PinnedDirectory:
        return self

    def __exit__(
        self, kind: type[BaseException] | None, error: BaseException | None, trace: TracebackType | None
    ) -> None:
        self.close()

    def close(self) -> None:
        if self._descriptor >= 0:
            os.close(self._descriptor)
            self._descriptor = -1

    def same_directory(self, other: PinnedDirectory) -> bool:
        mine = os.fstat(self._descriptor)
        theirs = os.fstat(other._descriptor)
        return (mine.st_dev, mine.st_ino) == (theirs.st_dev, theirs.st_ino)

    def open(self, parts: Sequence[str]) -> BinaryIO:
        """An audio file below this directory. No part of the path may be a link."""
        directory = os.dup(self._descriptor)
        try:
            for part in parts[:-1]:
                inner = _open(part, _DIRECTORY, directory)
                os.close(directory)
                directory = inner
            descriptor = _open(parts[-1], _FILE, directory)
        finally:
            os.close(directory)
        try:
            if not stat.S_ISREG(os.fstat(descriptor).st_mode):
                raise AudioRefusedError
            return os.fdopen(descriptor, "rb")
        except BaseException:
            with contextlib.suppress(OSError):
                os.close(descriptor)
            raise

    def read(self, name: str, max_bytes: int) -> bytes:
        """The content of a regular file in this directory, of at most `max_bytes` bytes."""
        descriptor = os.open(name, _FILE, dir_fd=self._descriptor)
        try:
            if not stat.S_ISREG(os.fstat(descriptor).st_mode):
                raise NotRegularFileError
            chunks = []
            remaining = max_bytes + 1
            while remaining > 0 and (chunk := os.read(descriptor, min(remaining, 1 << 20))):
                chunks.append(chunk)
                remaining -= len(chunk)
        finally:
            os.close(descriptor)
        content = b"".join(chunks)
        if len(content) > max_bytes:
            raise FileTooLargeError
        return content

    def write(self, name: str, content: bytes) -> None:
        """Puts a whole file at `name`: a new file with a random name is written and renamed over it."""
        temporary = f".{secrets.token_hex(16)}.part"
        flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC
        descriptor = os.open(temporary, flags, 0o644, dir_fd=self._descriptor)
        try:
            with os.fdopen(descriptor, "wb") as file:
                file.write(content)
                file.flush()
                os.fsync(file.fileno())
            os.rename(temporary, name, src_dir_fd=self._descriptor, dst_dir_fd=self._descriptor)
        except BaseException:
            with contextlib.suppress(OSError):
                os.unlink(temporary, dir_fd=self._descriptor)
            raise


def _open(name: str, flags: int, directory: int) -> int:
    try:
        return os.open(name, flags, dir_fd=directory)
    except FileNotFoundError as error:
        raise AudioMissingError from error
    except OSError as error:
        raise AudioRefusedError from error
