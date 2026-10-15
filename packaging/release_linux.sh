#!/bin/sh
# Build the Linux packages of a release in the Linux build image (ADR 0754): the tarball, the
# AppImage and the source directory of the update archive of each. Nothing is signed and
# nothing is published; the release tool publishes the two source directories
# (packaging/README.md).
set -eu
if [ -n "${JUNTO_TEST_UPDATE_KEYS_FILE:-}${JUNTO_TEST_FRESHNESS_KEYS_FILE:-}${JUNTO_TEST_CERTIFICATE_FILE:-}${JUNTO_TEST_BUILD_VERSION:-}" ]; then
    echo 'test release configuration is refused by the release entry point' >&2
    exit 2
fi
: "${JUNTO_UPDATE_SOURCE:?set the HTTPS base address of releases}"
if [ "$#" -ne 1 ]; then
    echo 'usage: packaging/release_linux.sh <new output directory, relative to the checkout>' >&2
    exit 2
fi
cd "$(dirname "$0")/.."
case $1 in
    /*|*..*) echo 'the output directory is a path inside the checkout' >&2; exit 2 ;;
esac
if [ -e "$1" ]; then
    echo 'the output directory must not exist' >&2
    exit 2
fi
version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
exec tools/linux-container.sh sh -c '
    set -eu
    cargo build --locked --release -p junto-client --bin junto -p junto-client-core --bin junto-recover
    python3 packaging/linux.py /target/release/junto /target/release/junto-recover "$1" "$2"
' sh "$version" "$1"
