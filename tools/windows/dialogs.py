#!/usr/bin/env python3
"""The dialogs of the system that the tests of the Windows job leave on the desktop of the
account (ADR 0915).

    python3 tools/windows/dialogs.py record <file>
    python3 tools/windows/dialogs.py check <file>

Windows shows some of its dialogs in a process of the session, PickerHost, for example the
prompt of Windows Defender Firewall when a new program opens a socket on a network interface.
Such a dialog stays until a person closes it, and its dimmed background changes the pictures
that the capture tests read. `record` writes the processes of these programs that run now.
`check` names each process of these programs that runs now and was not recorded, and fails
when there is one. It does not end them: a person at the desktop can have opened one of them
while the tests ran.
"""

import csv
import subprocess
import sys

PROGRAMS = ("PickerHost.exe",)


def parse(output):
    """The (program, process number) pairs of the CSV output of `tasklist /fo csv /nh`. A line
    that is not a row, such as the message that no process matches, is left out."""
    found = set()
    for row in csv.reader(line for line in output.splitlines() if line.startswith('"')):
        if len(row) >= 2 and row[1].isdigit():
            found.add((row[0], int(row[1])))
    return found


def running():
    """The processes of `PROGRAMS` that run now."""
    found = set()
    for program in PROGRAMS:
        output = subprocess.run(
            ["tasklist", "/fo", "csv", "/nh", "/fi", f"imagename eq {program}"],
            capture_output=True, text=True, check=True,
        ).stdout
        found |= {(name, number) for name, number in parse(output) if name.lower() == program.lower()}
    return found


def read(path):
    with open(path, encoding="utf-8") as file:
        return parse(file.read())


def write(path, processes):
    with open(path, "w", encoding="utf-8", newline="") as file:
        writer = csv.writer(file, quoting=csv.QUOTE_ALL, lineterminator="\n")
        for name, number in sorted(processes):
            writer.writerow([name, str(number)])


def left(recorded, now):
    """The processes that run now and were not recorded, in the order of their numbers."""
    return sorted(now - recorded, key=lambda process: process[1])


def main(arguments):
    if len(arguments) != 2 or arguments[0] not in ("record", "check"):
        print("usage: dialogs.py record|check <file>", file=sys.stderr)
        return 2
    command, path = arguments
    if command == "record":
        write(path, running())
        return 0
    new = left(read(path), running())
    for name, number in new:
        print(f"a dialog of the system stays open: {name}, process {number}; a test opened it, "
              "or a person at the desktop did; close it before the next run", file=sys.stderr)
    return 1 if new else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
