#!/usr/bin/env python3
"""Start the sound services of the Linux build image many times (ADR 0750)."""
import os
from pathlib import Path
import shutil
import signal
import subprocess
import tempfile
import time
import unittest

SOUND = Path(__file__).with_name("sound.sh")
PROGRAMS = ("dbus-run-session", "pipewire", "wireplumber", "pipewire-pulse", "pactl", "pw-cli")
STARTS = 60


def running(group):
    """The processes of the group that have not ended; a container without an init process
    keeps the ended ones as zombies."""
    found = []
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            fields = (entry / "stat").read_text().rsplit(")", 1)[1].split()
        except OSError:
            continue
        if fields[0] != "Z" and int(fields[2]) == group:
            found.append(int(entry.name))
    return found


def signal_group(group, number):
    for process in running(group):
        try:
            os.kill(process, number)
        except ProcessLookupError:
            pass


def stop_group(group):
    signal_group(group, signal.SIGTERM)
    deadline = time.monotonic() + 30
    while running(group):
        if time.monotonic() >= deadline:
            signal_group(group, signal.SIGKILL)
            return
        time.sleep(0.05)


@unittest.skipUnless(all(shutil.which(program) for program in PROGRAMS),
                     "PipeWire, WirePlumber and pactl are installed in the Linux build image")
class SoundStart(unittest.TestCase):
    def test_each_start_sets_the_virtual_devices_as_the_defaults(self):
        script = f"{SOUND} && pactl get-default-sink && pactl get-default-source"
        for start in range(1, STARTS + 1):
            with tempfile.TemporaryDirectory() as runtime:
                os.chmod(runtime, 0o700)
                environment = dict(os.environ, XDG_RUNTIME_DIR=runtime, JUNTO_SOUND_LOGS=runtime,
                                   JUNTO_SOUND_ARTIFACTS=os.path.join(runtime, "artifacts"))
                environment.pop("DBUS_SESSION_BUS_ADDRESS", None)
                process = subprocess.Popen(["dbus-run-session", "--", "sh", "-c", script],
                                           env=environment, stdout=subprocess.PIPE,
                                           stderr=subprocess.PIPE, text=True,
                                           start_new_session=True)
                try:
                    output, diagnostics = process.communicate(timeout=120)
                finally:
                    stop_group(process.pid)
                    process.wait()
                self.assertEqual(process.returncode, 0,
                                 f"start {start} of {STARTS}:\n{output}\n{diagnostics}")
                self.assertEqual(output.split(), ["junto-test-output", "junto-test-input.monitor"],
                                 f"start {start} of {STARTS}")


if __name__ == "__main__":
    unittest.main()
