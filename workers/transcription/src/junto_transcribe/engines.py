"""What the pipeline needs from the audio decoder and the three models."""

from __future__ import annotations

from collections.abc import Callable, Iterator, Sequence
from dataclasses import dataclass
from typing import BinaryIO, Protocol

import numpy as np
from numpy.typing import NDArray

from junto_transcribe.clips import Span

Samples = NDArray[np.float32]


class AudioMissingError(Exception):
    pass


class AudioRefusedError(Exception):
    """The path is not a regular file inside the audio root, or a part of it is a link."""


class AudioDecodeError(Exception):
    """Nothing could be decoded. `code` is `decode_failed` or `not_one_audio_stream`."""

    def __init__(self, code: str = "decode_failed") -> None:
        super().__init__(code)
        self.code = code


class AudioDamagedError(Exception):
    """Decoding stopped before the end of the file. Raised after the last decoded block."""

    def __init__(self, code: str) -> None:
        super().__init__(code)
        self.code = code


class EngineFatalError(Exception):
    """A model failed in a way that makes further calls in this process unreliable."""


@dataclass(frozen=True)
class Word:
    """`text` keeps its leading space: the words of a clip, joined without a separator, are its text."""

    start: int
    end: int
    text: str


@dataclass(frozen=True)
class Recognition:
    start: int
    end: int
    text: str
    language: str
    words: tuple[Word, ...] | None


class AudioFiles(Protocol):
    def open(self, parts: Sequence[str]) -> BinaryIO:
        """The audio file at this path under the audio root, as an open file."""
        ...


class AudioReader(Protocol):
    def blocks(
        self, source: BinaryIO, block_samples: int, max_samples: int, expired: Callable[[], bool]
    ) -> Iterator[tuple[Samples, bool]]:
        """Mono 16 kHz samples in blocks of `block_samples`, each with a flag that is true on the last block.

        Only the last block may be shorter. At most `max_samples` samples are given out, and the
        reader decodes at most one frame beyond them. Raises `AudioDecodeError` when nothing can
        be decoded. Raises `AudioDamagedError` after the last block when the audio ends before
        the end of the file: `decode_incomplete`, `timestamp_gap`, `timestamp_backward`,
        `audio_limit` (more audio than `max_samples`) or `time_limit` (`expired` returned true).
        """
        ...


class DetectorStream(Protocol):
    def probabilities(self, samples: Samples, last: bool) -> Sequence[float]:
        """Speech probability of each 512-sample frame of the next piece of one audio stream.

        A piece has whole frames, except the last one, which is completed with silence. The
        result for a stream does not depend on how it is divided into pieces.
        """
        ...


class SpeechDetector(Protocol):
    def stream(self) -> DetectorStream:
        """A detector for one audio stream, with no state from another stream."""
        ...


class Recognizer(Protocol):
    @property
    def languages(self) -> frozenset[str]: ...

    def count_tokens(self, text: str) -> int: ...

    def control_token_in(self, text: str) -> bool:
        """Whether the text contains the written form of a special token of the model, or is
        tokenised into one."""
        ...

    def recognize(
        self, samples: Samples, clips: Sequence[Span], language: str | None, glossary: str | None
    ) -> list[Recognition | None]:
        """One result per clip, in order. Positions are samples in `samples`. `None` means no text.

        `glossary` is the glossary text that every clip gets; its tokens were counted with `count_tokens`.
        """
        ...


class SpeakerEmbedder(Protocol):
    def embed(self, samples: Samples) -> NDArray[np.float32] | None:
        """A voice vector, or `None` when the audio is too short for the model."""
        ...
