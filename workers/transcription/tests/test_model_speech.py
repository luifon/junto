"""The real models on generated speech. Needs the model files and the files of `tests/speech/generate.py`."""

from __future__ import annotations

import json
import socket
import subprocess
import sys
from collections import Counter, defaultdict
from collections.abc import Iterator
from dataclasses import replace
from pathlib import Path
from typing import Any

import numpy as np
import pytest

from junto_transcribe.audio import AvAudioReader
from junto_transcribe.clips import SAMPLE_RATE, ClipRules, build_clips
from junto_transcribe.contract import JobError, parse_job, schema_errors
from junto_transcribe.files import PinnedDirectory
from junto_transcribe.glossary import TOKEN_LIMIT
from junto_transcribe.worker import load, transcribe
from tests.model_support import (
    Environment,
    Runner,
    environment,
    speed,
    spelled_right,
    text_of,
    word_error_rate,
)

pytestmark = pytest.mark.models

FILLER = (
    "alder birch cedar dogwood elm fir ginkgo hazel juniper larch maple oak pine rowan spruce "
    "willow yew amber bronze copper crimson indigo ivory jade khaki lilac magenta ochre olive "
    "russet saffron scarlet teal umber violet anchor barrel candle drawer engine feather garden "
    "hammer island jacket kettle ladder mirror needle orchard pencil quarry ribbon saddle tunnel"
).split()


@pytest.fixture(scope="session")
def env() -> Environment:
    found = environment()
    reason = found.missing()
    if reason is not None:
        if found.required:
            pytest.fail(reason)
        pytest.skip(reason)
    return found


@pytest.fixture(scope="session")
def runner(env: Environment) -> Iterator[Runner]:
    made = Runner(env, load(env.options(), speakers=True))
    yield made
    made.write_summary()


@pytest.fixture(autouse=True)
def no_network_call(monkeypatch: pytest.MonkeyPatch) -> None:
    def refuse(*_: object, **__: object) -> None:
        raise AssertionError("the worker tried to use the network")

    monkeypatch.setattr(socket.socket, "connect", refuse)
    monkeypatch.setattr(socket, "getaddrinfo", refuse)
    monkeypatch.setattr(socket, "create_connection", refuse)


@pytest.fixture(scope="session")
def terms() -> list[str]:
    path = Path(__file__).parent / "speech" / "texts" / "glossary_terms.txt"
    return [line.strip() for line in path.read_text("utf-8").splitlines() if line.strip()]


def read_file(path: Path) -> Any:
    with path.open("rb") as source:
        return np.concatenate([block for block, _ in AvAudioReader().blocks(source, 10**9)])


def check_lines(transcript: dict[str, Any], start_ms: int, seconds: float) -> None:
    previous = 0
    for line in transcript["lines"]:
        assert start_ms <= line["start_ms"] <= line["end_ms"] <= start_ms + round(seconds * 1000) + 50
        assert line["start_ms"] >= previous
        previous = line["start_ms"]
        assert line["words"], "a line has no word times"
        assert "".join(word["text"] for word in line["words"]).strip() == line["text"]
        word_start = line["start_ms"]
        for word in line["words"]:
            assert line["start_ms"] <= word["start_ms"] <= word["end_ms"] <= line["end_ms"]
            assert word["start_ms"] >= word_start
            word_start = word["start_ms"]


def sentence_of(reference: dict[str, Any], line: dict[str, Any], start_ms: int) -> dict[str, Any] | None:
    """The reference sentence that shares the most time with a line."""
    best = None
    most = 0.0
    for sentence in reference["sentences"]:
        shared = min(sentence["end"], (line["end_ms"] - start_ms) / 1000) - max(
            sentence["start"], (line["start_ms"] - start_ms) / 1000
        )
        if shared > most:
            best, most = sentence, shared
    return best


@pytest.mark.parametrize("language", ["en", "pt"])
@pytest.mark.parametrize("mode", ["auto", "fixed"])
def test_word_error_rate_of_one_language(runner: Runner, language: str, mode: str) -> None:
    name = runner.env.name(language)
    reference = runner.reference(name)
    transcript = runner.run([(name, "microphone", "p1", 5000)], language="auto" if mode == "auto" else language)
    rate = word_error_rate(reference["text"], text_of(transcript), language)
    runner.note(f"{name} ({mode})", wer=round(rate, 4), speed=speed(transcript), lines=len(transcript["lines"]))

    assert rate <= runner.env.bound("wer")
    assert transcript["segments"][0]["status"] == "ok"
    assert {line["source"]["kind"] for line in transcript["lines"]} == {"microphone"}
    languages = Counter(line["language"] for line in transcript["lines"])
    assert languages[language] >= 0.9 * len(transcript["lines"])
    check_lines(transcript, 5000, reference["seconds"])
    first = transcript["lines"][0]
    assert abs(first["start_ms"] - 5000 - reference["sentences"][0]["start"] * 1000) <= 700


def test_near_silence_gives_no_lines(runner: Runner) -> None:
    names = ["noise_brown_quick"] if runner.env.quick else ["noise_brown", "noise_pink"]
    segments = [(name, kind, "p1", 0) for name in names for kind in ("microphone", "shared")]
    transcript = runner.run(segments)
    runner.note("near silence", lines=len(transcript["lines"]), speed=speed(transcript), files=names)
    assert transcript["lines"] == []
    assert [segment["status"] for segment in transcript["segments"]] == ["empty"] * len(segments)
    assert all(segment["clips"] == 0 and segment["audio_ms"] >= 119_000 for segment in transcript["segments"])


@pytest.mark.parametrize("name", ["sparse", "sparse_room"])
def test_two_sentences_in_ten_minutes_of_noise_give_exactly_two_lines(runner: Runner, name: str) -> None:
    if runner.env.quick and name == "sparse_room":
        pytest.skip("the quick profile runs one noise file")
    reference = runner.reference(name)
    transcript = runner.run([(name, "microphone", "p1", 0)])
    lines = transcript["lines"]
    runner.note(name, lines=len(lines), speed=speed(transcript), text=[line["text"] for line in lines])
    assert len(lines) == 2
    for line, sentence in zip(lines, reference["sentences"], strict=True):
        assert line["language"] == sentence["language"]
        if runner.env.whisper == "large-v3-turbo":
            # No wrong word in a sentence of six or seven words. The development model is not held to
            # this: its text is recorded in the summary.
            assert word_error_rate(sentence["text"], line["text"], sentence["language"]) <= 0.15
        assert abs(line["start_ms"] - sentence["start"] * 1000) <= 700
        assert abs(line["end_ms"] - sentence["end"] * 1000) <= 700


def test_mixed_speech_is_transcribed_in_both_languages(runner: Runner) -> None:
    name = runner.env.name("mix")
    reference = runner.reference(name)
    transcript = runner.run([(name, "microphone", "p1", 0)])
    rate = word_error_rate(reference["text"], text_of(transcript), "pt")
    matches = [
        (sentence_of(reference, line, 0) or {}).get("language") == line["language"] for line in transcript["lines"]
    ]
    runner.note(name, wer=round(rate, 4), speed=speed(transcript), language_matches=f"{sum(matches)}/{len(matches)}")

    assert rate <= runner.env.bound("wer_mixed")
    assert {line["language"] for line in transcript["lines"]} == {"en", "pt"}
    assert sum(matches) >= 0.9 * len(matches)
    for language in ("en", "pt"):
        expected = " ".join(s["text"] for s in reference["sentences"] if s["language"] == language)
        found = " ".join(line["text"] for line in transcript["lines"] if line["language"] == language)
        assert word_error_rate(expected, found, language) <= 2 * runner.env.bound("wer_mixed")


@pytest.mark.parametrize("language", ["en", "pt"])
def test_a_glossary_improves_the_spelling_of_invented_terms(runner: Runner, terms: list[str], language: str) -> None:
    name = runner.env.name(f"gloss_{language}")
    reference = runner.reference(name)
    segments = [(name, "microphone", "p1", 0)]
    without = runner.run(segments)
    with_glossary = runner.run(segments, glossary=terms)
    right_without, wanted = spelled_right(terms, reference["text"], text_of(without))
    right_with, _ = spelled_right(terms, reference["text"], text_of(with_glossary))
    runner.note(
        name,
        spelled_right_without=f"{right_without}/{wanted}",
        spelled_right_with=f"{right_with}/{wanted}",
        wer_without=round(word_error_rate(reference["text"], text_of(without), language), 4),
        wer_with=round(word_error_rate(reference["text"], text_of(with_glossary), language), 4),
        glossary_tokens=with_glossary["glossary"]["token_count"],
    )

    assert wanted >= 8
    assert right_with > right_without
    assert right_with >= runner.env.glossary_minimum(name)
    assert with_glossary["glossary"]["used"] == terms
    assert with_glossary["glossary"]["cut"] == []
    assert 0 < with_glossary["glossary"]["token_count"] <= TOKEN_LIMIT
    assert without["glossary"] == {"token_limit": TOKEN_LIMIT, "token_count": 0, "used": [], "cut": []}


def test_a_glossary_over_the_limit_is_cut_in_priority_order_and_still_works(runner: Runner, terms: list[str]) -> None:
    name = runner.env.name("gloss_en")
    reference = runner.reference(name)
    long_glossary = terms + [f"{word.capitalize()} {number}" for number in range(1, 5) for word in FILLER]
    transcript = runner.run([(name, "microphone", "p1", 0)], glossary=long_glossary)
    glossary = transcript["glossary"]
    right, wanted = spelled_right(terms, reference["text"], text_of(transcript))
    runner.note(
        f"{name} (long glossary)",
        used=len(glossary["used"]),
        cut=len(glossary["cut"]),
        tokens=glossary["token_count"],
        spelled_right=f"{right}/{wanted}",
    )

    assert glossary["cut"], "the glossary was not over the limit"
    assert glossary["used"] + glossary["cut"] == long_glossary
    assert glossary["used"][: len(terms)] == terms
    assert TOKEN_LIMIT - 12 <= glossary["token_count"] <= TOKEN_LIMIT
    right_without, _ = spelled_right(terms, reference["text"], text_of(runner.run([(name, "microphone", "p1", 0)])))
    assert right > right_without
    assert right >= runner.env.bound("glossary_long") * wanted


def test_the_tokenizer_of_the_model_finds_every_control_token_in_a_glossary_term(runner: Runner) -> None:
    recognizer: Any = runner.loaded.engines.recognizer
    forms = recognizer.control_tokens
    runner.note("control tokens of the model", count=len(forms))
    assert {"<|endoftext|>", "<|startoftranscript|>", "<|translate|>", "<|transcribe|>", "<|en|>", "<|pt|>"} <= set(
        forms
    )
    assert len(forms) >= 100
    missed = [form for form in forms if not recognizer.control_token_in(f"Quorvex {form} Lumitrax")]
    assert missed == []
    for form in ("<|en|><|translate|>", "<|endoftext|>", "Quorvex<|notimestamps|>"):
        with pytest.raises(JobError) as refused:
            parse_job(runner.document([("sparse", "microphone", "p1", 0)], glossary=["Quorvex", form]))
        assert (refused.value.code, refused.value.location) == ("glossary_control_token", "/glossary/1")


def test_the_tokenizer_of_the_model_accepts_ordinary_terms(runner: Runner, terms: list[str]) -> None:
    recognizer = runner.loaded.engines.recognizer
    ordinary = [*terms, "C++ <template>", "A<B", "x|y", "Zyphra (v2) > Nexavio", "São João d'El-Rei", "100 %", "<en>"]
    assert [term for term in ordinary if recognizer.control_token_in(term)] == []


def test_a_job_whose_glossary_would_reach_the_model_as_a_control_token_is_refused(runner: Runner) -> None:
    """The check with the loaded tokenizer, for a job that did not pass the check of the job file."""
    assert runner.env.audio is not None
    job = parse_job(runner.document([("sparse", "microphone", "p1", 0)], glossary=["Lumitrax", "Quorvex"]))
    with PinnedDirectory(runner.env.audio) as files, pytest.raises(JobError) as refused:
        transcribe(replace(job, glossary=("Lumitrax", "<|pt|>")), files, runner.env.options(), runner.loaded)
    assert (refused.value.code, refused.value.location) == ("glossary_control_token", "/glossary/1")


def test_three_alternating_voices_get_three_stable_labels(runner: Runner) -> None:
    first = runner.reference("trio_first")
    second = runner.reference("trio_second")
    second_start = round(first["seconds"] * 1000) + 20_000
    microphone = runner.env.name("en")
    transcript = runner.run(
        [
            ("trio_first", "shared", "p1", 0),
            ("trio_second", "shared", "p1", second_start),
            (microphone, "microphone", "p2", 0),
        ]
    )

    shared = [line for line in transcript["lines"] if line["source"]["kind"] == "shared"]
    by_part: dict[str, dict[str, Counter[str]]] = {
        "trio_first": defaultdict(Counter),
        "trio_second": defaultdict(Counter),
    }
    for line in shared:
        part = "trio_first" if line["segment_id"].endswith("trio_first") else "trio_second"
        reference, offset = (first, 0) if part == "trio_first" else (second, second_start)
        sentence = sentence_of(reference, line, offset)
        assert sentence is not None
        by_part[part][sentence["speaker"]][line["source"]["speaker"]] += 1

    labels = {line["source"]["speaker"] for line in shared}
    mapping = {
        part: {voice: counts.most_common(1)[0][0] for voice, counts in voices.items()}
        for part, voices in by_part.items()
    }
    agreeing = sum(counts[mapping[part][voice]] for part, voices in by_part.items() for voice, counts in voices.items())
    rate = word_error_rate(f"{first['text']} {second['text']}", " ".join(line["text"] for line in shared), "en")
    runner.note(
        "trio in two shared segments",
        labels=sorted(labels),
        mapping=mapping,
        lines_with_the_label_of_their_voice=f"{agreeing}/{len(shared)}",
        wer=round(rate, 4),
        speed=speed(transcript),
    )

    assert labels == {"Speaker 1", "Speaker 2", "Speaker 3"}
    assert mapping["trio_first"] == mapping["trio_second"]
    assert len(set(mapping["trio_first"].values())) == 3
    assert agreeing >= 0.9 * len(shared)
    assert rate <= runner.env.bound("wer")
    assert [speaker["participant_id"] for speaker in transcript["speaker_separation"]["speakers"]] == ["p1"] * 3
    assert all(
        "speaker" not in line["source"] for line in transcript["lines"] if line["source"]["kind"] == "microphone"
    )
    assert [segment["speakers"] for segment in transcript["segments"]] == ["separated", "separated", "not_applicable"]


def test_the_same_job_twice_gives_the_same_transcript(runner: Runner) -> None:
    segments = [("sparse", "microphone", "p1", 0), (runner.env.name("en"), "shared", "p2", 1000)]
    one = runner.run(segments, cached=False)
    two = runner.run(segments, cached=False)
    assert one["lines"]
    assert {key: value for key, value in one.items() if key != "timing"} == {
        key: value for key, value in two.items() if key != "timing"
    }


@pytest.mark.parametrize("chunk_ms", [1000, 7000, 60_000])
def test_a_job_read_in_blocks_gives_the_transcript_of_the_job_read_whole(runner: Runner, chunk_ms: int) -> None:
    segments = [(runner.env.name("mix"), "microphone", "p1", 5000), ("trio_first", "shared", "p2", 0)]
    whole = runner.run(segments)
    blocks = runner.run(segments, chunk_ms=chunk_ms)
    runner.note(
        f"mixed speech and three voices read in {chunk_ms} ms blocks",
        same_as_whole=all(blocks[key] == whole[key] for key in ("lines", "segments", "speaker_separation")),
        lines=[len(blocks["lines"]), len(whole["lines"])],
    )
    assert len(whole["lines"]) > 10
    for key in ("lines", "segments", "speaker_separation", "glossary"):
        assert blocks[key] == whole[key], key


def test_the_detector_gives_the_same_probabilities_for_pieces_as_for_the_whole_audio(runner: Runner) -> None:
    assert runner.env.audio is not None
    samples = read_file(runner.env.audio / f"{runner.env.name('mix')}.wav")
    detector = runner.loaded.engines.detector
    whole = detector.stream().probabilities(samples, True)
    for frames in (1, 31, 500):
        stream = detector.stream()
        size = frames * 512
        pieces: list[float] = []
        for start in range(0, len(samples), size):
            piece = samples[start : start + size]
            pieces += stream.probabilities(piece, start + size >= len(samples))
        assert len(pieces) == len(whole)
        assert float(np.abs(np.array(pieces) - np.array(whole)).max()) < 1e-5, f"pieces of {frames} frames"


def test_the_header_names_models_libraries_and_times(runner: Runner) -> None:
    transcript = runner.run([("sparse", "microphone", "p1", 0)])
    engine = transcript["engine"]
    assert [model["role"] for model in engine["models"]] == ["voice-detector", "whisper", "speaker-embedding"]
    assert engine["models"][1]["name"] == runner.env.whisper
    assert {
        "faster-whisper",
        "ctranslate2",
        "onnxruntime",
        "sherpa-onnx",
        "av",
        "tokenizers",
        "numpy",
        "python",
    } <= set(engine["libraries"])
    assert engine["libraries"]["onnxruntime"] == "1.28.0"
    assert engine["settings"]["clip_pad_ms"] == 400 and engine["settings"]["speaker_threshold"] == 0.6
    assert engine["settings"]["glossary_token_limit"] == TOKEN_LIMIT == 200
    timing = transcript["timing"]
    assert timing["audio_ms"] == 600_000 and 0 < timing["speech_ms"] < 10_000
    stages = {"verify_models", "load_models", "verify_models_again", "decode", "voice_detection", "transcription"}
    assert stages <= set(timing["stages_ms"])
    assert timing["peak_memory_mb"] > 0
    runner.note(
        "resources",
        peak_memory_mb=timing["peak_memory_mb"],
        load_models_ms=runner.loaded.stages_ms["load_models"],
        verify_models_ms=runner.loaded.stages_ms["verify_models"],
    )


@pytest.mark.gpu
def test_the_gpu_is_used_when_it_is_asked_for(runner: Runner) -> None:
    if runner.env.device != "cuda":
        pytest.skip("JUNTO_TRANSCRIBE_DEVICE is not cuda")
    engine = runner.run([("sparse", "microphone", "p1", 0)])["engine"]
    assert (engine["device"], engine["compute_type"]) == ("cuda", "float16")
    assert {"nvidia-cublas-cu12", "nvidia-cudnn-cu12"} <= set(engine["libraries"])


def test_clips_on_real_speech_equal_the_measured_library_function(runner: Runner) -> None:
    from faster_whisper import vad

    assert runner.env.audio is not None
    name = runner.env.name("mix")
    samples = read_file(runner.env.audio / f"{name}.wav")
    probabilities = runner.loaded.engines.detector.stream().probabilities(samples, True)
    clips = build_clips(probabilities, len(samples), ClipRules())
    options = vad.VadOptions(min_silence_duration_ms=300, speech_pad_ms=400, max_speech_duration_s=29)
    expected = [(stamp["start"], stamp["end"]) for stamp in vad.get_speech_timestamps(samples, options)]
    assert len(clips) >= 5
    assert [(clip.start, clip.end) for clip in clips] == expected


def test_the_recorder_format_decodes_to_the_same_timeline_as_the_wav_file(runner: Runner) -> None:
    assert runner.env.audio is not None
    name = runner.env.name("en")
    wav = read_file(runner.env.audio / f"{name}.wav")
    ogg = read_file(runner.env.audio / f"{name}.ogg")
    assert abs(len(wav) - len(ogg)) <= SAMPLE_RATE // 50
    size = min(len(wav), len(ogg))
    correlation = float(np.corrcoef(wav[:size], ogg[:size])[0, 1])
    assert correlation > 0.97


def test_a_recorder_file_with_lost_pages_keeps_the_times_of_the_speech_after_them(
    runner: Runner, tmp_path: Path
) -> None:
    assert runner.env.audio is not None
    name = runner.env.name("pt")
    reference = runner.reference(name)
    data = (runner.env.audio / f"{name}.ogg").read_bytes()
    whole_ms = round(len(read_file(runner.env.audio / f"{name}.ogg")) / 16)
    lost = round(len(data) * 0.45)
    (tmp_path / "lost.ogg").write_bytes(data[:lost] + data[lost + 30_000 :])
    document = runner.document([("lost", "microphone", "p1", 0)])
    with PinnedDirectory(tmp_path) as files:
        transcript = transcribe(parse_job(document), files, runner.env.options(), runner.loaded)
    segment = transcript["segments"][0]
    last = transcript["lines"][-1]
    runner.note(
        f"{name} with 30 kB of pages lost",
        status=segment["status"],
        audio_ms=segment["audio_ms"],
        whole_file_ms=whole_ms,
        last_line_end_ms=last["end_ms"],
        reference_last_end_ms=round(reference["sentences"][-1]["end"] * 1000),
    )
    assert segment["status"] == "ok"
    assert abs(segment["audio_ms"] - whole_ms) <= 100
    assert abs(last["end_ms"] - reference["sentences"][-1]["end"] * 1000) <= 700


def run_cli(*arguments: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, "-m", "junto_transcribe", *arguments], capture_output=True, text=True, check=False
    )


def test_the_command_line_runs_a_whole_job_and_reports_a_failed_segment(
    runner: Runner, tmp_path: Path, terms: list[str]
) -> None:
    env = runner.env
    assert env.audio is not None and env.models is not None
    document = runner.document(
        [(env.name("pt"), "microphone", "p1", 0), ("sparse", "shared", "p2", 2000), ("absent", "microphone", "p3", 0)],
        glossary=terms,
    )
    job_file = tmp_path / "job.json"
    job_file.write_text(json.dumps(document))
    out = (env.out or tmp_path) / f"transcript-{runner.loaded.device.name}.json"
    out.unlink(missing_ok=True)
    engine = ["--models-dir", str(env.models), "--whisper-model", env.whisper, "--device", env.device]

    result = run_cli("run", str(job_file), "--out", str(out), "--audio-root", str(env.audio), *engine)
    assert result.returncode == 0, result.stderr
    transcript = json.loads(out.read_text("utf-8"))
    assert schema_errors("transcript", transcript) == []
    assert [segment["status"] for segment in transcript["segments"]] == ["ok", "ok", "failed"]
    assert transcript["segments"][2]["reason"]["code"] == "file_missing"
    assert [segment["speakers"] for segment in transcript["segments"]] == [
        "not_applicable",
        "separated",
        "not_applicable",
    ]
    assert len(transcript["lines"]) > 2
    assert transcript["glossary"]["used"] == terms
    for line in transcript["lines"]:
        assert line["text"] not in result.stderr
    for value in (*terms, "model-test", "Participant", "pt_quick", "sparse", "absent"):
        assert value not in result.stderr + result.stdout, value

    verified = run_cli("models", "verify", "--dir", str(env.models), "--whisper", env.whisper)
    assert verified.returncode == 0, verified.stderr
    self_test = run_cli("self-test", *engine)
    assert self_test.returncode == 0, self_test.stdout + self_test.stderr
    assert "self-test passed" in self_test.stdout


@pytest.mark.offline
def test_the_machine_has_no_network(env: Environment) -> None:
    classes = Path("/sys/class/net")
    interfaces = sorted(path.name for path in classes.iterdir()) if classes.is_dir() else None
    if interfaces != ["lo"]:
        reason = f"this machine has network interfaces: {interfaces}"
        if env.required:
            pytest.fail(reason)
        pytest.skip(reason)
