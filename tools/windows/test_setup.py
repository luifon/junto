"""The setup records scheduler diagnostics and reports a refusal to enable them."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("ci-setup.ps1").resolve()


def quoted(value):
    return "'" + str(value).replace("'", "''") + "'"


@unittest.skipUnless(os.name == "nt", "needs Windows PowerShell")
class TaskLog(unittest.TestCase):
    def invoke(self, status):
        with tempfile.TemporaryDirectory(prefix="junto-setup-") as temporary:
            command = (
                f". {quoted(SCRIPT)} -Directory {quoted(temporary)} -FunctionsOnly; "
                "function wevtutil.exe { Write-Output ($args -join '|'); "
                f"$global:LASTEXITCODE = {status} }}; "
                "function Block-TestLiveKit { 'blocked LiveKit' }; "
                "function Install-Git { 'installed Git' }; "
                "function Install-Python { 'installed Python' }; "
                "function Install-Rustup { 'installed rustup' }; "
                "function Install-BuildTools { 'installed Build Tools' }; Invoke-Setup"
            )
            return subprocess.run(
                ["powershell.exe", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
                 "-Command", command], capture_output=True, text=True, timeout=60)

    def test_setup_enables_the_task_scheduler_operational_log(self):
        result = self.invoke(0)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(result.stdout.splitlines()[:6], [
            "sl|Microsoft-Windows-TaskScheduler/Operational|/e:true", "blocked LiveKit",
            "installed Git", "installed Python", "installed rustup", "installed Build Tools"])
        self.assertEqual(result.stdout.count("/e:true"), 1)
        self.assertIn("ready:", result.stdout)

    def test_setup_reports_failure_to_enable_the_operational_log(self):
        result = self.invoke(5)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("the setup failed: could not enable the Task Scheduler operational log", result.stdout)
        self.assertNotIn("installed ", result.stdout)
        self.assertNotIn("ready:", result.stdout)


@unittest.skipUnless(os.name == "nt", "needs Windows PowerShell")
class LiveKitRule(unittest.TestCase):
    """The setup, which runs elevated, blocks the LiveKit program of the tests; the job only
    checks the rule (tools/with_native_livekit.py)."""

    def invoke(self, status):
        with tempfile.TemporaryDirectory(prefix="junto-setup-") as temporary:
            command = (
                f". {quoted(SCRIPT)} -Directory {quoted(temporary)} -FunctionsOnly; "
                "$env:USERPROFILE = 'C:\\Users\\ci'; "
                "function netsh.exe { [Console]::Out.WriteLine($args -join '|'); "
                f"$global:LASTEXITCODE = {status} }}; "
                "try { Block-TestLiveKit; 'blocked' } catch { \"failed: $_\" }"
            )
            return subprocess.run(
                ["powershell.exe", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
                 "-Command", command], capture_output=True, text=True, timeout=60)

    def test_the_rule_of_an_earlier_version_is_replaced_by_a_rule_that_blocks_the_program(self):
        result = self.invoke(0)
        program = "C:\\Users\\ci\\.cache\\junto-livekit\\livekit-1.13.9\\livekit-server.exe"
        self.assertEqual(result.stdout.splitlines(), [
            "advfirewall|firewall|delete|rule|name=junto-test-livekit",
            f"advfirewall|firewall|add|rule|name=junto-test-livekit|dir=in|action=block|program={program}"
            "|profile=any|enable=yes",
            "blocked"], result.stderr)

    def test_a_rule_that_cannot_be_created_fails_the_setup(self):
        result = self.invoke(1)
        self.assertIn("failed: could not create the firewall rule junto-test-livekit", result.stdout)


if __name__ == "__main__":
    unittest.main()
