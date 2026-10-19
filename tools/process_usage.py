#!/usr/bin/env python3
"""Reads the memory, the processor time and the threads of the processes of one application
on macOS, from outside, by the method of the test target `modes` of the client (ADR 0914).

Every interval it reads `phys_footprint` and the dirty memory of the categories
"IOAccelerator (graphics)" and "IOSurface" (the pixel buffers that a window draws as
surfaces) of each process with `footprint`, the processor time and the
threads of each process with `ps`, and adds them over the processes. At the end it prints
the highest sum of each memory reading, the processor share over the whole time and over
the busiest interval in percent of one core, and the highest sum of threads.

The processes are those whose executable path starts with the given path, read again at
each interval, so a process that the application starts later is counted from then on.

    python3 tools/process_usage.py /Applications/Example.app/ --seconds 120
"""

import argparse
import re
import subprocess
import sys
import time

INTERVAL = 5.0


def processes(prefix):
    """The process identifiers whose executable path starts with `prefix`."""
    output = subprocess.run(
        ["/bin/ps", "-Ao", "pid=,comm="], capture_output=True, text=True, check=True
    ).stdout
    found = []
    for line in output.splitlines():
        pid, _, path = line.strip().partition(" ")
        if path.strip().startswith(prefix):
            found.append(int(pid))
    return found


def parse_footprint(text):
    """The `phys_footprint`, the graphics memory and the memory of the pixel buffers of each
    process in the output of `footprint -f bytes`, by process identifier."""
    readings = {}
    current = None
    for line in text.splitlines():
        header = re.search(r"\[(\d+)\]: \d+-bit\s+Footprint:", line)
        if header:
            current = int(header.group(1))
            readings[current] = [0, 0, 0]
            continue
        if current is None:
            continue
        stripped = line.strip()
        if stripped.endswith("IOAccelerator (graphics)"):
            readings[current][1] = int(stripped.split()[0])
        elif stripped.endswith("IOSurface"):
            readings[current][2] = int(stripped.split()[0])
        elif stripped.startswith("phys_footprint:"):
            readings[current][0] = int(stripped.split()[1])
    return {pid: tuple(values) for pid, values in readings.items()}


def parse_time(text):
    """A processor time as `ps` states it, `[[days-]hours:]minutes:seconds`, in seconds."""
    text = text.strip()
    days = 0.0
    if "-" in text:
        day_text, text = text.split("-", 1)
        days = float(day_text)
    seconds = 0.0
    for part in text.split(":"):
        seconds = seconds * 60.0 + float(part)
    return days * 86400.0 + seconds


def read(pids):
    """One reading: the sums of footprint, graphics, pixel buffers, processor seconds and
    threads."""
    if not pids:
        return (0, 0, 0, 0.0, 0)
    arguments = ["/usr/bin/footprint", "-f", "bytes"]
    for pid in pids:
        arguments += ["-p", str(pid)]
    output = subprocess.run(arguments, capture_output=True, text=True).stdout
    memory = parse_footprint(output)
    footprint = sum(values[0] for values in memory.values())
    graphics = sum(values[1] for values in memory.values())
    surfaces = sum(values[2] for values in memory.values())
    listed = ",".join(str(pid) for pid in pids)
    times = subprocess.run(
        ["/bin/ps", "-o", "time=", "-p", listed], capture_output=True, text=True
    ).stdout
    processor = sum(parse_time(line) for line in times.splitlines() if line.strip())
    threads = subprocess.run(
        ["/bin/ps", "-M", "-p", listed], capture_output=True, text=True
    ).stdout
    # A line of titles, then one line for each thread; the first line of each process
    # names the process and is also its first thread.
    thread_count = max(0, len(threads.splitlines()) - 1)
    return (footprint, graphics, surfaces, processor, thread_count)


def summary(samples):
    """The highest footprint, graphics, pixel buffers and threads, and the processor share
    over the whole time and over the busiest interval. A sample is (seconds, footprint,
    graphics, pixel buffers, processor seconds, threads)."""
    first, last = samples[0], samples[-1]

    def share(earlier, later):
        return 100.0 * max(0.0, later[4] - earlier[4]) / max(0.001, later[0] - earlier[0])

    return {
        "footprint": max(sample[1] for sample in samples),
        "graphics": max(sample[2] for sample in samples),
        "surfaces": max(sample[3] for sample in samples),
        "processor_average": share(first, last),
        "processor_peak": max(
            (share(earlier, later) for earlier, later in zip(samples, samples[1:])),
            default=0.0,
        ),
        "threads": max(sample[5] for sample in samples),
    }


def main():
    parser = argparse.ArgumentParser(
        description="Reads the memory, the processor time and the threads of the processes of one application."
    )
    parser.add_argument("prefix", help="the start of the executable paths to read")
    parser.add_argument("--seconds", type=float, default=120.0)
    parser.add_argument("--label", default="processes")
    options = parser.parse_args()
    start = time.monotonic()
    samples = []
    while True:
        pids = processes(options.prefix)
        footprint, graphics, surfaces, processor, threads = read(pids)
        at = time.monotonic() - start
        samples.append((at, footprint, graphics, surfaces, processor, threads))
        print(
            f"{options.label}: reading {at:5.0f}s {len(pids)} processes, footprint "
            f"{footprint / 1048576:.1f} MiB, graphics {graphics / 1048576:.1f} MiB, "
            f"pixel buffers {surfaces / 1048576:.1f} MiB, "
            f"processor {processor:.2f}s, {threads} threads",
            flush=True,
        )
        if at >= options.seconds:
            break
        time.sleep(max(0.0, INTERVAL * len(samples) - (time.monotonic() - start)))
    if not samples[-1][1]:
        print(f"{options.label}: no process was read", file=sys.stderr)
        return 1
    result = summary(samples)
    print(
        f"{options.label}: footprint {result['footprint'] / 1048576:.1f} MiB, graphics "
        f"{result['graphics'] / 1048576:.1f} MiB, pixel buffers "
        f"{result['surfaces'] / 1048576:.1f} MiB, processor {result['processor_average']:.1f}% "
        f"on average and {result['processor_peak']:.1f}% over {INTERVAL:.0f} s, "
        f"{result['threads']} threads"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
