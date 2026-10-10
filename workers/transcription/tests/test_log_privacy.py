"""Logs carry positions, counts and fixed codes. They never carry a value of the job or transcript text."""

from __future__ import annotations

import json
import logging
import wave
from pathlib import Path
from typing import Any

import numpy as np
import pytest

from junto_transcribe import cli, worker
from junto_transcribe.audio import AvAudioReader
from junto_transcribe.models import select
from junto_transcribe.pipeline import Engines, Settings
from junto_transcribe.runtime import Device
from tests.fakes import FakeDetector, FakeEmbedder, FakeRecognizer, signal

SECRET = "S3CRET"


def write_wav(path: Path, samples: np.ndarray) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with wave.open(str(path), "wb") as file:
        file.setnchannels(1)
        file.setsampwidth(2)
        file.setframerate(16000)
        file.writeframes((samples * 32767).astype("<i2").tobytes())


def secret_job(root: Path) -> Path:
    write_wav(root / f"audio-{SECRET}" / f"mic-{SECRET}.wav", signal(6, [(1.0, 2.0, 1), (4.0, 5.0, 2)]))
    write_wav(root / f"audio-{SECRET}" / f"share-{SECRET}.wav", signal(12, [(1.0, 4.0, 1), (5.0, 8.0, 2)]))
    document = {
        "format_version": 1,
        "recording_id": f"recording-{SECRET}",
        "language": "auto",
        "glossary": [f"Term {SECRET}", f"Name {SECRET}"],
        "separate_speakers": True,
        "segments": [
            {
                "id": f"segment-{SECRET}-{index}",
                "path": f"audio-{SECRET}/{name}",
                "kind": kind,
                "participant": {"id": f"member-{SECRET}", "name": f"Person {SECRET}"},
                "start_ms": 0,
            }
            for index, (name, kind) in enumerate(
                [
                    (f"mic-{SECRET}.wav", "microphone"),
                    (f"share-{SECRET}.wav", "shared"),
                    (f"absent-{SECRET}.wav", "shared"),
                ]
            )
        ],
    }
    job_file = root / "job.json"
    job_file.write_text(json.dumps(document), "utf-8")
    return job_file


@pytest.fixture
def stand_in_models(monkeypatch: pytest.MonkeyPatch) -> None:
    def load(options: worker.Options, speakers: bool) -> worker.Loaded:
        engines = Engines(AvAudioReader(), FakeDetector(), FakeRecognizer(), FakeEmbedder())
        return worker.Loaded(engines, Settings(), Device("cpu", "int8", 1), select("small"), 8, {})

    monkeypatch.setattr(worker, "load", load)


def run(job_file: Path, caplog: pytest.LogCaptureFixture, capsys: pytest.CaptureFixture[str]) -> tuple[int, str]:
    out = job_file.parent / "transcript.json"
    with caplog.at_level(logging.DEBUG):
        code = cli.main(["--log-level", "debug", "run", str(job_file), "--out", str(out), "--models-dir", "/models"])
    captured = capsys.readouterr()
    return code, caplog.text + captured.err + captured.out


def test_a_run_logs_no_value_of_the_job_and_no_transcript_text(
    tmp_path: Path, stand_in_models: None, caplog: pytest.LogCaptureFixture, capsys: pytest.CaptureFixture[str]
) -> None:
    job_file = secret_job(tmp_path / "job")
    code, logged = run(job_file, caplog, capsys)
    transcript = json.loads((job_file.parent / "transcript.json").read_text("utf-8"))

    assert code == cli.EXIT_OK
    assert [segment["status"] for segment in transcript["segments"]] == ["ok", "ok", "failed"]
    assert len(transcript["lines"]) == 4
    assert "segment 3 of 3: status=failed" in logged and "reason=file_missing" in logged
    assert SECRET not in logged
    assert "utterance" not in logged
    for segment in transcript["segments"]:
        for reason in (segment["reason"], segment["speakers_reason"]):
            assert reason is None or SECRET not in reason["message"]


def test_an_unexpected_error_is_logged_without_its_message(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    caplog: pytest.LogCaptureFixture,
    capsys: pytest.CaptureFixture[str],
) -> None:
    job_file = secret_job(tmp_path / "job")

    def fail(*_: Any, **__: Any) -> None:
        raise RuntimeError(f"cannot handle Term {SECRET}")

    monkeypatch.setattr(worker, "transcribe", fail)
    code, logged = run(job_file, caplog, capsys)
    assert code == cli.EXIT_ERROR
    assert "RuntimeError" in logged
    assert SECRET not in logged
    assert not (job_file.parent / "transcript.json").exists()


def test_a_processing_failure_gives_a_reason_without_the_text_of_the_exception(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    caplog: pytest.LogCaptureFixture,
    capsys: pytest.CaptureFixture[str],
) -> None:
    job_file = secret_job(tmp_path / "job")

    def load(options: worker.Options, speakers: bool) -> worker.Loaded:
        recognizer = FakeRecognizer()

        def fail(*_: Any) -> None:
            raise ValueError(f"bad value Person {SECRET}")

        recognizer.recognize = fail  # type: ignore[method-assign, assignment]
        engines = Engines(AvAudioReader(), FakeDetector(), recognizer, FakeEmbedder())
        return worker.Loaded(engines, Settings(), Device("cpu", "int8", 1), select("small"), 8, {})

    monkeypatch.setattr(worker, "load", load)
    code, logged = run(job_file, caplog, capsys)
    transcript = json.loads((job_file.parent / "transcript.json").read_text("utf-8"))
    assert code == cli.EXIT_OK
    assert transcript["segments"][0]["reason"] == {
        "code": "processing_failed",
        "message": "a processing step failed (ValueError)",
    }
    assert SECRET not in logged


def refused_start(arguments: list[str], caplog: pytest.LogCaptureFixture, capsys: pytest.CaptureFixture[str]) -> str:
    with caplog.at_level(logging.DEBUG):
        code = cli.main(["--log-level", "debug", *arguments])
    captured = capsys.readouterr()
    assert code in (cli.EXIT_REFUSED, cli.EXIT_ENGINE, cli.EXIT_JOB)
    return caplog.text + captured.err + captured.out


def test_a_refused_start_names_a_code_and_no_path(
    tmp_path: Path, caplog: pytest.LogCaptureFixture, capsys: pytest.CaptureFixture[str]
) -> None:
    job_file = secret_job(tmp_path / f"job-{SECRET}")
    run_arguments = ["run", str(job_file), "--out"]

    logged = refused_start([*run_arguments, str(tmp_path / f"absent-{SECRET}" / "t.json")], caplog, capsys)
    assert "output_directory_not_usable" in logged and SECRET not in logged
    caplog.clear()

    out = str(job_file.parent / "t.json")
    models = ["--models-dir", str(tmp_path / f"models-{SECRET}"), "--whisper-model", "large-v3-turbo"]
    logged = refused_start([*run_arguments, out, *models], caplog, capsys)
    assert "model_file_missing: whisper large-v3-turbo model.bin" in logged and SECRET not in logged
    caplog.clear()

    logged = refused_start([*run_arguments, out, "--audio-root", str(tmp_path / f"no-{SECRET}")], caplog, capsys)
    assert "audio_root_not_usable" in logged and SECRET not in logged
    caplog.clear()

    logged = refused_start(["run", str(tmp_path / f"no-{SECRET}" / "job.json"), "--out", out], caplog, capsys)
    assert "job_directory_not_usable" in logged and SECRET not in logged


def test_model_file_problems_are_codes_with_names_from_the_manifest(
    tmp_path: Path, caplog: pytest.LogCaptureFixture, capsys: pytest.CaptureFixture[str]
) -> None:
    from junto_transcribe.models import problems

    models = tmp_path / f"models-{SECRET}"
    chosen = select("small")
    for model in chosen.values():
        if model.package is None:
            (models / model.directory).mkdir(parents=True, exist_ok=True)
            for file in model.files:
                (models / model.directory / file.name).write_bytes(b"0" * min(file.size, 64))
    (models / "speaker" / f"extra-{SECRET}.json").write_text("{}")
    (models / "faster-whisper-small" / "config.json").unlink()
    (models / "faster-whisper-small" / "config.json").symlink_to(models / "speaker")
    found = problems(chosen, models)
    assert "model_directory_has_unlisted_entry: speaker-embedding nemo_en_titanet_large" in found
    assert "model_file_not_regular: whisper small config.json" in found
    assert "model_file_size_differs: whisper small model.bin" in found
    assert SECRET not in " ".join(found)

    job_file = secret_job(tmp_path / "job")
    arguments = ["run", str(job_file), "--out", str(job_file.parent / "t.json"), "--models-dir", str(models)]
    logged = refused_start([*arguments, "--whisper-model", "small"], caplog, capsys)
    assert "model_files_writable" in logged and SECRET not in logged


def test_a_model_that_cannot_be_loaded_is_reported_without_the_text_of_the_library(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from junto_transcribe import whisper
    from junto_transcribe.engines import EngineFatalError

    def fail(*_: Any, **__: Any) -> None:
        raise RuntimeError(f"Unable to open file /models-{SECRET}/model.bin")

    monkeypatch.setattr(whisper, "WhisperModel", fail)
    with pytest.raises(EngineFatalError) as stopped:
        whisper.WhisperRecognizer(Path(f"/models-{SECRET}"), "cpu", "int8", 1, 8)
    assert SECRET not in str(stopped.value)
    assert "whisper_load_failed" in str(stopped.value) and "RuntimeError" in str(stopped.value)
