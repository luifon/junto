#!/usr/bin/env python3
"""The main recorded call (gates R1, R2, R3, R5, R6).

Three synthetic participants, one composite recording with the custom layout
page, one track egress per audio track. The controller log, the participants'
logs and the final ListEgress answer go to out/<room>/.
"""
import json
import sys

from harness import Run

MARKER = ["--marker-period-ms", 10000, "--marker-gap-ms", 300]


def main():
    run = Run(sys.argv[1] if len(sys.argv) > 1 else "call-a")
    run.watch()

    run.participant("alice", "--mic-hz", 440, *MARKER, "--camera", "--screen", "--screen-audio-hz", 1000,
                    "--screen-color", "2048c8", "--mute-after", 195, "--unmute-after", 215, "--leave-after", 262)
    run.participant("bob", "--mic-hz", 620, *MARKER, "--camera", "--screen", "--screen-audio-hz", 1370,
                    "--screen-color", "c83030", "--screen-after", 4, "--reconnect-after", 165,
                    "--reconnect-gap", 3, "--leave-after", 262)

    run.wait_until(12)
    composite = run.start_composite("debug")
    run.recording = True

    run.wait_until(70)
    run.participant("carol", "--mic-hz", 790, *MARKER, "--camera", "--screen", "--screen-audio-hz", 1730,
                    "--screen-color", "189046", "--screen-after", 5, "--leave-after", 192)

    run.wait_until(105)
    run.api("Egress", "UpdateLayout", {"egress_id": composite, "layout": "main:alice;debug"})
    run.wait_until(120)
    run.api("RoomService", "UpdateRoomMetadata", {"room": run.room, "metadata": json.dumps({"main_share": "bob"})})
    run.wait_until(135)
    run.send_layout_data({"main_share": "alice"})
    run.wait_until(150)
    run.send_layout_data({"main_share": ""})
    run.api("RoomService", "UpdateRoomMetadata", {"room": run.room, "metadata": ""})
    run.api("Egress", "UpdateLayout", {"egress_id": composite, "layout": "debug"})

    run.wait_until(245)
    run.recording = False
    run.stop_all_egress()
    run.finish()


if __name__ == "__main__":
    main()
