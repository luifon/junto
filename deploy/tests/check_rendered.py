"""Rules for the rendered Compose configuration of one form (ADR 0015).

usage: check_rendered.py <form> <file with the output of `docker compose config --format json`>
"""

import json
import sys

# Every published port of each form: (service, container port, protocol, host address).
# None means every address of the host. A form publishes exactly these, each once.
PUBLISHED = {
    "local": [
        ("livekit", 7880, "tcp", "127.0.0.1"),
        ("livekit", 7881, "tcp", "127.0.0.1"),
        ("livekit", 7882, "udp", "127.0.0.1"),
        ("server", 8080, "tcp", "127.0.0.1"),
    ],
    "one-host": [],
    "two-hosts-data": [],
    "two-hosts-public": [("link", 51820, "udp", None)],
}
# The containers of each network that is not "database" or "backend".
OTHER_NETWORKS = {
    "local": {},
    "one-host": {},
    "two-hosts-data": {},
    "two-hosts-public": {"inbound": {"link"}},
}
HOST_NETWORK = {
    "local": set(),
    "one-host": {"livekit", "entry"},
    "two-hosts-data": set(),
    "two-hosts-public": {"livekit", "entry"},
}
SERVICES = {
    "local": {"init", "postgres", "server", "redis", "livekit", "recorder", "worker"},
    "one-host": {"init", "postgres", "server", "redis", "livekit", "recorder", "worker", "entry"},
    "two-hosts-data": {"init", "postgres", "server", "recorder", "worker", "link"},
    "two-hosts-public": {"redis", "livekit", "entry", "link"},
}
BUILT = {"init", "server", "worker", "link"}
# Services that run as root, each with the capabilities that it may add.
ROOT = {
    "init": {"CHOWN", "FOWNER"},
    "entry": {"NET_BIND_SERVICE"},
    "link": {"NET_ADMIN", "SETUID", "SETGID"},
    # The loop of the worker container; a job runs as the user 10001 (ADR 0016).
    "worker": {"SETUID", "SETGID", "KILL"},
}
# The server image names its user itself, and that user is not root (ADR 0014).
USER_OF_IMAGE = {"server"}
# The browser and the sound server of the recorder write below its root (ADR 0020).
WRITABLE_ROOT = {"recorder"}
NO_HEALTH_CHECK = {"init", "worker"}
DATABASE_NETWORK = {"postgres", "server"}
# The directories of the server that are volumes, by the setting that names each one. The
# spool has no setting: the hand-over script names it.
SERVER_DIRECTORIES = {
    "JUNTO_ATTACHMENTS_DIR": "/var/lib/junto/attachments",
    "JUNTO_RECORDINGS_DIR": "/var/lib/junto/recordings",
    "JUNTO_EXPORTS_DIR": "/var/lib/junto/exports",
}


def check(form: str, project: dict) -> list[str]:
    problems: list[str] = []
    services = project["services"]
    if set(services) != SERVICES[form]:
        problems.append(f"services are {sorted(services)}, expected {sorted(SERVICES[form])}")

    published = []
    members: dict[str, set[str]] = {}
    for name, service in services.items():
        def problem(text: str, name: str = name) -> None:
            problems.append(f"{name}: {text}")

        for port in service.get("ports", []):
            published.append((name, int(port["target"]), port.get("protocol", "tcp"), port.get("host_ip")))
        if (service.get("network_mode") == "host") != (name in HOST_NETWORK[form]):
            problem("wrong use of the network of the host")

        image = service.get("image", "")
        if name in BUILT:
            if "build" not in service:
                problem("is not built from the repository")
        elif "@sha256:" not in image:
            problem(f"image {image!r} is not pinned by digest")

        for key in ("mem_limit", "cpus", "pids_limit"):
            if not service.get(key):
                problem(f"has no {key}")
        logging = service.get("logging", {})
        if logging.get("driver") != "json-file" or not logging.get("options", {}).get("max-size"):
            problem("has no log limit")
        expected_restart = "no" if name == "init" else "unless-stopped"
        if service.get("restart", "no") != expected_restart:
            problem(f"restart rule is {service.get('restart')!r}")
        if name not in NO_HEALTH_CHECK and "healthcheck" not in service and name != "server":
            problem("has no health check")

        if service.get("privileged"):
            problem("is privileged")
        added = set(service.get("cap_add", []))
        if service.get("cap_drop") != ["ALL"]:
            problem("does not drop every capability")
        if "no-new-privileges:true" not in service.get("security_opt", []):
            problem("may gain new privileges")
        if added != ROOT.get(name, set()):
            problem(f"adds the capabilities {sorted(added)}")
        user = str(service.get("user", ""))
        named = user.split(":")[0].strip()
        is_root = named in ("", "root") or named.lstrip("+-").strip("0") == "" and named.lstrip("+-") != ""
        if name in USER_OF_IMAGE and user == "":
            is_root = False
        if is_root != (name in ROOT):
            problem(f"user is {user!r}")
        if bool(service.get("read_only")) == (name in WRITABLE_ROOT):
            problem("wrong use of a read-only file system")

        networks = set(service.get("networks", {}) or {})
        if ("database" in networks) != (name in DATABASE_NETWORK and "postgres" in services):
            problem("wrong use of the network of the database")
        for network in networks - {"database", "backend"}:
            members.setdefault(network, set()).add(name)
        if name == "worker" and service.get("network_mode") != "none":
            problem("has a network")
        if name == "worker" and service.get("environment", {}).get("JUNTO_WORKER_JOB_USER") != "10001:10001":
            problem("does not run its jobs as the user 10001")
        for volume in service.get("volumes", []):
            if volume.get("source") == "/var/run/docker.sock" or volume.get("target") == "/var/run/docker.sock":
                problem("mounts the Docker socket")

    if "server" in services:
        server = services["server"]
        mounted = {volume.get("target") for volume in server.get("volumes", []) if volume.get("type") == "volume"}
        for setting, directory in SERVER_DIRECTORIES.items():
            if server.get("environment", {}).get(setting) != directory:
                problems.append(f"server: {setting} is not {directory}")
            if directory not in mounted:
                problems.append(f"server: {directory} is not a volume")
        if server.get("environment", {}).get("JUNTO_MIGRATION_COPY") != "database":
            problems.append("server: the copy before a migration is not on by default")
    # init sets the owner of every directory that a container without root rights gets as
    # a volume: it mounts the same volume at the same place and names that place with an
    # owner that is not root.
    if "init" in services:
        init = services["init"]
        mounted = {(volume.get("source"), volume.get("target")) for volume in init.get("volumes", []) if volume.get("type") == "volume"}
        prepared = {entry.split(":")[0] for entry in init.get("command") or [] if entry.split(":")[1:2] not in ([], ["0"], ["root"])}
        for name, service in services.items():
            if name in ROOT:
                continue
            for volume in service.get("volumes", []):
                if volume.get("type") != "volume":
                    continue
                place = (volume.get("source"), volume.get("target"))
                if place not in mounted or volume.get("target") not in prepared:
                    problems.append(f"{name}: init does not prepare the volume {place[0]} at {place[1]}")

    if "database" in project.get("networks", {}) and not project["networks"]["database"].get("internal"):
        problems.append("the network of the database is not internal")
    if members != OTHER_NETWORKS[form]:
        problems.append(f"other networks have the containers {members}, expected {OTHER_NETWORKS[form]}")
    if sorted(published, key=repr) != sorted(PUBLISHED[form], key=repr):
        problems.append(f"published ports are {published}, expected {PUBLISHED[form]}")
    return problems


def main() -> int:
    form, path = sys.argv[1], sys.argv[2]
    with open(path, encoding="utf-8") as file:
        project = json.load(file)
    problems = check(form, project)
    for text in problems:
        print(f"      {form}: {text}")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
