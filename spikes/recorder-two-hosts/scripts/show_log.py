#!/usr/bin/env python3
"""Prints the controller log of one test call as text.  show_log.py ROOM [kind ...]"""
import json
import sys
from pathlib import Path

ROOT = Path("/work")


def main():
    room = sys.argv[1]
    kinds = set(sys.argv[2:])
    rows = [json.loads(line) for line in (ROOT / "out" / room / "controller.jsonl").read_text().splitlines()]
    cpu = []
    for row in rows:
        kind = row["kind"]
        if kind == "docker_stats":
            if row["rows"]:
                cpu.append((float(row["rows"][0]["cpu"].rstrip("%")), row["rows"][0]["mem"], row["host_load_1m"], row["active_egress"], row["t"],
                            row.get("cpu_pressure_10s")))
            continue
        if kinds and kind not in kinds:
            continue
        t = f"{row['t']:8.2f}"
        if kind == "api":
            request = row["request"]
            target = request.get("track_id") or request.get("audio_track_id") or request.get("egress_id") or ""
            answer = "" if row["status"] == 200 else json.dumps(row["reply"])[:160]
            print(t, "api", row["method"], target, "->", row["status"], f"{row['seconds']} s", row["reply"].get("egress_id", "") if row["status"] == 200 else answer)
        elif kind == "egress_change":
            results = [(r.get("started_at"), r.get("ended_at"), r.get("segment_count")) for r in row["results"]]
            print(t, "egress", row["egress_id"], row["status"], "started", row["started_at"], "updated", row["updated_at"],
                  "ended", row["ended_at"], repr(row["error"]), repr(row["details"]), results)
        elif kind == "participants_change":
            print(t, "participants", row["participants"])
        elif kind == "docker":
            print(t, "docker", " ".join(row["args"]), "exit", row["exit"], f"{row['ended_unix'] - row['began_unix']:.2f} s", row["stderr"][:120])
        else:
            rest = {k: v for k, v in row.items() if k not in ("t", "unix_ms", "kind")}
            print(t, kind, json.dumps(rest)[:600])
    if cpu:
        values = [c[0] for c in cpu]
        print(f"egress container CPU: {len(values)} samples, mean {sum(values) / len(values):.0f} %, max {max(values):.0f} %, "
              f"memory {cpu[len(cpu) // 2][1]}, host load {min(c[2] for c in cpu)} to {max(c[2] for c in cpu)}")
        pressure = [c[5] for c in cpu if c[5] is not None]
        if pressure:
            print(f"host CPU pressure (some, 10 s): mean {sum(pressure) / len(pressure):.1f} %, max {max(pressure):.1f} %")


if __name__ == "__main__":
    main()
