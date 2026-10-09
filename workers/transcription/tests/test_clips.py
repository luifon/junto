from __future__ import annotations

import numpy as np
import pytest

from junto_transcribe.clips import (
    FRAME_SAMPLES,
    SAMPLE_RATE,
    ClipBuilder,
    ClipRules,
    Span,
    SpanTracker,
    build_clips,
    frame_count,
    ms_from_samples,
    pad_spans,
    samples_from_ms,
    speech_spans,
)

RULES = ClipRules()
FRAME_MS = 32


def frames(total_ms: int, speech: list[tuple[int, int]], level: float = 0.9) -> tuple[list[float], int]:
    """Probabilities for `total_ms` of audio with speech in the given millisecond ranges (multiples of 32)."""
    total = samples_from_ms(total_ms)
    probabilities = [0.0] * frame_count(total)
    for start, end in speech:
        for index in range(start // FRAME_MS, end // FRAME_MS):
            probabilities[index] = level
    return probabilities, total


def seconds(spans: list[Span]) -> list[tuple[float, float]]:
    return [(span.start / SAMPLE_RATE, span.end / SAMPLE_RATE) for span in spans]


def test_silence_gives_no_clip() -> None:
    probabilities, total = frames(60_000, [])
    assert build_clips(probabilities, total, RULES) == []


def test_one_utterance_is_padded_on_both_sides() -> None:
    probabilities, total = frames(10_000, [(3200, 4800)])
    assert seconds(build_clips(probabilities, total, RULES)) == [(2.8, 5.2)]


def test_padding_stops_at_the_start_and_the_end_of_the_audio() -> None:
    probabilities, total = frames(3200, [(0, 640), (2880, 3200)])
    clips = build_clips(probabilities, total, RULES)
    assert clips[0].start == 0
    assert clips[-1].end == total


def test_a_pause_shorter_than_the_minimum_silence_does_not_split() -> None:
    probabilities, total = frames(10_000, [(1600, 3200), (3456, 4800)])
    assert seconds(speech_spans(probabilities, total, RULES)) == [(1.6, 4.8)]


def test_a_pause_with_a_silent_frame_after_the_minimum_silence_splits() -> None:
    probabilities, total = frames(10_000, [(1600, 3200), (3552, 4800)])
    assert seconds(speech_spans(probabilities, total, RULES)) == [(1.6, 3.2), (3.552, 4.8)]


def test_neighbours_closer_than_twice_the_padding_meet_in_the_middle() -> None:
    probabilities, total = frames(10_000, [(1600, 3200), (3840, 4800)])
    assert seconds(build_clips(probabilities, total, RULES)) == [(1.2, 3.52), (3.52, 5.2)]


def test_neighbours_far_apart_keep_the_full_padding() -> None:
    probabilities, total = frames(10_000, [(1600, 3200), (6400, 7040)])
    assert seconds(build_clips(probabilities, total, RULES)) == [(1.2, 3.6), (6.0, 7.44)]


def test_a_frame_between_the_thresholds_continues_speech_and_does_not_start_it() -> None:
    probabilities, total = frames(10_000, [(1600, 3200)])
    for index in range(3200 // FRAME_MS, 4160 // FRAME_MS):
        probabilities[index] = 0.4
    for index in range(6400 // FRAME_MS, 7040 // FRAME_MS):
        probabilities[index] = 0.4
    assert seconds(speech_spans(probabilities, total, RULES)) == [(1.6, 4.16)]


def test_speech_that_reaches_the_end_of_the_audio_is_kept() -> None:
    probabilities, total = frames(5000, [(3200, 5000)])
    assert build_clips(probabilities, total, RULES)[-1] == Span(samples_from_ms(2800), total)


def test_long_speech_is_split_at_its_last_short_silence() -> None:
    probabilities, total = frames(60_000, [(1600, 20_000), (20_160, 45_024)])
    spans = speech_spans(probabilities, total, RULES)
    assert seconds(spans)[0] == (1.6, 20.0)
    assert spans[1].start == samples_from_ms(20_160)
    clips = build_clips(probabilities, total, RULES)
    assert all(clip.length <= samples_from_ms(RULES.max_clip_ms) for clip in clips)
    assert clips[0].end == clips[1].start


def test_long_speech_without_any_silence_is_cut_below_the_maximum_length() -> None:
    probabilities, total = frames(100_000, [(1600, 90_016)])
    clips = build_clips(probabilities, total, RULES)
    assert len(clips) == 4
    assert all(clip.length <= samples_from_ms(RULES.max_clip_ms) for clip in clips)
    assert sum(span.length for span in speech_spans(probabilities, total, RULES)) >= samples_from_ms(88_000)


def test_speaker_padding_uses_the_same_spans_with_less_padding() -> None:
    probabilities, total = frames(10_000, [(3200, 4800)])
    spans = speech_spans(probabilities, total, RULES)
    assert seconds(pad_spans(spans, total, RULES.speaker_pad_ms)) == [(3.0, 5.0)]


def test_milliseconds_round_to_the_nearest() -> None:
    assert ms_from_samples(0) == 0
    assert ms_from_samples(16) == 1
    assert ms_from_samples(7) == 0
    assert ms_from_samples(8) == 1
    assert ms_from_samples(SAMPLE_RATE * 3600) == 3_600_000


def test_rules_reject_values_that_cannot_work() -> None:
    with pytest.raises(ValueError):
        ClipRules(threshold=0.3, release_threshold=0.5)
    with pytest.raises(ValueError):
        ClipRules(max_clip_ms=700)


def _random_probabilities(generator: np.random.Generator) -> tuple[list[float], int]:
    """Speech and silence runs of random length, with dips and levels around both thresholds."""
    values: list[float] = []
    while len(values) < generator.integers(200, 4000):
        length = int(generator.choice([1, 2, 3, 5, 9, 12, 30, 80, 300, 1000]))
        kind = generator.choice(["speech", "silence", "between", "noise"])
        if kind == "speech":
            values += [float(generator.uniform(0.5, 1.0))] * length
        elif kind == "silence":
            values += [float(generator.uniform(0.0, 0.34))] * length
        elif kind == "between":
            values += [float(generator.uniform(0.35, 0.49))] * min(length, 12)
        else:
            values += generator.uniform(0.0, 1.0, size=min(length, 30)).tolist()
    total = (len(values) - 1) * FRAME_SAMPLES + int(generator.integers(1, FRAME_SAMPLES))
    return values, total


@pytest.mark.parametrize("pad_ms", [400, 200])
def test_clips_equal_the_measured_library_function(monkeypatch: pytest.MonkeyPatch, pad_ms: int) -> None:
    """The technical test measured `faster_whisper.vad.get_speech_timestamps` with these options."""
    from faster_whisper import vad

    generator = np.random.default_rng(20261009)
    for _ in range(300):
        probabilities, total = _random_probabilities(generator)
        monkeypatch.setattr(vad, "get_vad_model", lambda p=probabilities: lambda audio: np.array(p))
        options = vad.VadOptions(min_silence_duration_ms=300, speech_pad_ms=pad_ms, max_speech_duration_s=29)
        expected = [
            (stamp["start"], stamp["end"]) for stamp in vad.get_speech_timestamps(np.zeros(total, np.float32), options)
        ]
        rules = ClipRules(clip_pad_ms=pad_ms)
        assert [(clip.start, clip.end) for clip in build_clips(probabilities, total, rules)] == expected


def test_spans_and_padding_found_piece_by_piece_equal_those_of_the_whole_sequence() -> None:
    generator = np.random.default_rng(613)
    for _ in range(300):
        probabilities, total = _random_probabilities(generator)
        spans = speech_spans(probabilities, total, RULES)
        expected = list(
            zip(
                spans,
                pad_spans(spans, total, RULES.clip_pad_ms),
                pad_spans(spans, total, RULES.speaker_pad_ms),
                strict=True,
            )
        )
        tracker = SpanTracker(RULES)
        builder = ClipBuilder(RULES)
        clips = []
        index = 0
        needed = 0
        while index < len(probabilities):
            size = int(generator.choice([1, 2, 7, 31, 250, 2000]))
            closed = tracker.feed(probabilities[index : index + size])
            index += size
            ready = builder.feed(closed, tracker.open_start, tracker.position)
            for clip in ready:
                assert clip.padded.end <= tracker.position, "a clip was given out before its audio was read"
                assert clip.padded.start >= needed, "the audio of a clip was not kept"
            clips += ready
            needed = builder.needed_from(tracker.open_start, tracker.position)
        clips += builder.feed(tracker.finish(total), None, tracker.position) + builder.finish(total)
        assert [(clip.speech, clip.padded, clip.voice) for clip in clips] == expected


def test_a_clip_waits_no_longer_than_its_padding_needs() -> None:
    probabilities, _ = frames(20_000, [(3200, 4800)])
    tracker = SpanTracker(RULES)
    builder = ClipBuilder(RULES)
    given_at = None
    for index, probability in enumerate(probabilities):
        if builder.feed(tracker.feed([probability]), tracker.open_start, tracker.position):
            given_at = (index + 1) * FRAME_MS
            break
    assert given_at is not None and 4800 + 800 <= given_at <= 4800 + 800 + 2 * FRAME_MS
