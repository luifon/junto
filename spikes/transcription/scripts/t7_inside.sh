#!/bin/sh
# usage: t7_inside.sh <label> <worker.py arguments...>
label="$1"; shift
/usr/bin/time -v -o "/tmp/time.txt" python worker.py --quiet "$@" --stats "/out/t7_$label.json" | tail -1
echo "$label: $(grep -E 'Maximum resident set size|Elapsed \(wall clock\)' /tmp/time.txt | tr -s ' \t' ' ' | tr '\n' ';') cgroup memory.peak=$(( $(cat /sys/fs/cgroup/memory.peak) / 1048576 )) MiB"
