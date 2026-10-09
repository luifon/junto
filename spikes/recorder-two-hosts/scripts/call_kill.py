#!/usr/bin/env python3
"""Recorder crash test (gates X3 and X4).

  call_kill.py ROOM plain     one composite MP4, one track egress per audio track
  call_kill.py ROOM product   one 1080p composite as 2 s segments, one track egress per audio track
  call_kill.py ROOM variants  composite MP4 + 6 s segments, composite 2 s segments,
                              track egress files, audio-only segments, audio-only Ogg file

Kills the Egress container (SIGKILL) during the recordings, logs what
ListEgress and ListParticipants report every second, calls StopEgress for a
lost egress, starts the container again and starts new recordings in the room.
"""
import calendar
import json
import subprocess
import sys
import time

from harness import ACTIVE, EGRESS, ROOT, Run

KILL_AT = 70
RESTART_AT = 165
NEW_AT = 180
STOP_AT = 225


def main():
    room, mode = sys.argv[1], sys.argv[2]
    run = Run(room)
    run.watch(stats_every=5.0)
    run.person("alice", "--leave-after", STOP_AT + 25)
    run.person("bob", "--screen-after", 3, "--leave-after", STOP_AT + 25)

    run.wait_until(10)
    if mode == "plain":
        run.start_composite("debug", mp4=True, segments=0, tag="plain")
    elif mode == "product":
        run.start_composite("debug", preset="H264_1080P_30", mp4=False, segments=2, tag="product")
    else:
        run.start_composite("debug", mp4=True, segments=6, tag="both6")
        run.start_composite("debug", mp4=False, segments=2, tag="seg2")
        people = run.people()
        for identity, source, sid in run.audio_tracks(people):
            if source == "microphone" and identity == "alice":
                run.start_audio_composite(sid, "alice-mic-seg2", segments=2)
            if source == "microphone" and identity == "bob":
                run.start_audio_composite(sid, "bob-mic-ogg")
    run.recording = True

    run.wait_until(KILL_AT - 5)
    before = run.poll_egress()
    run.log("before_kill", egress=[{"egress_id": i["egress_id"], "status": i["status"]} for i in before])
    run.wait_until(KILL_AT)
    run.recording = False
    kill = run.docker("kill", EGRESS)
    finished = run.docker("inspect", "-f", "{{.State.FinishedAt}}", EGRESS)["stdout"]
    whole, fraction = finished.rstrip("Z").split(".")
    kill_unix = calendar.timegm(time.strptime(whole, "%Y-%m-%dT%H:%M:%S")) + float("0." + fraction)
    run.log("killed", kill_unix=round(kill_unix, 3), container_finished_at=finished)

    health = subprocess.run(["curl", "-s", "-m", "3", "-o", "/dev/null", "-w", "%{http_code} %{time_total}", "http://egress:9090/"],
                            capture_output=True, text=True)
    run.log("health_port_after_kill", exit=health.returncode, output=health.stdout)
    # What the dead container held in its own working directory (when it is not a mounted directory).
    dead = run.out / "dead-container-tmp"
    if not any((ROOT / "egress-tmp").iterdir()):
        run.docker("cp", f"{EGRESS}:/home/egress/tmp", str(dead))
    dead.mkdir(exist_ok=True)
    listing = subprocess.run(["find", str(dead), str(ROOT / "egress-tmp"), "-type", "f", "-printf", "%p %s\n"],
                             capture_output=True, text=True).stdout
    run.log("temporary_files_after_kill", files=listing.splitlines())
    snapshot = run.out / "persistent-tmp-after-kill"
    if any((ROOT / "egress-tmp").iterdir()):
        subprocess.run(["cp", "-a", str(ROOT / "egress-tmp"), str(snapshot)], capture_output=True)
    out_listing = subprocess.run(["find", str(run.out), "-type", "f", "-newermt", "@%d" % run.started,
                                  "-not", "-name", "*.jsonl", "-printf", "%p %s %T@\n"], capture_output=True, text=True).stdout
    run.log("output_files_after_kill", files=sorted(out_listing.splitlines()))

    run.wait_until(KILL_AT + 12)
    lost = [i["egress_id"] for i in before if i["status"] in ACTIVE]
    if lost:
        run.api("Egress", "StopEgress", {"egress_id": lost[0]}, timeout=30)

    run.wait_until(RESTART_AT)
    items = run.poll_egress()
    run.log("list_egress_before_restart", seconds_after_kill=round(time.time() - kill_unix, 1),
            items=[{k: i.get(k) for k in ("egress_id", "status", "error", "details", "ended_at", "updated_at")} for i in items])
    status, reply = run.api("RoomService", "ListRooms", {"names": [room]})
    run.docker("start", EGRESS)
    time.sleep(5)
    left = subprocess.run(["find", str(ROOT / "egress-tmp"), "-type", "f", "-not", "-path", "*chromedp*"],
                          capture_output=True, text=True).stdout.splitlines()
    run.log("mounted_working_directory_after_restart", files=left)

    run.wait_until(NEW_AT)
    run.track_egress = {}
    new_composite = run.start_composite("debug", mp4=True, segments=2, tag="after-restart")
    run.log("new_composite", egress_id=new_composite)
    run.recording = True

    run.wait_until(STOP_AT)
    run.recording = False
    for item in run.poll_egress():
        if item.get("status") in ACTIVE and item["egress_id"] not in lost:
            run.api("Egress", "StopEgress", {"egress_id": item["egress_id"]}, timeout=10)
    time.sleep(8)
    for egress_id in lost[1:3]:
        run.api("Egress", "StopEgress", {"egress_id": egress_id}, timeout=10)
    run.finish()


if __name__ == "__main__":
    main()
