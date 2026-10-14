#!/bin/sh
# Run the Linux capture tests with the compiler's address checking
# (crates/media/README.md). The toolchain and the build stay inside the container.
set -eu
toolchain=nightly-2026-10-01
work=/tmp/junto-linux-address-check
target=$(rustc -vV | sed -n 's/^host: //p')
export RUSTUP_HOME="$work/rustup"
rustup toolchain install "$toolchain" --profile minimal
export CARGO_TARGET_DIR="$work/target"
export RUSTFLAGS="${RUSTFLAGS:-} -Zsanitizer=address"
# PipeWire keeps process-wide allocations until the process ends.
export ASAN_OPTIONS=detect_leaks=0
export ASAN_SYMBOLIZER_PATH=/usr/lib/llvm-22/bin/llvm-symbolizer
# The sound capture always; the picture capture when portal.sh started the virtual portal.
set -- share::linux::audio::tests
if [ "${JUNTO_TEST_VIRTUAL_PORTAL:-}" = 1 ]; then
    set -- "$@" share::linux::live
fi
exec cargo "+$toolchain" test --target "$target" -p junto-media --features portal-tests --lib \
    -- --ignored --test-threads=1 "$@"
