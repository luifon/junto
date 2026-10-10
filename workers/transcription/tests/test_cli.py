from __future__ import annotations

import json
from pathlib import Path

import pytest

from junto_transcribe import cli
from tests.conftest import job_document

SEGMENTS = [("m", "m.wav", "microphone", "p1", 0)]


def write_job(directory: Path, **changes: object) -> Path:
    job_file = directory / "job.json"
    job_file.write_text(json.dumps({**job_document(SEGMENTS), **changes}))
    return job_file


def test_an_invalid_job_exits_with_2_and_writes_nothing(tmp_path: Path) -> None:
    job_file = write_job(tmp_path, format_version=7)
    out = tmp_path / "transcript.json"
    assert cli.main(["run", str(job_file), "--out", str(out), "--models-dir", str(tmp_path)]) == cli.EXIT_JOB
    assert not out.exists()


def test_run_refuses_to_start_without_the_model_files(tmp_path: Path, capsys: pytest.CaptureFixture[str]) -> None:
    job_file = write_job(tmp_path)
    out = tmp_path / "transcript.json"
    code = cli.main(["run", str(job_file), "--out", str(out), "--models-dir", str(tmp_path / "models")])
    assert code == cli.EXIT_REFUSED
    assert not out.exists()


def test_run_refuses_a_changed_model_file(tmp_path: Path, caplog: pytest.LogCaptureFixture) -> None:
    from junto_transcribe.models import select

    models = tmp_path / "models"
    for model in select("small").values():
        if model.package is None:
            (models / model.directory).mkdir(parents=True, exist_ok=True)
            for file in model.files:
                (models / model.directory / file.name).write_bytes(b"0" * min(file.size, 4096))
    job_file = write_job(tmp_path)
    arguments = ["run", str(job_file), "--out", str(tmp_path / "t.json"), "--models-dir", str(models)]
    assert cli.main([*arguments, "--whisper-model", "small"]) == cli.EXIT_REFUSED
    assert "model_files_writable" in caplog.text
    caplog.clear()

    for path in sorted(models.rglob("*"), reverse=True):
        path.chmod(0o555 if path.is_dir() else 0o444)
    models.chmod(0o555)
    try:
        assert cli.main([*arguments, "--whisper-model", "small"]) == cli.EXIT_REFUSED
        assert "model_file_size_differs: whisper small model.bin" in caplog.text
        assert not (tmp_path / "t.json").exists()
    finally:
        for path in [models, *models.rglob("*")]:
            path.chmod(0o755)


def test_run_refuses_an_output_directory_that_does_not_exist(tmp_path: Path) -> None:
    job_file = write_job(tmp_path)
    out = tmp_path / "absent" / "transcript.json"
    assert cli.main(["run", str(job_file), "--out", str(out), "--models-dir", str(tmp_path)]) == cli.EXIT_REFUSED


def test_models_verify_reports_every_missing_file(tmp_path: Path, capsys: pytest.CaptureFixture[str]) -> None:
    assert cli.main(["models", "verify", "--dir", str(tmp_path)]) == cli.EXIT_REFUSED
    errors = capsys.readouterr().err
    assert errors.count("model_file_missing") == 6
    assert "whisper large-v3-turbo model.bin" in errors and "nemo_en_titanet_large.onnx" in errors
    assert str(tmp_path) not in errors


def test_models_list_prints_licences_and_checksums(capsys: pytest.CaptureFixture[str]) -> None:
    assert cli.main(["models", "list"]) == cli.EXIT_OK
    output = capsys.readouterr().out
    assert "CC-BY-4.0" in output and "NVIDIA" in output
    assert "e76620f83d5f5b69efd3d87e3dc180c1bd21df9fbebacfd4335e5e1efcc018da" in output


def test_a_wrong_command_line_exits_with_2(tmp_path: Path) -> None:
    with pytest.raises(SystemExit) as exit_info:
        cli.main(["run", str(tmp_path / "job.json")])
    assert exit_info.value.code == 2


@pytest.mark.parametrize("option", ["--threads", "--batch-size"])
def test_a_count_below_one_is_refused(tmp_path: Path, option: str) -> None:
    with pytest.raises(SystemExit) as exit_info:
        cli.main(["run", str(tmp_path / "job.json"), "--out", str(tmp_path / "t.json"), option, "0"])
    assert exit_info.value.code == 2


def test_a_device_name_from_the_environment_that_does_not_exist_refuses_the_start(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from junto_transcribe import worker
    from junto_transcribe.runtime import choose_device

    monkeypatch.setenv("JUNTO_TRANSCRIBE_DEVICE", "gpu")
    monkeypatch.setattr(worker, "require", lambda *_: None)
    monkeypatch.setattr(worker, "choose_device", choose_device)
    job_file = write_job(tmp_path)
    arguments = ["run", str(job_file), "--out", str(tmp_path / "t.json"), "--models-dir", str(tmp_path)]
    assert cli.main(arguments) == cli.EXIT_REFUSED


def test_a_limit_from_the_environment_that_is_not_a_number_is_refused(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("JUNTO_TRANSCRIBE_MAX_SEGMENTS", "many")
    with pytest.raises(SystemExit) as exit_info:
        cli.main(["run", str(write_job(tmp_path)), "--out", str(tmp_path / "t.json")])
    assert exit_info.value.code == 2


def test_the_limits_of_the_command_line_reach_the_job_file_reader(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    job_file = write_job(tmp_path)
    arguments = ["run", str(job_file), "--out", str(tmp_path / "t.json"), "--models-dir", str(tmp_path / "models")]
    assert cli.main([*arguments, "--max-job-bytes", "10"]) == cli.EXIT_JOB
    monkeypatch.setenv("JUNTO_TRANSCRIBE_MAX_JOB_BYTES", "10")
    assert cli.main(arguments) == cli.EXIT_JOB
    monkeypatch.delenv("JUNTO_TRANSCRIBE_MAX_JOB_BYTES")
    assert cli.main(arguments) == cli.EXIT_REFUSED


def test_every_limit_has_an_option_with_the_documented_default() -> None:
    from junto_transcribe.limits import Limits

    arguments = cli._parser().parse_args(["run", "job.json", "--out", "t.json"])
    assert cli._limits(arguments) == Limits()
    options = ["--max-segment-seconds=60", "--max-job-seconds=120", "--max-processing-seconds=30"]
    options += ["--max-gap-seconds=5", "--max-speaker-windows=9", "--max-segments=3"]
    changed = cli._parser().parse_args(["run", "job.json", "--out", "t.json", *options])
    limits = cli._limits(changed)
    assert (limits.segment_audio_ms, limits.job_audio_ms, limits.processing_ms) == (60_000, 120_000, 30_000)
    assert (limits.gap_ms, limits.speaker_windows, limits.segments) == (5000, 9, 3)
