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
WINDOW_TESTS = "cargo test --locked --workspace --test app --"
WINDOWS_ONLY_SKIPS = [
    "recordings::",
    "messages::a_file_is_sent_with_a_message_and_saved_from_one",
    "messages::files_attached_before_a_send_are_in_the_message_in_the_order_of_their_drops",
    "messages::a_pending_file_picker_cannot_retarget_an_upload",
    "messages::the_picture_of_a_message_is_read_again_for_the_next_account_of_the_server",
    "messages::a_message_whose_files_the_retention_removed_says_so",
    "admin_pages::the_owner_exports_a_channel_saves_the_file_and_removes_the_export",
]
DATABASE = {"macos": "python3 tools/with_native_postgres.py", "windows": "python3 tools/windows/with_test_postgres.py"}


@unittest.skipIf(os.name == "nt", "the job runs with programs written as shell scripts")
class JobSteps(unittest.TestCase):
    def run_job(self, system, fail_on=""):
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
        }
        result = subprocess.run(["bash", str(root / "tools" / "ci" / "rust-other-systems.sh")], env=environment,
                                capture_output=True, text=True, timeout=120)
        return result.returncode, record.read_text().splitlines(), result.stdout + result.stderr

    def window_steps(self, commands, system):
        without = [c for c in commands if c.startswith(WINDOW_TESTS)]
        with_database = [c for c in commands if c.startswith(f"{DATABASE[system]} {WINDOW_TESTS}")]
        return without, with_database

    def test_macos_runs_the_window_tests_with_postgresql_and_leaves_out_only_the_live_tests(self):
        status, commands, output = self.run_job("macos")
        self.assertEqual(status, 0, output)
        without, with_database = self.window_steps(commands, "macos")
        self.assertEqual(without, [f"{WINDOW_TESTS} updates::"])
        self.assertEqual(with_database, [f"{DATABASE['macos']} {WINDOW_TESTS} --skip updates:: --skip live::"])
        self.assertLess(commands.index("python3 tools/test_with_native_postgres.py"),
                        commands.index(f"{DATABASE['macos']} --fetch"))
        self.assertLess(commands.index(f"{DATABASE['macos']} --fetch"), commands.index(with_database[0]))

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
        status, commands, _ = self.run_job("macos", fail_on="packaging/check_start.py")
        self.assertNotEqual(status, 0)
        self.assertFalse(any(c.startswith(WINDOW_TESTS) for c in commands))

    def test_a_failed_window_test_fails_the_macos_job(self):
        status, _, _ = self.run_job("macos", fail_on="--skip live::")
        self.assertNotEqual(status, 0)

    def test_windows_leaves_out_the_tests_that_store_files_and_starts_no_bundle(self):
        status, commands, output = self.run_job("windows")
        self.assertEqual(status, 0, output)
        without, with_database = self.window_steps(commands, "windows")
        self.assertEqual(without, [f"{WINDOW_TESTS} updates::"])
        skips = " ".join(f"--skip {name}" for name in ["updates::", "live::", *WINDOWS_ONLY_SKIPS])
        self.assertEqual(with_database, [f"{DATABASE['windows']} {WINDOW_TESTS} {skips}"])
        self.assertNotIn("python3 tools/with_native_postgres.py --fetch", commands)
        self.assertIn("python3 packaging/check_windows.py", commands)
        self.assertFalse(any("packaging/check_start.py" in c or "packaging/bundle.py" in c for c in commands))


if __name__ == "__main__":
    unittest.main()
