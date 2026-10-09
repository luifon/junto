#!/bin/bash
# Sourced by the other scripts. Finds the desktop session user on the test machine.
SPIKE_ROOT=/srv/junto-spike
SPIKE_UID=$(loginctl list-sessions --no-legend | awk '$4=="seat0"{print $2; exit}')
SPIKE_USER=$(id -nu "$SPIKE_UID")
export SPIKE_ROOT SPIKE_UID SPIKE_USER
as_user() {
  runuser -u "$SPIKE_USER" -- env XDG_RUNTIME_DIR=/run/user/"$SPIKE_UID" "$@"
}
