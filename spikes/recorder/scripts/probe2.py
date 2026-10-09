#!/usr/bin/env python3
"""Further checks: which of two existing shares the page picks without an
instruction, how long `docker restart` waits for a running egress, and what a
start request returns when no Egress instance is running.
"""
import subprocess
import sys
import threading
import time

from harness import Run
from lkapi import call

EGRESS = "junto-spike-rec-egress"


def state(run, label):
    result = subprocess.run(["docker", "inspect", "-f", "{{.State.Status}}", EGRESS], capture_output=True, text=True)
    _, reply = call("Egress", "ListEgress", {"room_name": run.room})
    run.log("state", label=label, container=result.stdout.strip(),
            egress=[(i["egress_id"], i["status"]) for i in reply.get("items", [])])


def timed_docker(run, *args):
    started = time.time()
    subprocess.run(["docker", *args], capture_output=True)
    run.log("docker", args=list(args), seconds=round(time.time() - started, 1))


def main():
    run = Run(sys.argv[1] if len(sys.argv) > 1 else "probe2")
    run.watch(sample_stats=False)
    run.participant("bob", "--mic-hz", 620, "--camera", "--screen", "--screen-audio-hz", 1370,
                    "--screen-color", "c83030", "--leave-after", 150)
    run.participant("alice", "--mic-hz", 440, "--camera", "--screen", "--screen-audio-hz", 1000,
                    "--screen-color", "2048c8", "--screen-after", 5, "--leave-after", 150)
    run.wait_until(10)
    mic = next(sid for identity, source, sid in run.audio_tracks() if (identity, source) == ("alice", "microphone"))
    composite = run.start_composite("debug")
    run.wait_until(30)
    run.api("Egress", "StopEgress", {"egress_id": composite})

    def track_egress(name):
        return run.api("Egress", "StartTrackEgress", {
            "room_name": run.room, "track_id": mic, "file": {"filepath": f"/out/{run.room}/{name}-{{time}}"}})[1]

    run.wait_until(36)
    track_egress("lost-by-restart")
    time.sleep(5)
    timed_docker(run, "restart", EGRESS)
    time.sleep(8)
    state(run, "after docker restart (default timeout)")

    second = track_egress("kept-by-long-timeout")
    time.sleep(5)
    restart = threading.Thread(target=timed_docker, args=(run, "restart", "-t", "40", EGRESS))
    restart.start()
    time.sleep(10)
    state(run, "10 s into docker restart -t 40")
    run.api("Egress", "StopEgress", {"egress_id": second.get("egress_id")})
    restart.join()
    time.sleep(6)
    state(run, "after docker restart -t 40")

    timed_docker(run, "stop", EGRESS)
    started = time.time()
    status, reply = call("Egress", "StartTrackEgress", {
        "room_name": run.room, "track_id": mic, "file": {"filepath": f"/out/{run.room}/no-instance-{{time}}"}},
        timeout=90)
    run.log("start_without_egress_instance", seconds=round(time.time() - started, 1), status=status, reply=reply)
    timed_docker(run, "start", EGRESS)
    time.sleep(5)
    state(run, "end")
    run.finish()


if __name__ == "__main__":
    main()
