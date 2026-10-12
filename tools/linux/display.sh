#!/bin/sh
# Run a command with an isolated software-rendered display inside the build container.
set -eu
backend=${1:?usage: display.sh x11|wayland command [arguments]}
shift
manager_pid=""
cleanup() {
    if [ -n "$manager_pid" ]; then
        kill "$manager_pid" 2>/dev/null || true
        wait "$manager_pid" 2>/dev/null || true
    fi
    kill "$display_pid" 2>/dev/null || true
    wait "$display_pid" 2>/dev/null || true
}
case "$backend" in
    x11)
        unset WAYLAND_DISPLAY
        export DISPLAY=:99
        Xvfb "$DISPLAY" -screen 0 1920x1080x24 -nolisten tcp >/tmp/junto-linux1-x11.log 2>&1 &
        display_pid=$!
        ready() { xdpyinfo >/dev/null 2>&1; }
        ;;
    wayland)
        unset DISPLAY
        export WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1
        export WLR_HEADLESS_OUTPUTS=1
        unset WAYLAND_DISPLAY
        cat > /tmp/junto-linux1-sway.conf <<'CONFIG'
output HEADLESS-1 mode 1920x1080
seat seat0 fallback true
CONFIG
        sway -c /tmp/junto-linux1-sway.conf >/tmp/junto-linux1-wayland.log 2>&1 &
        display_pid=$!
        ready() {
            for socket in "$XDG_RUNTIME_DIR"/wayland-*; do
                if [ -S "$socket" ]; then
                    export WAYLAND_DISPLAY="${socket##*/}"
                    return 0
                fi
            done
            return 1
        }
        ;;
    *) echo "unknown display backend: $backend" >&2; exit 2 ;;
esac
trap cleanup EXIT
trap 'exit 130' INT TERM
attempts=0
until ready; do
    attempts=$((attempts + 1))
    if [ "$attempts" -ge 60 ] || ! kill -0 "$display_pid" 2>/dev/null; then
        cat "/tmp/junto-linux1-$backend.log" >&2
        exit 1
    fi
    sleep 0.5
done
if [ "$backend" = x11 ]; then
    openbox >/tmp/junto-linux1-openbox.log 2>&1 &
    manager_pid=$!
    attempts=0
    until xprop -root _NET_SUPPORTING_WM_CHECK | grep -q 'window id'; do
        attempts=$((attempts + 1))
        if [ "$attempts" -ge 60 ] || ! kill -0 "$manager_pid" 2>/dev/null; then
            cat /tmp/junto-linux1-openbox.log >&2
            exit 1
        fi
        sleep 0.5
    done
fi
if [ "$backend" = wayland ]; then
    for socket in "$XDG_RUNTIME_DIR"/sway-ipc.*.sock; do
        if [ -S "$socket" ]; then export SWAYSOCK="$socket"; break; fi
    done
fi
"$@"
