"""Unit tests of tools/windows/lock.py. They run on every system; on Windows they test the lock
of Windows, elsewhere that of the system."""

import os
import pathlib
import shutil
import signal
import subprocess
import sys
import queue
import tempfile
import threading
import unittest

LOCK = pathlib.Path(__file__).resolve().with_name("lock.py")
SERIAL = pathlib.Path(__file__).resolve().parents[1] / "serial.sh"
# A command that states that it runs and then waits for a line on its standard input.
HOLD = [sys.executable, "-c", "import sys; print('held', flush=True); sys.stdin.readline()"]


# A command that starts a child, states the number of that child's process, and waits for the
# child, which waits for a line on the standard input that both share.
TREE = [sys.executable, "-c", "import subprocess, sys; child = subprocess.Popen([sys.executable, '-c', "
        "'import sys; sys.stdin.readline()']); print(child.pid, flush=True); child.wait()"]
# A command that states whether the process of the number in its argument still runs.
ALIVE = """
import os, sys
number = int(sys.argv[1])
if os.name == "nt":
    import ctypes
    kernel32 = ctypes.WinDLL("kernel32")
    kernel32.OpenProcess.restype = ctypes.c_void_p
    handle = kernel32.OpenProcess(0x00100000, False, number)  # SYNCHRONIZE
    running = bool(handle) and kernel32.WaitForSingleObject(ctypes.c_void_p(handle), 0) == 0x102
else:
    try:
        os.kill(number, 0)
        running = True
    except ProcessLookupError:
        running = False
print("running" if running else "ended")
"""


def debugged(number):
    """Attaches this thread as the debugger of a process: Windows then does not finish the end
    of that process until the debugger detaches, which stands for a process that takes long to
    end. Returns the function that detaches."""
    import ctypes

    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    if not kernel32.DebugActiveProcess(number):
        raise ctypes.WinError(ctypes.get_last_error())
    return lambda: kernel32.DebugActiveProcessStop(number)


def locked(lock, command, **options):
    return subprocess.Popen([sys.executable, str(LOCK), str(lock), *command], text=True, **options)


class Lock(unittest.TestCase):
    def test_a_second_command_waits_until_the_first_has_ended(self):
        with tempfile.TemporaryDirectory(prefix="junto-lock-") as temporary:
            lock = pathlib.Path(temporary) / "run.lock"
            first = locked(lock, HOLD, stdin=subprocess.PIPE, stdout=subprocess.PIPE)
            try:
                self.assertEqual(first.stdout.readline().strip(), "held")
                second = locked(lock, [sys.executable, "-c", "print('second')"], stdout=subprocess.PIPE)
                try:
                    # The second command cannot end while the first holds the lock.
                    with self.assertRaises(subprocess.TimeoutExpired):
                        second.wait(timeout=3)
                    first.stdin.write("end\n")
                    first.stdin.flush()
                    self.assertEqual(first.wait(timeout=60), 0)
                    output, _ = second.communicate(timeout=60)
                    self.assertEqual(output.strip(), "second")
                    self.assertEqual(second.returncode, 0)
                finally:
                    second.kill()
                    second.communicate()
            finally:
                first.kill()
                first.communicate()

    def test_the_exit_code_is_that_of_the_command(self):
        with tempfile.TemporaryDirectory(prefix="junto-lock-") as temporary:
            lock = pathlib.Path(temporary) / "run.lock"
            command = locked(lock, [sys.executable, "-c", "raise SystemExit(7)"])
            self.assertEqual(command.wait(timeout=60), 7)

    # A run that is ended from outside ends its jobs, or keeps the lock until they have ended,
    # so that the next run cannot run beside them.
    def test_the_lock_is_free_only_when_every_process_of_an_ended_holder_has_ended(self):
        with tempfile.TemporaryDirectory(prefix="junto-lock-") as temporary:
            lock = pathlib.Path(temporary) / "run.lock"
            first = locked(lock, TREE, stdin=subprocess.PIPE, stdout=subprocess.PIPE)
            try:
                child = int(first.stdout.readline())
                # TerminateProcess on Windows, SIGKILL elsewhere: the holder cannot clean up.
                first.kill()
                first.wait(timeout=60)
                second = locked(lock, [sys.executable, "-c", ALIVE, str(child)], stdout=subprocess.PIPE)
                try:
                    if os.name != "nt":
                        # The child still holds the locked file.
                        with self.assertRaises(subprocess.TimeoutExpired):
                            second.wait(timeout=3)
                        first.stdin.write("end\n")
                        first.stdin.flush()
                    output, _ = second.communicate(timeout=60)
                    self.assertEqual(output.strip(), "ended")
                finally:
                    second.kill()
                    second.communicate()
            finally:
                first.kill()
                first.stdin.close()
                first.stdout.close()

    # The processes of the job end after the holder: the lock must stay held until the last
    # of them has ended, also when it takes time to end.
    @unittest.skipIf(os.name != "nt", "the debugger that delays the end of a process is that of Windows")
    def test_the_lock_is_free_only_when_a_slowly_ending_process_of_an_ended_holder_has_ended(self):
        with tempfile.TemporaryDirectory(prefix="junto-lock-") as temporary:
            lock = pathlib.Path(temporary) / "run.lock"
            first = locked(lock, TREE, stdin=subprocess.PIPE, stdout=subprocess.PIPE)
            try:
                child = int(first.stdout.readline())
                detach = debugged(child)
                try:
                    first.kill()
                    first.wait(timeout=60)
                    second = locked(lock, [sys.executable, "-c", ALIVE, str(child)], stdout=subprocess.PIPE)
                    try:
                        with self.assertRaises(subprocess.TimeoutExpired):
                            second.wait(timeout=5)
                    finally:
                        detach()
                    output, _ = second.communicate(timeout=60)
                    self.assertEqual(output.strip(), "ended")
                finally:
                    detach()
            finally:
                first.kill()
                first.stdin.close()
                first.stdout.close()

    # A process that does not end keeps the lock, and the holder states it.
    @unittest.skipIf(os.name != "nt", "the job object is that of Windows")
    def test_a_process_that_does_not_end_is_reported_while_the_lock_stays_held(self):
        import importlib.util

        specification = importlib.util.spec_from_file_location("lock", LOCK)
        module = importlib.util.module_from_spec(specification)
        specification.loader.exec_module(module)
        job = module.Job()
        process = job.start([sys.executable, "-c", "import time; time.sleep(600)"])
        detach = debugged(process.pid)
        messages = queue.Queue()

        class Output:
            def write(self, text):
                messages.put(text)

            def flush(self):
                pass

        error, sys.stderr = sys.stderr, Output()
        ending = threading.Thread(target=job.end, kwargs={"report_after": 1})
        try:
            ending.start()
            message = messages.get(timeout=60)
            self.assertIn(f"1 processes of the job have not ended 1 s after they were ended ({process.pid})", message)
            self.assertTrue(ending.is_alive())
        finally:
            detach()
            ending.join(timeout=60)
            sys.stderr = error
            process.wait(timeout=60)
        self.assertFalse(ending.is_alive())

    # When the command ends and leaves a process behind, the lock is released only after that
    # process has ended (on Windows lock.py ends it).
    @unittest.skipIf(os.name != "nt", "on other systems the left process keeps the lock until it ends")
    def test_a_process_that_the_command_leaves_is_ended_before_the_lock_is_free(self):
        with tempfile.TemporaryDirectory(prefix="junto-lock-") as temporary:
            lock = pathlib.Path(temporary) / "run.lock"
            leaving = [sys.executable, "-c", "import subprocess, sys; child = subprocess.Popen([sys.executable, '-c', "
                       "'import time; time.sleep(600)']); print(child.pid, flush=True)"]
            first = locked(lock, leaving, stdout=subprocess.PIPE)
            child = int(first.stdout.readline())
            self.assertEqual(first.wait(timeout=60), 0)
            first.stdout.close()
            second = locked(lock, [sys.executable, "-c", ALIVE, str(child)], stdout=subprocess.PIPE)
            output, _ = second.communicate(timeout=60)
            self.assertEqual(output.strip(), "ended")

    @unittest.skipIf(os.name == "nt", "Windows has no signals between processes")
    def test_a_signal_to_the_holder_goes_to_the_command(self):
        with tempfile.TemporaryDirectory(prefix="junto-lock-") as temporary:
            lock = pathlib.Path(temporary) / "run.lock"
            first = locked(lock, HOLD, stdin=subprocess.PIPE, stdout=subprocess.PIPE)
            try:
                self.assertEqual(first.stdout.readline().strip(), "held")
                first.send_signal(signal.SIGTERM)
                # The command ends on TERM, and the holder returns its status as a shell does.
                self.assertEqual(first.wait(timeout=60), 128 + signal.SIGTERM)
            finally:
                first.kill()
                first.communicate()

    # tools/ci/run-jobs.sh ends a run that waits for the lock with TERM to lock.py.
    @unittest.skipIf(os.name == "nt", "Windows has no signals between processes")
    def test_a_signal_while_the_lock_is_awaited_ends_the_holder_without_the_command(self):
        with tempfile.TemporaryDirectory(prefix="junto-lock-") as temporary:
            lock = pathlib.Path(temporary) / "run.lock"
            ran = pathlib.Path(temporary) / "ran"
            first = locked(lock, HOLD, stdin=subprocess.PIPE, stdout=subprocess.PIPE)
            try:
                self.assertEqual(first.stdout.readline().strip(), "held")
                second = locked(lock, [sys.executable, "-c", f"open({str(ran)!r}, 'w')"])
                with self.assertRaises(subprocess.TimeoutExpired):
                    second.wait(timeout=1)
                second.send_signal(signal.SIGTERM)
                self.assertEqual(second.wait(timeout=60), 128 + signal.SIGTERM)
                first.stdin.write("end\n")
                first.stdin.flush()
                self.assertEqual(first.wait(timeout=60), 0)
                self.assertFalse(ran.exists())
            finally:
                first.kill()
                first.communicate()

    def test_a_command_that_cannot_start_is_a_failure(self):
        with tempfile.TemporaryDirectory(prefix="junto-lock-") as temporary:
            lock = pathlib.Path(temporary) / "run.lock"
            command = locked(lock, [str(pathlib.Path(temporary) / "absent-program")], stderr=subprocess.PIPE)
            _, error = command.communicate(timeout=60)
            self.assertEqual(command.returncode, 127)
            self.assertIn("absent-program", error)


# The Bash of Git for Windows has neither lockf nor flock; links in a folder stand for its
# programs, since a test cannot remove lockf from macOS.
@unittest.skipIf(os.name == "nt", "the folder of links needs a POSIX system")
class Serial(unittest.TestCase):
    def test_serial_takes_the_lock_with_python_without_lockf_and_flock(self):
        with tempfile.TemporaryDirectory(prefix="junto-serial-") as temporary:
            root = pathlib.Path(temporary)
            programs = root / "bin"
            programs.mkdir()
            for name in ["sh", "dirname", "nice"]:
                (programs / name).symlink_to(shutil.which(name))
            (programs / "python3").symlink_to(sys.executable)
            lock = root / "run.lock"
            environment = dict(os.environ, PATH=str(programs), JUNTO_HEAVY_LOCK=str(lock))
            first = subprocess.Popen(["sh", str(SERIAL), *HOLD], env=environment, text=True,
                                     stdin=subprocess.PIPE, stdout=subprocess.PIPE)
            try:
                self.assertEqual(first.stdout.readline().strip(), "held")
                # The lock of the running command is the lock file that serial.sh was given.
                second = locked(lock, [sys.executable, "-c", "print('second')"], stdout=subprocess.PIPE)
                try:
                    with self.assertRaises(subprocess.TimeoutExpired):
                        second.wait(timeout=3)
                    first.stdin.write("end\n")
                    first.stdin.flush()
                    self.assertEqual(first.wait(timeout=60), 0)
                    self.assertEqual(second.communicate(timeout=60)[0].strip(), "second")
                finally:
                    second.kill()
                    second.communicate()
            finally:
                first.kill()
                first.communicate()


if __name__ == "__main__":
    unittest.main()
