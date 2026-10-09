#!/bin/bash
# inside the tools container: private X server, private D-Bus session, AT-SPI bus with accessibility enabled
export XDG_RUNTIME_DIR=/tmp/xdg HOME=/tmp/home
mkdir -p $XDG_RUNTIME_DIR $HOME && chmod 700 $XDG_RUNTIME_DIR
if [ ! -S $XDG_RUNTIME_DIR/bus ]; then
  dbus-daemon --session --address=unix:path=$XDG_RUNTIME_DIR/bus --fork --print-pid > /tmp/dbus.pid
fi
export DBUS_SESSION_BUS_ADDRESS=unix:path=$XDG_RUNTIME_DIR/bus
if ! pgrep -x Xvfb >/dev/null; then
  setsid -f Xvfb :77 -screen 0 1920x1080x24 -nolisten tcp > /tmp/xvfb.log 2>&1
  sleep 1
fi
export DISPLAY=:77
if ! pgrep -f at-spi-bus-launcher >/dev/null; then
  setsid -f /usr/libexec/at-spi2/at-spi-bus-launcher --a11y=1 --screen-reader=1 > /tmp/atspi.log 2>&1
  sleep 1
fi
busctl --user get-property org.a11y.Bus /org/a11y/bus org.a11y.Status IsEnabled
xdpyinfo 2>/dev/null | grep dimensions || echo "X :77 up: $(pgrep -x Xvfb)"
