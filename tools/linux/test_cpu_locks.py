#!/usr/bin/env python3
"""Check the CPU allocation passed through each Linux worker lock."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SERIAL = Path(__file__).resolve().parents[1] / "serial.sh"


class WorkerCpuSets(unittest.TestCase):
    def allocation(self, lock, first=None, processors="4", system="Linux"):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name, body in {"uname": f"echo {system}", "flock": 'shift; exec "$@"',
                               "lockf": 'shift; shift; exec "$@"'}.items():
                program = root / name
                program.write_text(f"#!/bin/sh\n{body}\n")
                program.chmod(0o755)
            environment = dict(os.environ, PATH=f"{root}:{os.environ['PATH']}",
                               JUNTO_HEAVY_LOCK=lock, JUNTO_LINUX_CPUS=processors)
            environment.pop("JUNTO_LINUX_CPU_FIRST", None)
            if first is not None:
                environment["JUNTO_LINUX_CPU_FIRST"] = first
            result = subprocess.run([SERIAL, "sh", "-c", 'printf "%s %s" "${JUNTO_LINUX_CPU_FIRST-unset}" "$JUNTO_LINUX_CPUS"'],
                                    env=environment, capture_output=True, text=True, check=True)
            return result.stdout

    def test_four_worker_locks_have_disjoint_pairs(self):
        for lock, first in [("junto-heavy.lock", 0), ("junto-heavy-2.lock", 2),
                            ("junto-heavy-3.lock", 4), ("junto-heavy-4.lock", 6)]:
            with self.subTest(lock=lock):
                self.assertEqual(self.allocation(f"/tmp/{lock}"), f"{first} 2")

    def test_explicit_allocation_and_smaller_count_are_preserved(self):
        self.assertEqual(self.allocation("/tmp/junto-heavy.lock", first="8"), "8 4")
        self.assertEqual(self.allocation("/tmp/junto-heavy-2.lock", processors="1"), "2 1")

    def test_other_locks_and_systems_keep_the_callers_budget(self):
        self.assertEqual(self.allocation("/tmp/junto-run.lock"), "unset 4")
        self.assertEqual(self.allocation("/tmp/junto-heavy.lock", system="Darwin"), "unset 4")


if __name__ == "__main__":
    unittest.main()
