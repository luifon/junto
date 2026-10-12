#!/bin/sh
# Run through tools/serial.sh. The directory must be empty and belongs to this check.
set -eu
if [ "$#" -ne 1 ]; then
    echo 'usage: tools/check-client-start.sh <empty-data-directory>' >&2
    exit 2
fi
cargo build -p junto-client --bin junto
python3 - "$1" <<'PY'
import pathlib
import subprocess
import sys

folder = pathlib.Path(sys.argv[1]).resolve()
if folder.exists() and any(folder.iterdir()):
    raise SystemExit('startup check requires an empty data directory')
folder.mkdir(mode=0o700, parents=True, exist_ok=True)
folder.chmod(0o700)
child = subprocess.Popen(
    ['target/debug/junto', '--data-dir', str(folder), '--no-activate', '--check-startup'],
    stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
)
try:
    output, errors = child.communicate(timeout=60)
except subprocess.TimeoutExpired:
    child.kill()
    output, errors = child.communicate()
    print(output, end='')
    print(errors, end='', file=sys.stderr)
    raise SystemExit('client did not finish its first-frame startup check')
print(output, end='')
print(errors, end='', file=sys.stderr)
if child.returncode != 0 or 'Junto first frame rendered' not in output:
    raise SystemExit(f'client startup check failed: exit {child.returncode}')
PY
