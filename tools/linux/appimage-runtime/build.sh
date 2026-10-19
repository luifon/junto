#!/bin/sh
# Builds the type 2 runtime of an AppImage from the archives in sources/ beside this file
# (ADR 0754, ADR 0801). The steps follow the release build of the AppImage project at the
# same commit (scripts/common/install-dependencies.sh, src/runtime/Makefile and
# scripts/build-runtime.sh), with the musl C library of Debian 12 in place of Alpine Linux and
# the builtins of LLVM's compiler-rt in place of libgcc. The link is a step of its own:
# link.sh links the runtime from the files of a relink directory, which holds every file of the
# link, so that the runtime can be linked again with a changed libfuse.
#
# Usage: build.sh <output> <relink directory>
# Needs, from Debian 12: gcc, musl-tools, clang-22 with libclang-rt-22-dev, meson, ninja-build,
# make, autoconf, automake, libtool, pkg-config, patch, xz-utils, binutils. The versions of the
# Junto Linux build image are in README.md; with them the output is the same file, byte for
# byte, as the runtime of the AppImage that holds this directory.
set -eu
if [ "$#" -ne 2 ]; then
    echo "usage: $0 <output> <relink directory>" >&2
    exit 2
fi
here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
absolute() {
    case $1 in
        /*) printf '%s\n' "$1" ;;
        *) printf '%s\n' "$(pwd)/$1" ;;
    esac
}
output=$(absolute "$1")
relink=$(absolute "$2")
mkdir "$relink"
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

# The compile command of src/runtime/Makefile, with the include directories of this build.
# The Makefile names clang; GCC also reports that the sprintf of portable_option can write past
# its buffer of PATH_MAX bytes, which clang does not report, and that report stays a warning.
cd "type2-runtime-$runtime_commit/src/runtime"
printf 'https://github.com/AppImage/type2-runtime/commit/%s' "$(printf %s "$runtime_commit" | cut -c1-7)" > version
$CC -I"$prefix/include/squashfuse" -I"$prefix/include/fuse3" -I"$prefix/include" \
    -std=gnu99 -Os -D_FILE_OFFSET_BITS=64 -DGIT_COMMIT="\"$(cat version)\"" \
    -ffunction-sections -fdata-sections -Wall -Werror -Wno-error=format-overflow $flags \
    -c runtime.c -o runtime.o

# Every file of the link: the runtime, its libraries, the start files and the C library of
# musl, and the builtins and start files of compiler-rt, which give the functions that musl
# takes from the compiler's library (on arm64 the arithmetic of 128-bit floating point).
musl=/usr/lib/$(uname -m)-linux-musl
target=$(uname -m)-linux-gnu
cp runtime.o data_sections.ld "$relink/"
for library in squashfuse squashfuse_ll zstd z fuse3 mimalloc; do
    cp "$prefix/lib/lib$library.a" "$relink/"
done
cp "$musl/Scrt1.o" "$musl/crti.o" "$musl/crtn.o" "$musl/libc.a" "$relink/"
builtins=$(clang-22 --target="$target" --rtlib=compiler-rt -print-libgcc-file-name)
cp "$builtins" "$relink/libclang_rt.builtins.a"
# The start files of compiler-rt are beside its builtins, with the architecture in their name
# (clang_rt.crtbegin-aarch64.o) or without it, as the builtins are.
for name in crtbegin crtend; do
    case $(basename "$builtins") in
        libclang_rt.builtins.a) file=$(dirname "$builtins")/clang_rt.$name.o ;;
        *) file=$(dirname "$builtins")/clang_rt.$name-$(uname -m).o ;;
    esac
    cp "$file" "$relink/clang_rt.$name.o"
done
cp "$here/link.sh" "$relink/link.sh"
sh "$relink/link.sh" "$output"
