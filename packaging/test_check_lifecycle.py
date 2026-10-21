"""The real-package start check rejects stale binaries and incomplete starts."""
import os
import pathlib
import select
import signal
import subprocess
import sys
import tempfile
import unittest

import check_lifecycle


class LifecycleTests(unittest.TestCase):
    @unittest.skipIf(os.name == "nt", "the Unix CI lock is an inherited file descriptor")
    def test_commands_inherit_the_ci_lock(self):
        with tempfile.TemporaryFile() as lock:
            output = check_lifecycle.command(
                [sys.executable, "-c", "import os; os.fstat(int(os.environ['JUNTO_LOCK_FD'])); print('held')"],
                env=dict(os.environ, JUNTO_LOCK_FD=str(lock.fileno())))
            self.assertEqual(output.strip(), "held")

    @unittest.skipIf(os.name == "nt", "Unix signals and inherited pipe descriptors")
    def test_termination_stops_the_command_and_closes_its_inherited_lock(self):
        read, write = os.pipe()
        child = None
        runner = None
        try:
            program = ("import os, signal; "
                       "os.write(int(os.environ['JUNTO_LOCK_FD']), (str(os.getpid()) + '\\n').encode()); "
                       "signal.alarm(10); signal.pause()")
            script = (f"import sys; sys.path.insert(0, {str(pathlib.Path(__file__).parent)!r}); "
                      f"import check_lifecycle; check_lifecycle.command([sys.executable, '-c', {program!r}])")
            runner = subprocess.Popen([sys.executable, "-c", script], pass_fds=(write,),
                                      env=dict(os.environ, JUNTO_LOCK_FD=str(write)),
                                      stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            os.close(write)
            write = None
            self.assertTrue(select.select([read], [], [], 5)[0], "the child did not signal readiness")
            child = int(os.read(read, 128))
            runner.terminate()
            self.assertEqual(runner.wait(timeout=5), 128 + signal.SIGTERM)
            self.assertTrue(select.select([read], [], [], 5)[0], "the child retained the CI lock after TERM")
            self.assertEqual(os.read(read, 128), b"")
            child = None
        finally:
            if child is not None:
                try:
                    os.kill(child, signal.SIGKILL)
                except ProcessLookupError:
                    pass
            if runner is not None:
                if runner.poll() is None:
                    runner.kill()
                runner.communicate(timeout=5)
            os.close(read)
            if write is not None:
                os.close(write)

    def test_start_requires_a_first_frame_and_an_empty_private_profile(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            script = root / "client.py"
            script.write_text('import os, pathlib, sys\nif "--version" in sys.argv:\n print("1.0.0 (test configuration)")\nelse:\n data = pathlib.Path(sys.argv[sys.argv.index("--data-dir") + 1])\n data.mkdir(exist_ok=True)\n assert not list(data.iterdir())\n assert os.name == "nt" or data.stat().st_mode & 0o777 == 0o700\n assert "--no-activate" in sys.argv\n assert "--check-startup" in sys.argv\n print("Junto first frame rendered")\n')
            check_lifecycle.start([sys.executable, script], "1.0.0", root / "data")
            script.write_text(script.read_text().replace("Junto first frame rendered", "Junto started"))
            with self.assertRaisesRegex(AssertionError, "first frame"):
                check_lifecycle.start([sys.executable, script], "1.0.0", root / "other-data")

    def test_wrong_binary_version_is_rejected_before_start(self):
        with tempfile.TemporaryDirectory() as temporary:
            with self.assertRaisesRegex(AssertionError, "version"):
                check_lifecycle.start([sys.executable], "99.0.0", pathlib.Path(temporary) / "data")

    def test_nonempty_profile_is_refused(self):
        with tempfile.TemporaryDirectory() as temporary:
            data = pathlib.Path(temporary)
            (data / "existing").touch()
            with self.assertRaisesRegex(AssertionError, "empty"):
                check_lifecycle.start([sys.executable], "1.0.0", data)

    def test_timeout_ends_the_child(self):
        with self.assertRaisesRegex(AssertionError, "timed out"):
            check_lifecycle.command([sys.executable, "-c", "import time; time.sleep(30)"], timeout=0.1)

    def test_first_frame_without_successful_exit_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            script = pathlib.Path(temporary) / "client.py"
            script.write_text('import sys\nif "--version" in sys.argv:\n print("1.0.0 (test configuration)")\nelse:\n print("Junto first frame rendered")\n sys.exit(1)\n')
            with self.assertRaisesRegex(AssertionError, "code 1"):
                check_lifecycle.start([sys.executable, script], "1.0.0", pathlib.Path(temporary) / "data")


if __name__ == "__main__":
    unittest.main()
