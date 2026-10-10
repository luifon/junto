#!/usr/bin/env python3
"""A crash of the Egress process with a restart policy and the rescue entrypoint (gates X3, X4).

  call_rescue.py ROOM

Needs the Egress container started with EGRESS_ENTRYPOINT=/rescue-entrypoint.sh
and EGRESS_RESTART=unless-stopped. Kills the Egress service process inside the
container (SIGKILL), so the container ends by itself and Docker starts it
again. Logs when the health port answers again, what the entrypoint copied to
out/rescued/, and starts a new recording.
"""
import json
import subprocess
import sys
import time

from harness import ACTIVE, EGRESS, ROOT, Run

FIND_SERVICE = ("for p in /proc/[0-9]*; do [ \"$(cat $p/comm 2>/dev/null)\" = egress ] && echo ${p#/proc/}; done"
                " | sort -n | head -1")


def health():
    result = subprocess.run(["curl", "-s", "-m", "1", "-o", "/dev/null", "-w", "%{http_code}", "http://egress:9090/"],
                            capture_output=True, text=True)
    return result.stdout


def main():
    run = Run(sys.argv[1])
    rescued = ROOT / "out" / "rescued"
    subprocess.run(["rm", "-rf", str(rescued)])
    run.watch(stats_every=5.0)
    run.person("alice", "--leave-after", 150)
    run.person("bob", "--screen-after", 3, "--leave-after", 150)
    run.wait_until(8)
    run.start_composite("", preset="H264_1080P_30", mp4=False, segments=2, tag="before-crash")
    run.recording = True

    run.wait_until(50)
    run.recording = False
    before = [i["egress_id"] for i in run.poll_egress() if i["status"] in ACTIVE]
    state = run.docker("inspect", "-f", "{{.RestartCount}} {{.State.StartedAt}}", EGRESS)["stdout"]
    pid = subprocess.run(["docker", "exec", EGRESS, "sh", "-c", FIND_SERVICE], capture_output=True, text=True).stdout.strip()
    crashed = time.time()
    run.docker("exec", EGRESS, "sh", "-c", f"kill -9 {pid}")
    run.log("crashed", service_pid=pid, unix=round(crashed, 3), state_before=state)
    down_seen, up_at = False, None
    while time.time() - crashed < 60:
        code = health()
        if code != "200":
            down_seen = True
        elif down_seen:
            up_at = time.time()
            break
        time.sleep(0.2)
    state = run.docker("inspect", "-f", "{{.RestartCount}} {{.State.StartedAt}} {{.State.Status}}", EGRESS)["stdout"]
    run.log("egress_answers_again", seconds_after_crash=round(up_at - crashed, 2) if up_at else None, state_after=state)
    listing = subprocess.run(["find", str(rescued), "-type", "f", "-printf", "%P %s\n"], capture_output=True, text=True).stdout
    run.log("rescued_files", files=sorted(listing.splitlines()))
    left = subprocess.run(["find", str(ROOT / "egress-tmp"), "-type", "f", "-not", "-path", "*chromedp*", "-not", "-path", "*Chrome*"],
                          capture_output=True, text=True).stdout.splitlines()
    run.log("working_directory_after_restart", files=left)

    run.track_egress = {}
    new_composite = run.start_composite("", preset="H264_1080P_30", mp4=False, segments=2, tag="after-crash")
    run.log("new_composite", egress_id=new_composite, seconds_after_crash=round(time.time() - crashed, 2))
    run.recording = True
    run.wait_until(120)
    run.recording = False
    for item in run.poll_egress():
        if item.get("status") in ACTIVE and item["egress_id"] not in before:
            run.api("Egress", "StopEgress", {"egress_id": item["egress_id"]}, timeout=10)
    time.sleep(6)
    run.finish()

    kept = run.out / "rescued-by-entrypoint"
    subprocess.run(["mv", str(rescued), str(kept)])
    rescued = kept
    report = []
    for path in sorted(rescued.rglob("*.ogg")):
        probe = subprocess.run(["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", str(path)],
                               capture_output=True, text=True)
        decode = subprocess.run(["ffmpeg", "-v", "error", "-nostdin", "-i", str(path), "-f", "null", "-"], capture_output=True, text=True)
        report.append({"file": str(path.relative_to(rescued)), "bytes": path.stat().st_size, "seconds": probe.stdout.strip(),
                       "decode_errors": len(decode.stderr.strip().splitlines())})
    print(json.dumps(report, indent=1))


if __name__ == "__main__":
    main()
