#!/usr/bin/env python3
"""Tests of hold_output.py: the hold ends when the go file appears or when the test that set it
has ended, and never by a time of its own."""
from __future__ import annotations

import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

HOLD = Path(__file__).resolve().parent / "hold_output.py"
# Two MiB on the standard output, more than a pipe holds, so the call waits while it is held.
WRITER = [sys.executable, "-c", "import sys; sys.stdout.buffer.write(b'x' * 2097152)"]


class Hold(unittest.TestCase):
    def hold(self, watched: int):
        directory = Path(tempfile.mkdtemp(prefix="junto-hold-"))
        held, go = directory / "held", directory / "go"
        self.output = directory / "output"
        with open(self.output, "wb") as output:
            process = subprocess.Popen([sys.executable, "-B", str(HOLD), "1024", str(held), str(go), str(watched),
                                        *WRITER], stdout=output)
        self.addCleanup(process.kill)
        deadline = time.monotonic() + 30
        while not held.exists():
            self.assertLess(time.monotonic(), deadline, "the hold did not begin")
            time.sleep(0.05)
        return process, go

    def ended_within(self, process: subprocess.Popen, seconds: float) -> bool:
        try:
            process.wait(timeout=seconds)
            return True
        except subprocess.TimeoutExpired:
            return False

    def test_the_hold_lasts_while_the_test_runs_and_ends_at_the_go_file(self):
        watched = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"])
        self.addCleanup(watched.kill)
        process, go = self.hold(watched.pid)
        self.assertFalse(self.ended_within(process, 3), "the hold ended without the go file while the test ran")
        go.touch()
        self.assertTrue(self.ended_within(process, 30), "the hold did not end at the go file")
        self.assertEqual(process.returncode, 0)
        self.assertEqual(self.output.stat().st_size, 2097152)

    def test_the_hold_ends_when_the_test_has_ended(self):
        watched = subprocess.Popen([sys.executable, "-c", "pass"])
        watched.wait()
        process, _ = self.hold(watched.pid)
        self.assertTrue(self.ended_within(process, 30), "the hold went on after the end of the test")
        self.assertEqual(self.output.stat().st_size, 2097152)


if __name__ == "__main__":
    unittest.main()
