"""Loads the verified models and runs one job with them."""

from __future__ import annotations

import logging
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from junto_transcribe import __version__
from junto_transcribe.contract import Job
from junto_transcribe.engines import AudioFiles
from junto_transcribe.limits import Limits
from junto_transcribe.models import Model, ModelError, Role, problems, require, require_read_only, select
from junto_transcribe.pipeline import CLIP_GAP_MS, Engines, Settings, audio_ms_total, run_job, speech_ms_total
from junto_transcribe.runtime import Device, choose_device, library_versions, peak_memory_mb
from junto_transcribe.speakers import SpeakerRules
from junto_transcribe.transcript import build_transcript

log = logging.getLogger("junto_transcribe")


@dataclass(frozen=True)
class Options:
    models_dir: Path
    whisper: str
    device: str = "auto"
    compute_type: str | None = None
    threads: int | None = None
    batch_size: int | None = None
    limits: Limits = field(default_factory=Limits)


@dataclass(frozen=True)
class Loaded:
    engines: Engines
    settings: Settings
    device: Device
    models: dict[Role, Model]
    batch_size: int
    stages_ms: dict[str, int]


def load(options: Options, speakers: bool) -> Loaded:
    """Verifies the model files, loads the models, and verifies the files again.

    Nothing is loaded when the worker can write to the model files, or when a file is missing,
    unlisted or different from the manifest. The second check finds a file that changed while
    the models were loaded.
    """
    started = time.perf_counter()
    models = select(options.whisper)
    require_read_only(models, options.models_dir)
    require(models, options.models_dir)
    verified = time.perf_counter()

    device = choose_device(options.device, options.compute_type, options.threads)
    batch_size = options.batch_size or int(models["whisper"].settings["batch_size"])

    from junto_transcribe.audio import AvAudioReader
    from junto_transcribe.detector import SileroDetector
    from junto_transcribe.embedder import SherpaEmbedder
    from junto_transcribe.whisper import WhisperRecognizer

    engines = Engines(
        reader=AvAudioReader(options.limits.gap_ms),
        detector=SileroDetector(models["voice-detector"].weights_path(options.models_dir)),
        recognizer=WhisperRecognizer(
            models["whisper"].root(options.models_dir), device.name, device.compute_type, device.threads, batch_size
        ),
        embedder=SherpaEmbedder(models["speaker-embedding"].weights_path(options.models_dir), device.threads)
        if speakers
        else None,
    )
    loaded = time.perf_counter()
    changed = problems(models, options.models_dir)
    if changed:
        raise ModelError("model_files_changed_while_loading: " + "; ".join(changed))
    settings = Settings(
        speaker_rules=SpeakerRules(threshold=float(models["speaker-embedding"].settings["threshold"])),
        limits=options.limits,
    )
    log.info(
        "models verified and loaded: device=%s compute_type=%s threads=%d whisper=%s",
        device.name,
        device.compute_type,
        device.threads,
        options.whisper,
    )
    return Loaded(
        engines=engines,
        settings=settings,
        device=device,
        models=models,
        batch_size=batch_size,
        stages_ms={
            "verify_models": round((verified - started) * 1000),
            "load_models": round((loaded - verified) * 1000),
            "verify_models_again": round((time.perf_counter() - loaded) * 1000),
        },
    )


def engine_header(loaded: Loaded) -> dict[str, Any]:
    from junto_transcribe.whisper import BEAM_SIZE

    clip = loaded.settings.clip_rules
    speaker = loaded.settings.speaker_rules
    return {
        "worker": __version__,
        "device": loaded.device.name,
        "compute_type": loaded.device.compute_type,
        "threads": loaded.device.threads,
        "models": [
            {
                "role": model.role,
                "name": model.name,
                "version": model.version,
                "licence": model.licence,
                "sha256": model.weights_sha256,
            }
            for model in loaded.models.values()
        ],
        "libraries": library_versions(loaded.device.name),
        "settings": {
            "voice_threshold": clip.threshold,
            "voice_release_threshold": clip.release_threshold,
            "min_silence_ms": clip.min_silence_ms,
            "split_silence_ms": clip.split_silence_ms,
            "clip_pad_ms": clip.clip_pad_ms,
            "max_clip_ms": clip.max_clip_ms,
            "speaker_pad_ms": clip.speaker_pad_ms,
            "speaker_window_ms": speaker.window_ms,
            "speaker_hop_ms": speaker.hop_ms,
            "speaker_threshold": speaker.threshold,
            "speaker_min_cluster_windows": speaker.min_cluster_windows,
            "speaker_max_cluster_windows": speaker.max_cluster_windows,
            "glossary_token_limit": loaded.settings.glossary_token_limit,
            "beam_size": BEAM_SIZE,
            "temperature": 0.0,
            "batch_size": loaded.batch_size,
            "clips_per_call": loaded.settings.clips_per_call,
            "clip_gap_ms": CLIP_GAP_MS,
            "chunk_ms": loaded.settings.chunk_ms,
        },
    }


def transcribe(job: Job, files: AudioFiles, options: Options, loaded: Loaded | None = None) -> dict[str, Any]:
    """`files` is the audio root, pinned by the caller before this call (`contract.open_job`)."""
    started = time.perf_counter()
    if loaded is None:
        speakers = job.separate_speakers and any(segment.kind == "shared" for segment in job.segments)
        loaded = load(options, speakers)
    outcome = run_job(job, files, loaded.engines, loaded.settings)
    for number, result in enumerate(outcome.segments, start=1):
        log.info(
            "segment %d of %d: status=%s clips=%d lines=%d speakers=%s%s%s",
            number,
            len(outcome.segments),
            result.status,
            result.clips,
            result.lines,
            result.speakers,
            f" reason={result.reason_code}" if result.reason_code else "",
            f" speakers_reason={result.speakers_reason_code}" if result.speakers_reason_code else "",
        )
    timing = {
        "total_ms": round((time.perf_counter() - started) * 1000),
        "audio_ms": audio_ms_total(outcome),
        "speech_ms": speech_ms_total(outcome),
        "stages_ms": {**loaded.stages_ms, **outcome.stages_ms},
        "peak_memory_mb": peak_memory_mb(),
    }
    return build_transcript(job, outcome, engine_header(loaded), timing)
