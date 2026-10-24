#!/usr/bin/env python3
"""Tests of tools/windows/dialogs.py without the processes of the system."""
from pathlib import Path
import shutil
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).parent))
import dialogs  # noqa: E402

ROWS = (
    '"PickerHost.exe","8120","Console","1","24,112 K"\n'
    '"PickerHost.exe","944","Console","1","20,004 K"\n'
)
NONE = "INFO: No tasks are running which match the specified criteria.\n"


class Dialogs(unittest.TestCase):
    def setUp(self):
        self.directory = Path(tempfile.mkdtemp(prefix="junto-dialogs-"))
        self.addCleanup(shutil.rmtree, self.directory, True)
        self.file = self.directory / "dialogs"

    def run_with(self, command, output):
        with mock.patch.object(dialogs.subprocess, "run") as run:
            run.return_value.stdout = output
            status = dialogs.main([command, str(self.file)])
        self.assertEqual(run.call_args.args[0][-1], "imagename eq PickerHost.exe")
        return status

    def test_the_rows_of_tasklist_are_read_and_its_message_is_left_out(self):
        self.assertEqual(dialogs.parse(ROWS), {("PickerHost.exe", 8120), ("PickerHost.exe", 944)})
        self.assertEqual(dialogs.parse(NONE), set())

    def test_a_dialog_that_ran_before_the_tests_does_not_fail_the_check(self):
        self.assertEqual(self.run_with("record", ROWS), 0)
        self.assertEqual(self.run_with("check", ROWS), 0)

    def test_a_dialog_that_was_closed_during_the_tests_does_not_fail_the_check(self):
        self.assertEqual(self.run_with("record", ROWS), 0)
        self.assertEqual(self.run_with("check", NONE), 0)

    def test_a_dialog_that_the_tests_left_fails_the_check_and_is_named(self):
        self.assertEqual(self.run_with("record", NONE), 0)
        with mock.patch("sys.stderr") as error:
            self.assertEqual(self.run_with("check", ROWS), 1)
        written = "".join(call.args[0] for call in error.write.call_args_list)
        self.assertIn("PickerHost.exe, process 944", written)
        self.assertIn("PickerHost.exe, process 8120", written)

    def test_an_unknown_command_is_refused(self):
        with mock.patch("sys.stderr"):
            self.assertEqual(dialogs.main(["end", str(self.file)]), 2)


if __name__ == "__main__":
    unittest.main()
