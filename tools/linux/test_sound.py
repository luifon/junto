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


@unittest.skipUnless(Path("/usr/bin/timeout").is_file() and Path("/proc").is_dir(), "Linux timeouts")
class SoundDeadline(unittest.TestCase):
    def test_blocked_commands_and_their_children_end_before_diagnostics(self):
        import socket
        for blocked in ("pw-cli info", "pw-cli ls", "pactl info", "pactl load-module",
                        "pactl set-default-sink", "pactl set-default-source"):
            with self.subTest(blocked=blocked), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                commands = root / "bin"
                commands.mkdir()
                stub = commands / "stub"
                stub.write_text("#!/bin/sh\n" + r'''command="${0##*/} ${1:-}"
if [ "$command" = "$BLOCKED_SOUND_COMMAND" ]; then
    exec python3 -c '
import os, pathlib, signal, subprocess
signal.signal(signal.SIGTERM, signal.SIG_IGN)
child = subprocess.Popen(["sleep", "600"])
pathlib.Path(os.environ["SOUND_CHILD"]).write_text(str(child.pid))
child.wait()
'
fi
if [ "$command" = "pw-cli ls" ]; then
    printf '%s\n' 'metadata.name = "default"'
fi
''')
                stub.chmod(0o755)
                for name in PROGRAMS[1:]:
                    (commands / name).symlink_to(stub)
                limiter = commands / "timeout"
                limiter.write_text('#!/bin/sh\nshift 2\nexec /usr/bin/timeout --signal=KILL 0.3 "$@"\n')
                limiter.chmod(0o755)
                (root / "pulse").mkdir()
                with socket.socket(socket.AF_UNIX) as pulse:
                    pulse.bind(str(root / "pulse/native"))
                    environment = dict(os.environ, PATH=str(commands) + os.pathsep + os.environ['PATH'],
                                       XDG_RUNTIME_DIR=str(root), JUNTO_SOUND_LOGS=str(root),
                                       JUNTO_SOUND_ARTIFACTS=str(root / "artifacts"),
                                       BLOCKED_SOUND_COMMAND=blocked, SOUND_CHILD=str(root / "child"))
                    process = subprocess.Popen(["sh", str(SOUND)], env=environment,
                                               stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                               text=True, start_new_session=True)
                    try:
                        output, diagnostics = process.communicate(timeout=3)
                    except subprocess.TimeoutExpired:
                        self.fail(f"{blocked} exceeded the wall-clock deadline")
                    finally:
                        signal_group(process.pid, signal.SIGKILL)
                        process.wait(timeout=3)
                        process.stdout.close()
                        process.stderr.close()
                    self.assertNotEqual(process.returncode, 0, output)
                    self.assertIn("sound services did not start", diagnostics)
                    self.assertTrue((root / "artifacts/junto-linux-pipewire.log").is_file())
                    child = int((root / "child").read_text())
                    status = Path(f"/proc/{child}/stat")
                    self.assertTrue(not status.exists() or status.read_text().rsplit(')', 1)[1].split()[0] == 'Z',
                                    f"{blocked} left child {child} running")


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
