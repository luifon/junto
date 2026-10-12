#!/bin/sh
# The chooser-free portal is restricted to the container's one virtual Wayland output.
set -eu
if [ "${JUNTO_TEST_VIRTUAL_AUDIO:-}" != 1 ] || [ "${XDG_RUNTIME_DIR:-}" != /tmp/junto-linux1-runtime ] || [ -n "${DISPLAY:-}" ]; then
    echo 'the private Linux test session is required' >&2
    exit 2
fi
case "${WAYLAND_DISPLAY:-}" in wayland-*) ;; *) echo 'the virtual Wayland display is required' >&2; exit 2 ;; esac
swaymsg -t get_outputs | jq -e 'length == 1 and .[0].name == "HEADLESS-1"' >/dev/null
export XDG_CURRENT_DESKTOP=sway
export XDG_CONFIG_HOME=/tmp/junto-linux1-portal-config
mkdir -p "$XDG_CONFIG_HOME/xdg-desktop-portal-wlr"
cat > "$XDG_CONFIG_HOME/xdg-desktop-portal-wlr/config" <<'CONFIG'
[screencast]
chooser_type=none
output_name=HEADLESS-1
max_fps=30
CONFIG
pids=""
cleanup() {
    result=$?
    if [ "$result" -ne 0 ]; then
        cat /tmp/junto-linux1-portal-wlr.log /tmp/junto-linux1-portal.log >&2 || true
    fi
    for process in $pids; do kill "$process" 2>/dev/null || true; done
    for process in $pids; do wait "$process" 2>/dev/null || true; done
}
trap cleanup EXIT
trap 'exit 130' INT TERM
wait_owner() {
    attempts=0
    until dbus-send --session --reply-timeout=1000 --print-reply --dest=org.freedesktop.DBus \
        /org/freedesktop/DBus org.freedesktop.DBus.NameHasOwner string:"$2" | grep -q 'boolean true'; do
        attempts=$((attempts + 1))
        if [ "$attempts" -ge 60 ] || ! kill -0 "$1" 2>/dev/null; then
            echo "the private portal did not acquire $2" >&2
            exit 1
        fi
        sleep 0.5
    done
}
/usr/libexec/xdg-desktop-portal-wlr -l DEBUG -c "$XDG_CONFIG_HOME/xdg-desktop-portal-wlr/config" \
    >/tmp/junto-linux1-portal-wlr.log 2>&1 &
pids="$pids $!"
wait_owner "$!" org.freedesktop.impl.portal.desktop.wlr
G_MESSAGES_DEBUG=all /usr/libexec/xdg-desktop-portal >/tmp/junto-linux1-portal.log 2>&1 &
pids="$pids $!"
wait_owner "$!" org.freedesktop.portal.Desktop
attempts=0
until dbus-send --session --reply-timeout=1000 --print-reply --dest=org.freedesktop.portal.Desktop \
    /org/freedesktop/portal/desktop org.freedesktop.DBus.Properties.Get \
    string:org.freedesktop.portal.ScreenCast string:AvailableSourceTypes >/dev/null 2>&1; do
    attempts=$((attempts + 1))
    if [ "$attempts" -ge 60 ]; then
        cat /tmp/junto-linux1-portal-wlr.log /tmp/junto-linux1-portal.log >&2
        exit 1
    fi
    sleep 0.5
done
export JUNTO_TEST_VIRTUAL_PORTAL=1
"$@"
