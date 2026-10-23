#!/usr/bin/env python3
"""Runs one call of a program and holds its output after a number of bytes (ADR 0910).

usage: hold_output.py <bytes> <held file> <go file> <seconds> <program> <argument>...

The recording test puts this in front of `docker` for the archive step of deploy/backup.sh.
The call writes the archive to its standard output. This program passes the first bytes on,
creates the held file, and reads nothing more until the go file exists. A program that
writes into a full pipe waits, so `tar` stops inside the directories that it archives while
the test acts. Then the rest of the output is passed on, and this program ends with the exit
code of the call. Standard input and standard error go to the call unchanged.
"""

from __future__ import annotations

import subprocess
import sys
import time
from pathlib import Path


def main() -> int:
    count, held, go, seconds = int(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3]), float(sys.argv[4])
    call = subprocess.Popen(sys.argv[5:], stdout=subprocess.PIPE)
    assert call.stdout is not None
    output = sys.stdout.buffer
    left = count
    while left > 0:
        part = call.stdout.read(min(left, 65536))
        if not part:
            break
        output.write(part)
        left -= len(part)
    output.flush()
    held.touch()
    deadline = time.monotonic() + seconds
    while not go.exists() and time.monotonic() < deadline:
        time.sleep(0.2)
    while part := call.stdout.read(65536):
        output.write(part)
    output.flush()
    return call.wait()


if __name__ == "__main__":
    sys.exit(main())
