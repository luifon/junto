#!/bin/sh
# Run Linux builds and tests under the machine-wide build lock.
set -eu
root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
if [ "$#" -eq 0 ]; then
    echo "usage: $0 <command> [arguments]" >&2
    exit 2
fi
exec "$root/tools/serial.sh" "$root/tools/linux/run.sh" "$@"
