#!/usr/bin/env python3
"""Smaller checks: the unified StartEgress request, pass-through audio, a
graceful stop of the Egress container, and the refusal of a request.
Writes out/<room>/controller.jsonl.
"""
import subprocess
import sys
import threading
import time

from harness import ACTIVE, Run
from lkapi import call

EGRESS = "junto-spike-rec-egress"


def state(run, label):
    result = subprocess.run(["docker", "inspect", "-f", "{{.State.Status}}", EGRESS], capture_output=True, text=True)
    _, reply = call("Egress", "ListEgress", {"room_name": run.room, "active": True})
    run.log("state", label=label, container=result.stdout.strip(),
            active=[(i["egress_id"], i["status"]) for i in reply.get("items", [])])


def main():
    run = Run(sys.argv[1] if len(sys.argv) > 1 else "probe")
    run.watch(sample_stats=False)
    run.participant("alice", "--mic-hz", 440, "--camera", "--screen", "--screen-audio-hz", 1000,
                    "--screen-color", "2048c8", "--leave-after", 230)
    run.participant("bob", "--mic-hz", 620, "--camera", "--screen", "--screen-audio-hz", 1370,
                    "--screen-color", "c83030", "--screen-after", 3, "--leave-after", 230)
    run.wait_until(8)
    tracks = {f"{identity}/{source}": sid for identity, source, sid in run.audio_tracks()}
    run.log("tracks", tracks=tracks)

    _, composite = run.api("Egress", "StartEgress", {
        "room_name": run.room,
        "template": {"layout": "main:alice;debug", "custom_base_url": "http://layout/"},
        "preset": "H264_720P_30",
        "outputs": [{"file": {"file_type": "MP4", "filepath": f"/out/{run.room}/unified-composite-{{time}}.mp4"}}],
    })
    run.api("Egress", "StartEgress", {
        "room_name": run.room,
        "media": {"audio": {"routes": [{"track_id": tracks["alice/microphone"]}]}},
        "preset": "PASSTHROUGH",
        "outputs": [{"file": {"filepath": f"/out/{run.room}/passthrough-mic-{{time}}"}}],
    })
    run.api("Egress", "StartEgress", {
        "room_name": run.room,
        "media": {"audio": {"routes": [{"track_id": tracks["alice/screen_share_audio"]}]}},
        "outputs": [{"file": {"file_type": "OGG", "filepath": f"/out/{run.room}/unified-share-{{time}}.ogg"}}],
    })
    run.wait_until(32)
    if composite.get("egress_id"):
        run.api("Egress", "UpdateLayout", {"egress_id": composite["egress_id"], "layout": "main:bob;debug"})
    run.wait_until(50)
    run.stop_all_egress()

    run.wait_until(60)
    _, reply = run.api("Egress", "StartTrackEgress", {
        "room_name": run.room, "track_id": tracks["bob/microphone"],
        "file": {"filepath": f"/out/{run.room}/grace-{{time}}"}})
    time.sleep(6)

    def stop_container():
        started = time.time()
        subprocess.run(["docker", "stop", "-t", "60", EGRESS], capture_output=True)
        run.log("docker_stop_returned", seconds=round(time.time() - started, 1))

    run.log("docker_stop_called", grace_seconds=60)
    stopper = threading.Thread(target=stop_container)
    stopper.start()
    time.sleep(10)
    state(run, "10 s after docker stop -t 60")
    run.api("Egress", "StopEgress", {"egress_id": reply.get("egress_id")})
    stopper.join()
    state(run, "after docker stop returned")
    subprocess.run(["docker", "start", EGRESS], capture_output=True)
    time.sleep(8)
    state(run, "after docker start")

    accepted = 0
    for n in range(8):
        status, reply = run.api("Egress", "StartRoomCompositeEgress", {
            "room_name": run.room, "layout": "debug", "custom_base_url": "http://layout/", "preset": "H264_720P_30",
            "file_outputs": [{"file_type": "MP4", "filepath": f"/out/{run.room}/many-{n}-{{time}}.mp4"}]})
        if status != 200:
            run.log("composite_refused", after_accepted=accepted, status=status, reply=reply)
            break
        accepted += 1
        time.sleep(10)
    result = subprocess.run(["docker", "stats", "--no-stream", "--format", "{{.Name}} {{.CPUPerc}} {{.MemUsage}}",
                             EGRESS], capture_output=True, text=True)
    run.log("composites", accepted=accepted, stats=result.stdout.strip())
    run.stop_all_egress(timeout=90)
    run.finish()


if __name__ == "__main__":
    main()
