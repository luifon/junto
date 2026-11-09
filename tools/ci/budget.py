#!/usr/bin/env python3
"""The processors and memory of a step of the Linux CI job (ADR 0905):

    budget.py -- <command> [argument]...

runs the command with the budget of one container command, and tools/ci/lanes.py uses
`allocate` for commands that run at the same time.

The base budget is the one that tools/ci/run-jobs.sh gives the job: the build processors from
JUNTO_LINUX_CPU_FIRST (count JUNTO_LINUX_CPUS), the service processors from
JUNTO_LINUX_SERVICE_CPU_FIRST, and JUNTO_LINUX_MEMORY for the build container. On a Docker host of
16 or more processors (JUNTO_CI_HOST_CPUS) the four worker locks of tools/serial.sh each reserve a
pair of processors for a worker (0-1, 2-3, 4-5, 6-7). A step takes every worker lock that is free
at its start, without waiting, and adds the processors of each lock that it took to its build
processors; it keeps the locks until its command and every process of it have ended, so that a
worker that starts meanwhile waits for its lock. Locks are tried only when the host has at least
10 GiB of available memory and 30% of its processor time idle over five seconds, the rules for an
extra worker lock. The build memory then grows by 6 GiB (two Cargo jobs) per pair taken, up to the
available memory less 4 GiB for the other programs of the host and less the services of each
command; when that bound is not above the base memory, the step releases the locks and keeps the
base budget. With every worker lock held, a step has the base budget.

Without the base budget in the environment, or on a host of fewer processors, the command runs
with the environment unchanged. Each decision is printed as one line beginning with "Budget:".
"""
from dataclasses import dataclass, field
import os
import re
import sys
import time

GIB = 1024 ** 3
MIB = 1024 ** 2
# The services of one container command (tools/linux/run.sh): PostgreSQL 1536 MiB, LiveKit
# 768 MiB and the control container 128 MiB.
SERVICES = (1536 + 768 + 128) * MIB
RESERVE = 4 * GIB
PER_PAIR = 6 * GIB
LEAST_AVAILABLE = 10 * GIB
LEAST_IDLE = 0.30
IDLE_SECONDS = 5.0
WORKER_LOCKS = [("/tmp/junto-heavy.lock", 0), ("/tmp/junto-heavy-2.lock", 2),
                ("/tmp/junto-heavy-3.lock", 4), ("/tmp/junto-heavy-4.lock", 6)]
SMALLEST_HOST = 16


def size(text):
    match = re.fullmatch(r"([0-9]+)([bkmg]?)", text.lower())
    if not match:
        raise ValueError(f"not a memory size: {text}")
    count, unit = match.groups()
    return int(count) * 1024 ** "bkmg".index(unit or "b")


def text_of_size(value):
    return f"{value // MIB}m"


def cpu_text(cpus):
    ranges = []
    for cpu in sorted(set(cpus)):
        if ranges and ranges[-1][1] == cpu - 1:
            ranges[-1][1] = cpu
        else:
            ranges.append([cpu, cpu])
    return ",".join(f"{low}" if low == high else f"{low}-{high}" for low, high in ranges)


@dataclass
class Base:
    build: list
    services: list
    memory: int
    host: int


@dataclass
class Budget:
    build: list
    services: list
    memory: int
    locks: list = field(default_factory=list)
    reason: str = ""


def base_from(environment):
    """The base budget of the job, or None when the environment does not give it."""
    try:
        first = int(environment["JUNTO_LINUX_CPU_FIRST"])
        count = int(environment["JUNTO_LINUX_CPUS"])
        service_first = int(environment.get("JUNTO_LINUX_SERVICE_CPU_FIRST", first))
        memory = size(environment["JUNTO_LINUX_MEMORY"])
        host = int(environment.get("JUNTO_CI_HOST_CPUS", "0"))
    except (KeyError, ValueError):
        return None
    return Base(list(range(first, first + count)), list(range(service_first, service_first + count)), memory, host)


def available_memory(meminfo="/proc/meminfo"):
    with open(meminfo, encoding="ascii") as file:
        for line in file:
            if line.startswith("MemAvailable:"):
                return int(line.split()[1]) * 1024
    raise ValueError("no MemAvailable in /proc/meminfo")


def idle_share(seconds=IDLE_SECONDS, stat="/proc/stat"):
    def sample():
        with open(stat, encoding="ascii") as file:
            values = [int(value) for value in file.readline().split()[1:]]
        return values[3] + values[4], sum(values)
    idle_before, total_before = sample()
    time.sleep(seconds)
    idle_after, total_after = sample()
    total = total_after - total_before
    return (idle_after - idle_before) / total if total else 0.0


def worker_locks(environment):
    """The worker locks and the first processor of each: JUNTO_CI_WORKER_LOCKS
    ("path:first path:first") replaces the locks of tools/serial.sh, for the tests."""
    text = environment.get("JUNTO_CI_WORKER_LOCKS")
    if text is None:
        return WORKER_LOCKS
    return [(path, int(first)) for path, first in (item.rsplit(":", 1) for item in text.split())]


def take_free(locks):
    """Takes each lock that no other process holds; returns (descriptor, first processor)."""
    # Here, not at the top: tools/ci/lanes.py reads this module on Windows, which has no fcntl.
    import fcntl

    taken = []
    for path, first in locks:
        try:
            descriptor = os.open(path, os.O_RDWR | os.O_CREAT, 0o666)
        except OSError:
            continue
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            os.close(descriptor)
            continue
        taken.append((descriptor, first))
    return taken


def release(taken):
    for descriptor, _ in taken:
        os.close(descriptor)
    taken.clear()


def decide(base, commands, environment, available=None, idle=None):
    """The budget of a step of `commands` container commands that run at the same time."""
    unchanged = Budget(base.build, base.services, base.memory)
    if base.host < SMALLEST_HOST:
        unchanged.reason = f"the Docker host has {base.host} processors; worker pairs exist on {SMALLEST_HOST} or more"
        return unchanged
    locks = [(path, first) for path, first in worker_locks(environment)
             if not {first, first + 1} & set(base.build + base.services)]
    try:
        available = available_memory() if available is None else available()
    except (OSError, ValueError) as error:
        unchanged.reason = f"the available memory could not be read: {error}"
        return unchanged
    if available < LEAST_AVAILABLE:
        unchanged.reason = f"{available // MIB} MiB available, less than {LEAST_AVAILABLE // MIB} MiB"
        return unchanged
    try:
        idle = idle_share() if idle is None else idle()
    except (OSError, ValueError, IndexError) as error:
        unchanged.reason = f"the idle processor time could not be read: {error}"
        return unchanged
    if idle < LEAST_IDLE:
        unchanged.reason = f"{idle:.0%} of the processor time idle, less than {LEAST_IDLE:.0%}"
        return unchanged
    taken = take_free(locks)
    if not taken:
        unchanged.reason = "every worker lock is held"
        return unchanged
    bound = available - RESERVE - commands * SERVICES
    memory = min(base.memory + PER_PAIR * len(taken), bound)
    if memory <= base.memory:
        release(taken)
        unchanged.reason = f"{available // MIB} MiB available leaves no memory above the base budget"
        return unchanged
    build = sorted(base.build + [cpu for _, first in taken for cpu in (first, first + 1)])
    return Budget(build, base.services, memory, taken,
                  f"{len(taken)} free worker locks, {available // MIB} MiB available, {idle:.0%} idle")


def describe(budget):
    return (f"Budget: build processors {cpu_text(budget.build)}, services {cpu_text(budget.services)}, "
            f"build memory {text_of_size(budget.memory)} ({budget.reason})")


def environment_of(budget, environment):
    changed = dict(environment)
    changed["JUNTO_LINUX_CPU_SET"] = cpu_text(budget.build)
    changed["JUNTO_LINUX_SERVICE_CPU_SET"] = cpu_text(budget.services)
    changed["JUNTO_LINUX_MEMORY"] = text_of_size(budget.memory)
    return changed


def main(arguments, environment):
    if not arguments or arguments[0] != "--" or len(arguments) < 2:
        print("usage: budget.py -- <command> [argument]...", file=sys.stderr)
        return 2
    command = arguments[1:]
    base = base_from(environment)
    if base is None:
        os.execvp(command[0], command)
    budget = decide(base, 1, environment)
    print(describe(budget), flush=True)
    for descriptor, _ in budget.locks:
        os.set_inheritable(descriptor, True)
    os.execvpe(command[0], command, environment_of(budget, environment))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:], dict(os.environ)))
