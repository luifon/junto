#!/usr/bin/env python3
"""Prints the controller log of a recorded call (out/<room>/controller.jsonl) as text."""
import json
import sys
from pathlib import Path

room = sys.argv[1]
out = Path(__file__).resolve().parent.parent / "out" / room
for line in (out / "controller.jsonl").read_text().splitlines():
    e = json.loads(line)
    kind, at = e["kind"], f'{e["t"]:8.2f} {e["unix_ms"] / 1000:.3f}'
    if kind == "docker_stats":
        continue
    if kind == "api":
        reply = e["reply"]
        print(at, "API", e["method"], json.dumps(e["request"])[:110], "->", e["status"],
              reply.get("egress_id", ""), reply.get("status", ""), reply.get("msg", ""))
    elif kind == "egress_change":
        print(at, "EGRESS", e["egress_id"], e["status"], "ended_at", e["ended_at"], "file", e["file_started_at"],
              "->", e["file_ended_at"], "|", e["error"], "|", e["details"])
    else:
        print(at, kind, json.dumps({k: v for k, v in e.items() if k not in ("t", "unix_ms", "kind")})[:600])
if (out / "files.json").exists():
    for f in json.loads((out / "files.json").read_text()):
        print("FILE", f)
