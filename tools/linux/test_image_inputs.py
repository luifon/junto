#!/usr/bin/env python3
"""Tests of tools/linux/image_inputs.py, the key of the inputs of the Linux build image, and of
the processor lists of tools/linux/cpu_sets.py."""
import os
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import cpu_sets  # noqa: E402
import image_inputs  # noqa: E402


class ImageInputs(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="junto-image-inputs-")
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        (self.root / "Dockerfile").write_text("FROM example\n")
        (self.root / "session.sh").write_text("#!/bin/sh\n")
        (self.root / "sub").mkdir()
        (self.root / "sub" / "build.sh").write_text("echo\n")

    def key(self, *arguments):
        return image_inputs.key(self.root, list(arguments) or ["BUILD_UID=1000"])

    def test_the_same_inputs_give_the_same_key(self):
        self.assertEqual(self.key(), self.key())
        (self.root / "__pycache__").mkdir()
        (self.root / "__pycache__" / "x.pyc").write_bytes(b"\0")
        self.assertEqual(len(self.key()), 32)

    def test_a_change_of_a_file_a_mode_a_name_or_an_argument_changes_the_key(self):
        first = self.key()
        changes = [
            lambda: (self.root / "sub" / "build.sh").write_text("echo changed\n"),
            lambda: os.chmod(self.root / "session.sh", 0o755),
            lambda: (self.root / "sub" / "build.sh").rename(self.root / "sub" / "built.sh"),
        ]
        keys = {first}
        for change in changes:
            change()
            keys.add(self.key())
        keys.add(self.key("BUILD_UID=1001"))
        keys.add(self.key("BUILD_UID=1000", "JUNTO_LINUX_IMAGE=junto-ci"))
        self.assertEqual(len(keys), 6)

    def test_a_context_without_a_dockerfile_or_an_argument_without_a_value_is_refused(self):
        with self.assertRaises(ValueError):
            self.key("BUILD_UID")
        (self.root / "Dockerfile").unlink()
        with self.assertRaises(ValueError):
            self.key()


class ProcessorLists(unittest.TestCase):
    def test_lists_are_read_and_written_as_ranges(self):
        self.assertEqual(cpu_sets.explicit("16", "8-11,0-1,4", "12-15"), (7, "0-1,4,8-11", "12-15"))

    def test_a_list_past_the_host_or_not_a_list_is_refused(self):
        for build in ["8-16", "", "a-b", "3-1", "1,,2", "-1"]:
            with self.subTest(build=build), self.assertRaises(ValueError):
                cpu_sets.explicit("16", build, "12-15")


if __name__ == "__main__":
    unittest.main()
