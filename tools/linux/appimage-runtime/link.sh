#!/bin/sh
# Links the AppImage runtime from the files of this directory alone, and gives it the mark of a
# type 2 AppImage (ADR 0754, ADR 0801). build.sh writes this directory and builds the runtime
# with this script; to link the runtime with a changed libfuse, replace libfuse3.a (README.md
# beside the directory relink).
#
# Usage: link.sh <output>
# Needs GNU ld and strip (binutils) for the architecture of the objects, and dd.
set -eu
if [ "$#" -ne 1 ]; then
    echo "usage: $0 <output>" >&2
    exit 2
fi
case $1 in
    /*) output=$1 ;;
    *) output=$(pwd)/$1 ;;
esac
cd "$(dirname -- "$0")"
# The order of the files is the order of the musl-gcc link of the runtime: the start files of
# musl and of compiler-rt, the runtime, its libraries, the builtins of compiler-rt with the C
# library, and the end files.
ld -static -nostdlib --gc-sections -o "$output" \
    Scrt1.o crti.o clang_rt.crtbegin.o \
    runtime.o libsquashfuse.a libsquashfuse_ll.a libzstd.a libz.a libfuse3.a libmimalloc.a \
    --start-group libclang_rt.builtins.a libc.a --end-group \
    clang_rt.crtend.o crtn.o -T data_sections.ld
strip --strip-debug --strip-unneeded "$output"
# The mark of a type 2 AppImage at byte 8, after strip.
printf 'AI\002' | dd of="$output" bs=1 count=3 seek=8 conv=notrunc 2>/dev/null
