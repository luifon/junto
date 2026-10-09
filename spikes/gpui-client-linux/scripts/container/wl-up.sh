#!/bin/bash
# inside a tools container: starts a private D-Bus session and a headless labwc compositor
# usage: wl-up.sh [pixman|gles2|vulkan] [WIDTHxHEIGHT] [scale]
renderer="${1:-pixman}"
mode="${2:-1920x1080}"
scale="${3:-1}"
export XDG_RUNTIME_DIR=/tmp/xdg HOME=/tmp/home
mkdir -p $XDG_RUNTIME_DIR $HOME/.config/labwc && chmod 700 $XDG_RUNTIME_DIR
if [ ! -S $XDG_RUNTIME_DIR/bus ]; then
  dbus-daemon --session --address=unix:path=$XDG_RUNTIME_DIR/bus --fork --print-pid > /tmp/dbus.pid
fi
export DBUS_SESSION_BUS_ADDRESS=unix:path=$XDG_RUNTIME_DIR/bus
if ! pgrep -x labwc >/dev/null; then
  export WLR_BACKENDS=headless WLR_RENDERER=$renderer WLR_LIBINPUT_NO_DEVICES=1 WLR_HEADLESS_OUTPUTS=1
  [ "$renderer" != pixman ] && export WLR_RENDER_DRM_DEVICE=/dev/dri/renderD128
  setsid -f labwc > /tmp/labwc.log 2>&1
  for i in $(seq 1 40); do [ -S $XDG_RUNTIME_DIR/wayland-0 ] && break; sleep 0.5; done
fi
export WAYLAND_DISPLAY=wayland-0
wlr-randr --output HEADLESS-1 --custom-mode "$mode" --scale "$scale"
wlr-randr | grep -E "current|Scale"
grep -iE "renderer|EGL_VENDOR|GL_RENDERER|vulkan|error|Using" /tmp/labwc.log | head -12
