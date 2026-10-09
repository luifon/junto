#!/usr/bin/env python3
"""Measures sine tones in the first audio stream of a media file.

  tones.py levels FILE --freqs 440,620 [--start S] [--duration D] [--window W]
      Level of each frequency in dBFS (0 dBFS = full-scale sine), per window and overall.
  tones.py edges FILE --freq 440 [--start S] [--duration D]
      Times at which the tone turns off and on, to about one millisecond.

The file is decoded with ffmpeg. Decoding fills timestamp gaps with silence
(aresample async) so that times are the file's presentation times.
"""
import argparse
import json
import subprocess
import sys

import numpy as np

RATE = 48000


def decode(path, start, duration):
    command = ["ffmpeg", "-v", "error", "-nostdin"]
    if start:
        command += ["-ss", str(start)]
    command += ["-i", path]
    if duration:
        command += ["-t", str(duration)]
    command += ["-map", "0:a:0", "-ac", "1", "-af", f"aresample={RATE}:async=1:first_pts=0",
                "-f", "s16le", "-"]
    raw = subprocess.run(command, check=True, capture_output=True).stdout
    return np.frombuffer(raw, dtype=np.int16).astype(np.float64) / 32767.0


def level_dbfs(block, freq):
    n = len(block)
    window = np.hanning(n)
    phase = np.exp(-2j * np.pi * freq * np.arange(n) / RATE)
    amplitude = 4.0 * abs(np.sum(block * window * phase)) / n
    return 20.0 * np.log10(max(amplitude, 1e-9))


def levels(args):
    samples = decode(args.file, args.start, args.duration)
    freqs = [float(f) for f in args.freqs.split(",")]
    size = int(args.window * RATE)
    rows = []
    for offset in range(0, len(samples) - size + 1, size):
        block = samples[offset:offset + size]
        rows.append({"t": round(args.start + offset / RATE, 3),
                     **{f"{f:g}": round(level_dbfs(block, f), 1) for f in freqs}})
    overall = {f"{f:g}": round(float(np.median([r[f"{f:g}"] for r in rows])), 1) for f in freqs} if rows else {}
    result = {"file": args.file, "seconds": round(len(samples) / RATE, 3), "window": args.window,
              "median_dbfs": overall}
    if args.rows:
        result["rows"] = rows
    print(json.dumps(result))


def edges(args):
    samples = decode(args.file, args.start, args.duration)
    n = len(samples)
    baseband = samples * np.exp(-2j * np.pi * args.freq * np.arange(n) / RATE)
    width = int(0.02 * RATE)
    envelope = 2.0 * np.abs(np.convolve(baseband, np.ones(width) / width, mode="same"))
    threshold = 0.5 * np.percentile(envelope, 95)
    on = envelope > threshold
    changes = np.flatnonzero(on[1:] != on[:-1]) + 1
    found = [{"t": round(args.start + int(i) / RATE, 4), "to": "on" if on[i] else "off"} for i in changes]
    print(json.dumps({"file": args.file, "freq": args.freq, "seconds": round(n / RATE, 3),
                      "peak_dbfs": round(20 * np.log10(max(threshold * 2, 1e-9)), 1), "edges": found}))


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    for name in ("levels", "edges"):
        p = sub.add_parser(name)
        p.add_argument("file")
        p.add_argument("--start", type=float, default=0.0)
        p.add_argument("--duration", type=float, default=0.0)
    sub.choices["levels"].add_argument("--freqs", required=True)
    sub.choices["levels"].add_argument("--window", type=float, default=1.0)
    sub.choices["levels"].add_argument("--rows", action="store_true")
    sub.choices["edges"].add_argument("--freq", type=float, required=True)
    args = parser.parse_args()
    {"levels": levels, "edges": edges}[args.command](args)


if __name__ == "__main__":
    sys.exit(main())
