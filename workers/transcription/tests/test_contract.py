from __future__ import annotations

import copy
import json
from pathlib import Path
from typing import Any

import pytest

from junto_transcribe.contract import JobError, open_job, parse_job, schema_errors, write_transcript
from junto_transcribe.files import PinnedDirectory
from junto_transcribe.transcript import build_transcript
from tests.conftest import Bench, job_document, load_job
from tests.fakes import signal

SEGMENTS = [("m", "audio/m.ogg", "microphone", "p1", 0), ("s", "audio/s.ogg", "shared", "p2", 4200)]


def document(**changes: Any) -> dict[str, Any]:
    return {**job_document(SEGMENTS, glossary=["Quorvex", "Ysolde Varkonen"]), **changes}


def test_a_valid_job_is_read(tmp_path: Path) -> None:
    job = parse_job(document(language="pt"))
    assert job.recording_id == "recording-1"
    assert job.language == "pt"
    assert job.glossary == ("Quorvex", "Ysolde Varkonen")
    assert job.separate_speakers is True
    assert [(segment.id, segment.kind, segment.start_ms) for segment in job.segments] == [
        ("m", "microphone", 0),
        ("s", "shared", 4200),
    ]
    assert job.segments[0].path == ("audio", "m.ogg")
    assert [(participant.id, participant.name) for participant in job.participants] == [
        ("p1", "Name p1"),
        ("p2", "Name p2"),
    ]


def test_auto_means_no_fixed_language(tmp_path: Path) -> None:
    assert parse_job(document()).language is None


def test_paths_are_relative_to_the_job_file_unless_an_audio_root_is_given(tmp_path: Path) -> None:
    (tmp_path / "jobs").mkdir()
    job_file = tmp_path / "jobs" / "job.json"
    job_file.write_text(json.dumps(document()))
    (tmp_path / "jobs" / "audio").mkdir()
    (tmp_path / "jobs" / "audio" / "m.ogg").write_bytes(b"beside the job")
    (tmp_path / "audio").mkdir()
    (tmp_path / "audio" / "m.ogg").write_bytes(b"in the audio root")
    with open_job(job_file) as pinned:
        assert pinned.audio is pinned.directory
        assert pinned.audio.open(pinned.job.segments[0].path).read() == b"beside the job"
    with open_job(job_file, tmp_path) as pinned:
        assert pinned.audio.open(pinned.job.segments[0].path).read() == b"in the audio root"


@pytest.mark.parametrize("version", [0, 2, "1", None])
def test_another_format_version_is_refused(tmp_path: Path, version: object) -> None:
    with pytest.raises(JobError, match="format_version_not_supported"):
        parse_job(document(format_version=version))


@pytest.mark.parametrize(
    "change",
    [
        {"recording_id": ""},
        {"language": "english"},
        {"language": "EN"},
        {"glossary": ["Quorvex", "Quorvex"]},
        {"glossary": [" Quorvex"]},
        {"glossary": ["Quor\nvex"]},
        {"glossary": [""]},
        {"glossary": "Quorvex"},
        {"separate_speakers": "yes"},
        {"segments": []},
        {"unknown": 1},
    ],
)
def test_a_job_that_breaks_the_schema_is_refused(tmp_path: Path, change: dict[str, Any]) -> None:
    with pytest.raises(JobError) as refused:
        parse_job(document(**change))
    assert refused.value.code.startswith("schema.") or refused.value.code == "control_character"


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("kind", "camera"),
        ("start_ms", -1),
        ("start_ms", 1.5),
        ("participant", {"id": "p1"}),
        ("participant", {"id": "p1", "name": "Name p1", "role": "owner"}),
        ("path", ""),
        ("id", ""),
    ],
)
def test_a_segment_that_breaks_the_schema_is_refused(tmp_path: Path, field: str, value: object) -> None:
    changed = document()
    changed["segments"][0][field] = value
    with pytest.raises(JobError, match=r"/segments/0.*: schema\."):
        parse_job(changed)


def test_a_missing_field_is_refused(tmp_path: Path) -> None:
    for field in ("recording_id", "language", "glossary", "separate_speakers", "segments"):
        changed = document()
        del changed[field]
        with pytest.raises(JobError, match=field):
            parse_job(changed)


@pytest.mark.parametrize("path", ["/etc/passwd", "../outside.ogg", "audio/../../outside.ogg", "audio\\m.ogg", "."])
def test_a_path_that_could_leave_the_audio_root_is_refused(tmp_path: Path, path: str) -> None:
    changed = document()
    changed["segments"][0]["path"] = path
    with pytest.raises(JobError, match="/segments/0/path: path_not_inside"):
        parse_job(changed)


def test_segment_ids_are_unique(tmp_path: Path) -> None:
    changed = document()
    changed["segments"][1]["id"] = "m"
    with pytest.raises(JobError, match="/segments/1/id: segment_id_repeated"):
        parse_job(changed)


def test_a_participant_has_one_name(tmp_path: Path) -> None:
    changed = document()
    changed["segments"][1]["participant"] = {"id": "p1", "name": "Another name"}
    with pytest.raises(JobError, match="/segments/1/participant/name: participant_name_differs"):
        parse_job(changed)


def test_a_job_file_that_is_not_json_is_refused(tmp_path: Path) -> None:
    job_file = tmp_path / "job.json"
    job_file.write_text("{")
    with pytest.raises(JobError, match="job_file_not_json"):
        load_job(job_file)
    with pytest.raises(JobError, match="job_file_not_readable"):
        load_job(tmp_path / "absent.json")


ENGINE = {
    "worker": "0.1.0",
    "device": "cpu",
    "compute_type": "int8",
    "threads": 4,
    "models": [{"role": "whisper", "name": "small", "version": "r", "licence": "MIT", "sha256": "0" * 64}],
    "libraries": {"faster-whisper": "1.2.1"},
    "settings": {"clip_pad_ms": 400, "speaker_threshold": 0.6},
}
TIMING = {"total_ms": 10, "audio_ms": 5000, "speech_ms": 1000, "stages_ms": {"decode": 1}, "peak_memory_mb": 100}


@pytest.fixture
def transcript(bench: Bench) -> dict[str, Any]:
    bench.add("m.ogg", signal(5, [(1.0, 2.0, 1)]))
    bench.add("s.ogg", signal(12, [(1.0, 4.0, 1), (5.0, 8.0, 2), (9.0, 11.0, 1)]))
    segments = [
        ("m", "m.ogg", "microphone", "p1", 0),
        ("s", "s.ogg", "shared", "p2", 4200),
        ("x", "x.ogg", "shared", "p2", 0),
    ]
    return build_transcript(
        bench.job(segments, glossary=["Quorvex"]), bench.run(segments, glossary=["Quorvex"]), ENGINE, TIMING
    )


def test_a_built_transcript_matches_the_schema(transcript: dict[str, Any]) -> None:
    assert schema_errors("transcript", transcript) == []
    assert transcript["format_version"] == 1
    assert transcript["participants"] == [{"id": "p1", "name": "Name p1"}, {"id": "p2", "name": "Name p2"}]
    assert transcript["lines"][0] == {
        "start_ms": 1000,
        "end_ms": 2000,
        "segment_id": "m",
        "source": {"kind": "microphone", "participant_id": "p1"},
        "text": "utterance 1",
        "language": "en",
        "words": [
            {"start_ms": 1000, "end_ms": 1500, "text": " utterance"},
            {"start_ms": 1500, "end_ms": 2000, "text": " 1"},
        ],
    }
    assert transcript["lines"][1]["source"] == {"kind": "shared", "participant_id": "p2", "speaker": "Speaker 1"}
    assert transcript["glossary"] == {"token_limit": 200, "token_count": 1, "used": ["Quorvex"], "cut": []}
    assert [segment["status"] for segment in transcript["segments"]] == ["ok", "ok", "failed"]


@pytest.mark.parametrize(
    "damage",
    [
        lambda t: t.pop("engine"),
        lambda t: t.update(format_version=2),
        lambda t: t["lines"][0].update(start_ms=-1),
        lambda t: t["lines"][0].update(text=""),
        lambda t: t["lines"][0]["source"].update(speaker="Speaker 1"),
        lambda t: t["lines"][1]["source"].pop("speaker"),
        lambda t: t["lines"][1]["source"].update(speaker="Voice 1"),
        lambda t: t["lines"][0].pop("words"),
        lambda t: t["segments"][0].update(status="done"),
        lambda t: t["segments"][2].update(reason=None),
        lambda t: t["segments"][0].update(reason={"code": "file_missing", "message": ""}),
        lambda t: t["segments"][0].update(reason={"code": "other", "message": ""}, status="failed"),
        lambda t: t["glossary"].pop("cut"),
        lambda t: t["timing"].update(total_ms=1.5),
        lambda t: t.update(extra=True),
    ],
)
def test_a_transcript_that_breaks_the_schema_is_not_written(
    transcript: dict[str, Any], tmp_path: Path, damage: Any
) -> None:
    damaged = copy.deepcopy(transcript)
    damage(damaged)
    assert schema_errors("transcript", damaged) != []
    with pytest.raises(RuntimeError, match="transcript schema"):
        write_transcript(damaged, PinnedDirectory(tmp_path), "transcript.json")
    assert list(tmp_path.iterdir()) == []


def test_a_transcript_is_written_whole_and_replaces_the_old_file(transcript: dict[str, Any], tmp_path: Path) -> None:
    target = tmp_path / "transcript.json"
    target.write_text("old")
    write_transcript(transcript, PinnedDirectory(tmp_path), "transcript.json")
    assert json.loads(target.read_text("utf-8")) == transcript
    assert [path.name for path in tmp_path.iterdir()] == ["transcript.json"]


def test_the_examples_in_the_readme_match_the_schemas(tmp_path: Path) -> None:
    readme = (Path(__file__).parent.parent / "README.md").read_text("utf-8")
    blocks = [json.loads(block.split("```")[0]) for block in readme.split("```json\n")[1:]]
    job, transcript = blocks
    assert schema_errors("job", job) == []
    assert schema_errors("transcript", transcript) == []
    parsed = parse_job(job)
    assert [segment.id for segment in parsed.segments] == [segment["id"] for segment in transcript["segments"]]
