#!/bin/sh
# Starts PipeWire, WirePlumber and the PulseAudio server of PipeWire in the D-Bus session and
# XDG_RUNTIME_DIR of the caller, with two virtual null devices as the default output and input
# (ADR 0750). The services keep running after the script ends. WirePlumber ends when PipeWire
# does not answer yet, and then the metadata `default` that the default devices are written to
# never exists; a PulseAudio client that runs before the server created its socket can make the
# server fail to create it. Each step therefore waits for the state that the next one needs.
set -eu
logs=${JUNTO_SOUND_LOGS:-/tmp}
artifacts=${JUNTO_SOUND_ARTIFACTS:-/tmp/junto-linux-artifacts/sound}
failed() {
    echo "the sound services did not start: $1" >&2
    mkdir -p "$artifacts"
    for log in "$logs/junto-linux-pipewire.log" "$logs/junto-linux-wireplumber.log" "$logs/junto-linux-pulse.log"; do
        echo "--- $log" >&2
        tail -n 60 "$log" >&2 || true
        cp "$log" "$artifacts/" || true
    done
    exit 1
}
now() {
    read -r sound_uptime sound_idle < /proc/uptime
    printf '%s\n' "$(( ${sound_uptime%.*} * 100 + 1${sound_uptime#*.} - 100 ))"
}
deadline=$(( $(now) + 6000 ))
bounded() {
    remaining=$(( deadline - $(now) ))
    if [ "$remaining" -le 0 ]; then
        return 124
    fi
    timeout --signal=KILL "$(printf '%d.%02d' "$((remaining / 100))" "$((remaining % 100))")" "$@"
}
until_true() {
    what=$1
    shift
    while true; do
        status=0
        bounded "$@" >/dev/null 2>&1 || status=$?
        case "$status" in
            0) return ;;
            124|137) failed "after 60 s: $what" ;;
        esac
        sleep 0.1
    done
}
step() {
    output=$(bounded "$@" 2>&1) || failed "$* ($output)"
}
pipewire >"$logs/junto-linux-pipewire.log" 2>&1 &
until_true "PipeWire answers" pw-cli info 0
wireplumber >"$logs/junto-linux-wireplumber.log" 2>&1 &
pipewire-pulse >"$logs/junto-linux-pulse.log" 2>&1 &
until_true "the PulseAudio server created its socket" test -S "$XDG_RUNTIME_DIR/pulse/native"
until_true "the PulseAudio server answers" pactl info
until_true "WirePlumber created the metadata default" sh -c 'pw-cli ls Metadata | grep -q "metadata.name = \"default\""'
step pactl load-module module-null-sink sink_name=junto-test-output sink_properties=device.description=Junto-Test-Output
step pactl load-module module-null-sink sink_name=junto-test-input sink_properties=device.description=Junto-Test-Input
step pactl set-default-sink junto-test-output
step pactl set-default-source junto-test-input.monitor
