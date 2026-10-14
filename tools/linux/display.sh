#!/bin/sh
# Run a command with an isolated software-rendered display inside the build container.
set -eu
backend=${1:?usage: display.sh x11|xorg|wayland command [arguments]}
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
        Xvfb "$DISPLAY" -screen 0 1920x1080x24 -nolisten tcp >/tmp/junto-linux-x11.log 2>&1 &
        display_pid=$!
        ready() { xdpyinfo >/dev/null 2>&1; }
        ;;
    xorg)
        unset WAYLAND_DISPLAY
        export DISPLAY=:99
        cat > /tmp/junto-linux-xorg.conf <<'CONFIG'
Section "ServerFlags"
    Option "AutoAddDevices" "false"
    Option "AutoEnableDevices" "false"
    Option "AutoAddGPU" "false"
    Option "DontVTSwitch" "true"
EndSection
Section "Device"
    Identifier "virtual"
    Driver "dummy"
    VideoRam 256000
EndSection
Section "Monitor"
    Identifier "virtual"
    HorizSync 5.0-1000.0
    VertRefresh 5.0-200.0
    Modeline "1920x1080" 148.50 1920 2008 2052 2200 1080 1084 1089 1125
EndSection
Section "Screen"
    Identifier "virtual"
    Device "virtual"
    Monitor "virtual"
    DefaultDepth 24
    SubSection "Display"
        Depth 24
        Modes "1920x1080"
    EndSubSection
EndSection
CONFIG
        /usr/lib/xorg/Xorg "$DISPLAY" -noreset -nolisten tcp -novtswitch \
            -config /tmp/junto-linux-xorg.conf -logfile /tmp/junto-linux-xorg-server.log \
            >/tmp/junto-linux-xorg.log 2>&1 &
        display_pid=$!
        ready() { xdpyinfo >/dev/null 2>&1; }
        ;;
    wayland)
        unset DISPLAY
        export WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1
        export WLR_HEADLESS_OUTPUTS=1
        unset WAYLAND_DISPLAY
        cat > /tmp/junto-linux-sway.conf <<'CONFIG'
output HEADLESS-1 mode 1920x1080
seat seat0 fallback true
CONFIG
        sway -c /tmp/junto-linux-sway.conf >/tmp/junto-linux-wayland.log 2>&1 &
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
        cat "/tmp/junto-linux-$backend.log" >&2
        exit 1
    fi
    sleep 0.5
done
if [ "$backend" = x11 ] || [ "$backend" = xorg ]; then
    openbox >/tmp/junto-linux-openbox.log 2>&1 &
    manager_pid=$!
    attempts=0
    until xprop -root _NET_SUPPORTING_WM_CHECK | grep -q 'window id'; do
        attempts=$((attempts + 1))
        if [ "$attempts" -ge 60 ] || ! kill -0 "$manager_pid" 2>/dev/null; then
            cat /tmp/junto-linux-openbox.log >&2
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
