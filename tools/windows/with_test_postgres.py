"""Runs a command with a temporary PostgreSQL on Windows and removes the database afterwards
(ADR 0912). The command receives JUNTO_TEST_DATABASE_URL, as from tools/with-test-postgres.sh.

The Docker engine of a Windows runner runs Windows containers only, so the PostgreSQL image of
tools/with-test-postgres.sh cannot run there. This program uses the PostgreSQL build for
Windows of the same version, downloaded once into a folder of the account and checked by its
SHA-256, with the server settings of that script.

    python3 tools/windows/with_test_postgres.py <command> [arguments]
    python3 tools/windows/with_test_postgres.py --fetch

`--fetch` only downloads and checks the ZIP, so that CI can keep it before the tests run.

The downloaded ZIP is kept in JUNTO_TEST_POSTGRES_DOWNLOADS (default: .cache/junto-postgres in
the home folder). Each run has a new data directory, which is removed at the end. A failure to
start prints the log of the server.
"""

import hashlib
import os
import pathlib
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request
import zipfile

# The version is that of JUNTO_TEST_POSTGRES_IMAGE in .github/workflows/ci.yml;
# tools/check-pins.sh compares them.
VERSION = "17.11-1"
URL = f"https://get.enterprisedb.com/postgresql/postgresql-{VERSION}-windows-x64-binaries.zip"
SHA256 = "6eabdf00d2893713b75db4336a23c3fdf505f056e217ec6e2e95d901750cfea3"
# The settings of tools/with-test-postgres.sh, in its order; a unit test compares them.
SERVER_SETTINGS = [
    "max_connections=400",
    "fsync=off",
    "shared_preload_libraries=pg_stat_statements",
    "pg_stat_statements.max=100000",
]
# The parts of the ZIP that the server needs. The ZIP also holds pgAdmin, the documentation
# and the C headers, about 18,000 of its 21,903 files.
PARTS = ("bin", "lib", "share")
START_SECONDS = 120


class Failure(Exception):
    pass


def sha256_of(path):
    digest = hashlib.sha256()
    with open(path, "rb") as file:
        for block in iter(lambda: file.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def download(url, path):
    with urllib.request.urlopen(url, timeout=120) as response, open(path, "wb") as file:
        shutil.copyfileobj(response, file, 1 << 20)


def fetch(downloads, expected=SHA256, get=download):
    """The ZIP in `downloads`, downloaded when it is absent or has another SHA-256."""
    downloads.mkdir(parents=True, exist_ok=True)
    path = downloads / URL.rsplit("/", 1)[1]
    if path.exists() and sha256_of(path) == expected:
        print(f"PostgreSQL {VERSION}: the kept download has the pinned SHA-256", flush=True)
        return path
    partial = path.with_name(path.name + ".part")
    started = time.monotonic()
    get(URL, partial)
    found = sha256_of(partial)
    if found != expected:
        partial.unlink()
        raise Failure(f"the download of {URL} has the SHA-256 {found}, not the pinned {expected}")
    partial.replace(path)
    print(f"PostgreSQL {VERSION}: downloaded in {time.monotonic() - started:.0f} s, SHA-256 checked",
          flush=True)
    return path


def extract(archive, destination):
    """Extracts the parts of the server below `destination`, without the top folder `pgsql`."""
    with zipfile.ZipFile(archive) as opened:
        for member in opened.infolist():
            parts = pathlib.PurePosixPath(member.filename).parts
            if len(parts) < 3 or parts[0] != "pgsql" or parts[1] not in PARTS or member.is_dir():
                continue
            if any(part in ("..", "") for part in parts) or ":" in member.filename:
                raise Failure(f"the ZIP holds a path outside its folder: {member.filename}")
            target = destination.joinpath(*parts[1:])
            target.parent.mkdir(parents=True, exist_ok=True)
            with opened.open(member) as source, open(target, "wb") as file:
                shutil.copyfileobj(source, file, 1 << 20)
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
    settings = " ".join(f"-c {setting}" for setting in SERVER_SETTINGS)
    return [program(installation, "pg_ctl"), "start", "-D", str(data), "-l", str(log), "-w",
            "-t", str(START_SECONDS), "-o", f"-c listen_addresses=127.0.0.1 -p {port} {settings}"]


def stop_command(installation, data):
    return [program(installation, "pg_ctl"), "stop", "-D", str(data), "-m", "immediate", "-w"]


def command_environment(environment, port):
    return {**environment, "JUNTO_TEST_DATABASE_URL": f"postgres://postgres@127.0.0.1:{port}/postgres"}


def checked(command, what, output, log=None):
    """Runs a PostgreSQL program with its output in the file `output`; on a failure prints that
    output and the log of the server. On Windows the server that `pg_ctl start` starts holds
    the output handles of pg_ctl as long as it runs, so a pipe would not reach its end."""
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


def main(arguments):
    if not arguments:
        print("usage: with_test_postgres.py --fetch | <command> [arguments]", file=sys.stderr)
        return 2
    default = pathlib.Path.home() / ".cache" / "junto-postgres"
    downloads = pathlib.Path(os.environ.get("JUNTO_TEST_POSTGRES_DOWNLOADS", default))
    if arguments == ["--fetch"]:
        try:
            fetch(downloads)
        except (Failure, OSError) as failure:
            print(f"PostgreSQL {VERSION} was not downloaded: {failure}", flush=True)
            return 1
        return 0
    with tempfile.TemporaryDirectory(prefix="junto-postgres-", ignore_cleanup_errors=True) as temporary:
        root = pathlib.Path(temporary)
        installation, data, log = root / "pgsql", root / "data", root / "server.log"
        output = root / "programs.log"
        try:
            try:
                extract(fetch(downloads), installation)
                started = time.monotonic()
                checked(init_command(installation, data), "initdb", output)
                # The runner has no other process that takes a port between this choice and
                # the start; a taken port ends the start with the log of the server.
                port = free_port()
                # On Windows, pg_ctl starts the server with the administrator rights removed
                # from its token, which the server requires.
                checked(start_command(installation, data, log, port), "the start of PostgreSQL", output, log)
            except (Failure, OSError) as failure:
                print(f"PostgreSQL {VERSION} did not start: {failure}", flush=True)
                return 1
            print(f"PostgreSQL {VERSION} runs at 127.0.0.1:{port}, started in {time.monotonic() - started:.0f} s",
                  flush=True)
            return subprocess.run(arguments, env=command_environment(os.environ, port)).returncode
        finally:
            stop(installation, data, output)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
