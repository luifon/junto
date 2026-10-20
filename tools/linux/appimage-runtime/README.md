# The source of the AppImage runtime

The Junto AppImage file starts with its runtime: the program that mounts the SquashFS image that follows it and starts `junto`. This directory holds the complete source of that runtime and of the libraries built for it, the script that built it, and the directory `relink` with every file of the link of the runtime and the script `link.sh` that links them. The Linux build image of Junto (`tools/linux/Dockerfile` in the Junto repository) runs the same script on the same files.

## Components

The runtime is one program, linked statically from these parts. Each archive in `sources/` is the unchanged file that its project publishes; `SHA256SUMS` holds the SHA-256 of each.

| Component | Source | Licence |
|---|---|---|
| The type 2 runtime of the AppImage project, commit `dd6cebedcbddde9c82f89b011e8e1d40b6e43868` (release 20251108) | `type2-runtime-dd6cebedcbddde9c82f89b011e8e1d40b6e43868.tar.gz` | MIT |
| libfuse 3.15.0, with the change `patches/libfuse/mount.c.diff` of the runtime archive | `fuse-3.15.0.tar.xz` | LGPL-2.1 (the library, `lib/` and `include/`) |
| squashfuse 0.5.2 | `squashfuse-0.5.2.tar.gz` | BSD-2-Clause |
| zstd 1.5.6 | `zstd-1.5.6.tar.gz` | BSD-3-Clause (one of its two licences) |
| zlib 1.3.1 | `zlib-1.3.1.tar.gz` | Zlib |
| mimalloc 2.1.7 | `mimalloc-2.1.7.tar.gz` | MIT |
| musl 1.2.3, the C library | the object files in `relink`, from the Debian 12 package `musl-dev` 1.2.3-1 | MIT |
| The builtins and start files of compiler-rt of LLVM 22.1.8 | the object files in `relink`, from the Debian 12 package `libclang-rt-22-dev` 1:22.1.8-1~deb12u1 | Apache-2.0 WITH LLVM-exception |

The licence texts are in the archives and in `third-party-licences.txt` beside this directory.

## The files of the link

`relink/link.sh` links the runtime from the files of `relink` alone, in this order, with `ld -static -nostdlib --gc-sections`, and then removes its debug data with `strip` and writes the mark of a type 2 AppImage at byte 8:

| File | What it is |
|---|---|
| `Scrt1.o`, `crti.o`, `crtn.o` | the start and end files of musl |
| `clang_rt.crtbegin.o`, `clang_rt.crtend.o` | the start and end files of compiler-rt |
| `runtime.o` | `src/runtime/runtime.c` of the runtime archive, compiled |
| `libsquashfuse.a`, `libsquashfuse_ll.a` | squashfuse |
| `libzstd.a` | zstd |
| `libz.a` | zlib |
| `libfuse3.a` | libfuse, with the change of the AppImage project |
| `libmimalloc.a` | mimalloc |
| `libclang_rt.builtins.a` | the builtins of compiler-rt: the functions that musl takes from the library of the compiler, such as the arithmetic of 128-bit floating point on arm64 |
| `libc.a` | musl |
| `data_sections.ld` | `src/runtime/data_sections.ld` of the runtime archive: the sections of the AppImage format |

The runtime is a static program, not a position-independent one: the `musl-gcc` of Debian 12 links `-static-pie` as `-static`.

## Link the runtime with a changed libfuse

`link.sh` needs only GNU `ld` and `strip` (binutils) for the architecture of the files, and `dd`.

1. Unpack `sources/fuse-3.15.0.tar.xz`, apply `patches/libfuse/mount.c.diff` of the runtime archive if you start from the unchanged source, and change its files.
2. Build its static library for musl, as `build.sh` does:

   ```sh
   CC=musl-gcc meson setup --prefix=/tmp/fuse --libdir=lib --buildtype=release --default-library=static \
       -Dutils=false -Dexamples=false -Dtests=false build-fuse fuse-3.15.0
   ninja -C build-fuse install
   ```

3. Copy `/tmp/fuse/lib/libfuse3.a` over `relink/libfuse3.a` and run `sh relink/link.sh /tmp/runtime`.

## Build everything from the source

On Debian 12 (bookworm) with these packages:

```
gcc 4:12.2.0-3 (gcc-12 12.2.0-14+deb12u1), binutils 2.40-2, musl-tools 1.2.3-1, musl-dev 1.2.3-1, musl 1.2.3-1,
clang-22 and libclang-rt-22-dev 1:22.1.8-1~deb12u1, meson 1.0.1-5, ninja-build 1.11.1-2~deb12u1, make 4.3-4.1,
autoconf 2.71-3, automake 1:1.16.5-1.3, libtool 2.4.7-7~deb12u1, pkg-config 1.8.1-1, patch 2.7.6-7,
xz-utils 5.4.1-1+deb12u2
```

run:

```sh
sh build.sh /tmp/runtime /tmp/relink
```

With these versions, `/tmp/runtime` is the same file, byte for byte, as the runtime of the AppImage, and `/tmp/relink` holds the same files as `relink`. Other versions of the tools build a working runtime that can differ in its bytes.

## Put a runtime into the AppImage

The SquashFS image starts where the runtime ends. `./Junto.AppImage --appimage-offset` prints that position. With a new runtime in `/tmp/runtime`:

```sh
offset=$(./Junto.AppImage --appimage-offset)
cat /tmp/runtime > Junto-changed.AppImage
tail -c +"$((offset + 1))" Junto.AppImage >> Junto-changed.AppImage
chmod +x Junto-changed.AppImage
```
