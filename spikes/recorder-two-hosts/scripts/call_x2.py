#!/usr/bin/env python3
"""The recorded call across two hosts (gates X2 and X6).

  call_x2.py ROOM [PRESET] [SEGMENT_SECONDS]

Three synthetic participants on the data host join a room on the public host.
One composite recording (MP4 file and segments from one egress) and one track
egress per audio track. Writes out/ROOM/phases.json for analyze.py.
"""
import json
import sys
import time

from harness import Run, wait_quiet


def events(run, name):
    path = run.out / f"{name}.jsonl"
    return [json.loads(line) for line in path.read_text().splitlines() if line.startswith("{")]


def main():
    room = sys.argv[1]
    preset = sys.argv[2] if len(sys.argv) > 2 else "H264_1080P_30"
    segments = int(sys.argv[3]) if len(sys.argv) > 3 else 4
    waited = wait_quiet()
    run = Run(room)
    run.log("waited_for_quiet_host", seconds=waited[0], cpu_pressure_10s=waited[1], load_1m=waited[2])
    run.watch()
    run.person("alice", "--leave-after", 160)
    run.person("bob", "--screen-after", 4, "--leave-after", 160)

    run.wait_until(12)
    composite = run.start_composite("debug", preset=preset, mp4=True, segments=segments)
    started = time.time()
    run.recording = True

    run.wait_until(60)
    run.person("carol", "--screen-after", 5, "--leave-after", 38)

    run.wait_until(110)
    _, _ = run.api("Egress", "UpdateLayout", {"egress_id": composite, "layout": "main:alice;debug"})
    updated = time.time()

    run.wait_until(145)
    stopping = time.time()
    run.recording = False
    run.stop_all_egress()
    run.finish()

    def share_time(name, nth=0):
        found = [e for e in events(run, name) if e.get("event") == "published" and e.get("source") == "screen_share_audio"]
        return found[nth]["unix_ms"]

    carol_share = share_time("carol")
    carol_left = [e for e in events(run, "carol") if e.get("event") == "left"][0]["unix_ms"]
    phases = [
        {"name": "two shares, no instruction (bob's started last)", "start_unix_ms": int(started * 1000) + 8000,
         "end_unix_ms": carol_share - 1500, "present": [440, 620, 1370]},
        {"name": "third share started (carol)", "start_unix_ms": carol_share + 3000,
         "end_unix_ms": carol_left - 1500, "present": [440, 620, 790, 1730]},
        {"name": "carol left (bob's share is the latest again)", "start_unix_ms": carol_left + 3000,
         "end_unix_ms": int(updated * 1000) - 1000, "present": [440, 620, 1370]},
        {"name": "UpdateLayout main:alice", "start_unix_ms": int(updated * 1000) + 3000,
         "end_unix_ms": int(stopping * 1000) - 2000, "present": [440, 620, 1000]},
    ]
    (run.out / "phases.json").write_text(json.dumps(phases, indent=1))


if __name__ == "__main__":
    main()
