#!/bin/sh
# usage: fetch.sh [name...]; copies screenshots (all, or the named ones) from the test machine into out/. JUNTO_TEST_LINUX_HOST is the SSH name of the test machine.
set -eu
here="$(cd "$(dirname "$0")/.." && pwd)"
host="${JUNTO_TEST_LINUX_HOST:?set the SSH name of the Linux test machine}"
mkdir -p "$here/out"
if [ "$#" -eq 0 ]; then
    rsync -a "$host:/srv/junto-spike-gpui/shots/" "$here/out/"
else
    for name in "$@"; do
        scp -q "$host:/srv/junto-spike-gpui/shots/$name.png" "$here/out/$name.png"
    done
fi
