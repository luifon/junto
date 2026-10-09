#!/bin/bash
. "$(dirname "$0")/env.sh"
cp "$(dirname "$0")"/sandbox_user.sh "$(dirname "$0")"/watch_real_sinks.py "$SPIKE_ROOT/run/"
chown -R "$SPIKE_USER" "$SPIKE_ROOT/run"
cd "$SPIKE_ROOT/run" && as_user bash "$SPIKE_ROOT/run/sandbox_user.sh"
