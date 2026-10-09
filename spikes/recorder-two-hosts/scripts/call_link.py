#!/usr/bin/env python3
"""Loss of the private link during a recording (gate X5).

  call_link.py ROOM [OUTAGE_SECONDS]

Stops the link container on the data host during a recording and starts it
again. A subscriber reports every second what it receives, so the log shows
whether the call itself continued. During the outage the script asks Egress to
stop one track egress, lets one participant leave, and asks for a new egress.
"""
import json
import subprocess
import sys
import threading
import time

from harness import ACTIVE, LINK, Run

DOWN_AT = 60


def main():
    room = sys.argv[1]
    outage = int(sys.argv[2]) if len(sys.argv) > 2 else 30
    up_at = DOWN_AT + outage
    end_at = up_at + 75
    run = Run(room)
    run.watch(stats_every=5.0)
    run.person("alice", "--leave-after", end_at + 20)
    run.person("bob", "--screen-after", 3, "--leave-after", end_at + 20)
    run.person("carol", "--leave-after", DOWN_AT + 15, screen=False)
    time.sleep(3)
    run.participant("probe", "--duration", end_at + 10, "--report-every", 1, mode="subscribe")

    run.wait_until(10)
    run.start_composite("debug", preset="H264_1080P_30", mp4=True, segments=2)
    run.recording = True

    run.wait_until(DOWN_AT - 3)
    tracks = {(i, s): sid for i, s, sid in run.audio_tracks(run.people())}
    egress_of = dict(run.track_egress)
    run.wait_until(DOWN_AT)
    run.recording = False
    run.docker("stop", "-t", "1", LINK)

    run.wait_until(DOWN_AT + 8)
    target = egress_of.get(tracks.get(("alice", "screen_share_audio")))
    if target:
        run.api("Egress", "StopEgress", {"egress_id": target}, timeout=30)

    def late_start():
        sid = tracks.get(("bob", "microphone"))
        status, reply = run.api("Egress", "StartTrackEgress", {
            "room_name": room, "track_id": sid,
            "file": {"filepath": f"/out/{room}/tracks/started-during-outage-{{time}}"}}, timeout=60)
        run.log("start_during_outage", status=status, egress_id=reply.get("egress_id"))

    run.wait_until(DOWN_AT + 12)
    threading.Thread(target=late_start, daemon=True).start()

    run.wait_until(up_at)
    run.docker("start", LINK)
    for _ in range(20):
        shake = subprocess.run(["docker", "exec", LINK, "wg", "show", "wg0", "latest-handshakes"], capture_output=True, text=True)
        stamp = shake.stdout.split()[-1] if shake.stdout.split() else "0"
        if stamp != "0":
            run.log("link_handshake", handshake_unix=int(stamp), seconds_after_start=round(time.time() - (run.started + up_at), 2))
            break
        time.sleep(0.5)

    run.wait_until(up_at + 20)
    status, reply = run.api("Egress", "StartTrackEgress", {
        "room_name": room, "track_id": tracks.get(("bob", "microphone")),
        "file": {"filepath": f"/out/{room}/tracks/started-after-outage-{{time}}"}}, timeout=60)

    run.wait_until(end_at)
    for item in run.poll_egress():
        if item.get("status") in ACTIVE:
            run.api("Egress", "StopEgress", {"egress_id": item["egress_id"]}, timeout=15)
    time.sleep(10)
    run.poll_egress()
    run.finish()


if __name__ == "__main__":
    main()
