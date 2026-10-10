#!/usr/bin/env python3
"""Rewrites phases.json of the first X2 run, in which carol stayed until the end of the call."""
import json
import sys
from pathlib import Path

path = Path("/work/out") / sys.argv[1] / "phases.json"
phases = json.loads(path.read_text())
kept = [p for p in phases if p["end_unix_ms"] > p["start_unix_ms"]]
for phase in kept:
    if phase["name"].startswith("UpdateLayout"):
        phase["present"] = [440, 620, 790, 1000]
        update_ms = phase["start_unix_ms"] - 3000
for phase in kept:
    if phase["name"].startswith("third share"):
        phase["end_unix_ms"] = update_ms - 1000
path.write_text(json.dumps(kept, indent=1))
