"""Unit tests of tools/with_native_postgres.py. They run on every system and start no
PostgreSQL: the CI jobs on Windows and macOS start the real one."""

import contextlib
import hashlib
import io
import os
import pathlib
import re
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import time
import unittest
import zipfile
from unittest import mock

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import with_native_postgres as postgres  # noqa: E402

REPOSITORY = pathlib.Path(__file__).resolve().parents[1]


def archive_bytes(members):
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as archive:
        for name, data in members.items():
            archive.writestr(name, data)
    return buffer.getvalue()


def end_process(number):
    """Ends the process `number`; on Windows also waits until it has ended and its files are
    closed."""
    if os.name == "nt":
        import ctypes
        from ctypes import wintypes

        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
        kernel.OpenProcess.restype = wintypes.HANDLE
        kernel.TerminateProcess.argtypes = [wintypes.HANDLE, wintypes.UINT]
        kernel.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
        kernel.CloseHandle.argtypes = [wintypes.HANDLE]
        synchronize, terminate, infinite = 0x00100000, 0x0001, 0xFFFFFFFF
        handle = kernel.OpenProcess(synchronize | terminate, False, number)
        if not handle:
            return  # The process has ended.
        try:
            kernel.TerminateProcess(handle, 1)
            kernel.WaitForSingleObject(handle, infinite)
        finally:
            kernel.CloseHandle(handle)
    else:
        try:
            os.kill(number, signal.SIGKILL)
        except ProcessLookupError:
            pass


class Pin(unittest.TestCase):
    def setUp(self):
        self.pins = postgres.read_pins((REPOSITORY / "tools/ci/pins.sh").read_text())

    def test_the_version_is_the_version_of_the_ci_image(self):
        text = (REPOSITORY / "tools/ci/pins.sh").read_text()
        image = re.search(r"JUNTO_TEST_POSTGRES_IMAGE:=postgres:([0-9.]+)-", text)
        self.assertIsNotNone(image, "tools/ci/pins.sh names no PostgreSQL image")
        for system in postgres.SYSTEMS:
            version, _, _ = postgres.build_of(self.pins, system)
            self.assertEqual(version.split("-")[0], image.group(1))

    def test_each_download_is_named_by_its_version_and_has_a_sha256_of_its_own(self):
        self.assertEqual(sorted(postgres.SYSTEMS), ["macos", "windows"])
        sums = set()
        for system in postgres.SYSTEMS:
            version, url, expected = postgres.build_of(self.pins, system)
            self.assertIn(f"postgresql-{version}-", url)
            self.assertTrue(url.startswith("https://get.enterprisedb.com/postgresql/"))
            self.assertRegex(expected, r"^[0-9a-f]{64}$")
            sums.add(expected)
        self.assertEqual(len(sums), 2)

    def test_a_missing_or_malformed_pin_is_refused(self):
        with self.assertRaisesRegex(postgres.Failure, "JUNTO_CI_POSTGRES_MACOS_SHA256"):
            postgres.build_of(postgres.read_pins("JUNTO_CI_POSTGRES_BUILD=17.11-1\n"), "macos")
        malformed = postgres.read_pins("JUNTO_CI_POSTGRES_BUILD=17.11-1\nJUNTO_CI_POSTGRES_MACOS_SHA256=abc\n")
        with self.assertRaisesRegex(postgres.Failure, "no valid PostgreSQL build for macos"):
            postgres.build_of(malformed, "macos")

    def test_the_server_options_are_those_of_the_docker_wrapper(self):
        script = (REPOSITORY / "tools/with-test-postgres.sh").read_text()
        docker = re.findall(r"-c (\S+)", script)
        self.assertTrue(docker)
        self.assertEqual(postgres.SERVER_SETTINGS, docker)


class Fetch(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.downloads = pathlib.Path(self.temporary.name)
        self.data = archive_bytes({"pgsql/bin/postgres.exe": b"program"})
        self.url = "https://example.test/postgresql-17.11-1-osx-binaries.zip"
        self.sum = hashlib.sha256(self.data).hexdigest()
        self.requests = 0

    def tearDown(self):
        self.temporary.cleanup()

    def download(self, url, path):
        self.requests += 1
        path.write_bytes(self.data)

    def test_a_download_with_the_pinned_sum_is_kept_and_used_again(self):
        path = postgres.fetch(self.downloads, self.url, self.sum, self.download)
        self.assertEqual(path.read_bytes(), self.data)
        self.assertEqual(postgres.fetch(self.downloads, self.url, self.sum, self.download), path)
        self.assertEqual(self.requests, 1)
        self.assertEqual(sorted(p.name for p in self.downloads.iterdir()), [path.name])

    def test_a_download_with_another_sum_is_refused_and_removed(self):
        with self.assertRaisesRegex(postgres.Failure, "SHA-256"):
            postgres.fetch(self.downloads, self.url, "0" * 64, self.download)
        self.assertEqual(list(self.downloads.iterdir()), [])

    def test_a_kept_file_with_another_sum_is_downloaded_again(self):
        (self.downloads / "postgresql-17.11-1-osx-binaries.zip").write_bytes(b"cut")
        path = postgres.fetch(self.downloads, self.url, self.sum, self.download)
        self.assertEqual(path.read_bytes(), self.data)
        self.assertEqual(self.requests, 1)

    def test_the_download_of_an_earlier_version_is_removed(self):
        (self.downloads / "postgresql-17.10-1-osx-binaries.zip").write_bytes(b"old")
        (self.downloads / "notes.txt").write_bytes(b"kept")
        path = postgres.fetch(self.downloads, self.url, self.sum, self.download)
        self.assertEqual(sorted(p.name for p in self.downloads.iterdir()), sorted([path.name, "notes.txt"]))


def zip_entry(name, mode):
    entry = zipfile.ZipInfo(name)
    entry.create_system = 3
    entry.external_attr = mode << 16
    return entry


class System(unittest.TestCase):
    def test_windows_and_macos_have_a_build_and_other_systems_none(self):
        expected = {"nt": "windows"}.get(os.name, "macos" if sys.platform == "darwin" else None)
        if expected is None:
            with self.assertRaisesRegex(postgres.Failure, "with-test-postgres.sh"):
                postgres.current_system()
        else:
            self.assertEqual(postgres.current_system(), expected)
        with mock.patch.object(postgres.os, "name", "posix"), mock.patch.object(postgres.sys, "platform", "linux"):
            with self.assertRaisesRegex(postgres.Failure, "with-test-postgres.sh"):
                postgres.current_system()


class Extract(unittest.TestCase):
    def test_only_the_programs_libraries_and_shared_files_are_extracted(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            archive = root / "pg.zip"
            archive.write_bytes(archive_bytes({
                "pgsql/bin/initdb.exe": b"i",
                "pgsql/lib/pg_stat_statements.dll": b"l",
                "pgsql/share/extension/pg_stat_statements.control": b"c",
                "pgsql/pgAdmin 4/runtime/pgAdmin4.exe": b"a",
                "pgsql/doc/index.html": b"d",
                "pgsql/include/libpq-fe.h": b"h",
            }))
            installation = postgres.extract(archive, root / "installation")
            files = sorted(p.relative_to(installation).as_posix()
                           for p in installation.rglob("*") if p.is_file())
            self.assertEqual(files, ["bin/initdb.exe", "lib/pg_stat_statements.dll",
                                     "share/extension/pg_stat_statements.control"])

    @unittest.skipIf(os.name == "nt", "Windows has no permission bits of a program")
    def test_a_file_keeps_the_permission_bits_of_the_zip_without_writes_for_others(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            archive = root / "pg.zip"
            with zipfile.ZipFile(archive, "w") as opened:
                opened.writestr(zip_entry("pgsql/bin/initdb", 0o100777), b"i")
                opened.writestr(zip_entry("pgsql/share/postgres.bki", 0o100644), b"s")
            installation = postgres.extract(archive, root / "installation")
            self.assertEqual(stat.S_IMODE((installation / "bin/initdb").stat().st_mode), 0o755)
            self.assertEqual(stat.S_IMODE((installation / "share/postgres.bki").stat().st_mode), 0o644)

    def test_a_link_in_the_zip_is_refused(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            archive = root / "pg.zip"
            with zipfile.ZipFile(archive, "w") as opened:
                opened.writestr(zip_entry("pgsql/lib/libpq.dylib", 0o120777), b"/etc/passwd")
            with self.assertRaisesRegex(postgres.Failure, "not a file"):
                postgres.extract(archive, root / "installation")

    def test_a_name_with_backslashes_is_refused_on_every_system(self):
        # Windows reads the backslashes as separators: the file would be written beside the
        # folder of the installation.
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            archive = root / "pg.zip"
            archive.write_bytes(archive_bytes({"pgsql/bin/..\\..\\outside": b"x"}))
            with self.assertRaisesRegex(postgres.Failure, "outside its folder"):
                postgres.extract(archive, root / "installation")
            self.assertEqual(sorted(p.name for p in root.iterdir()), ["pg.zip"])

    def test_a_path_is_inside_its_folder_only_below_it(self):
        folder = pathlib.Path(tempfile.gettempdir()) / "installation"
        self.assertTrue(postgres.inside(folder / "bin" / "initdb", folder))
        self.assertFalse(postgres.inside(folder / "bin" / ".." / ".." / "outside", folder))
        self.assertFalse(postgres.inside(folder, folder))
        self.assertFalse(postgres.inside(folder.parent / "installation-2" / "x", folder))


STREAMS = "stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL"
PYTHON = sys.executable


def detached(program, arguments=()):
    """Starts a process that is no child of the tests: its parent ends at once. Returns its
    process number."""
    parent = (f"import subprocess, sys; child = subprocess.Popen({[str(program), *arguments]!r}, {STREAMS}); "
              "print(child.pid)")
    result = subprocess.run([sys.executable, "-c", parent], capture_output=True, text=True, timeout=30, check=True)
    return int(result.stdout.split()[0])


def alive(number):
    """Whether a process with the number runs. Only reads; the tests use it for numbers that
    they did not start as a server."""
    if os.name == "nt":
        import ctypes
        from ctypes import wintypes

        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
        kernel.OpenProcess.restype = wintypes.HANDLE
        kernel.GetExitCodeProcess.argtypes = [wintypes.HANDLE, ctypes.POINTER(wintypes.DWORD)]
        kernel.CloseHandle.argtypes = [wintypes.HANDLE]
        handle = kernel.OpenProcess(0x1000, False, number)
        if not handle:
            return False
        try:
            code = wintypes.DWORD()
            return bool(kernel.GetExitCodeProcess(handle, ctypes.byref(code))) and code.value == 259
        finally:
            kernel.CloseHandle(handle)
    try:
        os.kill(number, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def until(condition, seconds=30):
    deadline = time.monotonic() + seconds
    while not condition() and time.monotonic() < deadline:
        time.sleep(0.05)
    return condition()


# A server with one child process; the child states its number in the file of the first
# argument. With "ignore", the server ignores SIGINT.
SERVER = (f"import signal, subprocess, sys, time\n"
          f"if sys.argv[2:] == ['ignore']: signal.signal(signal.SIGINT, signal.SIG_IGN)\n"
          f"child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(60)'], {STREAMS})\n"
          f"open(sys.argv[1], 'w').write(str(child.pid))\n"
          f"time.sleep(60)\n")


class ServerProcess(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(ignore_cleanup_errors=True)
        self.addCleanup(self.temporary.cleanup)
        self.root = pathlib.Path(self.temporary.name)

    def start(self, *arguments):
        server = postgres.Server([PYTHON, "-c", SERVER, str(self.root / "child"), *arguments], self.root / "log")
        self.addCleanup(server.close)
        self.addCleanup(lambda: server.running() and server.stop(1))
        self.assertTrue(until(lambda: (self.root / "child").exists() and (self.root / "child").read_text()))
        return server, int((self.root / "child").read_text())

    def test_the_server_and_its_child_process_end_with_the_stop(self):
        server, child = self.start()
        self.assertTrue(server.running())
        self.assertIs(server.stop(10), True)
        self.assertFalse(server.running())
        self.assertTrue(until(lambda: not alive(child)))
        self.assertFalse(alive(server.number))

    @unittest.skipIf(os.name == "nt", "Windows ends every process of the job at once")
    def test_a_server_that_ignores_sigint_is_ended_with_its_group(self):
        server, child = self.start("ignore")
        began = time.monotonic()
        self.assertIs(server.stop(1), True)
        self.assertLess(time.monotonic() - began, 10)
        self.assertTrue(until(lambda: not alive(child)))

    @unittest.skipIf(os.name == "nt", "the Windows server is ended by its job, not by signals")
    def test_every_signal_goes_to_the_number_of_the_own_child_only(self):
        server, child = self.start("ignore")
        sent = []
        kill, killpg = os.kill, os.killpg

        def recorded_kill(number, signal_number):
            sent.append(("kill", number, signal_number))
            return kill(number, signal_number)

        def recorded_killpg(number, signal_number):
            sent.append(("killpg", number, signal_number))
            return killpg(number, signal_number)

        with mock.patch.object(postgres.os, "kill", recorded_kill), \
                mock.patch.object(postgres.os, "killpg", recorded_killpg):
            self.assertIs(server.stop(1), True)
        self.assertTrue(sent)
        self.assertEqual({number for _, number, _ in sent}, {server.number})
        self.assertNotIn(child, {number for _, number, _ in sent})

    @unittest.skipIf(os.name == "nt", "the Windows server is ended by its job, not by signals")
    def test_a_server_that_exited_before_the_stop_is_collected_after_the_signals(self):
        server = postgres.Server([PYTHON, "-c", "pass"], self.root / "log")
        self.assertTrue(until(lambda: not server.running()))
        self.assertIsNone(server.process.returncode)
        self.assertIs(server.stop(1), True)
        self.assertEqual(server.process.returncode, 0)

    @unittest.skipIf(os.name == "nt", "the Windows server is ended by its job, not by signals")
    def test_an_error_of_the_forced_end_leaves_the_stop_with_the_error(self):
        server, _ = self.start("ignore")

        def refused(number, signal_number):
            raise PermissionError(f"operation not permitted: {number} {signal_number}")

        with mock.patch.object(postgres.os, "killpg", refused):
            with self.assertRaises(PermissionError):
                server.stop(1)

    def test_a_program_that_cannot_start_is_an_error_of_the_system(self):
        with self.assertRaises(OSError):
            postgres.Server([str(self.root / "no-program")], self.root / "log")

    def test_the_wait_for_the_server_fails_when_the_server_ended(self):
        server = postgres.Server([PYTHON, "-c", "pass"], self.root / "log")
        self.addCleanup(server.close)
        self.addCleanup(server.stop, 1)
        self.assertTrue(until(lambda: not server.running()))
        with self.assertRaisesRegex(postgres.Failure, "ended during its start"):
            postgres.wait_until_ready(server, self.root / "pgsql", 5999, self.root / "log")


class FakeServer:
    """A server for the tests of main: `stop` runs the function of the test."""
    stop_with = staticmethod(lambda: True)
    started_with = staticmethod(lambda: None)

    def __init__(self, _command, _log):
        type(self).started_with()

    def running(self):
        return True

    def stop(self):
        return type(self).stop_with()

    def close(self):
        pass


@unittest.skipIf(os.name == "nt", "Windows ends a process on SIGTERM without a handler")
class Main(unittest.TestCase):
    """main with the download, initdb and the wait for the server replaced; the command is a
    Python program."""

    def setUp(self):
        self.handlers = {number: signal.getsignal(number) for number in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP)}
        self.stops = []
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = pathlib.Path(self.temporary.name) / "run"
        self.root.mkdir()
        patches = [
            mock.patch.object(postgres, "current_system", lambda: "macos"),
            mock.patch.object(postgres, "fetch", lambda *_arguments: pathlib.Path("pg.zip")),
            mock.patch.object(postgres, "extract", lambda *_arguments: None),
            mock.patch.object(postgres, "checked", lambda *_arguments, **_options: None),
            mock.patch.object(postgres, "free_port", lambda: 5999),
            mock.patch.object(postgres, "wait_until_ready", lambda *_arguments: None),
            mock.patch.object(postgres.tempfile, "mkdtemp", lambda **_options: str(self.root)),
        ]
        for patch in patches:
            patch.start()
            self.addCleanup(patch.stop)

    def tearDown(self):
        for number, handler in self.handlers.items():
            signal.signal(number, handler)

    def main(self, command):
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()) as errors:
            status = postgres.main([sys.executable, "-c", command])
        return status, errors.getvalue()

    def fake(self, stop, started=lambda: None):
        server = type("Server", (FakeServer,), {"stop_with": staticmethod(stop), "started_with": staticmethod(started)})
        patch = mock.patch.object(postgres, "Server", server)
        patch.start()
        self.addCleanup(patch.stop)

    def test_a_number_in_postmaster_pid_of_another_process_is_never_signalled(self):
        other = detached(PYTHON, ["-c", "import time; time.sleep(60)"])
        self.addCleanup(end_process, other)
        (self.root / "data").mkdir()
        (self.root / "data" / "postmaster.pid").write_text(f"{other}\n")
        servers = []
        real = postgres.Server

        def recorded(command, log):
            servers.append(real(command, log))
            return servers[-1]

        sleeper = [PYTHON, "-c", "import time; time.sleep(60)"]
        with mock.patch.object(postgres, "server_command", lambda *_arguments: sleeper), \
                mock.patch.object(postgres, "Server", recorded):
            status, errors = self.main("pass")
        self.assertEqual(status, 0, errors)
        self.assertTrue(alive(other))
        self.assertEqual(len(servers), 1)
        self.assertFalse(servers[0].running())
        self.assertFalse(self.root.exists())

    def test_a_server_that_cannot_be_ended_fails_the_program(self):
        sleeper = [PYTHON, "-c", "import signal, time; signal.signal(signal.SIGINT, signal.SIG_IGN); time.sleep(60)"]
        servers = []
        real = postgres.Server

        def recorded(command, log):
            servers.append(real(command, log))
            return servers[-1]

        killpg = os.killpg

        def refused(number, signal_number):
            if signal_number:
                raise PermissionError("operation not permitted")
            return killpg(number, signal_number)

        with mock.patch.object(postgres, "server_command", lambda *_arguments: sleeper), \
                mock.patch.object(postgres, "Server", recorded), \
                mock.patch.object(postgres, "STOP_SECONDS", 1), \
                mock.patch.object(postgres.os, "killpg", refused):
            status, errors = self.main("pass")
        killpg(servers[0].number, signal.SIGKILL)
        servers[0].process.wait(timeout=30)
        self.assertEqual(status, 1)
        self.assertIn("operation not permitted", errors)
        self.assertIn("still runs", errors)
        self.assertFalse(self.root.exists())

    def test_a_signal_during_the_stop_does_not_interrupt_it_and_the_exit_states_the_signal(self):
        def stop():
            self.stops.append("started")
            os.kill(os.getpid(), signal.SIGTERM)
            time.sleep(0.2)
            self.stops.append("finished")
            return True

        self.fake(stop)
        status, errors = self.main("pass")
        self.assertEqual(self.stops, ["started", "finished"])
        self.assertEqual(status, 128 + signal.SIGTERM)
        self.assertIn(f"stopped after signal {int(signal.SIGTERM)}", errors)

    def test_a_signal_during_the_start_of_the_server_stops_the_started_server(self):
        def stop():
            self.stops.append("stopped")
            return True

        self.fake(stop, started=lambda: os.kill(os.getpid(), signal.SIGTERM))
        status, _ = self.main("raise SystemExit(3)")
        self.assertEqual(self.stops, ["stopped"])
        self.assertEqual(status, 128 + signal.SIGTERM)

    def test_a_signal_during_the_command_ends_it_and_stops_the_server_once(self):
        def stop():
            self.stops.append("stopped")
            return len(self.stops) > 1

        self.fake(stop)
        command = "import os, signal, time; os.kill(os.getppid(), signal.SIGTERM); time.sleep(30)"
        began = time.monotonic()
        status, errors = self.main(command)
        self.assertLess(time.monotonic() - began, 20)
        self.assertEqual(self.stops, ["stopped"])
        self.assertEqual(status, 128 + signal.SIGTERM)
        self.assertIn("still runs", errors)
        self.assertFalse(self.root.exists())

    def test_a_server_that_still_runs_after_the_stop_fails_the_program(self):
        self.fake(lambda: False)
        status, errors = self.main("pass")
        self.assertEqual(status, 1)
        self.assertIn("still runs", errors)

    def test_an_error_of_the_stop_is_reported_and_the_directory_is_removed(self):
        def stop():
            raise PermissionError("operation not permitted")

        self.fake(stop)
        status, errors = self.main("pass")
        self.assertEqual(status, 1)
        self.assertIn("operation not permitted", errors)
        self.assertIn("still runs", errors)
        self.assertFalse(self.root.exists())

    def test_after_the_cleanup_a_signal_has_its_default_effect(self):
        self.fake(lambda: True)
        self.assertEqual(self.main("pass")[0], 0)
        for number in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
            self.assertEqual(signal.getsignal(number), signal.SIG_DFL)

    def test_the_exit_code_of_the_command_is_the_exit_code_of_the_program(self):
        self.fake(lambda: True)
        self.assertEqual(self.main("raise SystemExit(7)")[0], 7)
        self.root.mkdir()
        self.assertEqual(self.main("pass")[0], 0)


class Signals(unittest.TestCase):
    @unittest.skipIf(os.name == "nt", "Windows ends a process on SIGTERM without a handler")
    def test_only_the_first_signal_raises_none_after_defer_and_a_held_one_at_the_end(self):
        program = ("import os, signal, sys; sys.path.insert(0, sys.argv[1]); import with_native_postgres as p\n"
                   "s = p.Signals(); s.install()\n"
                   "try:\n"
                   "    os.kill(os.getpid(), signal.SIGTERM)\n"
                   "except p.Ended as ended:\n"
                   "    os.kill(os.getpid(), signal.SIGHUP)\n"
                   "    print('ended', ended.number, s.number)\n"
                   "t = p.Signals(); t.install(); t.defer()\n"
                   "os.kill(os.getpid(), signal.SIGINT)\n"
                   "print('deferred', t.number)\n"
                   "u = p.Signals(); u.install()\n"
                   "try:\n"
                   "    with u.held():\n"
                   "        os.kill(os.getpid(), signal.SIGHUP)\n"
                   "        print('held')\n"
                   "except p.Ended as ended:\n"
                   "    print('after', ended.number)\n")
        result = subprocess.run([sys.executable, "-c", program, str(REPOSITORY / "tools")],
                                capture_output=True, text=True, timeout=30)
        self.assertEqual(result.returncode, 0, result.stderr)
        term, interrupt = str(int(signal.SIGTERM)), str(int(signal.SIGINT))
        hangup = str(int(signal.SIGHUP))
        self.assertEqual(result.stdout.split(), ["ended", term, term, "deferred", interrupt, "held", "after", hangup])


class Commands(unittest.TestCase):
    def test_the_server_is_postgres_itself_on_the_loopback_address_without_a_unix_socket(self):
        installation = pathlib.Path("pg")
        command = postgres.server_command(installation, pathlib.Path("data"), 5999)
        self.assertEqual(command[0], postgres.program(installation, "postgres"))
        self.assertEqual(command[1:3], ["-D", "data"])
        pairs = list(zip(command[3::2], command[4::2]))
        self.assertIn(("-c", "listen_addresses=127.0.0.1"), pairs)
        self.assertIn(("-c", "unix_socket_directories="), pairs)
        self.assertIn(("-p", "5999"), pairs)
        for setting in postgres.SERVER_SETTINGS:
            self.assertIn(("-c", setting), pairs)

    def test_a_program_whose_child_keeps_its_output_open_does_not_hold_the_run(self):
        # The child states its process number, so that the test ends it before the folder is
        # removed: Windows removes no file that a process holds open.
        child = "import time; time.sleep(30)"
        parent = (f"import subprocess, sys; child = subprocess.Popen([sys.executable, '-c', {child!r}]); "
                  "print('started', child.pid)")
        with tempfile.TemporaryDirectory() as temporary:
            output = pathlib.Path(temporary) / "output"
            began = time.monotonic()
            postgres.checked([sys.executable, "-c", parent], "the program", output)
            elapsed = time.monotonic() - began
            words = output.read_text().split()
            try:
                self.assertLess(elapsed, 20)
                self.assertEqual(words[0], "started")
            finally:
                end_process(int(words[1]))

    def test_a_failed_program_prints_its_output_and_the_log_of_the_server(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            (root / "log").write_text("FATAL: port in use")
            printed = io.StringIO()
            with contextlib.redirect_stdout(printed), self.assertRaises(postgres.Failure):
                postgres.checked([sys.executable, "-c", "print('no start'); raise SystemExit(3)"], "the start",
                                 root / "output", root / "log")
            self.assertIn("exit code 3", printed.getvalue())
            self.assertIn("no start", printed.getvalue())
            self.assertIn("FATAL: port in use", printed.getvalue())

    def test_the_command_gets_the_address_of_the_database(self):
        environment = postgres.command_environment({"PATH": "x"}, 5999)
        self.assertEqual(environment["JUNTO_TEST_DATABASE_URL"], "postgres://postgres@127.0.0.1:5999/postgres")
        self.assertEqual(environment["PATH"], "x")


if __name__ == "__main__":
    unittest.main()
