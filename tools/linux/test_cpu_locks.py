#!/usr/bin/env python3
"""Check the CPU allocation passed through each Linux worker lock."""
import os
import select
import shutil
from pathlib import Path
import subprocess
import tempfile
import time
import unittest

SERIAL = Path(__file__).resolve().parents[1] / "serial.sh"


class WorkerCpuSets(unittest.TestCase):
    def allocation(self, lock, first=None, processors="4", system="Linux"):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name, body in {"uname": f"echo {system}", "getconf": "echo 16", "flock": 'shift; exec "$@"',
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


def tree(number):
    """The process `number` and every process below it."""
    children = subprocess.run(["pgrep", "-P", str(number)], capture_output=True, text=True).stdout.split()
    return [number, *[found for child in children for found in tree(int(child))]]


def waits_for_a_lock(process):
    """Waits until a process of the tree of `process` waits for a flock lock, which the kernel
    lists with "->" in /proc/locks. Fails if the process ends first."""
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        if process.poll() is not None:
            return False
        numbers = {str(number) for number in tree(process.pid)}
        for line in Path("/proc/locks").read_text().splitlines():
            fields = line.split()
            if fields[1:3] == ["->", "FLOCK"] and fields[5] in numbers:
                return True
        time.sleep(0.05)
    raise AssertionError("the worker neither waited for a lock nor ended")


@unittest.skipUnless(Path("/proc/locks").exists(), "the waiting locks are read from /proc/locks")
class SmallHostGate(unittest.TestCase):
    def test_ci_and_worker_locks_share_the_machine_gate(self):
        for processors in (8, 12):
            with self.subTest(processors=processors), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                serial = root / "serial.sh"
                gate = root / "machine.lock"
                serial.write_text(SERIAL.read_text().replace("/tmp/junto-heavy-machine.lock", str(gate)))
                serial.chmod(0o644)
                (root / "windows").mkdir()
                shutil.copy(SERIAL.parent / "windows/lock.py", root / "windows/lock.py")
                getconf = root / "getconf"
                getconf.write_text(f"#!/bin/sh\necho {processors}\n")
                getconf.chmod(0o755)
                environment = dict(os.environ, PATH=f"{root}:{os.environ['PATH']}",
                                   JUNTO_HEAVY_LOCK=str(root / "run.lock"))
                environment.pop("JUNTO_MACHINE_GATE_HELD", None)
                command = ["sh", serial, "--inherited", "env", f"JUNTO_HEAVY_LOCK={root / 'ci-heavy.lock'}",
                           "sh", serial, "sh", "-c", "echo ready; read release"]
                ci = subprocess.Popen(command, env=environment, stdin=subprocess.PIPE,
                                      stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
                worker = None
                try:
                    self.assertTrue(select.select([ci.stdout], [], [], 10)[0], "CI did not enter")
                    self.assertEqual(ci.stdout.readline(), "ready\n")
                    locked = subprocess.run(["flock", "-n", gate, "true"], check=False)
                    self.assertEqual(locked.returncode, 1, "CI did not hold the shared machine gate")
                    worker = subprocess.Popen(["sh", serial, "echo", "worker"],
                        env=dict(environment, JUNTO_HEAVY_LOCK=str(root / "worker.lock")),
                        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
                    # Only the gate is held by another process, so a waiting worker waits for the gate.
                    self.assertTrue(waits_for_a_lock(worker), "worker ran while CI held the machine gate")
                    self.assertFalse(select.select([worker.stdout], [], [], 0)[0], "worker overlapped CI")
                    ci.communicate("release\n", timeout=10)
                    self.assertEqual(worker.communicate(timeout=10)[0], "worker\n")
                    self.assertEqual(worker.returncode, 0)
                finally:
                    if ci.poll() is None:
                        ci.communicate("release\n", timeout=10)
                    if worker is not None and worker.poll() is None:
                        worker.communicate(timeout=10)


if __name__ == "__main__":
    unittest.main()
