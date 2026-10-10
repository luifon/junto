#!/bin/bash
# Runs on the Linux test machine as root. Operates the spike inside the desktop user's existing X11 session.
# usage: desk.sh <command> [values...]; see the case statement at the end.
set -eu
root="${SPIKE_ROOT:-/srv/junto-spike-gpui}"
run="$root/run"
shots="$root/shots"
binary="${SPIKE_BINARY:-$run/junto-gpui-spike}"

session="$(loginctl list-sessions --no-legend | awk '$4 == "seat0" { print $1; exit }')"
desk_user="$(loginctl show-session "$session" -p Name --value)"
desk_uid="$(id -u "$desk_user")"
desk_home="$(getent passwd "$desk_user" | cut -d: -f6)"

session_value() {
    for candidate in $(pgrep -u "$desk_uid"); do
        value="$(tr '\0' '\n' < "/proc/$candidate/environ" 2>/dev/null | sed -n "s/^$1=//p" | head -1)"
        if [ -n "$value" ]; then
            printf '%s' "$value"
            return
        fi
    done
}

display="$(loginctl show-session "$session" -p Display --value)"
xauthority="$(session_value XAUTHORITY)"

as_user() {
    runuser -u "$desk_user" -- env -i \
        HOME="$desk_home" USER="$desk_user" LOGNAME="$desk_user" PATH=/usr/local/bin:/usr/bin:/bin \
        DISPLAY="$display" XAUTHORITY="$xauthority" \
        XDG_RUNTIME_DIR="/run/user/$desk_uid" \
        DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$desk_uid/bus" \
        XDG_SESSION_TYPE="$(session_value XDG_SESSION_TYPE)" \
        XDG_CURRENT_DESKTOP="$(session_value XDG_CURRENT_DESKTOP)" \
        LANG="$(session_value LANG)" LC_CTYPE="$(session_value LC_CTYPE)" \
        XMODIFIERS="$(session_value XMODIFIERS)" \
        SOURCE_LOG="$run/drag-source.log" \
        ${SPIKE_ENV:-} "$@"
}

spike_pid() {
    cat "$run/pid" 2>/dev/null || true
}

spike_alive() {
    pid="$(spike_pid)"
    [ -n "$pid" ] && [ "$(readlink "/proc/$pid/exe" 2>/dev/null)" = "$binary" ]
}

windows() {
    as_user xdotool search --onlyvisible --pid "$(spike_pid)" 2>/dev/null || true
}

window_named() {
    for id in $(windows); do
        if [ "$(as_user xdotool getwindowname "$id")" = "$1" ]; then
            echo "$id"
            return
        fi
    done
}

main_window() {
    window_named "${SPIKE_WINDOW:-Junto gpui spike}"
}

active_report() {
    active="$(as_user xdotool getactivewindow 2>/dev/null || echo none)"
    if [ "$active" = none ]; then
        echo "active window: none"
        return
    fi
    echo "active window: $active pid=$(as_user xdotool getwindowpid "$active" 2>/dev/null || echo '?') name=$(as_user xdotool getwindowname "$active" 2>/dev/null | cut -c1-60)"
}

guard() {
    spike_alive || { echo "guard: the spike is not running"; exit 3; }
    active="$(as_user xdotool getactivewindow 2>/dev/null || echo none)"
    owner="$(as_user xdotool getwindowpid "$active" 2>/dev/null || echo none)"
    if [ "$owner" != "$(spike_pid)" ]; then
        echo "guard: refused, the active window does not belong to the spike"
        active_report
        exit 4
    fi
}

pointer_guard() {
    eval "$(as_user xdotool getmouselocation --shell)"
    owner="$(as_user xdotool getwindowpid "$WINDOW" 2>/dev/null || echo none)"
    if [ "$owner" != "${1:-$(spike_pid)}" ]; then
        echo "guard: refused, the window under the pointer does not belong to the expected process"
        exit 5
    fi
}

command="$1"
shift
case "$command" in
info)
    echo "session=$session type=$(loginctl show-session "$session" -p Type --value) display=$display uid=$desk_uid"
    active_report
    as_user xdotool getmouselocation
    for id in $(windows); do
        echo "spike window $id name=$(as_user xdotool getwindowname "$id") $(as_user xdotool getwindowgeometry "$id" | tr '\n' ' ')"
    done
    ;;
start)
    gate="$1"
    shift
    if spike_alive; then kill "$(spike_pid)"; sleep 1; fi
    as_user setsid -f sh -c 'cd "$1" && echo $$ > pid && exec "$2" --gate "$3" --out "$1" "${@:4}" > "run-$3.log" 2>&1' \
        sh "$run" "$binary" "$gate" "$@" < /dev/null
    sleep "${SPIKE_START_WAIT:-4}"
    echo "pid $(spike_pid) alive=$(spike_alive && echo yes || echo no)"
    active_report
    ;;
stop)
    if spike_alive; then kill "$(spike_pid)"; sleep 1; fi
    spike_alive && echo "still running" || echo "stopped"
    ;;
activate)
    spike_alive || { echo "not running"; exit 3; }
    as_user xdotool windowactivate --sync "$(window_named "${1:-Junto gpui spike}")"
    active_report
    ;;
shot)
    name="$1"
    id="$(window_named "${2:-Junto gpui spike}")"
    as_user import -window "$id" "png:-" > "$shots/$name.png"
    echo "saved $shots/$name.png"
    active_report
    ;;
shot-root)
    as_user import -window root ${2:+-crop "$2"} +repage "png:-" > "$shots/$1.png"
    echo "saved $shots/$1.png"
    active_report
    ;;
key)
    guard
    as_user xdotool key --delay "${KEY_DELAY:-60}" "$@"
    ;;
type)
    guard
    as_user xdotool type --delay "${KEY_DELAY:-40}" "$1"
    ;;
move)
    guard
    as_user xdotool mousemove --window "$(main_window)" "$1" "$2"
    ;;
click)
    guard
    as_user xdotool mousemove --sync --window "$(main_window)" "$1" "$2"
    pointer_guard
    as_user xdotool click ${4:+--repeat "$4" --delay 120} "${3:-1}"
    ;;
drag)
    guard
    as_user xdotool mousemove --sync --window "$(main_window)" "$1" "$2"
    pointer_guard
    as_user xdotool mousedown 1
    steps="${5:-12}"
    for step in $(seq 1 "$steps"); do
        x=$(( $1 + ($3 - $1) * step / steps ))
        y=$(( $2 + ($4 - $2) * step / steps ))
        as_user xdotool mousemove --window "$(main_window)" "$x" "$y"
        sleep "${DRAG_STEP_WAIT:-0.05}"
    done
    if [ -n "${DRAG_HOLD_SHOT:-}" ]; then
        as_user import -window "$(main_window)" "png:-" > "$shots/$DRAG_HOLD_SHOT.png"
    fi
    as_user xdotool mouseup 1
    ;;
scroll)
    guard
    as_user xdotool mousemove --sync --window "$(main_window)" "$1" "$2"
    pointer_guard
    as_user xdotool click --repeat "$4" --delay "${SCROLL_DELAY:-15}" "$3"
    ;;
clipboard-get)
    as_user xclip -o -selection "${1:-clipboard}" 2>/dev/null || echo "(empty or not text)"
    ;;
clipboard-set)
    printf '%s' "$2" | as_user xclip -i -selection "$1" > /dev/null 2>&1
    ;;
props)
    as_user xprop -id "$(window_named "${1:-Junto gpui spike}")"
    ;;
memory)
    pid="$(spike_pid)"
    grep -E "VmRSS|VmHWM|RssAnon|RssFile|RssShmem|Threads" "/proc/$pid/status" | tr -s '\t ' ' ' | tr '\n' ';'
    echo
    awk '/^Pss:/ { pss += $2 } END { print "Pss total kB: " pss }' "/proc/$pid/smaps_rollup"
    ;;
cpu)
    pid="$(spike_pid)"
    seconds="${1:-5}"
    ticks() { awk '{ print $14 + $15 }' "/proc/$pid/stat"; }
    before="$(ticks)"
    sleep "$seconds"
    after="$(ticks)"
    echo "process CPU over ${seconds}s: $(( (after - before) * 100 / $(getconf CLK_TCK) / seconds )) % of one core"
    ;;
events)
    tail -n "${1:-15}" "$run/events.txt"
    ;;
sheet)
    name="$1"
    crop="$2"
    shift 2
    parts=()
    for part in "$@"; do
        parts+=( "(" "$shots/$part.png" -crop "$crop" +repage -bordercolor "#b5502f" -border 2 ")" )
    done
    convert "${parts[@]}" -append "$shots/$name.png"
    echo "saved $shots/$name.png"
    ;;
xdnd)
    guard
    target_x="$1"
    target_y="$2"
    shift 2
    as_user setsid -f sh -c 'exec python3 "$0" 20 300 "$@" > "$SOURCE_LOG" 2>&1' "$root/tools/drag_source.py" "$@" < /dev/null
    sleep 2
    source_window="$(as_user xdotool search --name '^Junto spike drag source$' | tail -1)"
    source_pid="$(as_user xdotool getwindowpid "$source_window")"
    eval "$(as_user xdotool getwindowgeometry --shell "$source_window")"
    echo "drag source window $source_window at $X,$Y size ${WIDTH}x$HEIGHT pid $source_pid"
    start_x=$(( X + WIDTH / 2 ))
    start_y=$(( Y + HEIGHT / 2 ))
    eval "$(as_user xdotool getwindowgeometry --shell "$(main_window)")"
    end_x=$(( X + target_x ))
    end_y=$(( Y + target_y ))
    as_user xdotool mousemove --sync "$start_x" "$start_y"
    pointer_guard "$source_pid"
    as_user xdotool mousedown 1
    steps="${XDND_STEPS:-40}"
    for step in $(seq 1 "$steps"); do
        as_user xdotool mousemove $(( start_x + (end_x - start_x) * step / steps )) $(( start_y + (end_y - start_y) * step / steps ))
        sleep 0.08
    done
    for wiggle in 1 2 3 4 5 6; do
        as_user xdotool mousemove_relative -- $(( wiggle % 2 * 6 - 3 )) 2
        sleep 0.25
    done
    as_user import -window root "png:-" > "$shots/${XDND_SHOT:-c7-06-file-drag-over}.png"
    as_user xdotool mouseup 1
    sleep 1
    kill "$source_pid" 2>/dev/null || true
    cat "$run/drag-source.log"
    ;;
as-user)
    as_user "$@"
    ;;
*)
    echo "unknown command $command"
    exit 2
    ;;
esac
