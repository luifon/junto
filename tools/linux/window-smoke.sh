#!/bin/sh
# Start the real client only on the virtual displays created by display.sh.
set -eu
out=${1:?usage: window-smoke.sh output-directory}
mkdir -p "$out"
case "${WAYLAND_DISPLAY:-}:${DISPLAY:-}" in
    wayland-*:) backend=wayland ;;
    :\:99) backend=x11 ;;
    *) echo 'a private test display is required' >&2; exit 2 ;;
esac
renderer=${JUNTO_LINUX_TEST_RENDERER:-default}
case "$renderer" in default|opengl|mesa22) ;; *) echo 'unknown test renderer' >&2; exit 2 ;; esac
if [ "$backend" = x11 ] && [ "$renderer" = opengl ]; then
    # Keep the explicit fallback separate from checks of the normal renderer selection.
    export VK_ICD_FILENAMES=/dev/null LIBGL_ALWAYS_SOFTWARE=1
fi
if [ "$renderer" = mesa22 ]; then
    # The only Vulkan driver is the software driver of Mesa 22.3.6 (ADR 0705).
    export VK_ICD_FILENAMES=/opt/junto-linux/mesa22/lvp_icd.json
fi
/target/debug/junto --no-activate --data-dir "/tmp/junto-linux-window-$backend" >"$out/$backend.log" 2>&1 &
client_pid=$!
trap 'kill "$client_pid" 2>/dev/null || true; wait "$client_pid" 2>/dev/null || true' EXIT
trap 'exit 130' INT TERM
ready() {
    if [ "$backend" = x11 ]; then
        xdotool search --onlyvisible --pid "$client_pid" >"$out/x11-windows.txt" 2>/dev/null
    else
        swaymsg -t get_tree >"$out/wayland-tree.json"
        jq -e '.. | objects | select(.app_id? == "chat.junto.Junto") | select(.visible == true)' \
            "$out/wayland-tree.json" >/dev/null
    fi
}
attempts=0
until ready; do
    attempts=$((attempts + 1))
    if [ "$attempts" -ge 60 ] || ! kill -0 "$client_pid" 2>/dev/null; then
        cat "$out/$backend.log" >&2
        exit 1
    fi
    sleep 0.5
done
# Mapping a surface precedes its first presented frame, by a time that depends on the load.
painted() {
    if [ "$backend" = x11 ]; then
        python3 - "$out/x11-window.png" <<'PY'
import sys
from PIL import ImageGrab
ImageGrab.grab(xdisplay=':99').save(sys.argv[1])
PY
    else
        grim "$out/wayland-window.png"
    fi
    python3 - "$out/$backend-window.png" <<'PY'
import sys
from PIL import Image
picture = Image.open(sys.argv[1]).convert('RGB')
# The welcome dialog must paint an application surface, not only a window frame.
painted = sum(count for count, colour in picture.getcolors(picture.width * picture.height)
              if max(abs(value - target) for value, target in zip(colour, (242, 234, 218))) <= 1)
sys.exit(0 if painted > 1000 else 1)
PY
}
attempts=0
until painted; do
    attempts=$((attempts + 1))
    if [ "$attempts" -ge 60 ] || ! kill -0 "$client_pid" 2>/dev/null; then
        cat "$out/$backend.log" >&2
        echo 'the mapped window painted no application surface' >&2
        exit 1
    fi
    sleep 0.5
done
# The client still runs after it painted.
sleep 1
kill -0 "$client_pid"
echo "$backend: the real client painted a visible window and stayed running"
