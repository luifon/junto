"""Shared pieces of the test calls: logging, participants, egress requests, watchers.

Runs inside the tools container on the data host. Paths: /work is the spike's
directory on the data host; Egress sees /work/out as /out.
"""
import json
import os
import subprocess
import threading
import time
from pathlib import Path

from lkapi import ENV, call

ROOT = Path("/work")
PARTICIPANT = ROOT / "build" / "target" / "release" / "test-participant"
EGRESS = "junto-spike-rec-egress"
LINK = "junto-spike-rec-link"
ACTIVE = ("EGRESS_STARTING", "EGRESS_ACTIVE", "EGRESS_ENDING")
MARKER = ["--marker-period-ms", 10000, "--marker-gap-ms", 300]
PEOPLE = {
    "alice": ["--mic-hz", 440, "--screen-audio-hz", 1000, "--screen-color", "2048c8"],
    "bob": ["--mic-hz", 620, "--screen-audio-hz", 1370, "--screen-color", "c83030"],
    "carol": ["--mic-hz", 790, "--screen-audio-hz", 1730, "--screen-color", "189046"],
}


def cpu_pressure():
    """Share of the last 10 s in which some task on the host waited for a CPU (percent)."""
    try:
        text = Path("/proc/pressure/cpu").read_text()
        return float(text.split("avg10=")[1].split()[0])
    except (OSError, IndexError, ValueError):
        return None


def wait_quiet(threshold=6.0, max_wait=900):
    """Waits until the host's CPU pressure stays under the threshold for 10 s. Returns the waiting time."""
    began = time.time()
    calm = 0
    while time.time() - began < max_wait:
        value = cpu_pressure()
        calm = calm + 1 if value is not None and value < threshold else 0
        if calm >= 2:
            break
        time.sleep(5)
    return round(time.time() - began, 1), cpu_pressure(), round(os.getloadavg()[0], 1)


def docker(*args, timeout=120):
    started = time.time()
    result = subprocess.run(["docker", *args], capture_output=True, text=True, timeout=timeout)
    return {"args": list(args), "began_unix": round(started, 3), "ended_unix": round(time.time(), 3),
            "exit": result.returncode, "stdout": result.stdout.strip()[-2000:], "stderr": result.stderr.strip()[-500:]}


class Run:
    def __init__(self, room):
        self.room = room
        self.out = ROOT / "out" / room
        for directory in (self.out, self.out / "tracks", self.out / "hls"):
            directory.mkdir(parents=True, exist_ok=True)
            os.chmod(directory, 0o777)
        self.started = time.time()
        self.log_file = open(self.out / "controller.jsonl", "w")
        self.lock = threading.Lock()
        self.processes = []
        self.recording = False
        self.track_egress = {}
        self.seen_egress = {}
        self.seen_people = None
        self.stop_threads = threading.Event()

    def log(self, kind, **fields):
        now = time.time()
        line = {"t": round(now - self.started, 3), "unix_ms": int(now * 1000), "kind": kind, **fields}
        with self.lock:
            self.log_file.write(json.dumps(line) + "\n")
            self.log_file.flush()
        return line

    def wait_until(self, t):
        time.sleep(max(0.0, self.started + t - time.time()))

    def api(self, service, method, body, timeout=30):
        began = time.time()
        status, reply = call(service, method, body, timeout=timeout)
        self.log("api", service=service, method=method, request=body, status=status, reply=reply,
                 seconds=round(time.time() - began, 2))
        return status, reply

    def docker(self, *args):
        result = docker(*args)
        self.log("docker", **result)
        return result

    def participant(self, identity, *args, mode="publish"):
        log = open(self.out / f"{identity}.jsonl", "a")
        command = [str(PARTICIPANT), mode, "--room", self.room, "--identity", identity, *map(str, args)]
        process = subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT, env=ENV)
        self.processes.append(process)
        self.log("participant_started", identity=identity, mode=mode, args=[str(a) for a in args])
        return process

    def person(self, name, *extra, camera=True, screen=True):
        args = list(PEOPLE[name]) + MARKER
        if camera:
            args.append("--camera")
        if screen:
            args.append("--screen")
        else:
            index = args.index("--screen-audio-hz")
            del args[index:index + 4]
        return self.participant(name, *args, *extra)

    def start_composite(self, layout="debug", preset="H264_720P_30", mp4=True, segments=0, tag="composite"):
        body = {"room_name": self.room, "layout": layout, "custom_base_url": "http://layout/", "preset": preset}
        if mp4:
            body["file_outputs"] = [{"file_type": "MP4", "filepath": f"/out/{self.room}/{tag}-{{time}}.mp4"}]
        if segments:
            body["segment_outputs"] = [{
                "filename_prefix": f"/out/{self.room}/hls/{tag}",
                "playlist_name": f"{tag}.m3u8",
                "live_playlist_name": f"{tag}-live.m3u8",
                "segment_duration": segments,
            }]
        status, reply = self.api("Egress", "StartRoomCompositeEgress", body)
        return reply.get("egress_id") if status == 200 else None

    def start_track_egress(self, track_sid, label):
        status, reply = self.api("Egress", "StartTrackEgress", {
            "room_name": self.room,
            "track_id": track_sid,
            "file": {"filepath": f"/out/{self.room}/tracks/{label}-{track_sid}-{{time}}"},
        })
        return reply.get("egress_id") if status == 200 else None

    def start_audio_composite(self, track_sid, label, segments=0, file_type="OGG"):
        body = {"room_name": self.room, "audio_track_id": track_sid}
        if segments:
            body["segment_outputs"] = [{
                "filename_prefix": f"/out/{self.room}/hls/{label}",
                "playlist_name": f"{label}.m3u8",
                "live_playlist_name": f"{label}-live.m3u8",
                "segment_duration": segments,
            }]
        else:
            body["file_outputs"] = [{"file_type": file_type,
                                     "filepath": f"/out/{self.room}/tracks/{label}-tc-{{time}}"}]
        status, reply = self.api("Egress", "StartTrackCompositeEgress", body)
        return reply.get("egress_id") if status == 200 else None

    def people(self):
        status, reply = call("RoomService", "ListParticipants", {"room": self.room})
        if status != 200:
            return None
        return reply.get("participants", [])

    def audio_tracks(self, participants):
        found = []
        for participant in participants or []:
            if participant.get("kind", "STANDARD") != "STANDARD":
                continue
            for track in participant.get("tracks", []):
                if track.get("type") == "AUDIO":
                    found.append((participant["identity"], track["source"].lower(), track["sid"]))
        return found

    def watch(self, stats_every=2.0, interval=1.0):
        """Logs every change in ListParticipants and ListEgress; starts track egresses while recording."""
        def loop():
            while not self.stop_threads.is_set():
                participants = self.people()
                view = None if participants is None else sorted(
                    (p["identity"], p.get("kind", "STANDARD"), p.get("state", ""), len(p.get("tracks", [])))
                    for p in participants)
                if view != self.seen_people:
                    self.seen_people = view
                    self.log("participants_change", participants=view)
                if self.recording:
                    for identity, source, sid in self.audio_tracks(participants):
                        if sid not in self.track_egress:
                            self.track_egress[sid] = "starting"
                            self.log("new_audio_track", identity=identity, source=source, track_sid=sid)
                            self.track_egress[sid] = self.start_track_egress(sid, f"{identity}-{source}")
                self.poll_egress()
                self.stop_threads.wait(interval)

        def stats_loop():
            while not self.stop_threads.is_set():
                result = subprocess.run(["docker", "stats", "--no-stream", "--format", "{{json .}}", EGRESS],
                                        capture_output=True, text=True)
                rows = [json.loads(line) for line in result.stdout.splitlines() if line.startswith("{")]
                self.log("docker_stats", host_load_1m=round(os.getloadavg()[0], 1), cpu_pressure_10s=cpu_pressure(),
                         active_egress=sum(1 for s in self.seen_egress.values() if s["status"] in ACTIVE),
                         rows=[{"name": r["Name"], "cpu": r["CPUPerc"], "mem": r["MemUsage"]} for r in rows])
                self.stop_threads.wait(stats_every)

        threading.Thread(target=loop, daemon=True).start()
        if stats_every:
            threading.Thread(target=stats_loop, daemon=True).start()

    def poll_egress(self):
        status, reply = call("Egress", "ListEgress", {"room_name": self.room})
        if status != 200:
            if self.seen_egress.get("_failed") != status:
                self.seen_egress["_failed"] = status
                self.log("list_egress_failed", status=status, reply=reply)
            return []
        self.seen_egress.pop("_failed", None)
        items = reply.get("items", [])
        for item in items:
            results = item.get("file_results", []) + item.get("segment_results", [])
            view = {
                "status": item.get("status"),
                "started_at": item.get("started_at"),
                "ended_at": item.get("ended_at"),
                "updated_at": item.get("updated_at"),
                "error": item.get("error"),
                "details": item.get("details"),
                "results": [{k: r.get(k) for k in ("started_at", "ended_at", "segment_count", "filename", "playlist_name")}
                            for r in results],
            }
            if self.seen_egress.get(item["egress_id"]) != view:
                self.seen_egress[item["egress_id"]] = view
                self.log("egress_change", egress_id=item["egress_id"], **view)
        return items

    def stop_all_egress(self, timeout=45):
        for item in self.poll_egress():
            if item.get("status") in ACTIVE:
                self.api("Egress", "StopEgress", {"egress_id": item["egress_id"]}, timeout=10)
        deadline = time.time() + timeout
        while time.time() < deadline:
            if all(item.get("status") not in ACTIVE for item in self.poll_egress()):
                break
            time.sleep(1.0)

    def save_egress_log(self, name="egress.log", since=None):
        since = since or time.strftime("%Y-%m-%dT%H:%M:%S", time.gmtime(self.started - 2)) + "Z"
        result = subprocess.run(["docker", "logs", "--since", since, EGRESS], capture_output=True, text=True)
        text = result.stdout + result.stderr
        import re
        text = re.sub(r"token=[A-Za-z0-9_.\-]+", "token=<removed>", text)
        text = re.sub(r"eyJ[A-Za-z0-9_\-]+\.[A-Za-z0-9_\-]+\.[A-Za-z0-9_\-]+", "<token removed>", text)
        text = re.sub(r"\b\d{1,3}(\.\d{1,3}){3}\b", "<address>", text)
        text = re.sub(r"\b[0-9a-f]{1,4}(:[0-9a-f]{0,4}){4,7}\b", "<address>", text)
        (self.out / name).write_text(text)

    def finish(self, wait=30):
        self.stop_threads.set()
        for process in self.processes:
            try:
                process.wait(timeout=wait)
            except subprocess.TimeoutExpired:
                process.terminate()
        _, reply = call("Egress", "ListEgress", {"room_name": self.room})
        (self.out / "egress-final.json").write_text(json.dumps(reply, indent=1))
        self.save_egress_log()
        self.log("finished")
        self.log_file.close()
