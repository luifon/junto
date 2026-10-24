#!/usr/bin/env python3
"""Check display readiness and the handover from the X11 probe to the window manager."""
import os
from pathlib import Path
import shutil
import signal
import subprocess
import tempfile
import unittest

DISPLAY = Path(__file__).with_name("display.sh")


def display(*arguments, environment=None):
    with tempfile.TemporaryFile(mode="w+") as stdout, tempfile.TemporaryFile(mode="w+") as stderr:
        command = [DISPLAY, "x11", *arguments]
        process = subprocess.Popen(command, env=environment, stdout=stdout, stderr=stderr,
                                   start_new_session=True)
        try:
            status = process.wait(timeout=45)
        finally:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.wait()
        stdout.seek(0)
        stderr.seek(0)
        return subprocess.CompletedProcess(command, status, stdout.read(), stderr.read())


@unittest.skipUnless(shutil.which("Xvfb") and shutil.which("openbox"), "needs the Linux build image")
class X11Display(unittest.TestCase):
    def test_waits_for_display_state_and_keeps_it_after_the_probe_disconnects(self):
        with tempfile.TemporaryDirectory(prefix="junto-display-") as directory:
            root = Path(directory)
            probe = root / "xdpyinfo"
            probe.write_text(f'''#!/bin/sh
attempts=0
if [ -f "$JUNTO_PROBE_ATTEMPTS" ]; then read -r attempts < "$JUNTO_PROBE_ATTEMPTS"; fi
attempts=$((attempts + 1))
echo "$attempts" > "$JUNTO_PROBE_ATTEMPTS"
[ "$attempts" -ge 3 ] || exit 1
{shutil.which("xdpyinfo")} >/dev/null 2>&1 || exit 1
xprop -root -f JUNTO_READY_PROBE 8s -set JUNTO_READY_PROBE ready
''')
            probe.chmod(0o755)
            result = display(
                "xprop", "-root", "JUNTO_READY_PROBE",
                environment=dict(os.environ, PATH=f"{root}:{os.environ['PATH']}",
                                 JUNTO_PROBE_ATTEMPTS=str(root / "attempts")),
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertGreaterEqual(int((root / "attempts").read_text()), 3)
            self.assertIn('JUNTO_READY_PROBE(STRING) = "ready"', result.stdout)

    def test_repeated_fresh_displays_reach_the_window_manager(self):
        for iteration in range(20):
            with self.subTest(iteration=iteration):
                result = display("xprop", "-root", "_NET_SUPPORTING_WM_CHECK")
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn("window id", result.stdout)


if __name__ == "__main__":
    unittest.main()
