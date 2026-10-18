"""The standard output of the server container of a Compose project of the tests.

`docker compose logs` writes the standard output and the standard error of a container into
one stream. Docker reads the two separately, so a log record of standard error can be placed
between two lines of a text that the server wrote to standard output. `docker logs` keeps the
two apart (ADR 0008).
"""

from __future__ import annotations

import re
import subprocess
from pathlib import Path

_OWNER_CODE = re.compile(r"A restart replaces the code\.\s*\n\s*\n\s+(\S+)")


def standard_output(compose: list[str], directory: Path, service: str) -> str:
    """What the container of `service` wrote to standard output, over all its starts."""
    container = subprocess.run(
        [*compose, "ps", "-q", "-a", service],
        cwd=directory,
        capture_output=True,
        text=True,
        timeout=60,
        check=True,
    ).stdout.strip()
    return subprocess.run(
        ["docker", "logs", container],
        capture_output=True,
        text=True,
        timeout=300,
        check=True,
    ).stdout


def newest_owner_code(output: str) -> str | None:
    """The owner code of the last start in the standard output of a server. A restart
    replaces the code, so a code of an earlier start is no longer valid."""
    codes = _OWNER_CODE.findall(output)
    return codes[-1] if codes else None
