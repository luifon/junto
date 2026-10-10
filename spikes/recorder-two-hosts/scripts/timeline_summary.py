#!/usr/bin/env python3
"""One line per recorded file with its offsets on the timeline (gate X6).

  timeline_summary.py ROOM [ROOM ...]

"late" is: reported start time + position of a time marker in the file - true
time of the marker. A positive value means that the file's content is later in
the file than the reported start time says.
"""
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent / "out"
if not ROOT.exists():
    ROOT = Path("/work/out")


def main():
    rows = []
    for room in sys.argv[1:]:
        analysis = json.loads((ROOT / room / "analysis.json").read_text())
        align_path = ROOT / room / "align.json"
        align = json.loads(align_path.read_text()) if align_path.exists() else {"whole": [], "windows": []}
        for composite in analysis["composites"]:
            for kind in ("mp4", "segments"):
                part = composite.get(kind)
                if not part or "missing" in part or not part.get("audio_marker_spread"):
                    continue
                audio, video = part["audio_marker_spread"], part.get("video_marker_spread") or {}
                longer = part["audio_seconds"] - part["reported_duration_s"]
                whole = [w for w in align["whole"] if w["egress_id"] == composite["egress_id"] and w["composite"] == kind]
                windows = [w["error_ms"] for w in align.get("windows", []) if w["error_ms"] is not None]
                rows.append({
                    "room": room, "file": f"composite {kind}", "markers": audio["count"],
                    "sound_late_s": audio["median"], "sound_late_range": [audio["min"], audio["max"]], "outliers": audio["outliers"],
                    "picture_late_s": video.get("median"),
                    "media_longer_than_reported_s": round(longer, 3),
                    "late_minus_longer_s": round(audio["median"] - longer, 3),
                    "sound_alignment_error_ms": [w["error_of_method_ms"] for w in whole],
                    "start_error_after_alignment_ms": [w["composite_start_error_on_wall_clock_ms"] for w in whole],
                    "window_alignment_error_ms": [min(windows), max(windows)] if windows and kind == "mp4" else None,
                })
        for track in analysis["tracks"]:
            if track.get("marker_spread"):
                spread = track["marker_spread"]
                rows.append({"room": room, "file": "track " + track["track"], "markers": spread["count"],
                             "sound_late_s": spread["median"], "sound_late_range": [spread["min"], spread["max"]],
                             "media_longer_than_reported_s": round(track["media_seconds"] - track["reported_duration_s"], 3)})
    for row in rows:
        print(json.dumps(row))


if __name__ == "__main__":
    main()
