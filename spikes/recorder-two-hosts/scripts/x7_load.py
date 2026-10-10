#!/usr/bin/env python3
"""A load point from the capacity test, with or without a recording (gate X7, second part).

  x7_load.py LABEL a40|b16 rec|norec [LAYOUT|-] [SECONDS]

a40: one 2.5 Mbit/s H.264 stream (file published by the lk tool) to 40 subscribers.
b16: 16 participants publish simulcast video, 16 subscribers take them in a 4x4 grid.
The load generators are containers on the data host. With "rec", a 1080p30
composite recording (2 s segments) of the room starts 12 s after the load.
The public host is sampled from the Mac at the same time.
"""
import subprocess
import sys
import time

from harness import ACTIVE, Run, docker

SECONDS = 80
CLI = "livekit/livekit-cli:v2.18.8"
HOST_DIR = "/srv/junto-spike-rec"


def main():
    label, point, mode = sys.argv[1], sys.argv[2], sys.argv[3]
    layout = sys.argv[4] if len(sys.argv) > 4 and sys.argv[4] != "-" else ""
    seconds = int(sys.argv[5]) if len(sys.argv) > 5 else SECONDS
    room = f"rth-load-{label}"
    run = Run(room)
    run.watch(stats_every=2.0, interval=2.0)
    common = ["--env-file", "/work/.env"]
    docker("rm", "-f", "junto-spike-rec-loadpub", "junto-spike-rec-loadgen")
    if point == "a40":
        run.log("docker", **docker("run", "-d", "--name", "junto-spike-rec-loadpub", "--cpus", "1", "--memory", "512m", *common,
                                   "-v", f"{HOST_DIR}/media:/media:ro", CLI, "room", "join", "--identity", "screen-publisher",
                                   "--publish", "/media/screen-1080p15.h264", "--fps", "15", room))
        time.sleep(3)
        options = ["--subscribers", "40"]
    else:
        options = ["--video-publishers", "16", "--subscribers", "16", "--layout", "4x4"]
    run.log("load_started", options=options)
    generator = subprocess.Popen(["docker", "run", "--rm", "--name", "junto-spike-rec-loadgen", "--cpus", "4", "--memory", "4g", *common,
                                  CLI, "load-test", "--room", room, "--identity-prefix", "dh", "--duration", f"{seconds}s", *options],
                                 stdout=open(run.out / "loadgen.txt", "w"), stderr=subprocess.STDOUT)
    began = time.time()
    egress_id = None
    if mode == "rec":
        time.sleep(12)
        egress_id = run.start_composite(layout, preset="H264_1080P_30", mp4=False, segments=2, tag="load")
        run.log("recording_requested", egress_id=egress_id, layout=layout)
    time.sleep(max(0.0, began + seconds - 6 - time.time()))
    run.log("window_end")
    if egress_id:
        run.api("Egress", "StopEgress", {"egress_id": egress_id}, timeout=20)
    generator.wait(timeout=120)
    docker("rm", "-f", "junto-spike-rec-loadpub")
    time.sleep(4)
    run.stop_all_egress(timeout=20)
    run.finish(wait=5)


if __name__ == "__main__":
    main()
