#!/bin/sh
set -eu
export XDG_RUNTIME_DIR=/tmp/junto-linux-runtime
mkdir -p "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"
# A fresh private session has only virtual audio devices.
if [ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ]; then
    exec dbus-run-session -- "$0" "$@"
fi
"$(dirname "$0")/sound.sh"
attempts=0
until pg_isready -q -h postgres -U postgres && curl -fsS -o /dev/null http://livekit:7880/; do
    attempts=$((attempts + 1))
    if [ "$attempts" -ge 120 ]; then
        echo "the database or LiveKit did not answer in 60 s" >&2
        exit 1
    fi
    sleep 0.5
done
cd /src
exec "$@"
