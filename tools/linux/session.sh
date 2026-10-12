#!/bin/sh
set -eu
export XDG_RUNTIME_DIR=/tmp/junto-linux1-runtime
mkdir -p "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"
# A fresh private session has only virtual audio devices.
if [ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ]; then
    exec dbus-run-session -- "$0" "$@"
fi
pipewire >/tmp/junto-linux1-pipewire.log 2>&1 &
wireplumber >/tmp/junto-linux1-wireplumber.log 2>&1 &
pipewire-pulse >/tmp/junto-linux1-pulse.log 2>&1 &
attempts=0
until pg_isready -q -h postgres -U postgres && curl -fsS -o /dev/null http://livekit:7880/ && pactl info >/dev/null 2>&1; do
    attempts=$((attempts + 1))
    if [ "$attempts" -ge 120 ]; then
        cat /tmp/junto-linux1-pipewire.log /tmp/junto-linux1-wireplumber.log /tmp/junto-linux1-pulse.log >&2
        exit 1
    fi
    sleep 0.5
done
pactl load-module module-null-sink sink_name=junto-test-output sink_properties=device.description=Junto-Test-Output >/dev/null
pactl load-module module-null-sink sink_name=junto-test-input sink_properties=device.description=Junto-Test-Input >/dev/null
pactl set-default-sink junto-test-output
pactl set-default-source junto-test-input.monitor
cd /src
exec "$@"
