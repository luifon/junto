"""Audio files are opened inside the audio root, through a directory that is opened once."""

from __future__ import annotations

import os
import wave
from pathlib import Path

import numpy as np
import pytest

from junto_transcribe.audio import AvAudioReader
from junto_transcribe.engines import AudioMissingError, AudioRefusedError, Samples
from junto_transcribe.files import PinnedDirectory


def write_wav(path: Path, level: float, seconds: float = 1.0) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with wave.open(str(path), "wb") as file:
        file.setnchannels(1)
        file.setsampwidth(2)
        file.setframerate(16000)
        file.writeframes((np.full(int(seconds * 16000), level) * 32767).astype("<i2").tobytes())


def read(files: PinnedDirectory, *parts: str) -> Samples:
    with files.open(parts) as source:
        return np.concatenate([block for block, _ in AvAudioReader().blocks(source, 16000)])


@pytest.fixture
def root(tmp_path: Path) -> Path:
    write_wav(tmp_path / "root" / "audio" / "inside.wav", 0.25)
    write_wav(tmp_path / "outside" / "other.wav", 0.75)
    return tmp_path / "root"


def test_a_file_inside_the_root_is_read(root: Path) -> None:
    with PinnedDirectory(root) as files:
        assert float(read(files, "audio", "inside.wav").mean()) == pytest.approx(0.25, abs=0.01)


def test_a_file_replaced_by_a_link_after_validation_is_refused(root: Path) -> None:
    with PinnedDirectory(root) as files:
        (root / "audio" / "inside.wav").unlink()
        (root / "audio" / "inside.wav").symlink_to(root.parent / "outside" / "other.wav")
        with pytest.raises(AudioRefusedError):
            read(files, "audio", "inside.wav")


def test_a_directory_replaced_by_a_link_after_validation_is_refused(root: Path) -> None:
    with PinnedDirectory(root) as files:
        (root / "audio").rename(root / "audio-moved")
        (root / "audio").symlink_to(root.parent / "outside")
        write_wav(root.parent / "outside" / "inside.wav", 0.75)
        with pytest.raises(AudioRefusedError):
            read(files, "audio", "inside.wav")


def test_the_root_is_pinned_when_it_is_opened(root: Path) -> None:
    with PinnedDirectory(root) as files:
        root.rename(root.parent / "root-moved")
        (root.parent / "outside").rename(root)
        write_wav(root / "audio" / "inside.wav", 0.75)
        assert float(read(files, "audio", "inside.wav").mean()) == pytest.approx(0.25, abs=0.01)


def test_a_link_to_a_file_inside_the_root_is_refused_too(root: Path) -> None:
    (root / "audio" / "link.wav").symlink_to(root / "audio" / "inside.wav")
    with PinnedDirectory(root) as files, pytest.raises(AudioRefusedError):
        read(files, "audio", "link.wav")


def test_a_directory_a_pipe_and_an_absent_file_are_not_audio_files(root: Path) -> None:
    os.mkfifo(root / "audio" / "pipe.wav")
    with PinnedDirectory(root) as files:
        with pytest.raises(AudioRefusedError):
            files.open(("audio",))
        with pytest.raises(AudioRefusedError):
            files.open(("audio", "pipe.wav"))
        with pytest.raises(AudioMissingError):
            files.open(("audio", "absent.wav"))
        with pytest.raises(AudioMissingError):
            files.open(("absent", "inside.wav"))


def test_a_root_that_is_a_link_is_not_opened(root: Path) -> None:
    (root.parent / "link").symlink_to(root)
    with pytest.raises(OSError):
        PinnedDirectory(root.parent / "link")
