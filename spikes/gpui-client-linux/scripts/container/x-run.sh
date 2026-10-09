#!/bin/bash
# inside the tools container: starts the spike on the private X server
# usage: x-run.sh <out-dir-name> <gate> [spike args...]
export XDG_RUNTIME_DIR=/tmp/xdg HOME=/tmp/home DISPLAY=:77
export DBUS_SESSION_BUS_ADDRESS=unix:path=$XDG_RUNTIME_DIR/bus
export LANG=en_US.UTF-8 LC_CTYPE=pt_BR.UTF-8
unset WAYLAND_DISPLAY
out="/work/$1"; gate="$2"; shift 2
mkdir -p "$out"
[ -d "$out/fonts" ] || cp -r /work/run/fonts "$out/fonts"
if [ -f "$out/pid" ]; then kill "$(cat "$out/pid")" 2>/dev/null; sleep 1; fi
cd "$out"
setsid -f sh -c 'echo $$ > pid; exec /work/run/junto-gpui-spike --gate "$0" --out "$1" "${@:2}" > "run-$0.log" 2>&1' "$gate" "$out" "$@" < /dev/null
sleep "${SPIKE_START_WAIT:-4}"
echo "pid $(cat pid) alive=$(kill -0 "$(cat pid)" 2>/dev/null && echo yes || echo no)"
grep -E "Selected GPU adapter|scale factor|a11y|Accessib|ERROR|panicked|xim|XIM" "run-$gate.log" | grep -v zbus | cut -c1-220 | head -12
