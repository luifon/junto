"""Speaker separation for shared audio: windows, clustering of embeddings, turns."""

from __future__ import annotations

from collections import defaultdict
from collections.abc import Sequence
from dataclasses import dataclass

import numpy as np
from numpy.typing import NDArray

from junto_transcribe.clips import Span, samples_from_ms

Vectors = NDArray[np.float64]
Labels = NDArray[np.int64]

ASSIGN_BATCH = 4096


@dataclass(frozen=True)
class SpeakerRules:
    window_ms: int = 2500
    hop_ms: int = 1250
    threshold: float = 0.6
    min_cluster_windows: int = 3
    max_cluster_windows: int = 4000

    def __post_init__(self) -> None:
        if not 0 < self.hop_ms <= self.window_ms:
            raise ValueError("hop_ms must be positive and at most window_ms")
        if not 0.0 < self.threshold < 2.0:
            raise ValueError("threshold is a cosine distance between 0 and 2")
        if self.min_cluster_windows < 1 or self.max_cluster_windows < 2:
            raise ValueError("cluster sizes are too small")


@dataclass(frozen=True)
class Window:
    segment: int
    span: int
    start: int
    end: int


@dataclass(frozen=True)
class Turn:
    start: int
    end: int
    speaker: int


def windows_for_span(span: Span, rules: SpeakerRules) -> list[Span]:
    size = samples_from_ms(rules.window_ms)
    hop = samples_from_ms(rules.hop_ms)
    if span.length <= size:
        return [span]
    windows = []
    position = span.start
    while position + size < span.end:
        windows.append(Span(position, position + size))
        position += hop
    windows.append(Span(max(span.start, span.end - size), span.end))
    return windows


def unit_rows(vectors: NDArray[np.floating]) -> Vectors:
    rows: Vectors = np.array(vectors, dtype=np.float64)
    norms: Vectors = np.sqrt(np.einsum("ij,ij->i", rows, rows))
    np.maximum(norms, 1e-12, out=norms)
    rows /= norms[:, np.newaxis]
    return rows


def average_linkage(matrix: Vectors) -> list[tuple[int, int, float]]:
    """Merges of average-linkage clustering as (cluster, absorbed cluster, height).

    Nearest-neighbour chain: quadratic time, and the same tree as the textbook method
    because average linkage is reducible. The distance matrix is used as working memory and
    is changed.
    """
    count = matrix.shape[0]
    np.fill_diagonal(matrix, np.inf)
    sizes = np.ones(count)
    alive = np.ones(count, dtype=bool)
    merges: list[tuple[int, int, float]] = []
    chain: list[int] = []
    for _ in range(count - 1):
        if not chain:
            chain.append(int(np.flatnonzero(alive)[0]))
        while True:
            current = chain[-1]
            row = matrix[current]
            nearest = int(np.argmin(row))
            if len(chain) > 1 and row[chain[-2]] <= row[nearest]:
                break
            chain.append(nearest)
        absorbed = chain.pop()
        kept = chain.pop()
        merges.append((kept, absorbed, float(matrix[kept, absorbed])))
        merged = (sizes[kept] * matrix[kept] + sizes[absorbed] * matrix[absorbed]) / (sizes[kept] + sizes[absorbed])
        matrix[kept, :] = merged
        matrix[:, kept] = merged
        matrix[kept, kept] = np.inf
        matrix[absorbed, :] = np.inf
        matrix[:, absorbed] = np.inf
        alive[absorbed] = False
        sizes[kept] += sizes[absorbed]
    return merges


def _in_order_of_appearance(labels: Sequence[int] | Labels) -> Labels:
    values = np.asarray(labels, dtype=np.int64)
    _, first, inverse = np.unique(values, return_index=True, return_inverse=True)
    rank = np.empty(len(first), dtype=np.int64)
    rank[np.argsort(first)] = np.arange(len(first))
    numbered: Labels = rank[inverse]
    return numbered


def _cut(count: int, merges: Sequence[tuple[int, int, float]], threshold: float) -> Labels:
    parent = list(range(count))

    def root(index: int) -> int:
        while parent[index] != index:
            parent[index] = parent[parent[index]]
            index = parent[index]
        return index

    for kept, absorbed, height in merges:
        if height <= threshold:
            parent[root(absorbed)] = root(kept)
    return _in_order_of_appearance([root(index) for index in range(count)])


def _absorb_small_clusters(vectors: Vectors, labels: Labels, min_windows: int) -> Labels:
    sizes = np.bincount(labels)
    large = [label for label in range(len(sizes)) if sizes[label] >= min_windows]
    if not large or len(large) == len(sizes):
        return labels
    centroids = np.stack([vectors[labels == label].mean(axis=0) for label in large])
    result = labels.copy()
    for index, label in enumerate(labels):
        if sizes[label] < min_windows:
            result[index] = large[int(np.argmax(centroids @ vectors[index]))]
    return _in_order_of_appearance(result.tolist())


def cluster_speakers(vectors: NDArray[np.floating], rules: SpeakerRules) -> Labels:
    """One speaker number per vector, numbered in order of first appearance.

    Average linkage on cosine distance, cut at `rules.threshold`. A cluster with fewer than
    `min_cluster_windows` vectors joins the larger cluster with the nearest centroid. Above
    `max_cluster_windows` vectors, an evenly spaced subset is clustered and every other
    vector takes the speaker with the nearest centroid.

    Memory beyond `vectors`: one square matrix of the clustered subset, and one batch of
    `ASSIGN_BATCH` vectors.
    """
    count = vectors.shape[0]
    if count == 0:
        return np.zeros(0, dtype=np.int64)
    if count == 1:
        return np.zeros(1, dtype=np.int64)

    stride = -(-count // rules.max_cluster_windows)
    subset = unit_rows(vectors[::stride])
    distances: Vectors = subset @ subset.T
    np.subtract(1.0, distances, out=distances)
    np.clip(distances, 0.0, 2.0, out=distances)
    labels = _cut(len(subset), average_linkage(distances), rules.threshold)
    del distances
    labels = _absorb_small_clusters(subset, labels, rules.min_cluster_windows)
    if stride == 1:
        return labels

    centroids = np.stack([subset[labels == label].mean(axis=0) for label in range(int(labels.max()) + 1)])
    nearest = np.empty(count, dtype=np.int64)
    for start in range(0, count, ASSIGN_BATCH):
        batch = unit_rows(vectors[start : start + ASSIGN_BATCH])
        nearest[start : start + ASSIGN_BATCH] = np.argmax(batch @ centroids.T, axis=1)
    return _in_order_of_appearance(nearest)


def speaker_turns(windows: Sequence[Window], labels: Sequence[int]) -> dict[int, list[Turn]]:
    """Turns per segment. Inside a speech span, each moment belongs to the window with the nearest centre."""
    spans: dict[tuple[int, int], list[tuple[Window, int]]] = defaultdict(list)
    for window, label in zip(windows, labels, strict=True):
        spans[(window.segment, window.span)].append((window, int(label)))

    turns: dict[int, list[Turn]] = defaultdict(list)
    for (segment, _), members in sorted(spans.items()):
        members.sort(key=lambda member: (member[0].start + member[0].end, member[0].start))
        for index, (window, label) in enumerate(members):
            start = window.start
            end = window.end
            if index > 0:
                previous = members[index - 1][0]
                start = (previous.start + previous.end + window.start + window.end) // 4
            if index < len(members) - 1:
                following = members[index + 1][0]
                end = (following.start + following.end + window.start + window.end) // 4
            if end <= start:
                continue
            segment_turns = turns[segment]
            if segment_turns and segment_turns[-1].speaker == label and segment_turns[-1].end == start:
                segment_turns[-1] = Turn(segment_turns[-1].start, end, label)
            else:
                segment_turns.append(Turn(start, end, label))
    for segment_turns in turns.values():
        segment_turns.sort(key=lambda turn: (turn.start, turn.end))
    return dict(turns)


def speaker_of(turns: Sequence[Turn], start: int, end: int) -> int | None:
    """The speaker with the most time between `start` and `end`, or `None` when no turn overlaps."""
    overlap: dict[int, int] = defaultdict(int)
    for turn in turns:
        shared = min(turn.end, end) - max(turn.start, start)
        if shared > 0:
            overlap[turn.speaker] += shared
    if not overlap:
        return None
    return min(overlap, key=lambda speaker: (-overlap[speaker], speaker))
