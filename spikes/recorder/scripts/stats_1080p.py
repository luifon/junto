#!/usr/bin/env python3
"""CPU and memory of Egress for a 1080p30 composite plus four track egresses (gate R5)."""
import sys

from harness import Run

run = Run(sys.argv[1] if len(sys.argv) > 1 else "stats-1080p")
run.watch()
for identity, mic, share, color in (("alice", 440, 1000, "2048c8"), ("bob", 620, 1370, "c83030")):
    run.participant(identity, "--mic-hz", mic, "--camera", "--screen", "--screen-audio-hz", share,
                    "--screen-color", color, "--leave-after", 85)
run.wait_until(8)
run.start_composite("debug", preset="H264_1080P_30")
run.recording = True
run.wait_until(75)
run.recording = False
run.stop_all_egress()
run.finish()
