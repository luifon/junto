"""Speech clips built from the frame probabilities of the voice detector."""

from __future__ import annotations

import math
from collections.abc import Iterable, Sequence
from dataclasses import dataclass

SAMPLE_RATE = 16000
FRAME_SAMPLES = 512


@dataclass(frozen=True)
class Span:
    start: int
    end: int

    @property
    def length(self) -> int:
        return self.end - self.start


@dataclass(frozen=True)
class ClipRules:
    threshold: float = 0.5
    release_threshold: float = 0.35
    min_silence_ms: int = 300
    split_silence_ms: int = 98
    clip_pad_ms: int = 400
    max_clip_ms: int = 29000
    speaker_pad_ms: int = 200

    def __post_init__(self) -> None:
        if not 0.0 < self.release_threshold <= self.threshold < 1.0:
            raise ValueError("thresholds must satisfy 0 < release <= threshold < 1")
        if min(self.min_silence_ms, self.split_silence_ms, self.clip_pad_ms, self.speaker_pad_ms) < 0:
            raise ValueError("durations must not be negative")
        if self.max_speech_samples <= 0:
            raise ValueError("max_clip_ms leaves no room for speech after padding")

    @property
    def max_speech_samples(self) -> int:
        return samples_from_ms(self.max_clip_ms) - FRAME_SAMPLES - 2 * samples_from_ms(self.clip_pad_ms)


def samples_from_ms(milliseconds: int) -> int:
    return SAMPLE_RATE * milliseconds // 1000


def ms_from_samples(samples: int) -> int:
    return (samples * 1000 + SAMPLE_RATE // 2) // SAMPLE_RATE


class SpanTracker:
    """Unpadded speech spans, found frame by frame.

    Speech starts at the first frame at or above the threshold and ends at the first frame
    below the release threshold that is followed by `min_silence_ms` without a frame at or
    above the threshold. A span that passes the maximum length ends at its last silence of
    more than `split_silence_ms`, or at the current frame when it has none.

    The spans do not depend on how the probabilities are divided between calls of `feed`.
    """

    def __init__(self, rules: ClipRules) -> None:
        self._rules = rules
        self._min_silence = samples_from_ms(rules.min_silence_ms)
        self._split_silence = samples_from_ms(rules.split_silence_ms)
        self._max_speech = rules.max_speech_samples
        self._index = 0
        self._start: int | None = None
        self._silence_start = 0
        self._split_end = 0
        self._split_resume = 0

    @property
    def position(self) -> int:
        """Samples that the frames seen so far cover."""
        return FRAME_SAMPLES * self._index

    @property
    def open_start(self) -> int | None:
        """Start of the speech that has not ended yet."""
        return self._start

    def feed(self, probabilities: Iterable[float]) -> list[Span]:
        """The spans that ended in these frames."""
        rules = self._rules
        spans: list[Span] = []
        for probability in probabilities:
            position = FRAME_SAMPLES * self._index
            self._index += 1
            voiced = probability >= rules.threshold

            if voiced and self._silence_start:
                self._silence_start = 0
                if self._split_resume < self._split_end:
                    self._split_resume = position

            if voiced and self._start is None:
                self._start = position
                continue

            if self._start is not None and position - self._start > self._max_speech:
                if self._split_end:
                    spans.append(Span(self._start, self._split_end))
                    self._start = None if self._split_resume < self._split_end else self._split_resume
                    self._silence_start = self._split_end = self._split_resume = 0
                else:
                    spans.append(Span(self._start, position))
                    self._start = None
                    self._silence_start = self._split_end = self._split_resume = 0
                    continue

            if self._start is not None and probability < rules.release_threshold:
                if not self._silence_start:
                    self._silence_start = position
                if position - self._silence_start > self._split_silence:
                    self._split_end = self._silence_start
                if position - self._silence_start < self._min_silence:
                    continue
                if self._silence_start > self._start:
                    spans.append(Span(self._start, self._silence_start))
                self._start = None
                self._silence_start = self._split_end = self._split_resume = 0
        return spans

    def finish(self, total_samples: int) -> list[Span]:
        """The span that is still open at the end of the audio."""
        start, self._start = self._start, None
        return [Span(start, total_samples)] if start is not None and total_samples > start else []


def speech_spans(probabilities: Sequence[float], total_samples: int, rules: ClipRules) -> list[Span]:
    tracker = SpanTracker(rules)
    return tracker.feed(probabilities) + tracker.finish(total_samples)


@dataclass(frozen=True)
class Clip:
    """One speech span with its padding for Whisper (`padded`) and for speaker windows (`voice`)."""

    speech: Span
    padded: Span
    voice: Span


class ClipBuilder:
    """Pads spans as `pad_spans` does, and gives out a clip as soon as its padding can no longer change.

    A span waits until the next span has ended, or until the audio after it has been silent
    for twice the padding, or until the end of the audio.
    """

    def __init__(self, rules: ClipRules) -> None:
        self._pads = (samples_from_ms(rules.clip_pad_ms), samples_from_ms(rules.speaker_pad_ms))
        self._pending: Span | None = None
        self._starts = (0, 0)
        self._first = True

    def _take(self, span: Span, starts: tuple[int, ...]) -> None:
        self._pending = span
        self._starts = (starts[0], starts[1])

    def _give(self, ends: tuple[int, ...]) -> Clip:
        assert self._pending is not None
        span, self._pending = self._pending, None
        return Clip(span, Span(self._starts[0], ends[0]), Span(self._starts[1], ends[1]))

    def feed(self, spans: Iterable[Span], open_start: int | None, position: int) -> list[Clip]:
        """Clips that are final after these spans. `position` is how far the detector has read."""
        clips: list[Clip] = []
        for span in spans:
            if self._pending is None:
                self._take(span, tuple(max(0, span.start - pad) for pad in self._pads))
                continue
            silence = span.start - self._pending.end
            ends = []
            starts = []
            for pad in self._pads:
                if silence < 2 * pad:
                    ends.append(self._pending.end + silence // 2)
                    starts.append(max(0, span.start - silence // 2))
                else:
                    ends.append(self._pending.end + pad)
                    starts.append(max(0, span.start - pad))
            clips.append(self._give(tuple(ends)))
            self._take(span, tuple(starts))
        if self._pending is not None and open_start is None and position - self._pending.end >= 2 * max(self._pads):
            clips.append(self._give(tuple(self._pending.end + pad for pad in self._pads)))
        return clips

    def finish(self, total_samples: int) -> list[Clip]:
        if self._pending is None:
            return []
        return [self._give(tuple(min(total_samples, self._pending.end + pad) for pad in self._pads))]

    def needed_from(self, open_start: int | None, position: int) -> int:
        """Audio before this sample is not needed for any clip that is still to come."""
        earliest = position if open_start is None else min(open_start, position)
        if self._pending is not None:
            earliest = min(earliest, self._pending.start)
        return max(0, earliest - max(self._pads))


def pad_spans(spans: Sequence[Span], total_samples: int, pad_ms: int) -> list[Span]:
    """Adds `pad_ms` on each side. Two neighbours closer than twice the pad meet in the middle."""
    pad = samples_from_ms(pad_ms)
    starts = [span.start for span in spans]
    ends = [span.end for span in spans]
    for index in range(len(spans)):
        if index == 0:
            starts[0] = max(0, starts[0] - pad)
        if index == len(spans) - 1:
            ends[index] = min(total_samples, ends[index] + pad)
            continue
        silence = starts[index + 1] - ends[index]
        if silence < 2 * pad:
            ends[index] += silence // 2
            starts[index + 1] = max(0, starts[index + 1] - silence // 2)
        else:
            ends[index] = min(total_samples, ends[index] + pad)
            starts[index + 1] = max(0, starts[index + 1] - pad)
    return [Span(start, end) for start, end in zip(starts, ends, strict=True)]


def build_clips(probabilities: Sequence[float], total_samples: int, rules: ClipRules) -> list[Span]:
    return pad_spans(speech_spans(probabilities, total_samples, rules), total_samples, rules.clip_pad_ms)


def frame_count(total_samples: int) -> int:
    """Frames the detector reads: the audio is padded with silence up to the next full frame."""
    return math.floor(total_samples / FRAME_SAMPLES) + 1
