#!/bin/sh
# Explicit native media stages; ordinary test commands never call this runner.
set -eu
if [ "${JUNTO_TEST_NATIVE_DEVICES:-}" != 1 ]; then
    echo 'NOT RUN: set JUNTO_TEST_NATIVE_DEVICES=1 for an operator-supervised run' >&2
    exit 2
fi
case "${1:-}" in
    inspect|d0|request-microphone|request-camera|request-screen) ;;
    a1|a2|a3|c1|s1-s2|s3)
        : "${JUNTO_TEST_POSTGRES_CONTAINER:?set an isolated test container}"
        : "${JUNTO_TEST_LIVEKIT_CONTAINER:?set an isolated test container}"
        : "${JUNTO_TEST_WEBHOOK_ADDRESS:?set an isolated test address}"
        : "${JUNTO_TEST_LIVEKIT_PORT:?set an isolated test port}"
        : "${JUNTO_TEST_LIVEKIT_TCP_PORT:?set an isolated test port}"
        : "${JUNTO_TEST_LIVEKIT_UDP_PORT:?set an isolated test port}"
        ;;
    *) echo 'usage: tools/native-devices.sh inspect|d0|a1|a2|a3|c1|s1-s2|s3|request-microphone|request-camera|request-screen' >&2; exit 2 ;;
esac
if [ "$(uname -s)" != Darwin ]; then
    echo 'NOT RUN: native device stages require macOS' >&2
    exit 2
fi
here=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$here"
exec tools/serial.sh sh -eu -c '
    cargo build -p junto-media --example native-devices --features native-device-tests,test-support
    case "$1" in
        inspect|d0|request-*) exec target/debug/examples/native-devices "$1" ;;
    esac
    cargo build --workspace
    case "$1" in
        s1-s2|s3)
            swiftc crates/media/tests/native/Fixture.swift -o target/debug/native-fixture
            JUNTO_TEST_NATIVE_FIXTURE="$PWD/target/debug/native-fixture"
            export JUNTO_TEST_NATIVE_FIXTURE
            ;;
    esac
    exec tools/with-test-postgres.sh tools/with-test-livekit.sh target/debug/examples/native-devices "$1"
' native-devices "$1"
