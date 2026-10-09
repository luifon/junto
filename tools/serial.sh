#!/bin/sh
# Runs one heavy command at a time on this machine.
exec lockf -k "${TMPDIR:-/tmp}/junto-heavy.lock" nice -n 10 "$@"
