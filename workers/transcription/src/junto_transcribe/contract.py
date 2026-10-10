"""The job file and the transcript file: reading, validation, writing."""

from __future__ import annotations

import json
from collections.abc import Iterator
from dataclasses import dataclass
from functools import cache
from importlib import resources
from itertools import islice
from pathlib import Path
from types import TracebackType
from typing import Any, Literal, cast

from jsonschema import Draft202012Validator
from jsonschema.exceptions import ValidationError

from junto_transcribe.files import FileTooLargeError, NotRegularFileError, PinnedDirectory

FORMAT_VERSION = 1
MAX_JOB_FILE_BYTES = 4 * 1024 * 1024
MAX_SEGMENTS = 2000
MAX_REPORTED_ERRORS = 5

SegmentKind = Literal["microphone", "shared"]


class JobError(Exception):
    """The job file cannot be used. No transcript is written.

    The text names a location in the job and a fixed code. It never contains a value of the job.
    """

    def __init__(self, code: str, location: str = "", note: str = "") -> None:
        self.code = code
        self.location = location
        text = f"{location or '/'}: {code}"
        super().__init__(f"{text} ({note})" if note else text)


@dataclass(frozen=True)
class Participant:
    id: str
    name: str


@dataclass(frozen=True)
class Segment:
    id: str
    path: tuple[str, ...]
    kind: SegmentKind
    participant: Participant
    start_ms: int


@dataclass(frozen=True)
class Job:
    recording_id: str
    language: str | None
    glossary: tuple[str, ...]
    separate_speakers: bool
    segments: tuple[Segment, ...]

    @property
    def participants(self) -> tuple[Participant, ...]:
        return tuple({segment.participant.id: segment.participant for segment in self.segments}.values())


@cache
def _validator(name: str) -> Draft202012Validator:
    text = resources.files("junto_transcribe").joinpath("schemas", f"{name}.schema.json").read_text("utf-8")
    schema = json.loads(text)
    Draft202012Validator.check_schema(schema)
    return Draft202012Validator(schema)


@cache
def _schema_names(name: str) -> frozenset[str]:
    """Every property name that the schema defines."""
    names: set[str] = set()
    stack: list[object] = [_validator(name).schema]
    while stack:
        node = stack.pop()
        if isinstance(node, dict):
            properties = node.get("properties")
            if isinstance(properties, dict):
                names.update(properties)
            stack.extend(node.values())
        elif isinstance(node, list):
            stack.extend(node)
    return frozenset(names)


def _part(key: object, index: int, names: frozenset[str]) -> str:
    """A location part: an array index, a name of the schema, or a placeholder. Never another key."""
    if isinstance(key, int) and not isinstance(key, bool):
        return str(key)
    return key if isinstance(key, str) and key in names else f"<key {index}>"


def _location(error: ValidationError, name: str = "job") -> str:
    names = _schema_names(name)
    parts = []
    node: object = error.root.instance if hasattr(error, "root") else None
    for key in error.absolute_path:
        index = list(node).index(key) if isinstance(node, dict) and key in node else 0
        parts.append(_part(key, index, names))
        try:
            node = node[key]  # type: ignore[index]
        except (KeyError, IndexError, TypeError):
            node = None
    return "".join(f"/{part}" for part in parts)


def _first_schema_errors(name: Literal["job", "transcript"], document: object) -> list[ValidationError]:
    return list(islice(_validator(name).iter_errors(document), MAX_REPORTED_ERRORS))


def schema_errors(name: Literal["job", "transcript"], document: object) -> list[str]:
    """At most `MAX_REPORTED_ERRORS` differences from the schema, as location and schema keyword."""
    return [
        f"{_location(error, name) or '/'}: schema.{error.validator}" for error in _first_schema_errors(name, document)
    ]


def _missing_fields(error: ValidationError) -> str:
    required = error.validator_value
    if error.validator != "required" or not isinstance(error.instance, dict) or not isinstance(required, list):
        return ""
    return "missing: " + ", ".join(name for name in required if name not in error.instance)


def _strings(document: object) -> Iterator[tuple[str, str]]:
    """Every string of the document, keys included, with a location that contains no value."""
    names = _schema_names("job")
    stack: list[tuple[object, str]] = [(document, "")]
    while stack:
        current, where = stack.pop()
        if isinstance(current, str):
            yield current, where
        elif isinstance(current, dict):
            for index, (key, value) in enumerate(current.items()):
                yield str(key), f"{where}/<key {index}>"
                stack.append((value, f"{where}/{_part(key, index, names)}"))
        elif isinstance(current, list):
            stack.extend((value, f"{where}/{index}") for index, value in enumerate(current))


def _refuse_unsafe_characters(document: object) -> None:
    """Control characters and lone surrogates are refused in every string, before any other use."""
    for text, location in _strings(document):
        for character in text:
            code = ord(character)
            if 0xD800 <= code <= 0xDFFF:
                raise JobError("lone_surrogate", location)
            if code < 0x20 or 0x7F <= code <= 0x9F:
                raise JobError("control_character", location)


def _refuse_control_token_forms(document: object) -> None:
    """A glossary term may not contain the written form of a Whisper control token, `<|...|>`.

    The loaded tokenizer checks the terms again (`pipeline.run_job`); this check needs no model.
    """
    glossary = document.get("glossary") if isinstance(document, dict) else None
    if isinstance(glossary, list):
        for index, term in enumerate(glossary):
            if isinstance(term, str) and ("<|" in term or "|>" in term):
                raise JobError("glossary_control_token", f"/glossary/{index}")


def _path_parts(relative: str, location: str) -> tuple[str, ...]:
    parts = tuple(relative.split("/"))
    if "\\" in relative or any(part in ("", ".", "..") for part in parts):
        raise JobError("path_not_inside", location, "a relative path with '/' separators and no '.' or '..' part")
    return parts


def parse_job(document: object, max_segments: int = MAX_SEGMENTS) -> Job:
    _refuse_unsafe_characters(document)
    _refuse_control_token_forms(document)
    if isinstance(document, dict) and document.get("format_version") != FORMAT_VERSION:
        raise JobError("format_version_not_supported", "/format_version", f"this worker reads version {FORMAT_VERSION}")
    try:
        errors = _first_schema_errors("job", document)
    except RecursionError as error:
        raise JobError("job_file_nested_too_deep") from error
    if errors:
        first = errors[0]
        others = ", ".join(f"{_location(error) or '/'}: schema.{error.validator}" for error in errors[1:])
        note = "; ".join(part for part in (_missing_fields(first), f"also {others}" if others else "") if part)
        raise JobError(f"schema.{first.validator}", _location(first), note)
    data = cast(dict[str, Any], document)
    if len(data["segments"]) > max_segments:
        raise JobError("too_many_segments", "/segments", f"the limit is {max_segments}")

    segments: list[Segment] = []
    seen: set[str] = set()
    names: dict[str, str] = {}
    for index, item in enumerate(data["segments"]):
        where = f"/segments/{index}"
        if item["id"] in seen:
            raise JobError("segment_id_repeated", f"{where}/id")
        seen.add(item["id"])
        participant = Participant(item["participant"]["id"], item["participant"]["name"])
        if names.setdefault(participant.id, participant.name) != participant.name:
            raise JobError("participant_name_differs", f"{where}/participant/name")
        segments.append(
            Segment(
                id=item["id"],
                path=_path_parts(item["path"], f"{where}/path"),
                kind=item["kind"],
                participant=participant,
                start_ms=item["start_ms"],
            )
        )
    return Job(
        recording_id=data["recording_id"],
        language=None if data["language"] == "auto" else data["language"],
        glossary=tuple(data["glossary"]),
        separate_speakers=data["separate_speakers"],
        segments=tuple(segments),
    )


@dataclass
class JobInput:
    """A job with the directories it uses, pinned before any other work.

    `audio` is the audio root. `directory` is the directory of the job file, and is the same
    object unless another audio root was named.
    """

    job: Job
    directory: PinnedDirectory
    audio: PinnedDirectory

    def __enter__(self) -> JobInput:
        return self

    def __exit__(
        self, kind: type[BaseException] | None, error: BaseException | None, trace: TracebackType | None
    ) -> None:
        self.close()

    def close(self) -> None:
        self.audio.close()
        self.directory.close()


def _pin(directory: Path, code: str) -> PinnedDirectory:
    try:
        return PinnedDirectory(directory)
    except OSError as error:
        raise JobError(code, note=type(error).__name__) from error


def _parse_content(content: bytes, max_segments: int) -> Job:
    try:
        document = json.loads(content)
    except UnicodeDecodeError as error:
        raise JobError("job_file_not_utf8") from error
    except RecursionError as error:
        raise JobError("job_file_nested_too_deep") from error
    except ValueError as error:
        position = f"line {error.lineno}, column {error.colno}" if isinstance(error, json.JSONDecodeError) else ""
        raise JobError("job_file_not_json", note=position) from error
    return parse_job(document, max_segments)


def open_job(
    path: Path,
    audio_root: Path | None = None,
    max_bytes: int = MAX_JOB_FILE_BYTES,
    max_segments: int = MAX_SEGMENTS,
) -> JobInput:
    """Pins the directory of the job file, and the audio root when it is another directory, and reads the job.

    A directory that is a link is refused. The job file is read from the pinned directory, must
    be a regular file and not a link, and is read up to `max_bytes`.
    """
    directory = _pin(path.parent, "job_directory_not_usable")
    audio = directory
    try:
        if audio_root is not None:
            audio = _pin(audio_root, "audio_root_not_usable")
        try:
            content = directory.read(path.name, max_bytes)
        except NotRegularFileError as error:
            raise JobError("job_file_not_regular") from error
        except FileTooLargeError as error:
            raise JobError("job_file_too_large", note=f"the limit is {max_bytes} bytes") from error
        except OSError as error:
            raise JobError("job_file_not_readable", note=type(error).__name__) from error
        return JobInput(_parse_content(content, max_segments), directory, audio)
    except BaseException:
        audio.close()
        directory.close()
        raise


def write_transcript(document: dict[str, Any], directory: PinnedDirectory, name: str) -> None:
    """Validates the transcript and puts it at `name` in one step, so a reader never sees a partial file."""
    errors = schema_errors("transcript", document)
    if errors:
        raise RuntimeError("the transcript does not match the transcript schema: " + "; ".join(errors))
    directory.write(name, (json.dumps(document, ensure_ascii=False, indent=1) + "\n").encode("utf-8"))
