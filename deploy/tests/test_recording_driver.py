"""Tests of the process handling of deploy/tests/recording_driver.py (ADR 0910).

A script of deploy/ that the driver runs, and every command that the script started, must
end with the driver's call: after the time limit of the call and after a failure of the
driver, so that no backup or restore changes the deployment while the test removes it.
"""

from __future__ import annotations

import argparse
import os
import subprocess
import tempfile
import time
import unittest
from pathlib import Path
from typing import Any
from unittest import mock

from recording_driver import Deployment, Failed


def running(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    return True


def ended_within(pid: int, seconds: float) -> bool:
    deadline = time.monotonic() + seconds
    while running(pid):
        if time.monotonic() > deadline:
            return False
        time.sleep(0.05)
    return True


class ScriptProcesses(unittest.TestCase):
    def setUp(self) -> None:
        self.directory = Path(tempfile.mkdtemp())
        self.addCleanup(lambda: subprocess.run(["rm", "-rf", str(self.directory)], check=False))
        self.child = self.directory / "child"
        # A script that starts a command which ignores the stop signal, and waits for it.
        script = self.directory / "slow.sh"
        script.write_text(f"#!/bin/sh\n(trap '' TERM; exec sleep 300) &\necho $! > '{self.child}'\nwait\n")
        script.chmod(0o755)
        arguments = argparse.Namespace(project="junto-test", env_file=str(self.directory / "env"), compose_file=["local.yaml"])
        self.deployment = Deployment(arguments)
        self.deployment.directory = self.directory

    def child_pid(self) -> int:
        deadline = time.monotonic() + 10
        while not self.child.exists() or not self.child.read_text().strip():
            if time.monotonic() > deadline:
                self.fail("the script did not start its command")
            time.sleep(0.05)
        return int(self.child.read_text())

    def test_a_script_beyond_its_time_limit_is_ended_with_its_commands(self) -> None:
        with self.assertRaises(Failed):
            self.deployment.script("slow.sh", seconds=1)
        self.assertTrue(ended_within(self.child_pid(), 15), "a command of the script still runs")

    def test_a_failure_while_a_script_runs_ends_the_script_and_its_commands(self) -> None:
        original = subprocess.Popen.communicate
        failed: list[bool] = []

        def communicate(process: subprocess.Popen[str], *arguments: Any, **options: Any) -> Any:
            if not failed:
                failed.append(True)
                self.child_pid()
                raise KeyboardInterrupt
            return original(process, *arguments, **options)

        with mock.patch.object(subprocess.Popen, "communicate", communicate):
            with self.assertRaises(KeyboardInterrupt):
                self.deployment.script("slow.sh", seconds=60)
        self.assertTrue(ended_within(self.child_pid(), 15), "a command of the script still runs")


if __name__ == "__main__":
    unittest.main()
