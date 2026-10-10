#!/usr/bin/env python3
"""Continuity of the call and of the recorded files across the link outage.

  analyze_link.py ROOM

Reads the subscriber's per-second reports (out/ROOM/probe.jsonl) and prints,
for the seconds around the outage, how many audio and video frames arrived.
Then runs check_recording.py on every recorded file.
"""
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path("/work")


def main():
    room = sys.argv[1]
    out = ROOT / "out" / room
    log = [json.loads(line) for line in (out / "controller.jsonl").read_text().splitlines()]
    stops = [e for e in log if e["kind"] == "docker" and e["args"][0] in ("stop", "start", "kill")]
    down, up = stops[0]["began_unix"], stops[-1]["ended_unix"]
    reports = [json.loads(line) for line in (out / "probe.jsonl").read_text().splitlines() if '"event":"report"' in line]
    previous = {}
    worst = {}
    rows = []
    for index, report in enumerate(reports):
        now = report.get("unix_ms", 0) / 1000.0 if "unix_ms" in report else None
        row = {}
        for track in report["tracks"]:
            key = f"{track['participant']}/{track['source']}"
            count = track["audio_frames"] if track["kind"] == "audio" else track["video_frames"]
            if key in previous:
                delta = count - previous[key]
                row[key] = delta
                if index > 3:
                    worst[key] = min(worst.get(key, delta), delta)
            previous[key] = count
        rows.append((report.get("t"), now, row))
    summary = {"link_down_unix": down, "link_up_unix": up, "reports": len(reports),
               "fewest_frames_in_one_report_interval": worst}
    during = [r for r in rows if r[1] and down - 3 <= r[1] <= up + 5]
    print(json.dumps(summary, indent=1))
    for t, now, row in during[::3]:
        print(f"  t={t:7.2f} {'DOWN' if down <= now <= up else '    '} "
              + " ".join(f"{k.split('/')[0][:1]}/{k.split('/')[1][:9]}={v}" for k, v in sorted(row.items())))

    final = json.loads((out / "egress-final.json").read_text()).get("items", [])
    checks = []
    for item in final:
        for result in item.get("file_results", []) + item.get("segment_results", []):
            name = result.get("filename") or result.get("playlist_name")
            path = ROOT / name.lstrip("/")
            if not path.exists():
                checks.append({"file": name, "missing": True})
                continue
            command = ["python3", str(Path(__file__).with_name("check_recording.py")), str(path)]
            if path.suffix == ".ogg":
                command.append("--audio-only")
            answer = subprocess.run(command, capture_output=True, text=True)
            report = json.loads(answer.stdout)
            checks.append({"egress_id": item["egress_id"], "status": item["status"], "details": item.get("details"),
                           "file": path.name, "seconds": report["seconds"], "ok": report["ok"], "problems": report["problems"],
                           "sound": report.get("sound", {}).get("silent_ranges"), "check_seconds": report["check_seconds"]})
    for check in checks:
        print(json.dumps(check))
    (out / "link-analysis.json").write_text(json.dumps({"summary": summary, "checks": checks}, indent=1))


if __name__ == "__main__":
    main()
