#!/usr/bin/env python3
"""Validate the build and service processor sets against the Docker host (ADR 0905)."""
import sys


def sets(host, wanted, first, service_first):
    if any(not value.isascii() or not value.isdecimal() for value in (host, wanted, first, service_first)):
        raise ValueError("CPU counts and first processors must be non-negative integers")
    host, wanted, first, service_first = map(int, (host, wanted, first, service_first))
    if not host or not wanted:
        raise ValueError("CPU counts must be positive")
    count = min(host, wanted)
    if first + count > host or service_first + count > host:
        raise ValueError(f"CPU set exceeds the Docker host count of {host}")
    return count, f"{first}-{first + count - 1}", f"{service_first}-{service_first + count - 1}"


if __name__ == "__main__":
    try:
        print(*sets(*sys.argv[1:]))
    except ValueError as error:
        print(error, file=sys.stderr)
        sys.exit(2)
