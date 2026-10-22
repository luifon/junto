"""Expose only V4L2 device nodes to an explicitly requested native camera check."""
import os
from pathlib import Path
import stat
import sys


def command(arguments, nodes):
    if arguments[:2] != ["docker", "run"]:
        raise ValueError("camera devices require a docker run command")
    devices = []
    groups = set()
    for node in sorted(nodes):
        if not node.name.startswith("video") or not node.name[5:].isdigit():
            continue
        info = node.lstat()
        if not stat.S_ISCHR(info.st_mode):
            continue
        devices.extend(["--device", f"{node}:{node}:rw"])
        groups.add(info.st_gid)
    if not groups:
        raise ValueError("no V4L2 device node exists; native camera capture cannot run")
    options = ["-e", "JUNTO_TEST_CAMERA_DEVICE=1", "-e",
               "JUNTO_LINUX_CAMERA_GROUPS=" + ",".join(str(group) for group in sorted(groups))]
    return arguments[:2] + devices + options + arguments[2:]


if __name__ == "__main__":
    try:
        arguments = command(sys.argv[1:], Path("/dev").glob("video[0-9]*"))
    except (OSError, ValueError) as error:
        sys.exit(str(error))
    os.execvp(arguments[0], arguments)
