from __future__ import annotations

from collections.abc import Sequence
from pathlib import Path
from typing import Any

import pytest

from junto_transcribe.contract import MAX_JOB_FILE_BYTES, MAX_SEGMENTS, Job, open_job, parse_job
from junto_transcribe.engines import Samples
from junto_transcribe.pipeline import Engines, Outcome, Settings, run_job
from tests.fakes import FakeDetector, FakeEmbedder, FakeFiles, FakeReader, FakeRecognizer

SegmentSpec = tuple[str, str, str, str, int]


def job_document(
    segments: Sequence[SegmentSpec],
    language: str = "auto",
    glossary: Sequence[str] = (),
    separate_speakers: bool = True,
) -> dict[str, Any]:
    """Segments are (id, file name, kind, participant id, start_ms); the participant's name is "Name <id>"."""
    return {
        "format_version": 1,
        "recording_id": "recording-1",
        "language": language,
        "glossary": list(glossary),
        "separate_speakers": separate_speakers,
        "segments": [
            {
                "id": segment_id,
                "path": name,
                "kind": kind,
                "participant": {"id": participant, "name": f"Name {participant}"},
                "start_ms": start_ms,
            }
            for segment_id, name, kind, participant, start_ms in segments
        ],
    }


def load_job(
    path: Path, audio_root: Path | None = None, max_bytes: int = MAX_JOB_FILE_BYTES, max_segments: int = MAX_SEGMENTS
) -> Job:
    """The job of a file, without keeping its directories."""
    with open_job(path, audio_root, max_bytes, max_segments) as pinned:
        return pinned.job


class Clock:
    """Time that moves only when a test moves it, or by a fixed step at each reading."""

    def __init__(self, step: float = 0.0) -> None:
        self.now = 0.0
        self.step = step

    def __call__(self) -> float:
        self.now += self.step
        return self.now


class Bench:
    """A job run with the stand-in engines."""

    def __init__(self, root: Path) -> None:
        self.root = root
        self.files = FakeFiles()
        self.reader = FakeReader(self.files)
        self.recognizer = FakeRecognizer()
        self.embedder = FakeEmbedder()
        self.settings = Settings()
        self.clock = Clock()
        self.detector_memory = 0.0

    def add(self, name: str, samples: Samples) -> None:
        self.files.files[name] = samples

    def job(self, segments: Sequence[SegmentSpec], **options: Any) -> Job:
        return parse_job(job_document(segments, **options))

    def run(self, segments: Sequence[SegmentSpec], **options: Any) -> Outcome:
        engines = Engines(self.reader, FakeDetector(self.detector_memory), self.recognizer, self.embedder)
        return run_job(self.job(segments, **options), self.files, engines, self.settings, self.clock)


@pytest.fixture
def bench(tmp_path: Path) -> Bench:
    return Bench(tmp_path)
