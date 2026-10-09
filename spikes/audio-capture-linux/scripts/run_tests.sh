#!/bin/bash
# Usage (as root): run_tests.sh <test>...   Runs the named tests as the desktop user, with the real-sink watcher active.
. "$(dirname "$0")/env.sh"
mkdir -p "$SPIKE_ROOT/run/logs"; chown -R "$SPIKE_USER" "$SPIKE_ROOT/run"
cp "$(dirname "$0")"/tests_user.sh "$(dirname "$0")"/watch_real_sinks.py "$SPIKE_ROOT/run/"
cd "$SPIKE_ROOT/run" && as_user bash "$SPIKE_ROOT/run/tests_user.sh" "$@"
