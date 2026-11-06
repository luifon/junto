"""Runs a command with a temporary LiveKit server that runs as a program of the machine, without
Docker, and ends it afterwards (ADR 0905). The command receives the variables of
tools/with-test-livekit.sh except JUNTO_TEST_LIVEKIT_CONTAINER: JUNTO_TEST_LIVEKIT_API_URL,
JUNTO_TEST_LIVEKIT_URL, JUNTO_TEST_LIVEKIT_API_KEY, JUNTO_TEST_LIVEKIT_API_SECRET and
JUNTO_TEST_WEBHOOK_ADDRESS.

The Docker engine of the Windows runner runs Windows containers only, so the LiveKit image of
tools/with-test-livekit.sh cannot run there. This program uses the release of the same version
for Windows, or for Linux, downloaded once into a folder of the account and checked by its
SHA-256, with the settings of that script and of deploy/livekit-common.yaml.

    python3 tools/with_native_livekit.py <command> [arguments]
    python3 tools/with_native_livekit.py --fetch

`--fetch` only downloads and checks the archive. The download is kept in
JUNTO_TEST_LIVEKIT_DOWNLOADS (default: .cache/junto-livekit in the home folder). The ports are
those of tools/with-test-livekit.sh (JUNTO_TEST_LIVEKIT_PORT, JUNTO_TEST_LIVEKIT_UDP_PORT,
JUNTO_TEST_WEBHOOK_ADDRESS, with its defaults). Every socket of the server is bound to 127.0.0.1:
the media go over UDP on the loopback interface, and the server has no TCP port for media, so
it opens no port that another machine reaches and Windows asks nothing of its firewall. The
log of the server is printed when the command or the start fails.
"""

import os
import pathlib
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import native_downloads  # noqa: E402

# The version of JUNTO_TEST_LIVEKIT_IMAGE in tools/ci/pins.sh; tools/check-pins.sh compares
# them. The SHA-256 values are those of checksums.txt of the release.
VERSION = "1.13.9"
RELEASES = {
    "win32": (f"livekit_{VERSION}_windows_amd64.zip", "livekit-server.exe",
              "d5b5e44c197f613fdd6bdb2ae53e9d086d848156d43f614c688c6b595cd3f25e"),
    "linux": (f"livekit_{VERSION}_linux_amd64.tar.gz", "livekit-server",
              "0b7fa208b662d09cfdeae8c06cf4c481aead0556086558b48250501e2e2d6e20"),
}
URL = f"https://github.com/livekit/livekit/releases/download/v{VERSION}/"
# 120 tries, 0.5 s apart, as tools/with-test-livekit.sh.
READY_TRIES = 120
COMMON = pathlib.Path(__file__).resolve().parents[1] / "deploy" / "livekit-common.yaml"


def release(platform=sys.platform):
    if platform not in RELEASES:
        raise native_downloads.Failure(f"no LiveKit release is pinned for {platform}")
    return RELEASES[platform]


def settings(environment):
    """The ports and keys of tools/with-test-livekit.sh, from the environment or its defaults."""
    return {
        "port": int(environment.get("JUNTO_TEST_LIVEKIT_PORT", "17880")),
        "udp_port": int(environment.get("JUNTO_TEST_LIVEKIT_UDP_PORT", "17882")),
        "webhook": environment.get("JUNTO_TEST_WEBHOOK_ADDRESS", "127.0.0.1:17890"),
        "key": environment.get("JUNTO_TEST_LIVEKIT_API_KEY", "devkey"),
        "secret": environment.get("JUNTO_TEST_LIVEKIT_API_SECRET", "secret"),
    }


def config(chosen, common):
    webhook_port = chosen["webhook"].rsplit(":", 1)[1]
    return f"""port: {chosen["port"]}
keys:
  {chosen["key"]}: {chosen["secret"]}
rtc:
  tcp_port: 0
  udp_port: {chosen["udp_port"]}
  use_external_ip: false
  node_ip: 127.0.0.1
  enable_loopback_candidate: true
  ips:
    includes: [127.0.0.1/32]
webhook:
  api_key: {chosen["key"]}
  urls:
    - http://127.0.0.1:{webhook_port}/internal/media/notifications
{common}"""


def command_environment(environment, chosen):
    port = chosen["port"]
    return {
        **environment,
        "JUNTO_TEST_LIVEKIT_API_URL": f"http://127.0.0.1:{port}",
        "JUNTO_TEST_LIVEKIT_URL": f"ws://127.0.0.1:{port}",
        "JUNTO_TEST_LIVEKIT_API_KEY": chosen["key"],
        "JUNTO_TEST_LIVEKIT_API_SECRET": chosen["secret"],
        "JUNTO_TEST_WEBHOOK_ADDRESS": chosen["webhook"],
    }


def ready(port, tries=READY_TRIES, server=None):
    for _ in range(tries):
        if server is not None and server.poll() is not None:
            return False
        try:
            with urllib.request.urlopen(f"http://127.0.0.1:{port}/", timeout=2):
                return True
        except (urllib.error.URLError, OSError):
            time.sleep(0.5)
    return False


def main(arguments):
    if not arguments:
        print("usage: with_native_livekit.py --fetch | <command> [arguments]", file=sys.stderr)
        return 2
    default = pathlib.Path.home() / ".cache" / "junto-livekit"
    downloads = pathlib.Path(os.environ.get("JUNTO_TEST_LIVEKIT_DOWNLOADS", default))
    label = f"LiveKit {VERSION}"
    try:
        name, program, sha256 = release()
        archive = native_downloads.fetch(label, URL + name, sha256, downloads)
    except (native_downloads.Failure, OSError) as failure:
        print(f"{label} was not downloaded: {failure}", flush=True)
        return 1
    if arguments == ["--fetch"]:
        return 0
    chosen = settings(os.environ)
    with tempfile.TemporaryDirectory(prefix="junto-livekit-", ignore_cleanup_errors=True) as temporary:
        root = pathlib.Path(temporary)
        log_path = root / "server.log"
        try:
            [server_program] = native_downloads.extract(archive, [program], root)
        except (native_downloads.Failure, OSError) as failure:
            print(f"{label} was not extracted: {failure}", flush=True)
            return 1
        environment = {**os.environ, "LIVEKIT_CONFIG": config(chosen, COMMON.read_text())}
        with open(log_path, "wb") as log:
            server = subprocess.Popen([str(server_program), "--dev", "--bind", "127.0.0.1"], env=environment,
                                      stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT)
        status = 1
        try:
            started = time.monotonic()
            if not ready(chosen["port"], server=server):
                print(f"{label} did not become ready (exit code {server.poll()})", flush=True)
            else:
                print(f"{label} runs at 127.0.0.1:{chosen['port']}, ready in {time.monotonic() - started:.1f} s",
                      flush=True)
                status = subprocess.run(arguments, env=command_environment(os.environ, chosen)).returncode
        finally:
            server.terminate()
            try:
                server.wait(timeout=30)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait()
            if status != 0:
                print(f"\nthe log of {label} (exit {status}):", flush=True)
                print(log_path.read_text(errors="replace"), flush=True)
        return status


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
