#!/bin/bash
# runs on the test machine with the C12 screen open: Tab order, activation, shortcuts and both keyboard paths to the menu
D=/srv/junto-spike-gpui/tools/desk.sh
k() { $D key "$@"; sleep 0.25; }
shot() { $D as-user import -window "$($D as-user xdotool search --name '^Junto gpui spike$' | tail -1)" png:- > /srv/junto-spike-gpui/shots/$1.png; }
k Tab; shot c12-01-tab-focus; k space
k Tab; k Return
k Tab; $D type "typed after tab"; sleep 0.3; shot c12-04-input-focused-by-tab
k Tab; k space
k Tab; shot c12-05-raw-div-focus; k space
k shift+Tab; k Return
k Tab; k Tab; shot c12-06-server-tile-focus; k space; sleep 0.5; shot c12-02-menu-by-focus-and-space; k Down; k Down; k Return
k ctrl+k; k ctrl+k
k ctrl+shift+m; sleep 0.5; shot c12-03-shortcut-and-menu; k Down; k Down; k Down; k Return
sleep 0.3; shot c12-07-final
grep -E "keyboard|Cmd" /srv/junto-spike-gpui/run/events.txt | cut -c1-120
