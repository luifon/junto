#!/usr/bin/env python3
"""Cost of one recording (gate X7, first part).

  x7_cost.py ROOM

A call with three publishing participants (5 audio tracks, 2 shares, 3
cameras) and two subscribers. Phase A: no recording. Phase B: 1080p30
composite as 2 s segments plus five track egresses. Phase C: 1080p30 composite
as one MP4 plus the same five track egresses. The public host is sampled from
the Mac at the same time; this script logs the phase times.
"""
import sys
import time

from harness import ACTIVE, Run


def main():
    run = Run(sys.argv[1])
    run.watch(stats_every=2.0)
    run.person("alice", "--leave-after", 275)
    run.person("bob", "--screen-after", 3, "--leave-after", 275)
    run.person("carol", "--leave-after", 275, screen=False)
    time.sleep(2)
    run.participant("viewer1", "--duration", 270, "--report-every", 10, mode="subscribe")
    run.participant("viewer2", "--duration", 270, "--report-every", 10, mode="subscribe")

    run.wait_until(15)
    run.log("phase", name="A no recording", begin=True)
    run.wait_until(75)
    run.log("phase", name="A no recording", begin=False)

    segments = run.start_composite("", preset="H264_1080P_30", mp4=False, segments=2, tag="segments")
    run.recording = True
    run.wait_until(92)
    run.log("phase", name="B segments + 5 track files", begin=True)
    run.wait_until(165)
    run.log("phase", name="B segments + 5 track files", begin=False)
    run.api("Egress", "StopEgress", {"egress_id": segments}, timeout=20)

    run.wait_until(172)
    run.start_composite("", preset="H264_1080P_30", mp4=True, segments=0, tag="mp4")
    run.wait_until(188)
    run.log("phase", name="C MP4 + 5 track files", begin=True)
    run.wait_until(255)
    run.log("phase", name="C MP4 + 5 track files", begin=False)
    run.recording = False
    run.stop_all_egress()
    run.finish()


if __name__ == "__main__":
    main()
