#!/bin/sh
# Runs one heavy command at a time on this machine.
lock="${TMPDIR:-/tmp}/junto-heavy.lock"
if command -v lockf >/dev/null 2>&1; then
    exec lockf -k "$lock" nice -n 10 "$@"
fi
exec flock "$lock" nice -n 10 "$@"
