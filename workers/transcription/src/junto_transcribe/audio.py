"""Audio files as mono 16 kHz samples, read in blocks."""

from __future__ import annotations

import sys
from collections.abc import Callable, Iterator
from typing import Any, BinaryIO

import av
import av.audio.fifo
import av.audio.resampler
import av.error
import numpy as np

from junto_transcribe.audio_limits import MAX_GAP_MS
from junto_transcribe.clips import SAMPLE_RATE, samples_from_ms
from junto_transcribe.engines import AudioDamagedError, AudioDecodeError, Samples

# A timestamp may differ by this much from the audio decoded so far before it counts as a gap
# (later) or as a jump back (earlier than the end of the frame before it).
TIMESTAMP_TOLERANCE_SECONDS = 0.05
_FIFO_SAMPLES = 500_000
_ROUNDING_SAMPLES = 16
_ERRORS = (av.error.FFmpegError, OSError, ValueError)

Block = tuple[Samples, bool]


def _never() -> bool:
    return False


class _Output:
    """Collects converted samples up to a limit and gives them out in blocks of one length.

    A full block is given out only when more samples exist, so the block that is given out at
    the end is known to be the last one.
    """

    def __init__(self, block_samples: int, limit: int) -> None:
        self._block = block_samples
        self._limit = limit
        self._pending: list[Samples] = []
        self._count = 0
        self.total = 0
        self.cut = 0

    @property
    def room(self) -> int:
        return self._limit - self.total

    def add(self, samples: Samples) -> Iterator[Block]:
        if len(samples) > self.room:
            self.cut += len(samples) - self.room
            samples = samples[: self.room]
        if not len(samples):
            return
        self._pending.append(samples)
        self._count += len(samples)
        self.total += len(samples)
        while self._count > self._block:
            joined = np.concatenate(self._pending)
            yield joined[: self._block], False
            self._pending = [joined[self._block :]]
            self._count = len(self._pending[0])

    def silence(self, samples: int) -> Iterator[Block]:
        while samples > 0:
            piece = min(samples, self._block)
            yield from self.add(np.zeros(piece, dtype=np.float32))
            samples -= piece

    def rest(self) -> Iterator[Block]:
        if self._count:
            yield np.concatenate(self._pending), True
        self._pending = []
        self._count = 0


def decode_container(
    container: Any,
    block_samples: int,
    max_gap_samples: int,
    max_samples: int = sys.maxsize,
    expired: Callable[[], bool] = _never,
) -> Iterator[Block]:
    """Samples of the only audio stream. The first decoded sample is the start of the file.

    A gap between the timestamp of a frame and the end of the audio before it becomes silence of
    the same length, so later audio keeps its time. The audio ends, and `AudioDamagedError` is
    raised after the last block, when a packet cannot be read or decoded, when a gap is over
    `max_gap_samples`, when a timestamp jumps back, when `max_samples` are reached and more
    audio follows, or when `expired` returns true. At most one frame beyond `max_samples` is
    decoded, and silence is inserted only up to `max_samples`.
    """
    if len(container.streams.audio) != 1:
        raise AudioDecodeError("not_one_audio_stream")
    stream = container.streams.audio[0]
    resampler = av.audio.resampler.AudioResampler(format="s16", layout="mono", rate=SAMPLE_RATE)
    fifo = av.audio.fifo.AudioFifo()
    output = _Output(block_samples, max_samples)
    origin: float | None = None
    previous_end = 0.0
    source_seconds = 0.0
    inserted = 0
    damage: str | None = None

    def convert(frame: av.AudioFrame | None) -> Iterator[Block]:
        for converted in resampler.resample(frame):
            yield from output.add(converted.to_ndarray().reshape(-1).astype(np.float32) / 32768.0)

    def drain() -> Iterator[Block]:
        if fifo.samples > 0 and (grouped := fifo.read()) is not None:
            yield from convert(grouped)

    try:
        for packet in container.demux(stream):
            if expired():
                damage = "time_limit"
                break
            for frame in packet.decode():
                moment = frame.time
                if origin is None:
                    origin = moment if moment is not None else 0.0
                elif moment is not None:
                    if moment < previous_end - TIMESTAMP_TOLERANCE_SECONDS:
                        damage = "timestamp_backward"
                        break
                    gap = (moment - origin) - (source_seconds + inserted / SAMPLE_RATE)
                    if gap > TIMESTAMP_TOLERANCE_SECONDS:
                        missing = round(gap * SAMPLE_RATE)
                        if missing > max_gap_samples:
                            damage = "timestamp_gap"
                            break
                        yield from drain()
                        if missing >= output.room:
                            yield from output.silence(output.room)
                            damage = "audio_limit"
                            break
                        yield from output.silence(missing)
                        inserted += missing
                if round(source_seconds * SAMPLE_RATE) + inserted >= max_samples:
                    damage = "audio_limit"
                    break
                if moment is not None:
                    previous_end = moment + frame.samples / frame.sample_rate
                source_seconds += frame.samples / frame.sample_rate
                frame.pts = None
                fifo.write(frame)
                if fifo.samples >= _FIFO_SAMPLES:
                    yield from drain()
            if damage is not None:
                break
    except _ERRORS:
        damage = "decode_incomplete"
    try:
        yield from drain()
        yield from convert(None)
    except _ERRORS:
        damage = damage or "decode_incomplete"
    if output.total == 0:
        raise AudioDecodeError
    if damage is None and output.cut > _ROUNDING_SAMPLES:
        damage = "audio_limit"
    yield from output.rest()
    if damage is not None:
        raise AudioDamagedError(damage)


class AvAudioReader:
    def __init__(self, max_gap_ms: int = MAX_GAP_MS) -> None:
        self._max_gap_samples = samples_from_ms(max_gap_ms)

    def blocks(
        self,
        source: BinaryIO,
        block_samples: int,
        max_samples: int = sys.maxsize,
        expired: Callable[[], bool] = _never,
    ) -> Iterator[Block]:
        try:
            container = av.open(source, mode="r")
        except _ERRORS as error:
            raise AudioDecodeError from error
        with container:
            yield from decode_container(container, block_samples, self._max_gap_samples, max_samples, expired)
