"""Bounds on what one job may ask of the worker."""

from __future__ import annotations

from dataclasses import dataclass, fields

from junto_transcribe.audio_limits import MAX_GAP_MS
from junto_transcribe.contract import MAX_JOB_FILE_BYTES, MAX_SEGMENTS

_HOUR_MS = 3_600_000


@dataclass(frozen=True)
class Limits:
    """Every value has a command-line option and an environment variable; see the README."""

    job_file_bytes: int = MAX_JOB_FILE_BYTES
    segments: int = MAX_SEGMENTS
    segment_audio_ms: int = 6 * _HOUR_MS
    job_audio_ms: int = 24 * _HOUR_MS
    processing_ms: int = 12 * _HOUR_MS
    gap_ms: int = MAX_GAP_MS
    speaker_windows: int = 60_000

    def __post_init__(self) -> None:
        for item in fields(self):
            if getattr(self, item.name) < 1:
                raise ValueError(f"{item.name} must be at least 1")
