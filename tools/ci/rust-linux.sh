#!/bin/sh
# The job "Rust on Linux" (ADR 0905). The Rust steps run in the Linux build image of ADR 0750
# through tools/linux-container.sh: the system libraries of the client, clang for the call
# library, PipeWire for the sound tests and ffmpeg for the tests of recordings. Each command
# gets a new PostgreSQL and LiveKit server; Cargo's registry and target directory stay in
# Docker volumes between the commands. Run from any directory of a clone of the repository.
set -eu
cd "$(dirname "$0")/../.."
. tools/ci/pins.sh

step() { printf '\n=== %s\n' "$1"; }

step "Build the Linux image and start its services"
tools/linux-container.sh true
step "Format"
tools/linux-container.sh cargo fmt --all --check
step "Lint"
tools/linux-container.sh cargo clippy --workspace --all-targets --locked --keep-going -- -D warnings
# The device checks of the media crate and the native code that only they select, which the
# workspace lint does not compile (ADR 0913).
step "Lint the device checks of the media crate"
tools/linux-container.sh cargo clippy --locked -p junto-media --features native-device-tests --all-targets -- -D warnings
step "Build the server used by process tests"
tools/linux-container.sh cargo build --workspace --locked
# The V4L2 device in memory of the camera tests is not linked into the client (ADR 0913); the
# camera code itself is.
step "The client holds the camera code and not the test camera device"
tools/linux-container.sh sh -c 'llvm-nm-22 /target/debug/junto >/tmp/symbols && grep -q junto_camera_open /tmp/symbols && ! grep -q junto_fake_ /tmp/symbols'
step "Tests with PostgreSQL"
tools/linux-container.sh cargo test --workspace --locked
# Every screen of junto-screens: no empty band in an area, no control past an edge of the
# window, and each screen settles within its bound (ADR 0730, ADR 0731). It measures and
# writes no picture. The second run draws every screen at the least size of the window.
step "Screens without empty bands"
tools/linux-container.sh sh -c 'JUNTO_SCREENS_FILES=0 cargo run --locked -p junto-client --features test-support --bin junto-screens -- /tmp/junto-screens'
step "Screens at the least size of the window"
tools/linux-container.sh sh -c 'JUNTO_SCREENS_FILES=0 JUNTO_SCREENS_SIZE=720x480 cargo run --locked -p junto-client --features test-support --bin junto-screens -- /tmp/junto-screens'
# A panic in a task of the window thread, on the event loop of X11 and of Wayland: the client
# continues, writes the report and states it (ADR 0757).
step "Panics in the callbacks of the platform"
tools/linux-container.sh tools/linux/display.sh x11 sh -c 'JUNTO_TEST_NATIVE_PANICS=1 cargo test --locked -p junto-client --test native_panics'
tools/linux-container.sh tools/linux/display.sh wayland sh -c 'JUNTO_TEST_NATIVE_PANICS=1 cargo test --locked -p junto-client --test native_panics'
# The camera unit tests with the counts that the device checks read (ADR 0913); they open no
# device.
step "Camera unit tests of the device checks"
tools/linux-container.sh cargo test --locked -p junto-media --features native-device-tests --lib camera::
# The production sound host against virtual PipeWire devices, and the trust of the call
# library in a TURN server over TLS. Both need the isolated container (ADR 0750).
step "Linux sound devices and TURN over TLS"
tools/linux-container.sh cargo test --locked -p junto-media --test linux_audio --test tls_trust -- --ignored
step "CI runner unit tests"
tools/linux-container.sh python3 -m unittest discover -s tools/ci -p "test_*.py"
step "Container runner unit tests"
tools/linux-container.sh python3 -m unittest discover -s tools/linux -p "test_*.py"
step "Packaging unit tests"
tools/linux-container.sh python3 -m unittest discover -s packaging -p "test_*.py"
step "Test service wrapper failure paths"
tools/linux-container.sh python3 tools/test_test_services.py
# The tarball and the AppImage of the client: their contents, the rule of the bundled
# libraries, the first frame of each on a virtual display, and an update and a rollback of
# the directory package while its program runs (ADR 0754).
step "Linux packages"
tools/linux-container.sh python3 packaging/check_linux.py
step "The committed API documents equal the generated ones"
tools/linux-container.sh cargo run --locked -p junto-server --bin junto-openapi
tools/linux-container.sh git diff --exit-code -- docs/api
