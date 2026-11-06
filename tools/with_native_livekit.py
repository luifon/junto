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
JUNTO_TEST_WEBHOOK_ADDRESS, with its defaults; the webhook address must be 127.0.0.1:<port>, where
the server posts its notifications). The signalling of the server is bound to 127.0.0.1, and
the server has no TCP port for media. On Linux its UDP port for media is bound to 127.0.0.1 as
well. On Windows the call library of the client offers no candidate on the loopback interface,
and Windows routes no UDP between an address of the network and 127.0.0.1, so the UDP port is
also bound to the private IPv4 addresses of the machine (10/8, 172.16/12, 192.168/16, whichever
the machine has; DHCP can change them). An inbound rule of Windows Defender Firewall blocks the
program, so no other machine reaches the port and Windows shows no prompt of its firewall
(ADR 0915); the traffic of the machine to its own addresses passes. The program is extracted
once beside the download, so that the rule names one path; `--fetch` creates the rule, which
needs an elevated account, and the server does not start on Windows without it. The log of the
server is printed when the command or the start fails, and with JUNTO_TEST_LIVEKIT_LOG=1 also
after a command that passed.

    python3 tools/with_native_livekit.py --fetch      (also the rule of the firewall on Windows)
"""

import hashlib
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
# The addresses of the UDP port of the server on Windows, besides 127.0.0.1: the private
# networks, which hold the address that DHCP gives the machine.
PRIVATE_NETWORKS = ["10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16"]


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


def webhook_port(address):
    """The port of JUNTO_TEST_WEBHOOK_ADDRESS. The server runs on this machine and posts to the
    loopback address, so the test server must listen there."""
    host, _, port = address.rpartition(":")
    if host != "127.0.0.1" or not port.isdigit():
        raise native_downloads.Failure(
            f"JUNTO_TEST_WEBHOOK_ADDRESS is {address}: the native LiveKit posts its notifications to "
            "127.0.0.1, so the address must be 127.0.0.1:<port>")
    return port


def config(chosen, common, platform=sys.platform):
    port = webhook_port(chosen["webhook"])
    networks = ["127.0.0.1/32", *(PRIVATE_NETWORKS if platform == "win32" else [])]
    node = "" if platform == "win32" else "  node_ip: 127.0.0.1\n"
    return f"""port: {chosen["port"]}
keys:
  {chosen["key"]}: {chosen["secret"]}
rtc:
  tcp_port: 0
  udp_port: {chosen["udp_port"]}
  use_external_ip: false
{node}  enable_loopback_candidate: true
  ips:
    includes: [{", ".join(networks)}]
webhook:
  api_key: {chosen["key"]}
  urls:
    - http://127.0.0.1:{port}/internal/media/notifications
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


def installed(archive, program, downloads, digest, extract=native_downloads.extract):
    """The program of the server, extracted once for each checked download (`digest`) into a
    folder of its own."""
    folder = downloads / f"livekit-{VERSION}"
    stamp = folder / "extracted-from"
    if stamp.exists() and stamp.read_text() == digest and (folder / program).exists():
        return folder / program
    [extracted] = extract(archive, [program], folder)
    stamp.write_text(digest)
    return extracted


def rule_name(program):
    """The name of the inbound rule of the firewall that blocks `program`: one per path."""
    digest = hashlib.sha256(str(program).lower().encode()).hexdigest()[:12]
    return f"junto-test-livekit-{digest}"


def rule_commands(program):
    """The netsh commands that read and that create the rule that blocks every inbound
    connection to `program` in every profile."""
    name = rule_name(program)
    show = ["netsh", "advfirewall", "firewall", "show", "rule", f"name={name}"]
    add = ["netsh", "advfirewall", "firewall", "add", "rule", f"name={name}", "dir=in", "action=block",
           f"program={program}", "profile=any", "enable=yes"]
    return show, add


def has_rule(program, run=subprocess.run):
    show, _ = rule_commands(program)
    return run(show, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0


def block(program, run=subprocess.run):
    """Creates the rule when it is absent. False when it cannot be created."""
    if has_rule(program, run):
        return True
    _, add = rule_commands(program)
    created = run(add, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    if created.returncode != 0:
        print(f"the rule of the firewall for {program} was not created (an elevated account creates it): "
              f"{created.stdout.strip()}", flush=True)
        return False
    print(f"the firewall blocks inbound connections to {program} (rule {rule_name(program)})", flush=True)
    return True


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
        server_program = installed(archive, program, downloads, sha256)
    except (native_downloads.Failure, OSError) as failure:
        print(f"{label} was not downloaded or extracted: {failure}", flush=True)
        return 1
    if arguments == ["--fetch"]:
        return 0 if sys.platform != "win32" or block(server_program) else 1
    if sys.platform == "win32" and not has_rule(server_program):
        print(f"{label} does not start: no rule of the firewall blocks {server_program}; "
              "`with_native_livekit.py --fetch` on an elevated account creates it", flush=True)
        return 1
    chosen = settings(os.environ)
    try:
        webhook_port(chosen["webhook"])
    except native_downloads.Failure as failure:
        print(failure, flush=True)
        return 2
    with tempfile.TemporaryDirectory(prefix="junto-livekit-", ignore_cleanup_errors=True) as temporary:
        root = pathlib.Path(temporary)
        log_path = root / "server.log"
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
            if status != 0 or os.environ.get("JUNTO_TEST_LIVEKIT_LOG") == "1":
                print(f"\nthe log of {label} (exit {status}):", flush=True)
                print(log_path.read_text(errors="replace"), flush=True)
        return status


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
