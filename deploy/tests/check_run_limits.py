"""Every `docker run` of the deployment names the limits of its container (ADR 0015).

A container that a script of the deployment or a command of the install document starts
with `docker run` has no service of a Compose file, so it gets a memory limit, a CPU limit
and a limit of processes only when the command names them.

usage: check_run_limits.py <file> ...
"""

import re
import sys

REQUIRED = ("--memory", "--cpus", "--pids-limit")


def commands(text: str) -> list[tuple[int, str]]:
    """The `docker run` commands of a text, each with the number of its first line."""
    found = []
    lines = text.splitlines()
    index = 0
    while index < len(lines):
        first = index
        command = lines[index]
        while command.rstrip().endswith("\\") and index + 1 < len(lines):
            index += 1
            command = command.rstrip()[:-1] + " " + lines[index]
        index += 1
        if re.search(r"\bdocker run\b", command):
            found.append((first + 1, command))
    return found


def main() -> int:
    problems = []
    for path in sys.argv[1:]:
        with open(path, encoding="utf-8") as file:
            text = file.read()
        for line, command in commands(text):
            options = set(re.findall(r"--[a-z-]+", command))
            missing = [option for option in REQUIRED if option not in options]
            if missing:
                problems.append(f"{path}:{line}: docker run without {', '.join(missing)}")
    for problem in problems:
        print(problem)
    return 1 if problems else 0


if __name__ == "__main__":
    if len(sys.argv) < 2:
        print(__doc__, file=sys.stderr)
        sys.exit(2)
    sys.exit(main())
