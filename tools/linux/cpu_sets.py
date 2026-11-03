#!/usr/bin/env python3
"""Validate the build and service processor sets against the Docker host (ADR 0905).

    cpu_sets.py <host count> <wanted count> <first build processor> <first service processor>
    cpu_sets.py --sets <host count> <build set> <service set>

The first form gives each set the wanted count from its first processor. The second takes the
two sets as lists of the form of `docker run --cpuset-cpus`, such as 0-7,8-11. Both print the
processor count of the build set and the two sets."""
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


def processors(text):
    """The processors of a list such as 0-3,8,10-11, in order and without repetition."""
    found = []
    for part in text.split(","):
        low, _, high = part.partition("-")
        if not low.isascii() or not low.isdecimal() or (high and (not high.isascii() or not high.isdecimal())):
            raise ValueError(f"not a CPU set: {text}")
        low, high = int(low), int(high or low)
        if high < low:
            raise ValueError(f"not a CPU set: {text}")
        found.extend(cpu for cpu in range(low, high + 1) if cpu not in found)
    return found


def text_of(cpus):
    """The shortest list of ranges of a set of processors."""
    ranges = []
    for cpu in sorted(set(cpus)):
        if ranges and ranges[-1][1] == cpu - 1:
            ranges[-1][1] = cpu
        else:
            ranges.append([cpu, cpu])
    return ",".join(f"{low}" if low == high else f"{low}-{high}" for low, high in ranges)


def explicit(host, build, service):
    if not host.isascii() or not host.isdecimal() or not int(host):
        raise ValueError("the CPU count of the host must be a positive integer")
    host = int(host)
    build_cpus, service_cpus = processors(build), processors(service)
    if max(build_cpus + service_cpus) >= host:
        raise ValueError(f"CPU set exceeds the Docker host count of {host}")
    return len(build_cpus), text_of(build_cpus), text_of(service_cpus)


if __name__ == "__main__":
    try:
        if sys.argv[1:2] == ["--sets"]:
            print(*explicit(*sys.argv[2:]))
        else:
            print(*sets(*sys.argv[1:]))
    except (TypeError, ValueError) as error:
        print(error, file=sys.stderr)
        sys.exit(2)
