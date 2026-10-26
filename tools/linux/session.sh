#!/bin/sh
set -eu
export XDG_RUNTIME_DIR=/tmp/junto-linux-runtime
mkdir -p "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"
# A fresh private session has only virtual audio devices.
if [ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ]; then
    exec dbus-run-session -- "$0" "$@"
fi
logs="/tmp/junto-linux-pipewire.log /tmp/junto-linux-wireplumber.log /tmp/junto-linux-pulse.log"
pipewire >/tmp/junto-linux-pipewire.log 2>&1 &
wireplumber >/tmp/junto-linux-wireplumber.log 2>&1 &
pipewire-pulse >/tmp/junto-linux-pulse.log 2>&1 &
# A failed start names its step, prints the end of each sound log and keeps the logs in the
# artifacts of the run.
failed() {
    echo "the session did not start: $1" >&2
    mkdir -p /tmp/junto-linux-artifacts/session
    for log in $logs; do
        echo "--- $log" >&2
        tail -n 60 "$log" >&2 || true
        cp "$log" /tmp/junto-linux-artifacts/session/ || true
    done
    exit 1
}
step() {
    output=$("$@" 2>&1) || failed "$* ($output)"
}
attempts=0
until pg_isready -q -h postgres -U postgres && curl -fsS -o /dev/null http://livekit:7880/ && pactl info >/dev/null 2>&1; do
    attempts=$((attempts + 1))
    if [ "$attempts" -ge 120 ]; then
        failed "the database, LiveKit or the sound server did not answer in 60 s"
    fi
    sleep 0.5
done
step pactl load-module module-null-sink sink_name=junto-test-output sink_properties=device.description=Junto-Test-Output
step pactl load-module module-null-sink sink_name=junto-test-input sink_properties=device.description=Junto-Test-Input
step pactl set-default-sink junto-test-output
step pactl set-default-source junto-test-input.monitor
cd /src
exec "$@"
