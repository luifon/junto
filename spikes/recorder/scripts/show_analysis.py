#!/usr/bin/env python3
"""Prints the analysis of a recorded call (out/<room>/analysis.json) as text."""
import json
import sys
from pathlib import Path

room = sys.argv[1] if len(sys.argv) > 1 else "call-a"
d = json.loads((Path(__file__).resolve().parent.parent / "out" / room / "analysis.json").read_text())
print("composite", d["composite"])
for p in d["phases"]:
    print(f'{p["phase"]}. {p["name"]} file {p["file_seconds"]} s, expected {p["expected_present_hz"]} -> {p["verdict"]}')
    print("     median/p95 dBFS:", {f: (v["median"], v["p95"]) for f, v in p["levels_dbfs"].items()})
print("switches", d["switch_file_seconds_after_call"])
for t in d["tracks"]:
    print(t["file"], "|", t["container"], t["codec"], f'{t["channels"]}ch', t["kbit_s"], "kbit/s | media",
          t["media_seconds"], "s, wall", t["wall_seconds"], "s |", t["expected_hz"], "Hz at", t["expected_tone_dbfs"],
          "dBFS, strongest other", t["strongest_other_dbfs"], "|", t["verdict"], "|", t["status"], t["details"])
for name, value in d["alignment_summary"].items():
    print(name, value)
