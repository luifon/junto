#!/usr/bin/env python3
"""Tests of the steps of tools/ci/rust-linux.sh. The job runs with a program in place of
tools/linux-container.sh that records each command, so that the tests read which tests the
Linux job runs."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
RECORDER = """#!/bin/sh
echo "$*" >>"$RECORD"
case "$*" in *"$FAIL_ON"*) [ -z "$FAIL_ON" ] || exit 17 ;; esac
exit 0
"""
DEVICE_TESTS = "cargo test --locked -p junto-media --test linux_audio --test tls_trust -- --ignored"
SCREENS = ("sh -c JUNTO_SCREENS_FILES=0 cargo run --locked -p junto-client --features test-support "
           "--bin junto-screens -- /tmp/junto-screens")


@unittest.skipIf(os.name == "nt", "the job runs with programs written as shell scripts")
class LinuxSteps(unittest.TestCase):
    def run_job(self, fail_on=""):
        """Runs the job in a copy of the files it reads; returns its exit code and the
        recorded commands."""
        with tempfile.TemporaryDirectory(prefix="junto-job-") as directory:
            root = Path(directory)
            (root / "tools" / "ci").mkdir(parents=True)
            shutil.copy(ROOT / "tools" / "ci" / "rust-linux.sh", root / "tools" / "ci")
            shutil.copy(ROOT / "tools" / "ci" / "pins.sh", root / "tools" / "ci")
            recorder = root / "tools" / "linux-container.sh"
            recorder.write_text(RECORDER)
            recorder.chmod(0o755)
            record = root / "record"
            record.touch()
            environment = dict(os.environ, RECORD=str(record), FAIL_ON=fail_on)
            result = subprocess.run(["sh", str(root / "tools" / "ci" / "rust-linux.sh")],
                                    env=environment, capture_output=True, text=True, timeout=60)
            return result.returncode, record.read_text().splitlines(), result.stdout + result.stderr

    def test_the_linux_job_runs_the_tests_of_sound_devices_and_of_turn_over_tls(self):
        status, commands, output = self.run_job()
        self.assertEqual(status, 0, output)
        self.assertIn(DEVICE_TESTS, commands)
        self.assertLess(commands.index("cargo build --workspace --locked"), commands.index(DEVICE_TESTS))

    def test_the_linux_job_measures_the_empty_bands_of_every_screen(self):
        status, commands, output = self.run_job()
        self.assertEqual(status, 0, output)
        self.assertIn(SCREENS, commands)
        status, commands, _ = self.run_job(fail_on="--bin junto-screens")
        self.assertEqual(status, 17)
        self.assertEqual(commands[-1], SCREENS)

    def test_a_failed_test_of_sound_devices_fails_the_linux_job(self):
        status, commands, _ = self.run_job(fail_on="--test linux_audio")
        self.assertEqual(status, 17)
        self.assertEqual(commands[-1], DEVICE_TESTS)


if __name__ == "__main__":
    unittest.main()
