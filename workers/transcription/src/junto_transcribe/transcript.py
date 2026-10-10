"""The transcript document built from the result of a job."""

from __future__ import annotations

from typing import Any

from junto_transcribe.contract import FORMAT_VERSION, Job
from junto_transcribe.pipeline import Line, Outcome, SegmentResult


def _line(job: Job, line: Line) -> dict[str, Any]:
    segment = job.segments[line.segment_index]
    source: dict[str, Any] = {"kind": segment.kind, "participant_id": segment.participant.id}
    if segment.kind == "shared":
        source["speaker"] = line.speaker
    return {
        "start_ms": line.start_ms,
        "end_ms": line.end_ms,
        "segment_id": segment.id,
        "source": source,
        "text": line.text,
        "language": line.language,
        "words": None
        if line.words is None
        else [{"start_ms": word.start_ms, "end_ms": word.end_ms, "text": word.text} for word in line.words],
    }


def _reason(code: str | None, message: str | None) -> dict[str, str] | None:
    return None if code is None else {"code": code, "message": message or ""}


def _segment(result: SegmentResult) -> dict[str, Any]:
    return {
        "id": result.id,
        "status": result.status,
        "reason": _reason(result.reason_code, result.reason_message),
        "audio_ms": result.audio_ms,
        "processed_ms": result.processed_ms,
        "speech_ms": result.speech_ms,
        "clips": result.clips,
        "lines": result.lines,
        "speakers": result.speakers,
        "speakers_reason": _reason(result.speakers_reason_code, result.speakers_reason_message),
    }


def build_transcript(job: Job, outcome: Outcome, engine: dict[str, Any], timing: dict[str, Any]) -> dict[str, Any]:
    return {
        "format_version": FORMAT_VERSION,
        "recording_id": job.recording_id,
        "language": job.language or "auto",
        "participants": [{"id": participant.id, "name": participant.name} for participant in job.participants],
        "lines": [_line(job, line) for line in outcome.lines],
        "segments": [_segment(result) for result in outcome.segments],
        "glossary": {
            "token_limit": outcome.glossary.token_limit,
            "token_count": outcome.glossary.token_count,
            "used": list(outcome.glossary.used),
            "cut": list(outcome.glossary.cut),
        },
        "speaker_separation": {
            "requested": job.separate_speakers,
            "speakers": [
                {"label": speaker.label, "participant_id": speaker.participant_id} for speaker in outcome.speakers
            ],
        },
        "engine": engine,
        "timing": timing,
    }
