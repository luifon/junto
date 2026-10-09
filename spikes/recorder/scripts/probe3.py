#!/usr/bin/env python3
"""After Egress is restarted during a recording: is the lost egress still listed
as a participant of the room, and what do ListEgress and StopEgress answer?
"""
import subprocess
import sys
import time

from harness import Run
from lkapi import call

run = Run(sys.argv[1] if len(sys.argv) > 1 else "probe3")
run.participant("alice", "--mic-hz", 440, "--leave-after", 100)
run.wait_until(5)
mic = run.audio_tracks()[0][2]
_, reply = run.api("Egress", "StartTrackEgress", {
    "room_name": run.room, "track_id": mic, "file": {"filepath": f"/out/{run.room}/lost-{{time}}"}})
egress_id = reply["egress_id"]


def observe(label):
    _, people = call("RoomService", "ListParticipants", {"room": run.room})
    _, listed = call("Egress", "ListEgress", {"egress_id": egress_id})
    _, room = call("RoomService", "ListRooms", {"names": [run.room]})
    run.log("observe", label=label,
            participants=[(p["identity"], p.get("kind"), p.get("state")) for p in people.get("participants", [])],
            egress_status=[i["status"] for i in listed.get("items", [])],
            active_recording=[r.get("active_recording") for r in room.get("rooms", [])])


time.sleep(5)
observe("recording")
subprocess.run(["docker", "restart", "junto-spike-rec-egress"], capture_output=True)
run.log("egress_restarted")
for wait in (2, 10, 20, 40, 70):
    run.wait_until(12 + wait)
    observe(f"about {wait} s after the restart")
run.api("Egress", "StopEgress", {"egress_id": egress_id})
run.finish()
