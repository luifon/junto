"""Unit tests of tools/windows/start-run.ps1. They need Windows PowerShell, so they run on Windows
only, in the Windows CI job."""

import os
import pathlib
import subprocess
import sys
import tempfile
import unittest

SCRIPT = pathlib.Path(__file__).resolve().with_name("start-run.ps1")


def powershell(*arguments):
    return subprocess.run(["powershell.exe", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
                           *arguments], capture_output=True, text=True, timeout=120)


def script(directory, *arguments):
    return powershell("-File", str(SCRIPT), "-Directory", str(directory), "-Run", "sample-run", *arguments)


@unittest.skipIf(os.name != "nt", "start-run.ps1 runs on Windows")
class Session(unittest.TestCase):
    def test_an_account_without_a_desktop_session_cannot_start_a_run(self):
        with tempfile.TemporaryDirectory(prefix="junto-start-") as temporary:
            result = script(temporary, "-Check", "-User", "NO-SUCH-MACHINE\\no-desktop")
            self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
            self.assertIn("NO-SUCH-MACHINE\\no-desktop is not logged on to the desktop", result.stdout)

    def test_a_disconnected_or_locked_session_is_refused_with_its_state(self):
        sessions = {
            "@()": "is not logged on to the desktop",
            "@([pscustomobject]@{ Connected = $false; Locked = $false })": "is disconnected",
            "@([pscustomobject]@{ Connected = $true; Locked = $true })": "is locked",
            "@([pscustomobject]@{ Connected = $false; Locked = $true },"
            " [pscustomobject]@{ Connected = $true; Locked = $true })": "is locked",
            "@([pscustomobject]@{ Connected = $true; Locked = $false })": "",
            "@([pscustomobject]@{ Connected = $false; Locked = $false },"
            " [pscustomobject]@{ Connected = $true; Locked = $false })": "",
        }
        for given, expected in sessions.items():
            with self.subTest(sessions=given):
                result = powershell("-Command", f". '{SCRIPT}' -Directory x -Run sample-run -FunctionsOnly; "
                                    f"Get-DesktopRefusal 'MACHINE\\account' {given}")
                self.assertEqual(result.returncode, 0, result.stderr)
                if expected:
                    self.assertIn(expected, result.stdout)
                else:
                    self.assertEqual(result.stdout.strip(), "")

    # The tests of the Windows job run in the desktop session without elevation.
    def test_the_sessions_are_read_without_elevation(self):
        own = powershell("-Command", "[Security.Principal.WindowsIdentity]::GetCurrent().Name; "
                         "[Diagnostics.Process]::GetCurrentProcess().SessionId").stdout.split("\n")
        account, session = own[0].strip(), int(own[1])
        result = powershell("-Command", f". '{SCRIPT}' -Directory x -Run sample-run -FunctionsOnly; "
                            "Get-Session | ForEach-Object { \"$($_.Id) $($_.Account)\" }")
        self.assertEqual(result.returncode, 0, result.stderr)
        if session > 0:
            self.assertIn(f"{session} {account}".lower(), result.stdout.lower())


@unittest.skipIf(os.name != "nt", "start-run.ps1 runs on Windows")
class Ended(unittest.TestCase):
    # tools/ci/ci.sh reports such a run as failed, and "ci.sh forget" removes it.
    def test_a_start_that_is_refused_records_its_end_and_removes_the_bundle(self):
        with tempfile.TemporaryDirectory(prefix="junto-start-") as temporary:
            here = pathlib.Path(temporary) / "runs" / "sample-run"
            here.mkdir(parents=True)
            (here / "commit.bundle").write_bytes(b"bundle")
            result = powershell("-Command", f". '{SCRIPT}' -Directory '{temporary}' -Run sample-run -FunctionsOnly; "
                                "Stop-Start 2 'the run sample-run cannot start: the screen is locked'")
            self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
            self.assertEqual((here / "finished").read_text(), "2\n")
            self.assertEqual((here / "runner.log").read_text().strip(),
                             "the run sample-run cannot start: the screen is locked")
            self.assertFalse((here / "commit.bundle").exists())

    def test_a_run_ends_when_its_recorded_process_no_longer_runs(self):
        with tempfile.TemporaryDirectory(prefix="junto-start-") as temporary:
            here = pathlib.Path(temporary) / "runs" / "sample-run"
            here.mkdir(parents=True)
            self.assertEqual(script(temporary, "-Ended").returncode, 1, "a run without a process file has not ended")
            child = subprocess.Popen([sys.executable, "-c", "import sys; sys.stdin.readline()"],
                                     stdin=subprocess.PIPE, text=True)
            try:
                start = powershell("-Command", f"(Get-Process -Id {child.pid}).StartTime.ToFileTimeUtc()").stdout.strip()
                (here / "process").write_text(f"{child.pid} {start}\n")
                self.assertEqual(script(temporary, "-Ended").returncode, 1, "the process still runs")
                # The same number with another start time is another process.
                (here / "process").write_text(f"{child.pid} {int(start) - 1}\n")
                self.assertEqual(script(temporary, "-Ended").returncode, 0)
                (here / "process").write_text(f"{child.pid} {start}\n")
            finally:
                child.communicate("end\n", timeout=60)
            result = script(temporary, "-Ended")
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(result.stdout.strip(), "ended")


if __name__ == "__main__":
    unittest.main()
