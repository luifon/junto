#!/usr/bin/env python3
"""Tests of the steps of tools/ci/rust-other-systems.sh on macOS and on Windows. The job runs
with programs in place of cargo, python3 and uname that record each command, so that the
tests read which window tests, database and start checks each system runs."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

JOB = Path(__file__).with_name("rust-other-systems.sh")
REPOSITORY = JOB.parents[2]
# A program in place of cargo and python3: it records its name and its arguments, one
# command per line, and fails when its arguments hold FAIL_ON.
RECORDER = """#!/bin/sh
line=$(basename "$0")
for argument in "$@"; do line="$line $argument"; done
echo "$line" >>"$RECORD"
if [ -n "${FAIL_ON:-}" ]; then
    case "$line" in *"$FAIL_ON"*) exit 1 ;; esac
fi
exit 0
"""
SYSTEMS = {"macos": "Darwin", "windows": "MINGW64_NT-10.0-26100"}
WINDOW_TESTS = "cargo test --no-fail-fast --locked --workspace --test app --"
WINDOWS_RECORDING_SKIPS = [
    'recordings::',
    'transcripts::',
    'admin_pages::exports_actions::a_saved_channel_export_contains_messages_threads_files_recordings_and_transcripts',
    'system_lines::the_end_of_a_transcript_is_a_line_that_states_whether_it_is_ready_or_failed',
]
DATABASE = {"macos": "python3 tools/with_native_postgres.py", "windows": "python3 tools/windows/with_test_postgres.py"}


@unittest.skipIf(os.name == "nt", "the job runs with programs written as shell scripts")
class JobSteps(unittest.TestCase):
    def run_job(self, system, fail_on="", parts=""):
        """Runs the job in a copy of the files it reads; returns its exit code, the recorded
        commands and its output."""
        temporary = Path(tempfile.mkdtemp(prefix="junto-job-"))
        self.addCleanup(shutil.rmtree, temporary, True)
        root = temporary / "repository"
        (root / "tools" / "ci").mkdir(parents=True)
        shutil.copy(JOB, root / "tools" / "ci")
        shutil.copy(REPOSITORY / "tools" / "ci" / "pins.sh", root / "tools" / "ci")
        programs = temporary / "programs"
        programs.mkdir()
        for name in ("cargo", "python3"):
            (programs / name).write_text(RECORDER)
        (root / "tools" / "check-condition-wait.sh").write_text(RECORDER)
        (programs / "uname").write_text(f"#!/bin/sh\necho {SYSTEMS[system]}\n")
        for program in [*programs.iterdir(), root / "tools" / "check-condition-wait.sh"]:
            program.chmod(0o755)
        record = temporary / "record"
        record.touch()
        environment = {
            "PATH": f"{programs}{os.pathsep}/usr/bin{os.pathsep}/bin",
            "HOME": str(temporary),
            "TMPDIR": str(temporary),
            "RECORD": str(record),
            "FAIL_ON": fail_on,
            "CARGO_TARGET_DIR": str(temporary / "target"),
            "JUNTO_CI_PARTS": parts,
        }
        result = subprocess.run(["bash", str(root / "tools" / "ci" / "rust-other-systems.sh")], env=environment,
                                capture_output=True, text=True, timeout=120)
        return result.returncode, record.read_text().splitlines(), result.stdout + result.stderr

    def window_steps(self, commands, system):
        without = [c for c in commands if c.startswith(WINDOW_TESTS)]
        with_database = [c for c in commands if c.startswith(f"{DATABASE[system]} {WINDOW_TESTS}")]
        return without, with_database

    def test_a_change_of_python_unit_tests_runs_only_the_python_unit_tests(self):
        for system, own in [("macos", "python3 tools/test_with_native_postgres.py"),
                            ("windows", 'python3 -m unittest discover -s tools/windows -p test_*.py')]:
            with self.subTest(system=system):
                status, commands, output = self.run_job(system, parts="python")
                self.assertEqual(status, 0, output)
                self.assertEqual(commands, ['python3 -m unittest discover -s packaging -p test_*.py',
                                            "python3 tools/test_test_services.py", own])

    def test_macos_runs_the_window_tests_with_postgresql_and_has_no_live_tests(self):
        status, commands, output = self.run_job("macos")
        self.assertEqual(status, 0, output)
        without, with_database = self.window_steps(commands, "macos")
        self.assertEqual(without, [f"{WINDOW_TESTS} updates::"])
        self.assertEqual(with_database, [f"{DATABASE['macos']} {WINDOW_TESTS} --skip updates::"])
        self.assertFalse(any("live::" in c for c in commands), commands)
        self.assertLess(commands.index("python3 tools/test_with_native_postgres.py"),
                        commands.index(f"{DATABASE['macos']} --fetch"))
        self.assertLess(commands.index(f"{DATABASE['macos']} --fetch"), commands.index(with_database[0]))

    def test_both_systems_run_the_file_storage_checks_and_keep_their_failure(self):
        for system in SYSTEMS:
            status, commands, output = self.run_job(system)
            self.assertEqual(status, 0, output)
            self.assertIn("cargo test --no-fail-fast --locked -p junto-server --lib recordings::files::", commands)
            status, _, _ = self.run_job(system, fail_on="recordings::files::")
            self.assertNotEqual(status, 0, system)

    def test_both_systems_measure_the_emoji_of_their_fonts(self):
        for system in SYSTEMS:
            status, commands, output = self.run_job(system)
            self.assertEqual(status, 0, output)
            self.assertIn("cargo test --no-fail-fast --locked --workspace --test emoji_fonts", commands, system)
            status, _, _ = self.run_job(system, fail_on="--test emoji_fonts")
            self.assertNotEqual(status, 0, system)

    def test_macos_starts_the_bundle_of_the_build_until_its_first_frame(self):
        status, commands, output = self.run_job("macos")
        self.assertEqual(status, 0, output)
        bundles = [c.split() for c in commands if c.startswith("python3 packaging/bundle.py ")]
        starts = [c.split() for c in commands if c.startswith("python3 packaging/check_start.py ")]
        self.assertEqual(len(bundles), 1, commands)
        self.assertEqual(len(starts), 1, commands)
        _, _, binary, bundle, recovery, icon = bundles[0]
        self.assertTrue(binary.endswith("/target/debug/junto"), binary)
        self.assertTrue(recovery.endswith("/target/debug/junto-recover"), recovery)
        self.assertTrue(icon.endswith("/target/debug/junto-icon"), icon)
        self.assertTrue(bundle.endswith("/Junto.app"), bundle)
        self.assertEqual(starts[0][2:], [bundle])
        self.assertLess(commands.index(" ".join(bundles[0])), commands.index(" ".join(starts[0])))

    def test_a_bundle_that_draws_no_first_frame_fails_the_macos_job(self):
        status, commands, output = self.run_job("macos", fail_on="packaging/check_start.py")
        self.assertEqual(status, 1)
        self.assertIn("First frame of the macOS bundle (exit 1): python3 packaging/check_start.py", output)

    def test_a_failed_step_does_not_stop_the_later_steps_and_the_job_lists_every_failure(self):
        for system in SYSTEMS:
            status, commands, output = self.run_job(system, fail_on="--test emoji_fonts")
            self.assertEqual(status, 1, system)
            self.assertTrue(any(c.startswith(WINDOW_TESTS) for c in commands), system)
            failed = output.split("=== Failed steps", 1)[1]
            self.assertIn("Emoji of the fonts of the system (exit 1): cargo test --no-fail-fast --locked "
                          "--workspace --test emoji_fonts", failed, system)
            self.assertNotIn("Lint", failed, system)

    def test_native_macos_windows_run_only_on_macos_and_failure_fails_the_job(self):
        command = "cargo test --no-fail-fast --locked --workspace --test native_macos_windows"
        for system in SYSTEMS:
            status, commands, output = self.run_job(system)
            self.assertEqual(status, 0, output)
            self.assertEqual(command in commands, system == "macos")
        status, _, output = self.run_job("macos", fail_on="--test native_macos_windows")
        self.assertEqual(status, 1)
        self.assertIn("Native macOS window placement and Dock reopening (exit 1)", output)

    def test_every_cargo_test_runs_every_test_program(self):
        for system in SYSTEMS:
            status, commands, output = self.run_job(system)
            self.assertEqual(status, 0, output)
            tests = [c for c in commands if " test " in f" {c} " and "cargo" in c]
            self.assertTrue(tests, system)
            self.assertTrue(all("cargo test --no-fail-fast " in c for c in tests), tests)

    def test_a_failed_window_test_fails_the_macos_job(self):
        status, _, _ = self.run_job("macos", fail_on=f"{WINDOW_TESTS} --skip updates::")
        self.assertNotEqual(status, 0)

    def test_windows_runs_file_window_tests_and_starts_no_bundle(self):
        status, commands, output = self.run_job("windows")
        self.assertEqual(status, 0, output)
        without, with_database = self.window_steps(commands, "windows")
        self.assertEqual(without, [f"{WINDOW_TESTS} updates::"])
        skips = " ".join(f"--skip {name}" for name in ["updates::", *WINDOWS_RECORDING_SKIPS])
        self.assertEqual(with_database, [f"{DATABASE['windows']} {WINDOW_TESTS} {skips}"])
        self.assertNotIn("python3 tools/with_native_postgres.py --fetch", commands)
        self.assertIn("python3 packaging/check_windows.py", commands)
        self.assertFalse(any("packaging/check_start.py" in c or "packaging/bundle.py" in c for c in commands))

    def test_windows_runs_the_live_window_tests_with_native_livekit_and_keeps_their_failure(self):
        live = (f"{DATABASE['windows']} python3 tools/with_native_livekit.py "
                "cargo test --no-fail-fast --locked --workspace --test app live:: -- --ignored")
        status, commands, output = self.run_job("windows")
        self.assertEqual(status, 0, output)
        self.assertIn(live, commands)
        self.assertLess(commands.index("python3 tools/with_native_livekit.py --fetch"), commands.index(live))
        self.assertLess(commands.index("python3 tools/test_native_programs.py"), commands.index(live))
        status, _, output = self.run_job("windows", fail_on="live:: -- --ignored")
        self.assertEqual(status, 1)
        self.assertIn("Window tests with PostgreSQL and LiveKit (exit 1)", output)

    def test_windows_checks_the_dialogs_of_the_system_after_the_tests_also_after_a_failure(self):
        media = "cargo test --no-fail-fast --locked -p junto-media --lib --test native_exceptions --test windows_capture"
        for fail_on, expected in [("", 0), ("windows_capture", None), ("dialogs.py check", 1)]:
            status, commands, output = self.run_job("windows", fail_on=fail_on)
            if expected is None:
                self.assertNotEqual(status, 0, output)
            else:
                self.assertEqual(status, expected, output)
            records = [c for c in commands if c.startswith("python3 tools/windows/dialogs.py record ")]
            checks = [c for c in commands if c.startswith("python3 tools/windows/dialogs.py check ")]
            self.assertEqual(len(records), 1, commands)
            self.assertEqual(checks, [records[0].replace(" record ", " check ")], commands)
            self.assertLess(commands.index(records[0]), commands.index(media))
            self.assertEqual(commands[-1], checks[0], fail_on)

    def test_macos_does_not_check_the_dialogs_of_windows(self):
        status, commands, output = self.run_job("macos")
        self.assertEqual(status, 0, output)
        self.assertFalse(any("dialogs.py" in c for c in commands))

    def test_both_systems_lint_each_crate_alone_and_keep_its_failure(self):
        for system in SYSTEMS:
            status, commands, output = self.run_job(system)
            self.assertEqual(status, 0, output)
            for targets in ("", "--all-targets "):
                self.assertIn(f"python3 tools/ci/each_crate.py clippy --locked {targets}--keep-going -- -D warnings",
                              commands, system)
            status, _, output = self.run_job(system, fail_on="each_crate.py clippy --locked --all-targets")
            self.assertEqual(status, 1, system)
            self.assertIn("Lint each crate alone (exit 1)", output, system)

    def test_both_systems_lint_the_device_tests_of_the_media_crate(self):
        for system in SYSTEMS:
            status, commands, output = self.run_job(system)
            self.assertEqual(status, 0, output)
            self.assertIn(
                "cargo clippy --locked -p junto-media --features native-device-tests --all-targets "
                "--keep-going -- -D warnings", commands, system)


if __name__ == "__main__":
    unittest.main()
