#!/bin/bash
# inside a tools container: runs the video benchmark under the headless compositor for each frame size given
for size in "$@"; do
  export SPIKE_VIDEO_SIZE=$size
  bash /work/tools/wl-run.sh "$OUT" c9 > /dev/null 2>&1
  rm -f /work/$OUT/c9-bench.txt
  bash /work/tools/wl.sh click 579 182
  until [ -s /work/$OUT/c9-bench.txt ]; do sleep 2; done
  echo "== frame size $size, load $(cut -d' ' -f1-3 /proc/loadavg)"
  cat /work/$OUT/c9-bench.txt
  cp /work/$OUT/c9-bench.txt /work/$OUT/c9-bench-$size.txt
  grep -E "VmHWM" /proc/$(cat /work/$OUT/pid)/status | tr -s '\t ' ' '
done
