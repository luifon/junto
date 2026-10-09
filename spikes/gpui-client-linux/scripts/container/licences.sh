#!/bin/bash
cd /work/cargo/registry/src/*/
for d in */; do
  l=$(grep -m1 -E '^license(-file)? *=' "$d/Cargo.toml" | sed 's/^[^=]*= *//; s/"//g')
  echo "${d%/}|$l"
done > /work/licences-linux.txt
echo "packages: $(wc -l < /work/licences-linux.txt)"
echo "== licence expressions, count"
cut -d'|' -f2 /work/licences-linux.txt | sort | uniq -c | sort -rn
echo "== containing GPL / MPL / LGPL / no licence field"
grep -Ei 'GPL|MPL|^[^|]*\|$|LICENSE' /work/licences-linux.txt
echo "== new for linux"
for n in ahash as-raw-xcb-connection calloop-wayland-source const-random const-random-macro downcast-rs filedescriptor gethostname hashbrown-0.14 is-docker is-wsl open scoped-tls tiny-keccak wayland-backend wayland-client wayland-cursor wayland-protocols wayland-protocols-plasma wayland-protocols-wlr wayland-scanner x11rb x11rb-protocol xcursor xim-ctext xim-parser xkbcommon xkeysym zed-xim gpui-pre-linux gpui-pre-wgpu; do grep -E "^$n-?[0-9v.]*[^|]*\|" /work/licences-linux.txt | head -2; done
