"""Stand-ins for the decoder and the models. Audio is a signal whose level encodes what is said and by whom."""

from __future__ import annotations

import io
from collections.abc import Callable, Iterator, Sequence
from dataclasses import dataclass, field
from typing import BinaryIO

import numpy as np
from numpy.typing import NDArray

from junto_transcribe.clips import FRAME_SAMPLES, SAMPLE_RATE, Span
from junto_transcribe.engines import (
    AudioDamagedError,
    AudioDecodeError,
    AudioMissingError,
    AudioRefusedError,
    Recognition,
    Samples,
    Word,
)

Burst = tuple[float, float, int]


def signal(seconds: float, bursts: Sequence[Burst]) -> Samples:
    """Silence with bursts (start seconds, end seconds, code): a burst has the constant level code / 10."""
    samples = np.zeros(round(seconds * SAMPLE_RATE), dtype=np.float32)
    for start, end, code in bursts:
        samples[round(start * SAMPLE_RATE) : round(end * SAMPLE_RATE)] = code / 10
    return samples


def code_of(samples: Samples) -> int:
    loud = samples[samples > 0.05]
    return round(float(np.median(loud)) * 10) if loud.size else 0


@dataclass
class FakeFiles:
    """Audio files by name. A file is a stream that holds its own name."""

    files: dict[str, Samples] = field(default_factory=dict)
    links: set[str] = field(default_factory=set)
    opened: list[str] = field(default_factory=list)

    def open(self, parts: Sequence[str]) -> BinaryIO:
        name = parts[-1]
        self.opened.append(name)
        if name in self.links:
            raise AudioRefusedError
        if name not in self.files:
            raise AudioMissingError
        return io.BytesIO(name.encode())


@dataclass
class FakeReader:
    files: FakeFiles
    broken: dict[str, str] = field(default_factory=dict)
    damaged: dict[str, str] = field(default_factory=dict)
    delivered: int = 0

    def blocks(
        self, source: BinaryIO, block_samples: int, max_samples: int, expired: Callable[[], bool]
    ) -> Iterator[tuple[Samples, bool]]:
        name = source.read().decode()
        if name in self.broken:
            raise AudioDecodeError(self.broken[name])
        samples = self.files.files[name]
        allowed = samples[:max_samples]
        for start in range(0, len(allowed), block_samples):
            block = allowed[start : start + block_samples]
            self.delivered += len(block)
            yield block, start + block_samples >= len(allowed)
        if len(samples) > max_samples:
            raise AudioDamagedError("audio_limit")
        if name in self.damaged:
            raise AudioDamagedError(self.damaged[name])


@dataclass
class FakeDetector:
    """A frame is speech when it is loud. With `memory`, a frame also depends on the frames before it,
    as the real detector does, so a result changes when the state is not carried between pieces."""

    memory: float = 0.0

    def stream(self) -> FakeDetectorStream:
        return FakeDetectorStream(self.memory)


@dataclass
class FakeDetectorStream:
    memory: float
    previous: float = 0.0

    def probabilities(self, samples: Samples, last: bool) -> list[float]:
        if last:
            samples = np.pad(samples, (0, FRAME_SAMPLES - len(samples) % FRAME_SAMPLES))
        assert len(samples) % FRAME_SAMPLES == 0, "a piece that is not the last one must have whole frames"
        result = []
        for level in np.abs(samples.reshape(-1, FRAME_SAMPLES)).max(axis=1):
            self.previous = self.memory * self.previous + (1.0 - self.memory) * (1.0 if level > 0.05 else 0.0)
            result.append(self.previous)
        return result


@dataclass
class FakeRecognizer:
    """Text "utterance N" for a clip with level N / 10; odd N is English and even N is Portuguese."""

    fail_on_code: int | None = None
    token_counts: dict[str, int] = field(default_factory=dict)
    calls: list[tuple[int, str | None, str | None]] = field(default_factory=list)
    clip_lengths: list[int] = field(default_factory=list)

    @property
    def languages(self) -> frozenset[str]:
        return frozenset({"en", "pt"})

    def count_tokens(self, text: str) -> int:
        return self.token_counts.get(text, len(text.split()))

    def control_token_in(self, text: str) -> bool:
        """A stand-in tokenizer with three control tokens; the third has no bracket form."""
        return any(token in text for token in ("<|en|>", "<|translate|>", "[CTRL]"))

    def recognize(
        self, samples: Samples, clips: Sequence[Span], language: str | None, glossary: str | None
    ) -> list[Recognition | None]:
        self.calls.append((len(clips), language, glossary))
        self.clip_lengths += [clip.length for clip in clips]
        results: list[Recognition | None] = []
        for clip in clips:
            part = samples[clip.start : clip.end]
            code = code_of(part)
            if code == self.fail_on_code:
                raise RuntimeError("the model failed on this clip")
            if code == 0:
                results.append(None)
                continue
            loud = np.flatnonzero(part > 0.05)
            start = clip.start + int(loud[0])
            end = clip.start + int(loud[-1]) + 1
            middle = (start + end) // 2
            results.append(
                Recognition(
                    start=start,
                    end=end,
                    text=f"utterance {code}",
                    language=language or ("en" if code % 2 else "pt"),
                    words=(Word(start, middle, " utterance"), Word(middle, end, f" {code}")),
                )
            )
        return results


@dataclass
class FakeEmbedder:
    """A voice vector per level: voices are orthogonal directions with a small deterministic variation."""

    fail: bool = False
    silent: bool = False
    calls: int = 0

    def embed(self, samples: Samples) -> NDArray[np.float32] | None:
        self.calls += 1
        if self.fail:
            raise RuntimeError("the embedding model failed")
        code = code_of(samples)
        if code == 0 or self.silent:
            return None
        vector = np.zeros(16, dtype=np.float32)
        vector[code] = 1.0
        vector += np.random.default_rng(self.calls).standard_normal(16).astype(np.float32) * 0.05
        return vector
