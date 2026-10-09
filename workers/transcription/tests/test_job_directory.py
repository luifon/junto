"""The job directory is pinned when the job file is read, before a model is loaded."""

from __future__ import annotations

import json
import wave
from collections.abc import Callable
from pathlib import Path

import numpy as np
import pytest

from junto_transcribe import cli, worker
from junto_transcribe.audio import AvAudioReader
from junto_transcribe.models import select
from junto_transcribe.pipeline import Engines, Settings
from junto_transcribe.runtime import Device
from tests.conftest import job_document
from tests.fakes import FakeDetector, FakeEmbedder, FakeRecognizer, signal


def write_wav(path: Path, code: int) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with wave.open(str(path), "wb") as file:
        file.setnchannels(1)
        file.setsampwidth(2)
        file.setframerate(16000)
        file.writeframes((signal(4, [(1.0, 2.0, code)]) * 32767).astype("<i2").tobytes())


def make_job(directory: Path, code: int) -> Path:
    write_wav(directory / "audio" / "a.wav", code)
    (directory / "job.json").write_text(json.dumps(job_document([("a", "audio/a.wav", "microphone", "p", 0)])))
    return directory / "job.json"


def replace_with_link(directory: Path, target: Path) -> None:
    directory.rename(directory.with_name(directory.name + "-original"))
    directory.symlink_to(target)


def models_that_load_after(action: Callable[[], None], monkeypatch: pytest.MonkeyPatch) -> None:
    """Stand-in models. `action` runs where the real models would be loaded."""

    def load(options: worker.Options, speakers: bool) -> worker.Loaded:
        action()
        engines = Engines(AvAudioReader(), FakeDetector(), FakeRecognizer(), FakeEmbedder())
        return worker.Loaded(engines, Settings(), Device("cpu", "int8", 1), select("small"), 8, {})

    monkeypatch.setattr(worker, "load", load)


def lines(path: Path) -> list[str]:
    return [line["text"] for line in json.loads(path.read_text("utf-8"))["lines"]]


def test_a_job_directory_replaced_by_a_link_while_the_models_load_is_not_followed(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    job_file = make_job(tmp_path / "meeting", 1)
    make_job(tmp_path / "other-meeting", 7)
    models_that_load_after(lambda: replace_with_link(tmp_path / "meeting", tmp_path / "other-meeting"), monkeypatch)

    code = cli.main(
        ["run", str(job_file), "--out", str(tmp_path / "meeting" / "transcript.json"), "--models-dir", "/models"]
    )

    assert code == cli.EXIT_OK
    assert lines(tmp_path / "meeting-original" / "transcript.json") == ["utterance 1"]
    assert not (tmp_path / "other-meeting" / "transcript.json").exists()


def test_an_audio_root_and_an_output_directory_elsewhere_are_pinned_at_the_same_moment(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    job_file = make_job(tmp_path / "meeting", 1)
    write_wav(tmp_path / "recordings" / "audio" / "a.wav", 2)
    write_wav(tmp_path / "private" / "audio" / "a.wav", 7)
    (tmp_path / "out").mkdir()
    (tmp_path / "elsewhere").mkdir()

    def swap() -> None:
        replace_with_link(tmp_path / "recordings", tmp_path / "private")
        replace_with_link(tmp_path / "out", tmp_path / "elsewhere")

    models_that_load_after(swap, monkeypatch)
    arguments = [
        "run",
        str(job_file),
        "--out",
        str(tmp_path / "out" / "t.json"),
        "--audio-root",
        str(tmp_path / "recordings"),
    ]
    assert cli.main([*arguments, "--models-dir", "/models"]) == cli.EXIT_OK
    assert lines(tmp_path / "out-original" / "t.json") == ["utterance 2"]
    assert list((tmp_path / "elsewhere").iterdir()) == []


def test_a_job_directory_that_is_a_link_is_refused(tmp_path: Path, caplog: pytest.LogCaptureFixture) -> None:
    make_job(tmp_path / "real", 1)
    (tmp_path / "link").symlink_to(tmp_path / "real")
    arguments = ["run", str(tmp_path / "link" / "job.json"), "--out", str(tmp_path / "real" / "t.json")]
    assert cli.main([*arguments, "--models-dir", "/models"]) == cli.EXIT_JOB
    assert "job_directory_not_usable" in caplog.text
    assert not (tmp_path / "real" / "t.json").exists()


def test_a_job_file_that_is_a_link_is_refused(tmp_path: Path, caplog: pytest.LogCaptureFixture) -> None:
    make_job(tmp_path / "real", 1)
    (tmp_path / "meeting").mkdir()
    (tmp_path / "meeting" / "job.json").symlink_to(tmp_path / "real" / "job.json")
    arguments = ["run", str(tmp_path / "meeting" / "job.json"), "--out", str(tmp_path / "meeting" / "t.json")]
    assert cli.main([*arguments, "--models-dir", "/models"]) == cli.EXIT_JOB
    assert "job_file_not_readable" in caplog.text


def test_an_output_directory_or_audio_root_that_is_a_link_or_absent_is_refused(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, caplog: pytest.LogCaptureFixture
) -> None:
    job_file = make_job(tmp_path / "meeting", 1)
    models_that_load_after(lambda: pytest.fail("a model was loaded"), monkeypatch)
    (tmp_path / "real-out").mkdir()
    (tmp_path / "out").symlink_to(tmp_path / "real-out")
    base = ["run", str(job_file), "--models-dir", "/models"]
    assert cli.main([*base, "--out", str(tmp_path / "out" / "t.json")]) == cli.EXIT_REFUSED
    assert cli.main([*base, "--out", str(tmp_path / "absent" / "t.json")]) == cli.EXIT_REFUSED
    assert "output_directory_not_usable" in caplog.text
    assert (
        cli.main([*base, "--out", str(tmp_path / "meeting" / "t.json"), "--audio-root", str(tmp_path / "out")])
        == cli.EXIT_JOB
    )
    assert "audio_root_not_usable" in caplog.text
    assert list((tmp_path / "real-out").iterdir()) == []


def test_the_transcript_replaces_an_old_one_whole_and_is_never_seen_in_part(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    job_file = make_job(tmp_path / "meeting", 1)
    out = tmp_path / "meeting" / "transcript.json"
    out.write_text("old")
    models_that_load_after(lambda: None, monkeypatch)
    assert cli.main(["run", str(job_file), "--out", str(out), "--models-dir", "/models"]) == cli.EXIT_OK
    assert lines(out) == ["utterance 1"]
    assert sorted(path.name for path in (tmp_path / "meeting").iterdir()) == ["audio", "job.json", "transcript.json"]


def sample_sum(path: Path) -> float:
    with wave.open(str(path), "rb") as file:
        return float(np.frombuffer(file.readframes(file.getnframes()), dtype="<i2").sum())


def test_a_transcript_that_cannot_be_written_whole_leaves_the_old_one(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import os

    job_file = make_job(tmp_path / "meeting", 1)
    out = tmp_path / "meeting" / "transcript.json"
    out.write_text("old")
    models_that_load_after(lambda: None, monkeypatch)

    def disk_full(descriptor: int) -> None:
        raise OSError(28, "No space left on device")

    monkeypatch.setattr(os, "fsync", disk_full)
    assert cli.main(["run", str(job_file), "--out", str(out), "--models-dir", "/models"]) == cli.EXIT_ERROR
    assert out.read_text() == "old"
    assert sorted(path.name for path in (tmp_path / "meeting").iterdir()) == ["audio", "job.json", "transcript.json"]
