#!/bin/sh
# Checks the condition waits of the media crate on macOS (ADR 0786), against the WebRTC
# library that the build of this checkout links. Build the media crate first:
#
#     tools/serial.sh cargo build -p junto-media
#
# 1. One blocking call of the WebRTC library whose wait is refused with EINVAL returns
#    before its task ran when the library is used as it is.
# 2. The same call returns after its task with crates/media/src/condition_wait.rs linked in.
# 3. Each program given as an argument (for example target/debug/junto) defines
#    pthread_cond_wait and pthread_cond_timedwait itself and does not take them from the
#    system library. The check prints which condition functions the program takes from
#    the system.
set -eu

if [ "$(uname)" != Darwin ]; then
    echo "this check is for macOS only" >&2
    exit 0
fi

root=$(cd "$(dirname "$0")/.." && pwd)
archive=$(find "$root/target" -type f -name libwebrtc.a -path '*livekit_webrtc*' 2>/dev/null | head -n 1)
if [ -z "$archive" ]; then
    echo "no libwebrtc.a under $root/target: build the media crate first" >&2
    exit 2
fi
webrtc=$(dirname "$(dirname "$archive")")
sources="$root/crates/media/native-check"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

flags="-std=c++20 -DWEBRTC_POSIX -DWEBRTC_MAC -DNDEBUG -Wno-nullability-completeness"
includes="-I$webrtc/include -I$webrtc/include/third_party/abseil-cpp"
frameworks="-framework Foundation -framework CoreFoundation"

clang -dynamiclib -o "$work/librefuse_once.dylib" "$sources/refuse_once.c"
# shellcheck disable=SC2086
clang++ $flags $includes "$sources/blocking_call.cc" "$work/librefuse_once.dylib" "$archive" \
    $frameworks -o "$work/as_it_is" 2>"$work/link.log" || { cat "$work/link.log" >&2; exit 2; }
rustc --edition 2024 --crate-type staticlib -C panic=abort \
    -o "$work/libcondition_wait.a" "$root/crates/media/src/condition_wait.rs"
# shellcheck disable=SC2086
clang++ $flags $includes "$sources/blocking_call.cc" "$work/libcondition_wait.a" \
    "$work/librefuse_once.dylib" "$archive" $frameworks -o "$work/with_the_crate" \
    2>"$work/link.log" || { cat "$work/link.log" >&2; exit 2; }

# The wait with a time limit, and the wait without one after the three-second warning of
# the library.
for wait in timed untimed; do
    status=0
    "$work/as_it_is" "$wait" >"$work/as_it_is.log" 2>&1 || status=$?
    if [ "$status" -ne 1 ] || ! grep -q "returned before its task ran" "$work/as_it_is.log"; then
        echo "FAIL: the library as it is did not show the fault for the $wait wait (exit $status):" >&2
        cat "$work/as_it_is.log" >&2
        exit 1
    fi
    echo "1. $wait wait, as it is: $(grep FAIL "$work/as_it_is.log")"

    status=0
    "$work/with_the_crate" "$wait" >"$work/with_the_crate.log" 2>&1 || status=$?
    if [ "$status" -ne 0 ] || ! grep -q "^PASS" "$work/with_the_crate.log"; then
        echo "FAIL: with condition_wait.rs the blocking call is not correct for the $wait wait (exit $status):" >&2
        cat "$work/with_the_crate.log" >&2
        exit 1
    fi
    echo "2. $wait wait, with condition_wait.rs: $(grep PASS "$work/with_the_crate.log")"
done

for program in "$@"; do
    symbols=$(nm -m "$program" | grep ' _pthread_cond_' || true)
    for name in _pthread_cond_wait _pthread_cond_timedwait; do
        defined=$(printf '%s\n' "$symbols" | grep -c "(__TEXT,__text) external $name\$" || true)
        taken=$(printf '%s\n' "$symbols" | grep -c "(undefined) external $name (from" || true)
        if [ "$defined" -ne 1 ] || [ "$taken" -ne 0 ]; then
            echo "FAIL: $program does not define $name for all its code:" >&2
            printf '%s\n' "$symbols" >&2
            exit 1
        fi
    done
    echo "3. $program defines both condition waits; its other condition functions:"
    printf '%s\n' "$symbols" | sed 's/^ */     /'
done
