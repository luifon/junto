#!/usr/bin/env python3
"""Recordings with known defects for the result check (gate X8).

  call_x8.py ROOM

Four composite recordings of the same call, one after the other, each 40 s:
  good      the normal layout page
  nosound   the page attaches no audio (Egress reports no error)
  novideo   the page shows no picture (Egress reports no error)
  starved   the Egress container is limited to 0.5 CPU for 25 s during the recording
One track egress per audio track runs for the whole call and gives the reference files.
Then three damaged copies of the good file are made with ffmpeg, and
check_recording.py runs on every file. Writes out/ROOM/x8.json.
"""
import json
import subprocess
import sys
import time
from pathlib import Path

from harness import ACTIVE, EGRESS, ROOT, Run

CHECK = str(Path(__file__).with_name("check_recording.py"))


def record(run, tag, layout, starve=False, seconds=40):
    egress_id = run.start_composite(layout, preset="H264_720P_30", mp4=True, segments=0, tag=tag)
    began = time.time()
    if starve:
        time.sleep(10)
        run.docker("update", "--cpus", "0.5", EGRESS)
        time.sleep(25)
        run.docker("update", "--cpus", "6", EGRESS)
        time.sleep(5)
    else:
        time.sleep(seconds)
    run.api("Egress", "StopEgress", {"egress_id": egress_id}, timeout=20)
    deadline = time.time() + 90
    while time.time() < deadline:
        items = [i for i in run.poll_egress() if i["egress_id"] == egress_id]
        if items and items[0]["status"] not in ACTIVE:
            return items[0], time.time() - began
        time.sleep(1)
    return None, time.time() - began


def check(path, *extra):
    result = subprocess.run(["python3", CHECK, str(path), *extra], capture_output=True, text=True)
    try:
        return json.loads(result.stdout)
    except ValueError:
        return {"ok": None, "problems": [result.stderr[-300:]]}


def ffmpeg(*args):
    return subprocess.run(["ffmpeg", "-v", "error", "-nostdin", "-y", *args], capture_output=True, text=True).stderr.strip()[:200]


def main():
    run = Run(sys.argv[1])
    run.watch(stats_every=2.0)
    run.person("alice", "--leave-after", 290)
    run.person("bob", "--screen-after", 3, "--leave-after", 290)
    run.wait_until(8)
    run.recording = True
    time.sleep(4)
    results = {}
    for tag, layout, starve in (("good", "", False), ("nosound", "nosound", False), ("novideo", "novideo", False),
                                ("starved", "", True)):
        item, seconds = record(run, tag, layout, starve)
        results[tag] = {"egress": {k: item.get(k) for k in ("egress_id", "status", "error", "details")} if item else None,
                        "file": item["file_results"][0]["filename"] if item and item.get("file_results") else None,
                        "file_started_at": item["file_results"][0]["started_at"] if item and item.get("file_results") else None}
        run.log("recorded", tag=tag, result=results[tag])
        time.sleep(3)
    run.recording = False
    run.stop_all_egress()
    run.finish(wait=5)
    for process in run.processes:
        process.terminate()

    final = json.loads((run.out / "egress-final.json").read_text())["items"]
    reference = None
    log = [json.loads(line) for line in (run.out / "controller.jsonl").read_text().splitlines()]
    alice_mic = [e["track_sid"] for e in log if e["kind"] == "new_audio_track" and e["identity"] == "alice" and e["source"] == "microphone"]
    for item in final:
        if "track" in item and alice_mic and item["track"]["track_id"] == alice_mic[0] and item.get("file_results"):
            reference = (ROOT / item["file_results"][0]["filename"].lstrip("/"), int(item["file_results"][0]["started_at"]) / 1e9)

    report = {"reference": str(reference[0]) if reference else None, "files": {}}
    for tag, result in results.items():
        if not result["file"]:
            report["files"][tag] = {"egress": result["egress"], "check": None}
            continue
        path = ROOT / result["file"].lstrip("/")
        extra = []
        if reference:
            offset = reference[1] - int(result["file_started_at"]) / 1e9
            extra = ["--reference", f"{reference[0]}:{offset:.3f}"]
        report["files"][tag] = {"egress": result["egress"], "check": check(path, *extra)}

    good = ROOT / results["good"]["file"].lstrip("/")
    offset = reference[1] - int(results["good"]["file_started_at"]) / 1e9 if reference else 0.0
    extra = ["--reference", f"{reference[0]}:{offset:.3f}"] if reference else []
    damaged = {
        "audio-hole": ffmpeg("-i", str(good), "-c", "copy", "-bsf:a", "noise=drop=between(pts*tb\\,10\\,18)", str(run.out / "damaged-audio-hole.mp4")),
        "audio-muted": ffmpeg("-i", str(good), "-c:v", "copy", "-af", "volume=enable='between(t,10,18)':volume=0", "-c:a", "aac",
                              str(run.out / "damaged-audio-muted.mp4")),
        "video-short": ffmpeg("-i", str(good), "-t", "15", "-i", str(good), "-map", "0:a", "-map", "1:v", "-c", "copy",
                              str(run.out / "damaged-video-short.mp4")),
    }
    for name, error in damaged.items():
        path = run.out / f"damaged-{name}.mp4"
        report["files"][f"damaged-{name}"] = {"made_with_error": error, "check": check(path, *extra) if path.exists() else None}
    (run.out / "x8.json").write_text(json.dumps(report, indent=1))
    for tag, entry in report["files"].items():
        c = entry["check"] or {}
        print(tag, "| egress:", entry.get("egress"), "| ok:", c.get("ok"), "| problems:", c.get("problems"),
              "| seconds:", c.get("seconds"), "| check took:", c.get("check_seconds"), "| sound:", c.get("sound"), "| picture:", c.get("picture"),
              "| fps:", c.get("video_frames_per_second"))


if __name__ == "__main__":
    main()
