#!/bin/bash
# runs on the test machine; usage: stacking.sh <X11 id of the call window>; reads the stacking order with a second test window active, before and after _NET_WM_STATE_ABOVE
D=/srv/junto-spike-gpui/tools/desk.sh
stack() { $D as-user xprop -root _NET_CLIENT_LIST_STACKING | sed 's/.*# //'; }
main=$($D as-user xdotool search --name '^Junto gpui spike$' | tail -1)
call=$1
printf 'main=0x%x call=0x%x\n' "$main" "$call"
$D as-user setsid -f python3 -c 'import gi; gi.require_version("Gtk","3.0"); from gi.repository import Gtk,GLib; w=Gtk.Window(title="Junto spike other application"); w.set_default_size(900,600); w.show_all(); w.move(480,200); GLib.timeout_add_seconds(25,Gtk.main_quit); Gtk.main()' > /dev/null 2>&1 < /dev/null
sleep 2
other=$($D as-user xdotool search --name '^Junto spike other application$' | tail -1)
printf 'other=0x%x\n' "$other"
$D as-user xdotool windowactivate --sync "$other"; sleep 0.5
echo "3 stacking, another application activated:  $(stack)"
$D as-user import -window root png:- > /srv/junto-spike-gpui/shots/c8-07-floating-under-other-app.png
$D as-user wmctrl -i -r "$call" -b add,above; sleep 0.5
echo "4 call window state after add,above:        $($D as-user xprop -id "$call" _NET_WM_STATE)"
$D as-user xdotool windowactivate --sync "$other"; sleep 0.5
echo "5 stacking, another application activated:  $(stack)"
$D as-user import -window root png:- > /srv/junto-spike-gpui/shots/c8-08-above-other-app.png
$D as-user wmctrl -i -c "$other"; sleep 0.5
$D as-user xdotool windowactivate --sync "$main"; sleep 0.3
$D as-user wmctrl -i -c "$call"; sleep 1
echo "6 windows after closing the call window with the window manager's close request:"
$D info | grep -E "spike window|active"
tail -2 /srv/junto-spike-gpui/run/events.txt | cut -c1-120
