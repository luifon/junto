#!/usr/bin/env python3
"""Tests of running.py with a stand-in for lsappinfo."""
import subprocess
import unittest

import running


def lsappinfo(listed, described):
    calls = []

    def run(arguments, **_):
        calls.append(arguments)
        if arguments[1] == "find":
            return subprocess.CompletedProcess(arguments, 0, listed, "")
        return subprocess.CompletedProcess(arguments, 0, described[arguments[-1]], "")
    return run, calls


class Running(unittest.TestCase):
    def test_an_installed_junto_that_runs_ends_the_check_with_its_description(self):
        # The form of lsappinfo on macOS: the display name in quotes after the number.
        run, calls = lsappinfo('ASN:0x0-0x1d01d-"Junto":\n', {
            'ASN:0x0-0x1d01d-"Junto":': '"LSDisplayName"="Junto"\n"pid"=4242\n"LSBundlePath"="/Applications/Junto.app"\n'})
        with self.assertRaises(SystemExit) as ended:
            running.refuse_when_running(run=run, system=lambda: "Darwin")
        message = str(ended.exception)
        self.assertIn("bundle identifier org.junto.app runs", message)
        self.assertIn('"pid"=4242', message)
        self.assertIn("/Applications/Junto.app", message)
        self.assertIn("Quit Junto", message)
        self.assertEqual(calls[0], [running.LSAPPINFO, "find", "bundleid=org.junto.app"])

    def test_a_display_name_with_spaces_is_one_application(self):
        listed = 'ASN:0x0-0x1d01d-"Junto Test":\nASN:0x0-0x2e02e-"Junto":\n'
        run, calls = lsappinfo(listed, {
            'ASN:0x0-0x1d01d-"Junto Test":': '"pid"=1\n', 'ASN:0x0-0x2e02e-"Junto":': '"pid"=2\n'})
        self.assertEqual(running.running(run=run, system=lambda: "Darwin"), ['"pid"=1', '"pid"=2'])
        self.assertEqual([call[-1] for call in calls[1:]], ['ASN:0x0-0x1d01d-"Junto Test":', 'ASN:0x0-0x2e02e-"Junto":'])

    def test_no_application_of_the_identifier_lets_the_check_run(self):
        run, calls = lsappinfo("", {})
        running.refuse_when_running(run=run, system=lambda: "Darwin")
        self.assertEqual(len(calls), 1)

    def test_other_systems_do_not_ask(self):
        run, calls = lsappinfo("ASN:0x0-0x1:", {})
        self.assertEqual(running.running(run=run, system=lambda: "Linux"), [])
        self.assertEqual(calls, [])

    def test_a_failure_of_lsappinfo_is_not_taken_for_no_application(self):
        def run(arguments, **_):
            return subprocess.CompletedProcess(arguments, 1, "", "no session")
        with self.assertRaises(RuntimeError):
            running.running(run=run, system=lambda: "Darwin")
        # The check states it and runs, as it did before it asked.
        running.refuse_when_running(run=run, system=lambda: "Darwin")

    def test_the_checks_that_start_a_bundle_refuse_first(self):
        for name in ["check_update.py", "check_start.py", "check_lifecycle.py"]:
            with self.subTest(check=name):
                text = (running.__file__.rsplit("/", 1)[0] + "/" + name)
                with open(text, encoding="utf-8") as file:
                    self.assertIn("refuse_when_running(", file.read())


if __name__ == "__main__":
    unittest.main()
