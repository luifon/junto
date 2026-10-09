#!/bin/sh
# Runs a command with the output volume at QUIET_VOLUME (default 0) and
# QUIET_MUTED (default true), then restores the previous volume and mute state.
settings=$(osascript -e 'get volume settings')
previous_volume=$(printf '%s' "$settings" | sed -E 's/.*output volume:([0-9]+).*/\1/')
previous_muted=$(printf '%s' "$settings" | sed -E 's/.*output muted:(true|false).*/\1/')

apply() {
    osascript -e "set volume output volume $1 output muted $2" >/dev/null
}

restore() {
    apply "$previous_volume" "$previous_muted"
    echo "volume restored: $(osascript -e 'get volume settings')" >&2
}
trap restore EXIT
trap 'exit 130' INT TERM

apply "${QUIET_VOLUME:-0}" "${QUIET_MUTED:-true}"
echo "volume during run: $(osascript -e 'get volume settings')" >&2
"$@"
