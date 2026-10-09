"""Limits on audio length, processing time and speaker data."""

from __future__ import annotations

import tracemalloc
from dataclasses import replace

import numpy as np
import pytest

from junto_transcribe import speakers
from junto_transcribe.limits import Limits
from junto_transcribe.speakers import SpeakerRules, cluster_speakers
from tests.conftest import Bench
from tests.fakes import signal
from tests.test_speakers import same_partition, voices


def with_limits(bench: Bench, **limits: int) -> None:
    bench.settings = replace(bench.settings, limits=replace(bench.settings.limits, **limits))


def test_a_segment_over_the_duration_limit_is_transcribed_up_to_the_limit_and_reported(bench: Bench) -> None:
    bench.add("long.ogg", signal(100, [(10.0, 12.0, 1), (50.0, 52.0, 2), (90.0, 92.0, 3)]))
    with_limits(bench, segment_audio_ms=60_000)
    outcome = bench.run([("long", "long.ogg", "microphone", "a", 0)])
    result = outcome.segments[0]
    assert (result.status, result.reason_code, result.audio_ms) == ("damaged", "segment_duration_limit", 60_000)
    assert [line.text for line in outcome.lines] == ["utterance 1", "utterance 2"]


def test_a_segment_of_exactly_the_limit_is_whole(bench: Bench) -> None:
    bench.add("exact.ogg", signal(60, [(10.0, 12.0, 1)]))
    with_limits(bench, segment_audio_ms=60_000)
    result = bench.run([("exact", "exact.ogg", "microphone", "a", 0)]).segments[0]
    assert (result.status, result.reason_code, result.audio_ms) == ("ok", None, 60_000)


def test_speech_at_the_limit_is_cut_at_the_limit(bench: Bench) -> None:
    bench.add("long.ogg", signal(100, [(58.0, 64.0, 5)]))
    with_limits(bench, segment_audio_ms=60_000)
    outcome = bench.run([("long", "long.ogg", "microphone", "a", 0)])
    assert [(line.start_ms, line.end_ms) for line in outcome.lines] == [(58_000, 60_000)]


def test_the_job_duration_limit_cuts_the_segment_that_reaches_it_and_fails_the_later_ones(bench: Bench) -> None:
    for name in ("a.ogg", "b.ogg", "c.ogg", "d.ogg"):
        bench.add(name, signal(40, [(5.0, 6.0, 1), (30.0, 31.0, 2)]))
    with_limits(bench, job_audio_ms=100_000)
    outcome = bench.run([(name[0], name, "microphone", name[0], 0) for name in ("a.ogg", "b.ogg", "c.ogg", "d.ogg")])
    assert [(result.status, result.reason_code, result.audio_ms) for result in outcome.segments] == [
        ("ok", None, 40_000),
        ("ok", None, 40_000),
        ("damaged", "job_duration_limit", 20_000),
        ("failed", "job_duration_limit", 0),
    ]
    assert bench.files.opened == ["a.ogg", "b.ogg", "c.ogg"]
    assert len(outcome.lines) == 5


def test_the_processing_time_limit_stops_the_current_segment_and_fails_the_later_ones(bench: Bench) -> None:
    bench.add("a.ogg", signal(40, [(start, start + 1.0, 1) for start in (1, 8, 15, 22, 29)]))
    bench.add("b.ogg", signal(5, [(1.0, 2.0, 2)]))
    bench.settings = replace(bench.settings, clips_per_call=1)
    with_limits(bench, processing_ms=250_000)
    recognize = bench.recognizer.recognize

    def slow(*arguments: object) -> object:
        bench.clock.now += 100.0
        return recognize(*arguments)  # type: ignore[arg-type]

    bench.recognizer.recognize = slow  # type: ignore[method-assign, assignment]
    outcome = bench.run([("a", "a.ogg", "microphone", "a", 0), ("b", "b.ogg", "microphone", "b", 0)])
    assert [(result.status, result.reason_code, result.lines) for result in outcome.segments] == [
        ("damaged", "time_limit", 3),
        ("failed", "time_limit", 0),
    ]
    assert bench.files.opened == ["a.ogg"]


def test_the_limit_of_voice_windows_stops_the_labels_and_is_reported(bench: Bench) -> None:
    turns = [(1.0 + index * 4.0, 4.0 + index * 4.0, 1 + index % 2) for index in range(6)]
    bench.add("share.ogg", signal(30, turns))
    with_limits(bench, speaker_windows=8)
    outcome = bench.run([("s", "share.ogg", "shared", "a", 0)])
    result = outcome.segments[0]
    assert bench.embedder.calls == 8
    assert [line.speaker for line in outcome.lines] == ["Speaker 1", "Speaker 2", "Speaker 1", "Speaker 2", None, None]
    assert (result.status, result.speakers, result.speakers_reason_code) == ("ok", "unassigned", "speaker_window_limit")


def test_a_limit_must_be_positive() -> None:
    for name in (
        "job_file_bytes",
        "segments",
        "segment_audio_ms",
        "job_audio_ms",
        "processing_ms",
        "gap_ms",
        "speaker_windows",
    ):
        with pytest.raises(ValueError, match=name):
            Limits(**{name: 0})


def test_the_default_limits_are_the_documented_ones() -> None:
    assert Limits() == Limits(
        job_file_bytes=4 * 1024 * 1024,
        segments=2000,
        segment_audio_ms=6 * 3_600_000,
        job_audio_ms=24 * 3_600_000,
        processing_ms=12 * 3_600_000,
        gap_ms=3_600_000,
        speaker_windows=60_000,
    )


def test_vectors_over_the_clustering_limit_are_assigned_in_batches(monkeypatch: pytest.MonkeyPatch) -> None:
    vectors, truth = voices([400, 300, 200], seed=11)
    monkeypatch.setattr(speakers, "ASSIGN_BATCH", 37)
    sizes: list[int] = []
    unit_rows = speakers.unit_rows

    def counting(rows: np.ndarray) -> np.ndarray:
        sizes.append(len(rows))
        return unit_rows(rows)

    monkeypatch.setattr(speakers, "unit_rows", counting)
    assert same_partition(cluster_speakers(vectors, SpeakerRules(max_cluster_windows=150)).tolist(), truth)
    assert sizes[0] == 150, "the clustered subset is every sixth vector"
    assert sizes[1:] == [37] * 24 + [12], "every other vector is assigned in batches"


def test_clustering_many_vectors_needs_little_memory_beyond_the_vectors() -> None:
    generator = np.random.default_rng(4)
    centres = generator.standard_normal((3, 192)).astype(np.float32)
    vectors = (centres[generator.integers(0, 3, 60_000)] + 0.2 * generator.standard_normal((60_000, 192))).astype(
        np.float32
    )
    tracemalloc.start()
    labels = cluster_speakers(vectors, SpeakerRules(max_cluster_windows=500))
    _, peak = tracemalloc.get_traced_memory()
    tracemalloc.stop()
    assert len(set(labels.tolist())) == 3
    assert vectors.nbytes == 46_080_000
    assert peak < 16_000_000


# Second review, B2: the limits bound the decoding work, not only what is kept.


def test_a_segment_limit_stops_the_decoding_at_the_limit(bench: Bench) -> None:
    bench.add("hour.ogg", signal(3600, [(2.0, 4.0, 1), (1000.0, 1002.0, 2)]))
    with_limits(bench, segment_audio_ms=10_000)
    outcome = bench.run([("hour", "hour.ogg", "microphone", "a", 0)])
    result = outcome.segments[0]
    assert (result.status, result.reason_code, result.audio_ms) == ("damaged", "segment_duration_limit", 10_000)
    assert bench.reader.delivered <= 11 * 16000
    assert [line.text for line in outcome.lines] == ["utterance 1"]


def test_many_segments_of_one_long_file_decode_no_more_than_the_job_limit(bench: Bench) -> None:
    bench.add("hour.ogg", signal(3600, [(2.0, 4.0, 1)]))
    with_limits(bench, segment_audio_ms=10_000, job_audio_ms=35_000)
    outcome = bench.run([(f"s{index}", "hour.ogg", "microphone", "a", 0) for index in range(6)])
    assert [(result.status, result.reason_code, result.audio_ms) for result in outcome.segments] == [
        ("damaged", "segment_duration_limit", 10_000),
        ("damaged", "segment_duration_limit", 10_000),
        ("damaged", "segment_duration_limit", 10_000),
        ("damaged", "job_duration_limit", 5000),
        ("failed", "job_duration_limit", 0),
        ("failed", "job_duration_limit", 0),
    ]
    assert bench.reader.delivered <= 39 * 16000


def test_processed_audio_is_reported_apart_from_decoded_audio(bench: Bench) -> None:
    bench.add("a.ogg", signal(40, [(start, start + 1.0, 1) for start in (1, 8, 15, 22, 29)]))
    bench.settings = replace(bench.settings, clips_per_call=1)
    with_limits(bench, processing_ms=250_000)
    recognize = bench.recognizer.recognize

    def slow(*arguments: object) -> object:
        bench.clock.now += 100.0
        return recognize(*arguments)  # type: ignore[arg-type]

    bench.recognizer.recognize = slow  # type: ignore[method-assign, assignment]
    result = bench.run([("a", "a.ogg", "microphone", "a", 0)]).segments[0]
    assert (result.status, result.reason_code, result.lines) == ("damaged", "time_limit", 3)
    assert result.audio_ms == 40_000
    assert 16_000 < result.processed_ms <= 22_000

    bench.clock.now = 0.0
    with_limits(bench, processing_ms=10**9)
    whole = bench.run([("a", "a.ogg", "microphone", "a", 0)]).segments[0]
    assert (whole.status, whole.audio_ms, whole.processed_ms) == ("ok", 40_000, 40_000)


# Second review, B7: the deadline also stops speaker work, and the skipped work is reported.


def test_the_deadline_stops_the_embedding_of_voice_windows(bench: Bench) -> None:
    turns = [(1.0 + index * 4.0, 4.0 + index * 4.0, 1 + index % 2) for index in range(6)]
    bench.add("share.ogg", signal(30, turns))
    with_limits(bench, processing_ms=250_000)
    embed = bench.embedder.embed

    def slow(samples: np.ndarray) -> np.ndarray | None:
        bench.clock.now += 100.0
        return embed(samples)

    bench.embedder.embed = slow  # type: ignore[method-assign]
    outcome = bench.run([("s", "share.ogg", "shared", "a", 0)])
    result = outcome.segments[0]
    assert bench.embedder.calls == 3
    assert (result.status, result.reason_code) == ("damaged", "time_limit")
    assert (result.speakers, result.speakers_reason_code) == ("failed", "time_limit")
    assert bench.recognizer.calls == []
    assert outcome.lines == ()
    assert result.processed_ms < result.audio_ms == 30_000


def test_the_deadline_stops_the_clustering_of_later_participants(bench: Bench) -> None:
    turns = [(1.0 + index * 4.0, 4.0 + index * 4.0, 1 + index % 2) for index in range(6)]
    bench.add("a.ogg", signal(30, turns))
    bench.add("b.ogg", signal(30, turns))
    bench.add("late.ogg", signal(5, [(1.0, 2.0, 3)]))
    with_limits(bench, processing_ms=250_000)
    recognize = bench.recognizer.recognize

    def slow_on_the_last_file(samples: np.ndarray, *arguments: object) -> object:
        found = recognize(samples, *arguments)  # type: ignore[arg-type]
        if any(item is not None and item.text == "utterance 3" for item in found):
            bench.clock.now += 1000.0
        return found

    bench.recognizer.recognize = slow_on_the_last_file  # type: ignore[method-assign, assignment]
    outcome = bench.run(
        [("a", "a.ogg", "shared", "a", 0), ("b", "b.ogg", "shared", "b", 0), ("late", "late.ogg", "microphone", "c", 0)]
    )
    assert [(result.speakers, result.speakers_reason_code) for result in outcome.segments[:2]] == [
        ("failed", "time_limit"),
        ("failed", "time_limit"),
    ]
    assert {line.speaker for line in outcome.lines} == {None}
    assert len(outcome.lines) == 13
