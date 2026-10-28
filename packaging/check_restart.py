"""Keep the Linux display alive until the native restart and all its processes end."""
import ctypes
import os
from pathlib import Path
import signal
import subprocess
import sys
import time


def children():
    return [int(pid) for pid in Path(f"/proc/self/task/{os.getpid()}/children").read_text().split()]


def reap():
    while True:
        try:
            pid, status = os.waitpid(-1, os.WNOHANG)
        except ChildProcessError:
            return
        if not pid:
            return
        if status:
            raise AssertionError(f"a restart process {pid} failed with wait status {status}")


def check(arguments, timeout=270):
    # gpui starts its restart helper in another process group. Adopt it when the old
    # client exits so this driver can wait for and clean up the complete handoff.
    libc = ctypes.CDLL(None, use_errno=True)
    if libc.prctl(36, 1, 0, 0, 0):  # PR_SET_CHILD_SUBREAPER
        raise OSError(ctypes.get_errno(), "cannot adopt restart processes")
    def interrupted(number, frame):
        raise SystemExit(128 + number)

    for number in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
        signal.signal(number, interrupted)
    child = subprocess.Popen(arguments)
    deadline = time.monotonic() + timeout
    try:
        while True:
            result = child.poll()
            if result is not None:
                if result:
                    raise AssertionError(f"the installed client exited with {result}")
                reap()
                if not children():
                    return
            if time.monotonic() >= deadline:
                raise AssertionError("the native update and restart did not end within its deadline")
            time.sleep(.05)
    finally:
        deadline = time.monotonic() + 10
        while children():
            for pid in children():
                try:
                    os.kill(pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
            try:
                while os.waitpid(-1, os.WNOHANG)[0]:
                    pass
            except ChildProcessError:
                break
            if time.monotonic() >= deadline:
                raise AssertionError("restart processes did not end after SIGKILL")
            time.sleep(.05)


if __name__ == "__main__":
    check(sys.argv[1:])
