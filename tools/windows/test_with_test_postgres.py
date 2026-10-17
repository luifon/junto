"""Unit tests of tools/windows/with_test_postgres.py. They run on every system and start no
PostgreSQL: the CI job on Windows starts the real one."""

import contextlib
import hashlib
import io
import os
import pathlib
import re
import signal
import sys
import tempfile
import time
import unittest
import zipfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import with_test_postgres as postgres  # noqa: E402

REPOSITORY = pathlib.Path(__file__).resolve().parents[2]


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
    def test_the_version_is_the_version_of_the_ci_image(self):
        pins = (REPOSITORY / "tools/ci/pins.sh").read_text()
        image = re.search(r"JUNTO_TEST_POSTGRES_IMAGE:=postgres:([0-9.]+)-", pins)
        self.assertIsNotNone(image, "tools/ci/pins.sh names no PostgreSQL image")
        self.assertEqual(postgres.VERSION.split("-")[0], image.group(1))

    def test_the_download_is_named_by_its_version_and_has_a_sha256(self):
        self.assertIn(postgres.VERSION, postgres.URL)
        self.assertTrue(postgres.URL.startswith("https://"))
        self.assertRegex(postgres.SHA256, r"^[0-9a-f]{64}$")

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
        self.sum = hashlib.sha256(self.data).hexdigest()
        self.requests = 0

    def tearDown(self):
        self.temporary.cleanup()

    def download(self, url, path):
        self.requests += 1
        path.write_bytes(self.data)

    def test_a_download_with_the_pinned_sum_is_kept_and_used_again(self):
        path = postgres.fetch(self.downloads, self.sum, self.download)
        self.assertEqual(path.read_bytes(), self.data)
        self.assertEqual(postgres.fetch(self.downloads, self.sum, self.download), path)
        self.assertEqual(self.requests, 1)
        self.assertEqual(sorted(p.name for p in self.downloads.iterdir()), [path.name])

    def test_a_download_with_another_sum_is_refused_and_removed(self):
        with self.assertRaisesRegex(postgres.Failure, "SHA-256"):
            postgres.fetch(self.downloads, "0" * 64, self.download)
        self.assertEqual(list(self.downloads.iterdir()), [])

    def test_a_kept_file_with_another_sum_is_downloaded_again(self):
        (self.downloads / postgres.URL.rsplit("/", 1)[1]).write_bytes(b"cut")
        path = postgres.fetch(self.downloads, self.sum, self.download)
        self.assertEqual(path.read_bytes(), self.data)
        self.assertEqual(self.requests, 1)


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


class Commands(unittest.TestCase):
    def test_the_server_listens_on_the_loopback_address_at_the_given_port(self):
        installation = pathlib.Path("C:/pg")
        command = postgres.start_command(installation, pathlib.Path("C:/data"), pathlib.Path("C:/log"), 5999)
        self.assertEqual(command[:2], [str(installation / "bin" / "pg_ctl"), "start"])
        self.assertIn("-w", command)
        options = command[command.index("-o") + 1]
        self.assertIn("-c listen_addresses=127.0.0.1", options)
        self.assertIn("-p 5999", options)
        for setting in postgres.SERVER_SETTINGS:
            self.assertIn(f"-c {setting}", options)

    def test_a_program_whose_child_keeps_its_output_open_does_not_hold_the_start(self):
        # pg_ctl start leaves the server running with the output handles of pg_ctl. The child
        # states its process number, so that the test ends it before the folder is removed:
        # Windows removes no file that a process holds open.
        child = "import time; time.sleep(30)"
        parent = (f"import subprocess, sys; child = subprocess.Popen([sys.executable, '-c', {child!r}]); "
                  "print('started', child.pid)")
        with tempfile.TemporaryDirectory() as temporary:
            output = pathlib.Path(temporary) / "output"
            began = time.monotonic()
            postgres.checked([sys.executable, "-c", parent], "the start", output)
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
