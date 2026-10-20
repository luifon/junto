"""Runs a command with a temporary PostgreSQL on macOS, without Docker, and removes the
database afterwards (ADR 0916). The command receives JUNTO_TEST_DATABASE_URL, as from
tools/with-test-postgres.sh.

The macOS runners of GitHub have no Docker, so the PostgreSQL image of
tools/with-test-postgres.sh cannot run there. This program uses the PostgreSQL build for
macOS of the same version, downloaded once into a folder of the account and checked by its
SHA-256, with the server settings of that script. Windows has its own program,
tools/windows/with_test_postgres.py (ADR 0912).

    python3 tools/with_native_postgres.py <command> [arguments]
    python3 tools/with_native_postgres.py --fetch
    python3 tools/with_native_postgres.py --pin

`--fetch` only downloads and checks the ZIP, so that CI can keep it before the tests run.

The version and the SHA-256 of the ZIP are those of tools/ci/pins.sh. `--pin` prints the
SHA-256, for the key of a cache.

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
# and the C headers, about 23,000 of its 24,600 files.
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
    if sys.platform == "darwin":
        return "macos"
    raise Failure(f"no PostgreSQL build for {sys.platform}; tools/with-test-postgres.sh starts one in Docker"
                  " and tools/windows/with_test_postgres.py one on Windows")


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
            if mode:
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


class Server:
    """The postmaster of the run, a direct child of this program in a session of its own, so
    that the end of the process group of this program does not reach it. The program
    collects it only after its last signal: until then the system gives its number to no
    other process, and every signal goes to this postmaster only. The child processes of
    PostgreSQL start sessions of their own and end when the postmaster has ended; they get
    no signal from this program."""

    def __init__(self, command, log, data, port):
        self.data, self.port = data, port
        with open(log, "ab") as file:
            self.process = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=file, stderr=subprocess.STDOUT,
                                            start_new_session=True)
        self.number = self.process.pid

    def running(self):
        """Whether the postmaster has not exited, read without collecting it. macOS also
        reports a stopped child to waitid with WEXITED; only an exit counts."""
        if self.process.returncode is not None:
            return False
        try:
            state = os.waitid(os.P_PID, self.number, os.WEXITED | os.WNOHANG | os.WNOWAIT)
        except ChildProcessError:
            return False
        return state is None or state.si_code not in (os.CLD_EXITED, os.CLD_KILLED, os.CLD_DUMPED)

    def signal(self, number):
        """Sends the signal to the postmaster, which is not collected. Not Popen.send_signal:
        it collects an exited child first. macOS refuses a signal to an exited child that is
        not collected with EPERM: then the postmaster has ended."""
        try:
            os.kill(self.number, number)
        except ProcessLookupError:
            pass
        except PermissionError:
            if self.running():
                raise

    def stop(self, seconds=None):
        """Stops the server in the order of PostgreSQL: SIGINT (fast shutdown, the postmaster
        ends its child processes), SIGQUIT (immediate shutdown), SIGKILL; each signal only
        while the postmaster runs, each followed by a wait of at most `seconds`. Then
        collects the postmaster and waits until no process uses the data directory. Returns
        whether none does."""
        seconds = STOP_SECONDS if seconds is None else seconds
        for number in (signal.SIGINT, signal.SIGQUIT, signal.SIGKILL):
            if not self.running():
                break
            self.signal(number)
            deadline = time.monotonic() + seconds
            while self.running() and time.monotonic() < deadline:
                time.sleep(0.05)
        self.process.wait(timeout=seconds)
        deadline = time.monotonic() + seconds
        while self.in_use() and time.monotonic() < deadline:
            time.sleep(0.05)
        if self.in_use():
            return False
        self.remove_segment()
        return True

    def segment(self):
        """The key and the identifier of the shared memory segment of the server, from line 7
        of postmaster.pid; None when the file or the line is absent."""
        try:
            fields = (self.data / "postmaster.pid").read_text().splitlines()[6].split()
            return int(fields[0]), int(fields[1])
        except (OSError, IndexError, ValueError):
            return None

    def attached(self, segment):
        """The number of processes attached to the segment with this key and identifier, as
        `ipcs` states it; 0 when no such segment exists. PostgreSQL itself decides with this
        number whether a data directory is in use."""
        key, identifier = segment
        listing = subprocess.run(["ipcs", "-m", "-o"], capture_output=True, text=True, timeout=30, check=True)
        for line in listing.stdout.splitlines():
            fields = line.split()
            if len(fields) >= 7 and fields[0] == "m" and fields[1] == str(identifier) and int(fields[2], 16) == key:
                return int(fields[6])
        return 0

    def port_open(self):
        try:
            with socket.create_connection(("127.0.0.1", self.port), timeout=5):
                return True
        except OSError:
            return False

    def in_use(self):
        """Whether a process of the server still uses its data directory: the port accepts
        connections, or a process is attached to the shared memory segment that
        postmaster.pid names. PostgreSQL removes postmaster.pid when it shuts down; after a
        SIGKILL the file stays."""
        segment = self.segment()
        return self.port_open() or (segment is not None and self.attached(segment) > 0)

    def remove_segment(self):
        """After a SIGKILL PostgreSQL leaves its segment; the system keeps it until it is
        removed. Removed only when no process is attached."""
        segment = self.segment()
        if segment is not None and self.attached(segment) == 0:
            subprocess.run(["ipcrm", "-m", str(segment[1])], capture_output=True, timeout=30)

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
            servers.append(Server(server_command(installation, data, port), log, data, port))
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
