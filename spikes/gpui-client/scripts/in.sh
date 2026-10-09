#!/bin/sh
# usage: in.sh <scroll|drag|move|rightclick> <values...>; posts pointer events to the spike process in window coordinates. JUNTO_TEST_AUTOMATION_CLI is the command of the desktop automation tool.
set -eu
automation="${JUNTO_TEST_AUTOMATION_CLI:?set the command of the desktop automation tool}"
here="$(cd "$(dirname "$0")/.." && pwd)"
pid="${SPIKE_PID:-$(cat "$here/out/pid")}"
origin="$($automation list-windows --app "pid:$pid" --json | python3 -I -c '
import json, sys
w = json.load(sys.stdin)["result"]["windows"]
w = [x for x in w if x.get("title") == sys.argv[1]] or w
print(w[0]["id"], w[0]["x"], w[0]["y"])' "${SPIKE_WINDOW:-Junto gpui spike}")"
exec "$here/out/bin/input" "$pid" $origin "$@"
