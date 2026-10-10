"""A check of the installed models that needs no speech recording."""

from __future__ import annotations

import json
import tempfile
import wave
from collections.abc import Callable
from pathlib import Path

import numpy as np

from junto_transcribe.clips import SAMPLE_RATE, Span
from junto_transcribe.contract import open_job, schema_errors
from junto_transcribe.glossary import plan_glossary
from junto_transcribe.worker import Options, load, transcribe

_NOISE_SECONDS = 6
_GLOSSARY = ["Junto", "Quorvex", "Lumitrax"]


def _noise(seconds: float) -> np.ndarray:
    return (np.random.default_rng(7).standard_normal(int(seconds * SAMPLE_RATE)) * 0.001).astype(np.float32)


def _write_wav(path: Path, samples: np.ndarray) -> None:
    with wave.open(str(path), "wb") as file:
        file.setnchannels(1)
        file.setsampwidth(2)
        file.setframerate(SAMPLE_RATE)
        file.writeframes((samples * 32767).astype("<i2").tobytes())


def run(options: Options, report: Callable[[str], None] = print) -> bool:
    """Runs every model once and one whole job on generated noise. Returns whether all checks passed."""
    failures = 0

    def check(name: str, passed: bool, detail: str = "") -> None:
        nonlocal failures
        failures += 0 if passed else 1
        report(f"{'ok  ' if passed else 'FAIL'}  {name}{f': {detail}' if detail else ''}")

    loaded = load(options, speakers=True)
    check("model files match the manifest", True, ", ".join(model.name for model in loaded.models.values()))
    check("models loaded", True, f"{loaded.device.name}, {loaded.device.compute_type}, {loaded.device.threads} threads")

    recognizer = loaded.engines.recognizer
    plan = plan_glossary(_GLOSSARY, recognizer.count_tokens)
    check(
        "glossary tokenised", plan.used == tuple(_GLOSSARY) and 0 < plan.token_count < 40, f"{plan.token_count} tokens"
    )

    noise = _noise(3)
    for language in (None, "en"):
        found = recognizer.recognize(noise, [Span(0, len(noise))], language, plan.text)
        check(f"Whisper ran on one clip (language {language or 'auto'})", len(found) == 1)

    assert loaded.engines.embedder is not None
    vector = loaded.engines.embedder.embed(noise[: int(2.5 * SAMPLE_RATE)])
    check("speaker embedding computed", vector is not None and bool(np.isfinite(vector).all()) and vector.size > 0)

    with tempfile.TemporaryDirectory(prefix="junto-self-test-") as name:
        directory = Path(name)
        _write_wav(directory / "noise.wav", _noise(_NOISE_SECONDS))
        participant = {"id": "self-test", "name": "Self-test"}
        job_file = directory / "job.json"
        job_file.write_text(
            json.dumps(
                {
                    "format_version": 1,
                    "recording_id": "self-test",
                    "language": "auto",
                    "glossary": _GLOSSARY,
                    "separate_speakers": True,
                    "segments": [
                        {
                            "id": "microphone",
                            "path": "noise.wav",
                            "kind": "microphone",
                            "participant": participant,
                            "start_ms": 0,
                        },
                        {
                            "id": "shared",
                            "path": "noise.wav",
                            "kind": "shared",
                            "participant": participant,
                            "start_ms": 500,
                        },
                        {
                            "id": "absent",
                            "path": "absent.wav",
                            "kind": "microphone",
                            "participant": participant,
                            "start_ms": 0,
                        },
                    ],
                }
            )
        )
        with open_job(job_file) as pinned:
            document = transcribe(pinned.job, pinned.audio, options, loaded)

    statuses = {segment["id"]: segment["status"] for segment in document["segments"]}
    check("whole job produced a valid transcript", not schema_errors("transcript", document))
    check("noise without speech gave no lines", document["lines"] == [] and statuses["microphone"] == "empty")
    check("shared segment processed", statuses["shared"] == "empty")
    check("missing file reported and the job continued", statuses["absent"] == "failed")
    report(f"self-test {'passed' if failures == 0 else 'FAILED'}")
    return failures == 0
