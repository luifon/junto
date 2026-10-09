"""Samples CPU time of the load generator processes and interface throughput on the Mac."""
import subprocess
import sys
import time

seconds, step, iface, pattern = int(sys.argv[1]), int(sys.argv[2]), sys.argv[3], sys.argv[4]


def cpu_seconds():
    total = 0.0
    out = subprocess.run(["ps", "-axo", "time=,comm="], capture_output=True, text=True).stdout
    for line in out.splitlines():
        parts = line.split(None, 1)
        if len(parts) == 2 and parts[1].rsplit("/", 1)[-1] == pattern:
            fields = parts[0].split(":")
            value = 0.0
            for f in fields:
                value = value * 60 + float(f)
            total += value
    return total


def net_bytes():
    out = subprocess.run(["netstat", "-ibn", "-I", iface], capture_output=True, text=True).stdout
    for line in out.splitlines()[1:]:
        cols = line.split()
        if "<Link#" in line:
            return int(cols[6]), int(cols[9])
    return 0, 0


ncpu = int(subprocess.run(["sysctl", "-n", "hw.ncpu"], capture_output=True, text=True).stdout)
print(f"time gen_cpu%_of_one_core gen_cpu%_of_{ncpu}_cores rx_Mbit tx_Mbit", flush=True)
prev = (time.time(), cpu_seconds(), *net_bytes())
elapsed = 0
while elapsed < seconds:
    time.sleep(step)
    elapsed += step
    cur = (time.time(), cpu_seconds(), *net_bytes())
    dt = cur[0] - prev[0]
    cpu = 100 * (cur[1] - prev[1]) / dt
    print(
        f"{time.strftime('%H:%M:%S')} {cpu:.0f} {cpu / ncpu:.0f} "
        f"{(cur[2] - prev[2]) * 8 / dt / 1e6:.1f} {(cur[3] - prev[3]) * 8 / dt / 1e6:.1f}",
        flush=True,
    )
    prev = cur
