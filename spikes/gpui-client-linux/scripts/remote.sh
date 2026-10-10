#!/bin/sh
# usage: remote.sh <desk.sh command> [values...]; runs scripts/desk.sh on the test machine. Environment names starting with SPIKE_, KEY_, DRAG_ or SCROLL_ are passed on. JUNTO_TEST_LINUX_HOST is the SSH name of the test machine.
set -eu
host="${JUNTO_TEST_LINUX_HOST:?set the SSH name of the Linux test machine}"
quoted=""
for value in "$@"; do
    quoted="$quoted '$(printf '%s' "$value" | sed "s/'/'\\\\''/g")'"
done
passed="$(env | grep -E '^(SPIKE_ENV|SPIKE_WINDOW|SPIKE_START_WAIT|SPIKE_BINARY|KEY_DELAY|DRAG_[A-Z_]+|SCROLL_DELAY)=' | sed "s/'/'\\\\''/g; s/=\(.*\)$/='\1'/" | tr '\n' ' ')"
exec ssh "$host" "$passed /srv/junto-spike-gpui/tools/desk.sh $quoted"
