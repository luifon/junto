#!/usr/bin/env python3
"""Averages of one X7 measurement. Run on the Mac:  x7_summary.py ROOM

Joins the public host's samples (out/ROOM/public-host.txt, one line per 5 s,
stamped with Unix time) with the windows that the test script logged.
"""
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def windows(log):
    found = []
    opened = {}
    for row in log:
        if row["kind"] == "phase":
            if row["begin"]:
                opened[row["name"]] = row["unix_ms"] / 1000.0
            else:
                found.append((row["name"], opened.pop(row["name"]), row["unix_ms"] / 1000.0))
    load = [r for r in log if r["kind"] == "load_started"]
    if load:
        start = load[0]["unix_ms"] / 1000.0
        end = [r for r in log if r["kind"] == "window_end"][0]["unix_ms"] / 1000.0
        active = [r for r in log if r["kind"] == "egress_change" and r["status"] == "EGRESS_ACTIVE"]
        if active:
            found.append(("load with recording active", active[0]["unix_ms"] / 1000.0 + 8.0, end))
        else:
            found.append(("load without recording", start + 20.0, end))
    return found


def main():
    room = sys.argv[1]
    out = ROOT / "out" / room
    log = [json.loads(line) for line in (out / "controller.jsonl").read_text().splitlines()]
    samples = []
    for line in (out / "public-host.txt").read_text().splitlines():
        parts = line.split()
        if len(parts) == 12 and parts[0].isdigit():
            samples.append([float(p) for p in parts])
    stats = [(r["unix_ms"] / 1000.0, float(r["rows"][0]["cpu"].rstrip("%")), r["rows"][0]["mem"].split(" / ")[0],
              r.get("cpu_pressure_10s"), r["host_load_1m"]) for r in log if r["kind"] == "docker_stats" and r["rows"]]
    summary = []
    for name, begin, end in windows(log):
        inside = [s for s in samples if begin + 5 <= s[0] <= end]
        egress = [s for s in stats if begin <= s[0] <= end]
        entry = {"window": name, "seconds": round(end - begin, 1), "public_host_samples": len(inside)}
        if inside:
            mean = lambda index: sum(s[index] for s in inside) / len(inside)
            entry.update({
                "livekit_cpu_percent_mean": round(mean(1), 1), "livekit_cpu_percent_max": max(s[1] for s in inside),
                "host_busy_percent_mean": round(100 - mean(6), 1), "host_busy_percent_max": 100 - min(s[6] for s in inside),
                "host_steal_percent_mean": round(mean(5), 1), "livekit_memory_mib_max": max(s[7] for s in inside),
                "host_rx_mbit_mean": round(mean(9), 1), "host_tx_mbit_mean": round(mean(10), 1),
                "udp_buffer_errors": int(sum(s[11] for s in inside)),
            })
        if egress:
            cpu = [s[1] for s in egress]
            pressure = [s[3] for s in egress if s[3] is not None]
            entry.update({"egress_cpu_percent_mean": round(sum(cpu) / len(cpu), 0), "egress_cpu_percent_max": max(cpu),
                          "egress_memory_max": max((s[2] for s in egress), key=lambda m: float(m.rstrip("GMiB")) * (1024 if "GiB" in m else 1)),
                          "data_host_cpu_pressure_mean": round(sum(pressure) / len(pressure), 1) if pressure else None,
                          "data_host_load_1m": [min(s[4] for s in egress), max(s[4] for s in egress)]})
        summary.append(entry)
    generator = out / "loadgen.txt"
    if generator.exists():
        total = [line for line in generator.read_text().splitlines() if "Total" in line]
        summary.append({"load_generator_total": total[-1].strip() if total else "no total line"})
    (out / "x7-summary.json").write_text(json.dumps(summary, indent=1))
    print(json.dumps(summary, indent=1))


if __name__ == "__main__":
    main()
