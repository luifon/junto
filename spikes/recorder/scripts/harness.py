"""Shared pieces of the recorded test calls: logging, participants, the egress watcher."""
import base64
import json
import os
import subprocess
import threading
import time
from pathlib import Path

from lkapi import ENV, call

ROOT = Path(__file__).resolve().parent.parent
PARTICIPANT = ROOT.parent / "test-participant" / "target" / "release" / "test-participant"
CONTAINERS = ["junto-spike-rec-egress", "junto-spike-rec-livekit"]
ACTIVE = ("EGRESS_STARTING", "EGRESS_ACTIVE", "EGRESS_ENDING")


class Run:
    def __init__(self, room):
        self.room = room
        self.out = ROOT / "out" / room
        self.out.mkdir(parents=True, exist_ok=True)
        (self.out / "tracks").mkdir(exist_ok=True)
        self.started = time.time()
        self.log_file = open(self.out / "controller.jsonl", "w")
        self.lock = threading.Lock()
        self.processes = []
        self.recording = False
        self.track_egress = {}
        self.seen = {}
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

    def api(self, service, method, body):
        status, reply = call(service, method, body)
        self.log("api", service=service, method=method, request=body, status=status, reply=reply)
        return status, reply

    def participant(self, identity, *args):
        log = open(self.out / f"{identity}.jsonl", "a")
        command = [str(PARTICIPANT), "publish", "--room", self.room, "--identity", identity, *map(str, args)]
        process = subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT, env=ENV)
        self.processes.append(process)
        self.log("participant_started", identity=identity, args=[str(a) for a in args])
        return process

    def start_composite(self, layout, preset="H264_720P_30"):
        _, reply = self.api("Egress", "StartRoomCompositeEgress", {
            "room_name": self.room,
            "layout": layout,
            "custom_base_url": "http://layout/",
            "preset": preset,
            "file_outputs": [{"file_type": "MP4", "filepath": f"/out/{self.room}/composite-{{time}}.mp4"}],
        })
        return reply.get("egress_id")

    def start_track_egress(self, track_sid, label):
        status, reply = self.api("Egress", "StartTrackEgress", {
            "room_name": self.room,
            "track_id": track_sid,
            "file": {"filepath": f"/out/{self.room}/tracks/{label}-{track_sid}-{{time}}"},
        })
        return reply.get("egress_id") if status == 200 else None

    def audio_tracks(self):
        status, reply = call("RoomService", "ListParticipants", {"room": self.room})
        found = []
        for participant in reply.get("participants", []) if status == 200 else []:
            if participant.get("kind", "STANDARD") != "STANDARD":
                continue
            for track in participant.get("tracks", []):
                if track.get("type") == "AUDIO":
                    found.append((participant["identity"], track["source"].lower(), track["sid"]))
        return found

    def watch(self, sample_stats=True):
        """Starts a track egress for every new audio track and logs every egress change."""
        def egress_loop():
            while not self.stop_threads.is_set():
                if self.recording:
                    for identity, source, sid in self.audio_tracks():
                        if sid not in self.track_egress:
                            self.track_egress[sid] = "starting"
                            self.log("new_audio_track", identity=identity, source=source, track_sid=sid)
                            self.track_egress[sid] = self.start_track_egress(sid, f"{identity}-{source}")
                self.poll_egress()
                self.stop_threads.wait(1.0)

        def stats_loop():
            while not self.stop_threads.is_set():
                result = subprocess.run(["docker", "stats", "--no-stream", "--format", "{{json .}}", *CONTAINERS],
                                        capture_output=True, text=True)
                rows = [json.loads(line) for line in result.stdout.splitlines() if line.startswith("{")]
                self.log("docker_stats", host_load_1m=round(os.getloadavg()[0], 1), active_egress=sum(1 for s in self.seen.values() if s["status"] in ACTIVE),
                         rows=[{"name": r["Name"], "cpu": r["CPUPerc"], "mem": r["MemUsage"]} for r in rows])
                self.stop_threads.wait(3.0)

        threading.Thread(target=egress_loop, daemon=True).start()
        if sample_stats:
            threading.Thread(target=stats_loop, daemon=True).start()

    def poll_egress(self):
        status, reply = call("Egress", "ListEgress", {"room_name": self.room})
        if status != 200:
            self.log("list_egress_failed", status=status, reply=reply)
            return []
        items = reply.get("items", [])
        for item in items:
            files = item.get("file_results", [])
            view = {
                "status": item.get("status"),
                "started_at": item.get("started_at"),
                "ended_at": item.get("ended_at"),
                "error": item.get("error"),
                "details": item.get("details"),
                "file_started_at": files[0].get("started_at") if files else None,
                "file_ended_at": files[0].get("ended_at") if files else None,
            }
            if self.seen.get(item["egress_id"]) != view:
                self.seen[item["egress_id"]] = view
                self.log("egress_change", egress_id=item["egress_id"], **view)
        return items

    def stop_all_egress(self, timeout=40):
        for item in self.poll_egress():
            if item.get("status") in ACTIVE:
                self.api("Egress", "StopEgress", {"egress_id": item["egress_id"]})
        deadline = time.time() + timeout
        while time.time() < deadline:
            if all(item.get("status") not in ACTIVE for item in self.poll_egress()):
                break
            time.sleep(1.0)

    def send_layout_data(self, payload):
        data = base64.b64encode(json.dumps(payload).encode()).decode()
        return self.api("RoomService", "SendData",
                        {"room": self.room, "data": data, "kind": "RELIABLE", "topic": "layout"})

    def finish(self):
        self.stop_threads.set()
        for process in self.processes:
            try:
                process.wait(timeout=30)
            except subprocess.TimeoutExpired:
                process.terminate()
        _, reply = call("Egress", "ListEgress", {"room_name": self.room})
        (self.out / "egress-final.json").write_text(json.dumps(reply, indent=1))
        self.log("finished")
        self.log_file.close()
