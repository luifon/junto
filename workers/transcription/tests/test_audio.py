from __future__ import annotations

import wave
from pathlib import Path

import numpy as np
import pytest

from junto_transcribe.audio import AvAudioReader
from junto_transcribe.engines import AudioDecodeError


def write_wav(path: Path, samples: np.ndarray, rate: int, channels: int = 1) -> None:
    with wave.open(str(path), "wb") as file:
        file.setnchannels(channels)
        file.setsampwidth(2)
        file.setframerate(rate)
        file.writeframes((np.repeat(samples, channels) * 32767).astype("<i2").tobytes())


def tone(seconds: float, rate: int, frequency: float = 440.0) -> np.ndarray:
    return (0.5 * np.sin(2 * np.pi * frequency * np.arange(int(seconds * rate)) / rate)).astype(np.float32)


def test_a_16_khz_mono_file_is_read_unchanged(tmp_path: Path) -> None:
    samples = tone(3.0, 16000)
    write_wav(tmp_path / "a.wav", samples, 16000)
    blocks = [block for block, _ in AvAudioReader().blocks((tmp_path / "a.wav").open("rb"), 10 * 16000)]
    assert len(blocks) == 1
    assert blocks[0].dtype == np.float32
    assert len(blocks[0]) == 48_000
    assert float(np.abs(blocks[0] - samples).max()) < 1e-3


def test_a_48_khz_stereo_file_becomes_16_khz_mono(tmp_path: Path) -> None:
    write_wav(tmp_path / "a.wav", tone(4.0, 48000), 48000, channels=2)
    samples = np.concatenate([block for block, _ in AvAudioReader().blocks((tmp_path / "a.wav").open("rb"), 16000)])
    assert abs(len(samples) - 64_000) <= 64
    spectrum = np.abs(np.fft.rfft(samples[:16000]))
    assert int(np.argmax(spectrum)) == 440
    assert 0.3 < float(np.abs(samples[1000:60_000]).max()) < 0.6


def test_blocks_have_the_asked_length_and_only_the_last_is_shorter(tmp_path: Path) -> None:
    samples = tone(70.5, 16000)
    write_wav(tmp_path / "a.wav", samples, 16000)
    blocks = list(AvAudioReader().blocks((tmp_path / "a.wav").open("rb"), 20 * 16000))
    assert [(len(block), last) for block, last in blocks] == [(320_000, False)] * 3 + [(168_000, True)]
    assert float(np.abs(np.concatenate([block for block, _ in blocks]) - samples).max()) < 1e-3


def test_a_file_that_is_not_audio_cannot_be_decoded(tmp_path: Path) -> None:
    (tmp_path / "text.ogg").write_bytes(b"this is not an audio file\n" * 400)
    with pytest.raises(AudioDecodeError), (tmp_path / "text.ogg").open("rb") as source:
        list(AvAudioReader().blocks(source, 16000))
    (tmp_path / "empty.ogg").write_bytes(b"")
    with pytest.raises(AudioDecodeError), (tmp_path / "empty.ogg").open("rb") as source:
        list(AvAudioReader().blocks(source, 16000))
