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
the program receives SIGTERM, SIGINT or SIGHUP. The server is a direct child process of this
program; pg_ctl is not used. A failure to start prints the log of the server.
"""

import contextlib
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
    return str(installation / "bin" / (name + ".exe" if os.name == "nt" else name))


def free_port():
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


def init_command(installation, data):
    return [program(installation, "initdb"), "-D", str(data), "-U", "postgres", "-A", "trust",
            "-E", "UTF8", "--no-sync"]


def command_environment(environment, port):
    return {**environment, "JUNTO_TEST_DATABASE_URL": f"postgres://postgres@127.0.0.1:{port}/postgres"}


def checked(command, what, output, log=None):
    """Runs a PostgreSQL program with its output in the file `output`; on a failure prints that
    output and the log of the server. A program whose child process keeps its output open
    would hold a pipe open until that child ends."""
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


STOP_SECONDS = 30


class UnixServer:
    """The postgres process of the run, a direct child of this program in a session and a
    process group of its own, whose number is the number of the process. Until this program
    collects the child, the system gives neither number to another process, so the signals
    of `stop` reach only this server and the processes of its group."""

    def __init__(self, command, log):
        with open(log, "ab") as file:
            self.process = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=file, stderr=subprocess.STDOUT,
                                            start_new_session=True)
        self.number = self.process.pid

    def running(self):
        """Whether the server has not exited. The exited server is not collected here."""
        if self.process.returncode is not None:
            return False
        try:
            return os.waitid(os.P_PID, self.process.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT) is None
        except ChildProcessError:
            return False

    def signal(self, send, number):
        """Sends the signal to the server or its group, before the server is collected. macOS
        refuses a signal to an exited server that is not collected, and to a group with only
        that server, with EPERM: then no process is left to signal."""
        try:
            send(self.process.pid, number)
        except ProcessLookupError:
            pass
        except PermissionError:
            if self.running():
                raise

    def group_runs(self):
        try:
            os.killpg(self.process.pid, 0)
        except ProcessLookupError:
            return False
        except PermissionError:
            return True
        return True

    def stop(self, seconds=None):
        """A fast shutdown with SIGINT; after `seconds`, or at once when the server has exited,
        SIGKILL to the processes of its group; then collects the server. Returns whether no
        process of the group runs."""
        seconds = STOP_SECONDS if seconds is None else seconds
        deadline = time.monotonic() + seconds
        if self.process.returncode is None:
            # Not Popen.send_signal: it collects an exited child first, and the number of a
            # collected child can belong to another process.
            self.signal(os.kill, signal.SIGINT)
            while self.running() and time.monotonic() < deadline:
                time.sleep(0.05)
            self.signal(os.killpg, signal.SIGKILL)
            self.process.wait(timeout=max(1.0, deadline - time.monotonic()))
        # The group of the collected server can have members only of this server: a group
        # number stays with its group as long as one member runs.
        while self.group_runs() and time.monotonic() < deadline + seconds:
            time.sleep(0.05)
        return not self.group_runs()

    def close(self):
        pass


if os.name == "nt":
    import ctypes
    from ctypes import wintypes

    KERNEL = ctypes.WinDLL("kernel32", use_last_error=True)
    ADVAPI = ctypes.WinDLL("advapi32", use_last_error=True)

    class StartupInfo(ctypes.Structure):
        _fields_ = [("cb", wintypes.DWORD), ("lpReserved", wintypes.LPWSTR), ("lpDesktop", wintypes.LPWSTR),
                    ("lpTitle", wintypes.LPWSTR), ("dwX", wintypes.DWORD), ("dwY", wintypes.DWORD),
                    ("dwXSize", wintypes.DWORD), ("dwYSize", wintypes.DWORD), ("dwXCountChars", wintypes.DWORD),
                    ("dwYCountChars", wintypes.DWORD), ("dwFillAttribute", wintypes.DWORD),
                    ("dwFlags", wintypes.DWORD), ("wShowWindow", wintypes.WORD), ("cbReserved2", wintypes.WORD),
                    ("lpReserved2", ctypes.c_void_p), ("hStdInput", wintypes.HANDLE),
                    ("hStdOutput", wintypes.HANDLE), ("hStdError", wintypes.HANDLE)]

    class ProcessInformation(ctypes.Structure):
        _fields_ = [("hProcess", wintypes.HANDLE), ("hThread", wintypes.HANDLE),
                    ("dwProcessId", wintypes.DWORD), ("dwThreadId", wintypes.DWORD)]

    class SidAndAttributes(ctypes.Structure):
        _fields_ = [("Sid", ctypes.c_void_p), ("Attributes", wintypes.DWORD)]

    class SidAuthority(ctypes.Structure):
        _fields_ = [("Value", ctypes.c_ubyte * 6)]

    class BasicLimits(ctypes.Structure):
        _fields_ = [("PerProcessUserTimeLimit", ctypes.c_int64), ("PerJobUserTimeLimit", ctypes.c_int64),
                    ("LimitFlags", wintypes.DWORD), ("MinimumWorkingSetSize", ctypes.c_size_t),
                    ("MaximumWorkingSetSize", ctypes.c_size_t), ("ActiveProcessLimit", wintypes.DWORD),
                    ("Affinity", ctypes.c_size_t), ("PriorityClass", wintypes.DWORD),
                    ("SchedulingClass", wintypes.DWORD)]

    class ExtendedLimits(ctypes.Structure):
        _fields_ = [("BasicLimitInformation", BasicLimits), ("IoInfo", ctypes.c_uint64 * 6),
                    ("ProcessMemoryLimit", ctypes.c_size_t), ("JobMemoryLimit", ctypes.c_size_t),
                    ("PeakProcessMemoryUsed", ctypes.c_size_t), ("PeakJobMemoryUsed", ctypes.c_size_t)]

    class BasicAccounting(ctypes.Structure):
        _fields_ = [("TotalUserTime", ctypes.c_int64), ("TotalKernelTime", ctypes.c_int64),
                    ("ThisPeriodTotalUserTime", ctypes.c_int64), ("ThisPeriodTotalKernelTime", ctypes.c_int64),
                    ("TotalPageFaultCount", wintypes.DWORD), ("TotalProcesses", wintypes.DWORD),
                    ("ActiveProcesses", wintypes.DWORD), ("TotalTerminatedProcesses", wintypes.DWORD)]

    KERNEL.GetCurrentProcess.restype = wintypes.HANDLE
    KERNEL.CreateJobObjectW.argtypes = [ctypes.c_void_p, wintypes.LPCWSTR]
    KERNEL.CreateJobObjectW.restype = wintypes.HANDLE
    KERNEL.SetInformationJobObject.argtypes = [wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p, wintypes.DWORD]
    KERNEL.QueryInformationJobObject.argtypes = [wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p, wintypes.DWORD,
                                                 ctypes.c_void_p]
    KERNEL.AssignProcessToJobObject.argtypes = [wintypes.HANDLE, wintypes.HANDLE]
    KERNEL.TerminateJobObject.argtypes = [wintypes.HANDLE, wintypes.UINT]
    KERNEL.ResumeThread.argtypes = [wintypes.HANDLE]
    KERNEL.ResumeThread.restype = wintypes.DWORD
    KERNEL.TerminateProcess.argtypes = [wintypes.HANDLE, wintypes.UINT]
    KERNEL.GetExitCodeProcess.argtypes = [wintypes.HANDLE, ctypes.POINTER(wintypes.DWORD)]
    KERNEL.CloseHandle.argtypes = [wintypes.HANDLE]
    ADVAPI.OpenProcessToken.argtypes = [wintypes.HANDLE, wintypes.DWORD, ctypes.POINTER(wintypes.HANDLE)]
    ADVAPI.AllocateAndInitializeSid.argtypes = [ctypes.POINTER(SidAuthority), ctypes.c_ubyte] + \
        [wintypes.DWORD] * 8 + [ctypes.POINTER(ctypes.c_void_p)]
    ADVAPI.FreeSid.argtypes = [ctypes.c_void_p]
    ADVAPI.CreateRestrictedToken.argtypes = [wintypes.HANDLE, wintypes.DWORD, wintypes.DWORD,
                                             ctypes.POINTER(SidAndAttributes), wintypes.DWORD, ctypes.c_void_p,
                                             wintypes.DWORD, ctypes.c_void_p, ctypes.POINTER(wintypes.HANDLE)]
    ADVAPI.CreateProcessAsUserW.argtypes = [wintypes.HANDLE, wintypes.LPCWSTR, wintypes.LPWSTR, ctypes.c_void_p,
                                            ctypes.c_void_p, wintypes.BOOL, wintypes.DWORD, ctypes.c_void_p,
                                            wintypes.LPCWSTR, ctypes.POINTER(StartupInfo),
                                            ctypes.POINTER(ProcessInformation)]

    def checked_call(result):
        if not result:
            raise ctypes.WinError(ctypes.get_last_error())
        return result

    def restricted_token():
        """The token of this program without its privileges and with the groups Administrators
        and Power Users only for denial, as pg_ctl starts the server: PostgreSQL refuses to run
        with administrator rights."""
        token, restricted = wintypes.HANDLE(), wintypes.HANDLE()
        all_access, disable_max_privilege = 0xF01FF, 0x1
        checked_call(ADVAPI.OpenProcessToken(KERNEL.GetCurrentProcess(), all_access, ctypes.byref(token)))
        authority = SidAuthority((ctypes.c_ubyte * 6)(0, 0, 0, 0, 0, 5))
        sids = [ctypes.c_void_p(), ctypes.c_void_p()]
        try:
            for sid, alias in zip(sids, (544, 547)):
                checked_call(ADVAPI.AllocateAndInitializeSid(ctypes.byref(authority), 2, 32, alias, 0, 0, 0, 0, 0, 0,
                                                             ctypes.byref(sid)))
            disabled = (SidAndAttributes * 2)(SidAndAttributes(sids[0], 0), SidAndAttributes(sids[1], 0))
            checked_call(ADVAPI.CreateRestrictedToken(token, disable_max_privilege, 2, disabled, 0, None, 0, None,
                                                      ctypes.byref(restricted)))
            return restricted
        finally:
            for sid in sids:
                if sid.value:
                    ADVAPI.FreeSid(sid)
            KERNEL.CloseHandle(token)

    class WindowsServer:
        """The postgres process of the run, created suspended in a job object of its own that
        ends its processes when the job is closed, and resumed only after it is in the job, so
        every child process of the server is in the job. The program keeps the handles of the
        process and of the job: it never acts on a process number."""

        def __init__(self, command, log):
            self.job = checked_call(KERNEL.CreateJobObjectW(None, None))
            self.process = None
            try:
                limits = ExtendedLimits()
                kill_on_job_close, extended_limits = 0x2000, 9
                limits.BasicLimitInformation.LimitFlags = kill_on_job_close
                checked_call(KERNEL.SetInformationJobObject(self.job, extended_limits, ctypes.byref(limits),
                                                            ctypes.sizeof(limits)))
                self.start(command, log)
            except BaseException:
                self.close()
                raise

        def start(self, command, log):
            import msvcrt

            token = restricted_token()
            try:
                with open(log, "ab") as output, open(os.devnull, "rb") as nothing:
                    handles = [msvcrt.get_osfhandle(output.fileno()), msvcrt.get_osfhandle(nothing.fileno())]
                    for handle in handles:
                        os.set_handle_inheritable(handle, True)
                    startup = StartupInfo()
                    startup.cb = ctypes.sizeof(startup)
                    startup.dwFlags = 0x100  # STARTF_USESTDHANDLES
                    startup.hStdInput, startup.hStdOutput, startup.hStdError = handles[1], handles[0], handles[0]
                    information = ProcessInformation()
                    line = ctypes.create_unicode_buffer(subprocess.list2cmdline(command))
                    create_suspended, create_no_window = 0x4, 0x08000000
                    checked_call(ADVAPI.CreateProcessAsUserW(token, None, line, None, None, True,
                                                             create_suspended | create_no_window, None, None,
                                                             ctypes.byref(startup), ctypes.byref(information)))
            finally:
                KERNEL.CloseHandle(token)
            self.process, self.number = information.hProcess, information.dwProcessId
            try:
                if not KERNEL.AssignProcessToJobObject(self.job, self.process):
                    error = ctypes.WinError(ctypes.get_last_error())
                    KERNEL.TerminateProcess(self.process, 1)
                    raise error
                if KERNEL.ResumeThread(information.hThread) == 0xFFFFFFFF:
                    raise ctypes.WinError(ctypes.get_last_error())
            finally:
                KERNEL.CloseHandle(information.hThread)

        def running(self):
            code = wintypes.DWORD()
            return bool(KERNEL.GetExitCodeProcess(self.process, ctypes.byref(code))) and code.value == 259

        def active_processes(self):
            accounting = BasicAccounting()
            basic_accounting = 1
            checked_call(KERNEL.QueryInformationJobObject(self.job, basic_accounting, ctypes.byref(accounting),
                                                          ctypes.sizeof(accounting), None))
            return accounting.ActiveProcesses

        def stop(self, seconds=None):
            """Ends every process of the job. Returns whether no process of the job runs."""
            checked_call(KERNEL.TerminateJobObject(self.job, 1))
            deadline = time.monotonic() + (STOP_SECONDS if seconds is None else seconds)
            while self.active_processes() and time.monotonic() < deadline:
                time.sleep(0.05)
            return self.active_processes() == 0

        def close(self):
            for handle in (self.process, self.job):
                if handle:
                    KERNEL.CloseHandle(handle)
            self.process = self.job = None

    Server = WindowsServer
else:
    Server = UnixServer


def server_command(installation, data, port):
    """The server listens on TCP only: without a Unix socket it writes no file outside its
    data directory."""
    settings = [part for setting in SERVER_SETTINGS for part in ("-c", setting)]
    return [program(installation, "postgres"), "-D", str(data), "-c", "listen_addresses=127.0.0.1",
            "-c", "unix_socket_directories=", "-p", str(port), *settings]


def wait_until_ready(server, installation, port, log):
    """Waits until the server accepts connections; fails when it exits or after START_SECONDS."""
    deadline = time.monotonic() + START_SECONDS
    ready = [program(installation, "pg_isready"), "-q", "-h", "127.0.0.1", "-p", str(port), "-U", "postgres"]
    while time.monotonic() < deadline:
        if not server.running():
            raise Failure("the server ended during its start")
        if subprocess.run(ready, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                          timeout=30).returncode == 0:
            return
        time.sleep(0.1)
    raise Failure(f"the server did not accept connections within {START_SECONDS} s")


class Ended(BaseException):
    """A signal that ends the program; the server is stopped before the exit."""

    def __init__(self, number):
        super().__init__(number)
        self.number = number


class Signals:
    """SIGTERM, SIGINT and SIGHUP end the program after the stop of the server. The server runs
    in a session of its own, so the end of the process group of this program does not end it.
    The first signal raises Ended once, outside `held` and before `defer`; a signal while the
    signals are held is raised at the end of `held`; after `defer` signals are only recorded."""

    def __init__(self):
        self.number = None
        self.raising = True
        self.raised = False

    def install(self):
        for name in ("SIGTERM", "SIGINT", "SIGHUP"):
            if hasattr(signal, name):
                signal.signal(getattr(signal, name), self.handle)

    def handle(self, number, _frame):
        if self.number is None:
            self.number = number
        if self.raising and not self.raised:
            self.raised = True
            raise Ended(number)

    @contextlib.contextmanager
    def held(self):
        """No signal raises inside: the start of the server and its record are not separated."""
        before, self.raising = self.raising, False
        try:
            yield
        finally:
            self.raising = before
        if before and self.number is not None and not self.raised:
            self.raised = True
            raise Ended(self.number)

    def defer(self):
        self.raising = False

    def restore(self):
        """The default handling again: a signal after the cleanup ends the program at once,
        with the signal as its status. Returns the first recorded signal."""
        for name in ("SIGTERM", "SIGINT", "SIGHUP"):
            if hasattr(signal, name):
                signal.signal(getattr(signal, name), signal.SIG_DFL)
        return self.number


def serve(arguments, version, downloads, url, expected, installation, data, log, output, servers, signals):
    """Starts the server of `data` and runs the command with its address; returns the exit
    code of the command, or 1 when the server did not start. The started server goes into
    `servers`."""
    try:
        extract(fetch(downloads, url, expected), installation)
        started = time.monotonic()
        checked(init_command(installation, data), "initdb", output)
        # The runner has no other process that takes a port between this choice and the
        # start; a taken port ends the start with the log of the server.
        port = free_port()
        with signals.held():
            servers.append(Server(server_command(installation, data, port), log))
        wait_until_ready(servers[0], installation, port, log)
    except (Failure, OSError) as failure:
        print(f"PostgreSQL {version} did not start: {failure}", flush=True)
        if log.exists():
            print(f"the log of the server:\n{log.read_text(errors='replace')}", flush=True)
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
    servers, results = [], []

    def cleanup():
        """Stops the server and removes the directory of the run, once; later calls return
        the result of the first."""
        signals.defer()
        if not results:
            stopped = True
            try:
                for server in servers:
                    stopped = server.stop() and stopped
            except Exception as error:
                print(f"the stop of PostgreSQL failed: {error!r}", file=sys.stderr, flush=True)
                stopped = False
            finally:
                for server in servers:
                    server.close()
                shutil.rmtree(root, ignore_errors=True)
            results.append(stopped)
        return results[0]

    # Ended can be raised once, before the cleanup has called defer. When it is raised at
    # the start of the cleanup of the finally, the cleanup runs in the except.
    status = 1
    try:
        try:
            signals.install()
            status = serve(arguments, version, downloads, url, expected, installation, data, log, output, servers,
                           signals)
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
