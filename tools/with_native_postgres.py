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


def extract(archive, destination):
    """Extracts the parts of the server below `destination`, without the top folder `pgsql`.
    A file keeps the permission bits that the ZIP states for it."""
    with zipfile.ZipFile(archive) as opened:
        for member in opened.infolist():
            parts = pathlib.PurePosixPath(member.filename).parts
            if len(parts) < 3 or parts[0] != "pgsql" or parts[1] not in PARTS or member.is_dir():
                continue
            if any(part in ("..", "") for part in parts) or ":" in member.filename:
                raise Failure(f"the ZIP holds a path outside its folder: {member.filename}")
            # A ZIP written on Unix states the type and the permission bits of each entry.
            mode = member.external_attr >> 16 if member.create_system == 3 else 0
            if stat.S_IFMT(mode) not in (0, stat.S_IFREG):
                raise Failure(f"the ZIP holds an entry that is not a file: {member.filename}")
            target = destination.joinpath(*parts[1:])
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


def stop(installation, data, output):
    """Stops the server of `data` when one runs; prints why a stop failed."""
    if not (data / "postmaster.pid").exists():
        return
    try:
        checked(stop_command(installation, data), "the stop of PostgreSQL", output)
    except Failure:
        pass


class Ended(BaseException):
    """A signal that ends the program; the server is stopped before the exit."""

    def __init__(self, number):
        super().__init__(number)
        self.number = number


def end_on_signals():
    """pg_ctl starts the server in a session of its own, so the end of the process group of
    this program does not end it. The first signal that ends this program raises Ended, and
    the stop of the server runs; the next signals are ignored, so that they do not end the
    stop."""
    numbers = [getattr(signal, name) for name in ("SIGTERM", "SIGINT", "SIGHUP") if hasattr(signal, name)]

    def raise_ended(number, _frame):
        for each in numbers:
            signal.signal(each, signal.SIG_IGN)
        raise Ended(number)

    for number in numbers:
        signal.signal(number, raise_ended)


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
    end_on_signals()
    try:
        with tempfile.TemporaryDirectory(prefix="junto-postgres-", ignore_cleanup_errors=True) as temporary:
            root = pathlib.Path(temporary)
            installation, data, log = root / "pgsql", root / "data", root / "server.log"
            output = root / "programs.log"
            try:
                try:
                    extract(fetch(downloads, url, expected), installation)
                    started = time.monotonic()
                    checked(init_command(installation, data), "initdb", output)
                    # The runner has no other process that takes a port between this choice
                    # and the start; a taken port ends the start with the log of the server.
                    port = free_port()
                    # On Windows, pg_ctl starts the server with the administrator rights
                    # removed from its token, which the server requires.
                    checked(start_command(installation, data, log, port), "the start of PostgreSQL", output, log)
                except (Failure, OSError) as failure:
                    print(f"PostgreSQL {version} did not start: {failure}", flush=True)
                    return 1
                print(f"PostgreSQL {version} runs at 127.0.0.1:{port}, started in "
                      f"{time.monotonic() - started:.0f} s", flush=True)
                return subprocess.run(arguments, env=command_environment(os.environ, port)).returncode
            finally:
                stop(installation, data, output)
    except Ended as ended:
        print(f"PostgreSQL {version} stopped after signal {ended.number}", file=sys.stderr, flush=True)
        return 128 + ended.number


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
