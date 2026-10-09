from __future__ import annotations

from junto_transcribe.pipeline import Outcome
from tests.conftest import Bench
from tests.fakes import signal


def turns(order: list[int], length: float = 3.0, gap: float = 1.0, lead: float = 1.0) -> list[tuple[float, float, int]]:
    return [
        (lead + index * (length + gap), lead + index * (length + gap) + length, voice)
        for index, voice in enumerate(order)
    ]


def labels(outcome: Outcome) -> list[tuple[int, str, str | None]]:
    return [(line.segment_index, line.text, line.speaker) for line in outcome.lines]


def voice_labels(outcome: Outcome, segment_index: int) -> dict[str, set[str | None]]:
    found: dict[str, set[str | None]] = {}
    for line in outcome.lines:
        if line.segment_index == segment_index:
            found.setdefault(line.text, set()).add(line.speaker)
    return found


def test_three_alternating_voices_get_three_labels_in_order_of_first_appearance(bench: Bench) -> None:
    bench.add("share.ogg", signal(40, turns([3, 1, 2, 3, 1, 2, 1, 3, 2])))
    outcome = bench.run([("s", "share.ogg", "shared", "a", 0)])
    assert voice_labels(outcome, 0) == {
        "utterance 3": {"Speaker 1"},
        "utterance 1": {"Speaker 2"},
        "utterance 2": {"Speaker 3"},
    }
    assert [(speaker.label, speaker.participant_id) for speaker in outcome.speakers] == [
        ("Speaker 1", "a"),
        ("Speaker 2", "a"),
        ("Speaker 3", "a"),
    ]
    assert outcome.segments[0].speakers == "separated"


def test_a_voice_keeps_its_label_in_every_segment_of_the_same_shared_audio(bench: Bench) -> None:
    bench.add("first.ogg", signal(30, turns([1, 2, 1, 2, 3, 3])))
    bench.add("second.ogg", signal(30, turns([3, 2, 3, 1, 2, 1])))
    outcome = bench.run([("first", "first.ogg", "shared", "a", 0), ("second", "second.ogg", "shared", "a", 90_000)])
    assert voice_labels(outcome, 0) == voice_labels(outcome, 1)
    assert voice_labels(outcome, 0) == {
        "utterance 1": {"Speaker 1"},
        "utterance 2": {"Speaker 2"},
        "utterance 3": {"Speaker 3"},
    }


def test_labels_do_not_depend_on_the_order_of_segments_in_the_job(bench: Bench) -> None:
    bench.add("first.ogg", signal(30, turns([1, 2, 1, 2, 3, 3])))
    bench.add("second.ogg", signal(30, turns([3, 2, 3, 1, 2, 1])))
    segments = [("first", "first.ogg", "shared", "a", 0), ("second", "second.ogg", "shared", "a", 90_000)]
    one = [(line.start_ms, line.text, line.speaker) for line in bench.run(segments).lines]
    two = [(line.start_ms, line.text, line.speaker) for line in bench.run(segments[::-1]).lines]
    assert one == two


def test_shared_audio_of_two_participants_is_clustered_apart_and_numbered_across_the_recording(bench: Bench) -> None:
    bench.add("a.ogg", signal(30, turns([1, 2, 1, 2, 1, 2])))
    bench.add("b.ogg", signal(30, turns([1, 2, 1, 2, 1, 2])))
    outcome = bench.run([("a", "a.ogg", "shared", "a", 0), ("b", "b.ogg", "shared", "b", 100)])
    assert voice_labels(outcome, 0) == {"utterance 1": {"Speaker 1"}, "utterance 2": {"Speaker 3"}}
    assert voice_labels(outcome, 1) == {"utterance 1": {"Speaker 2"}, "utterance 2": {"Speaker 4"}}
    assert [(speaker.label, speaker.participant_id) for speaker in outcome.speakers] == [
        ("Speaker 1", "a"),
        ("Speaker 2", "b"),
        ("Speaker 3", "a"),
        ("Speaker 4", "b"),
    ]


def test_microphones_are_never_clustered(bench: Bench) -> None:
    bench.add("mic.ogg", signal(30, turns([1, 2, 3, 1, 2, 3])))
    outcome = bench.run([("m", "mic.ogg", "microphone", "a", 0)])
    assert bench.embedder.calls == 0
    assert {line.speaker for line in outcome.lines} == {None}
    assert outcome.segments[0].speakers == "not_applicable"
    assert outcome.speakers == ()


def test_shared_audio_has_no_labels_when_the_job_does_not_ask_for_them(bench: Bench) -> None:
    bench.add("share.ogg", signal(30, turns([1, 2, 1, 2, 1, 2])))
    outcome = bench.run([("s", "share.ogg", "shared", "a", 0)], separate_speakers=False)
    assert bench.embedder.calls == 0
    assert {line.speaker for line in outcome.lines} == {None}
    assert outcome.segments[0].speakers == "not_requested"


def test_a_voice_heard_once_briefly_joins_the_nearest_speaker(bench: Bench) -> None:
    bench.add("share.ogg", signal(40, [*turns([1, 2, 1, 2, 1, 2]), (30.0, 31.0, 5)]))
    outcome = bench.run([("s", "share.ogg", "shared", "a", 0)])
    assert len(outcome.speakers) == 2
    assert outcome.lines[-1].text == "utterance 5"
    assert outcome.lines[-1].speaker in {"Speaker 1", "Speaker 2"}
