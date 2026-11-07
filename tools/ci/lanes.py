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

With JUNTO_CI_LANES=machine (the Windows job), the lanes run on the machine itself: a
lane starts, in the order of the plan, when its memory and that of the lanes that run fit into
the memory that the machine has available at the start, less RESERVE, which the desktop of the
machine keeps; a lane that does not fit even alone runs when no other lane runs. Each lane gets
CARGO_BUILD_JOBS, its part of the processors of the machine in proportion to its share, for a
step that compiles; the test programs keep their own number of threads.

A command that writes nothing for 20 minutes (JUNTO_CI_SILENCE seconds) is ended: in the Linux
job the threads of the test programs in the build container of its lane are first written to
its output, and its step fails with the status 124.

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
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))
import budget  # noqa: E402

LANE_MEMORY = 4 * budget.GIB
# The time that a command of a step may write nothing. A command that is silent for longer is
# ended after the threads of its test programs are written to its output, so a test that waits
# forever fails its step within the bound and does not hold the job (ADR 0905).
# JUNTO_CI_SILENCE sets another number of seconds.
SILENCE = 20 * 60
# The threads of the test programs in the build container of a lane: every process whose
# program is a test program of Cargo (`deps/` of the target directory), with gdb.
THREADS = r"""for p in /proc/[0-9]*; do
    program=$(readlink "$p/exe" 2>/dev/null) || continue
    case "$program" in */deps/*) ;; *) continue ;; esac
    echo "--- process ${p#/proc/}: $(tr '\0' ' ' < "$p/cmdline")"
    timeout 120 gdb -p "${p#/proc/}" -batch -ex "thread apply all bt 40" 2>&1
done"""
# The memory that the lanes on the machine itself leave to the desktop and the other programs.
RESERVE = 4 * budget.GIB


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


def silence_of(environment):
    value = environment.get("JUNTO_CI_SILENCE", "")
    return int(value) if value.isascii() and value.isdecimal() and int(value) > 0 else SILENCE


def threads_command(environment):
    """The command that writes the threads of the test programs of a lane, or None where the
    job cannot read them. On the machine itself (the Windows job) there is no such reader; in
    the Linux job it runs in the build container of the lane (tools/linux/run.sh names it), with
    the privileges that gdb needs to attach to a process that is not its child."""
    if environment.get("JUNTO_CI_LANES") == "machine":
        return None
    prefix = environment.get("JUNTO_LINUX_PREFIX") or "junto-linux"
    instance = environment.get("JUNTO_LINUX_INSTANCE")
    container = f"{prefix}-{instance}-build" if instance else f"{prefix}-build"
    return ["docker", "exec", "--privileged", "--user", "0", container, "sh", "-c", THREADS]


def end(process):
    """Ends a command and every process that it started."""
    if os.name == "nt":
        subprocess.run(["taskkill", "/T", "/F", "/PID", str(process.pid)],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    else:
        import signal
        for sent, wait in ((signal.SIGTERM, 30), (signal.SIGKILL, 30)):
            try:
                os.killpg(process.pid, sent)
            except ProcessLookupError:
                break
            try:
                process.wait(timeout=wait)
                break
            except subprocess.TimeoutExpired:
                continue
    process.wait()


def run_command(command, environment, root, output):
    """Runs a command with its output in the file `output`, and returns its exit status. A
    command that writes nothing for the silence bound gets the threads of its test programs
    written to its output and is ended, with the status 124."""
    bound = silence_of(environment)
    process = subprocess.Popen(command, env=environment, cwd=root, stdout=output, stderr=subprocess.STDOUT,
                               stdin=subprocess.DEVNULL, start_new_session=os.name != "nt")
    interval = min(5.0, bound / 4)
    written, since = -1, time.monotonic()
    while True:
        try:
            return process.wait(timeout=interval)
        except subprocess.TimeoutExpired:
            pass
        size = os.fstat(output.fileno()).st_size
        if size != written:
            written, since = size, time.monotonic()
        elif time.monotonic() - since >= bound:
            break
    output.write(f"\n=== {shlex.join(command)} wrote nothing for {bound} s; the threads of its test "
                 f"programs follow, and then it is ended\n".encode())
    output.flush()
    reader = threads_command(environment)
    if reader is None:
        output.write(b"(the threads of a program are read only in the build container of the Linux job)\n")
        output.flush()
    else:
        try:
            subprocess.run(reader, env=environment, stdout=output, stderr=subprocess.STDOUT,
                           stdin=subprocess.DEVNULL, timeout=600)
        except (OSError, subprocess.TimeoutExpired) as error:
            output.write(f"the threads could not be read: {error}\n".encode())
            output.flush()
    end(process)
    output.write(f"=== ended after {bound} s without output\n".encode())
    output.flush()
    return 124


def run_lane(steps, environment, directory, root):
    for step in steps:
        step.start = now()
        path = Path(directory) / f"{len(list(Path(directory).iterdir())):03d}.log"
        with open(path, "wb") as output:
            for command in step.commands:
                try:
                    status = run_command(command, environment, root, output)
                except OSError as error:
                    output.write(f"{shlex.join(command)}: {error}\n".encode())
                    output.flush()
                    status = 127
                if status != 0:
                    step.failures.append((status, shlex.join(command)))
        step.end = now()
        step.output = path


def machine_memory():
    """The memory that is available on this machine now, in bytes."""
    if sys.platform != "win32":
        return budget.available_memory()
    import ctypes

    class Status(ctypes.Structure):
        _fields_ = [("length", ctypes.c_uint32), ("load", ctypes.c_uint32)] + [
            (name, ctypes.c_uint64) for name in ("total", "available", "total_page", "available_page",
                                                 "total_virtual", "available_virtual", "extended")]

    status = Status()
    status.length = ctypes.sizeof(Status)
    if not ctypes.windll.kernel32.GlobalMemoryStatusEx(ctypes.byref(status)):
        raise OSError("GlobalMemoryStatusEx failed")
    return status.available


def run_on_machine(lanes, steps, environment, directory, root, machine):
    """Runs the lanes on this machine: each when its memory fits beside the lanes that run."""
    processors, available = machine
    room = max(0, available - RESERVE)
    total = sum(lane.share for lane in lanes)
    print(f"Machine: {processors} processors, {budget.text_of_size(available)} available, "
          f"{budget.text_of_size(room)} for the lanes", flush=True)
    names = [lane.name for lane in lanes]
    print(f"Lanes: {', '.join(names)}, each when its memory fits", flush=True)
    ended = threading.Condition()
    running = {}

    def lane_run(lane, lane_environment, lane_directory):
        try:
            run_lane([s for s in steps if s.lane == lane.name], lane_environment, lane_directory, root)
        finally:
            with ended:
                del running[lane.name]
                ended.notify()

    waiting = list(lanes)
    threads = []
    with ended:
        while waiting or running:
            used = sum(lane.memory for lane in lanes if lane.name in running)
            fits = [lane for lane in waiting if not running or used + lane.memory <= room]
            for lane in fits[:1]:
                waiting.remove(lane)
                jobs = max(1, processors * lane.share // total)
                lane_environment = dict(environment, CARGO_BUILD_JOBS=str(jobs))
                lane_directory = Path(directory) / lane.name
                lane_directory.mkdir()
                running[lane.name] = True
                print(f"{stamp(now())} lane {lane.name} starts: {budget.text_of_size(lane.memory)}, "
                      f"{jobs} Cargo jobs", flush=True)
                work = threading.Thread(target=lane_run, args=(lane, lane_environment, lane_directory))
                threads.append(work)
                work.start()
            if not fits:
                ended.wait()
    for work in threads:
        work.join()


def run(plan, failures_file, environment, root, decide=None, machine=None):
    lanes, steps = parse(plan)
    base = budget.base_from(environment)
    if environment.get("JUNTO_CI_LANES") == "machine" and len(lanes) > 1:
        machine = machine or (os.cpu_count() or 1, machine_memory())
        with tempfile.TemporaryDirectory(prefix="junto-ci-lanes-") as directory:
            run_on_machine(lanes, steps, environment, directory, root, machine)
            return report(steps, failures_file)
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
        return report(steps, failures_file)


def report(steps, failures_file):
    """Prints the output of each step in the order of the plan and the table of the steps,
    appends each failed command to the failures file, and returns the first failed status."""
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
