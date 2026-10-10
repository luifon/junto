#!/usr/bin/env python3
"""A 45 s recorded call, used as the last check of the arrangement.  call_short.py ROOM"""
import json
import subprocess
import sys
from pathlib import Path

from harness import Run

CHECK = str(Path(__file__).with_name("check_recording.py"))


def main():
    run = Run(sys.argv[1])
    run.watch(stats_every=5.0)
    run.person("alice", "--leave-after", 60)
    run.person("bob", "--screen-after", 3, "--leave-after", 60)
    run.wait_until(8)
    run.start_composite("", preset="H264_1080P_30", mp4=False, segments=2)
    run.recording = True
    run.wait_until(48)
    run.recording = False
    run.stop_all_egress()
    run.finish()
    final = json.loads((run.out / "egress-final.json").read_text())["items"]
    for item in final:
        for result in item.get("file_results", []) + item.get("segment_results", []):
            name = result.get("filename") or result.get("playlist_name")
            path = Path("/work") / name.lstrip("/")
            extra = ["--audio-only"] if path.suffix == ".ogg" else []
            report = json.loads(subprocess.run(["python3", CHECK, str(path), *extra], capture_output=True, text=True).stdout)
            print(item["egress_id"], item["status"], path.name, report["seconds"], "ok" if report["ok"] else report["problems"])


if __name__ == "__main__":
    main()
