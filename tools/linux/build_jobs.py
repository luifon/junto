#!/usr/bin/env python3
"""Bound Cargo concurrency by both processors and the container memory limit (ADR 0905)."""
import re
import sys

GIB = 1024 ** 3


def jobs(processors, memory):
    match = re.fullmatch(r"([0-9]+)([bkmg]?)", memory.lower())
    if not match:
        raise ValueError("JUNTO_LINUX_MEMORY must be an integer byte count or use b, k, m or g")
    count, unit = match.groups()
    size = int(count) * 1024 ** ("bkmg".index(unit or "b"))
    available = (size - 2 * GIB) // (3 * GIB)
    if available < 1:
        raise ValueError("JUNTO_LINUX_MEMORY needs at least 5 GiB for one build job")
    return min(int(processors), available)


if __name__ == "__main__":
    try:
        print(jobs(*sys.argv[1:]))
    except ValueError as error:
        print(error, file=sys.stderr)
        sys.exit(2)
