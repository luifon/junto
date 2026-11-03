#!/usr/bin/env python3
"""Runs the steps of the Linux CI job in lanes that run at the same time (ADR 0905):

    lanes.py <plan file> <failures file>

The plan names lanes and their steps, one item per line:

    lane <name> [share <n>] [memory <size>] [without services]
    step <name of the step>
    <command>

A command is split into words as the shell splits them, without expansion. The steps of one lane
run one after the other, in their order; every step runs also after a failed step. A lane needs
the memory that it names (LANE_MEMORY when it names none) and, unless it is "without services",
the PostgreSQL, LiveKit and control containers of tools/linux/run.sh. The lanes run at the same
time when the budget of the step (tools/ci/budget.py, for the lanes with services) is above the
base budget and gives the lanes together their memory and each lane at least one build
processor; otherwise one after the other with the base budget. At the same time, each lane gets
a part of the build processors in proportion to its share (1 when it names none), its memory and
an equal part of the rest of the build memory, one Cargo job (JUNTO_LINUX_CARGO_JOBS=1: the lanes run programs that an earlier
step built), the service processors of the job, and its own containers and network
(JUNTO_LINUX_INSTANCE=<lane name>). In both cases a lane runs on the image that an earlier step built (JUNTO_LINUX_IMAGE_BUILT=1), with
a lock of tools/serial.sh of its own, and a lane without services sets JUNTO_LINUX_SERVICES=none.

The output of each step goes to a file. After the last lane has ended, the output of each step is
printed in the order of the plan, under a line with the step, its lane, its start and end (UTC)
and its duration, and then the table of the steps. Each failed command is appended to the
failures file as "<step> (exit <status>): <command>", and the exit status is that of the first
failed command in the order of the plan.
"""
import datetime
import os
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile
import threading

sys.path.insert(0, str(Path(__file__).resolve().parent))
import budget  # noqa: E402

LANE_MEMORY = 4 * budget.GIB


class Lane:
    def __init__(self, name, memory=LANE_MEMORY, services=True, share=1):
        self.name, self.memory, self.services, self.share = name, memory, services, share


class Step:
    def __init__(self, lane, name):
        self.lane, self.name, self.commands = lane, name, []
        self.start = self.end = None
        self.failures = []
        self.output = None


def parse(text):
    lanes, steps, lane, step = [], [], None, None
    for number, line in enumerate(text.splitlines(), 1):
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        word, _, rest = line.partition(" ")
        if word == "lane":
            if not rest:
                raise ValueError(f"line {number}: a lane needs a name")
            name, *options = rest.split()
            if not name or not all(c.isascii() and (c.islower() or c.isdigit() or c == "-") for c in name):
                raise ValueError(f"line {number}: a lane name has only a-z, 0-9 and -: {name}")
            if name in [known.name for known in lanes]:
                raise ValueError(f"line {number}: the lane {name} is named twice")
            declared = Lane(name)
            while options:
                if options[:2] == ["without", "services"]:
                    declared.services, options = False, options[2:]
                elif options[0] == "share" and len(options) > 1 and options[1].isascii() \
                        and options[1].isdecimal() and int(options[1]) > 0:
                    declared.share, options = int(options[1]), options[2:]
                elif options[0] == "memory" and len(options) > 1:
                    declared.memory, options = budget.size(options[1]), options[2:]
                else:
                    raise ValueError(f"line {number}: not an option of a lane: {' '.join(options)}")
            lane, step = name, None
            lanes.append(declared)
        elif word == "step":
            if lane is None or not rest:
                raise ValueError(f"line {number}: a step needs a lane before it and a name")
            step = Step(lane, rest)
            steps.append(step)
        else:
            if step is None:
                raise ValueError(f"line {number}: a command needs a step before it")
            step.commands.append(shlex.split(line))
    if not steps:
        raise ValueError("the plan has no step")
    return lanes, steps


def split(cpus, shares):
    """Splits the processors into contiguous groups in proportion to the shares, each group at
    least one processor; the processors left by rounding go to the largest remainders."""
    total = sum(shares)
    counts = [max(1, len(cpus) * share // total) for share in shares]
    remainders = sorted(range(len(shares)), key=lambda i: -(len(cpus) * shares[i] % total))
    while sum(counts) > len(cpus):
        counts[counts.index(max(counts))] -= 1
    index = 0
    while sum(counts) < len(cpus):
        counts[remainders[index % len(shares)]] += 1
        index += 1
    groups, start = [], 0
    for count in counts:
        groups.append(cpus[start:start + count])
        start += count
    return groups


def now():
    return datetime.datetime.now(datetime.timezone.utc)


def stamp(moment):
    return moment.strftime("%Y-%m-%dT%H:%M:%SZ")


def duration(start, end):
    seconds = int((end - start).total_seconds())
    return f"{seconds // 60}m{seconds % 60:02d}s"


def lane_environments(lanes, environment, allocation):
    """The environment of each lane: its instance, its lock, and its part of the budget."""
    result = {}
    groups = split(allocation.build, [lane.share for lane in lanes]) if allocation else None
    rest = allocation.memory - sum(lane.memory for lane in lanes) if allocation else 0
    for index, lane in enumerate(lanes):
        lane_environment = dict(environment, JUNTO_LINUX_INSTANCE=lane.name, JUNTO_LINUX_IMAGE_BUILT="1")
        if not lane.services:
            lane_environment["JUNTO_LINUX_SERVICES"] = "none"
        if "JUNTO_HEAVY_LOCK" in environment:
            lane_environment["JUNTO_HEAVY_LOCK"] = f"{environment['JUNTO_HEAVY_LOCK']}-{lane.name}"
        if allocation:
            share = budget.Budget(groups[index], allocation.services, lane.memory + rest // len(lanes))
            lane_environment = budget.environment_of(share, lane_environment)
            lane_environment["JUNTO_LINUX_CARGO_JOBS"] = "1"
        result[lane.name] = lane_environment
    return result


def run_lane(steps, environment, directory, root):
    for step in steps:
        step.start = now()
        path = Path(directory) / f"{len(list(Path(directory).iterdir())):03d}.log"
        with open(path, "wb") as output:
            for command in step.commands:
                status = subprocess.run(command, env=environment, cwd=root, stdout=output,
                                        stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL).returncode
                if status != 0:
                    step.failures.append((status, shlex.join(command)))
        step.end = now()
        step.output = path


def run(plan, failures_file, environment, root, decide=None):
    lanes, steps = parse(plan)
    base = budget.base_from(environment)
    allocation = None
    together = False
    if base is not None and len(lanes) > 1:
        allocation = (decide or budget.decide)(base, sum(lane.services for lane in lanes), environment)
        print(budget.describe(allocation), flush=True)
        # The base budget runs the lanes one after the other, as the job ran them before.
        together = (allocation.memory > base.memory and len(allocation.build) >= len(lanes)
                    and allocation.memory >= sum(lane.memory for lane in lanes))
        if not together:
            budget.release(allocation.locks)
            allocation = None
    names = [lane.name for lane in lanes]
    print(f"Lanes: {', '.join(names)}, {'at the same time' if together else 'one after the other'}", flush=True)
    environments = lane_environments(lanes, environment, allocation)
    with tempfile.TemporaryDirectory(prefix="junto-ci-lanes-") as directory:
        threads = []
        for lane in names:
            lane_directory = Path(directory) / lane
            lane_directory.mkdir()
            work = threading.Thread(target=run_lane, args=([s for s in steps if s.lane == lane],
                                                           environments[lane], lane_directory, root))
            threads.append(work)
            work.start()
            if not together:
                work.join()
        for work in threads:
            work.join()
        if allocation:
            budget.release(allocation.locks)
        first = 0
        with open(failures_file, "a", encoding="utf-8") as failures:
            for step in steps:
                print(f"\n=== {step.name}  lane {step.lane}  {stamp(step.start)} to {stamp(step.end)}  "
                      f"{duration(step.start, step.end)}", flush=True)
                sys.stdout.buffer.write(step.output.read_bytes())
                sys.stdout.flush()
                for status, command in step.failures:
                    failures.write(f"{step.name} (exit {status}): {command}\n")
                    first = first or status
        print("\n=== Lanes", flush=True)
        for step in steps:
            print(f"{step.lane:<8} {stamp(step.start)}  {stamp(step.end)}  {duration(step.start, step.end):>8}  "
                  f"{'failed' if step.failures else 'passed'}  {step.name}", flush=True)
    return first


if __name__ == "__main__":
    if len(sys.argv) != 3:
        print("usage: lanes.py <plan file> <failures file>", file=sys.stderr)
        sys.exit(2)
    try:
        text = Path(sys.argv[1]).read_text(encoding="utf-8")
        sys.exit(run(text, sys.argv[2], dict(os.environ), os.getcwd()))
    except ValueError as error:
        print(error, file=sys.stderr)
        sys.exit(2)
