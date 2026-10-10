#!/bin/bash
# inside a tools container: input and screenshots for the headless compositor
# usage: wl.sh click <x> <y> [left|right|middle] | move <x> <y> | scroll <x> <y> <dy> <count> | key <wtype args...> | type <text> | shot <name> | list
export XDG_RUNTIME_DIR=/tmp/xdg WAYLAND_DISPLAY=wayland-0
command="$1"; shift
to() { wlrctl pointer move -10000 -10000; wlrctl pointer move "$1" "$2"; }
case "$command" in
click) to "$1" "$2"; sleep 0.15; wlrctl pointer click "${3:-left}" ;;
move) to "$1" "$2" ;;
scroll) to "$1" "$2"; for i in $(seq 1 "$4"); do wlrctl pointer scroll "$3" 0; sleep 0.02; done ;;
key) wtype "$@" ;;
type) wtype -d 30 "$1" ;;
shot) grim "/work/shots/$1.png" && echo "saved /work/shots/$1.png" ;;
list) wlrctl toplevel list ;;
esac
