#!/usr/bin/env python3
"""State recovery test (gate R4).

  call_b.py ROOM egress-restart   docker restart of the Egress container (SIGTERM, 10 s, SIGKILL)
  call_b.py ROOM egress-kill      docker kill of the Egress container (SIGKILL), then docker start
  call_b.py ROOM egress-term      SIGTERM to the Egress container without a forced kill
  call_b.py ROOM livekit-restart  docker restart of the LiveKit container

A composite recording and one track egress per audio track run when the
container is restarted. Afterwards the controller starts whatever is missing.
"""
import json
import subprocess
import sys
import time

from harness import ACTIVE, ROOT, Run

EGRESS = "junto-spike-rec-egress"
LIVEKIT = "junto-spike-rec-livekit"


def docker(run, *args):
    started = time.time()
    result = subprocess.run(["docker", *args], capture_output=True, text=True)
    run.log("docker", args=list(args), seconds=round(time.time() - started, 2), exit=result.returncode)


def container_state(run, name):
    result = subprocess.run(["docker", "inspect", "-f", "{{.State.Status}}", name], capture_output=True, text=True)
    run.log("container_state", container=name, state=result.stdout.strip())
    return result.stdout.strip()


def probe(path):
    result = subprocess.run(["ffprobe", "-v", "error", "-show_entries", "format=duration,size:stream=codec_name",
                             "-of", "json", str(path)], capture_output=True, text=True)
    decode = subprocess.run(["ffmpeg", "-v", "error", "-nostdin", "-i", str(path), "-f", "null", "-"],
                            capture_output=True, text=True)
    info = json.loads(result.stdout or "{}")
    return {
        "file": str(path.relative_to(ROOT / "out")),
        "bytes": path.stat().st_size,
        "ffprobe_ok": result.returncode == 0 and bool(info.get("streams")),
        "codecs": [s.get("codec_name") for s in info.get("streams", [])],
        "duration": info.get("format", {}).get("duration"),
        "decodes_without_error": decode.returncode == 0 and not decode.stderr.strip(),
        "first_error": (result.stderr or decode.stderr).strip().splitlines()[:1],
    }


def main():
    room, mode = sys.argv[1], sys.argv[2]
    run = Run(room)
    run.watch(sample_stats=False)
    common = ["--marker-period-ms", 10000, "--camera", "--screen", "--leave-after", 135]
    run.participant("alice", "--mic-hz", 440, "--screen-audio-hz", 1000, "--screen-color", "2048c8", *common)
    run.participant("bob", "--mic-hz", 620, "--screen-audio-hz", 1370, "--screen-color", "c83030", *common)

    run.wait_until(10)
    run.start_composite("debug")
    run.recording = True

    run.wait_until(45)
    if mode == "egress-restart":
        docker(run, "restart", EGRESS)
    elif mode == "egress-kill":
        docker(run, "kill", EGRESS)
        time.sleep(5)
        docker(run, "start", EGRESS)
    elif mode == "egress-term":
        docker(run, "kill", "-s", "TERM", EGRESS)
    elif mode == "livekit-restart":
        docker(run, "restart", LIVEKIT)
    else:
        sys.exit(__doc__)

    run.wait_until(75)
    container_state(run, EGRESS)
    items = run.poll_egress()
    run.log("list_egress_30s_after", items=[{k: i.get(k) for k in ("egress_id", "status", "error", "details", "ended_at")}
                                            for i in items])
    composite_active = any("room_composite" in i and i["status"] in ACTIVE for i in items)
    if not composite_active:
        run.log("controller_restarts_composite")
        run.start_composite("debug")
    recorded = {i["track"]["track_id"] for i in items if "track" in i and i["status"] in ACTIVE}
    for sid in list(run.track_egress):
        if sid not in recorded:
            run.log("controller_forgets_track_egress", track_sid=sid)
            del run.track_egress[sid]

    run.wait_until(120)
    run.recording = False
    run.stop_all_egress()
    time.sleep(3)
    if container_state(run, EGRESS) != "running":
        docker(run, "start", EGRESS)
    run.finish()
    files = sorted(p for p in run.out.rglob("*") if p.suffix in (".mp4", ".ogg", ".tmp") or "tmp" in p.name)
    report = [probe(p) for p in files if p.is_file()]
    (run.out / "files.json").write_text(json.dumps(report, indent=1))


if __name__ == "__main__":
    main()
