"""No state of one recognition call reaches the next call."""

from __future__ import annotations

from pathlib import Path
from types import SimpleNamespace
from typing import Any, ClassVar

import numpy as np
import pytest

from junto_transcribe import whisper
from junto_transcribe.clips import SAMPLE_RATE, Span
from junto_transcribe.engines import EngineFatalError


class Model:
    supported_languages = ("en", "pt")

    def __init__(self, *_: object, **__: object) -> None:
        self.hf_tokenizer = SimpleNamespace(
            encode=lambda text, add_special_tokens: SimpleNamespace(
                ids=[50259 if word == "ZZ" else index for index, word in enumerate(text.split())]
            ),
            get_added_tokens_decoder=lambda: {
                50257: SimpleNamespace(content="<|endoftext|>"),
                50259: SimpleNamespace(content="<|en|>"),
            },
        )


class Pipeline:
    """Keeps the two attributes that the library's pipeline keeps between calls."""

    instances: ClassVar[list[Pipeline]] = []
    failures: ClassVar[list[Exception]] = []
    late_failures: ClassVar[list[Exception]] = []

    def __init__(self, model: Any) -> None:
        self.last_speech_timestamp = 0.0
        self.detected: list[str] = []
        self.seen_at_start: tuple[float, list[str]] | None = None
        Pipeline.instances.append(self)

    def transcribe(self, samples: Any, **options: Any) -> tuple[Any, None]:
        self.seen_at_start = (self.last_speech_timestamp, list(self.detected))
        self.options = options
        clips = options["clip_timestamps"]
        self.last_speech_timestamp = clips[-1]["end"]
        self.detected += ["en"] * len(clips)
        if Pipeline.failures:
            raise Pipeline.failures.pop(0)
        segments = [SimpleNamespace(text=" words", start=clip["start"], end=clip["end"], words=None) for clip in clips]
        return self._results(segments), None

    def _results(self, segments: list[Any]) -> Any:
        for number, segment in enumerate(segments):
            if number == 1 and Pipeline.late_failures:
                raise Pipeline.late_failures.pop(0)
            yield segment


@pytest.fixture
def recognizer(monkeypatch: pytest.MonkeyPatch) -> whisper.WhisperRecognizer:
    Pipeline.instances = []
    Pipeline.failures = []
    Pipeline.late_failures = []
    monkeypatch.setattr(whisper, "WhisperModel", Model)
    monkeypatch.setattr(whisper, "_ClipPipeline", Pipeline)
    return whisper.WhisperRecognizer(Path("/models/invented"), "cpu", "int8", 1, 8)


SAMPLES = np.zeros(40 * SAMPLE_RATE, dtype=np.float32)
CLIPS = [Span(SAMPLE_RATE, 5 * SAMPLE_RATE), Span(20 * SAMPLE_RATE, 30 * SAMPLE_RATE)]


def test_a_failed_call_leaves_no_state_for_the_next_call(recognizer: whisper.WhisperRecognizer) -> None:
    Pipeline.failures = [RuntimeError("the model failed on this clip")]
    with pytest.raises(RuntimeError):
        recognizer.recognize(SAMPLES, CLIPS, None, None)
    found = recognizer.recognize(SAMPLES, CLIPS[:1], None, None)
    assert Pipeline.instances[-1].seen_at_start == (0.0, [])
    assert [result.language for result in found if result is not None] == ["en"]


def test_a_successful_call_leaves_no_state_either(recognizer: whisper.WhisperRecognizer) -> None:
    recognizer.recognize(SAMPLES, CLIPS, None, None)
    recognizer.recognize(SAMPLES, CLIPS, "pt", None)
    assert [instance.seen_at_start for instance in Pipeline.instances] == [(0.0, []), (0.0, [])]


def test_an_error_of_the_device_names_the_device_problem_and_not_the_text_of_the_error(
    recognizer: whisper.WhisperRecognizer,
) -> None:
    Pipeline.failures = [RuntimeError("CUDA failed with error out of memory near 'Term S3CRET'")]
    with pytest.raises(EngineFatalError) as stopped:
        recognizer.recognize(SAMPLES, CLIPS, None, None)
    assert "S3CRET" not in str(stopped.value)
    assert "out of memory" in str(stopped.value)


def test_the_glossary_goes_to_whisper_as_hotwords(recognizer: whisper.WhisperRecognizer) -> None:
    recognizer.recognize(SAMPLES, CLIPS, None, "Oswin Thackerby, Quorvex.")
    options = Pipeline.instances[-1].options
    assert options["hotwords"] == "Oswin Thackerby, Quorvex."
    assert options.get("initial_prompt") is None
    assert options.get("prefix") is None
    recognizer.recognize(SAMPLES, CLIPS, None, None)
    assert Pipeline.instances[-1].options["hotwords"] is None


def test_a_call_that_fails_while_its_results_are_read_leaves_no_state(recognizer: whisper.WhisperRecognizer) -> None:
    Pipeline.late_failures = [RuntimeError("the model failed after the first clip")]
    with pytest.raises(RuntimeError):
        recognizer.recognize(SAMPLES, CLIPS, None, None)
    assert Pipeline.instances[-1].last_speech_timestamp == 30.0
    recognizer.recognize(SAMPLES, CLIPS[:1], None, None)
    assert Pipeline.instances[-1].seen_at_start == (0.0, [])
    assert len(Pipeline.instances) == 2


def test_a_control_token_is_found_by_its_written_form_and_by_its_token_number(
    recognizer: whisper.WhisperRecognizer,
) -> None:
    assert recognizer.control_token_in("<|en|>")
    assert recognizer.control_token_in("Quorvex <|endoftext|> Lumitrax")
    assert recognizer.control_token_in("Velkira ZZ")
    assert not recognizer.control_token_in("Quorvex")
    assert not recognizer.control_token_in("A<B | C>D")
