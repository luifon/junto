#!/usr/bin/env python3
"""Tests of the steps of tools/ci/rust-other-systems.sh on macOS and on Windows. The job runs
with programs in place of cargo, python3 and uname that record each command, so that the
tests read which window tests, database and start checks each system runs."""
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

JOB = Path(__file__).with_name("rust-other-systems.sh")
REPOSITORY = JOB.parents[2]
# A program in place of cargo and python3: it records its name and its arguments, one
# command per line, and fails when its arguments hold FAIL_ON. tools/ci/lanes.py runs in the
# Python of the test, so that the commands of its plan are recorded.
RECORDER = """#!/bin/sh
if [ "$(basename "$0")" = python3 ] && [ "${1:-}" = tools/ci/lanes.py ]; then exec "$REAL_PYTHON" "$@"; fi
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
RECORDING_SKIPS = [
    'recordings::',
    'transcripts::',
    'admin_pages::exports_actions::a_saved_channel_export_contains_messages_threads_files_recordings_and_transcripts',
    'system_lines::the_end_of_a_transcript_is_a_line_that_states_whether_it_is_ready_or_failed',
]
DATABASE = {"macos": "python3 tools/with_native_postgres.py", "windows": "python3 tools/windows/with_test_postgres.py"}


@unittest.skipIf(os.name == "nt", "the job runs with programs written as shell scripts")
class JobSteps(unittest.TestCase):
    def run_job(self, system, fail_on="", parts="", extra=None):
        """Runs the job in a copy of the files it reads; returns its exit code, the recorded
        commands and its output."""
        temporary = Path(tempfile.mkdtemp(prefix="junto-job-"))
        self.addCleanup(shutil.rmtree, temporary, True)
        root = temporary / "repository"
        (root / "tools" / "ci").mkdir(parents=True)
        shutil.copy(JOB, root / "tools" / "ci")
        for name in ("pins.sh", "lanes.py", "budget.py"):
            shutil.copy(REPOSITORY / "tools" / "ci" / name, root / "tools" / "ci")
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
            "REAL_PYTHON": sys.executable,
            **(extra or {}),
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

    # The GitHub-hosted macOS runner has no FFmpeg (ADR 0905): its job skips the window tests
    # and the server tests that need it, and states why.
    def test_a_macos_runner_without_ffmpeg_skips_the_tests_that_need_it_and_says_so(self):
        status, commands, output = self.run_job("macos", extra={"JUNTO_CI_FFMPEG": "absent"})
        self.assertEqual(status, 0, output)
        _, with_database = self.window_steps(commands, "macos")
        skips = " ".join(f"--skip {name}" for name in ["updates::", *RECORDING_SKIPS])
        self.assertEqual(with_database, [f"{DATABASE['macos']} {WINDOW_TESTS} {skips}"])
        self.assertIn("JUNTO_CI_FFMPEG=absent: this runner has no FFmpeg", output)
        self.assertFalse([c for c in commands if "-p junto-server --lib" in c or "--test api" in c], commands)
        self.assertIn("the tests of the server run in the Linux job", output)

    def test_both_systems_run_the_library_and_the_api_tests_of_the_server_with_postgresql(self):
        wrapped = {"macos": DATABASE["macos"],
                   "windows": f"{DATABASE['windows']} python3 tools/with_native_ffmpeg.py"}
        for system in SYSTEMS:
            status, commands, output = self.run_job(system)
            self.assertEqual(status, 0, output)
            library = f"{wrapped[system]} cargo test --no-fail-fast --locked -p junto-server --lib"
            api = f"{wrapped[system]} cargo test --no-fail-fast --locked --workspace --test api"
            self.assertIn(library, commands, system)
            self.assertIn(api, commands, system)
            self.assertFalse([c for c in commands if "-p junto-server --lib" in c and c != library], system)
            status, _, output = self.run_job(system, fail_on="--workspace --test api")
            self.assertEqual(status, 1, system)
            self.assertIn("Server tests with PostgreSQL (exit 1)", output, system)

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

    def test_windows_runs_every_window_test_with_ffmpeg_skips_no_test_by_name_and_starts_no_bundle(self):
        status, commands, output = self.run_job("windows")
        self.assertEqual(status, 0, output)
        without, _ = self.window_steps(commands, "windows")
        self.assertEqual(without, [f"{WINDOW_TESTS} updates::"])
        with_ffmpeg = f"{DATABASE['windows']} python3 tools/with_native_ffmpeg.py {WINDOW_TESTS} --skip updates::"
        self.assertIn(with_ffmpeg, commands)
        self.assertEqual([c for c in commands if "--skip" in c], [with_ffmpeg])
        self.assertLess(commands.index("python3 tools/with_native_ffmpeg.py --fetch"), commands.index(with_ffmpeg))
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

    def test_windows_tests_the_panic_reports_of_a_release_build_with_line_tables(self):
        release = "cargo test --no-fail-fast --locked --release -p junto-client --lib panics::"
        for system in SYSTEMS:
            status, commands, output = self.run_job(system)
            self.assertEqual(status, 0, output)
            self.assertEqual(release in commands, system == "windows", system)
        script = JOB.read_text()
        self.assertIn(f"env CARGO_PROFILE_RELEASE_DEBUG=line-tables-only {release}", script)
        status, _, output = self.run_job("windows", fail_on="--release -p junto-client")
        self.assertEqual(status, 1)
        self.assertIn("Panic reports of a release build (exit 1)", output)
        # A change that touches neither the client, gpui nor a Cargo file: no release build.
        status, commands, output = self.run_job("windows", parts="base+server+live+crates")
        self.assertEqual(status, 0, output)
        self.assertNotIn(release, commands)
        self.assertIn("python3 tools/with_native_livekit.py --fetch", commands)

    def test_windows_runs_only_the_parts_that_a_change_selects_and_every_part_in_a_full_run(self):
        server = "-p junto-server --lib"
        live = "--test app live:: -- --ignored"
        release = "--release -p junto-client --lib panics::"
        crates = "tools/ci/each_crate.py clippy"
        everything = (server, live, release, crates)
        for parts, expected in [("", everything), ("base", ()), ("base+server", (server,)),
                                ("base+release+live", (release, live)), ("base+crates", (crates,))]:
            with self.subTest(parts=parts):
                status, commands, output = self.run_job("windows", parts=parts)
                self.assertEqual(status, 0, output)
                for part in everything:
                    self.assertEqual(any(part in c for c in commands), part in expected, part)
                # Every part runs the base: the window tests, the package and the tests without a database.
                self.assertIn(f"{DATABASE['windows']} python3 tools/with_native_ffmpeg.py {WINDOW_TESTS} --skip updates::",
                              commands)
                self.assertIn("python3 packaging/check_windows.py", commands)
                self.assertIn("cargo test --no-fail-fast --locked -p junto-authz -p junto-protocol", commands)
        # The parts are for Windows: the macOS job runs its server tests and the lint of each crate.
        status, commands, output = self.run_job("macos")
        self.assertEqual(status, 0, output)
        self.assertTrue(any(server in c for c in commands) and any(crates in c for c in commands))

    def test_windows_runs_the_tests_after_the_desktop_steps_in_lanes_with_the_output_of_each_step(self):
        status, commands, output = self.run_job("windows")
        self.assertEqual(status, 0, output)
        self.assertIn("Lanes: server, window, release, live, checks, each when its memory fits", output)
        desktop = [commands.index(c) for c in ("python3 packaging/check_windows.py",
                                               "cargo test --no-fail-fast --locked --workspace --test native_panics")]
        lanes = [commands.index(c) for c in commands if c.startswith(("cargo test --no-fail-fast --locked -p junto-authz",
                                                                          f"{DATABASE['windows']} "))
                 and "--fetch" not in c]
        self.assertTrue(lanes)
        self.assertLess(max(desktop), min(lanes))
        for step in ("Server tests with PostgreSQL", "Window tests without a database", "Window tests with PostgreSQL",
                     "Panic reports of a release build", "Window tests with PostgreSQL and LiveKit",
                     "Tests that need no database", "Packaging unit tests", "Windows tool unit tests",
                     "Emoji of the fonts of the system"):
            self.assertRegex(output, rf"=== {step}  lane [a-z]+  \S+Z to \S+Z", step)

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
                command = f"python3 tools/ci/each_crate.py clippy --locked {targets}--keep-going -- -D warnings"
                self.assertEqual(commands.count(command), 1, system)
            status, _, output = self.run_job(system, fail_on="each_crate.py clippy --locked --all-targets")
            self.assertEqual(status, 1, system)
            self.assertIn("Lint each crate alone (exit 1)", output, system)

    def test_the_hosted_windows_job_creates_the_firewall_rule_before_the_job_checks_it(self):
        workflow = (REPOSITORY / ".github" / "workflows" / "ci.yml").read_text()
        step = workflow.index("- name: Firewall rule of the LiveKit of the tests")
        job = workflow.index("run: tools/ci/rust-other-systems.sh")
        self.assertLess(step, job)
        block = workflow[step:job]
        self.assertIn("if: runner.os == 'Windows'", block)
        self.assertIn("-FunctionsOnly", block)
        self.assertIn("Set-NetFirewallProfile -Profile Domain,Private,Public -Enabled True", block)
        self.assertIn("Block-TestLiveKit", block)
        self.assertIn("function Block-TestLiveKit", (REPOSITORY / "tools" / "windows" / "ci-setup.ps1").read_text())

    def test_both_systems_lint_the_device_tests_of_the_media_crate(self):
        for system in SYSTEMS:
            status, commands, output = self.run_job(system)
            self.assertEqual(status, 0, output)
            self.assertIn(
                "cargo clippy --locked -p junto-media --features native-device-tests --all-targets "
                "--keep-going -- -D warnings", commands, system)


if __name__ == "__main__":
    unittest.main()
