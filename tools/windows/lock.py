"""Runs a command while this process holds an exclusive lock of a file, and returns the exit
code of the command (127 when it cannot start). tools/serial.sh uses it on systems that have
neither `lockf` nor `flock`, such as the Bash of Git for Windows.

    python3 tools/windows/lock.py <lock file> <command> [arguments]

A second caller waits until the first has ended. The system releases the lock when the process
that holds it ends, also when it is killed, so a run that ended without its cleanup leaves no
lock behind.
"""

import os
import subprocess
import sys


def hold(file):
    """Waits until this process holds the exclusive lock of the first byte of `file`."""
    if os.name == "nt":
        import ctypes
        from ctypes import wintypes
        import msvcrt

        class Overlapped(ctypes.Structure):
            _fields_ = [("internal", ctypes.c_size_t), ("internal_high", ctypes.c_size_t),
                        ("offset", wintypes.DWORD), ("offset_high", wintypes.DWORD),
                        ("event", wintypes.HANDLE)]

        lock_file = ctypes.WinDLL("kernel32", use_last_error=True).LockFileEx
        lock_file.argtypes = [wintypes.HANDLE, wintypes.DWORD, wintypes.DWORD, wintypes.DWORD,
                              wintypes.DWORD, ctypes.POINTER(Overlapped)]
        lock_file.restype = wintypes.BOOL
        exclusive = 0x2  # LOCKFILE_EXCLUSIVE_LOCK; without LOCKFILE_FAIL_IMMEDIATELY it waits.
        if not lock_file(msvcrt.get_osfhandle(file.fileno()), exclusive, 0, 1, 0, ctypes.byref(Overlapped())):
            raise ctypes.WinError(ctypes.get_last_error())
    else:
        import fcntl

        fcntl.flock(file.fileno(), fcntl.LOCK_EX)


def main(arguments):
    if len(arguments) < 2:
        print("usage: lock.py <lock file> <command> [arguments]", file=sys.stderr)
        return 2
    with open(arguments[0], "a+b") as file:
        hold(file)
        try:
            return subprocess.run(arguments[1:]).returncode
        except OSError as error:
            print(f"lock.py: {arguments[1]} did not start: {error}", file=sys.stderr)
            return 127


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
