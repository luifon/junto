"""The programs of the test bundles run on this machine with a signature that is not Apple's."""
import pathlib
import subprocess
import sys
import tempfile
import unittest

from configuration import architecture
from programs import MINIMUM_MACOS, outcome, program


class ProgramTests(unittest.TestCase):
    @unittest.skipUnless(sys.platform == "darwin", "Mach-O programs")
    def test_programs_hold_only_code_of_this_machine_and_end_with_their_status(self):
        expected = {"aarch64": "arm64", "x86_64": "x86_64"}[architecture()]
        with tempfile.TemporaryDirectory(prefix="junto-programs-") as temporary:
            for status in [0, 1, 7]:
                built = program(pathlib.Path(temporary) / f"exit-{status}", status)
                archs = subprocess.run(["/usr/bin/lipo", "-archs", str(built)], capture_output=True, text=True, timeout=30)
                self.assertEqual(archs.returncode, 0, outcome(archs))
                self.assertEqual(archs.stdout.split(), [expected])
                commands = subprocess.run(["/usr/bin/otool", "-l", str(built)], capture_output=True, text=True, timeout=30)
                self.assertEqual(commands.returncode, 0, outcome(commands))
                self.assertIn(f"minos {MINIMUM_MACOS}", commands.stdout)
                subprocess.run(["/usr/bin/codesign", "--force", "--sign", "-", str(built)], check=True, capture_output=True, timeout=30)
                ended = subprocess.run([str(built)], capture_output=True, timeout=30)
                self.assertEqual(ended.returncode, status, outcome(ended))

    def test_the_report_names_the_command_code_signal_and_both_outputs(self):
        ended = subprocess.CompletedProcess(["junto", "--version"], -9, b"out\n", "err\n")
        report = outcome(ended)
        for part in ["junto --version", "code -9", "signal 9", "out", "err"]:
            self.assertIn(part, report)
        self.assertIn("code 3", outcome(subprocess.CompletedProcess(["junto"], 3, None, None)))


if __name__ == "__main__":
    unittest.main()
