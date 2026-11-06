#!/usr/bin/env python3
"""Tests of tools/ci/each_crate.py with a program in place of cargo that records each command."""
import io
import json
import os
from pathlib import Path
import shutil
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))

import each_crate  # noqa: E402

METADATA = {"packages": [{"name": "first"}, {"name": "second"}, {"name": "third"}]}


@unittest.skipIf(os.name == "nt", "the program in place of cargo is a shell script")
class EachCrate(unittest.TestCase):
    def cargo(self, fail_on=""):
        directory = Path(tempfile.mkdtemp(prefix="junto-each-"))
        self.addCleanup(shutil.rmtree, directory, True)
        (directory / "metadata.json").write_text(json.dumps(METADATA))
        record = directory / "record"
        record.touch()
        program = directory / "cargo"
        program.write_text(f"""#!/bin/sh
if [ "$1" = metadata ]; then cat {directory / "metadata.json"}; exit 0; fi
echo "$*" >>{record}
case "$*" in *"-p {fail_on} "*) [ -z "{fail_on}" ] || exit 7 ;; esac
exit 0
""")
        program.chmod(0o755)
        return str(program), record

    def test_each_package_runs_alone_with_the_arguments_after_its_name(self):
        cargo, record = self.cargo()
        out = io.StringIO()
        status = each_crate.run(["clippy", "--locked", "--all-targets", "--", "-D", "warnings"], cargo, out)
        self.assertEqual(status, 0, out.getvalue())
        self.assertEqual(record.read_text().splitlines(), [
            f"clippy -p {name} --locked --all-targets -- -D warnings" for name in ("first", "second", "third")])
        self.assertNotIn("failed packages", out.getvalue())

    def test_a_failed_package_does_not_stop_the_others_and_is_named(self):
        cargo, record = self.cargo(fail_on="second")
        out = io.StringIO()
        status = each_crate.run(["clippy", "--locked"], cargo, out)
        self.assertEqual(status, 7)
        self.assertEqual(len(record.read_text().splitlines()), 3)
        self.assertIn("=== failed packages: second (exit 7)", out.getvalue())


if __name__ == "__main__":
    unittest.main()
