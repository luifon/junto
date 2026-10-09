#!/bin/sh
# usage: shot.sh <name> [extra automation tool arguments...]; saves out/<name>.png and prints the accessibility tree unless TREE=0. JUNTO_TEST_AUTOMATION_CLI is the command of the desktop automation tool.
set -eu
automation="${JUNTO_TEST_AUTOMATION_CLI:?set the command of the desktop automation tool}"
name="$1"; shift
here="$(cd "$(dirname "$0")/.." && pwd)"
pid="${SPIKE_PID:-$(cat "$here/out/pid")}"
$automation get-app-state --app "pid:$pid" --json "$@" > "$here/out/state.json"
python3 -I "$here/scripts/state.py" "$here/out/state.json" "$here/out/$name.png" "${TREE:-1}"
