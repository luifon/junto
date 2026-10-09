#!/usr/bin/env python3
"""Checks the files of the main recorded call against what the controller did.

Reads out/<room>/controller.jsonl, the participants' logs and egress-final.json,
measures tone levels per phase in the composite MP4 and in every track file,
extracts one frame per phase and measures the timing markers. Prints JSON.
"""
import json
import subprocess
import sys
from pathlib import Path

import numpy as np

import tones

ROOT = Path(__file__).resolve().parent.parent
TONES = {"alice/microphone": 440, "bob/microphone": 620, "carol/microphone": 790,
         "alice/screen_share_audio": 1000, "bob/screen_share_audio": 1370, "carol/screen_share_audio": 1730}
FREQS = sorted(TONES.values())
# A tone counts as present when its median level over 0.1 s windows is above
# PRESENT_DBFS and as absent when the median is below ABSENT_DBFS. The 95th
# percentile is reported too; the on and off edges of the time markers are
# clicks that raise it for every frequency.
PRESENT_DBFS = -20.0
ABSENT_DBFS = -60.0
WINDOW = 0.1


def read_jsonl(path):
    return [json.loads(line) for line in path.read_text().splitlines() if line.startswith("{")]


def window_levels(samples, freq):
    size = int(WINDOW * tones.RATE)
    count = len(samples) // size
    blocks = samples[:count * size].reshape(count, size)
    phase = np.exp(-2j * np.pi * freq * np.arange(size) / tones.RATE) * np.hanning(size)
    amplitude = 4.0 * np.abs(blocks @ phase) / size
    return 20.0 * np.log10(np.maximum(amplitude, 1e-9))


def phase_levels(levels, start, end):
    result = {}
    for freq in FREQS:
        part = levels[freq][int(start / WINDOW):int(end / WINDOW)]
        result[freq] = {"median": round(float(np.median(part)), 1), "p95": round(float(np.percentile(part, 95)), 1),
                        "p05": round(float(np.percentile(part, 5)), 1)}
    return result


def verdict(measured, expected_present):
    problems = []
    for freq in FREQS:
        if freq in expected_present and measured[freq]["median"] < PRESENT_DBFS:
            problems.append(f"{freq} Hz missing")
        if freq not in expected_present and measured[freq]["median"] > ABSENT_DBFS:
            problems.append(f"{freq} Hz present")
    return "ok" if not problems else "; ".join(problems)


def marker_errors(path, freq, file_start, period=10.0):
    """Offsets between where a marker gap is in the file and when it happened (seconds)."""
    samples = tones.decode(str(path), 0, 0)
    n = len(samples)
    baseband = samples * np.exp(-2j * np.pi * freq * np.arange(n) / tones.RATE)
    width = int(0.02 * tones.RATE)
    envelope = 2.0 * np.abs(np.convolve(baseband, np.ones(width) / width, mode="same"))
    on = envelope > 0.5 * np.percentile(envelope, 90)
    changes = np.flatnonzero(on[1:] != on[:-1]) + 1
    errors = {}
    for a, b in zip(changes[:-1], changes[1:]):
        gap = (b - a) / tones.RATE
        if not on[a] and 0.25 < gap < 0.35:
            wall = file_start + a / tones.RATE
            marker = round(wall / period) * period
            errors[int(marker)] = round(wall - marker, 4)
    return errors


def video_marker_errors(path, file_start, period=10.0):
    """Offsets of the white video marker in the first camera tile of a 1280x720 composite."""
    result = subprocess.run(
        ["ffmpeg", "-v", "error", "-nostdin", "-i", str(path), "-an", "-vf",
         "crop=110:30:1150:105,signalstats,metadata=mode=print:key=lavfi.signalstats.YAVG:file=-",
         "-f", "null", "-"], capture_output=True, text=True, check=True)
    times, values = [], []
    for line in result.stdout.splitlines():
        if "pts_time:" in line:
            times.append(float(line.split("pts_time:")[1].split()[0]))
        elif "YAVG=" in line:
            values.append(float(line.split("=")[1]))
    errors = {}
    for i in range(1, min(len(times), len(values))):
        if values[i] > 200 and values[i - 1] <= 200:
            wall = file_start + times[i]
            marker = round(wall / period) * period
            errors[int(marker)] = round(wall - marker, 4)
    return errors


def spread(values):
    values = sorted(values)
    if not values:
        return None
    median = float(np.median(values))
    usual = [v for v in values if abs(v - median) <= 0.3]
    return {"count": len(values), "median": round(median, 4), "min_usual": usual[0], "max_usual": usual[-1],
            "outliers": [v for v in values if abs(v - median) > 0.3]}


def main():
    room = sys.argv[1] if len(sys.argv) > 1 else "call-a"
    out = ROOT / "out" / room
    log = read_jsonl(out / "controller.jsonl")
    run_start_ns = (log[0]["unix_ms"] - 1000) * 1_000_000
    final = [i for i in json.loads((out / "egress-final.json").read_text())["items"]
             if int(i["started_at"]) >= run_start_ns]
    people = {name: read_jsonl(out / f"{name}.jsonl") for name in ("alice", "bob", "carol")}

    composite = [i for i in final if "room_composite" in i][-1]
    cfile = composite["file_results"][0]
    c0 = int(cfile["started_at"]) / 1e9
    cpath = ROOT / cfile["filename"].lstrip("/")
    in_file = lambda unix_ms: unix_ms / 1000.0 - c0

    def api_time(method, nth=0):
        return [e for e in log if e["kind"] == "api" and e["method"] == method][nth]["unix_ms"]

    def person_event(name, predicate, nth=0):
        return [e for e in people[name] if predicate(e)][nth]["unix_ms"]

    is_share_audio = lambda e: e.get("event") == "published" and e.get("source") == "screen_share_audio"
    carol_share = person_event("carol", is_share_audio)
    bob_back = person_event("bob", is_share_audio, 1)
    bob_left = person_event("bob", lambda e: e.get("event") == "left")
    mute = person_event("alice", lambda e: e.get("action") == "Mute")
    unmute = person_event("alice", lambda e: e.get("action") == "Unmute")
    stop = api_time("StopEgress")
    marks = [
        ("two shares, no instruction (bob's started last)", 3000 + c0 * 1000, carol_share - 1500, {440, 620, 1370}),
        ("third share started (carol)", carol_share + 3000, api_time("UpdateLayout") - 1000, {440, 620, 790, 1730}),
        ("UpdateLayout main:alice", api_time("UpdateLayout") + 3000, api_time("UpdateRoomMetadata") - 1000, {440, 620, 790, 1000}),
        ("room metadata main_share=bob", api_time("UpdateRoomMetadata") + 3000, api_time("SendData") - 1000, {440, 620, 790, 1370}),
        ("data message main_share=alice", api_time("SendData") + 3000, api_time("SendData", 1) - 1000, {440, 620, 790, 1000}),
        ("instruction cleared (latest share: carol)", api_time("SendData", 1) + 3000, bob_left - 1000, {440, 620, 790, 1730}),
        ("bob reconnected (his new share is the latest)", bob_back + 4000, mute - 1000, {440, 620, 790, 1370}),
        ("alice muted", mute + 3000, unmute - 1000, {620, 790, 1370}),
        ("alice unmuted", unmute + 3000, stop - 2000, {440, 620, 790, 1370}),
    ]

    samples = tones.decode(str(cpath), 0, 0)
    levels = {freq: window_levels(samples, freq) for freq in FREQS}
    frames = out / "frames"
    frames.mkdir(exist_ok=True)
    phases = []
    for index, (name, start_ms, end_ms, present) in enumerate(marks, 1):
        start, end = in_file(start_ms), in_file(end_ms)
        measured = phase_levels(levels, start, end)
        frame = frames / f"phase{index}.png"
        subprocess.run(["ffmpeg", "-v", "error", "-y", "-ss", f"{(start + end) / 2:.2f}", "-i", str(cpath),
                        "-frames:v", "1", "-vf", "scale=960:-1", str(frame)], check=True)
        phases.append({"phase": index, "name": name, "file_seconds": [round(start, 1), round(end, 1)],
                       "expected_present_hz": sorted(present), "levels_dbfs": measured,
                       "verdict": verdict(measured, present), "frame": str(frame.relative_to(ROOT))})

    def first_window_above(freq, from_ms):
        start = int(in_file(from_ms) / WINDOW)
        above = np.flatnonzero(levels[freq][start:] > PRESENT_DBFS)
        return round(float(above[0]) * WINDOW, 1) if len(above) else None

    switches = {
        "UpdateLayout -> 1000 Hz audible": first_window_above(1000, api_time("UpdateLayout")),
        "UpdateRoomMetadata -> 1370 Hz audible": first_window_above(1370, api_time("UpdateRoomMetadata")),
        "SendData -> 1000 Hz audible": first_window_above(1000, api_time("SendData")),
        "carol's share published -> 1730 Hz audible": first_window_above(1730, carol_share),
    }

    tracks = []
    track_markers = {}
    for item in final:
        if "track" not in item:
            continue
        info = item["file_results"][0]
        path = ROOT / info["filename"].lstrip("/")
        identity, source = path.name.split("-")[0], path.name.split("-")[1]
        expected = TONES[f"{identity}/{source}"]
        decoded = tones.decode(str(path), 0, 0)
        medians = {freq: round(float(np.median(window_levels(decoded, freq))), 1) for freq in FREQS}
        strongest_other = max(v for f, v in medians.items() if f != expected)
        probe = json.loads(subprocess.run(
            ["ffprobe", "-v", "error", "-show_entries", "format=format_name,duration,size:stream=codec_name,sample_rate,channels",
             "-of", "json", str(path)], capture_output=True, text=True).stdout)
        start, end = int(info["started_at"]) / 1e9, int(info["ended_at"]) / 1e9
        tracks.append({
            "file": path.name, "egress_id": item["egress_id"], "track_id": item["track"]["track_id"],
            "status": item["status"], "details": item["details"],
            "container": probe["format"]["format_name"], "codec": probe["streams"][0]["codec_name"],
            "channels": probe["streams"][0]["channels"], "sample_rate": probe["streams"][0]["sample_rate"],
            "bytes": int(probe["format"]["size"]), "media_seconds": round(float(probe["format"]["duration"]), 2),
            "kbit_s": round(int(probe["format"]["size"]) * 8 / float(probe["format"]["duration"]) / 1000, 1),
            "egress_started_at": int(item["started_at"]) / 1e9, "egress_ended_at": int(item["ended_at"]) / 1e9,
            "file_started_at": start, "file_ended_at": end, "wall_seconds": round(end - start, 2),
            "expected_hz": expected, "expected_tone_dbfs": medians[expected], "strongest_other_dbfs": strongest_other,
            "verdict": "ok" if medians[expected] > PRESENT_DBFS and strongest_other < ABSENT_DBFS else "wrong tone",
        })
        if source == "microphone":
            for marker, error in marker_errors(path, expected, start).items():
                track_markers[(expected, marker)] = error

    alignment = []
    for freq in (440, 620, 790):
        for marker, error in marker_errors(cpath, freq, c0).items():
            if (freq, marker) in track_markers:
                alignment.append({"tone_hz": freq, "marker_unix": marker, "composite_error_s": error,
                                  "track_error_s": track_markers[(freq, marker)],
                                  "composite_minus_track_s": round(error - track_markers[(freq, marker)], 4)})
    video = video_marker_errors(cpath, c0)
    composite_audio = {}
    for a in alignment:
        composite_audio.setdefault(a["marker_unix"], []).append(a["composite_error_s"])
    video_minus_audio = [round(video[m] - float(np.median(v)), 4) for m, v in composite_audio.items() if m in video]
    summary = {
        "track_file_vs_wall_clock_s": spread(list(track_markers.values())),
        "composite_audio_vs_wall_clock_s": spread([a["composite_error_s"] for a in alignment]),
        "composite_audio_minus_track_s": spread([a["composite_minus_track_s"] for a in alignment]),
        "composite_video_vs_wall_clock_s": spread(list(video.values())),
        "composite_video_minus_composite_audio_s": spread(video_minus_audio),
    }
    print(json.dumps({"composite": {"file": cpath.name, "file_started_at": c0, "egress": composite["egress_id"]},
                      "phases": phases, "switch_file_seconds_after_call": switches, "tracks": tracks,
                      "alignment_summary": summary, "alignment": alignment}, indent=1))


if __name__ == "__main__":
    main()
