#!/usr/bin/env python3
"""Polls the graph and logs the state of every audio sink other than the test sink,
and every link whose input is such a sink. Stops when the stop file exists."""
import json
import os
import subprocess
import sys
import time

TEST_SINK = "junto-spike-sink"
out_path, stop_path = sys.argv[1], sys.argv[2]
polls = 0
non_suspended = 0
links_seen = 0
last = None
with open(out_path, "w") as out:
    while not os.path.exists(stop_path):
        try:
            objects = json.loads(subprocess.run(["pw-dump"], capture_output=True, timeout=10).stdout)
        except Exception as error:  # noqa: BLE001
            out.write(f"{time.time():.3f} poll-error {error}\n")
            time.sleep(0.3)
            continue
        polls += 1
        real = {}
        for o in objects:
            info = o.get("info") or {}
            props = info.get("props") or {}
            if o["type"].endswith(":Node") and props.get("media.class") == "Audio/Sink" and props.get("node.name") != TEST_SINK:
                real[o["id"]] = info.get("state")
        links = [
            (o["info"]["output-node-id"], o["info"]["input-node-id"])
            for o in objects
            if o["type"].endswith(":Link") and o["info"]["input-node-id"] in real
        ]
        if any(state != "suspended" for state in real.values()):
            non_suspended += 1
        if links:
            links_seen += 1
        snapshot = (sorted(real.items()), sorted(links))
        if snapshot != last:
            out.write(f"{time.time():.3f} real-sinks={sorted(real.items())} links-into-real-sinks={sorted(links)}\n")
            out.flush()
            last = snapshot
        time.sleep(0.25)
    out.write(f"SUMMARY polls={polls} polls_with_non_suspended_real_sink={non_suspended} polls_with_link_into_real_sink={links_seen}\n")
