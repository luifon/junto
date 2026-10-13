#!/bin/sh
# The test backend accepts shared-memory output from the private Pixman compositor.
set -eu
portal_source=/tmp/junto-linux-portal-source
portal_archive=/tmp/junto-linux-portal-source.tar.gz
cleanup() { rm -rf "$portal_source"; rm -f "$portal_archive"; }
trap cleanup EXIT
trap 'exit 130' INT TERM
curl --fail --location --retry 3 \
    https://github.com/emersion/xdg-desktop-portal-wlr/releases/download/v0.7.1/xdg-desktop-portal-wlr-0.7.1.tar.gz \
    --output "$portal_archive"
printf '%s  %s\n' eec6e4be808e1a445e677dba1e20e5acb2f091825f5ff4c6ac49d5843b2185f9 "$portal_archive" | sha256sum --check -
mkdir "$portal_source"
tar -xzf "$portal_archive" -C "$portal_source" --strip-components=1
# A valid shared-memory format suffices when no DMA-BUF format is available.
# This patch belongs only to the container test backend; it still requires valid pixels.
python3 - "$portal_source/src/screencast/screencast.c" <<'PY'
from pathlib import Path
import sys
file = Path(sys.argv[1])
original = '''if (cast->screencopy_frame_info[WL_SHM].format == DRM_FORMAT_INVALID ||
\t\t\t(cast->ctx->state->screencast_version >= 3 &&
\t\t\t cast->screencopy_frame_info[DMABUF].format == DRM_FORMAT_INVALID)) {'''
replacement = '''if (cast->screencopy_frame_info[WL_SHM].format == DRM_FORMAT_INVALID) {'''
source = file.read_text()
assert source.count(original) == 1, 'the pinned portal source no longer matches the shared-memory patch'
file.write_text(source.replace(original, replacement))
PY
CC=gcc meson setup "$portal_source/build" "$portal_source" --prefix=/usr --buildtype=release \
    --libexecdir=libexec -Dsystemd=disabled -Dman-pages=disabled -Dsd-bus-provider=libsystemd
meson compile -C "$portal_source/build" -j 4
meson install -C "$portal_source/build"
