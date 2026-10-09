#!/bin/sh
# usage: run.sh <gate> [spike args...]; starts the spike binary and stores its pid in out/pid
set -eu
here="$(cd "$(dirname "$0")/.." && pwd)"
gate="$1"; shift
pkill -x spike 2>/dev/null || true
sleep 0.5
cd "$here"
binary="${SPIKE_BINARY:-$here/target/release/spike}"
"$binary" --gate "$gate" --out "$here/out" "$@" > "$here/out/run-$gate.log" 2>&1 &
echo $! > "$here/out/pid"
sleep "${SPIKE_START_WAIT:-3}"
echo "pid $(cat "$here/out/pid")"
