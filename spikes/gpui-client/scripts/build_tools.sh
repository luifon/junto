#!/bin/sh
# Compiles the Swift test tools into out/bin. Run it through tools/serial.sh.
set -eu
here="$(cd "$(dirname "$0")/.." && pwd)"
mkdir -p "$here/out/bin"
for tool in input hold_key drag_source drag_dest windows ax_dump; do
    swiftc -O "$here/scripts/$tool.swift" -o "$here/out/bin/$tool"
done
