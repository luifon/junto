#!/bin/sh
# The job "Rust on Linux" (ADR 0905). The Rust steps run in the Linux build image of ADR 0750
# through tools/linux-container.sh: the system libraries of the client, clang for the call
# library, PipeWire for the sound tests and ffmpeg for the tests of recordings. Each command
# gets a new PostgreSQL and LiveKit server; Cargo's registry and target directory stay in
# Docker volumes between the commands. Run from any directory of a clone of the repository.
set -eu
cd "$(dirname "$0")/../.."
. tools/ci/pins.sh

# Every step runs also after a failed step, and every test program runs also after a failed
# one (--no-fail-fast), so that one run shows every failure of the commit; the job ends with the
# list of the failed commands and the exit status of the first (ADR 0905). Each step states its
# start, its end (UTC) and its duration, and the job ends with the table of the steps.
current=""
failed=""
first=0
began=0
times=""
utc() { date -u +%Y-%m-%dT%H:%M:%SZ; }
ended() {
    [ -n "$current" ] || return 0
    seconds=$(($(date +%s) - began))
    printf '=== %s: ended %s after %dm%02ds\n' "$current" "$(utc)" $((seconds / 60)) $((seconds % 60))
    times="$times
$(printf '%5dm%02ds  %s' $((seconds / 60)) $((seconds % 60)) "$current")"
}
step() {
    ended
    current=$1
    began=$(date +%s)
    printf '\n=== %s  %s\n' "$1" "$(utc)"
}
record() {
    [ "$first" -ne 0 ] || first=$1
    failed="$failed
$2"
}
run() {
    status=0
    "$@" || status=$?
    if [ "$status" -ne 0 ]; then
        [ "$1" != budget ] || shift
        record "$status" "$current (exit $status): $*"
    fi
}
# budget <command>: the command with the build processors and memory of tools/ci/budget.py,
# which adds the processors of the free worker locks of the host to a step that compiles.
budget() { python3 tools/ci/budget.py -- "$@"; }
# lanes: runs the plan on the standard input with tools/ci/lanes.py, its lanes at the same time
# when the budget allows, and records each failed command of the plan.
lanes() {
    plan=$(mktemp)
    failures=$(mktemp)
    cat >"$plan"
    status=0
    python3 tools/ci/lanes.py "$plan" "$failures" || status=$?
    if [ "$status" -ne 0 ] && [ -s "$failures" ]; then
        [ "$first" -ne 0 ] || first=$status
        failed="$failed
$(cat "$failures")"
    elif [ "$status" -ne 0 ]; then
        record "$status" "$current (exit $status): tools/ci/lanes.py"
    fi
    rm -f "$plan" "$failures"
}
finish() {
    ended
    printf '\n=== Step times%s\n' "$times"
    if [ "$first" -ne 0 ]; then
        printf '\n=== Failed steps%s\n' "$failed"
        exit "$first"
    fi
    exit 0
}

# Without the image no other step can run.
step "Build the Linux image and start its services"
run tools/linux-container.sh true
[ "$first" -eq 0 ] || finish
# A change of Python unit tests only (tools/ci/changes.py, JUNTO_CI_PARTS=python): those tests.
if [ "${JUNTO_CI_PARTS:-}" = python ]; then
    step "Container runner unit tests"
    run tools/linux-container.sh python3 -m unittest discover -s tools/linux -p "test_*.py"
    step "Packaging unit tests"
    run tools/linux-container.sh python3 -m unittest discover -s packaging -p "test_*.py"
    step "Test service wrapper failure paths"
    run tools/linux-container.sh python3 tools/test_test_services.py
    finish
fi
step "Format"
run tools/linux-container.sh cargo fmt --all --check
step "Lint"
run budget tools/linux-container.sh cargo clippy --workspace --all-targets --locked --keep-going -- -D warnings
# The device checks of the media crate and the native code that only they select, which the
# workspace lint does not compile (ADR 0913).
step "Lint the device checks of the media crate"
run budget tools/linux-container.sh cargo clippy --locked -p junto-media --features native-device-tests --all-targets -- -D warnings
# Each package alone, with only the features that it and its dependencies ask for: its library
# and programs, then every target (tools/ci/each_crate.py).
step "Lint each crate alone"
run budget tools/linux-container.sh python3 tools/ci/each_crate.py clippy --locked --keep-going -- -D warnings
run budget tools/linux-container.sh python3 tools/ci/each_crate.py clippy --locked --all-targets --keep-going -- -D warnings
step "Build the server used by process tests"
run budget tools/linux-container.sh cargo build --workspace --locked
# The V4L2 device in memory of the camera tests is not linked into the client (ADR 0913); the
# camera code itself is.
step "The client holds the camera code and not the test camera device"
run tools/linux-container.sh sh -c 'llvm-nm-22 /target/debug/junto >/tmp/symbols && grep -q junto_camera_open /tmp/symbols && ! grep -q junto_fake_ /tmp/symbols'
# The test programs of `cargo test --workspace`, built once, and junto-screens. The lanes of the
# next step run them and build nothing; the messages of the build name the test programs for
# tools/ci/cargo_targets.py.
step "Build the test programs"
run budget tools/linux-container.sh sh -c 'mkdir -p /target/junto-ci && cargo test --no-fail-fast --workspace --locked --no-run --message-format=json-render-diagnostics >/target/junto-ci/test-programs.json'
run budget tools/linux-container.sh cargo build --locked -p junto-client --features test-support --bin junto-screens
# The test programs of `cargo test --workspace` in two lanes of about the same length: the window
# tests, and the API tests of the server, the other test programs with the doc tests and the
# Python unit tests. Every command has --workspace, so the features are those of the build above.
# junto-screens draws every screen and checks that no area has an empty band, no control is past
# an edge of the window, and each screen settles within its bound (ADR 0730, ADR 0731); it
# measures and writes no picture, and needs no database. The second run draws every screen at
# the least size of the window. Each run uses about one processor and 0.3 GiB (the
# run of 2026-11-03: 19 and 15 minutes), so each runs in two parts (JUNTO_SCREENS_PART), each
# part in a lane of its own without services.
step "Tests with PostgreSQL and the screens"
lanes <<'EOF'
lane app share 4 memory 4g
step Window tests with PostgreSQL
tools/linux-container.sh cargo test --no-fail-fast --workspace --locked --test app
lane other share 2 memory 4g
step Server API tests with PostgreSQL
tools/linux-container.sh cargo test --no-fail-fast --workspace --locked --test api
step Other tests with PostgreSQL
tools/linux-container.sh sh -c 'targets=$(python3 tools/ci/cargo_targets.py /target/junto-ci/test-programs.json app api) && exec cargo test --no-fail-fast --workspace --locked $targets'
tools/linux-container.sh cargo test --no-fail-fast --workspace --locked --doc
step CI runner unit tests
tools/linux-container.sh python3 -m unittest discover -s tools/ci -p "test_*.py"
step Container runner unit tests
tools/linux-container.sh python3 -m unittest discover -s tools/linux -p "test_*.py"
step Packaging unit tests
tools/linux-container.sh python3 -m unittest discover -s packaging -p "test_*.py"
step Test service wrapper failure paths
tools/linux-container.sh python3 tools/test_test_services.py
lane screens-1 memory 1g without services
step Screens without empty bands, part 1 of 2
tools/linux-container.sh sh -c 'JUNTO_SCREENS_FILES=0 JUNTO_SCREENS_PART=1/2 cargo run --locked -p junto-client --features test-support --bin junto-screens -- /tmp/junto-screens'
lane screens-2 memory 1g without services
step Screens without empty bands, part 2 of 2
tools/linux-container.sh sh -c 'JUNTO_SCREENS_FILES=0 JUNTO_SCREENS_PART=2/2 cargo run --locked -p junto-client --features test-support --bin junto-screens -- /tmp/junto-screens'
lane least-1 memory 1g without services
step Screens at the least size of the window, part 1 of 2
tools/linux-container.sh sh -c 'JUNTO_SCREENS_FILES=0 JUNTO_SCREENS_SIZE=720x480 JUNTO_SCREENS_PART=1/2 cargo run --locked -p junto-client --features test-support --bin junto-screens -- /tmp/junto-screens'
lane least-2 memory 1g without services
step Screens at the least size of the window, part 2 of 2
tools/linux-container.sh sh -c 'JUNTO_SCREENS_FILES=0 JUNTO_SCREENS_SIZE=720x480 JUNTO_SCREENS_PART=2/2 cargo run --locked -p junto-client --features test-support --bin junto-screens -- /tmp/junto-screens'
EOF
# A panic in a task of the window thread, on the event loop of X11 and of Wayland: the client
# continues, writes the report and states it (ADR 0757).
step "Panics in the callbacks of the platform"
run budget tools/linux-container.sh tools/linux/display.sh x11 sh -c 'JUNTO_TEST_NATIVE_PANICS=1 cargo test --no-fail-fast --locked -p junto-client --test native_panics'
run tools/linux-container.sh tools/linux/display.sh wayland sh -c 'JUNTO_TEST_NATIVE_PANICS=1 cargo test --no-fail-fast --locked -p junto-client --test native_panics'
step "One desktop process per profile and restoration of its main window"
run tools/linux-container.sh tools/linux/display.sh x11 python3 tools/check_single_instance.py /target/debug/junto
step "Window placement across restart and maximization"
run tools/linux-container.sh tools/linux/display.sh x11 python3 tools/check_window_placement.py /target/debug/junto
run budget tools/linux-container.sh sh -c 'cargo test --no-fail-fast --locked -p junto-client --test native_placement --no-run --message-format=json-render-diagnostics > /tmp/junto-placement-build.json && tools/linux/display.sh x11 python3 tools/check_window_placement.py --call-target /tmp/junto-placement-build.json'
# The keys of a call in every application: the reader of the keyboard of X11 against the X
# server of the run, whose keys xdotool presses (ADR 0776).
step "Keys of a call read from the X server"
run budget tools/linux-container.sh tools/linux/display.sh x11 sh -c 'JUNTO_TEST_GLOBAL_KEYS=1 cargo test --no-fail-fast --locked -p junto-client-core --lib global_keys::x11'
# A press on a notification gives the main window the focus under Wayland, with the activation
# token of the press (ADR 0752).
step "Focus after a press on a notification"
run tools/linux-container.sh tools/linux/display.sh wayland sh -c 'JUNTO_TEST_NOTIFICATION_FOCUS=1 cargo test --no-fail-fast --locked -p junto-client --test notification_focus'
# The camera unit tests with the counts that the device checks read (ADR 0913); they open no
# device.
step "Camera unit tests of the device checks"
run budget tools/linux-container.sh cargo test --no-fail-fast --locked -p junto-media --features native-device-tests --lib camera::
# The production sound host against virtual PipeWire devices, and the trust of the call
# library in a TURN server over TLS. Both need the isolated container (ADR 0750).
step "Linux sound devices and TURN over TLS"
run tools/linux-container.sh cargo test --no-fail-fast --locked -p junto-media --test linux_audio --test tls_trust -- --ignored
# The tarball and the AppImage of the client: their contents, the rule of the bundled
# libraries, the first frame of each on a virtual display, and an update and a rollback of
# the directory package while its program runs (ADR 0754).
step "Linux packages"
run budget tools/linux-container.sh python3 packaging/check_linux.py
step "The committed API documents equal the generated ones"
run budget tools/linux-container.sh cargo run --locked -p junto-server --bin junto-openapi
run tools/linux-container.sh git diff --exit-code -- docs/api
finish
