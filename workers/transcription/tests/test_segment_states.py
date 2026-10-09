"""Segments that are damaged, that leave the audio root, or whose lines get no speaker."""

from __future__ import annotations

import json
import wave
from pathlib import Path

import numpy as np

from junto_transcribe.audio import AvAudioReader
from junto_transcribe.contract import open_job, schema_errors
from junto_transcribe.pipeline import Engines, Settings, run_job
from junto_transcribe.transcript import build_transcript
from tests.conftest import Bench, job_document
from tests.fakes import FakeDetector, FakeRecognizer, signal

ENGINE = {
    "worker": "0",
    "device": "cpu",
    "compute_type": "int8",
    "threads": 1,
    "models": [],
    "libraries": {},
    "settings": {},
}
TIMING = {"total_ms": 1, "audio_ms": 1, "speech_ms": 1, "stages_ms": {}, "peak_memory_mb": None}


def test_decoding_that_stops_early_gives_a_damaged_segment_with_the_text_before_the_stop(bench: Bench) -> None:
    bench.add("cut.ogg", signal(8, [(1.0, 2.0, 1), (5.0, 6.0, 2)]))
    bench.add("whole.ogg", signal(4, [(1.0, 2.0, 3)]))
    bench.reader.damaged["cut.ogg"] = "decode_incomplete"
    segments = [("cut", "cut.ogg", "microphone", "a", 0), ("whole", "whole.ogg", "microphone", "b", 10_000)]
    outcome = bench.run(segments)

    damaged, whole = outcome.segments
    assert (damaged.status, damaged.reason_code, damaged.audio_ms, damaged.lines) == (
        "damaged",
        "decode_incomplete",
        8000,
        2,
    )
    assert (whole.status, whole.reason_code) == ("ok", None)
    assert [line.text for line in outcome.lines] == ["utterance 1", "utterance 2", "utterance 3"]

    document = build_transcript(bench.job(segments), outcome, ENGINE, TIMING)
    assert schema_errors("transcript", document) == []
    assert document["segments"][0]["status"] == "damaged"
    assert document["segments"][0]["reason"]["code"] == "decode_incomplete"
    assert document["segments"][0]["audio_ms"] == 8000


def test_a_damaged_segment_without_speech_is_damaged_and_not_empty(bench: Bench) -> None:
    bench.add("cut.ogg", signal(3, []))
    bench.reader.damaged["cut.ogg"] = "timestamp_gap"
    result = bench.run([("cut", "cut.ogg", "shared", "a", 0)]).segments[0]
    assert (result.status, result.reason_code, result.lines, result.speakers) == (
        "damaged",
        "timestamp_gap",
        0,
        "separated",
    )


def test_speech_that_reaches_the_point_of_damage_is_transcribed(bench: Bench) -> None:
    bench.add("cut.ogg", signal(3, [(1.0, 3.0, 4)]))
    bench.reader.damaged["cut.ogg"] = "decode_incomplete"
    outcome = bench.run([("cut", "cut.ogg", "microphone", "a", 0)])
    assert [(line.start_ms, line.end_ms, line.text) for line in outcome.lines] == [(1000, 3000, "utterance 4")]


def test_a_path_that_is_a_link_fails_its_segment_only(bench: Bench) -> None:
    bench.add("a.ogg", signal(4, [(1.0, 2.0, 1)]))
    bench.files.links.add("link.ogg")
    outcome = bench.run([("l", "link.ogg", "microphone", "a", 0), ("a", "a.ogg", "microphone", "b", 0)])
    assert [(result.status, result.reason_code) for result in outcome.segments] == [
        ("failed", "not_a_regular_file"),
        ("ok", None),
    ]


def write_wav(path: Path, samples: np.ndarray) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with wave.open(str(path), "wb") as file:
        file.setnchannels(1)
        file.setsampwidth(2)
        file.setframerate(16000)
        file.writeframes((samples * 32767).astype("<i2").tobytes())


def test_a_file_replaced_by_a_link_after_the_job_was_read_is_not_transcribed(tmp_path: Path) -> None:
    root = tmp_path / "job"
    write_wav(root / "audio" / "mine.wav", signal(4, [(1.0, 2.0, 1)]))
    write_wav(root / "audio" / "other.wav", signal(4, [(1.0, 2.0, 2)]))
    write_wav(tmp_path / "elsewhere" / "private.wav", signal(4, [(1.0, 2.0, 7)]))
    segments = [("mine", "audio/mine.wav", "microphone", "a", 0), ("other", "audio/other.wav", "microphone", "b", 0)]
    (root / "job.json").write_text(json.dumps(job_document(segments)))

    with open_job(root / "job.json") as pinned:
        (root / "audio" / "mine.wav").unlink()
        (root / "audio" / "mine.wav").symlink_to(tmp_path / "elsewhere" / "private.wav")
        engines = Engines(AvAudioReader(), FakeDetector(), FakeRecognizer(), None)
        outcome = run_job(pinned.job, pinned.audio, engines, Settings())

    assert [line.text for line in outcome.lines] == ["utterance 2"]
    assert [(result.status, result.reason_code) for result in outcome.segments] == [
        ("failed", "not_a_regular_file"),
        ("ok", None),
    ]


def test_lines_without_a_voice_vector_are_not_reported_as_separated(bench: Bench) -> None:
    bench.add("share.ogg", signal(10, [(1.0, 4.0, 1), (5.0, 8.0, 2)]))
    bench.embedder.silent = True
    segments = [("s", "share.ogg", "shared", "a", 0)]
    outcome = bench.run(segments)
    shared = outcome.segments[0]
    assert {line.speaker for line in outcome.lines} == {None}
    assert (shared.status, shared.speakers, shared.speakers_reason_code) == ("ok", "unassigned", "no_voice_window")
    assert outcome.speakers == ()

    document = build_transcript(bench.job(segments), outcome, ENGINE, TIMING)
    assert schema_errors("transcript", document) == []
    assert document["segments"][0]["speakers"] == "unassigned"
    assert document["segments"][0]["speakers_reason"]["code"] == "no_voice_window"
    assert document["segments"][0]["reason"] is None


def test_one_line_without_a_voice_vector_marks_its_segment_and_no_other(bench: Bench) -> None:
    bench.add("first.ogg", signal(20, [(1.0, 4.0, 1), (5.0, 8.0, 2), (9.0, 12.0, 1), (13.0, 16.0, 2)]))
    bench.add("second.ogg", signal(20, [(1.0, 4.0, 1), (5.0, 8.0, 2), (10.0, 10.4, 9)]))
    original = bench.embedder.embed

    def short_speech_has_no_vector(samples: np.ndarray) -> np.ndarray | None:
        return None if (samples > 0.85).any() else original(samples)

    bench.embedder.embed = short_speech_has_no_vector  # type: ignore[method-assign]
    outcome = bench.run([("first", "first.ogg", "shared", "a", 0), ("second", "second.ogg", "shared", "a", 30_000)])
    assert [(result.speakers, result.speakers_reason_code) for result in outcome.segments] == [
        ("separated", None),
        ("unassigned", "no_voice_window"),
    ]
    assert [line.speaker for line in outcome.lines if line.segment_index == 1] == ["Speaker 1", "Speaker 2", None]


def test_a_failed_shared_segment_is_not_reported_as_separated(bench: Bench) -> None:
    result = bench.run([("s", "absent.ogg", "shared", "a", 0)]).segments[0]
    assert (result.status, result.speakers, result.speakers_reason_code) == ("failed", "failed", "segment_failed")


def test_a_file_without_exactly_one_audio_stream_fails_its_segment_with_its_own_code(bench: Bench) -> None:
    bench.add("two.mka", signal(4, [(1.0, 2.0, 1)]))
    bench.add("a.ogg", signal(4, [(1.0, 2.0, 2)]))
    bench.reader.broken["two.mka"] = "not_one_audio_stream"
    segments = [("two", "two.mka", "microphone", "a", 0), ("a", "a.ogg", "microphone", "b", 0)]
    outcome = bench.run(segments)
    assert [(result.status, result.reason_code) for result in outcome.segments] == [
        ("failed", "not_one_audio_stream"),
        ("ok", None),
    ]
    assert schema_errors("transcript", build_transcript(bench.job(segments), outcome, ENGINE, TIMING)) == []


def test_a_timestamp_that_goes_back_gives_a_damaged_segment_with_its_own_code(bench: Bench) -> None:
    bench.add("back.ogg", signal(4, [(1.0, 2.0, 1)]))
    bench.reader.damaged["back.ogg"] = "timestamp_backward"
    segments = [("back", "back.ogg", "microphone", "a", 0)]
    outcome = bench.run(segments)
    result = outcome.segments[0]
    assert (result.status, result.reason_code, result.lines) == ("damaged", "timestamp_backward", 1)
    assert schema_errors("transcript", build_transcript(bench.job(segments), outcome, ENGINE, TIMING)) == []
