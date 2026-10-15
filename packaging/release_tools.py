"""Shared parts of the release checks: the release tools of a test configuration and a local
release source that records what a client asks for."""
import contextlib
import functools
import http.server
import os
import pathlib
import subprocess
import sys
import threading

from programs import outcome

REPOSITORY = pathlib.Path(__file__).resolve().parent.parent
SUFFIX = ".exe" if os.name == "nt" else ""


def run(*arguments, **options):
    """Runs a command and returns its standard output; a failure reports the command and both outputs."""
    result = subprocess.run([str(argument) for argument in arguments], capture_output=True, text=True,
                            timeout=options.pop("timeout", 600), **options)
    if result.returncode:
        raise RuntimeError(outcome(result))
    return result.stdout


def keys(tool, directory):
    """Creates a release key and a freshness key; returns the secret and public files of each."""
    directory = pathlib.Path(directory)
    files = {}
    for role in ["release", "freshness"]:
        secret, public = directory / f"{role}.seed", directory / f"{role}-public.json"
        run(tool, "keygen", secret, public)
        files[role] = (secret, public)
    return files


def build(files, certificate=None):
    """Builds junto-release and junto-recover with the test keys compiled in, in the target
    directory of the repository. A build with these variables states "test configuration"."""
    environment = os.environ.copy()
    environment.update(JUNTO_TEST_UPDATE_KEYS_FILE=str(files["release"][1]),
                       JUNTO_TEST_FRESHNESS_KEYS_FILE=str(files["freshness"][1]))
    if certificate is not None:
        environment["JUNTO_TEST_CERTIFICATE_FILE"] = str(certificate)
    run("cargo", "build", "--locked", "-p", "junto-client-core", "--bin", "junto-release", "--bin", "junto-recover",
        cwd=REPOSITORY, env=environment, timeout=3600)
    target = pathlib.Path(environment.get("CARGO_TARGET_DIR", REPOSITORY / "target")) / "debug"
    return target / f"junto-release{SUFFIX}", target / f"junto-recover{SUFFIX}"


def package(tool, files, source, output, version, system, package_name, architecture):
    run(tool, "package", source, output, files["release"][0], files["freshness"][0], version, system, package_name,
        architecture)


@contextlib.contextmanager
def source(directory):
    """Serves `directory` on a loopback address and records the path of each request."""
    class Handler(http.server.SimpleHTTPRequestHandler):
        def do_GET(self):
            self.server.paths.append(self.path)
            super().do_GET()

        def log_message(self, *_arguments):
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), functools.partial(Handler, directory=str(directory)))
    server.paths = []
    thread = threading.Thread(target=server.serve_forever)
    thread.start()
    try:
        yield server, f"http://127.0.0.1:{server.server_port}/"
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


def report(message):
    print(message, flush=True)
    sys.stdout.flush()
