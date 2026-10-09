#!/usr/bin/env python3
"""Admission limits of one Egress instance (gate R5).

Part 1: one composite recording, then track egress requests for one microphone
track until one is refused (at most 30). Part 2: composite recordings until one
is refused (at most 4). Writes out/<room>/controller.jsonl.
"""
import subprocess
import sys
import time

from harness import ACTIVE, CONTAINERS, Run
from lkapi import call


def stats(run, label):
    result = subprocess.run(["docker", "stats", "--no-stream", "--format", "{{.Name}} {{.CPUPerc}} {{.MemUsage}}",
                             *CONTAINERS], capture_output=True, text=True)
    run.log("docker_stats", label=label, rows=result.stdout.splitlines())


def active(run):
    _, reply = call("Egress", "ListEgress", {"room_name": run.room, "active": True})
    return [i for i in reply.get("items", []) if i["status"] in ACTIVE]


def main():
    run = Run(sys.argv[1] if len(sys.argv) > 1 else "limits")
    run.participant("alice", "--mic-hz", 440, "--camera", "--screen", "--screen-audio-hz", 1000, "--leave-after", 170)
    run.wait_until(6)
    mic = next(sid for _, source, sid in run.audio_tracks() if source == "microphone")

    run.start_composite("debug")
    time.sleep(10)
    accepted = 0
    for n in range(30):
        status, reply = run.api("Egress", "StartTrackEgress", {
            "room_name": run.room, "track_id": mic,
            "file": {"filepath": f"/out/{run.room}/tracks/mic-{n:02d}-{{time}}"}})
        if status != 200:
            run.log("track_egress_refused", after_accepted=accepted, status=status, reply=reply)
            break
        accepted += 1
        time.sleep(1.0)
    time.sleep(8)
    run.log("part1", composites=1, track_egress_accepted=accepted, active_now=len(active(run)))
    stats(run, f"1 composite + {accepted} track egresses")
    run.stop_all_egress(timeout=60)

    time.sleep(5)
    composites = 0
    for n in range(4):
        status, reply = run.api("Egress", "StartRoomCompositeEgress", {
            "room_name": run.room, "layout": "debug", "custom_base_url": "http://layout/", "preset": "H264_720P_30",
            "file_outputs": [{"file_type": "MP4", "filepath": f"/out/{run.room}/composite-{n}-{{time}}.mp4"}]})
        if status != 200:
            run.log("composite_refused", after_accepted=composites, status=status, reply=reply)
            break
        composites += 1
        time.sleep(12)
    run.log("part2", composites_accepted=composites, active_now=len(active(run)))
    stats(run, f"{composites} composites")
    for item in run.poll_egress():
        if item["status"] not in ACTIVE and item.get("error"):
            run.log("egress_error", egress_id=item["egress_id"], status=item["status"], error=item["error"])
    run.stop_all_egress(timeout=60)
    run.finish()


if __name__ == "__main__":
    main()
