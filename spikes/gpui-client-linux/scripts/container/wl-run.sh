#!/bin/bash
# inside a tools container: starts the spike under the headless compositor
# usage: wl-run.sh <out-dir-name> <gate> [spike args...]
export XDG_RUNTIME_DIR=/tmp/xdg HOME=/tmp/home WAYLAND_DISPLAY=wayland-0
export DBUS_SESSION_BUS_ADDRESS=unix:path=$XDG_RUNTIME_DIR/bus
export LANG=en_US.UTF-8 LC_CTYPE=pt_BR.UTF-8
unset DISPLAY
out="/work/$1"; gate="$2"; shift 2
mkdir -p "$out"
[ -d "$out/fonts" ] || cp -r /work/run/fonts "$out/fonts"
if [ -f "$out/pid" ]; then kill "$(cat "$out/pid")" 2>/dev/null; sleep 1; fi
cd "$out"
setsid -f sh -c 'echo $$ > pid; exec /work/run/junto-gpui-spike --gate "$0" --out "$1" "${@:2}" > "run-$0.log" 2>&1' "$gate" "$out" "$@" < /dev/null
sleep "${SPIKE_START_WAIT:-4}"
echo "pid $(cat pid) alive=$(kill -0 "$(cat pid)" 2>/dev/null && echo yes || echo no)"
grep -E "gpui_wgpu|wayland|Wayland|ERROR|WARN|panicked" "run-$gate.log" | grep -v zbus | cut -c1-230 | head -24
