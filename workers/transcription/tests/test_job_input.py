"""What a job file may contain, how much of it is read, and what an error may reveal."""

from __future__ import annotations

import json
import logging
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pytest

from junto_transcribe import cli, contract
from junto_transcribe.contract import JobError, parse_job
from tests.conftest import job_document, load_job

SECRET = "S3CRET-VALUE"
SEGMENTS = [("m", "audio/m.ogg", "microphone", "p1", 0), ("s", "audio/s.ogg", "shared", "p2", 4200)]


def document(**changes: Any) -> dict[str, Any]:
    return {**job_document(SEGMENTS, glossary=["Quorvex"]), **changes}


def with_segment_field(field: str, value: object) -> dict[str, Any]:
    changed = document()
    if field.startswith("participant."):
        changed["segments"][0]["participant"][field.split(".")[1]] = value
    else:
        changed["segments"][0][field] = value
    return changed


LINE_BREAK_CASES = {
    "recording_id": document(recording_id="recording\n"),
    "language": document(language="en\n"),
    "glossary": document(glossary=["Quorvex\n"]),
    "segment id": with_segment_field("id", "m\n"),
    "path": with_segment_field("path", "audio/m.ogg\n"),
    "participant id": with_segment_field("participant.id", "p1\n"),
    "participant name": with_segment_field("participant.name", "Name p1\n"),
}


@pytest.mark.parametrize("case", LINE_BREAK_CASES)
def test_a_string_that_ends_with_a_line_break_is_refused(tmp_path: Path, case: str) -> None:
    with pytest.raises(JobError):
        parse_job(LINE_BREAK_CASES[case])


@pytest.mark.parametrize(
    "changed",
    [
        with_segment_field("participant.name", "Name \ud800"),
        with_segment_field("id", "\udfff"),
        document(glossary=["Quor\ud83dvex"]),
        document(recording_id="rec\udc00"),
    ],
)
def test_a_lone_surrogate_is_refused_before_it_can_break_the_transcript(
    tmp_path: Path, changed: dict[str, Any]
) -> None:
    with pytest.raises(JobError) as refused:
        parse_job(json.loads(json.dumps(changed)))
    assert refused.value.code == "lone_surrogate"


@pytest.mark.parametrize("character", ["\x00", "\x07", "\x1b", "\x7f", "\x85", "\r", "\t"])
def test_a_control_character_is_refused_in_every_string(tmp_path: Path, character: str) -> None:
    for changed in (
        document(recording_id=f"rec{character}ording"),
        document(glossary=[f"Quor{character}vex"]),
        with_segment_field("participant.name", f"Name{character}p1"),
        with_segment_field("path", f"audio/m{character}.ogg"),
    ):
        with pytest.raises(JobError) as refused:
            parse_job(changed)
        assert refused.value.code == "control_character"


def test_an_object_key_with_a_control_character_is_refused(tmp_path: Path) -> None:
    with pytest.raises(JobError) as refused:
        parse_job({**document(), "extra\n": 1})
    assert refused.value.code == "control_character"


PRIVATE_CASES: dict[str, tuple[dict[str, Any], str, str]] = {
    "glossary term with spaces at the end": (document(glossary=[f"{SECRET} "]), "/glossary/0", "schema.pattern"),
    "oversized participant name": (
        with_segment_field("participant.name", SECRET * 40),
        "/segments/0/participant/name",
        "schema.maxLength",
    ),
    "repeated glossary terms": (document(glossary=[SECRET, "Quorvex", SECRET]), "/glossary", "schema.uniqueItems"),
    "unknown property": ({**document(), SECRET: 1}, "", "schema.additionalProperties"),
    "wrong kind": (with_segment_field("kind", SECRET), "/segments/0/kind", "schema.enum"),
    "wrong language": (document(language=SECRET), "/language", "schema.pattern"),
    "path that leaves the root": (
        with_segment_field("path", f"../{SECRET}.ogg"),
        "/segments/0/path",
        "path_not_inside",
    ),
    "absolute path": (with_segment_field("path", f"/{SECRET}.ogg"), "/segments/0/path", "path_not_inside"),
    "segment id twice": (
        {
            **document(),
            "segments": [document()["segments"][0] | {"id": SECRET}, document()["segments"][1] | {"id": SECRET}],
        },
        "/segments/1/id",
        "segment_id_repeated",
    ),
    "participant with two names": (
        {
            **document(),
            "segments": [
                document()["segments"][0] | {"participant": {"id": "p1", "name": SECRET}},
                document()["segments"][1] | {"participant": {"id": "p1", "name": f"{SECRET} 2"}},
            ],
        },
        "/segments/1/participant/name",
        "participant_name_differs",
    ),
    "wrong version": (document(format_version=SECRET), "/format_version", "format_version_not_supported"),
    "missing field": (
        {k: v for k, v in document(recording_id=SECRET).items() if k != "language"},
        "",
        "schema.required",
    ),
}


@pytest.mark.parametrize("case", PRIVATE_CASES)
def test_a_refused_job_names_the_field_and_a_code_and_no_value(
    tmp_path: Path, caplog: pytest.LogCaptureFixture, capsys: pytest.CaptureFixture[str], case: str
) -> None:
    changed, location, code = PRIVATE_CASES[case]
    with pytest.raises(JobError) as refused:
        parse_job(changed)
    assert SECRET not in str(refused.value)
    assert (refused.value.location, refused.value.code) == (location, code)

    job_file = tmp_path / "job.json"
    job_file.write_text(json.dumps(changed), "utf-8")
    with caplog.at_level(logging.DEBUG):
        exit_code = cli.main(["run", str(job_file), "--out", str(tmp_path / "t.json"), "--models-dir", str(tmp_path)])
    captured = capsys.readouterr()
    assert exit_code == cli.EXIT_JOB
    assert SECRET not in caplog.text + captured.err + captured.out
    assert code in caplog.text


def test_a_missing_field_is_named_because_the_name_comes_from_the_schema(tmp_path: Path) -> None:
    changed = document()
    del changed["segments"][1]["kind"]
    with pytest.raises(JobError) as refused:
        parse_job(changed)
    assert (refused.value.location, refused.value.code) == ("/segments/1", "schema.required")
    assert "kind" in str(refused.value)


def test_json_that_cannot_be_parsed_is_refused_without_its_content(tmp_path: Path) -> None:
    job_file = tmp_path / "job.json"
    job_file.write_text('{"recording_id": "' + SECRET + '", ]', "utf-8")
    with pytest.raises(JobError) as refused:
        load_job(job_file)
    assert refused.value.code == "job_file_not_json"
    assert SECRET not in str(refused.value)


def test_a_job_file_over_the_byte_limit_is_refused_before_it_is_parsed(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    job_file = tmp_path / "job.json"
    job_file.write_text(json.dumps(document()) + " " * 5_000_000, "utf-8")

    def not_reached(*_: object, **__: object) -> None:
        raise AssertionError("a file over the limit was parsed")

    monkeypatch.setattr(json, "loads", not_reached)
    with pytest.raises(JobError) as refused:
        load_job(job_file)
    assert refused.value.code == "job_file_too_large"


def test_the_byte_limit_of_the_job_file_is_a_setting(tmp_path: Path) -> None:
    job_file = tmp_path / "job.json"
    job_file.write_text(json.dumps(document()), "utf-8")
    size = job_file.stat().st_size
    assert load_job(job_file, max_bytes=size).recording_id == "recording-1"
    with pytest.raises(JobError) as refused:
        load_job(job_file, max_bytes=size - 1)
    assert refused.value.code == "job_file_too_large"


@pytest.mark.parametrize("path", ["/dev/null", "/dev/zero"])
def test_a_job_file_that_is_not_a_regular_file_is_refused_without_reading_it(path: str) -> None:
    with pytest.raises(JobError) as refused:
        load_job(Path(path))
    assert refused.value.code == "job_file_not_regular"


def test_a_directory_and_an_absent_file_are_refused(tmp_path: Path) -> None:
    with pytest.raises(JobError) as refused:
        load_job(tmp_path)
    assert refused.value.code == "job_file_not_regular"
    with pytest.raises(JobError) as refused:
        load_job(tmp_path / "absent.json")
    assert refused.value.code == "job_file_not_readable"


def test_validation_stops_after_a_few_errors(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    changed = document(glossary=[f" term {number}" for number in range(1500)])
    produced = 0
    validator = contract._validator("job")
    original = type(validator).iter_errors

    def counted(self: Any, instance: Any, *arguments: Any, **options: Any) -> Iterator[Any]:
        nonlocal produced
        for error in original(self, instance, *arguments, **options):
            if self is validator:
                produced += 1
            yield error

    monkeypatch.setattr(type(validator), "iter_errors", counted)
    with pytest.raises(JobError) as refused:
        parse_job(changed)
    assert refused.value.code == "schema.pattern"
    assert produced <= contract.MAX_REPORTED_ERRORS + 1
    assert len(str(refused.value)) < 600


def test_more_segments_than_the_limit_are_refused(tmp_path: Path) -> None:
    many = [(f"s{number}", f"s{number}.ogg", "microphone", "p1", 0) for number in range(6)]
    assert len(parse_job(job_document(many), max_segments=6).segments) == 6
    with pytest.raises(JobError) as refused:
        parse_job(job_document(many), max_segments=5)
    assert (refused.value.location, refused.value.code) == ("/segments", "too_many_segments")


@pytest.mark.parametrize(
    "changed",
    [
        {**document(), "S3CRET_VALUE": "\n"},
        {**document(), "S3CRET_VALUE": {"inner": ["\x00"]}},
        {**document(), "S3CRET_VALUE\n": 1},
        {**document(), "segments": [{**document()["segments"][0], "S3CRET_VALUE": "\x07"}]},
    ],
)
def test_a_location_never_contains_a_key_that_the_schema_does_not_define(
    tmp_path: Path, caplog: pytest.LogCaptureFixture, capsys: pytest.CaptureFixture[str], changed: dict[str, Any]
) -> None:
    with pytest.raises(JobError) as refused:
        parse_job(changed)
    assert refused.value.code == "control_character"
    assert "S3CRET" not in str(refused.value) + refused.value.location
    assert "<key" in refused.value.location

    job_file = tmp_path / "job.json"
    job_file.write_text(json.dumps(changed), "utf-8")
    with caplog.at_level(logging.DEBUG):
        cli.main(["run", str(job_file), "--out", str(tmp_path / "t.json"), "--models-dir", str(tmp_path)])
    captured = capsys.readouterr()
    assert "S3CRET" not in caplog.text + captured.err + captured.out


def test_a_location_keeps_the_names_that_the_schema_defines() -> None:
    changed = document()
    changed["segments"][1]["participant"]["name"] = "Name\tp2"
    with pytest.raises(JobError) as refused:
        parse_job(changed)
    assert refused.value.location == "/segments/1/participant/name"


def test_the_job_file_is_not_read_beyond_its_limit(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    import os

    job_file = tmp_path / "job.json"
    job_file.write_text(json.dumps(document()) + " " * 3_000_000, "utf-8")
    read = 0
    original = os.read

    def counting(descriptor: int, size: int) -> bytes:
        nonlocal read
        data: bytes = original(descriptor, size)
        read += len(data)
        return data

    monkeypatch.setattr(os, "read", counting)
    with pytest.raises(JobError) as refused:
        load_job(job_file, max_bytes=10_000)
    assert refused.value.code == "job_file_too_large"
    assert 10_000 < read <= 10_001
