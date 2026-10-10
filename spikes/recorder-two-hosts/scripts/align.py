#!/usr/bin/env python3
"""Places a composite recording on the timeline of the track files by its sound.

  align.py ROOM

For each composite of the room: decodes its sound and the sound of one
microphone track file, computes the loudness envelope of both (2 ms steps) and
finds the shift at which the two envelopes agree best. The shift gives the
wall-clock time of the composite's first sample on the track file's timeline:

    composite_start = track_started_at + (track position - composite position of the same sound)

Compares the result with the time markers of the test signal (analysis.json)
and with the start time that Egress reports for the composite.
"""
import json
import subprocess
import sys
from pathlib import Path

import numpy as np

ROOT = Path("/work")
RATE = 16000
STEP = 32
WINDOW_SECONDS = 60.0
SEARCH_SECONDS = 6.0


def decode(path, start, seconds):
    command = ["ffmpeg", "-v", "error", "-nostdin", "-ss", f"{start:.3f}", "-i", str(path), "-t", f"{seconds:.3f}", "-map", "0:a:0",
               "-ac", "1", "-af", f"aresample={RATE}:async=1", "-f", "s16le", "-"]
    raw = subprocess.run(command, check=True, capture_output=True).stdout
    return np.frombuffer(raw, dtype=np.int16).astype(np.float64) / 32767.0


def envelope(samples):
    count = len(samples) // STEP
    frames = samples[:count * STEP].reshape(count, STEP)
    level = np.sqrt((frames ** 2).mean(axis=1))
    smooth = np.convolve(level, np.ones(5) / 5.0, mode="same")
    return smooth - smooth.mean()


def best_shift(reference, other):
    """Seconds by which `other` must be delayed to match `reference`, searched within +-SEARCH_SECONDS."""
    n = 1 << int(np.ceil(np.log2(len(reference) + len(other))))
    spectrum = np.fft.rfft(reference, n) * np.conj(np.fft.rfft(other, n))
    correlation = np.fft.irfft(spectrum, n)
    limit = int(SEARCH_SECONDS * RATE / STEP)
    lags = np.concatenate([np.arange(0, limit + 1), np.arange(-limit, 0)])
    values = np.concatenate([correlation[:limit + 1], correlation[-limit:]])
    index = int(np.argmax(values))
    peak = values[index]
    shift = float(lags[index])
    if 0 < index < len(values) - 1 and lags[index - 1] == lags[index] - 1 and lags[index + 1] == lags[index] + 1:
        a, b, c = values[index - 1], values[index], values[index + 1]
        if (a - 2 * b + c) != 0:
            shift += 0.5 * (a - c) / (a - 2 * b + c)
    norm = np.sqrt((reference ** 2).sum() * (other ** 2).sum())
    return shift * STEP / RATE, float(peak / norm) if norm else 0.0


def main():
    room = sys.argv[1]
    out = ROOT / "out" / room
    analysis = json.loads((out / "analysis.json").read_text())
    tracks = [t for t in analysis["tracks"] if t["track"].endswith("/microphone") and "file" in t]
    results = []
    for composite in analysis["composites"]:
        for kind in ("mp4", "segments"):
            part = composite.get(kind)
            if not part or "missing" in part:
                continue
            composite_start = int(part["reported_started_at"]) / 1e9
            composite_bias = part["audio_marker_spread"]["median"]
            for track in tracks[:2]:
                track_start = int(track["reported_started_at"]) / 1e9
                track_bias = track["marker_spread"]["median"]
                track_path = out / "tracks" / track["file"]
                # A window that both files cover, by the reported start times.
                window_wall = max(composite_start, track_start) + 8.0
                a = decode(part["path"], window_wall - composite_start, WINDOW_SECONDS)
                b = decode(track_path, window_wall - track_start, WINDOW_SECONDS)
                size = min(len(a), len(b))
                shift, score = best_shift(envelope(a[:size]), envelope(b[:size]))
                # shift > 0: the sound is `shift` seconds later in the composite than the reported times say.
                estimated_start = composite_start - shift
                true_start = composite_start - composite_bias
                results.append({
                    "composite": kind, "egress_id": composite["egress_id"], "against": track["track"],
                    "shift_seconds": round(shift, 4), "correlation": round(score, 3),
                    "composite_late_by_markers": round(composite_bias, 4), "track_late_by_markers": round(track_bias, 4),
                    "expected_shift": round(composite_bias - track_bias, 4),
                    "error_of_method_ms": round((shift - (composite_bias - track_bias)) * 1000, 1),
                    "composite_start_error_on_wall_clock_ms": round((estimated_start - true_start) * 1000, 1),
                })
    # The same per 20 s window, to follow a shift that changes inside one recording.
    windows = []
    for composite in analysis["composites"]:
        part = composite.get("mp4") or composite.get("segments")
        if not part or "missing" in part or not tracks:
            continue
        track = tracks[0]
        composite_start = int(part["reported_started_at"]) / 1e9
        track_start = int(track["reported_started_at"]) / 1e9
        track_bias = track["marker_spread"]["median"]
        markers = {int(k): v for k, v in part["audio_marker_errors"].get(track["track"], {}).items()}
        wall = max(composite_start, track_start) + 4.0
        end = min(composite_start + part["audio_seconds"], track_start + track["media_seconds"]) - 24.0
        while wall < end:
            a = decode(part["path"], wall - composite_start, 20.0)
            b = decode(out / "tracks" / track["file"], wall - track_start, 20.0)
            size = min(len(a), len(b))
            shift, score = best_shift(envelope(a[:size]), envelope(b[:size]))
            inside = [v for k, v in markers.items() if wall <= k <= wall + 20.0]
            expected = (sum(inside) / len(inside) - track_bias) if inside else None
            windows.append({"window_at": round(wall - composite_start, 1), "shift_seconds": round(shift, 4),
                            "by_markers": round(expected, 4) if expected is not None else None,
                            "error_ms": round((shift - expected) * 1000, 1) if expected is not None else None})
            wall += 20.0
    (out / "align.json").write_text(json.dumps({"whole": results, "windows": windows}, indent=1))
    errors = [w["error_ms"] for w in windows if w["error_ms"] is not None]
    print("windows of 20 s:", [(w["window_at"], w["shift_seconds"], w["error_ms"]) for w in windows])
    if errors:
        print("window errors ms: min", min(errors), "max", max(errors))
    for row in results:
        print(json.dumps(row))


if __name__ == "__main__":
    main()
