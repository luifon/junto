"""Unit tests of tools/windows/lock.py. They run on every system; on Windows they test the lock
of Windows, elsewhere that of the system."""

import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import unittest

LOCK = pathlib.Path(__file__).resolve().with_name("lock.py")
SERIAL = pathlib.Path(__file__).resolve().parents[1] / "serial.sh"
# A command that states that it runs and then waits for a line on its standard input.
HOLD = [sys.executable, "-c", "import sys; print('held', flush=True); sys.stdin.readline()"]


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
