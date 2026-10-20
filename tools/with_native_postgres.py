"""Runs a command with a temporary PostgreSQL on Windows or macOS, without Docker, and removes
the database afterwards (ADR 0912). The command receives JUNTO_TEST_DATABASE_URL, as from
tools/with-test-postgres.sh.

The Docker engine of a Windows runner runs Windows containers only, and the macOS runners of
GitHub have no Docker, so the PostgreSQL image of tools/with-test-postgres.sh cannot run
there. This program uses the PostgreSQL build of the system, of the same version, downloaded
once into a folder of the account and checked by its SHA-256, with the server settings of
that script.

    python3 tools/with_native_postgres.py <command> [arguments]
    python3 tools/with_native_postgres.py --fetch
    python3 tools/with_native_postgres.py --pin

`--fetch` only downloads and checks the ZIP, so that CI can keep it before the tests run.

The version and the SHA-256 of each ZIP are those of tools/ci/pins.sh. `--pin` prints the
SHA-256 of the ZIP of the system, for the key of a cache.

The downloaded ZIP is kept in JUNTO_TEST_POSTGRES_DOWNLOADS (default: .cache/junto-postgres in
the home folder). Each run has a new data directory, which is removed at the end, also when
the program receives SIGTERM, SIGINT or SIGHUP. A failure to start prints the log of the
server.
"""

import hashlib
import os
import pathlib
import re
import shutil
import signal
import socket
import stat
import subprocess
import sys
import tempfile
import time
import typing
import urllib.request
import zipfile

PINS = pathlib.Path(__file__).resolve().parent / "ci" / "pins.sh"
# The name of the ZIP of binaries from EnterpriseDB for each system, and the name of its
# SHA-256 in tools/ci/pins.sh. The build for macOS holds programs for x86-64 and arm64.
SYSTEMS = {
    "windows": ("windows-x64", "JUNTO_CI_POSTGRES_WINDOWS_SHA256"),
    "macos": ("osx", "JUNTO_CI_POSTGRES_MACOS_SHA256"),
}
# The settings of tools/with-test-postgres.sh, in its order; a unit test compares them.
SERVER_SETTINGS = [
    "max_connections=400",
    "fsync=off",
    "shared_preload_libraries=pg_stat_statements",
    "pg_stat_statements.max=100000",
]
# The parts of the ZIP that the server needs. The ZIP also holds pgAdmin, the documentation
# and the C headers, about 18,000 of the 21,903 files of the Windows ZIP.
PARTS = ("bin", "lib", "share")
START_SECONDS = 120


class Failure(Exception):
    pass


def read_pins(text):
    """The values of the lines NAME=VALUE of tools/ci/pins.sh whose name starts with
    JUNTO_CI_POSTGRES_."""
    return dict(re.findall(r"^(JUNTO_CI_POSTGRES_[A-Z0-9_]+)=(\S+)$", text, re.MULTILINE))


def build_of(pins, system):
    """The version, the address and the SHA-256 of the ZIP of `system`."""
    name, sha256 = SYSTEMS[system]
    try:
        version, expected = pins["JUNTO_CI_POSTGRES_BUILD"], pins[sha256]
    except KeyError as missing:
        raise Failure(f"tools/ci/pins.sh has no {missing.args[0]}") from None
    if not re.fullmatch(r"[0-9]+\.[0-9]+-[0-9]+", version) or not re.fullmatch(r"[0-9a-f]{64}", expected):
        raise Failure(f"tools/ci/pins.sh has no valid PostgreSQL build for {system}: {version} {expected}")
    return version, f"https://get.enterprisedb.com/postgresql/postgresql-{version}-{name}-binaries.zip", expected


def current_system():
    if os.name == "nt":
        return "windows"
    if sys.platform == "darwin":
        return "macos"
    raise Failure(f"no PostgreSQL build for {sys.platform}; tools/with-test-postgres.sh starts one in Docker")


def sha256_of(path):
    digest = hashlib.sha256()
    with open(path, "rb") as file:
        for block in iter(lambda: file.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def download(url, path):
    with urllib.request.urlopen(url, timeout=120) as response, open(path, "wb") as file:
        shutil.copyfileobj(response, file, 1 << 20)


def fetch(downloads, url, expected, get=download):
    """The ZIP of `url` in `downloads`, downloaded when it is absent or has another SHA-256.
    The other ZIPs of the folder, of an earlier version, are removed."""
    downloads.mkdir(parents=True, exist_ok=True)
    path = downloads / url.rsplit("/", 1)[1]
    if path.exists() and sha256_of(path) == expected:
        print(f"{path.name}: the kept download has the pinned SHA-256", flush=True)
    else:
        partial = path.with_name(path.name + ".part")
        started = time.monotonic()
        get(url, partial)
        found = sha256_of(partial)
        if found != expected:
            partial.unlink()
            raise Failure(f"the download of {url} has the SHA-256 {found}, not the pinned {expected}")
        partial.replace(path)
        print(f"{path.name}: downloaded in {time.monotonic() - started:.0f} s, SHA-256 checked", flush=True)
    for other in downloads.glob("postgresql-*.zip"):
        if other != path:
            other.unlink()
    return path


def inside(path, folder):
    """Whether `path`, with its links and its ".." parts resolved, is below `folder`."""
    path, folder = os.path.realpath(path), os.path.realpath(folder)
    return os.path.commonpath([path, folder]) == folder and path != folder


def extract(archive, destination):
    """Extracts the parts of the server below `destination`, without the top folder `pgsql`.
    A file keeps the permission bits that the ZIP states for it."""
    with zipfile.ZipFile(archive) as opened:
        for member in opened.infolist():
            parts = pathlib.PurePosixPath(member.filename).parts
            if len(parts) < 3 or parts[0] != "pgsql" or parts[1] not in PARTS or member.is_dir():
                continue
            # Windows reads a backslash as a separator, so pgsql/bin/..\..\x would leave the
            # folder there although no part of the name is "..".
            target = destination.joinpath(*parts[1:])
            if (any(part in ("..", "") for part in parts) or ":" in member.filename or "\\" in member.filename
                    or not inside(target, destination)):
                raise Failure(f"the ZIP holds a path outside its folder: {member.filename}")
            # A ZIP written on Unix states the type and the permission bits of each entry.
            mode = member.external_attr >> 16 if member.create_system == 3 else 0
            if stat.S_IFMT(mode) not in (0, stat.S_IFREG):
                raise Failure(f"the ZIP holds an entry that is not a file: {member.filename}")
            target.parent.mkdir(parents=True, exist_ok=True)
            with opened.open(member) as source, open(target, "wb") as file:
                shutil.copyfileobj(source, file, 1 << 20)
            if mode and os.name != "nt":
                target.chmod(stat.S_IMODE(mode) & 0o755)
    return destination


def program(installation, name):
    return str(installation / "bin" / name)


def free_port():
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


def init_command(installation, data):
    return [program(installation, "initdb"), "-D", str(data), "-U", "postgres", "-A", "trust",
            "-E", "UTF8", "--no-sync"]


def start_command(installation, data, log, port):
    """The server listens on TCP only: without a Unix socket it writes no file outside its
    data directory."""
    settings = " ".join(f"-c {setting}" for setting in SERVER_SETTINGS)
    return [program(installation, "pg_ctl"), "start", "-D", str(data), "-l", str(log), "-w",
            "-t", str(START_SECONDS), "-o",
            f"-c listen_addresses=127.0.0.1 -c unix_socket_directories= -p {port} {settings}"]


def stop_command(installation, data):
    return [program(installation, "pg_ctl"), "stop", "-D", str(data), "-m", "immediate", "-w"]


def command_environment(environment, port):
    return {**environment, "JUNTO_TEST_DATABASE_URL": f"postgres://postgres@127.0.0.1:{port}/postgres"}


def checked(command, what, output, log=None):
    """Runs a PostgreSQL program with its output in the file `output`; on a failure prints that
    output and the log of the server. The server that `pg_ctl start` starts holds the output
    handles of pg_ctl as long as it runs, so a pipe would not reach its end."""
    with open(output, "w") as file:
        try:
            returncode = subprocess.run(command, stdin=subprocess.DEVNULL, stdout=file, stderr=subprocess.STDOUT,
                                        timeout=START_SECONDS + 60).returncode
        except subprocess.TimeoutExpired:
            returncode = None
    if returncode != 0:
        print(f"{what} failed (exit code {returncode}): {command}", flush=True)
        print(output.read_text(errors="replace"), flush=True)
        if log is not None and log.exists():
            print(f"the log of the server:\n{log.read_text(errors='replace')}", flush=True)
        raise Failure(f"{what} failed")


class Process(typing.NamedTuple):
    """A process as the system states it: its number, its start time and the resolved path of
    its executable. A process number that the system gives again to another process has
    another start time."""
    number: int
    start: int
    path: str


def program_path(path):
    return os.path.normcase(os.path.realpath(path))


def server_program(installation):
    return program_path(installation / "bin" / ("postgres.exe" if os.name == "nt" else "postgres"))


if os.name == "nt":
    import ctypes
    from ctypes import wintypes

    KERNEL = ctypes.WinDLL("kernel32", use_last_error=True)
    KERNEL.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    KERNEL.OpenProcess.restype = wintypes.HANDLE
    KERNEL.GetExitCodeProcess.argtypes = [wintypes.HANDLE, ctypes.POINTER(wintypes.DWORD)]
    KERNEL.GetProcessTimes.argtypes = [wintypes.HANDLE] + [ctypes.POINTER(wintypes.FILETIME)] * 4
    KERNEL.QueryFullProcessImageNameW.argtypes = [wintypes.HANDLE, wintypes.DWORD, wintypes.LPWSTR,
                                                  ctypes.POINTER(wintypes.DWORD)]
    KERNEL.TerminateProcess.argtypes = [wintypes.HANDLE, wintypes.UINT]
    KERNEL.CloseHandle.argtypes = [wintypes.HANDLE]
    KERNEL.CreateToolhelp32Snapshot.argtypes = [wintypes.DWORD, wintypes.DWORD]
    KERNEL.CreateToolhelp32Snapshot.restype = wintypes.HANDLE

    class ProcessEntry(ctypes.Structure):
        _fields_ = [("dwSize", wintypes.DWORD), ("cntUsage", wintypes.DWORD),
                    ("th32ProcessID", wintypes.DWORD), ("th32DefaultHeapID", ctypes.c_size_t),
                    ("th32ModuleID", wintypes.DWORD), ("cntThreads", wintypes.DWORD),
                    ("th32ParentProcessID", wintypes.DWORD), ("pcPriClassBase", ctypes.c_long),
                    ("dwFlags", wintypes.DWORD), ("szExeFile", ctypes.c_wchar * 260)]

    KERNEL.Process32FirstW.argtypes = [wintypes.HANDLE, ctypes.POINTER(ProcessEntry)]
    KERNEL.Process32NextW.argtypes = [wintypes.HANDLE, ctypes.POINTER(ProcessEntry)]
    QUERY_LIMITED_INFORMATION, TERMINATE, STILL_ACTIVE = 0x1000, 0x0001, 259

    def handle_identity(handle, number):
        """The identity of the running process of `handle`, or None when it has ended."""
        code = wintypes.DWORD()
        if not KERNEL.GetExitCodeProcess(handle, ctypes.byref(code)) or code.value != STILL_ACTIVE:
            return None
        created, ended, kernel_time, user_time = (wintypes.FILETIME() for _ in range(4))
        if not KERNEL.GetProcessTimes(handle, ctypes.byref(created), ctypes.byref(ended),
                                      ctypes.byref(kernel_time), ctypes.byref(user_time)):
            return None
        size = wintypes.DWORD(32768)
        path = ctypes.create_unicode_buffer(size.value)
        if not KERNEL.QueryFullProcessImageNameW(handle, 0, path, ctypes.byref(size)):
            return None
        return Process(number, (created.dwHighDateTime << 32) | created.dwLowDateTime, program_path(path.value))

    def children_of(number):
        """The numbers of the processes whose parent is `number`."""
        snapshot = KERNEL.CreateToolhelp32Snapshot(0x2, 0)
        if snapshot in (None, wintypes.HANDLE(-1).value):
            raise ctypes.WinError(ctypes.get_last_error())
        try:
            entry = ProcessEntry()
            entry.dwSize = ctypes.sizeof(ProcessEntry)
            found = []
            more = KERNEL.Process32FirstW(snapshot, ctypes.byref(entry))
            while more:
                if entry.th32ParentProcessID == number and entry.th32ProcessID != number:
                    found.append(entry.th32ProcessID)
                more = KERNEL.Process32NextW(snapshot, ctypes.byref(entry))
            return found
        finally:
            KERNEL.CloseHandle(snapshot)

elif sys.platform == "darwin":
    import ctypes

    LIBPROC = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)

    class BsdInfo(ctypes.Structure):
        # struct proc_bsdinfo of <sys/proc_info.h>
        _fields_ = [("flags", ctypes.c_uint32), ("status", ctypes.c_uint32), ("xstatus", ctypes.c_uint32),
                    ("pid", ctypes.c_uint32), ("ppid", ctypes.c_uint32), ("uid", ctypes.c_uint32),
                    ("gid", ctypes.c_uint32), ("ruid", ctypes.c_uint32), ("rgid", ctypes.c_uint32),
                    ("svuid", ctypes.c_uint32), ("svgid", ctypes.c_uint32), ("rfu", ctypes.c_uint32),
                    ("comm", ctypes.c_char * 16), ("name", ctypes.c_char * 32), ("nfiles", ctypes.c_uint32),
                    ("pgid", ctypes.c_uint32), ("pjobc", ctypes.c_uint32), ("tdev", ctypes.c_uint32),
                    ("tpgid", ctypes.c_uint32), ("nice", ctypes.c_int32), ("start_seconds", ctypes.c_uint64),
                    ("start_microseconds", ctypes.c_uint64)]


def identity(number):
    """The process `number` as the system states it now, or None when no such process runs."""
    if number is None or number <= 0:
        return None
    if os.name == "nt":
        handle = KERNEL.OpenProcess(QUERY_LIMITED_INFORMATION, False, number)
        if not handle:
            return None
        try:
            return handle_identity(handle, number)
        finally:
            KERNEL.CloseHandle(handle)
    if sys.platform == "darwin":
        info = BsdInfo()
        bsd_info = 3
        if LIBPROC.proc_pidinfo(number, bsd_info, ctypes.c_uint64(0), ctypes.byref(info),
                                ctypes.sizeof(info)) != ctypes.sizeof(info):
            return None
        path = ctypes.create_string_buffer(4096)
        if LIBPROC.proc_pidpath(number, path, len(path)) <= 0:
            return None
        return Process(number, info.start_seconds * 1_000_000 + info.start_microseconds,
                       program_path(os.fsdecode(path.value)))
    try:
        with open(f"/proc/{number}/stat") as file:
            fields = file.read().rsplit(")", 1)[1].split()
        if fields[0] == "Z":
            return None
        return Process(number, int(fields[19]), program_path(os.readlink(f"/proc/{number}/exe")))
    except (OSError, IndexError, ValueError):
        return None


def runs(process):
    """Whether `process` still runs: a process with its number and its start time. The path is
    read only to find the server; the start time alone tells it from a later process that
    received the same number."""
    now = None if process is None else identity(process.number)
    return now is not None and now.start == process.start


def server_of(installation, data):
    """The server that `data` names in postmaster.pid, when that process runs the program of
    `installation`. The directory of the installation is new in each run, so a process of
    another program, which received a number that the server had, is never taken for it."""
    try:
        number = int((data / "postmaster.pid").read_text().splitlines()[0])
    except (OSError, ValueError, IndexError):
        return None
    process = identity(number)
    if process is None or process.path != server_program(installation):
        return None
    return process


def end(process):
    """Ends `process` and its child processes, when `process` still runs. On Unix the server
    that pg_ctl starts leads a process group of its own, which holds its child processes; on
    Windows each child with the program of the server is ended by a handle that is checked
    against its identity first."""
    if not runs(process):
        return
    if os.name == "nt":
        targets = [process] + [child for child in map(identity, children_of(process.number))
                               if child is not None and child.path == process.path and child.start >= process.start]
        for target in targets:
            handle = KERNEL.OpenProcess(QUERY_LIMITED_INFORMATION | TERMINATE, False, target.number)
            if not handle:
                continue
            try:
                now = handle_identity(handle, target.number)
                if now is not None and now.start == target.start and not KERNEL.TerminateProcess(handle, 1):
                    raise ctypes.WinError(ctypes.get_last_error())
            finally:
                KERNEL.CloseHandle(handle)
        return
    try:
        if os.getpgid(process.number) == process.number:
            os.killpg(process.number, signal.SIGKILL)
        else:
            os.kill(process.number, signal.SIGKILL)
    except ProcessLookupError:
        pass


def stop(installation, data, output, server=None, end_seconds=30):
    """Stops `server`, or the server of `data` when it is not given, with pg_ctl. When pg_ctl
    fails or its time limit ends and the server still runs, ends the server and its child
    processes. Acts only while the process has the identity of the server, never on the
    number of postmaster.pid alone. Returns whether the server has ended."""
    server = server or server_of(installation, data)
    if not runs(server):
        return True
    try:
        checked(stop_command(installation, data), "the stop of PostgreSQL", output)
    except (Failure, OSError) as failure:
        print(f"PostgreSQL was not stopped by pg_ctl: {failure}", flush=True)
    if not runs(server):
        return True
    print(f"PostgreSQL still runs after pg_ctl stop: process {server.number} and its child processes are ended",
          flush=True)
    try:
        end(server)
    except (OSError, subprocess.SubprocessError) as error:
        print(f"the processes of PostgreSQL were not ended: {error}", flush=True)
    deadline = time.monotonic() + end_seconds
    while runs(server) and time.monotonic() < deadline:
        time.sleep(0.1)
    return not runs(server)


class Ended(BaseException):
    """A signal that ends the program; the server is stopped before the exit."""

    def __init__(self, number):
        super().__init__(number)
        self.number = number


class Signals:
    """SIGTERM, SIGINT and SIGHUP end the program after the stop of the server. pg_ctl starts
    the server in a session of its own, so the end of the process group of this program does
    not end it. Only the first signal raises Ended, and only before `defer`: the cleanup calls
    `defer` first, so no signal interrupts it, and a signal during the cleanup is recorded."""

    def __init__(self):
        self.number = None
        self.raising = True

    def install(self):
        for name in ("SIGTERM", "SIGINT", "SIGHUP"):
            if hasattr(signal, name):
                signal.signal(getattr(signal, name), self.handle)

    def handle(self, number, _frame):
        if self.number is None:
            self.number = number
        if self.raising:
            self.raising = False
            raise Ended(number)

    def defer(self):
        self.raising = False

    def restore(self):
        """The default handling again: a signal after the cleanup ends the program at once,
        with the signal as its status. Returns the first recorded signal."""
        for name in ("SIGTERM", "SIGINT", "SIGHUP"):
            if hasattr(signal, name):
                signal.signal(getattr(signal, name), signal.SIG_DFL)
        return self.number


def serve(arguments, version, downloads, url, expected, installation, data, log, output, started_servers):
    """Starts the server of `data` and runs the command with its address; returns the exit
    code of the command, or 1 when the server did not start. The identity of the started
    server goes into `started_servers`."""
    try:
        extract(fetch(downloads, url, expected), installation)
        started = time.monotonic()
        checked(init_command(installation, data), "initdb", output)
        # The runner has no other process that takes a port between this choice and the
        # start; a taken port ends the start with the log of the server.
        port = free_port()
        # On Windows, pg_ctl starts the server with the administrator rights removed from
        # its token, which the server requires.
        checked(start_command(installation, data, log, port), "the start of PostgreSQL", output, log)
        started_servers.append(server_of(installation, data))
    except (Failure, OSError) as failure:
        print(f"PostgreSQL {version} did not start: {failure}", flush=True)
        return 1
    print(f"PostgreSQL {version} runs at 127.0.0.1:{port}, started in {time.monotonic() - started:.0f} s",
          flush=True)
    return subprocess.run(arguments, env=command_environment(os.environ, port)).returncode


def main(arguments):
    if not arguments:
        print("usage: with_native_postgres.py --fetch | --pin | <command> [arguments]", file=sys.stderr)
        return 2
    default = pathlib.Path.home() / ".cache" / "junto-postgres"
    downloads = pathlib.Path(os.environ.get("JUNTO_TEST_POSTGRES_DOWNLOADS", default))
    try:
        version, url, expected = build_of(read_pins(PINS.read_text()), current_system())
    except (Failure, OSError) as failure:
        print(failure, file=sys.stderr)
        return 2
    if arguments == ["--pin"]:
        print(expected)
        return 0
    if arguments == ["--fetch"]:
        try:
            fetch(downloads, url, expected)
        except (Failure, OSError) as failure:
            print(f"PostgreSQL {version} was not downloaded: {failure}", flush=True)
            return 1
        return 0
    signals = Signals()
    root = pathlib.Path(tempfile.mkdtemp(prefix="junto-postgres-"))
    installation, data, log = root / "pgsql", root / "data", root / "server.log"
    output = root / "programs.log"
    started_servers, results = [], []

    def cleanup():
        """Stops the server and removes the directory of the run, once; later calls return
        the result of the first."""
        signals.defer()
        if not results:
            try:
                stopped = stop(installation, data, output, started_servers[0] if started_servers else None)
            except Exception as error:
                print(f"the stop of PostgreSQL failed: {error!r}", file=sys.stderr, flush=True)
                stopped = False
            finally:
                shutil.rmtree(root, ignore_errors=True)
            results.append(stopped)
        return results[0]

    # Ended can be raised once, before the cleanup has called defer. When it is raised at
    # the start of the cleanup of the finally, the cleanup runs in the except.
    status = 1
    try:
        try:
            signals.install()
            status = serve(arguments, version, downloads, url, expected, installation, data, log, output,
                           started_servers)
        finally:
            cleanup()
    except Ended:
        cleanup()
    stopped = results[0]
    number = signals.restore()
    if number is not None:
        print(f"PostgreSQL {version} stopped after signal {number}", file=sys.stderr, flush=True)
        status = 128 + number
    if not stopped:
        print(f"PostgreSQL {version} still runs: its processes did not end", file=sys.stderr, flush=True)
        status = status or 1
    return status


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
