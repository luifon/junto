#!/usr/bin/env python3
"""What a killed Egress left on disk, whether it plays, and how many seconds are lost.

  survey_kill.py ROOM

Looks at three places: the output directory (the final location), the copy of
the dead container's own temporary directory, and the copy of the mounted
temporary directory. Writes out/ROOM/survey.json and prints a table.

Seconds lost = time of the kill - wall-clock time of the last recorded sample.
The last sample's wall-clock time comes from the last 10 s time marker in the
recovered sound: marker time + (length of the sound - position of the marker).
For sound without a marker tone it is the reported start time plus the length.
"""
import json
import subprocess
import sys
import time
from pathlib import Path

import numpy as np

import tones
from analyze import marker_positions, probe, read_jsonl

ROOT = Path("/work")
MIC = {"alice": 440, "bob": 620, "carol": 790}
MEDIA = (".mp4", ".ogg", ".ts", ".m3u8", ".webm", ".aac", ".m4a")


def decode_errors(path):
    result = subprocess.run(["ffmpeg", "-v", "error", "-nostdin", "-i", str(path), "-f", "null", "-"],
                            capture_output=True, text=True)
    lines = [line for line in result.stderr.splitlines() if line.strip()]
    return {"exit": result.returncode, "error_lines": len(lines), "first": lines[:2]}


def sound_end(path, start_unix, kill_unix):
    """Wall-clock time of the last sample, by marker and by reported start time."""
    try:
        samples = tones.decode(str(path), 0, 0)
    except subprocess.CalledProcessError:
        return None
    length = len(samples) / tones.RATE
    result = {"sound_seconds": round(length, 3)}
    if start_unix:
        result["lost_by_start_time"] = round(kill_unix - (start_unix + length), 2)
    best = None
    from analyze import window_levels
    levels = {freq: float(np.median(window_levels(samples, freq))) for freq in MIC.values()} if length > 1 else {}
    for name, freq in MIC.items():
        if levels.get(freq, -99.0) < -30.0:
            continue
        positions = marker_positions(samples, freq)
        if positions:
            position = positions[-1]
            guess = (start_unix or (kill_unix - length)) + position
            marker = round(guess / 10.0) * 10.0
            end = marker + (length - position)
            best = {"marker_tone_hz": freq, "markers": len(positions), "last_marker_at": round(position, 3),
                    "lost_by_marker": round(kill_unix - end, 2), "start_by_marker": round(marker - position, 3)}
            break
    if best:
        result.update(best)
    return result


def describe(path, start_unix, kill_unix):
    info = probe(path)
    entry = {"file": str(path.relative_to(ROOT / "out")), "bytes": path.stat().st_size,
             "ffprobe_ok": info["ok"], "ffprobe_error": info["error"],
             "streams": [f"{s.get('codec_name')}" for s in info["streams"]],
             "container_duration": info["format"].get("duration")}
    if info["ok"]:
        entry["decode"] = decode_errors(path)
        entry["end"] = sound_end(path, start_unix, kill_unix)
    return entry


def remux(command, target, timeout=90):
    began = time.time()
    try:
        result = subprocess.run(command, capture_output=True, text=True, timeout=timeout)
        code, stderr = result.returncode, result.stderr.strip().splitlines()[:3]
    except subprocess.TimeoutExpired:
        code, stderr = None, [f"no end after {timeout} s; stopped"]
    return {"command": " ".join(command), "exit": code, "seconds": round(time.time() - began, 2),
            "stderr": stderr, "bytes": target.stat().st_size if target.exists() else 0}


def main():
    room = sys.argv[1]
    out = ROOT / "out" / room
    log = read_jsonl(out / "controller.jsonl")
    killed = [e for e in log if e["kind"] in ("killed", "crashed")][0]
    kill_unix = killed.get("kill_unix") or killed["unix"]
    starts, kinds = {}, {}
    for event in log:
        if event["kind"] == "egress_change" and event["unix_ms"] / 1000.0 < kill_unix:
            for result in event["results"]:
                if result.get("started_at") not in (None, "0"):
                    starts[event["egress_id"]] = int(result["started_at"]) / 1e9
    report = {"room": room, "kill_unix": kill_unix, "final_location": [], "container_tmp": [], "mounted_tmp": [], "segments": []}

    def start_for(path):
        for egress_id, start in starts.items():
            if egress_id in str(path):
                return start
        return None

    # Final location: anything written before the kill.
    final = [p for p in out.rglob("*") if p.is_file() and p.suffix in MEDIA
             and "dead-container-tmp" not in p.parts and "persistent-tmp-after-kill" not in p.parts
             and "recovered" not in p.parts and "rescued-by-entrypoint" not in p.parts
             and p.stat().st_mtime <= kill_unix + 1.0]
    manifests = {}
    for path in out.rglob("EG_*.json"):
        if "tmp" in str(path):
            continue
    singles = [p for p in final if p.suffix in (".mp4", ".ogg", ".webm")]
    for path in sorted(singles):
        report["final_location"].append(describe(path, None, kill_unix))

    recovered = out / "recovered"
    recovered.mkdir(exist_ok=True)
    prefixes = sorted({p.name.rsplit("_", 1)[0] for p in final if p.suffix == ".ts"})
    for prefix in prefixes:
        parts = sorted(p for p in final if p.suffix == ".ts" and p.name.rsplit("_", 1)[0] == prefix)
        playlist = out / "hls" / f"{prefix}.m3u8"
        live = out / "hls" / f"{prefix}-live.m3u8"
        entry = {"prefix": prefix, "segments_on_disk": len(parts), "bytes": sum(p.stat().st_size for p in parts),
                 "last_segment_mtime_before_kill": round(kill_unix - max(p.stat().st_mtime for p in parts), 2)}
        for name, candidate in (("playlist", playlist), ("live_playlist", live)):
            if candidate.exists() and candidate.stat().st_mtime <= kill_unix + 1.0:
                text = candidate.read_text()
                entry[name] = {"segments_listed": text.count("#EXTINF"), "endlist": "#EXT-X-ENDLIST" in text,
                               "first_program_date_time": (__import__("re").findall(r"#EXT-X-PROGRAM-DATE-TIME:(\S+)", text) or [None])[0]}
            else:
                entry[name] = None
        start = None
        first_date = (entry.get("playlist") or {}).get("first_program_date_time")
        if first_date:
            from analyze import iso_to_unix
            start = iso_to_unix(first_date)
        if entry["playlist"]:
            if not entry["playlist"]["endlist"]:
                target = recovered / f"{prefix}-from-open-playlist.mp4"
                entry["from_open_playlist"] = remux(["ffmpeg", "-v", "error", "-nostdin", "-y", "-i", str(playlist), "-c", "copy",
                                                     "-movflags", "+faststart", str(target)], target, timeout=25)
                closed = playlist.with_name(f"{prefix}-closed.m3u8")
                closed.write_text(playlist.read_text().rstrip("\n") + "\n#EXT-X-ENDLIST\n")
                playlist = closed
            target = recovered / f"{prefix}-from-playlist.mp4"
            entry["from_playlist"] = remux(["ffmpeg", "-v", "error", "-nostdin", "-y", "-i", str(playlist), "-c", "copy",
                                            "-movflags", "+faststart", str(target)], target)
            entry["from_playlist"]["result"] = describe(target, start, kill_unix)
        listing = recovered / f"{prefix}-list.txt"
        listing.write_text("".join(f"file '{p}'\n" for p in parts))
        target = recovered / f"{prefix}-from-files.mp4"
        entry["from_files"] = remux(["ffmpeg", "-v", "error", "-nostdin", "-y", "-f", "concat", "-safe", "0", "-i", str(listing),
                                     "-c", "copy", "-movflags", "+faststart", str(target)], target)
        entry["from_files"]["result"] = describe(target, start, kill_unix)
        report["segments"].append(entry)

    for key, directory in (("container_tmp", out / "dead-container-tmp"), ("mounted_tmp", out / "persistent-tmp-after-kill"),
                           ("mounted_tmp", out / "rescued-by-entrypoint")):
        if not directory.exists():
            continue
        for path in sorted(directory.rglob("*")):
            if path.is_file() and path.suffix in MEDIA and "chromedp" not in str(path) and "Chrome" not in str(path):
                if path.suffix in (".ts", ".m3u8"):
                    report[key].append({"file": str(path.relative_to(ROOT / "out")), "bytes": path.stat().st_size})
                else:
                    report[key].append(describe(path, start_for(path), kill_unix))

    (out / "survey.json").write_text(json.dumps(report, indent=1))
    show(report)


def show(report):
    print("kill at", report["kill_unix"])
    for key in ("final_location", "container_tmp", "mounted_tmp"):
        print(f"== {key}: {len(report[key])} media file(s)")
        for entry in report[key]:
            if "ffprobe_ok" not in entry:
                print(f"  {entry['file']} {entry['bytes']} bytes")
                continue
            print(f"  {entry['file']} {entry['bytes']} bytes ffprobe_ok={entry['ffprobe_ok']} {entry['streams']} "
                  f"duration={entry['container_duration']} decode={entry.get('decode')} end={entry.get('end')} {entry['ffprobe_error'][:1]}")
    print(f"== segments in the final location: {len(report['segments'])} group(s)")
    for entry in report["segments"]:
        print(f"  {entry['prefix']}: {entry['segments_on_disk']} segments, {entry['bytes']} bytes, newest written "
              f"{entry['last_segment_mtime_before_kill']} s before the kill, playlist={entry['playlist']} live={entry['live_playlist']}")
        if "from_open_playlist" in entry:
            r = entry["from_open_playlist"]
            print(f"    from_open_playlist (no ENDLIST line): exit {r['exit']} in {r['seconds']} s, {r['stderr'][:1]}")
        for way in ("from_playlist", "from_files"):
            if way in entry:
                r = entry[way]
                print(f"    {way}: exit {r['exit']} in {r['seconds']} s, {r['bytes']} bytes, {r['stderr'][:1]}")
                print(f"      {r['result'].get('streams')} duration={r['result'].get('container_duration')} decode={r['result'].get('decode')} end={r['result'].get('end')}")


if __name__ == "__main__":
    main()
