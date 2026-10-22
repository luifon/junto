"""The real-package check: its commands, starts, comparisons and choices."""
import contextlib
import os
import pathlib
import select
import shutil
import signal
import subprocess
import sys
import tempfile
import unittest

import check_lifecycle
from programs import program
from unittest import mock


# A program that starts another program, which keeps a pipe of the test open until it ends.
# With "end" the first program ends at once; with "wait" it waits.
STARTER = """
import os, subprocess, sys
child = [sys.executable, "-c", "import time; time.sleep(30)"]
if os.name == "nt":
    other = subprocess.Popen(child, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    open(os.environ["JUNTO_TEST_STARTED"], "w").write(str(other.pid))
else:
    lock = int(os.environ["JUNTO_LOCK_FD"])
    subprocess.Popen(child, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, pass_fds=(lock,))
print("started", flush=True)
if sys.argv[1] == "wait":
    import time
    time.sleep(30)
"""


@contextlib.contextmanager
def started_program():
    """The arguments and environment of STARTER, and a function that states whether the program
    that it started has ended, waiting at most 5 seconds for its end."""
    with tempfile.TemporaryDirectory() as temporary:
        script = pathlib.Path(temporary) / "starter.py"
        script.write_text(STARTER)
        arguments = [sys.executable, str(script)]
        if os.name == "nt":
            record = pathlib.Path(temporary) / "started"
            environment = dict(os.environ, JUNTO_TEST_STARTED=str(record))

            def ended():
                import ctypes
                kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
                kernel32.OpenProcess.restype = ctypes.c_void_p
                process = kernel32.OpenProcess(0x00100000, False, int(record.read_text()))  # SYNCHRONIZE
                if not process:
                    return True
                try:
                    return kernel32.WaitForSingleObject(ctypes.c_void_p(process), 5000) == 0
                finally:
                    kernel32.CloseHandle(ctypes.c_void_p(process))

            yield arguments, environment, ended
            return
        read, write = os.pipe()
        try:
            environment = dict(os.environ, JUNTO_LOCK_FD=str(write))

            def ended():
                os.close(write)
                return bool(select.select([read], [], [], 5)[0]) and os.read(read, 1) == b""

            yield arguments, environment, ended
        finally:
            os.close(read)
            try:
                os.close(write)
            except OSError:
                pass


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

    def test_timeout_ends_the_command_and_the_program_that_it_started(self):
        with started_program() as (arguments, environment, ended):
            with self.assertRaisesRegex(AssertionError, "timed out"):
                check_lifecycle.command(arguments + ["wait"], timeout=2, env=environment)
            self.assertTrue(ended(), "the started program is still running")

    def test_a_command_that_ends_leaves_no_program_that_it_started(self):
        with started_program() as (arguments, environment, ended):
            self.assertEqual(check_lifecycle.command(arguments + ["end"], env=environment).strip(), "started")
            self.assertTrue(ended(), "the started program is still running")

    @unittest.skipIf(os.name == "nt", "a program leaves the process group only on Unix")
    def test_a_timeout_keeps_the_output_when_a_program_outside_the_group_holds_the_pipes(self):
        program = ("import subprocess, sys; "
                   "other = subprocess.Popen([sys.executable, '-c', 'import signal; signal.alarm(10); signal.pause()'], "
                   "start_new_session=True); "
                   "print('output so far', other.pid, flush=True); "
                   "import time; time.sleep(30)")
        with mock.patch.object(check_lifecycle, "DRAIN_BOUND", 1):
            with self.assertRaises(AssertionError) as raised:
                check_lifecycle.command([sys.executable, "-c", program], timeout=2)
        message = str(raised.exception)
        self.assertIn("timed out", message)
        self.assertIn("output so far", message)
        other = int(message.split("output so far ", 1)[1].split()[0])
        try:
            os.kill(other, signal.SIGKILL)
        except ProcessLookupError:
            pass

    @unittest.skipIf(os.name == "nt", "a program leaves the process group only on Unix")
    def test_a_command_whose_output_a_program_outside_the_group_keeps_open_fails_in_bounded_time(self):
        program = ("import subprocess, sys; "
                   "other = subprocess.Popen([sys.executable, '-c', 'import signal; signal.alarm(10); signal.pause()'], "
                   "start_new_session=True); "
                   "print('output so far', other.pid, flush=True)")
        with mock.patch.object(check_lifecycle, "DRAIN_BOUND", 1):
            with self.assertRaises(AssertionError) as raised:
                check_lifecycle.command([sys.executable, "-c", program], timeout=20)
        message = str(raised.exception)
        self.assertIn("kept its output open", message)
        self.assertIn("output so far", message)
        try:
            os.kill(int(message.split("output so far ", 1)[1].split()[0]), signal.SIGKILL)
        except ProcessLookupError:
            pass

    def test_output_is_read_as_utf8_and_bytes_that_are_not_utf8_are_replaced(self):
        program = "import sys; sys.stdout.buffer.write('\u00e9 '.encode() + bytes([0x81, 0x8d, 0x8f, 0x90, 0x9d]))"
        output = check_lifecycle.command([sys.executable, "-c", program])
        self.assertEqual(output, "\u00e9 " + "\ufffd" * 5)

    def test_first_frame_without_successful_exit_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            script = pathlib.Path(temporary) / "client.py"
            script.write_text('import sys\nif "--version" in sys.argv:\n print("1.0.0 (test configuration)")\nelse:\n print("Junto first frame rendered")\n sys.exit(1)\n')
            with self.assertRaisesRegex(AssertionError, "code 1"):
                check_lifecycle.start([sys.executable, script], "1.0.0", pathlib.Path(temporary) / "data")


    def test_the_versions_are_the_workspace_version_and_the_next_patch(self):
        manifest = '[workspace]\nmembers = []\n[workspace.package]\nversion = "0.4.9"\n'
        self.assertEqual(check_lifecycle.versions(manifest), ["0.4.9", "0.4.10"])

    def test_an_update_asks_for_the_new_release_then_the_installed_one(self):
        self.assertEqual(check_lifecycle.requests("linux", "appimage", "x86_64", ["0.1.0", "0.1.1"]), [
            "/latest-linux-appimage-x86_64.json", "/junto-0.1.1-linux-appimage-x86_64.junto",
            "/release-0.1.0-linux-appimage-x86_64.json", "/junto-0.1.0-linux-appimage-x86_64.junto"])

    def test_the_appimage_uses_fuse_only_when_its_device_and_fusermount_can_be_used(self):
        with tempfile.TemporaryDirectory() as temporary, mock.patch("sys.stdout"):
            root = pathlib.Path(temporary)
            device = root / "fuse"
            with mock.patch.dict(os.environ, APPIMAGE="/a", APPDIR="/b", APPIMAGE_EXTRACT_AND_RUN="0"):
                missing = check_lifecycle.fuse_environment(root, device, lambda _: "/bin/fusermount")
            self.assertEqual(missing["APPIMAGE_EXTRACT_AND_RUN"], "1")
            self.assertEqual(missing["TMPDIR"], str(root))
            self.assertNotIn("APPIMAGE", missing)
            self.assertNotIn("APPDIR", missing)
            device.touch()
            self.assertEqual(check_lifecycle.fuse_environment(root, device, lambda _: None)
                             ["APPIMAGE_EXTRACT_AND_RUN"], "1")
            self.assertNotIn("APPIMAGE_EXTRACT_AND_RUN",
                             check_lifecycle.fuse_environment(root, device, lambda _: "/bin/fusermount3"))
            if os.name != "nt" and os.geteuid() != 0:
                device.chmod(0)
                self.assertEqual(check_lifecycle.fuse_environment(root, device, lambda _: "/bin/fusermount")
                                 ["APPIMAGE_EXTRACT_AND_RUN"], "1")

    def test_the_installation_must_equal_the_package_of_its_version(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            expected = root / "package/Junto"
            (expected / "licences").mkdir(parents=True)
            (expected / "junto").write_text("new program")
            (expected / "junto-package.json").write_text('{"version": "0.1.1"}')
            (expected / "licences/LICENSE.txt").write_text("licence")
            installed = root / "installed"
            installed.mkdir()
            application = installed / "Junto"
            shutil.copytree(expected, application)
            (installed / ".Junto.junto-update/record").mkdir(parents=True)
            check_lifecycle.compare(installed, application, expected)

            (application / "junto-package.json").write_text('{"version": "0.1.0"}')
            with self.assertRaisesRegex(AssertionError, "junto-package.json"):
                check_lifecycle.compare(installed, application, expected)
            shutil.copy2(expected / "junto-package.json", application / "junto-package.json")
            (application / "licences/LICENSE.txt").unlink()
            with self.assertRaisesRegex(AssertionError, "LICENSE.txt"):
                check_lifecycle.compare(installed, application, expected)
            shutil.copy2(expected / "licences/LICENSE.txt", application / "licences/LICENSE.txt")
            (installed / "stage-1").mkdir()
            with self.assertRaisesRegex(AssertionError, "stage-1"):
                check_lifecycle.compare(installed, application, expected)
            (installed / "stage-1").rmdir()
            if os.name != "nt":
                (application / "junto").chmod(0o755)
                with self.assertRaisesRegex(AssertionError, "junto"):
                    check_lifecycle.compare(installed, application, expected)

    @unittest.skipUnless(sys.platform == "darwin", "codesign is a macOS program")
    def test_a_bundle_with_a_changed_resource_has_no_valid_signature(self):
        with tempfile.TemporaryDirectory() as temporary:
            bundle = pathlib.Path(temporary) / "Test.app"
            (bundle / "Contents/MacOS").mkdir(parents=True)
            (bundle / "Contents/Resources").mkdir()
            (bundle / "Contents/Info.plist").write_text(
                '<?xml version="1.0" encoding="UTF-8"?>\n<plist version="1.0"><dict>'
                "<key>CFBundleExecutable</key><string>test</string>"
                "<key>CFBundleIdentifier</key><string>org.example.test</string></dict></plist>\n")
            (bundle / "Contents/Resources/text").write_text("first")
            program(bundle / "Contents/MacOS/test", 0)
            subprocess.run(["/usr/bin/codesign", "--force", "--sign", "-", str(bundle)], check=True,
                           capture_output=True, timeout=60)
            check_lifecycle.verify_signature(bundle)
            (bundle / "Contents/Resources/text").write_text("second")
            with self.assertRaisesRegex(AssertionError, "signature"):
                check_lifecycle.verify_signature(bundle)

    def test_a_folder_is_removed_after_failed_attempts_and_a_last_failure_names_what_is_left(self):
        with tempfile.TemporaryDirectory() as temporary:
            folder = pathlib.Path(temporary) / "check"
            (folder / "Junto").mkdir(parents=True)
            (folder / "Junto/junto.exe").write_text("program")
            remove = shutil.rmtree
            failures = [PermissionError(32, "in use")] * 2

            def held(path):
                if failures:
                    raise failures.pop()
                remove(path)

            with mock.patch.object(check_lifecycle.shutil, "rmtree", held):
                check_lifecycle.remove_tree(folder, attempts=3, interval=0)
            self.assertFalse(folder.exists())

            (folder / "Junto").mkdir(parents=True)
            (folder / "Junto/junto.exe").write_text("program")
            with mock.patch.object(check_lifecycle.shutil, "rmtree", side_effect=PermissionError(32, "in use")):
                with self.assertRaisesRegex(AssertionError, r"after 3 attempts(.|\n)*junto\.exe"):
                    check_lifecycle.remove_tree(folder, attempts=3, interval=0)


if __name__ == "__main__":
    unittest.main()
