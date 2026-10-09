"""A segment read in blocks gives the same clips, lines and speakers as the same audio read whole."""

from __future__ import annotations

from dataclasses import replace

import numpy as np
import pytest

from junto_transcribe.clips import SAMPLE_RATE
from junto_transcribe.pipeline import Outcome
from tests.conftest import Bench
from tests.fakes import signal


def result(bench: Bench, outcome: Outcome) -> dict[str, object]:
    segment = outcome.segments[0]
    return {
        "lines": [(line.start_ms, line.end_ms, line.text, line.speaker, line.words) for line in outcome.lines],
        "clip lengths": list(bench.recognizer.clip_lengths),
        "segment": (
            segment.status,
            segment.audio_ms,
            segment.speech_ms,
            segment.clips,
            segment.lines,
            segment.speakers,
        ),
    }


def run(bench: Bench, chunk_ms: int | None, kind: str = "shared") -> dict[str, object]:
    bench.recognizer.clip_lengths.clear()
    bench.embedder.calls = 0
    if chunk_ms is not None:
        bench.settings = replace(bench.settings, chunk_ms=chunk_ms)
    return result(bench, bench.run([("s", "audio.ogg", kind, "a", 1000)]))


def test_speech_right_after_a_silent_block_keeps_its_padding(bench: Bench) -> None:
    bench.add("audio.ogg", signal(70, [(60.128, 62.0, 3)]))
    whole = run(bench, None, "microphone")
    # 1.872 s of speech in whole detector frames (59 frames), with 400 ms on each side.
    assert whole["clip lengths"] == [59 * 512 + 2 * round(0.4 * SAMPLE_RATE)]
    assert run(bench, 60_000, "microphone") == whole


def test_speech_across_a_block_boundary_is_one_clip(bench: Bench) -> None:
    bench.add("audio.ogg", signal(130, [(58.0, 63.0, 2), (119.9, 120.6, 4)]))
    whole = run(bench, None)
    assert run(bench, 60_000) == whole
    assert len(whole["clip lengths"]) == 2  # type: ignore[arg-type]


@pytest.mark.parametrize("seed", range(12))
def test_any_block_length_gives_the_result_of_the_whole_audio(bench: Bench, seed: int) -> None:
    generator = np.random.default_rng(seed)
    bursts = []
    cursor = float(generator.uniform(0.0, 2.0))
    while cursor < 170:
        length = float(generator.choice([0.2, 0.7, 1.5, 3.0, 6.0, 31.0]))
        bursts.append((round(cursor, 3), round(cursor + length, 3), int(generator.integers(1, 4))))
        cursor += length + float(generator.choice([0.1, 0.25, 0.4, 0.7, 1.0, 5.0, 20.0]))
    bench.add("audio.ogg", signal(180, bursts))
    bench.detector_memory = 0.6
    whole = run(bench, None)
    assert whole["lines"]
    for chunk_ms in (1000, 2752, 7000, 29_984, 60_000):
        assert run(bench, chunk_ms) == whole, f"blocks of {chunk_ms} ms"


def test_the_recognizer_gets_the_same_groups_of_clips_whatever_the_block_length(bench: Bench) -> None:
    bench.add("audio.ogg", signal(120, [(start, start + 1.0, 1) for start in range(1, 118, 3)]))
    bench.settings = replace(bench.settings, clips_per_call=7)
    run(bench, None, "microphone")
    whole = [call[0] for call in bench.recognizer.calls]
    bench.recognizer.calls.clear()
    run(bench, 5000, "microphone")
    assert [call[0] for call in bench.recognizer.calls] == whole == [7, 7, 7, 7, 7, 4]
