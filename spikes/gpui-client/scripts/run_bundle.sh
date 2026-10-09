#!/bin/sh
# usage: run_bundle.sh <gate>; starts the bundled application through LaunchServices and stores its pid in out/pid
set -eu
here="$(cd "$(dirname "$0")/.." && pwd)"
gate="$1"
pkill -x spike 2>/dev/null || true
sleep 0.5
open -n "$here/out/JuntoSpike.app" --stderr "$here/out/run-$gate.log" --stdout "$here/out/run-$gate.log" --args --gate "$gate" --out "$here/out"
sleep "${SPIKE_START_WAIT:-3}"
pgrep -n -x spike > "$here/out/pid"
echo "pid $(cat "$here/out/pid")"
