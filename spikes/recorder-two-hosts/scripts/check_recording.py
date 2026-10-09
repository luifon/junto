#!/usr/bin/env python3
"""A cheap check of a finished recording for missing sound or picture.

  check_recording.py FILE [--expect-seconds N] [--fps 30] [--audio-only]
                          [--reference TRACKFILE:OFFSET_SECONDS ...]

FILE is an MP4, an Ogg file or a segment playlist. Three passes:

1. Packet timestamps (ffprobe, no decoding): every stream is present, starts
   near zero, is as long as the file, and has no hole between packets.
2. Sound (audio decoding only): the share of digital silence, and the ranges.
   With --reference, a range that is silent in FILE while a reference track
   file has sound at the same time is reported as missing sound.
3. Picture (decoding of key frames only): the share of key frames that are
   black or of one colour.

Prints JSON with "ok" and the list of problems. Exit status 1 when not ok.
"""
import argparse
import json
import re
import subprocess
import sys
import time

AUDIO_HOLE = 0.25
VIDEO_HOLE = 1.0
SILENCE_DB = -70
SILENCE_MIN = 1.0


def packets(path):
    result = subprocess.run(["ffprobe", "-v", "error", "-show_entries", "packet=stream_index,pts_time,duration_time:stream=index,codec_type,codec_name",
                             "-of", "json", path], capture_output=True, text=True)
    data = json.loads(result.stdout or "{}")
    streams = {s["index"]: {"type": s["codec_type"], "codec": s.get("codec_name"), "pts": []} for s in data.get("streams", [])}
    for packet in data.get("packets", []):
        if "pts_time" in packet and packet["stream_index"] in streams:
            streams[packet["stream_index"]]["pts"].append(float(packet["pts_time"]))
    return streams, result.stderr.strip().splitlines()[:2]


def timing(stream, hole):
    pts = sorted(stream["pts"])
    if not pts:
        return {"type": stream["type"], "codec": stream["codec"], "packets": 0}
    holes = [(round(a, 3), round(b - a, 3)) for a, b in zip(pts[:-1], pts[1:]) if b - a > hole]
    return {"type": stream["type"], "codec": stream["codec"], "packets": len(pts), "first": round(pts[0], 3),
            "last": round(pts[-1], 3), "holes": len(holes), "hole_seconds": round(sum(h[1] for h in holes), 3),
            "first_holes": holes[:5]}


def silence(path):
    result = subprocess.run(["ffmpeg", "-v", "info", "-nostdin", "-i", path, "-vn", "-af",
                             f"silencedetect=noise={SILENCE_DB}dB:d={SILENCE_MIN},volumedetect", "-f", "null", "-"],
                            capture_output=True, text=True)
    starts = [float(x) for x in re.findall(r"silence_start: (-?[0-9.]+)", result.stderr)]
    ends = [float(x) for x in re.findall(r"silence_end: ([0-9.]+)", result.stderr)]
    mean = re.findall(r"mean_volume: (-?[0-9.]+|-inf) dB", result.stderr)
    peak = re.findall(r"max_volume: (-?[0-9.]+|-inf) dB", result.stderr)
    return list(zip(starts, ends + [None] * (len(starts) - len(ends)))), (mean or [None])[0], (peak or [None])[0]


def key_frames(path):
    result = subprocess.run(["ffmpeg", "-v", "error", "-nostdin", "-skip_frame", "nokey", "-i", path, "-an", "-vsync", "passthrough",
                             "-vf", "scale=160:90,signalstats,metadata=mode=print:file=-", "-f", "null", "-"],
                            capture_output=True, text=True)
    frames, current = [], {}
    for line in result.stdout.splitlines():
        if "pts_time:" in line:
            if current:
                frames.append(current)
            current = {"t": float(line.split("pts_time:")[1].split()[0])}
        for key in ("YAVG", "YMIN", "YMAX"):
            if f"lavfi.signalstats.{key}=" in line:
                current[key] = float(line.split("=")[1])
    if current:
        frames.append(current)
    return frames


def overlap(a, b):
    return max(0.0, min(a[1], b[1]) - max(a[0], b[0]))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("file")
    parser.add_argument("--expect-seconds", type=float, default=0.0)
    parser.add_argument("--fps", type=float, default=30.0)
    parser.add_argument("--audio-only", action="store_true")
    parser.add_argument("--reference", action="append", default=[])
    args = parser.parse_args()
    began = time.time()
    problems = []
    report = {"file": args.file}

    streams, errors = packets(args.file)
    if errors:
        problems.append(f"ffprobe: {errors[0]}")
    audio = [timing(s, AUDIO_HOLE) for s in streams.values() if s["type"] == "audio"]
    video = [timing(s, VIDEO_HOLE) for s in streams.values() if s["type"] == "video"]
    report["audio"], report["video"] = audio, video
    if not audio or not audio[0].get("packets"):
        problems.append("no audio stream")
    if not args.audio_only and (not video or not video[0].get("packets")):
        problems.append("no video stream")
    present = [s for s in audio + video if s.get("packets")]
    origin = min((s["first"] for s in present), default=0.0)
    length = max((s["last"] for s in present), default=0.0) - origin
    report["seconds"] = round(length, 3)
    for stream in present:
        if stream["holes"]:
            problems.append(f"{stream['type']}: {stream['holes']} hole(s) in the timestamps, {stream['hole_seconds']} s in total, first at {stream['first_holes'][0][0] - origin:.1f} s")
        if stream["first"] - origin > 1.0:
            problems.append(f"{stream['type']} begins {stream['first'] - origin:.1f} s after the file")
        if length - (stream["last"] - origin) > 1.0:
            problems.append(f"{stream['type']} ends {length - (stream['last'] - origin):.1f} s before the file")
    if video and video[0].get("packets") and length > 0:
        rate = video[0]["packets"] / length
        report["video_frames_per_second"] = round(rate, 2)
        if rate < 0.9 * args.fps:
            problems.append(f"video has {rate:.1f} frames per second, expected {args.fps:g}")
    if args.expect_seconds and abs(length - args.expect_seconds) > 3.0:
        problems.append(f"length {length:.1f} s, expected {args.expect_seconds:.1f} s")
    report["pass1_seconds"] = round(time.time() - began, 2)

    if audio and audio[0].get("packets"):
        began2 = time.time()
        ranges, mean, peak = silence(args.file)
        ranges = [(a, b if b is not None else length) for a, b in ranges]
        silent = sum(b - a for a, b in ranges)
        report["sound"] = {"mean_volume_db": mean, "max_volume_db": peak, "silent_seconds": round(silent, 2),
                           "silent_ranges": [(round(a, 2), round(b, 2)) for a, b in ranges][:10]}
        if peak in ("-inf", None) or float(peak) < -60:
            problems.append("sound is digital silence for the whole file")
        for reference in args.reference:
            ref_path, _, offset = reference.rpartition(":")
            offset = float(offset)
            ref_ranges, _, ref_peak = silence(ref_path)
            ref_packets, _ = packets(ref_path)
            ref_pts = sorted(next(iter(ref_packets.values()))["pts"]) if ref_packets else []
            if not ref_pts or ref_peak in ("-inf", None):
                continue
            ref_span = (ref_pts[0] + offset, ref_pts[-1] + offset)
            ref_silent = [(a + offset, (b if b is not None else ref_pts[-1]) + offset) for a, b in ref_ranges]
            missing = 0.0
            for a, b in ranges:
                inside = overlap((a, b), ref_span)
                covered = sum(overlap((max(a, ref_span[0]), min(b, ref_span[1])), r) for r in ref_silent) if inside else 0.0
                missing += max(0.0, inside - covered)
            report["sound"].setdefault("missing_against_reference", {})[ref_path.rsplit("/", 1)[-1]] = round(missing, 2)
            if missing > 1.0:
                problems.append(f"{missing:.1f} s without sound while {ref_path.rsplit('/', 1)[-1]} has sound")
        report["pass2_seconds"] = round(time.time() - began2, 2)

    if not args.audio_only and video and video[0].get("packets"):
        began3 = time.time()
        frames = key_frames(args.file)
        dark = [f for f in frames if f.get("YAVG", 255) < 24]
        flat = [f for f in frames if f.get("YMAX", 255) - f.get("YMIN", 0) < 16]
        report["picture"] = {"key_frames": len(frames), "dark": len(dark), "one_colour": len(flat),
                             "first_dark_at": round(dark[0]["t"] - origin, 1) if dark else None}
        if not frames:
            problems.append("no key frame could be decoded")
        elif len(dark) > 0.05 * len(frames) or len(flat) > 0.05 * len(frames):
            problems.append(f"picture: {len(dark)} dark and {len(flat)} one-colour key frames of {len(frames)}")
        report["pass3_seconds"] = round(time.time() - began3, 2)

    report["check_seconds"] = round(time.time() - began, 2)
    report["problems"] = problems
    report["ok"] = not problems
    print(json.dumps(report))
    sys.exit(0 if report["ok"] else 1)


if __name__ == "__main__":
    main()
