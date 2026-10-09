import time

import numpy as np
from scipy.cluster.hierarchy import fcluster, linkage
from scipy.optimize import linear_sum_assignment

STEP = 0.01
RATE = 16000


def diarize(audio, segmentation, embedding, clusters=-1, threshold=0.5, threads=8, provider="cpu"):
    import sherpa_onnx

    config = sherpa_onnx.OfflineSpeakerDiarizationConfig(
        segmentation=sherpa_onnx.OfflineSpeakerSegmentationModelConfig(
            pyannote=sherpa_onnx.OfflineSpeakerSegmentationPyannoteModelConfig(model=segmentation),
            num_threads=threads,
            provider=provider,
        ),
        embedding=sherpa_onnx.SpeakerEmbeddingExtractorConfig(
            model=embedding, num_threads=threads, provider=provider
        ),
        clustering=sherpa_onnx.FastClusteringConfig(num_clusters=clusters, threshold=threshold),
        min_duration_on=0.3,
        min_duration_off=0.5,
    )
    if not config.validate():
        raise RuntimeError("invalid diarization config")
    started = time.time()
    engine = sherpa_onnx.OfflineSpeakerDiarization(config)
    load_seconds = time.time() - started
    started = time.time()
    result = engine.process(audio).sort_by_start_time()
    seconds = time.time() - started
    turns = [(r.start, r.end, int(r.speaker)) for r in result]
    return turns, seconds, load_seconds


def grid(spans, labels, duration):
    frames = int(round(duration / STEP)) + 1
    matrix = np.zeros((frames, len(labels)), dtype=bool)
    for start, end, label in spans:
        matrix[int(round(start / STEP)) : int(round(end / STEP)), labels.index(label)] = True
    return matrix


def reference_spans(ref):
    return [(a, b, seg["speaker"]) for seg in ref["segments"] for a, b in seg["spans"]]


def error_rate(ref_spans, hyp_spans, duration, collar=0.0):
    ref_labels = sorted({label for _, _, label in ref_spans})
    hyp_labels = sorted({label for _, _, label in hyp_spans})
    ref = grid(ref_spans, ref_labels, duration)
    hyp = grid(hyp_spans, hyp_labels or [0], duration)
    keep = np.ones(ref.shape[0], dtype=bool)
    if collar:
        width = int(round(collar / STEP))
        for start, end, _ in ref_spans:
            for edge in (start, end):
                center = int(round(edge / STEP))
                keep[max(0, center - width) : center + width] = False
    ref = ref[keep]
    hyp = hyp[keep]
    overlap = ref.astype(int).T @ hyp.astype(int)
    rows, cols = linear_sum_assignment(-overlap)
    mapping = {hyp_labels[c] if hyp_labels else 0: ref_labels[r] for r, c in zip(rows, cols) if overlap[r, c] > 0}
    correct = np.zeros(ref.shape[0], dtype=int)
    for r, c in zip(rows, cols):
        correct += (ref[:, r] & hyp[:, c]).astype(int)
    n_ref = ref.sum(axis=1)
    n_hyp = hyp.sum(axis=1)
    total = int(n_ref.sum())
    missed = int(np.maximum(0, n_ref - n_hyp).sum())
    false_alarm = int(np.maximum(0, n_hyp - n_ref).sum())
    confusion = int((np.minimum(n_ref, n_hyp) - correct).sum())
    return {
        "der": (missed + false_alarm + confusion) / total,
        "missed": missed / total,
        "false_alarm": false_alarm / total,
        "confusion": confusion / total,
        "correct_share": int(correct.sum()) / total,
        "speakers_found": len(hyp_labels),
        "speakers_reference": len(ref_labels),
        "mapping": mapping,
    }


def speaker_windows(audio, window=2.5, hop=1.25):
    from faster_whisper.vad import VadOptions, get_speech_timestamps

    stamps = get_speech_timestamps(audio, VadOptions(min_silence_duration_ms=300, speech_pad_ms=200))
    size = int(window * RATE)
    hop = int(hop * RATE)
    out = []
    for stamp in stamps:
        start, end = stamp["start"], stamp["end"]
        if end - start <= size:
            out.append((start, end))
            continue
        position = start
        while position + size < end:
            out.append((position, position + size))
            position += hop
        out.append((max(start, end - size), end))
    return out


def embed_windows(extractor, audio, spans):
    rows = []
    for start, end in spans:
        stream = extractor.create_stream()
        stream.accept_waveform(RATE, audio[start:end])
        stream.input_finished()
        vector = np.array(extractor.compute(stream), dtype=np.float32)
        rows.append(vector / (np.linalg.norm(vector) + 1e-9))
    return np.stack(rows)


def cluster_windows(vectors, threshold):
    if len(vectors) == 1:
        return np.zeros(1, dtype=int)
    tree = linkage(vectors, method="average", metric="cosine")
    labels = fcluster(tree, t=threshold, criterion="distance") - 1
    lengths = np.bincount(labels)
    keep = [label for label in range(len(lengths)) if lengths[label] >= 3]
    if keep and len(keep) < len(lengths):
        centroids = np.stack([vectors[labels == label].mean(axis=0) for label in keep])
        for index, label in enumerate(labels):
            if label not in keep:
                labels[index] = keep[int(np.argmax(centroids @ vectors[index]))]
    return labels


def turns_from_windows(spans, labels):
    step = RATE // 100
    starts = np.array([start for start, _ in spans])
    ends = np.array([end for _, end in spans])
    centers = (starts + ends) / 2
    order = np.argsort(centers)
    first = int(starts.min())
    samples = np.arange(first, int(ends.max()), step)
    position = np.searchsorted(centers[order], samples)
    best = np.full(len(samples), -1)
    distance = np.full(len(samples), np.inf)
    for shift in (-2, -1, 0, 1):
        candidate = np.clip(position + shift, 0, len(order) - 1)
        index = order[candidate]
        inside = (starts[index] <= samples) & (samples < ends[index])
        gap = np.where(inside, np.abs(centers[index] - samples), np.inf)
        better = gap < distance
        best[better] = index[better]
        distance[better] = gap[better]
    turns = []
    last = None
    for sample, index in zip(samples, best):
        if index < 0:
            last = None
            continue
        label = int(labels[index])
        moment = sample / RATE
        if last and last[2] == label:
            last[1] = moment + 0.01
        else:
            last = [moment, moment + 0.01, label]
            turns.append(last)
    return [(a, b, c) for a, b, c in turns]


def load_extractor(embedding, threads=8, provider="cpu"):
    import sherpa_onnx

    return sherpa_onnx.SpeakerEmbeddingExtractor(
        sherpa_onnx.SpeakerEmbeddingExtractorConfig(model=embedding, num_threads=threads, provider=provider)
    )


def diarize_simple(audio, embedding, threshold=0.6, window=2.5, hop=1.25, threads=8):
    started = time.time()
    extractor = load_extractor(embedding, threads)
    load_seconds = time.time() - started
    started = time.time()
    pieces = speaker_windows(audio, window, hop)
    if not pieces:
        return [], time.time() - started, load_seconds
    vectors = embed_windows(extractor, audio, pieces)
    turns = turns_from_windows(pieces, cluster_windows(vectors, threshold))
    return turns, time.time() - started, load_seconds
