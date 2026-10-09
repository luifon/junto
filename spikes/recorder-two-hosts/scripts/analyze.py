#!/usr/bin/env python3
"""Checks the files of one recorded test call against what the controller did.

  analyze.py ROOM

Reads out/ROOM/controller.jsonl, phases.json (written by the call script) and
egress-final.json. For the composite (MP4 and segment playlist) it measures tone
levels per phase, extracts one frame per phase and measures the time markers.
For every track file it measures the tone and the time markers. Writes
out/ROOM/analysis.json and prints a summary.
"""
import json
import re
import subprocess
import sys
import time
from pathlib import Path

import numpy as np

import tones

ROOT = Path("/work")
TONES = {"alice/microphone": 440, "bob/microphone": 620, "carol/microphone": 790,
         "alice/screen_share_audio": 1000, "bob/screen_share_audio": 1370, "carol/screen_share_audio": 1730}
FREQS = sorted(TONES.values())
PRESENT_DBFS = -20.0
ABSENT_DBFS = -60.0
WINDOW = 0.1
PERIOD = 10.0


def read_jsonl(path):
    return [json.loads(line) for line in path.read_text().splitlines() if line.startswith("{")]


def local(path):
    return ROOT / path.lstrip("/")


def probe(path):
    result = subprocess.run(["ffprobe", "-v", "error", "-show_entries",
                             "format=duration,size,start_time:stream=codec_name,codec_type,width,height,channels,sample_rate,avg_frame_rate,bit_rate,duration,start_time",
                             "-of", "json", str(path)], capture_output=True, text=True)
    info = json.loads(result.stdout or "{}")
    return {"ok": result.returncode == 0 and bool(info.get("streams")), "format": info.get("format", {}),
            "streams": info.get("streams", []), "error": result.stderr.strip().splitlines()[:2]}


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
        part = levels[freq][max(0, int(start / WINDOW)):int(end / WINDOW)]
        if len(part) == 0:
            result[freq] = None
            continue
        result[freq] = round(float(np.median(part)), 1)
    return result


def verdict(measured, expected_present):
    problems = []
    for freq in FREQS:
        if measured[freq] is None:
            return "phase is outside the file"
        if freq in expected_present and measured[freq] < PRESENT_DBFS:
            problems.append(f"{freq} Hz missing")
        if freq not in expected_present and measured[freq] > ABSENT_DBFS:
            problems.append(f"{freq} Hz present")
    return "ok" if not problems else "; ".join(problems)


def marker_positions(samples, freq):
    """File times (s) at which a marker gap of the tone begins."""
    n = len(samples)
    if n < tones.RATE:
        return []
    baseband = samples * np.exp(-2j * np.pi * freq * np.arange(n) / tones.RATE)
    width = int(0.02 * tones.RATE)
    envelope = 2.0 * np.abs(np.convolve(baseband, np.ones(width) / width, mode="same"))
    on = envelope > 0.5 * np.percentile(envelope, 90)
    changes = np.flatnonzero(on[1:] != on[:-1]) + 1
    found = []
    for a, b in zip(changes[:-1], changes[1:]):
        gap = (b - a) / tones.RATE
        if not on[a] and 0.25 < gap < 0.35:
            found.append(a / tones.RATE)
    return found


def marker_errors(samples, freq, file_start):
    errors = {}
    for position in marker_positions(samples, freq):
        wall = file_start + position
        marker = round(wall / PERIOD) * PERIOD
        errors[int(marker)] = round(wall - marker, 4)
    return errors


def video_marker_errors(path, file_start, width, height):
    """Offsets of the white marker in the first camera tile of the composite."""
    scale = width / 1280.0
    crop = "crop=%d:%d:%d:%d" % (110 * scale, 30 * scale, 1150 * scale, 105 * scale)
    result = subprocess.run(
        ["ffmpeg", "-v", "error", "-nostdin", "-i", str(path), "-an", "-vf",
         f"{crop},signalstats,metadata=mode=print:key=lavfi.signalstats.YAVG:file=-",
         "-f", "null", "-"], capture_output=True, text=True)
    times, values = [], []
    for line in result.stdout.splitlines():
        if "pts_time:" in line:
            times.append(float(line.split("pts_time:")[1].split()[0]))
        elif "YAVG=" in line:
            values.append(float(line.split("=")[1]))
    first = times[0] if times else 0.0
    errors = {}
    for i in range(1, min(len(times), len(values))):
        if values[i] > 200 and values[i - 1] <= 200:
            wall = file_start + times[i] - first
            marker = round(wall / PERIOD) * PERIOD
            errors[int(marker)] = round(wall - marker, 4)
    return errors


def spread(values):
    values = sorted(values)
    if not values:
        return None
    median = float(np.median(values))
    usual = [v for v in values if abs(v - median) <= 0.3]
    return {"count": len(values), "median": round(median, 4), "min": usual[0], "max": usual[-1],
            "outliers": [v for v in values if abs(v - median) > 0.3]}


def playlist_info(path):
    text = path.read_text()
    dates = re.findall(r"#EXT-X-PROGRAM-DATE-TIME:(\S+)", text)
    durations = [float(d) for d in re.findall(r"#EXTINF:([0-9.]+)", text)]
    return {"segments": len(durations), "sum_extinf": round(sum(durations), 3), "first_program_date_time": dates[0] if dates else None,
            "program_date_times": len(dates), "endlist": "#EXT-X-ENDLIST" in text,
            "target_duration": (re.findall(r"#EXT-X-TARGETDURATION:(\d+)", text) or [None])[0]}


def iso_to_unix(text):
    from datetime import datetime
    return datetime.fromisoformat(text.replace("Z", "+00:00")).timestamp()


def analyze_composite(path, file_start, phases, frames_dir, tag, width):
    samples = tones.decode(str(path), 0, 0)
    levels = {freq: window_levels(samples, freq) for freq in FREQS}
    info = probe(path)
    video = next((s for s in info["streams"] if s.get("codec_type") == "video"), {})
    result = {"path": str(path), "probe": info, "audio_seconds": round(len(samples) / tones.RATE, 3), "phases": []}
    for index, phase in enumerate(phases, 1):
        start = phase["start_unix_ms"] / 1000.0 - file_start
        end = phase["end_unix_ms"] / 1000.0 - file_start
        measured = phase_levels(levels, start, end)
        frame = frames_dir / f"{tag}-phase{index}.png"
        subprocess.run(["ffmpeg", "-v", "error", "-y", "-ss", f"{max(0.0, (start + end) / 2):.2f}", "-i", str(path),
                        "-frames:v", "1", "-vf", "scale=960:-1", str(frame)])
        result["phases"].append({"phase": index, "name": phase["name"], "file_seconds": [round(start, 1), round(end, 1)],
                                 "expected_present_hz": phase["present"], "levels_dbfs": measured,
                                 "verdict": verdict(measured, set(phase["present"])), "frame": frame.name})
    audio = {}
    for name in ("alice/microphone", "bob/microphone", "carol/microphone"):
        errors = marker_errors(samples, TONES[name], file_start)
        if errors:
            audio[name] = errors
    result["audio_marker_errors"] = audio
    result["audio_marker_spread"] = spread([v for e in audio.values() for v in e.values()])
    vwidth = int(video.get("width") or width)
    video_errors = video_marker_errors(path, file_start, vwidth, int(video.get("height") or 0))
    result["video_marker_errors"] = video_errors
    result["video_marker_spread"] = spread(list(video_errors.values()))
    return result


def main():
    room = sys.argv[1]
    out = ROOT / "out" / room
    log = read_jsonl(out / "controller.jsonl")
    phases = json.loads((out / "phases.json").read_text()) if (out / "phases.json").exists() else []
    final = json.loads((out / "egress-final.json").read_text()).get("items", [])
    track_names = {e["track_sid"]: f"{e['identity']}/{e['source']}" for e in log if e["kind"] == "new_audio_track"}
    frames = out / "frames"
    frames.mkdir(exist_ok=True)
    report = {"room": room, "composites": [], "tracks": []}

    for item in final:
        if "room_composite" not in item:
            continue
        entry = {"egress_id": item["egress_id"], "status": item["status"], "error": item.get("error"),
                 "details": item.get("details"), "started_at": item.get("started_at"), "ended_at": item.get("ended_at"),
                 "preset": item["room_composite"].get("preset")}
        width = 1920 if "1080" in str(entry["preset"]) else 1280
        for result in item.get("file_results", []):
            path = local(result["filename"])
            if not path.exists():
                entry["mp4"] = {"missing": result["filename"]}
                continue
            start = int(result["started_at"]) / 1e9
            entry["mp4"] = analyze_composite(path, start, phases, frames, "mp4-" + item["egress_id"][-4:], width)
            entry["mp4"].update(reported_started_at=result["started_at"], reported_ended_at=result["ended_at"],
                                reported_duration_s=round(int(result.get("duration", 0)) / 1e9, 3), bytes=path.stat().st_size)
        for result in item.get("segment_results", []):
            path = local(result["playlist_name"])
            if not path.exists():
                entry["segments"] = {"missing": result["playlist_name"]}
                continue
            start = int(result["started_at"]) / 1e9
            remuxed = path.with_name(path.stem + "-remux.mp4")
            began = time.time()
            remux = subprocess.run(["ffmpeg", "-v", "error", "-nostdin", "-y", "-i", str(path), "-c", "copy",
                                    "-movflags", "+faststart", str(remuxed)], capture_output=True, text=True)
            remux_seconds = round(time.time() - began, 2)
            entry["segments"] = analyze_composite(remuxed, start, phases, frames, "hls-" + item["egress_id"][-4:], width)
            entry["segments"].update(remux_seconds=remux_seconds, remux_stderr=remux.stderr.strip().splitlines()[:3],
                                     remux_bytes=remuxed.stat().st_size, source_probe=probe(path))
            info = playlist_info(path)
            entry["segments"].update(reported_started_at=result["started_at"], reported_ended_at=result["ended_at"],
                                     reported_duration_s=round(int(result.get("duration", 0)) / 1e9, 3),
                                     segment_count=result.get("segment_count"), playlist=info,
                                     bytes=sum(p.stat().st_size for p in path.parent.glob(path.stem + "_*")))
            if info["first_program_date_time"]:
                pdt = iso_to_unix(info["first_program_date_time"])
                entry["segments"]["program_date_time_minus_started_at"] = round(pdt - start, 4)
        report["composites"].append(entry)

    for item in final:
        if "track" not in item:
            continue
        sid = item["track"]["track_id"]
        name = track_names.get(sid, sid)
        entry = {"egress_id": item["egress_id"], "track": name, "track_sid": sid, "status": item["status"],
                 "error": item.get("error"), "details": item.get("details")}
        for result in item.get("file_results", []):
            path = local(result["filename"])
            if not path.exists():
                entry["missing"] = result["filename"]
                continue
            start = int(result["started_at"]) / 1e9
            samples = tones.decode(str(path), 0, 0)
            levels = {freq: float(np.median(window_levels(samples, freq))) for freq in FREQS}
            expected = TONES.get(name)
            others = {f: v for f, v in levels.items() if f != expected}
            info = probe(path)
            entry.update(file=path.name, bytes=path.stat().st_size, media_seconds=round(len(samples) / tones.RATE, 3),
                         reported_duration_s=round(int(result.get("duration", 0)) / 1e9, 3),
                         reported_started_at=result["started_at"],
                         codec=[s.get("codec_name") for s in info["streams"]],
                         expected_hz=expected, expected_level_dbfs=round(levels.get(expected, -999.0), 1),
                         strongest_other_dbfs=round(max(others.values()), 1))
            if name.endswith("/microphone"):
                errors = marker_errors(samples, expected, start)
                entry["marker_errors"] = errors
                entry["marker_spread"] = spread(list(errors.values()))
        report["tracks"].append(entry)

    (out / "analysis.json").write_text(json.dumps(report, indent=1))
    summarize(report)


def summarize(report):
    for composite in report["composites"]:
        print(f"composite {composite['egress_id']} {composite['status']} preset={composite['preset']} error={composite['error']!r}")
        for kind in ("mp4", "segments"):
            part = composite.get(kind)
            if not part:
                continue
            if "missing" in part:
                print(f"  {kind}: missing {part['missing']}")
                continue
            streams = ", ".join(f"{s.get('codec_name')} {s.get('width', '')}x{s.get('height', '')}".strip("x ") for s in part["probe"]["streams"])
            print(f"  {kind}: {part['audio_seconds']} s audio, reported {part['reported_duration_s']} s, {part['bytes']} bytes, {streams}")
            if kind == "segments":
                print(f"    playlist: {part['playlist']}  pdt-started_at={part.get('program_date_time_minus_started_at')}")
            for phase in part["phases"]:
                print(f"    phase {phase['phase']} {phase['name']}: {phase['file_seconds']} {phase['levels_dbfs']} -> {phase['verdict']}")
            print(f"    audio markers: {part['audio_marker_spread']}")
            print(f"    video markers: {part['video_marker_spread']}")
    for track in report["tracks"]:
        if "missing" in track or "file" not in track:
            print(f"track {track['track']} {track['status']} no file ({track.get('error')!r} {track.get('details')!r})")
            continue
        print(f"track {track['track']:26s} {track['status']} {track['media_seconds']:8.2f} s {track['codec']} "
              f"expected {track['expected_hz']} Hz at {track['expected_level_dbfs']} dBFS, other max {track['strongest_other_dbfs']} "
              f"markers {track.get('marker_spread')} ({track.get('details')})")


if __name__ == "__main__":
    main()
