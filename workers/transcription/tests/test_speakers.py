from __future__ import annotations

import numpy as np
import pytest
from scipy.cluster.hierarchy import fcluster, linkage

from junto_transcribe.clips import SAMPLE_RATE, Span
from junto_transcribe.speakers import (
    SpeakerRules,
    Turn,
    Window,
    cluster_speakers,
    speaker_of,
    speaker_turns,
    windows_for_span,
)

RULES = SpeakerRules()


def seconds(span: Span) -> tuple[float, float]:
    return (span.start / SAMPLE_RATE, span.end / SAMPLE_RATE)


def voices(counts: list[int], seed: int, spread: float = 0.25, dimensions: int = 32) -> tuple[np.ndarray, list[int]]:
    generator = np.random.default_rng(seed)
    centres = generator.standard_normal((len(counts), dimensions))
    rows = []
    truth = []
    for voice, count in enumerate(counts):
        rows.append(centres[voice] + generator.standard_normal((count, dimensions)) * spread)
        truth += [voice] * count
    order = generator.permutation(len(truth))
    return np.concatenate(rows)[order], [truth[index] for index in order]


def same_partition(first: list[int], second: list[int]) -> bool:
    return len(set(zip(first, second, strict=True))) == len(set(first)) == len(set(second))


def test_a_short_span_is_one_window() -> None:
    assert windows_for_span(Span(1600, 33_600), RULES) == [Span(1600, 33_600)]


def test_a_long_span_gets_overlapping_windows_that_end_at_its_end() -> None:
    windows = windows_for_span(Span(0, 6 * SAMPLE_RATE), RULES)
    assert [seconds(window) for window in windows] == [(0.0, 2.5), (1.25, 3.75), (2.5, 5.0), (3.5, 6.0)]


@pytest.mark.parametrize("seed", range(40))
def test_clusters_equal_average_linkage_cut_at_the_threshold(seed: int) -> None:
    generator = np.random.default_rng(seed)
    count = int(generator.integers(2, 80))
    vectors = generator.standard_normal((count, 6))
    vectors[: count // 2] += 3.0 * generator.standard_normal(6)
    threshold = float(generator.uniform(0.2, 1.2))
    rules = SpeakerRules(threshold=threshold, min_cluster_windows=1)
    expected = fcluster(linkage(vectors, method="average", metric="cosine"), t=threshold, criterion="distance")
    assert same_partition(cluster_speakers(vectors, rules).tolist(), expected.tolist())


def test_three_voices_give_three_speakers_numbered_by_first_appearance() -> None:
    vectors, truth = voices([20, 14, 9], seed=3)
    labels = cluster_speakers(vectors, RULES).tolist()
    assert same_partition(labels, truth)
    assert [labels.index(number) for number in range(3)] == sorted(labels.index(number) for number in range(3))


def test_the_same_vectors_in_another_order_give_the_same_speakers() -> None:
    vectors, _ = voices([12, 12, 12], seed=5)
    order = np.random.default_rng(1).permutation(len(vectors))
    first = cluster_speakers(vectors, RULES).tolist()
    second = cluster_speakers(vectors[order], RULES).tolist()
    assert same_partition([first[index] for index in order], second)


def test_a_cluster_of_fewer_than_three_windows_joins_the_nearest_voice() -> None:
    vectors, truth = voices([10, 10], seed=8, spread=0.1)
    stray = vectors[truth.index(1)] * 0.4 + np.random.default_rng(2).standard_normal(32) * 1.5
    labels = cluster_speakers(np.vstack([vectors, stray]), RULES).tolist()
    assert len(set(labels)) == 2
    assert same_partition(labels[:-1], truth)


def test_one_vector_is_one_speaker_and_none_is_none() -> None:
    assert cluster_speakers(np.ones((1, 4)), RULES).tolist() == [0]
    assert cluster_speakers(np.zeros((0, 4)), RULES).tolist() == []


def test_more_vectors_than_the_limit_are_assigned_by_nearest_centroid() -> None:
    vectors, truth = voices([400, 300, 200], seed=11)
    rules = SpeakerRules(max_cluster_windows=150)
    assert same_partition(cluster_speakers(vectors, rules).tolist(), truth)


def test_turns_change_halfway_between_window_centres() -> None:
    windows = [Window(0, 0, 0, 40_000), Window(0, 0, 20_000, 60_000), Window(0, 0, 40_000, 80_000)]
    turns = speaker_turns(windows, [0, 0, 1])
    assert turns == {0: [Turn(0, 50_000, 0), Turn(50_000, 80_000, 1)]}


def test_turns_stay_inside_their_segment_and_span() -> None:
    windows = [Window(0, 0, 0, 30_000), Window(0, 1, 50_000, 70_000), Window(1, 0, 10_000, 20_000)]
    turns = speaker_turns(windows, [0, 1, 0])
    assert turns == {0: [Turn(0, 30_000, 0), Turn(50_000, 70_000, 1)], 1: [Turn(10_000, 20_000, 0)]}


def test_a_line_takes_the_speaker_with_the_most_time() -> None:
    turns = [Turn(0, 10_000, 0), Turn(10_000, 50_000, 1), Turn(50_000, 60_000, 0)]
    assert speaker_of(turns, 5000, 30_000) == 1
    assert speaker_of(turns, 0, 12_000) == 0
    assert speaker_of(turns, 0, 20_000) == 0
    assert speaker_of(turns, 70_000, 80_000) is None
    assert speaker_of([], 0, 10) is None
