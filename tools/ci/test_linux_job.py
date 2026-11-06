#!/usr/bin/env python3
"""Tests of the steps of tools/ci/rust-linux.sh. The job runs with a program in place of
tools/linux-container.sh that records each command, so that the tests read which tests the
Linux job runs."""
import os
from pathlib import Path
import shlex
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
DEVICE_TESTS = "cargo test --no-fail-fast --locked -p junto-media --test linux_audio --test tls_trust -- --ignored"
SCREENS = ("sh -c JUNTO_SCREENS_FILES=0 JUNTO_SCREENS_PART=1/2 cargo run --locked -p junto-client --features "
           "test-support --bin junto-screens -- /tmp/junto-screens")
LEAST_SCREENS = ("sh -c JUNTO_SCREENS_FILES=0 JUNTO_SCREENS_SIZE=720x480 JUNTO_SCREENS_PART=1/2 cargo run --locked "
                 "-p junto-client --features test-support --bin junto-screens -- /tmp/junto-screens")


def quoted(command):
    """A command "sh -c <script>" of a lane as the job lists it when it fails."""
    return shlex.join(["sh", "-c", command[len("sh -c "):]])


WINDOW_LANE = "cargo test --no-fail-fast --workspace --locked --test app"
API_LANE = "cargo test --no-fail-fast --workspace --locked --test api"
LIVE_LANE = "cargo test --no-fail-fast --workspace --locked --test app live:: -- --ignored"
OTHER_LANE = ("sh -c targets=$(python3 tools/ci/cargo_targets.py /target/junto-ci/test-programs.json app api) "
              "&& exec cargo test --no-fail-fast --workspace --locked $targets")
DOC_LANE = "cargo test --no-fail-fast --workspace --locked --doc"


@unittest.skipIf(os.name == "nt", "the job runs with programs written as shell scripts")
class LinuxSteps(unittest.TestCase):
    def run_job(self, fail_on="", parts=""):
        """Runs the job in a copy of the files it reads; returns its exit code and the
        recorded commands."""
        with tempfile.TemporaryDirectory(prefix="junto-job-") as directory:
            root = Path(directory)
            (root / "tools" / "ci").mkdir(parents=True)
            shutil.copy(ROOT / "tools" / "ci" / "rust-linux.sh", root / "tools" / "ci")
            for name in ("pins.sh", "budget.py", "lanes.py", "cargo_targets.py"):
                shutil.copy(ROOT / "tools" / "ci" / name, root / "tools" / "ci")
            recorder = root / "tools" / "linux-container.sh"
            recorder.write_text(RECORDER)
            recorder.chmod(0o755)
            record = root / "record"
            record.touch()
            environment = dict(os.environ, RECORD=str(record), FAIL_ON=fail_on, JUNTO_CI_PARTS=parts)
            result = subprocess.run(["sh", str(root / "tools" / "ci" / "rust-linux.sh")],
                                    env=environment, capture_output=True, text=True, timeout=60)
            return result.returncode, record.read_text().splitlines(), result.stdout + result.stderr

    def test_the_linux_job_runs_the_tests_of_sound_devices_and_of_turn_over_tls(self):
        status, commands, output = self.run_job()
        self.assertEqual(status, 0, output)
        self.assertIn(DEVICE_TESTS, commands)
        self.assertLess(commands.index("cargo build --workspace --locked"), commands.index(DEVICE_TESTS))

    def test_the_linux_job_lints_each_crate_alone_and_keeps_its_failure(self):
        status, commands, output = self.run_job()
        self.assertEqual(status, 0, output)
        for targets in ("", "--all-targets "):
            command = f"python3 tools/ci/each_crate.py clippy --locked {targets}--keep-going -- -D warnings"
            self.assertEqual(commands.count(command), 1)
        status, commands, output = self.run_job(fail_on="each_crate.py clippy --locked --all-targets")
        self.assertEqual(status, 17)
        self.assertIn("Lint each crate alone (exit 17)", output.split("=== Failed steps", 1)[1])

    def test_the_linux_job_measures_the_empty_bands_of_every_screen(self):
        status, commands, output = self.run_job()
        self.assertEqual(status, 0, output)
        self.assertIn(SCREENS, commands)
        status, commands, output = self.run_job(fail_on="--bin junto-screens")
        self.assertEqual(status, 17)
        self.assertIn(f"Screens without empty bands, part 1 of 2 (exit 17): tools/linux-container.sh {quoted(SCREENS)}",
                      output.split("=== Failed steps", 1)[1])

    def test_the_linux_job_draws_every_screen_at_the_least_size_of_the_window(self):
        status, commands, output = self.run_job()
        self.assertEqual(status, 0, output)
        self.assertIn(LEAST_SCREENS, commands)
        self.assertLess(commands.index(SCREENS), commands.index(LEAST_SCREENS))
        status, commands, output = self.run_job(fail_on="JUNTO_SCREENS_SIZE=720x480")
        self.assertEqual(status, 17)
        self.assertIn(f"Screens at the least size of the window, part 1 of 2 (exit 17): tools/linux-container.sh {quoted(LEAST_SCREENS)}",
                      output.split("=== Failed steps", 1)[1])

    def test_a_failed_test_of_sound_devices_fails_the_linux_job(self):
        status, commands, output = self.run_job(fail_on="--test linux_audio")
        self.assertEqual(status, 17)
        self.assertIn(DEVICE_TESTS, commands)
        self.assertIn(f"Linux sound devices and TURN over TLS (exit 17): tools/linux-container.sh {DEVICE_TESTS}",
                      output)

    def test_a_failed_step_does_not_stop_the_later_steps_and_the_job_lists_every_failure(self):
        status, commands, output = self.run_job(fail_on="cargo fmt")
        self.assertEqual(status, 17)
        self.assertIn("git diff --exit-code -- docs/api", commands)
        self.assertIn("cargo test --no-fail-fast --workspace --locked --test app", commands)
        failed = output.split("=== Failed steps", 1)[1]
        self.assertIn("Format (exit 17): tools/linux-container.sh cargo fmt --all --check", failed)
        self.assertNotIn("Lint", failed)

    def test_every_cargo_test_of_the_linux_job_runs_every_test_program(self):
        status, commands, output = self.run_job()
        self.assertEqual(status, 0, output)
        tests = [c for c in commands if "cargo test" in c]
        self.assertTrue(tests)
        self.assertTrue(all("cargo test --no-fail-fast " in c for c in tests), tests)

    def test_the_test_programs_are_built_once_and_every_one_runs_in_a_lane(self):
        status, commands, output = self.run_job()
        self.assertEqual(status, 0, output)
        build = next(c for c in commands if "--no-run" in c and "--workspace" in c)
        lanes = [WINDOW_LANE, LIVE_LANE, API_LANE, OTHER_LANE, DOC_LANE]
        for command in lanes:
            self.assertEqual(commands.count(command), 1, command)
            self.assertLess(commands.index(build), commands.index(command))
        # Every other test program: the build's own list without the two lanes of their own.
        self.assertIn("cargo_targets.py /target/junto-ci/test-programs.json app api", OTHER_LANE)
        self.assertIn("Lanes: app, live, other, screens-1, screens-2, least-1, least-2, one after the other", output)
        self.assertFalse([c for c in commands if c.startswith("cargo test") and "--workspace" in c
                          and c not in lanes and "--no-run" not in c])

    def test_every_run_of_the_screens_is_drawn_in_all_of_its_parts(self):
        status, commands, output = self.run_job()
        self.assertEqual(status, 0, output)
        for screens in (SCREENS, LEAST_SCREENS):
            for part in ("1/2", "2/2"):
                self.assertEqual(commands.count(screens.replace("JUNTO_SCREENS_PART=1/2", f"JUNTO_SCREENS_PART={part}")), 1)
        self.assertFalse([c for c in commands if "--bin junto-screens --" in c and "JUNTO_SCREENS_PART=" not in c])

    def test_a_failed_command_of_a_lane_is_listed_with_its_step(self):
        status, commands, output = self.run_job(fail_on="--test api")
        self.assertEqual(status, 17)
        self.assertIn(WINDOW_LANE, commands)
        self.assertIn(OTHER_LANE, commands)
        failed = output.split("=== Failed steps", 1)[1]
        self.assertIn(f"Server API tests with PostgreSQL (exit 17): tools/linux-container.sh {API_LANE}", failed)
        self.assertNotIn("Window tests", failed)

    def test_the_window_tests_of_calls_run_in_a_lane_of_their_own_and_keep_their_failure(self):
        status, commands, output = self.run_job()
        self.assertEqual(status, 0, output)
        self.assertEqual(commands.count(LIVE_LANE), 1)
        status, commands, output = self.run_job(fail_on="live:: -- --ignored")
        self.assertEqual(status, 17)
        failed = output.split("=== Failed steps", 1)[1]
        self.assertIn(f"Window tests of calls with LiveKit (exit 17): tools/linux-container.sh {LIVE_LANE}", failed)
        self.assertIn(WINDOW_LANE, commands)

    def test_each_step_states_its_start_end_and_duration(self):
        status, commands, output = self.run_job()
        self.assertEqual(status, 0, output)
        self.assertRegex(output, r"=== Format  \d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ\n")
        self.assertRegex(output, r"=== Format: ended \d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ after \d+m\d\ds\n")
        self.assertRegex(output, r"=== Window tests with PostgreSQL  lane app  \S+Z to \S+Z  \d+m\d\ds\n")
        table = output.split("=== Step times", 1)[1]
        for name in ("Build the Linux image and start its services", "Tests with PostgreSQL and the screens",
                     "The committed API documents equal the generated ones"):
            self.assertRegex(table, rf"\d+m\d\ds  {name}\n")

    def test_a_change_of_python_unit_tests_runs_only_the_python_unit_tests(self):
        status, commands, output = self.run_job(parts="python")
        self.assertEqual(status, 0, output)
        self.assertEqual(commands, ["true", "python3 -m unittest discover -s tools/linux -p test_*.py",
                                    "python3 -m unittest discover -s packaging -p test_*.py",
                                    "python3 tools/test_test_services.py"])

    def test_a_failed_image_build_stops_the_linux_job(self):
        status, commands, _ = self.run_job(fail_on="true")
        self.assertEqual(status, 17)
        self.assertEqual(commands, ["true"])


if __name__ == "__main__":
    unittest.main()
