#!/bin/sh
# Internal entry point; its caller holds tools/serial.sh.
set -eu
if [ -e "$1" ]; then echo 'the output directory must not exist' >&2; exit 2; fi
mkdir "$1"
release_output=$1
shift
completed=false
cleanup() {
    if [ "$completed" = false ]; then python3 -c 'import shutil,sys; shutil.rmtree(sys.argv[1])' "$release_output"; fi
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM
cargo build --locked --release -p junto-client --bin junto -p junto-client-core --bin junto-release --bin junto-recover
python3 packaging/bundle.py target/release/junto "$release_output/.bundle/Junto.app" target/release/junto-recover
packaging/sign.sh "$release_output/.bundle/Junto.app"
architecture=$(uname -m)
if [ "$architecture" = arm64 ]; then architecture=aarch64; fi
if [ -n "${JUNTO_TEST_UPDATE_KEYS_FILE:-}${JUNTO_TEST_FRESHNESS_KEYS_FILE:-}${JUNTO_TEST_CERTIFICATE_FILE:-}${JUNTO_TEST_BUILD_VERSION:-}" ]; then
    target/release/junto-release --test-configuration publish macos bundle "$architecture" "$release_output/.bundle" "$release_output" "$JUNTO_UPDATE_KEY" "$JUNTO_FRESHNESS_KEY" "$@"
else
    target/release/junto-release publish macos bundle "$architecture" "$release_output/.bundle" "$release_output" "$JUNTO_UPDATE_KEY" "$JUNTO_FRESHNESS_KEY" "$@"
fi
mv "$release_output/.bundle/Junto.app" "$release_output/Junto.app"
rmdir "$release_output/.bundle"
version=$(python3 -c 'import plistlib,sys; print(plistlib.load(open(sys.argv[1], "rb"))["CFBundleShortVersionString"])' "$release_output/Junto.app/Contents/Info.plist")
# The first install is the application alone; an update keeps its own record (ADR 0726).
/usr/bin/ditto -c -k --sequesterRsrc --keepParent "$release_output/Junto.app" "$release_output/Junto-$version-macos-$architecture.zip"

completed=true
