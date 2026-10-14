#!/bin/sh
# Build and sign a macOS release into a new directory. Nothing is published.
set -eu
if [ -n "${JUNTO_TEST_UPDATE_KEYS_FILE:-}${JUNTO_TEST_FRESHNESS_KEYS_FILE:-}${JUNTO_TEST_CERTIFICATE_FILE:-}${JUNTO_TEST_BUILD_VERSION:-}" ]; then
    echo 'test release configuration is only accepted by the internal test entry point' >&2
    exit 2
fi
case ${JUNTO_CODESIGN_IDENTITY:-} in
    ''|-) echo 'a code-signing certificate is required; ad-hoc releases are refused' >&2; exit 2 ;;
esac
: "${JUNTO_UPDATE_SOURCE:?set the HTTPS base address of releases}"
: "${JUNTO_UPDATE_KEY:?set the external release signing key path}"
: "${JUNTO_FRESHNESS_KEY:?set the external freshness signing key path}"
: "${JUNTO_CODESIGN_KEYCHAIN:?set the code-signing keychain path}"
if [ "$#" -lt 1 ]; then
    echo 'usage: packaging/release.sh <new output directory> [additional release key paths...]' >&2
    exit 2
fi
cd "$(dirname "$0")/.."
if [ -e "$1" ]; then
    echo 'the output directory must not exist' >&2
    exit 2
fi
exec tools/serial.sh sh packaging/release_locked.sh "$@"
