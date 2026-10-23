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
                f"$global:LASTEXITCODE = {status} }}; Enable-TaskLog"
            )
            return subprocess.run(
                ["powershell.exe", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
                 "-Command", command], capture_output=True, text=True, timeout=60)

    def test_setup_enables_the_task_scheduler_operational_log(self):
        result = self.invoke(0)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(result.stdout.strip(),
                         "sl|Microsoft-Windows-TaskScheduler/Operational|/e:true")

    def test_setup_reports_failure_to_enable_the_operational_log(self):
        result = self.invoke(5)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("could not enable the Task Scheduler operational log", result.stderr)


if __name__ == "__main__":
    unittest.main()
