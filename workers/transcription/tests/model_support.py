"""Support for the tests that run the real models on generated speech."""

from __future__ import annotations

import json
import os
import re
import unicodedata
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Any

import jiwer

from junto_transcribe.contract import parse_job, schema_errors
from junto_transcribe.files import PinnedDirectory
from junto_transcribe.models import problems, select
from junto_transcribe.worker import Loaded, Options, transcribe

NUMBER_WORDS = {
    "en": (
        "zero one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen "
        "sixteen seventeen eighteen nineteen twenty"
    ).split(),
    "pt": (
        "zero um dois três quatro cinco seis sete oito nove dez onze doze treze catorze quinze "
        "dezesseis dezessete dezoito dezenove vinte"
    ).split(),
}

# Bounds for generated speech. The technical test measured a word error rate of 0.3 to 1.5 % (one
# language) and 0.9 to 1.4 % (mixed) for large-v3-turbo. `small` is the development model of the
# fast tier.
BOUNDS: dict[str, dict[str, float]] = {
    "large-v3-turbo": {
        "wer": 0.04,
        "wer_quick": 0.06,
        "wer_mixed": 0.05,
        "wer_mixed_quick": 0.08,
        "glossary_long": 0.6,
        "glossary_long_quick": 0.5,
    },
    "small": {
        "wer": 0.10,
        "wer_mixed": 0.12,
        "glossary_long": 0.4,
    },
}

# Lowest number of glossary terms that must be spelled right with the glossary, per test file.
# For large-v3-turbo each number is the lowest result of repeated measurements minus one term.
# The measurements are in the README ("Glossary result over several generations of the speech"):
# ten generations of the full files gave 18 to 20 of 23 (English) and 16 to 20 of 23 (Portuguese);
# eight generations of the quick files gave 5 to 6 of 8 and 4 to 6 of 8.
# The numbers for `small` are half of the occurrences; they come from no repeated measurement.
GLOSSARY_MINIMUM: dict[str, dict[str, int]] = {
    "large-v3-turbo": {"gloss_en": 17, "gloss_pt": 15, "gloss_en_quick": 4, "gloss_pt_quick": 3},
    "small": {"gloss_en": 12, "gloss_pt": 12, "gloss_en_quick": 4, "gloss_pt_quick": 4},
}


def _checksums(path: Path) -> dict[str, str]:
    """Checksum by file name, from lines of the form `<checksum>  <name>`."""
    pairs = (line.split("  ", 1) for line in path.read_text().splitlines() if line)
    return {name: digest for digest, name in pairs}


@dataclass(frozen=True)
class Environment:
    models: Path | None
    audio: Path | None
    out: Path | None
    profile: str
    whisper: str
    device: str
    required: bool

    @property
    def quick(self) -> bool:
        return self.profile == "quick"

    def missing(self) -> str | None:
        if self.models is None or self.audio is None:
            return "JUNTO_TRANSCRIBE_MODELS and JUNTO_TRANSCRIBE_TEST_AUDIO are not both set"
        if not (self.audio / "en.json").is_file():
            return f"no generated speech in {self.audio}"
        found = problems(select(self.whisper), self.models)
        return f"model files are not as the manifest states: {found[0]}" if found else None

    def name(self, base: str) -> str:
        return f"{base}_quick" if self.quick else base

    def bound(self, key: str) -> float:
        return BOUNDS[self.whisper][f"{key}_quick" if self.quick and f"{key}_quick" in BOUNDS[self.whisper] else key]

    def glossary_minimum(self, name: str) -> int:
        return GLOSSARY_MINIMUM[self.whisper][name]

    def speech_is_the_recorded_one(self) -> bool:
        """Whether the generated speech has the checksums that are recorded in the repository."""
        assert self.audio is not None
        recorded = Path(__file__).parent / "speech" / "SHA256SUMS"
        generated = self.audio / "SHA256SUMS"
        if not recorded.is_file() or not generated.is_file():
            return False
        expected = _checksums(recorded)
        found = _checksums(generated)
        return all(expected.get(name) == digest for name, digest in found.items())

    def options(self) -> Options:
        assert self.models is not None
        return Options(models_dir=self.models, whisper=self.whisper, device=self.device)


def environment() -> Environment:
    def directory(name: str) -> Path | None:
        value = os.environ.get(name)
        return Path(value) if value else None

    return Environment(
        models=directory("JUNTO_TRANSCRIBE_MODELS"),
        audio=directory("JUNTO_TRANSCRIBE_TEST_AUDIO"),
        out=directory("JUNTO_TRANSCRIBE_TEST_OUT"),
        profile=os.environ.get("JUNTO_TRANSCRIBE_TEST_PROFILE", "quick"),
        whisper=os.environ.get("JUNTO_TRANSCRIBE_WHISPER_MODEL", "large-v3-turbo"),
        device=os.environ.get("JUNTO_TRANSCRIBE_DEVICE", "auto"),
        required=os.environ.get("JUNTO_TRANSCRIBE_TEST_REQUIRE") == "1",
    )


def normalize(text: str, language: str = "pt") -> str:
    text = unicodedata.normalize("NFC", text).lower()
    text = re.sub(r"[-\u2013\u2014/]", " ", text)
    text = "".join(character for character in text if not unicodedata.category(character).startswith(("P", "S")))
    words = []
    for word in text.split():
        if word.isdigit() and int(word) < len(NUMBER_WORDS[language]):
            words.append(NUMBER_WORDS[language][int(word)])
        else:
            words.append(word)
    return " ".join(words)


def word_error_rate(reference: str, hypothesis: str, language: str) -> float:
    expected = normalize(reference, language)
    found = normalize(hypothesis, language)
    if not found:
        return 1.0
    rate: float = jiwer.wer(expected, found)
    return rate


def spelled_right(terms: list[str], reference: str, hypothesis: str) -> tuple[int, int]:
    """Occurrences of glossary terms written as in the glossary; case and punctuation are ignored."""
    expected = f" {normalize(reference)} "
    found = f" {normalize(hypothesis)} "
    right = wanted = 0
    for term in terms:
        key = f" {normalize(term)} "
        wanted += expected.count(key)
        right += min(expected.count(key), found.count(key))
    return right, wanted


Segment = tuple[str, str, str, int]


class Runner:
    """Runs jobs on the generated speech with models that are loaded once."""

    def __init__(self, env: Environment, loaded: Loaded) -> None:
        self.env = env
        self.loaded = loaded
        self.summary: dict[str, Any] = {}
        self._cache: dict[str, dict[str, Any]] = {}

    def reference(self, name: str) -> dict[str, Any]:
        assert self.env.audio is not None
        loaded: dict[str, Any] = json.loads((self.env.audio / f"{name}.json").read_text("utf-8"))
        return loaded

    def document(
        self,
        segments: list[Segment],
        language: str = "auto",
        glossary: list[str] | None = None,
        separate_speakers: bool = True,
    ) -> dict[str, Any]:
        """Segments are (file name without suffix, kind, participant id, start_ms)."""
        return {
            "format_version": 1,
            "recording_id": "model-test",
            "language": language,
            "glossary": glossary or [],
            "separate_speakers": separate_speakers,
            "segments": [
                {
                    "id": f"{index}-{name}",
                    "path": f"{name}.ogg",
                    "kind": kind,
                    "participant": {"id": participant, "name": f"Participant {participant}"},
                    "start_ms": start_ms,
                }
                for index, (name, kind, participant, start_ms) in enumerate(segments)
            ],
        }

    def run(
        self, segments: list[Segment], cached: bool = True, chunk_ms: int | None = None, **options: Any
    ) -> dict[str, Any]:
        assert self.env.audio is not None
        document = self.document(segments, **options)
        key = json.dumps([document, chunk_ms], sort_keys=True)
        if cached and key in self._cache:
            return self._cache[key]
        loaded = self.loaded
        if chunk_ms is not None:
            loaded = replace(loaded, settings=replace(loaded.settings, chunk_ms=chunk_ms))
        with PinnedDirectory(self.env.audio) as files:
            transcript = transcribe(parse_job(document), files, self.env.options(), loaded)
        assert schema_errors("transcript", transcript) == []
        self._cache[key] = transcript
        return transcript

    def note(self, name: str, **values: Any) -> None:
        self.summary.setdefault(name, {}).update(values)

    def write_summary(self) -> None:
        if self.env.out is None or not self.summary:
            return
        header = {
            "speech_has_the_recorded_checksums": self.env.speech_is_the_recorded_one(),
            "whisper": self.env.whisper,
            "profile": self.env.profile,
            "device": self.loaded.device.name,
            "compute_type": self.loaded.device.compute_type,
            "threads": self.loaded.device.threads,
        }
        path = self.env.out / f"summary-{self.loaded.device.name}-{self.env.whisper}-{self.env.profile}.json"
        path.write_text(json.dumps({**header, "results": self.summary}, ensure_ascii=False, indent=1) + "\n")


def text_of(transcript: dict[str, Any]) -> str:
    return " ".join(line["text"] for line in transcript["lines"])


def speed(transcript: dict[str, Any]) -> float:
    stages = transcript["timing"]["stages_ms"]
    processing: int = sum(
        value for name, value in stages.items() if name not in ("verify_models", "load_models", "verify_models_again")
    )
    audio: int = transcript["timing"]["audio_ms"]
    return round(audio / max(processing, 1), 1)
