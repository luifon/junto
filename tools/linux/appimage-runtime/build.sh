#!/bin/sh
# Builds the type 2 runtime of an AppImage from the archives in sources/ beside this file
# (ADR 0754, ADR 0801). The steps follow the release build of the AppImage project at the
# same commit (scripts/common/install-dependencies.sh, src/runtime/Makefile and
# scripts/build-runtime.sh), with the musl C library of Debian 12 in place of Alpine Linux.
#
# Usage: build.sh <output>
# Needs, from Debian 12: gcc, musl-tools, meson, ninja-build, make, autoconf, automake, libtool,
# pkg-config, patch, xz-utils. The versions of the Junto Linux build image are in
# README.md; with them the output is the same file, byte for byte, as the runtime of the
# AppImage that holds this directory.
set -eu
if [ "$#" -ne 1 ]; then
    echo "usage: $0 <output>" >&2
    exit 2
fi
here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
case $1 in
    /*) output=$1 ;;
    *) output=$(pwd)/$1 ;;
esac
(cd "$here/sources" && sha256sum --quiet --strict -c ../SHA256SUMS)

runtime_commit=dd6cebedcbddde9c82f89b011e8e1d40b6e43868
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
prefix=$work/prefix
cd "$work"
for archive in "$here"/sources/*; do
    tar -xf "$archive"
done

export CC=musl-gcc
# The same paths in every build directory, so that the output does not depend on it.
flags="-ffile-prefix-map=$work=."
# On arm64, GCC calls atomic operations through functions of libgcc that read the processor
# features with __getauxval of the GNU C library, which musl does not have.
if [ "$(uname -m)" = aarch64 ]; then
    flags="$flags -mno-outline-atomics"
fi
export CFLAGS="-ffunction-sections -fdata-sections -Os $flags"
export PKG_CONFIG_LIBDIR="$prefix/lib/pkgconfig"
export PKG_CONFIG_PATH=""
export SOURCE_DATE_EPOCH=0

# libfuse with the change of the AppImage project: fusermount is the program that
# FUSERMOUNT_PROG names.
(cd fuse-3.15.0 && patch -p1 < "../type2-runtime-$runtime_commit/patches/libfuse/mount.c.diff")
meson setup --prefix="$prefix" --libdir=lib --buildtype=release --default-library=static \
    -Dutils=false -Dexamples=false -Dtests=false build-fuse fuse-3.15.0
ninja -C build-fuse install

(cd zlib-1.3.1 && ./configure --static --prefix="$prefix" && make -j"$(nproc)" install)

make -C zstd-1.5.6/lib -j"$(nproc)" install-static install-includes PREFIX="$prefix"

# mimalloc from its one-file source src/static.c, with the definitions and flags that its CMake
# file gives a static release build with GCC for musl (MI_LIBC_MUSL); the CMake file would also
# need a C++ compiler.
(
    cd mimalloc-2.1.7
    $CC $CFLAGS -O3 -DNDEBUG -DMI_LIBC_MUSL=1 -DMI_STATIC_LIB -fPIC -Wall -Wextra -Wno-unknown-pragmas \
        -fvisibility=hidden -Wstrict-prototypes -ftls-model=local-dynamic -fno-builtin-malloc -Iinclude \
        -c src/static.c -o mimalloc.o
    ar rcs "$prefix/lib/libmimalloc.a" mimalloc.o
)

(
    cd squashfuse-0.5.2
    ./autogen.sh
    ./configure --prefix="$prefix" --disable-shared --enable-static CPPFLAGS="-I$prefix/include" \
        LDFLAGS="-static -L$prefix/lib"
    make -j"$(nproc)"
    make install
    install -m 644 ./*.h "$prefix/include/squashfuse"
)

# The command of src/runtime/Makefile, with the include and library directories of this build.
# The Makefile names clang; GCC also reports that the sprintf of portable_option can write past
# its buffer of PATH_MAX bytes, which clang does not report, and that report stays a warning.
cd "type2-runtime-$runtime_commit/src/runtime"
printf 'https://github.com/AppImage/type2-runtime/commit/%s' "$(printf %s "$runtime_commit" | cut -c1-7)" > version
$CC -I"$prefix/include/squashfuse" -I"$prefix/include/fuse3" -I"$prefix/include" \
    -std=gnu99 -Os -D_FILE_OFFSET_BITS=64 -DGIT_COMMIT="\"$(cat version)\"" -T data_sections.ld \
    -ffunction-sections -fdata-sections -Wl,--gc-sections -static -Wall -Werror -Wno-error=format-overflow -static-pie $flags \
    runtime.c -L"$prefix/lib" \
    -lsquashfuse -lsquashfuse_ll -lzstd -lz -lfuse3 -lmimalloc -o runtime
strip --strip-debug --strip-unneeded runtime
# The mark of a type 2 AppImage at byte 8, after strip.
printf 'AI\002' | dd of=runtime bs=1 count=3 seek=8 conv=notrunc 2>/dev/null
cp runtime "$output"
