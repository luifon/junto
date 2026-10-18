# The source of the AppImage runtime

The Junto AppImage file starts with its runtime: the program that mounts the SquashFS image that follows it and starts `junto`. This directory holds the complete source of that runtime and the script that built it. The Linux build image of Junto (`tools/linux/Dockerfile` in the Junto repository) runs the same script on the same files.

## Components

The runtime is one program, linked statically from these parts. Each archive in `sources/` is the unchanged file that its project publishes; `SHA256SUMS` holds the SHA-256 of each.

| Component | Archive | Licence |
|---|---|---|
| The type 2 runtime of the AppImage project, commit `dd6cebedcbddde9c82f89b011e8e1d40b6e43868` (release 20251108) | `type2-runtime-dd6cebedcbddde9c82f89b011e8e1d40b6e43868.tar.gz` | MIT |
| libfuse 3.15.0, with the change `patches/libfuse/mount.c.diff` of the runtime archive | `fuse-3.15.0.tar.xz` | LGPL-2.1 (the library, `lib/` and `include/`) |
| squashfuse 0.5.2 | `squashfuse-0.5.2.tar.gz` | BSD-2-Clause |
| zstd 1.5.6 | `zstd-1.5.6.tar.gz` | BSD-3-Clause (one of its two licences) |
| zlib 1.3.1 | `zlib-1.3.1.tar.gz` | Zlib |
| mimalloc 2.1.7 | `mimalloc-2.1.7.tar.gz` | MIT |
| musl 1.2.3, the C library | Debian 12 package `musl-dev` 1.2.3-1 | MIT |
| libgcc of GCC 12.2 | Debian 12 package `libgcc-12-dev` 12.2.0-14+deb12u1 | GPL-3.0 with the GCC Runtime Library Exception |

The licence texts are in the archives and in `third-party-licences.txt` beside this directory.

## Build the runtime

On Debian 12 (bookworm) with these packages:

```
gcc 4:12.2.0-3 (gcc-12 12.2.0-14+deb12u1), musl-tools 1.2.3-1, musl-dev 1.2.3-1, musl 1.2.3-1,
meson 1.0.1-5, ninja-build 1.11.1-2~deb12u1, make 4.3-4.1, autoconf 2.71-3,
automake 1:1.16.5-1.3, libtool 2.4.7-7~deb12u1, pkg-config 1.8.1-1, patch 2.7.6-7, xz-utils 5.4.1-1+deb12u2
```

run:

```sh
sh build.sh /tmp/runtime
```

With these versions, `/tmp/runtime` is the same file, byte for byte, as the runtime of the AppImage. Other versions of the tools build a working runtime that can differ in its bytes.

## Build it with a changed libfuse

1. Unpack `sources/fuse-3.15.0.tar.xz`, change its files, and pack the directory `fuse-3.15.0` again into `sources/fuse-3.15.0.tar.xz`.
2. Write the new SHA-256 of the archive into `SHA256SUMS`: `(cd sources && sha256sum fuse-3.15.0.tar.xz)`.
3. Run `sh build.sh /tmp/runtime`.

## Put a runtime into the AppImage

The SquashFS image starts where the runtime ends. `./Junto.AppImage --appimage-offset` prints that position. With a new runtime in `/tmp/runtime`:

```sh
offset=$(./Junto.AppImage --appimage-offset)
cat /tmp/runtime > Junto-changed.AppImage
tail -c +"$((offset + 1))" Junto.AppImage >> Junto-changed.AppImage
chmod +x Junto-changed.AppImage
```
