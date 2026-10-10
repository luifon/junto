from __future__ import annotations

from dataclasses import replace

from junto_transcribe.pipeline import Outcome
from tests.conftest import Bench
from tests.fakes import signal


def rows(outcome: Outcome) -> list[tuple[int, int, str, int]]:
    return [(line.start_ms, line.end_ms, line.text, line.segment_index) for line in outcome.lines]


def test_lines_of_several_segments_are_placed_on_one_timeline(bench: Bench) -> None:
    bench.add("a1.ogg", signal(10, [(1.0, 2.0, 1), (5.0, 6.5, 2)]))
    bench.add("a2.ogg", signal(4, [(0.5, 1.5, 3)]))
    bench.add("b1.ogg", signal(6, [(0.2, 1.2, 4)]))
    outcome = bench.run(
        [
            ("a1", "a1.ogg", "microphone", "a", 0),
            ("a2", "a2.ogg", "microphone", "a", 15_000),
            ("b1", "b1.ogg", "microphone", "b", 2500),
        ]
    )
    assert rows(outcome) == [
        (1000, 2000, "utterance 1", 0),
        (2700, 3700, "utterance 4", 2),
        (5000, 6500, "utterance 2", 0),
        (15_500, 16_500, "utterance 3", 1),
    ]


def test_a_reconnect_gap_leaves_no_line_and_keeps_the_offset_of_the_second_file(bench: Bench) -> None:
    bench.add("before.ogg", signal(8, [(6.0, 7.9, 1)]))
    bench.add("after.ogg", signal(5, [(0.0, 1.0, 1)]))
    outcome = bench.run(
        [("before", "before.ogg", "microphone", "a", 1000), ("after", "after.ogg", "microphone", "a", 12_345)]
    )
    assert rows(outcome) == [(7000, 8900, "utterance 1", 0), (12_345, 13_345, "utterance 1", 1)]
    assert [(result.id, result.audio_ms) for result in outcome.segments] == [("before", 8000), ("after", 5000)]


def test_word_times_are_on_the_same_timeline(bench: Bench) -> None:
    bench.add("a.ogg", signal(5, [(1.0, 3.0, 7)]))
    line = bench.run([("a", "a.ogg", "microphone", "a", 60_000)]).lines[0]
    assert line.words is not None
    assert "".join(word.text for word in line.words).strip() == line.text
    assert [(word.start_ms, word.end_ms, word.text) for word in line.words] == [
        (61_000, 62_000, " utterance"),
        (62_000, 63_000, " 7"),
    ]


def test_lines_that_start_together_keep_the_order_of_the_job(bench: Bench) -> None:
    bench.add("a.ogg", signal(3, [(1.0, 2.0, 1)]))
    bench.add("b.ogg", signal(3, [(1.0, 2.0, 2)]))
    outcome = bench.run([("b", "b.ogg", "microphone", "b", 0), ("a", "a.ogg", "microphone", "a", 0)])
    assert [line.text for line in outcome.lines] == ["utterance 2", "utterance 1"]


def test_the_order_of_segments_in_the_job_does_not_change_the_lines(bench: Bench) -> None:
    bench.add("a.ogg", signal(10, [(1.0, 2.0, 1), (5.0, 6.5, 2)]))
    bench.add("b.ogg", signal(6, [(0.2, 1.2, 4)]))
    first = [("a", "a.ogg", "microphone", "a", 0), ("b", "b.ogg", "microphone", "b", 2500)]
    one = [(line.start_ms, line.end_ms, line.text) for line in bench.run(first).lines]
    two = [(line.start_ms, line.end_ms, line.text) for line in bench.run(first[::-1]).lines]
    assert one == two


def test_a_fixed_language_reaches_every_clip_and_auto_detects_each_clip(bench: Bench) -> None:
    bench.add("a.ogg", signal(10, [(1.0, 2.0, 1), (5.0, 6.0, 2)]))
    segments = [("a", "a.ogg", "microphone", "a", 0)]
    assert [line.language for line in bench.run(segments).lines] == ["en", "pt"]
    assert [line.language for line in bench.run(segments, language="pt").lines] == ["pt", "pt"]
    assert [call[1] for call in bench.recognizer.calls] == [None, "pt"]


def test_silence_never_reaches_the_recognizer(bench: Bench) -> None:
    bench.add("silent.ogg", signal(600, []))
    outcome = bench.run([("s", "silent.ogg", "microphone", "a", 0)])
    assert bench.recognizer.calls == []
    assert outcome.lines == ()
    assert (outcome.segments[0].status, outcome.segments[0].audio_ms, outcome.segments[0].clips) == (
        "empty",
        600_000,
        0,
    )


def test_a_long_file_read_in_blocks_gives_the_same_lines_as_one_block(bench: Bench) -> None:
    bursts = [
        (10.0, 12.0, 1),
        (58.0, 63.0, 2),
        (100.0, 101.0, 3),
        (119.9, 120.6, 4),
        (179.5, 180.0, 5),
        (200.0, 204.0, 6),
    ]
    bench.add("long.ogg", signal(210, bursts))
    segments = [("long", "long.ogg", "shared", "a", 1000)]
    whole = bench.run(segments)
    bench.settings = replace(bench.settings, chunk_ms=60_000)
    blocks = bench.run(segments)
    assert rows(blocks) == rows(whole)
    assert [line.text for line in blocks.lines] == [f"utterance {code}" for code in range(1, 7)]
    assert blocks.segments[0].audio_ms == whole.segments[0].audio_ms == 210_000
    # A block starts a new grid of 32 ms detector frames, so each span can move by one frame at each end.
    assert abs(blocks.segments[0].speech_ms - whole.segments[0].speech_ms) <= 6 * 64
    assert blocks.segments[0].clips == whole.segments[0].clips == 6


def test_clips_go_to_the_recognizer_in_groups(bench: Bench) -> None:
    bench.add("many.ogg", signal(40, [(start, start + 1.0, 1) for start in range(1, 38, 3)]))
    bench.settings = replace(bench.settings, clips_per_call=5)
    outcome = bench.run([("m", "many.ogg", "microphone", "a", 0)])
    assert [call[0] for call in bench.recognizer.calls] == [5, 5, 3]
    assert len(outcome.lines) == 13
