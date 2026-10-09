#!/bin/sh
# Usage (from the spike directory, after `../../tools/serial.sh cargo build --release`):
#   ./make_apps.sh
#   [BACKGROUND_BUNDLE=<bundle id of an application that is already playing sound>] \
#       ./quiet.sh ./run.sh <experiment>...
# Experiments: baseline matrix siblings volume_0_muted volume_0_unmuted volume_5_muted formats latency
set -u
SPIKE=./target/release/spike
HERE=$(pwd)
ID=spike.audiocapture
FREQS=1000,2500,3200,4300,6100,7300,5300
BACKGROUND_BUNDLE=${BACKGROUND_BUNDLE:-}
mkdir -p out
for hz in 1000 2500 6100 7300; do
    [ -f "out/tone$hz.wav" ] || $SPIKE wav --hz $hz --seconds 90 --out "out/tone$hz.wav"
done

stop_players() {
    pkill -f "$HERE/out/apps/" 2>/dev/null
    pkill -f "$HERE/out/tone" 2>/dev/null
    [ -n "${SIBLING:-}" ] && kill "$SIBLING" 2>/dev/null
    sleep 0.5
}
trap stop_players EXIT

start_players() {
    open -n -g out/apps/WindowedA.app --args -autoexit -loglevel error -x 200 -y 100 "$HERE/out/tone1000.wav"
    open -n -g out/apps/WindowedB.app --args -autoexit -loglevel error -x 200 -y 100 -left 300 "$HERE/out/tone2500.wav"
    open -n -g out/apps/Windowless.app --args tone --hz 3200 --seconds 90
    open -n -g out/apps/ChildHost.app --args "$HERE/out/tone7300.wav"
    afplay out/tone6100.wav &
    SIBLING=$!
    sleep 3
    echo "players:"
    echo "  1000 Hz  application WindowedA (own bundle, window, started by open)"
    echo "  2500 Hz  application WindowedB (own bundle, window, started by open)"
    echo "  3200 Hz  application Windowless (own bundle, no window, started by open)"
    echo "  7300 Hz  afplay as a child process of application ChildHost (started by open)"
    echo "  6100 Hz  afplay started from the same terminal as the capture process"
    echo "  4300 Hz  the capture process itself (--own-tone)"
    echo "  5300 Hz  nothing (reference)"
}

capture() {
    name=$1
    shift
    echo "=== $name: spike capture $* [$(osascript -e 'get volume settings' | sed -E 's/, input.*alert volume:[0-9]+//')]"
    $SPIKE capture "$@" --freqs $FREQS --out "out/$name.wav" 2>&1
}

baseline() {
    capture baseline_no_players --mode system --seconds 3
}

matrix() {
    start_players
    capture app_a --mode app --bundle $ID.WindowedA --seconds 4 --own-tone 4300 --exclude-self true
    capture app_b --mode app --bundle $ID.WindowedB --seconds 4 --own-tone 4300 --exclude-self true
    capture app_a_include_self --mode app --bundle $ID.WindowedA --seconds 4 --own-tone 4300 --exclude-self false
    capture window_a --mode window --bundle $ID.WindowedA --seconds 4 --own-tone 4300 --exclude-self true
    capture system_exclude_self --mode system --seconds 4 --own-tone 4300 --exclude-self true
    capture system_include_self --mode system --seconds 4 --own-tone 4300 --exclude-self false
    if [ -n "$BACKGROUND_BUNDLE" ]; then
        capture system_exclude_self_without_background --mode system --exclude-bundles "$BACKGROUND_BUNDLE" --seconds 4 --own-tone 4300 --exclude-self true
        capture system_include_self_without_background --mode system --exclude-bundles "$BACKGROUND_BUNDLE" --seconds 4 --own-tone 4300 --exclude-self false
    fi
    stop_players
}

siblings() {
    ffplay -autoexit -loglevel error -x 200 -y 100 out/tone1000.wav &
    WINDOWED=$!
    afplay out/tone6100.wav &
    SIBLING=$!
    sleep 2
    echo "players: 1000 Hz windowed ffplay pid=$WINDOWED and 6100 Hz afplay, both started from the same terminal as the capture process; 4300 Hz capture process"
    capture siblings_app --mode app --pid "$WINDOWED" --seconds 4 --own-tone 4300 --exclude-self true
    kill "$WINDOWED" 2>/dev/null
    stop_players
}

volume() {
    open -n -g out/apps/WindowedA.app --args -autoexit -loglevel error -x 200 -y 100 "$HERE/out/tone1000.wav"
    sleep 3
    capture "volume_$1" --mode app --bundle $ID.WindowedA --seconds 3
    capture "volume_$1_system" --mode system --seconds 3
    stop_players
}
volume_0_muted() { volume 0_muted; }
volume_0_unmuted() { volume 0_unmuted; }
volume_5_muted() { volume 5_muted; }

formats() {
    open -n -g out/apps/WindowedA.app --args -autoexit -loglevel error -x 200 -y 100 "$HERE/out/tone1000.wav"
    sleep 3
    for setting in "16000 1" "24000 2" "44100 2" "8000 1"; do
        set -- $setting
        capture "format_$1_$2" --mode app --bundle $ID.WindowedA --seconds 2 --rate "$1" --channels "$2"
    done
    stop_players
}

latency() {
    exclude=""
    [ -n "$BACKGROUND_BUNDLE" ] && exclude="--exclude-bundles $BACKGROUND_BUNDLE"
    for round in 1 2 3 4 5; do
        $SPIKE capture --mode system $exclude --exclude-self false --seconds 6 --freqs 1000 --out "out/latency_$round.wav" > "out/latency_capture_$round.txt" 2>&1 &
        CAPTURE=$!
        sleep 2
        $SPIKE tone --hz 1000 --lead 1 --seconds 1.5 > "out/latency_tone_$round.txt" 2>&1
        wait "$CAPTURE"
        played=$(sed -n 's/.*first_tone_callback_unix_nanos=\([0-9]*\).*/\1/p' "out/latency_tone_$round.txt")
        tone_offset=$(sed -n 's/.*first_tone_frame_offset=\([0-9]*\).*/\1/p' "out/latency_tone_$round.txt")
        device_latency=$(sed -n 's/.*output_latency_nanos=\([0-9]*\).*/\1/p' "out/latency_tone_$round.txt")
        received=$(sed -n 's/.*buffer_arrival_unix_nanos=\([0-9]*\).*/\1/p' "out/latency_capture_$round.txt")
        capture_offset=$(sed -n 's/.*offset_in_buffer_frames=\([0-9]*\).*/\1/p' "out/latency_capture_$round.txt")
        echo "--- latency round $round"
        grep -E '^(audio_buffers|host_clock|onset)' "out/latency_capture_$round.txt"
        grep 'first_tone' "out/latency_tone_$round.txt"
        awk -v p="$played" -v r="$received" -v o="$tone_offset" -v c="$capture_offset" -v d="$device_latency" 'BEGIN {
            printf "source callback that holds the first tone sample -> capture callback that delivers it: %.1f ms\n", (r - p) / 1e6
            printf "  first tone sample is %d frames (%.1f ms) into the source buffer and %d frames (%.1f ms) into the captured buffer; source output latency reported by the device: %.1f ms\n", o, o / 48.0, c, c / 48.0, d / 1e6 }'
    done
}

for experiment in "$@"; do
    "$experiment"
done
