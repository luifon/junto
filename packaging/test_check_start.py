"""The start check of a bundle, with an executable that states what it received."""
import json
import os
import pathlib
import sys
import tempfile
import unittest

import check_start

PROGRAM = """#!{python}
import json, os, sys
data = sys.argv[sys.argv.index("--data-dir") + 1]
with open({record!r}, "w") as file:
    json.dump({{"arguments": sys.argv[1:], "data": os.listdir(data)}}, file)
print({line!r})
sys.exit({status})
"""


@unittest.skipIf(os.name == "nt", "the bundle executable of the test is a script")
class StartCheckTests(unittest.TestCase):
    def bundle(self, root, line, status):
        executable = root / "Junto.app/Contents/MacOS/junto"
        executable.parent.mkdir(parents=True)
        executable.write_text(PROGRAM.format(python=sys.executable, record=str(root / "record.json"),
                                             line=line, status=status))
        executable.chmod(0o755)
        return root / "Junto.app"

    def test_the_first_frame_line_and_exit_code_0_pass_with_an_empty_data_directory(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            check_start.check(self.bundle(root, "Junto first frame rendered", 0))
            record = json.loads((root / "record.json").read_text())
            self.assertEqual(record["data"], [])
            arguments = record["arguments"]
            self.assertEqual(arguments[0], "--data-dir")
            self.assertEqual(arguments[2:], ["--no-activate", "--check-startup"])

    def test_a_start_without_the_first_frame_line_fails(self):
        with tempfile.TemporaryDirectory() as temporary:
            with self.assertRaisesRegex(SystemExit, "did not confirm its first frame"):
                check_start.check(self.bundle(pathlib.Path(temporary), "Junto started", 0))

    def test_a_start_that_ends_with_another_code_fails(self):
        with tempfile.TemporaryDirectory() as temporary:
            with self.assertRaisesRegex(SystemExit, "code 1"):
                check_start.check(self.bundle(pathlib.Path(temporary), "Junto first frame rendered", 1))


if __name__ == "__main__":
    unittest.main()
