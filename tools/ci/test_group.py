#!/usr/bin/env python3
"""Tests of tools/ci/group.py on Unix: it returns only after every process of the group of its
command has ended."""
import fcntl
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest

GROUP = Path(__file__).with_name("group.py")
LOCK = Path(__file__).resolve().parents[1] / "windows" / "lock.py"
# Starts group.py with a time before KILL of `seconds`.
WITH_KILL_SECONDS = """
import importlib.util, sys
specification = importlib.util.spec_from_file_location("group", sys.argv[1])
group = importlib.util.module_from_spec(specification)
specification.loader.exec_module(group)
group.KILL_SECONDS = float(sys.argv[2])
sys.exit(group.main(sys.argv[3:]))
"""


def running(number):
    try:
        os.kill(number, 0)
    except ProcessLookupError:
        return False
    return True


def appeared(path, seconds=60):
    """Waits until a file exists."""
    deadline = time.monotonic() + seconds
    while not path.exists():
        if time.monotonic() > deadline:
            raise AssertionError(f"{path} did not appear")
        time.sleep(0.05)


@unittest.skipIf(os.name == "nt", "the Unix process group")
class Group(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="junto-ci-group-")
        self.root = Path(self.temporary.name)

    def tearDown(self):
        self.temporary.cleanup()

    def test_the_exit_code_is_that_of_the_command(self):
        result = subprocess.run([sys.executable, str(GROUP), "sample", "sh", "-c", "exit 3"], timeout=60)
        self.assertEqual(result.returncode, 3)

    def test_a_process_that_the_command_leaves_is_ended_before_group_returns(self):
        left = self.root / "left"
        result = subprocess.run([sys.executable, str(GROUP), "sample", "sh", "-c",
                                 f"sleep 600 & echo $! > {left}; exit 0"], timeout=60)
        self.assertEqual(result.returncode, 0)
        self.assertFalse(running(int(left.read_text())))

    # A process that takes a second to end after TERM: group.py returns after it.
    def test_a_signal_ends_every_process_of_the_group_and_is_awaited(self):
        started, ended = self.root / "started", self.root / "ended"
        child = (f"trap 'sleep 1; echo > {ended}; exit 0' TERM; echo > {started}; "
                 "while :; do sleep 0.1; done")
        process = subprocess.Popen([sys.executable, str(GROUP), "sample", "sh", "-c", f"sh -c \"{child}\" & wait"])
        appeared(started)
        process.send_signal(signal.SIGTERM)
        self.assertEqual(process.wait(timeout=60), 128 + signal.SIGTERM)
        self.assertTrue(ended.exists(), "group.py returned before the process of the group had ended")

    def test_a_process_that_ignores_term_receives_kill(self):
        started, pid = self.root / "started", self.root / "pid"
        child = f"trap '' TERM; echo $$ > {pid}; echo > {started}; while :; do sleep 0.1; done"
        process = subprocess.Popen([sys.executable, "-c", WITH_KILL_SECONDS, str(GROUP), "1", "sample",
                                    "sh", "-c", f"sh -c \"{child}\" & wait"], stderr=subprocess.PIPE, text=True)
        appeared(started)
        process.send_signal(signal.SIGTERM)
        _, error = process.communicate(timeout=60)
        self.assertEqual(process.returncode, 128 + signal.SIGTERM)
        self.assertIn("remain 1.0 s after TERM; KILL", error)
        self.assertFalse(running(int(pid.read_text())))

    # The OOM killer or "pkill -9 -f run-jobs.sh" ends lock.py, the locked shell and group.py,
    # but not the processes of the job, which must keep the run lock while they write.
    def test_the_processes_of_the_command_hold_the_lock_of_lock_py(self):
        lock, started = self.root / "run.lock", self.root / "started"
        holder = subprocess.Popen([sys.executable, str(LOCK), str(lock), sys.executable, str(GROUP), "sample",
                                   "sh", "-c", f"echo $$ > {self.root / 'pid'}; echo > {started}; exec sleep 60"])
        appeared(started)
        job = int((self.root / "pid").read_text())
        self.addCleanup(lambda: running(job) and os.kill(job, signal.SIGKILL))
        group = [int(number) for number in subprocess.run(["pgrep", "-P", str(holder.pid)], capture_output=True,
                                                          text=True).stdout.split()]
        for number in [holder.pid, *group]:
            os.kill(number, signal.SIGKILL)
        holder.wait(timeout=60)
        self.assertTrue(running(job))
        with open(lock, "a") as file:
            with self.assertRaises(BlockingIOError):
                fcntl.flock(file.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        os.kill(job, signal.SIGKILL)


if __name__ == "__main__":
    unittest.main()
