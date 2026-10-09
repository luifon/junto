#!/bin/sh
# usage: press.sh "<accessibility label>" [extra automation tool arguments]; clicks the first element whose line ends with that label. JUNTO_TEST_AUTOMATION_CLI is the command of the desktop automation tool.
set -eu
automation="${JUNTO_TEST_AUTOMATION_CLI:?set the command of the desktop automation tool}"
here="$(cd "$(dirname "$0")/.." && pwd)"
pid="${SPIKE_PID:-$(cat "$here/out/pid")}"
label="$1"; shift
$automation get-app-state --app "pid:$pid" --no-screenshot --json "$@" > "$here/out/press.json"
index="$(python3 -I -c '
import json, re, sys
tree = json.load(open(sys.argv[1]))["result"]["snapshot"]["treeText"]
for line in tree.split("\n"):
    m = re.match(r"\s*(\d+) (.*)", line)
    if m and m.group(2).endswith(sys.argv[2]):
        print(m.group(1)); break
' "$here/out/press.json" "$label")"
if [ -z "$index" ]; then echo "press: no element named '$label'"; exit 1; fi
$automation click --app "pid:$pid" --element-index "$index" --no-screenshot --json "$@" | python3 -I -c '
import json, sys
d = json.load(sys.stdin)
a = (d.get("result") or {}).get("action") or {}
print("press", sys.argv[1], "ok" if d.get("ok") else "FAILED", a.get("path"), d.get("error") or "")' "$label"
