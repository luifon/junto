#!/usr/bin/env python3
"""Runs a command as one group of processes, and returns only after every process of the group
has ended (ADR 0905). tools/ci/run-jobs.sh runs each CI job with it, so that the caches are
measured and removed, and the run lock is released, only after the processes of the job have
ended.

    group.py <name> <command> [arguments]   runs the command; its exit code, or 128 + the signal
    group.py --end <name>                   ends the group of that name from another process

- On Unix the command starts in a session and process group of its own. When the command ends,
  or when this process receives TERM, INT or HUP, this process sends TERM to the group, sends
  KILL to the processes that remain after KILL_SECONDS, and waits until the group is empty. A
  process that changes its process group (setpgid, as Command::process_group of Rust does, or a
  session of its own) leaves the group and is not ended or awaited. When JUNTO_LOCK_FD names an
  open descriptor (the run lock of tools/windows/lock.py), the command and every process that
  it starts inherit it, so the run lock lasts until the last of them has ended, also when this
  process and its callers are killed, and also for a process that left the group. --end is not
  used on Unix: a signal to this process ends the group.
- On Windows the command runs in a job object named <name> (tools/windows/lock.py), and its
  child processes are in the job from their start. When the command ends, every process of the
  job is ended and awaited. A signal from the Bash of Git for Windows ends a native process at
  once, without its cleanup, so the caller ends the job with --end and then waits for this
  process.
"""
import importlib.util
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

# The time after TERM after which the processes of the group that remain receive KILL.
KILL_SECONDS = 30


def unix(command):
    received = []
    groups = []

    # Installed before the start, so that a signal during the start also ends the group.
    def on_signal(number, _):
        if not received:
            received.append(number)
        for group in groups:
            try:
                os.killpg(group, signal.SIGTERM)
            except ProcessLookupError:
                pass

    for number in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
        signal.signal(number, on_signal)
    process = subprocess.Popen(command, start_new_session=True, pass_fds=lock_descriptor())
    groups.append(process.pid)
    if received:
        on_signal(received[0], None)
    code = process.wait()
    end(process.pid, process)
    if received:
        return 128 + received[0]
    return 128 - code if code < 0 else code


def lock_descriptor():
    """The descriptor of the run lock that the caller holds, to pass to the command."""
    value = os.environ.get("JUNTO_LOCK_FD", "")
    if not value.isdigit():
        return []
    try:
        os.fstat(int(value))
    except OSError:
        return []
    return [int(value)]


def end(group, process):
    """Ends every process of the group and waits until none remains."""
    deadline = None
    while True:
        process.poll()  # the leader is a child of this process: a zombie until it is reaped
        try:
            os.killpg(group, 0)
        except ProcessLookupError:
            return
        now = time.monotonic()
        if deadline is None:
            os.killpg(group, signal.SIGTERM)
            deadline = now + KILL_SECONDS
        elif deadline and now >= deadline:
            print(f"group.py: processes of the group {group} remain {KILL_SECONDS} s after TERM; KILL",
                  file=sys.stderr, flush=True)
            os.killpg(group, signal.SIGKILL)
            deadline = 0
        # No call waits for the end of a process that is not a child of this one.
        time.sleep(0.1)


def lock_module():
    specification = importlib.util.spec_from_file_location(
        "lock", Path(__file__).resolve().parents[1] / "windows" / "lock.py")
    module = importlib.util.module_from_spec(specification)
    specification.loader.exec_module(module)
    return module


def windows(name, command):
    job = lock_module().Job(name)
    process = job.start(command)
    code = process.wait()
    job.end()
    return code


def main(arguments):
    if arguments[:1] == ["--end"] and len(arguments) == 2:
        if os.name != "nt":
            print("group.py: --end is for Windows; on Unix send TERM to group.py", file=sys.stderr)
            return 2
        try:
            job = lock_module().Job.opened(arguments[1])
        except OSError:
            return 0  # no such job: it has ended
        job.end()
        return 0
    if len(arguments) < 2 or arguments[0].startswith("-"):
        print("usage: group.py <name> <command> [arguments] | group.py --end <name>", file=sys.stderr)
        return 2
    try:
        if os.name == "nt":
            return windows(arguments[0], arguments[1:])
        return unix(arguments[1:])
    except OSError as error:
        print(f"group.py: {arguments[1]} did not start: {error}", file=sys.stderr)
        return 127


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
