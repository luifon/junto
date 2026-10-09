from __future__ import annotations

import pytest

from junto_transcribe.contract import JobError, schema_errors
from junto_transcribe.engines import EngineFatalError
from junto_transcribe.transcript import build_transcript
from tests.conftest import Bench
from tests.fakes import signal

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


def test_a_missing_file_fails_its_segment_and_the_others_are_transcribed(bench: Bench) -> None:
    bench.add("a.ogg", signal(5, [(1.0, 2.0, 1)]))
    bench.add("c.ogg", signal(5, [(1.0, 2.0, 3)]))
    segments = [
        ("a", "a.ogg", "microphone", "a", 0),
        ("b", "absent.ogg", "microphone", "b", 0),
        ("c", "c.ogg", "microphone", "c", 3000),
    ]
    outcome = bench.run(segments)
    assert [line.text for line in outcome.lines] == ["utterance 1", "utterance 3"]
    assert [(result.id, result.status, result.reason_code) for result in outcome.segments] == [
        ("a", "ok", None),
        ("b", "failed", "file_missing"),
        ("c", "ok", None),
    ]
    document = build_transcript(bench.job(segments), outcome, ENGINE, TIMING)
    assert schema_errors("transcript", document) == []
    assert document["segments"][1]["reason"] == {"code": "file_missing", "message": "the audio file does not exist"}


def test_a_file_that_cannot_be_decoded_fails_its_segment_only(bench: Bench) -> None:
    bench.add("a.ogg", signal(5, [(1.0, 2.0, 1)]))
    bench.add("bad.ogg", signal(1, []))
    bench.reader.broken["bad.ogg"] = "decode_failed"
    outcome = bench.run([("bad", "bad.ogg", "shared", "a", 0), ("a", "a.ogg", "microphone", "a", 0)])
    assert [(result.status, result.reason_code, result.speakers) for result in outcome.segments] == [
        ("failed", "decode_failed", "failed"),
        ("ok", None, "not_applicable"),
    ]
    assert len(outcome.lines) == 1


def test_a_model_error_fails_its_segment_and_drops_the_lines_it_already_had(bench: Bench) -> None:
    bench.add("a.ogg", signal(80, [(1.0, 2.0, 1), (70.0, 71.0, 9)]))
    bench.add("b.ogg", signal(5, [(1.0, 2.0, 2)]))
    bench.recognizer.fail_on_code = 9
    bench.settings = type(bench.settings)(clips_per_call=1)
    outcome = bench.run([("a", "a.ogg", "microphone", "a", 0), ("b", "b.ogg", "microphone", "b", 0)])
    assert [line.text for line in outcome.lines] == ["utterance 2"]
    failed = outcome.segments[0]
    assert (failed.status, failed.reason_code, failed.lines) == ("failed", "processing_failed", 0)
    assert failed.reason_message == "a processing step failed (RuntimeError)"


def test_a_failed_speaker_model_keeps_the_text_without_labels(bench: Bench) -> None:
    bench.add("share.ogg", signal(10, [(1.0, 4.0, 1), (5.0, 8.0, 2)]))
    bench.add("mic.ogg", signal(5, [(1.0, 2.0, 3)]))
    bench.embedder.fail = True
    segments = [("s", "share.ogg", "shared", "a", 0), ("m", "mic.ogg", "microphone", "a", 0)]
    outcome = bench.run(segments)
    assert [line.text for line in outcome.lines] == ["utterance 3", "utterance 1", "utterance 2"]
    assert {line.speaker for line in outcome.lines} == {None}
    shared = outcome.segments[0]
    assert (shared.status, shared.reason_code) == ("ok", None)
    assert (shared.speakers, shared.speakers_reason_code) == ("failed", "embedding_failed")
    assert schema_errors("transcript", build_transcript(bench.job(segments), outcome, ENGINE, TIMING)) == []


def test_every_segment_can_fail_and_the_job_still_gives_a_transcript(bench: Bench) -> None:
    segments = [("a", "x.ogg", "microphone", "a", 0), ("b", "y.ogg", "shared", "b", 0)]
    outcome = bench.run(segments)
    assert outcome.lines == ()
    assert [result.status for result in outcome.segments] == ["failed", "failed"]
    assert schema_errors("transcript", build_transcript(bench.job(segments), outcome, ENGINE, TIMING)) == []


def test_an_error_that_makes_the_process_unreliable_stops_the_job(bench: Bench) -> None:
    bench.add("a.ogg", signal(5, [(1.0, 2.0, 1)]))

    def out_of_memory(*_: object) -> None:
        raise EngineFatalError("CUDA failed with error out of memory")

    bench.recognizer.recognize = out_of_memory  # type: ignore[method-assign, assignment]
    with pytest.raises(EngineFatalError):
        bench.run([("a", "a.ogg", "microphone", "a", 0)])


def test_a_language_the_model_does_not_know_refuses_the_job(bench: Bench) -> None:
    bench.add("a.ogg", signal(5, [(1.0, 2.0, 1)]))
    with pytest.raises(JobError, match="/language: language_not_known_to_the_model"):
        bench.run([("a", "a.ogg", "microphone", "a", 0)], language="xx")


def test_the_glossary_cut_is_reported_and_only_the_used_terms_reach_the_model(bench: Bench) -> None:
    bench.add("a.ogg", signal(5, [(1.0, 2.0, 1)]))
    bench.settings = type(bench.settings)(glossary_token_limit=3)
    outcome = bench.run([("a", "a.ogg", "microphone", "a", 0)], glossary=["Oswin Thackerby", "Quorvex", "Lumitrax"])
    assert outcome.glossary.used == ("Oswin Thackerby", "Quorvex")
    assert outcome.glossary.cut == ("Lumitrax",)
    assert bench.recognizer.calls == [(1, None, "Oswin Thackerby, Quorvex.")]
