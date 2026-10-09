"""Command line of the transcription worker."""

from __future__ import annotations

import argparse
import logging
import os
import sys
import traceback
from collections.abc import Sequence
from pathlib import Path
from typing import TYPE_CHECKING

from junto_transcribe import __version__
from junto_transcribe.contract import JobError, open_job, write_transcript
from junto_transcribe.engines import EngineFatalError
from junto_transcribe.files import PinnedDirectory
from junto_transcribe.limits import Limits
from junto_transcribe.models import DEFAULT_WHISPER, ModelError, manifest, problems, select, whisper_names
from junto_transcribe.runtime import DEVICES, StartError, require_pinned_libraries

if TYPE_CHECKING:
    from junto_transcribe.worker import Options

EXIT_OK = 0
EXIT_ERROR = 1
EXIT_JOB = 2
EXIT_REFUSED = 3
EXIT_ENGINE = 4

log = logging.getLogger("junto_transcribe")


def _positive(text: str) -> int:
    value = int(text)
    if value < 1:
        raise argparse.ArgumentTypeError("must be at least 1")
    return value


def _engine_arguments(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "--models-dir",
        type=Path,
        default=Path(os.environ.get("JUNTO_TRANSCRIBE_MODELS", "/models")),
        help="directory with the model files (default: $JUNTO_TRANSCRIBE_MODELS or /models)",
    )
    parser.add_argument(
        "--whisper-model",
        default=os.environ.get("JUNTO_TRANSCRIBE_WHISPER_MODEL", DEFAULT_WHISPER),
        choices=whisper_names(),
        help=f"Whisper model of the manifest (default: $JUNTO_TRANSCRIBE_WHISPER_MODEL or {DEFAULT_WHISPER})",
    )
    parser.add_argument(
        "--device",
        default=os.environ.get("JUNTO_TRANSCRIBE_DEVICE", "auto"),
        choices=DEVICES,
        help="auto uses a CUDA device when one is available (default: $JUNTO_TRANSCRIBE_DEVICE or auto)",
    )
    parser.add_argument("--compute-type", help="CTranslate2 compute type (default: float16 on cuda, int8 on cpu)")
    parser.add_argument(
        "--threads", type=_positive, help="CPU threads (default: the CPUs the process may use, at most 8)"
    )
    parser.add_argument(
        "--batch-size", type=_positive, help="clips per Whisper batch (default: the value in the manifest)"
    )


# Option, field of Limits, factor from the option's unit to the field's unit, unit in the help text.
_LIMIT_OPTIONS = (
    ("--max-job-bytes", "job_file_bytes", 1, "size of the job file in bytes"),
    ("--max-segments", "segments", 1, "number of segments in a job"),
    ("--max-segment-seconds", "segment_audio_ms", 1000, "decoded audio of one segment in seconds"),
    ("--max-job-seconds", "job_audio_ms", 1000, "decoded audio of the whole job in seconds"),
    (
        "--max-processing-seconds",
        "processing_ms",
        1000,
        "processing time of a job in seconds, after the models are loaded",
    ),
    ("--max-gap-seconds", "gap_ms", 1000, "one gap between packet timestamps that is filled with silence, in seconds"),
    ("--max-speaker-windows", "speaker_windows", 1, "voice windows of a job that get a speaker embedding"),
)


def _limit_arguments(parser: argparse.ArgumentParser) -> None:
    defaults = Limits()
    for option, name, factor, text in _LIMIT_OPTIONS:
        variable = "JUNTO_TRANSCRIBE_" + option[2:].upper().replace("-", "_")
        parser.add_argument(
            option,
            type=_positive,
            default=os.environ.get(variable, str(getattr(defaults, name) // factor)),
            help=f"limit: {text} (default: ${variable} or {getattr(defaults, name) // factor})",
        )


def _limits(arguments: argparse.Namespace) -> Limits:
    return Limits(
        **{
            name: getattr(arguments, option[2:].replace("-", "_")) * factor
            for option, name, factor, _ in _LIMIT_OPTIONS
        }
    )


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="junto-transcribe", description="Transcription worker of Junto.")
    parser.add_argument("--version", action="version", version=__version__)
    parser.add_argument("--log-level", default="info", choices=("debug", "info", "warning", "error"))
    commands = parser.add_subparsers(dest="command", required=True)

    run = commands.add_parser("run", help="transcribe the audio files of one job")
    run.add_argument("job", type=Path, help="job file (JSON)")
    run.add_argument("--out", type=Path, required=True, help="transcript file to write (JSON)")
    run.add_argument(
        "--audio-root", type=Path, help="directory that segment paths are relative to (default: the job's)"
    )
    _engine_arguments(run)
    _limit_arguments(run)

    models = commands.add_parser("models", help="install and check the model files")
    actions = models.add_subparsers(dest="action", required=True)
    for name, text in (
        ("fetch", "download the model files and verify their checksums (uses the network)"),
        ("verify", "check that the model files are present and have the right checksums"),
    ):
        action = actions.add_parser(name, help=text)
        action.add_argument("--dir", type=Path, required=True, help="directory with the model files")
        action.add_argument(
            "--whisper",
            action="append",
            choices=whisper_names(),
            help=f"Whisper model; can be given more than once (default: {DEFAULT_WHISPER})",
        )
    actions.add_parser("list", help="print the manifest: every model with its licence, source and checksums")

    self_test = commands.add_parser("self-test", help="run every installed model once on generated noise")
    _engine_arguments(self_test)
    return parser


def _options(arguments: argparse.Namespace) -> Options:
    from junto_transcribe.worker import Options

    return Options(
        models_dir=arguments.models_dir,
        whisper=arguments.whisper_model,
        device=arguments.device,
        compute_type=arguments.compute_type,
        threads=arguments.threads,
        batch_size=arguments.batch_size,
        limits=_limits(arguments) if hasattr(arguments, "max_segments") else Limits(),
    )


def _run(arguments: argparse.Namespace) -> int:
    limits = _limits(arguments)
    # The job directory, the audio root and the output directory are pinned here, before a model is loaded.
    with open_job(arguments.job, arguments.audio_root, limits.job_file_bytes, limits.segments) as pinned:
        try:
            output = PinnedDirectory(arguments.out.parent)
        except OSError as error:
            raise StartError(f"output_directory_not_usable ({type(error).__name__})") from error
        with output:
            target = pinned.directory if output.same_directory(pinned.directory) else output
            from junto_transcribe import worker

            document = worker.transcribe(pinned.job, pinned.audio, _options(arguments))
            write_transcript(document, target, arguments.out.name)
    failed = sum(segment["status"] == "failed" for segment in document["segments"])
    damaged = sum(segment["status"] == "damaged" for segment in document["segments"])
    log.info(
        "transcript written: lines=%d segments=%d damaged=%d failed=%d",
        len(document["lines"]),
        len(document["segments"]),
        damaged,
        failed,
    )
    return EXIT_OK


def _models(arguments: argparse.Namespace) -> int:
    if arguments.action == "list":
        for model in manifest():
            print(f"{model.role} {model.name} ({model.purpose}; {model.licence}; {model.version})")
            print(f"  source: {model.source}")
            print(f"  attribution: {model.attribution}")
            for file in model.files:
                print(f"  {file.sha256}  {file.size:>10}  {file.name}")
        return EXIT_OK

    found: list[str] = []
    for whisper in arguments.whisper or [DEFAULT_WHISPER]:
        chosen = select(whisper)
        if arguments.action == "fetch":
            from junto_transcribe.fetch import fetch

            fetch(chosen, arguments.dir)
        found += [problem for problem in problems(chosen, arguments.dir) if problem not in found]
    for problem in found:
        print(problem, file=sys.stderr)
    if found:
        return EXIT_REFUSED
    print("all model files match the manifest")
    return EXIT_OK


def _self_test(arguments: argparse.Namespace) -> int:
    from junto_transcribe import selftest

    return EXIT_OK if selftest.run(_options(arguments)) else EXIT_ERROR


def main(argv: Sequence[str] | None = None) -> int:
    arguments = _parser().parse_args(argv)
    logging.basicConfig(
        stream=sys.stderr,
        level=arguments.log_level.upper(),
        format="%(asctime)s %(levelname)s %(message)s",
    )
    if arguments.log_level != "debug":
        logging.getLogger("faster_whisper").setLevel(logging.WARNING)
    try:
        require_pinned_libraries()
        if arguments.command == "run":
            return _run(arguments)
        if arguments.command == "models":
            return _models(arguments)
        return _self_test(arguments)
    except JobError as error:
        log.error("job refused: %s", error)
        return EXIT_JOB
    except (StartError, ModelError) as error:
        log.error("not started: %s", error)
        return EXIT_REFUSED
    except EngineFatalError as error:
        log.error("stopped, no transcript written: %s", error)
        return EXIT_ENGINE
    except Exception as error:
        # The text of an unexpected exception can contain a value of the job, so only its type and place are logged.
        places = "".join(traceback.format_tb(error.__traceback__))
        log.error("unexpected error (%s), no transcript written\n%s", type(error).__name__, places)
        return EXIT_ERROR
