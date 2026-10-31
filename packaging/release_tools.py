"""Shared parts of the release checks: the release tools of a test configuration and a local
release source, laid out and redirected as GitHub releases, that records what a client asks for."""
import contextlib
import http.server
import os
import pathlib
import re
import shutil
import subprocess
import sys
import threading

from programs import outcome

REPOSITORY = pathlib.Path(__file__).resolve().parent.parent
SUFFIX = ".exe" if os.name == "nt" else ""
TAG = re.compile(r"v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)")
NAME = re.compile(r"[A-Za-z0-9_-][A-Za-z0-9._-]*")


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


def publish_into(source, published, version):
    """Copies the files of one publication into the local source as the release of the tag
    v<version>, as a GitHub release holds them (ADR 0724)."""
    release = pathlib.Path(source) / "download" / f"v{version}"
    release.mkdir(parents=True)
    for path in sorted(pathlib.Path(published).iterdir()):
        shutil.copyfile(path, release / path.name)


def latest_requests(name, newest):
    """The requests of the latest file `name` with the redirects of the source."""
    return [f"/latest/download/{name}", f"/download/v{newest}/{name}", f"/storage/v{newest}/{name}"]


def version_requests(version, name):
    """The requests of the file `name` of the release of `version` with the redirect of the source."""
    return [f"/download/v{version}/{name}", f"/storage/v{version}/{name}"]


def update_requests(target, current, new, checks=0):
    """The requests of an update of the target `<system>-<package>-<architecture>` from `current`
    to `new`: the latest file of `checks` update checks and of the update, the new archive,
    then the release file and archive of the installed version for the rollback."""
    latest = latest_requests(f"latest-{target}.json", new)
    return [*latest * (checks + 1),
            *version_requests(new, f"junto-{new}-{target}.junto"),
            *version_requests(current, f"release-{current}-{target}.json"),
            *version_requests(current, f"junto-{current}-{target}.junto")]


def version_key(tag):
    match = TAG.fullmatch(tag)
    return tuple(int(part) for part in match.groups()) if match else None


@contextlib.contextmanager
def source(directory):
    """Serves the releases in `directory/download/v<version>/` on a loopback address as GitHub
    serves the files of releases, and records the path of each request. `/latest/download/<file>`
    redirects to the file of the highest version, `/download/<tag>/<file>` to
    `/storage/<tag>/<file>`, and that address holds the file."""
    releases = pathlib.Path(directory) / "download"

    class Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            self.server.paths.append(self.path)
            parts = self.path.split("/")
            if len(parts) != 4 or parts[0] or not all(NAME.fullmatch(part) for part in parts[1:]):
                return self.answer(404)
            _, first, second, name = parts
            if first == "latest" and second == "download":
                tags = [path.name for path in releases.iterdir() if version_key(path.name)] if releases.is_dir() else []
                if not tags:
                    return self.answer(404)
                return self.answer(302, f"/download/{max(tags, key=version_key)}/{name}")
            if first in ("download", "storage") and version_key(second) and (releases / second / name).is_file():
                if first == "download":
                    return self.answer(302, f"/storage/{second}/{name}")
                return self.answer(200, file=releases / second / name)
            return self.answer(404)

        def answer(self, status, location=None, file=None):
            self.send_response(status)
            if location is not None:
                self.send_header("Location", f"http://127.0.0.1:{self.server.server_port}{location}")
            self.send_header("Content-Length", str(file.stat().st_size if file else 0))
            self.end_headers()
            if file is not None:
                with open(file, "rb") as reader:
                    shutil.copyfileobj(reader, self.wfile)

        def log_message(self, *_arguments):
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
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
