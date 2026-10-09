#!/bin/bash
# runs on the test machine; usage: notification_click.sh hover <x> <y> | click <x> <y>; acts only when the window under the pointer is a small notification popup
D=/srv/junto-spike-gpui/tools/desk.sh
$D as-user xdotool mousemove --sync "$2" "$3"
sleep 0.4
eval "$($D as-user xdotool getmouselocation --shell)"
eval "$($D as-user xdotool getwindowgeometry --shell "$WINDOW" | sed 's/^/W_/')"
name="$($D as-user xdotool getwindowname "$WINDOW" 2>/dev/null)"
class="$($D as-user xprop -id "$WINDOW" WM_CLASS _NET_WM_WINDOW_TYPE | tr '\n' ' ')"
echo "under pointer: window=$WINDOW size=${W_WIDTH}x${W_HEIGHT} at ${W_X},${W_Y} name='$name' $class"
if [ "$1" = click ]; then
  case "$class" in
    *plasmashell*NOTIFICATION*) $D as-user xdotool click 1; echo clicked ;;
    *) echo "refused: not a notification popup" ;;
  esac
fi
