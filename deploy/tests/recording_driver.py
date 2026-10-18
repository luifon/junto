#!/usr/bin/env python3
"""The steps and the checks of deploy/tests/recording.sh (ADR 0020).

usage: recording_driver.py <main|low-storage> --project <name> --env-file <file>
           --compose-file <file>... --origin <http://127.0.0.1:port> --participant <program>
           [--speech <directory>] [--models] [--keep-files <directory>]

The driver talks to a running local form of the deployment: it creates accounts through the
API, runs test participants that publish generated sound and pictures, records the call with
the recorder of the deployment and examines the files with the ffmpeg of the server
container. It prints one line per check and ends with the code 1 when a check failed.
"""

from __future__ import annotations

import argparse
import difflib
import hashlib
import json
import os
import re
import signal
import subprocess
import sys
import threading
import time
import unicodedata
from pathlib import Path
from typing import Any, Callable

from junto_api import Device, Unexpected, request
from server_output import newest_owner_code, standard_output

RECORDINGS = "/var/lib/junto/recordings"
RECORDER_WORK = "/home/egress/tmp"
RESERVATION_BYTES = 8 * 1024**3
FINAL_STATES = ("ready", "partly_failed", "failed")

# The picture that the recorder asks for (ADR 0604) and the rules of the layout page
# (recording-layout/theme.css and layout.js): the share in the main area, the cameras in
# the lower 24 percent, one pixel of border and 8 pixels of gap.
WIDTH, HEIGHT = 1920, 1080
TILES_SHARE = 0.24
GAP = 8.0

TONE_A, TONE_B, TONE_SHARE = 440, 880, 1000
CONTROL_BANDS = (620, 1500)
# Colours of the test pictures, bright so that the dark label of the page stands out.
COLOUR_A, COLOUR_B, COLOUR_SHARE = (0xE0, 0xC0, 0x20), (0x40, 0xD0, 0xA0), (0x30, 0xA0, 0xE0)
CAMERA_BARS = [
    (192, 192, 192),
    (192, 192, 0),
    (0, 192, 192),
    (0, 192, 0),
    (192, 0, 192),
    (192, 0, 0),
    (0, 0, 192),
    (32, 32, 32),
]
NAME_A, NAME_B, NAME_C, NAME_D = "Ada", "Bartholomew Longname", "Cyd Share", "Dora"

# How far the words of the lines of a track must agree with the words of its speech file, in
# their order: twice the number of agreeing words, divided by the words of both texts. Words
# that are missing, added or out of order lower the value. The worker measured a word error
# rate under 10 % for the development model on this speech
# (workers/transcription/tests/model_support.py); the recorder encodes the sound twice more.
WORDS_AGREE = 0.8


class Failed(Exception):
    """A step that the later steps need did not work."""


class Report:
    def __init__(self) -> None:
        self.failures = 0

    def ok(self, text: str) -> None:
        print(f"ok    {text}", flush=True)

    def fail(self, text: str, detail: object = "") -> None:
        self.failures += 1
        print(f"FAIL  {text}" + (f": {detail}" if detail != "" else ""), flush=True)

    def check(self, text: str, holds: bool, detail: object = "") -> bool:
        if holds:
            self.ok(text)
        else:
            self.fail(text, detail)
        return holds

    def info(self, text: str) -> None:
        print(f"info  {text}", flush=True)


report = Report()


def wait_for(what: str, holds: Callable[[], Any], seconds: float, pause: float = 0.5) -> Any:
    deadline = time.monotonic() + seconds
    while True:
        value = holds()
        if value:
            return value
        if time.monotonic() > deadline:
            raise Failed(f"after {seconds:.0f} s this did not hold: {what}")
        time.sleep(pause)


class Deployment:
    """The Compose project of the test, and commands inside its containers."""

    def __init__(self, arguments: argparse.Namespace) -> None:
        self.directory = Path(__file__).resolve().parent.parent
        self.command = ["docker", "compose", "-p", arguments.project, "--env-file", arguments.env_file]
        for file in arguments.compose_file:
            self.command += ["-f", file]

    def run(self, *arguments: str, check: bool = True, seconds: float = 300) -> str:
        done = subprocess.run(
            [*self.command, *arguments],
            cwd=self.directory,
            capture_output=True,
            text=True,
            timeout=seconds,
            check=False,
        )
        if check and done.returncode != 0:
            raise Failed(f"docker compose {' '.join(arguments[:3])} ended with {done.returncode}: {done.stderr[-400:]}")
        return done.stdout

    def inside(self, service: str, script: str, check: bool = True, seconds: float = 300) -> str:
        return self.run("exec", "-T", service, "/bin/sh", "-c", script, check=check, seconds=seconds)

    def owner_code(self) -> str:
        code = newest_owner_code(standard_output(self.command, self.directory, "server"))
        if code is None:
            raise Failed("the standard output of the server has no owner code")
        return code

    def sql(self, query: str) -> str:
        return self.run("exec", "-T", "postgres", "psql", "-U", "junto", "-d", "junto", "-At", "-c", query).strip()

    def recording_files(self) -> list[tuple[str, int, str]]:
        """Everything below the directory of the recordings: (type, bytes, path)."""
        listing = self.inside("server", f"find {RECORDINGS} -mindepth 1 -printf '%y %s %P\\n'")
        entries = []
        for line in listing.splitlines():
            kind, size, name = line.split(" ", 2)
            entries.append((kind, int(size), name))
        return sorted(entries, key=lambda entry: entry[2])

    def recording_files_of(self, recording_id: str) -> list[tuple[str, int, str]]:
        return [entry for entry in self.recording_files() if entry[2].startswith(recording_id)]

    def recorder_work_files(self) -> list[str]:
        """The files and links in the temporary directory of the recorder. The socket of the
        recorder process is there while it runs."""
        listing = self.inside("recorder", f"find {RECORDER_WORK} -mindepth 1 '(' -type f -o -type l ')'")
        return sorted(listing.split())

    def restarts(self, service: str) -> int:
        container = self.run("ps", "-q", "-a", service).strip()
        done = subprocess.run(
            ["docker", "inspect", "--format", "{{.RestartCount}}", container],
            capture_output=True,
            text=True,
            timeout=60,
            check=True,
        )
        return int(done.stdout.strip())

    def recorder_memory(self) -> tuple[int, int] | None:
        """The memory of the recorder container in bytes: the highest value that its control
        group counted, which holds the cache of the files that it wrote, and the present value
        without that cache."""
        text = self.inside(
            "recorder",
            "cat /sys/fs/cgroup/memory.peak /sys/fs/cgroup/memory.current; grep -E '^(active_file|inactive_file) ' /sys/fs/cgroup/memory.stat",
            check=False,
        ).split()
        if len(text) != 6 or not all(value.isdigit() for value in (text[0], text[1], text[3], text[5])):
            return None
        return int(text[0]), int(text[1]) - int(text[3]) - int(text[5])

    def log(self, service: str) -> str:
        return self.run("logs", "--no-log-prefix", service)


class Participant:
    """One process of spikes/test-participant that publishes into a call."""

    def __init__(self, tool: str, name: str, media_url: str, token: str, options: list[str]) -> None:
        self.name = name
        self.events: list[dict[str, Any]] = []
        self._lock = threading.Lock()
        # The join token goes through the environment, so that the list of processes of the
        # machine does not show it.
        self.process = subprocess.Popen(
            [tool, "publish", "--url", media_url, *options],
            env={**os.environ, "LIVEKIT_TOKEN": token},
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
        )
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self) -> None:
        assert self.process.stdout is not None
        for line in self.process.stdout:
            try:
                event = json.loads(line)
            except ValueError:
                continue
            with self._lock:
                self.events.append(event)

    def seen(self, event: str) -> list[dict[str, Any]]:
        with self._lock:
            return [item for item in self.events if item.get("event") == event]

    def wait(self, event: str, count: int = 1, seconds: float = 30) -> None:
        def holds() -> bool:
            if self.process.poll() is not None:
                raise Failed(f"the participant {self.name} ended with the code {self.process.returncode}")
            return len(self.seen(event)) >= count

        wait_for(f"the participant {self.name} reports {count} x {event}", holds, seconds, 0.1)

    def play(self) -> None:
        assert self.process.stdin is not None
        self.process.stdin.write("play\n")
        self.process.stdin.flush()

    def leave(self) -> None:
        if self.process.poll() is None:
            self.process.send_signal(signal.SIGINT)
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait()

    def kill(self) -> None:
        if self.process.poll() is None:
            self.process.kill()
            self.process.wait()


def yuv_of(colour: tuple[int, int, int]) -> tuple[float, float, float]:
    """The colour as the composite stores it: BT.709 with limited range."""
    r, g, b = (part / 255 for part in colour)
    luma = 0.2126 * r + 0.7152 * g + 0.0722 * b
    return (16 + 219 * luma, 128 + 224 * (b - luma) / 1.8556, 128 + 224 * (r - luma) / 1.5748)


def same_colour(measured: tuple[float, float, float], colour: tuple[int, int, int]) -> bool:
    expected = yuv_of(colour)
    return (
        abs(measured[0] - expected[0]) <= 14
        and abs(measured[1] - expected[1]) <= 10
        and abs(measured[2] - expected[2]) <= 10
    )


class Layout:
    """Where the layout page draws a share and `cameras` camera tiles (layout.js, fitTiles)."""

    def __init__(self, cameras: int) -> None:
        tiles_top = HEIGHT * (1 - TILES_SHARE)
        # The main tile: 8 pixels inside the main area, one pixel of border, the picture of
        # 16:9 inside it.
        box_width, box_height = WIDTH - 2 * GAP - 2, tiles_top - 2 * GAP - 2
        picture_height = min(box_height, box_width * 9 / 16)
        picture_width = picture_height * 16 / 9
        self.main = (GAP + 1 + (box_width - picture_width) / 2, GAP + 1, picture_width, picture_height)
        self.main_tile = (GAP + 1, GAP + 1, box_width, box_height)

        area_width, area_height = float(WIDTH), HEIGHT * TILES_SHARE - 1
        best = (0.0, 1)
        for columns in range(1, cameras + 1):
            rows = -(-cameras // columns)
            size = min((area_width - GAP * (columns + 1)) / columns, (area_height - GAP * (rows + 1)) / rows * 16 / 9)
            if size > best[0]:
                best = (size, columns)
        tile_width, columns = best
        tile_height = tile_width * 9 / 16
        rows = -(-cameras // columns)
        self.cameras = []
        for index in range(cameras):
            row = index // columns
            in_row = min(columns, cameras - row * columns)
            left = (area_width - in_row * (tile_width + GAP) + GAP) / 2 + index % columns * (tile_width + GAP)
            top = tiles_top + 1 + (area_height - rows * (tile_height + GAP) + GAP) / 2 + row * (tile_height + GAP)
            self.cameras.append((left + 1, top + 1, tile_width - 2, tile_height - 2))


class MediaChecks:
    """Runs ffprobe and ffmpeg of the server container on files of the volume of the recordings.

    The ffmpeg of the server image writes no raw picture, so a part of a frame is measured
    with the filters `crop`, `signalstats` and `bbox`, which print their result.
    """

    def __init__(self, deployment: Deployment) -> None:
        self.deployment = deployment

    def probe(self, path: str) -> dict[str, Any]:
        text = self.deployment.inside(
            "server",
            "ffprobe -v error -show_entries "
            "stream=codec_type,codec_name,width,height,sample_rate,channels,duration,avg_frame_rate,nb_frames,color_space"
            f":format=duration,format_name -of json '{path}'",
        )
        return json.loads(text)

    def sha256(self, path: str) -> str:
        return self.deployment.inside("server", f"sha256sum '{path}'").split()[0]

    def regions(self, path: str, at: float, crops: dict[str, tuple[float, float, float, float]]) -> dict[str, tuple[float, float, float]]:
        """The average Y, U and V of each named part of the frame at `at` seconds."""
        script = [f"f='{path}'"]
        for key, (x, y, width, height) in crops.items():
            script.append(
                f"echo '## {key}'; ffmpeg -v error -threads 1 -ss {at} -i \"$f\" -frames:v 1 -an "
                f"-vf 'crop={round(width)}:{round(height)}:{round(x)}:{round(y)},signalstats,metadata=print:file=-' "
                "-f null - 2>&1"
            )
        result: dict[str, tuple[float, float, float]] = {}
        for key, values in self._sections(self.deployment.inside("server", "\n".join(script))).items():
            if all(name in values for name in ("YAVG", "UAVG", "VAVG")):
                result[key] = (values["YAVG"], values["UAVG"], values["VAVG"])
        return result

    def boxes(
        self, path: str, at: float, crops: dict[str, tuple[float, float, float, float, int, bool]]
    ) -> dict[str, tuple[int, int] | None]:
        """For each named part of the frame: the first and last column of the pixels that are
        brighter than the threshold, or of the darker ones when the part is inverted."""
        script = [f"f='{path}'"]
        for key, (x, y, width, height, threshold, inverted) in crops.items():
            invert = "negate," if inverted else ""
            level = 255 - threshold if inverted else threshold
            script.append(
                f"echo '## {key}'; ffmpeg -v error -threads 1 -ss {at} -i \"$f\" -frames:v 1 -an "
                f"-vf 'crop={round(width)}:{round(height)}:{round(x)}:{round(y)},{invert}bbox=min_val={level},metadata=print:file=-' "
                "-f null - 2>&1"
            )
        result: dict[str, tuple[int, int] | None] = {}
        for key, values in self._sections(self.deployment.inside("server", "\n".join(script))).items():
            result[key] = (int(values["x1"]), int(values["x2"])) if "x1" in values and "x2" in values else None
        return result

    def band_levels(self, path: str, start: float, length: float, bands: tuple[int, ...]) -> dict[int, float]:
        """The mean level in dB of the sound inside a narrow band around each frequency."""
        script = [f"f='{path}'"]
        for band in bands:
            script.append(
                f"echo '## {band}'; ffmpeg -hide_banner -nostats -threads 1 -ss {start} -t {length} -i \"$f\" -vn "
f"-af 'bandpass=f={band}:width_type=h:w=40,bandpass=f={band}:width_type=h:w=40,volumedetect' -f null - 2>&1 | grep mean_volume"
            )
        levels: dict[int, float] = {}
        section = None
        for line in self.deployment.inside("server", "\n".join(script), check=False).splitlines():
            if line.startswith("## "):
                section = int(line[3:])
            elif section is not None and (found := re.search(r"mean_volume: (-?[0-9.]+) dB", line)):
                levels[section] = float(found.group(1))
        return levels

    def changes(
        self, path: str, crops: dict[str, tuple[float, float, float, float]]
    ) -> dict[str, list[tuple[float, float]]] | None:
        """For each named part of the picture and each frame: the time of the frame and the
        largest difference of one pixel to the frame before it, from 0 to 255. `None` when
        ffmpeg did not read the whole video."""
        keys = list(crops)
        chains = [f"[0:v]split={len(keys)}" + "".join(f"[in{index}]" for index in range(len(keys)))]
        outputs = []
        for index, key in enumerate(keys):
            x, y, width, height = crops[key]
            chains.append(
                f"[in{index}]crop={round(width)}:{round(height)}:{round(x)}:{round(y)},tblend=all_mode=difference,"
                f"signalstats,metadata=mode=print:key=lavfi.signalstats.YMAX:file=/tmp/changes-{index}[out{index}]"
            )
            outputs.append(f"-map '[out{index}]' -f null -")
        script = (
            f"rm -f /tmp/changes-*; ffmpeg -v error -threads 2 -i '{path}' -an "
            f"-filter_complex '{';'.join(chains)}' {' '.join(outputs)} && echo '## read'; "
            + "; ".join(f"echo '## {index}'; cat /tmp/changes-{index}" for index in range(len(keys)))
            + "; rm -f /tmp/changes-*"
        )
        text = self.deployment.inside("server", script, check=False, seconds=600)
        if "## read" not in text:
            return None
        found: dict[str, list[tuple[float, float]]] = {key: [] for key in keys}
        section, at = None, None
        for line in text.splitlines():
            if line.startswith("## ") and line[3:].isdigit():
                section, at = keys[int(line[3:])], None
            elif section is not None and (time_of := re.search(r"pts_time:([0-9.]+)", line)):
                at = float(time_of.group(1))
            elif section is not None and at is not None and (value := re.match(r"lavfi\.signalstats\.YMAX=([0-9.]+)", line)):
                found[section].append((at, float(value.group(1))))
        return found

    def mean_level(self, path: str, start: float, length: float) -> float | None:
        text = self.deployment.inside(
            "server",
            f"ffmpeg -hide_banner -nostats -threads 1 -ss {start} -t {length} -i '{path}' -vn -af volumedetect -f null - 2>&1",
            check=False,
        )
        found = re.search(r"mean_volume: (-?[0-9.]+) dB", text)
        return float(found.group(1)) if found else None

    @staticmethod
    def _sections(text: str) -> dict[str, dict[str, float]]:
        sections: dict[str, dict[str, float]] = {}
        current: dict[str, float] = {}
        for line in text.splitlines():
            if line.startswith("## "):
                current = sections.setdefault(line[3:], {})
            elif found := re.match(r"lavfi\.(?:signalstats|bbox)\.(\w+)=(-?[0-9.]+)", line):
                current[found.group(1)] = float(found.group(2))
        return sections


def words(text: str) -> list[str]:
    plain = unicodedata.normalize("NFKD", text.lower())
    plain = "".join(letter for letter in plain if not unicodedata.combining(letter))
    return re.findall(r"[a-z0-9]+", plain)


def agreement(reference: str, heard: str) -> float:
    """How far two texts agree in their words and the order of the words, from 0 to 1."""
    return difflib.SequenceMatcher(None, words(reference), words(heard), autojunk=False).ratio()


def words_within(part: str, whole: str) -> float:
    """The share of the words of `part` that are in `whole`, in their order."""
    wanted = words(part)
    matcher = difflib.SequenceMatcher(None, wanted, words(whole), autojunk=False)
    return sum(block.size for block in matcher.get_matching_blocks()) / max(1, len(wanted))


class Scene:
    """The accounts of the test and the voice channel of the default workspace."""

    def __init__(self, arguments: argparse.Namespace, deployment: Deployment, members: int) -> None:
        self.arguments = arguments
        self.deployment = deployment
        self.media = MediaChecks(deployment)
        self.owner = Device(arguments.origin)
        self.owner.claim_owner(deployment.owner_code(), NAME_A)
        self.members: list[Device] = []
        for name in (NAME_B, NAME_C, NAME_D)[:members]:
            invite = self.owner.call("POST", "/api/v1/invites", {}, expect=(200, 201)).json()
            device = Device(arguments.origin)
            device.join(invite["code"], name)
            self.members.append(device)
        snapshot = self.owner.get("/api/v1/snapshot")
        workspace = snapshot["workspaces"][0]
        self.workspace_id = workspace["workspace"]["id"]
        self.channel_id = next(channel["id"] for channel in workspace["channels"] if channel["kind"] == "voice")
        self.participants: list[Participant] = []
        self.in_call: list[Device] = []

    def enter(self, device: Device, name: str, options: list[str]) -> Participant:
        join = device.call("POST", f"/api/v1/channels/{self.channel_id}/call/join", expect=(200, 201)).json()
        self.in_call.append(device)
        participant = Participant(self.arguments.participant, name, join["media_url"], join["token"], options)
        self.participants.append(participant)
        participant.wait("joined")
        return participant

    def people_in_call(self) -> set[str]:
        answer = self.owner.call("GET", f"/api/v1/channels/{self.channel_id}/call")
        if answer.status != 200:
            return set()
        call = answer.json().get("call") or {}
        return {person["account_id"] for person in call.get("participants", [])}

    def everybody_leaves(self) -> None:
        """Each person leaves the call through the API, as a client does, and disconnects."""
        for device in self.in_call:
            device.call("DELETE", "/api/v1/call")
        self.in_call.clear()
        for participant in self.participants:
            participant.leave()
        self.participants.clear()
        wait_for("the call has ended", lambda: not self.people_in_call(), 60)

    def recording(self, recording_id: str) -> dict[str, Any]:
        detail: dict[str, Any] = self.owner.get(f"/api/v1/recordings/{recording_id}")
        return detail

    def start_recording(self) -> str:
        started = self.owner.call("PUT", f"/api/v1/channels/{self.channel_id}/call/recording", {}, expect=200).json()
        return str(started["id"])

    def stop_recording(self) -> None:
        self.owner.call("DELETE", f"/api/v1/channels/{self.channel_id}/call/recording", expect=204)

    def wait_recording(self, recording_id: str, sources: list[str], seconds: float = 60) -> dict[str, Any]:
        """Waits until one segment of each named source records and the composite has written a
        piece, which is the sign that its page has started."""

        def holds() -> dict[str, Any] | None:
            detail = self.recording(recording_id)
            if detail["recording"]["state"] in FINAL_STATES:
                raise Failed(f"the recording became {detail['recording']['state']} before it recorded")
            recording = sorted(segment["source"] for segment in detail["segments"] if segment["state"] == "recording")
            if detail["recording"]["state"] != "recording" or recording != sorted(sources):
                return None
            composite = next(segment for segment in detail["segments"] if segment["state"] == "recording" and segment["source"] == "composite")
            pieces = self.deployment.inside(
                "server", f"ls {RECORDINGS}/{recording_id}/{composite['id']} 2>/dev/null | grep -c '\\.ts$' || true"
            ).strip()
            return detail if pieces.isdigit() and int(pieces) >= 1 else None

        found: dict[str, Any] = wait_for("the recorder records every source", holds, seconds)
        return found

    def wait_final(self, recording_id: str, seconds: float = 180) -> dict[str, Any]:
        def holds() -> dict[str, Any] | None:
            detail = self.recording(recording_id)
            return detail if detail["recording"]["state"] in FINAL_STATES else None

        found: dict[str, Any] = wait_for("the recording reaches a final state", holds, seconds)
        return found

    def reservation(self, recording_id: str) -> str:
        return self.deployment.sql(f"select bytes from storage_reservations where subject_id = '{recording_id}'")

    def remove_and_check_nothing_is_left(self, recording_id: str, what: str) -> None:
        # A removal ends a transcript that still runs, and the request of that run then stays
        # in the spool until a later hand-over (ADR 0016). The test waits for the end of the
        # transcript, so that an empty spool can be required afterwards.
        ended = ("none", "ready", "partial", "failed")
        wait_for(
            "the transcript of the recording has ended",
            lambda: self.owner.get(f"/api/v1/recordings/{recording_id}/transcript")["state"] in ended,
            900,
            2,
        )
        self.owner.call("DELETE", f"/api/v1/recordings/{recording_id}", expect=204)

        def gone() -> bool:
            return not self.deployment.recording_files_of(recording_id) and self.deployment.sql("select count(*) from file_removals") == "0"

        try:
            wait_for("the files of the removed recording are deleted", gone, 60)
        except Failed:
            pass
        files = [entry for entry in self.deployment.recording_files() if entry[0] != "d"]
        report.check(f"{what}: after the removal no file is left below the recordings", not files, files[:6])
        left = [entry[2] for entry in self.deployment.recording_files() if entry[0] == "d" and entry[2] != "rescued"]
        report.check(f"{what}: after the removal no directory of a recording or of a lost job is left", not left, left[:6])
        work = self.deployment.recorder_work_files()
        report.check(f"{what}: the work directory of the recorder holds no file", not work, work[:6])
        spool = self.deployment.inside("server", "find /var/lib/junto/spool -mindepth 1").split()
        report.check(f"{what}: the spool of the transcription jobs is empty", not spool, spool[:6])
        answer = self.owner.call("GET", f"/api/v1/recordings/{recording_id}")
        report.check(f"{what}: the removed recording is not found", answer.status == 404, answer.status)


def participant_options(arguments: argparse.Namespace) -> tuple[list[str], list[str], list[str]]:
    a = ["--mic-hz", str(TONE_A), "--camera", "--label", "A", "--screen-color", "%02x%02x%02x" % COLOUR_A]
    b = ["--mic-hz", str(TONE_B), "--camera", "--label", "B", "--screen-color", "%02x%02x%02x" % COLOUR_B]
    c = ["--no-mic", "--screen", "--screen-audio-hz", str(TONE_SHARE), "--label", "C", "--screen-color", "%02x%02x%02x" % COLOUR_SHARE]
    if arguments.speech:
        speech = Path(arguments.speech)
        a += ["--mic-file", str(speech / "en_quick.wav"), "--play-on-stdin"]
        b += ["--mic-file", str(speech / "pt_quick.wav"), "--play-on-stdin"]
        c += ["--screen-audio-file", str(speech / "trio_first.wav"), "--play-on-stdin"]
    return a, b, c


def start_call(scene: Scene, arguments: argparse.Namespace) -> None:
    """A and B with a microphone and a camera, C with a screen share and its sound."""
    options_a, options_b, options_c = participant_options(arguments)
    member_b, member_c = scene.members[0], scene.members[1]
    a = scene.enter(scene.owner, "A", options_a)
    b = scene.enter(member_b, "B", options_b)
    c = scene.enter(member_c, "C", options_c)
    a.wait("published", 2)
    b.wait("published", 2)
    c.wait("published", 2)
    member_c.call("PUT", "/api/v1/call/screen-share", {"audio": "application"}, expect=(200, 204))
    everybody = {scene.owner.account_id, member_b.account_id, member_c.account_id}
    wait_for("the server lists the three people in the call", lambda: scene.people_in_call() == everybody, 30)


def check_tones(scene: Scene, what: str, path: str, present: tuple[int, ...], start: float = 1.0, length: float = 5.0) -> None:
    bands = (TONE_A, TONE_B, TONE_SHARE, *CONTROL_BANDS)
    levels = scene.media.band_levels(path, start, length, bands)
    if len(levels) != len(bands):
        report.fail(f"{what}: the sound could not be measured", levels)
        return
    floor = max(levels[band] for band in bands if band not in present)
    weakest = min(levels[band] for band in present)
    report.check(
        f"{what}: the sound holds the tones {', '.join(str(band) for band in present)} Hz and no other tone of the call",
        weakest >= -30 and weakest - floor >= 25,
        levels,
    )


def check_motion(scene: Scene, path: str, length: float) -> None:
    """The test pictures move: the share draws a new frame five times per second and each
    camera fifteen times, each with a white shape on another place. A part of the composite
    that shows one of them has, at least every 2 seconds, a frame in which one pixel differs
    from the frame before by more than half of the range. The noise of the encoder stays far
    below that: the stage beside the tiles and the name label of a camera tile, which never
    change, show it."""
    layout = Layout(cameras=2)
    x, y, _, height = layout.cameras[0]
    parts = {
        "share": layout.main,
        "camera-1": layout.cameras[0],
        "camera-2": layout.cameras[1],
        "stage": (20.0, HEIGHT * (1 - TILES_SHARE) + 50, 200.0, 100.0),
        "label": (x + 8, y + height - 19, 26.0, 8.0),
    }
    measured = scene.media.changes(path, parts)
    if measured is None or any(len(frames) < length * 25 for frames in measured.values()):
        report.fail("the changes of the picture of the composite could not be measured", {key: len(frames) for key, frames in (measured or {}).items()})
        return
    longest: dict[str, float] = {}
    for key, frames in measured.items():
        last = frames[0][0]
        longest[key] = 0.0
        for at, difference in frames:
            if difference >= 128:
                longest[key] = max(longest[key], at - last)
                last = at
        longest[key] = max(longest[key], frames[-1][0] - last)
    report.check(
        "the share and each camera in the composite change at least every 2 seconds, over its whole length",
        all(longest[key] < 2 for key in ("share", "camera-1", "camera-2")),
        longest,
    )
    report.check(
        "the stage and a name label, which do not change, are measured as still over the whole length",
        all(longest[key] >= length - 1 for key in ("stage", "label")),
        longest,
    )


def check_frame(scene: Scene, when: str, path: str, at: float) -> None:
    """Examines one frame of the composite: the share, the two cameras and the name labels."""
    layout = Layout(cameras=2)
    main_x, main_y, main_width, main_height = layout.main
    crops: dict[str, tuple[float, float, float, float]] = {}
    # Four fields of the grid of the screen picture (1280x720, lines every 80 pixels), away
    # from the two texts that the picture draws at its upper left.
    scale = main_width / 1280
    for index, (column, row) in enumerate(((12, 2), (12, 6), (3, 6), (8, 7))):
        crops[f"share-{index}"] = (main_x + (column * 80 + 22) * scale, main_y + (row * 80 + 22) * scale, 36 * scale, 36 * scale)
    for tile, (x, y, width, height) in enumerate(layout.cameras):
        picture_width = min(width, height * 16 / 9)
        left = x + (width - picture_width) / 2
        unit = picture_width / 640
        for bar in range(8):
            crops[f"camera-{tile}-bar-{bar}"] = (left + (bar * 80 + 28) * unit, y + 170 * unit, 24 * unit, 24 * unit)
        crops[f"camera-{tile}-band"] = (left + 500 * unit, y + 306 * unit, 100 * unit, 20 * unit)
    crops["corner"] = (2, 2, 4, 4)
    measured = scene.media.regions(path, at, crops)
    if set(measured) != set(crops):
        report.fail(f"frame {when}: the frame of the composite could not be measured", sorted(set(crops) - set(measured))[:5])
        return

    share = [same_colour(measured[f"share-{index}"], COLOUR_SHARE) for index in range(4)]
    report.check(
        f"frame {when}: the main area shows the shared screen (its colour in at least 3 of 4 fields)",
        sum(share) >= 3,
        [tuple(round(value) for value in measured[f"share-{index}"]) for index in range(4)],
    )
    owners = {}
    for tile in range(2):
        bars = [same_colour(measured[f"camera-{tile}-bar-{bar}"], CAMERA_BARS[bar]) for bar in range(8)]
        report.check(
            f"frame {when}: camera tile {tile + 1} shows the eight bars of a camera picture (at least 7)",
            sum(bars) >= 7,
            [tuple(round(value) for value in measured[f"camera-{tile}-bar-{bar}"]) for bar in range(8)],
        )
        band = measured[f"camera-{tile}-band"]
        owners[tile] = "A" if same_colour(band, COLOUR_A) else "B" if same_colour(band, COLOUR_B) else None
    report.check(
        f"frame {when}: the two camera tiles are the cameras of A and of B",
        sorted(str(owner) for owner in owners.values()) == ["A", "B"],
        {tile: tuple(round(value) for value in measured[f"camera-{tile}-band"]) for tile in range(2)},
    )
    report.check(f"frame {when}: the stage beside the tiles is dark", measured["corner"][0] < 60, measured["corner"])

    # The page writes the display name of a participant on a dark box at the lower left of its
    # tile. On a camera tile the box lies on the bright band of the camera picture; on the main
    # tile it lies beside the picture, where only its text is bright.
    boxes: dict[str, tuple[float, float, float, float, int, bool]] = {}
    for tile, (x, y, width, height) in enumerate(layout.cameras):
        boxes[f"label-{tile}"] = (x + 2, y + height - 20, width - 4, 8, 70, True)
    tile_x, tile_y, _, tile_height = layout.main_tile
    boxes["label-main"] = (tile_x, tile_y + tile_height - 30, main_x - tile_x - 4, 28, 110, False)
    found = scene.media.boxes(path, at, boxes)
    widths = {key: (box[1] - box[0] + 1 if box else 0) for key, box in found.items()}
    report.info(f"frame {when}: label boxes (first and last column): {found}")
    by_owner = {owners[tile]: widths.get(f"label-{tile}", 0) for tile in range(2)}
    short, long = by_owner.get("A", 0), by_owner.get("B", 0)
    report.check(
        f"frame {when}: each camera tile carries a name label, and the label of \"{NAME_B}\" is wider than the label of \"{NAME_A}\"",
        20 <= short <= 80 and long >= 2.5 * short,
        by_owner,
    )
    starts = [found[f"label-{tile}"][0] for tile in range(2) if found.get(f"label-{tile}")]  # type: ignore[index]
    report.check(f"frame {when}: the labels start at the left edge of their tiles", len(starts) == 2 and all(start <= 10 for start in starts), starts)
    report.check(
        f"frame {when}: the main tile carries the name of the person who shares (\"{NAME_C}\")",
        30 <= widths.get("label-main", 0) <= 160,
        widths.get("label-main"),
    )


def check_downloads(scene: Scene, recording_id: str, composite: dict[str, Any], audio: dict[str, Any]) -> None:
    origin = scene.arguments.origin
    path = f"/api/v1/recordings/{recording_id}/segments/{composite['id']}/file"
    answer = request("GET", origin + path)
    report.check("a download without a session is refused with 401", answer.status == 401, answer.status)

    answer = scene.owner.call("GET", path, timeout=120)
    on_volume = f"{RECORDINGS}/{recording_id}/{composite['id']}.mp4"
    report.check(
        "the video downloads with its type, its length and the bytes of the file on the volume",
        answer.status == 200
        and answer.headers.get("content-type") == "video/mp4"
        and int(answer.headers.get("content-length", -1)) == len(answer.body) == composite["file"]["bytes"]
        and answer.headers.get("accept-ranges") == "bytes"
        and answer.headers.get("x-content-type-options") == "nosniff"
        and hashlib.sha256(answer.body).hexdigest() == scene.media.sha256(on_volume),
        (answer.status, dict(answer.headers)),
    )
    whole = answer.body
    if scene.arguments.keep_files:
        # For a person who examines a failure: the video and one audio file as downloaded.
        directory = Path(scene.arguments.keep_files)
        directory.mkdir(parents=True, exist_ok=True)
        (directory / "composite.mp4").write_bytes(whole)
        (directory / "microphone-a.ogg").write_bytes(scene.owner.call("GET", f"/api/v1/recordings/{recording_id}/segments/{audio['id']}/file").body)
    part = scene.owner.call("GET", path, headers={"Range": "bytes=100-299"})
    report.check(
        "a range of the video is answered with 206 and those bytes",
        part.status == 206 and part.body == whole[100:300] and part.headers.get("content-range") == f"bytes 100-299/{len(whole)}",
        (part.status, part.headers.get("content-range")),
    )
    part = scene.owner.call("GET", path, headers={"Range": "bytes=-64"})
    report.check("a range from the end is answered with the last bytes", part.status == 206 and part.body == whole[-64:], part.status)
    part = scene.owner.call("GET", path, headers={"Range": f"bytes={len(whole)}-"})
    report.check(
        "a range outside the file is answered with 416",
        part.status == 416 and part.headers.get("content-range") == f"bytes */{len(whole)}",
        (part.status, part.headers.get("content-range")),
    )

    member = scene.members[0]
    audio_path = f"/api/v1/recordings/{recording_id}/segments/{audio['id']}/file"
    answer = member.call("GET", audio_path)
    report.check(
        "a member who sees the voice channel downloads an audio file as audio/ogg",
        answer.status == 200 and answer.headers.get("content-type") == "audio/ogg" and answer.body[:4] == b"OggS",
        (answer.status, answer.headers.get("content-type")),
    )
    answer = member.call("GET", f"/api/v1/recordings/{recording_id}/segments/{recording_id}/file")
    report.check("an unknown segment is answered with 404", answer.status == 404, answer.status)

    # A member whose role is denied `view_channel` in the voice channel reads nothing of it.
    outsider = scene.members[2]
    before = outsider.call("GET", audio_path).status
    role = scene.owner.call(
        "POST", f"/api/v1/workspaces/{scene.workspace_id}/roles", {"name": "outside", "permissions": []}, expect=(200, 201)
    ).json()
    scene.owner.call(
        "PUT", f"/api/v1/workspaces/{scene.workspace_id}/members/{outsider.account_id}/roles/{role['id']}", expect=(200, 204)
    )
    scene.owner.call(
        "PUT", f"/api/v1/channels/{scene.channel_id}/overrides/{role['id']}", {"allow": [], "deny": ["view_channel"]}, expect=(200, 204)
    )
    after_file = outsider.call("GET", audio_path)
    after_recording = outsider.call("GET", f"/api/v1/recordings/{recording_id}")
    report.check(
        "a member without `view_channel` in the voice channel gets 404 for the file and for the recording",
        before == 200 and after_file.status == 404 and after_recording.status == 404,
        (before, after_file.status, after_recording.status),
    )
    answer = member.call("DELETE", f"/api/v1/recordings/{recording_id}")
    report.check("a member without `manage_recordings` cannot remove the recording (403)", answer.status == 403, answer.status)


def check_transcript(scene: Scene, recording_id: str, arguments: argparse.Namespace) -> None:
    def holds() -> dict[str, Any] | None:
        transcript: dict[str, Any] = scene.owner.get(f"/api/v1/recordings/{recording_id}/transcript")
        return transcript if transcript["state"] in ("ready", "partial", "failed") else None

    started = time.monotonic()
    transcript = wait_for("the transcript reaches a final state", holds, 1500 if arguments.models else 240, 2)
    report.info(f"transcript: state {transcript['state']} after {time.monotonic() - started:.0f} s")
    state = scene.recording(recording_id)["recording"]["state"]
    report.check("the state of the recording does not depend on the transcript", state == "ready", state)
    if not arguments.models:
        report.check(
            "without model files the real worker refuses to start, and the transcript fails with `worker_not_ready`",
            transcript["state"] == "failed" and transcript.get("failure") == "worker_not_ready",
            (transcript["state"], transcript.get("failure")),
        )
        log = scene.deployment.log("server")
        report.check("the log of the server holds the line of the worker about the missing model file", "model_file_missing" in log)
        return

    if not report.check("the transcript is ready", transcript["state"] == "ready", (transcript["state"], transcript.get("failure"))):
        return
    results = {result["segment_id"]: result for result in transcript["segments"]}
    statuses = sorted((result["status"], result["speakers"]) for result in results.values())
    report.check(
        "every audio segment was transcribed whole, and the shared sound has its voices separated",
        statuses == [("ok", "not_applicable"), ("ok", "not_applicable"), ("ok", "separated")],
        statuses,
    )
    lines: list[dict[str, Any]] = []
    after = None
    for _ in range(100):
        page = scene.owner.get(f"/api/v1/recordings/{recording_id}/transcript/lines" + (f"?after={after}" if after is not None else ""))
        lines += page["lines"]
        following = page.get("next_after")
        if following is None:
            break
        if after is not None and following <= after:
            raise Failed("the pages of the transcript lines do not advance")
        after = following
    else:
        raise Failed("the transcript lines have more than 100 pages")
    report.check("the lines are in the order of the timeline", [line["start_ms"] for line in lines] == sorted(line["start_ms"] for line in lines))
    speech = Path(arguments.speech)
    sources = {
        "A": (scene.owner.account_id, "microphone", "en_quick"),
        "B": (scene.members[0].account_id, "microphone", "pt_quick"),
        "C": (scene.members[1].account_id, "shared", "trio_first"),
    }
    for name, (account_id, source, file) in sources.items():
        reference = json.loads((speech / f"{file}.json").read_text("utf-8"))["text"]
        heard = " ".join(line["text"] for line in lines if line["account_id"] == account_id and line["source"] == source)
        agrees = agreement(reference, heard)
        others = max(
            agreement(json.loads((speech / f"{other}.json").read_text("utf-8"))["text"], heard)
            for _, _, other in sources.values()
            if other != file
        )
        report.check(
            f"the lines of {name} ({source}) are the words of the speech file {file} in their order, and of no other file "
            f"(agreement {agrees:.2f} with {len(words(reference))} words, {others:.2f} with another file)",
            agrees >= WORDS_AGREE and others < 0.3,
            heard[:300],
        )
    microphone = [line for line in lines if line["source"] == "microphone"]
    report.check("a line of a microphone has no speaker label", all(line.get("speaker") is None for line in microphone))
    languages = {name: [line["language"] for line in lines if line["account_id"] == sources[name][0]] for name in sources}
    mostly = {name: max(set(found), key=found.count) if found else None for name, found in languages.items()}
    report.check(
        "the lines carry the language that was detected for each of them: English for A and C, Portuguese for B",
        mostly == {"A": "en", "B": "pt", "C": "en"}
        and all(found.count(mostly[name]) >= 0.75 * len(found) for name, found in languages.items()),
        {name: sorted(set(found)) for name, found in languages.items()},
    )
    shared = [line for line in lines if line["source"] == "shared"]
    labels = [line.get("speaker") for line in shared]
    order: list[int] = []
    for line in lines:
        if line.get("speaker") is not None and line["speaker"] not in order:
            order.append(line["speaker"])
    report.check("every line of the shared sound has a speaker label", bool(shared) and all(label is not None for label in labels), labels)
    report.check(
        "the labels are numbered from 1 in the order in which the voices first appear, and the speech of three voices has three",
        order == [1, 2, 3],
        order,
    )
    # Each voice of the speech file has one label of its own: a line belongs to the sentence
    # of the file with which its words agree best, and that sentence names its voice.
    sentences = json.loads((speech / "trio_first.json").read_text("utf-8"))["sentences"]
    by_voice: dict[str, list[int]] = {}
    for line in shared:
        best = max(sentences, key=lambda sentence: words_within(line["text"], sentence["text"]))
        if line.get("speaker") is not None and words_within(line["text"], best["text"]) >= 0.6:
            by_voice.setdefault(best["speaker"], []).append(line["speaker"])
    main_label = {voice: max(sorted(set(found)), key=found.count) for voice, found in by_voice.items()}
    report.check(
        "each of the three voices of the shared sound has a label of its own, on at least 8 of 10 of its lines",
        len(main_label) == 3
        and len(set(main_label.values())) == 3
        and sum(len(found) for found in by_voice.values()) >= 0.8 * len(shared)
        and all(found.count(main_label[voice]) >= 0.8 * len(found) for voice, found in by_voice.items()),
        by_voice,
    )
    listed = sorted((speaker["number"], speaker["account_id"]) for speaker in transcript["speakers"])
    report.check(
        "the transcript lists each label with the person whose screen share holds the voice",
        listed == [(number, scene.members[1].account_id) for number in sorted(order)],
        listed,
    )
    text = scene.owner.call("GET", f"/api/v1/recordings/{recording_id}/transcript/text", expect=200).body.decode()
    report.check("the transcript as plain text names the people and the speaker labels", NAME_A in text and "Speaker 1" in text, text[:200])
    stored = scene.deployment.inside("server", f"test -s {RECORDINGS}/{recording_id}/transcript.json && echo yes || true").strip()
    report.check("the file of the worker is kept beside the recording", stored == "yes")


def phase_recording(scene: Scene, arguments: argparse.Namespace) -> None:
    deployment = scene.deployment
    health = request("GET", f"{arguments.origin}/api/v1/health", expect=200).json()
    report.check("the health route states that the storage is not low", health.get("storage_low") is False, health)

    start_call(scene, arguments)
    requested = time.time()
    recording_id = scene.start_recording()
    sources = ["composite", "microphone", "microphone", "screen_share_audio"]
    scene.wait_recording(recording_id, sources)
    recording_seen = time.time()
    report.ok(f"the recorder records the composite and three audio tracks {recording_seen - requested:.1f} s after the request")
    report.check(
        "the recording holds a reservation of 8 GiB while it runs",
        scene.reservation(recording_id) == str(RESERVATION_BYTES),
        scene.reservation(recording_id),
    )
    again = scene.owner.call("PUT", f"/api/v1/channels/{scene.channel_id}/call/recording", {}, expect=200).json()
    report.check("a second start answers with the running recording", again["id"] == recording_id, again["id"])

    time.sleep(9)
    if arguments.speech:
        for participant in scene.participants:
            participant.play()
        for participant in scene.participants:
            participant.wait("file_ended", seconds=180)
        time.sleep(3)
    else:
        time.sleep(9)
    memory = deployment.recorder_memory()
    scene.stop_recording()
    stopped = time.time()
    detail = scene.wait_final(recording_id)
    final_seen = time.time()
    recording = detail["recording"]
    report.info(f"recorded for {stopped - recording_seen:.1f} s; final state {final_seen - stopped:.1f} s after the stop")
    report.check("the recording is ready", recording["state"] == "ready", json.dumps(detail)[:1500])
    if memory is not None:
        report.info(
            f"memory of the recorder container at the end of the recording: peak {memory[0] / 1024**2:.0f} MiB, "
            f"{memory[1] / 1024**2:.0f} MiB without the cache of the written files"
        )
    report.check("the stop names the person who asked for it", (recording.get("stop") or {}).get("reason") == "stopped" and recording["stop"].get("by") == scene.owner.account_id, recording.get("stop"))
    report.check("the reservation ends with the processing", scene.reservation(recording_id) == "", scene.reservation(recording_id))

    segments = detail["segments"]
    report.check(
        "the recording has four segments, each ended in order with a file that passed the check of the server",
        len(segments) == 4 and all(segment["state"] == "ended" and segment.get("file") and segment["file"]["check"] == "passed" for segment in segments),
        [(segment["source"], segment["state"], (segment.get("file") or {}).get("problems")) for segment in segments],
    )
    if any(not segment.get("file") for segment in segments):
        raise Failed("a segment has no file; the files cannot be examined")
    composite = next(segment for segment in segments if segment["source"] == "composite")
    by_account = {segment["account_id"]: segment for segment in segments if segment["source"] != "composite"}
    audio = {
        "A": by_account[scene.owner.account_id],
        "B": by_account[scene.members[0].account_id],
        "C": by_account[scene.members[1].account_id],
    }
    report.check(
        "the audio segments are the microphones of A and B and the shared sound of C",
        (audio["A"]["source"], audio["B"]["source"], audio["C"]["source"]) == ("microphone", "microphone", "screen_share_audio"),
    )

    # The files on the volume, as ADR 0606 states their paths.
    on_volume = {name: size for kind, size, name in deployment.recording_files_of(recording_id) if kind == "f"}
    expected = {f"{recording_id}/{composite['id']}.mp4": composite["file"]["bytes"]}
    expected.update({f"{recording_id}/{segment['id']}.ogg": segment["file"]["bytes"] for segment in audio.values()})
    without_transcript = {name: size for name, size in on_volume.items() if "/transcript" not in name}
    report.check(
        "the volume holds the video, one Ogg file per audio track and nothing else of the recorder; the pieces are deleted",
        without_transcript == expected,
        without_transcript,
    )
    report.check("the recording states the bytes of its files", recording.get("bytes") == sum(expected.values()), recording.get("bytes"))
    for name, size in sorted(expected.items()):
        report.info(f"file {name.split('/', 1)[1]}: {size} bytes")

    video = f"{RECORDINGS}/{recording_id}/{composite['id']}.mp4"
    probe = scene.media.probe(video)
    streams = {stream["codec_type"]: stream for stream in probe["streams"]}
    picture, sound = streams.get("video", {}), streams.get("audio", {})
    length = float(probe["format"]["duration"])
    report.info(f"composite: {picture.get('codec_name')} {picture.get('width')}x{picture.get('height')} {picture.get('avg_frame_rate')}, {sound.get('codec_name')}, {length:.2f} s")
    numerator, _, denominator = str(picture.get("avg_frame_rate", "0/1")).partition("/")
    rate = float(numerator) / float(denominator or 1)
    report.check(
        "the composite is H.264 with 1920x1080 at 30 frames per second, with AAC sound",
        (picture.get("codec_name"), picture.get("width"), picture.get("height"), sound.get("codec_name")) == ("h264", WIDTH, HEIGHT, "aac")
        and abs(rate - 30) < 0.5,
        streams,
    )
    report.check(
        "the server states the size of the picture and the length of the video",
        (composite["file"].get("width"), composite["file"].get("height")) == (WIDTH, HEIGHT) and abs(composite["file"]["duration_ms"] / 1000 - length) < 0.1,
        composite["file"],
    )
    report.check(
        "the length of the composite agrees with the time between its first piece and the stop",
        stopped - recording_seen - 1 <= length <= stopped - requested + 3 and abs(length - (composite["duration_ms"] or 0) / 1000) <= 3,
        (length, stopped - recording_seen, stopped - requested, composite["duration_ms"]),
    )
    frames = int(picture.get("nb_frames") or 0)
    report.check("the composite holds the frames of its length", abs(frames - length * 30) <= 45, frames)
    check_frame(scene, "near its start", video, 4.0)
    check_frame(scene, "near its end", video, length - 4.0)
    check_motion(scene, video, length)
    check_tones(scene, "composite at its start", video, (TONE_A, TONE_B, TONE_SHARE))
    if not arguments.speech:
        check_tones(scene, "composite at its end", video, (TONE_A, TONE_B, TONE_SHARE), start=length - 7)
    level = scene.media.mean_level(video, length / 2 - 3, 6)
    report.check("the composite has sound in its middle", level is not None and level >= -40, level)
    for name, tone in (("A", TONE_A), ("B", TONE_B), ("C", TONE_SHARE)):
        path = f"{RECORDINGS}/{recording_id}/{audio[name]['id']}.ogg"
        probe = scene.media.probe(path)
        stream = probe["streams"][0] if len(probe["streams"]) == 1 else {}
        length = float(probe["format"]["duration"])
        report.check(
            f"the audio file of {name} is one Opus stream in Ogg, as long as the recording",
            stream.get("codec_name") == "opus" and "ogg" in probe["format"]["format_name"] and stopped - recording_seen - 1 <= length <= stopped - requested + 3,
            (probe, stopped - recording_seen),
        )
        check_tones(scene, f"audio file of {name} at its start", path, (tone,))
        if not arguments.speech:
            check_tones(scene, f"audio file of {name} at its end", path, (tone,), start=length - 7)

    check_downloads(scene, recording_id, composite, audio["A"])
    scene.everybody_leaves()
    check_transcript(scene, recording_id, arguments)
    scene.remove_and_check_nothing_is_left(recording_id, "recording")


def phase_everybody_leaves(scene: Scene, arguments: argparse.Namespace) -> None:
    """A recording of a channel that everybody leaves: two people leave through the API, the
    third loses the connection without a word."""
    start_call(scene, arguments)
    recording_id = scene.start_recording()
    scene.wait_recording(recording_id, ["composite", "microphone", "microphone", "screen_share_audio"])
    time.sleep(8)
    a, b, c = scene.participants
    left = time.time()
    scene.owner.call("DELETE", "/api/v1/call", expect=(200, 204))
    a.leave()
    scene.members[0].call("DELETE", "/api/v1/call", expect=(200, 204))
    b.leave()
    c.kill()
    scene.participants.clear()
    scene.in_call.clear()
    detail = scene.wait_final(recording_id, 240)
    recording = detail["recording"]
    report.info(f"everybody left: final state {recording['state']} {time.time() - left:.1f} s after the first person left")
    stop = recording.get("stop") or {}
    report.check("the server stopped the recording because its call ended", stop.get("reason") == "call_ended" and stop.get("by") is None, stop)
    report.check(
        "the recording of a call that everybody left is ready, with a checked file for each of its four segments",
        recording["state"] == "ready" and len(detail["segments"]) == 4 and all(segment["state"] == "ended" and (segment.get("file") or {}).get("check") == "passed" for segment in detail["segments"]),
        [(segment["source"], segment["state"], segment.get("failure"), bool(segment.get("file")), (segment.get("file") or {}).get("problems")) for segment in detail["segments"]],
    )
    report.check("its reservation has ended", scene.reservation(recording_id) == "")
    report.check("no call is left in the channel", scene.people_in_call() == set(), scene.people_in_call())
    scene.remove_and_check_nothing_is_left(recording_id, "everybody left")


def phase_recorder_lost(scene: Scene, arguments: argparse.Namespace) -> None:
    """The processes of the recorder are killed during a recording. The container ends and its
    restart rule starts it again (ADR 0601)."""
    deployment = scene.deployment
    start_call(scene, arguments)
    recording_id = scene.start_recording()
    sources = ["composite", "microphone", "microphone", "screen_share_audio"]
    first = scene.wait_recording(recording_id, sources)
    first_ids = {segment["id"] for segment in first["segments"]}
    time.sleep(8)
    restarts = deployment.restarts("recorder")
    killed = time.time()
    # Every process of the user of the recorder gets the kill signal. The first process of
    # the container then ends, and with it the container.
    deployment.inside("recorder", "kill -9 -1", check=False)
    wait_for("the recorder container was started again", lambda: deployment.restarts("recorder") > restarts, 60)
    report.ok(f"the recorder container ended and its restart rule started it again {time.time() - killed:.1f} s after the kill")

    def lost() -> bool:
        detail = scene.recording(recording_id)
        return all(segment["state"] == "failed" for segment in detail["segments"] if segment["id"] in first_ids)

    wait_for("the server declares the four jobs lost", lost, 90)
    report.ok(f"the server declared the jobs of the killed recorder lost after {time.time() - killed:.1f} s")
    detail = scene.recording(recording_id)
    report.check("the recording stays `recording` while the call runs", detail["recording"]["state"] == "recording", detail["recording"]["state"])

    def records_again() -> dict[str, Any] | None:
        detail = scene.recording(recording_id)
        if detail["recording"]["state"] != "recording":
            raise Failed(f"the recording became {detail['recording']['state']} after the loss of the recorder")
        again = sorted(segment["source"] for segment in detail["segments"] if segment["state"] == "recording" and segment["id"] not in first_ids)
        if again != sorted(sources):
            return None
        composite = next(segment for segment in detail["segments"] if segment["state"] == "recording" and segment["source"] == "composite")
        pieces = deployment.inside("server", f"ls {RECORDINGS}/{recording_id}/{composite['id']} 2>/dev/null | grep -c '\\.ts$' || true").strip()
        return detail if pieces.isdigit() and int(pieces) >= 1 else None

    wait_for("the recorder that started again records every source in new segments", records_again, 180, 1)
    report.ok(f"new jobs record every source {time.time() - killed:.1f} s after the kill")
    time.sleep(8)
    scene.stop_recording()
    detail = scene.wait_final(recording_id, 240)
    recording = detail["recording"]
    segments = detail["segments"]
    summary = [
        (segment["source"], segment["state"], segment.get("failure"), bool(segment.get("file")), (segment.get("file") or {}).get("recovered"), (segment.get("file") or {}).get("problems"))
        for segment in segments
    ]
    report.info(f"recorder lost: final state {recording['state']}, segments {summary}")
    report.check("the recording with lost jobs is partly failed", recording["state"] == "partly_failed", recording["state"])
    lost_segments = [segment for segment in segments if segment["id"] in first_ids]
    later = [segment for segment in segments if segment["id"] not in first_ids]
    report.check(
        "the four segments of the killed recorder failed with `recorder_lost`",
        all(segment["state"] == "failed" and segment.get("failure") == "recorder_lost" for segment in lost_segments),
        summary,
    )
    report.check(
        "the server recovered a file for each lost segment: the video from the pieces, the audio from `rescued/`",
        all(segment.get("file") and segment["file"]["recovered"] for segment in lost_segments),
        summary,
    )
    report.check(
        "the segments of the recorder that started again ended in order with checked files",
        sorted(segment["source"] for segment in later if segment["state"] == "ended" and (segment.get("file") or {}).get("check") == "passed") == sorted(sources),
        summary,
    )
    rescued = [name for kind, _, name in deployment.recording_files() if name.startswith("rescued/")]
    report.check("after the processing the directories of the lost jobs below `rescued/` are deleted", not rescued, rescued[:6])
    report.check("the reservation has ended", scene.reservation(recording_id) == "")
    scene.everybody_leaves()
    scene.remove_and_check_nothing_is_left(recording_id, "recorder lost")


def phase_low_storage(scene: Scene, arguments: argparse.Namespace) -> None:
    """The volume of the recordings has less free space than the reserve (ADR 0605)."""
    health = request("GET", f"{arguments.origin}/api/v1/health", expect=200).json()
    report.check("the health route states that the storage is low", health.get("storage_low") is True, health)
    participant = scene.enter(scene.owner, "A", ["--mic-hz", str(TONE_A)])
    participant.wait("published")
    wait_for("the server lists the person in the call", lambda: scene.people_in_call() == {scene.owner.account_id}, 30)
    answer = scene.owner.call("PUT", f"/api/v1/channels/{scene.channel_id}/call/recording", {})
    report.check(
        "a start of a recording is refused with `storage_low` (503)",
        answer.status == 503 and answer.error_code() == "storage_low",
        (answer.status, answer.error_code()),
    )
    time.sleep(3)
    listed = scene.owner.get(f"/api/v1/workspaces/{scene.workspace_id}/recordings")["recordings"]
    report.check("no recording exists", listed == [], listed)
    report.check("no reservation exists", scene.deployment.sql("select count(*) from storage_reservations") == "0")
    files = scene.deployment.recording_files()
    report.check("the volume of the recordings holds nothing", files == [], files[:6])
    scene.everybody_leaves()


def recorder_reach_checks(deployment: Deployment) -> None:
    """What the recorder container reaches on the networks of the deployment (ADR 0015)."""

    def reaches(host: str, port: int) -> bool:
        script = f"timeout 3 bash -c 'exec 3<>/dev/tcp/{host}/{port}' 2>/dev/null && echo yes || echo no"
        return deployment.inside("recorder", script, check=False).strip() == "yes"

    needed = {name: reaches(name, port) for name, port in (("server", 8080), ("livekit", 7880), ("redis", 6379))}
    report.check("the recorder reaches the server, the media server and Redis", all(needed.values()), needed)
    report.check("the recorder does not reach PostgreSQL", not reaches("postgres", 5432))


def recorder_log_checks(deployment: Deployment) -> None:
    """A room token is a signed token of three parts, each in base64url; its first two parts
    begin with `eyJ`. No log may hold one, in a page address or in any other form."""
    token = re.compile(r"eyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}")
    log = deployment.log("recorder")
    report.check("the log of the recorder ran through four recordings", log.count("START_RECORDING") >= 4, log.count("START_RECORDING"))
    report.check("the log of the recorder holds no room token", "token=" not in log and not token.search(log))
    servers = sorted(set(re.findall(r"stun:[A-Za-z0-9.:-]+", log)))
    report.info(f"STUN servers named in the log of the recorder: {servers or 'none'}")
    log = deployment.log("server")
    report.check(
        "the log of the server holds the requests for the layout page without their query, and no token",
        "/recording-layout/" in log and "recording-layout/?" not in log and "token=" not in log and not token.search(log),
    )
    for service in ("livekit", "worker"):
        report.check(f"the log of {service} holds no token", not token.search(deployment.log(service)))


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("phase", choices=["main", "low-storage"])
    parser.add_argument("--project", required=True)
    parser.add_argument("--env-file", required=True)
    parser.add_argument("--compose-file", action="append", required=True)
    parser.add_argument("--origin", required=True)
    parser.add_argument("--participant", required=True)
    parser.add_argument("--speech")
    parser.add_argument("--models", action="store_true")
    parser.add_argument("--keep-files")
    arguments = parser.parse_args()
    deployment = Deployment(arguments)
    scene: Scene | None = None
    try:
        if arguments.phase == "main":
            scene = Scene(arguments, deployment, members=3)
            for step in (phase_recording, phase_everybody_leaves, phase_recorder_lost):
                step(scene, arguments)
            recorder_reach_checks(deployment)
            recorder_log_checks(deployment)
        else:
            scene = Scene(arguments, deployment, members=0)
            phase_low_storage(scene, arguments)
    except (Failed, Unexpected, subprocess.SubprocessError, OSError, ValueError, KeyError, StopIteration) as error:
        report.fail("the test stopped at a step that the later steps need", f"{type(error).__name__}: {error}")
    finally:
        if scene is not None:
            for participant in scene.participants:
                participant.kill()
    if report.failures:
        print(f"{report.failures} check(s) failed")
        return 1
    print(f"all checks of the phase {arguments.phase} passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
