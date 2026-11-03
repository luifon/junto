#!/usr/bin/env python3
"""Runs the steps of the Linux CI job in lanes that run at the same time (ADR 0905):

    lanes.py <plan file> <failures file>

The plan names lanes and their steps, one item per line:

    lane <name>
    step <name of the step>
    <command>

A command is split into words as the shell splits them, without expansion. The steps of one lane
run one after the other, in their order; every step runs also after a failed step. The lanes run
at the same time when the budget of the step (tools/ci/budget.py) gives each lane at least
LANE_PROCESSORS build processors and LANE_MEMORY of build memory; otherwise one after the other
with the base budget. Each lane gets an equal part of the build processors and of the build
memory, the service processors of the job, and its own containers and network
(JUNTO_LINUX_INSTANCE=<lane name>) on the image that an earlier step built
(JUNTO_LINUX_IMAGE_BUILT=1), and a lock of tools/serial.sh of its own.

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

LANE_PROCESSORS = 2
LANE_MEMORY = 5 * budget.GIB


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
            if not rest or not all(c.isascii() and (c.islower() or c.isdigit() or c == "-") for c in rest):
                raise ValueError(f"line {number}: a lane name has only a-z, 0-9 and -: {rest}")
            if rest in lanes:
                raise ValueError(f"line {number}: the lane {rest} is named twice")
            lane, step = rest, None
            lanes.append(lane)
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


def split(cpus, parts):
    """Splits the processors into `parts` contiguous groups of nearly equal size."""
    size, extra = divmod(len(cpus), parts)
    groups, start = [], 0
    for index in range(parts):
        end = start + size + (1 if index < extra else 0)
        groups.append(cpus[start:end])
        start = end
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
    groups = split(allocation.build, len(lanes)) if allocation else None
    for index, lane in enumerate(lanes):
        lane_environment = dict(environment, JUNTO_LINUX_INSTANCE=lane, JUNTO_LINUX_IMAGE_BUILT="1")
        if "JUNTO_HEAVY_LOCK" in environment:
            lane_environment["JUNTO_HEAVY_LOCK"] = f"{environment['JUNTO_HEAVY_LOCK']}-{lane}"
        if allocation:
            share = budget.Budget(groups[index], allocation.services, allocation.memory // len(lanes))
            lane_environment = budget.environment_of(share, lane_environment)
        result[lane] = lane_environment
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
        allocation = (decide or budget.decide)(base, len(lanes), environment)
        print(budget.describe(allocation), flush=True)
        together = (len(allocation.build) // len(lanes) >= LANE_PROCESSORS
                    and allocation.memory // len(lanes) >= LANE_MEMORY)
        if not together:
            budget.release(allocation.locks)
            allocation = None
    print(f"Lanes: {', '.join(lanes)}, {'at the same time' if together else 'one after the other'}", flush=True)
    environments = lane_environments(lanes, environment, allocation)
    with tempfile.TemporaryDirectory(prefix="junto-ci-lanes-") as directory:
        threads = []
        for lane in lanes:
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
