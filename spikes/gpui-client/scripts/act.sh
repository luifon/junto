#!/bin/sh
# usage: act.sh <automation tool subcommand> [args...]; runs it against the spike pid and prints the outcome in one line. JUNTO_TEST_AUTOMATION_CLI is the command of the desktop automation tool.
set -eu
automation="${JUNTO_TEST_AUTOMATION_CLI:?set the command of the desktop automation tool}"
here="$(cd "$(dirname "$0")/.." && pwd)"
pid="${SPIKE_PID:-$(cat "$here/out/pid")}"
command="$1"; shift
$automation "$command" --app "pid:$pid" --no-screenshot --json "$@" | python3 -I -c '
import json, sys
d = json.load(sys.stdin)
a = (d.get("result") or {}).get("action") or {}
print(sys.argv[1], "ok" if d.get("ok") else "FAILED", a.get("path"), (a.get("verification") or {}).get("state"), d.get("error") or "")
' "$command"
