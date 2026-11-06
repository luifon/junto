#!/bin/bash
# The job "Rust on macOS" and "Rust on Windows" (ADR 0905, ADR 0912). Needs the Rust version
# of rust-toolchain.toml with rustfmt and clippy, and Python 3. On Windows it runs in the
# Bash of Git for Windows. On Windows, tools/windows/with_test_postgres.py keeps the
# PostgreSQL download in ~/.cache/junto-postgres, which a runner may keep between runs.
set -euo pipefail
cd "$(dirname "$0")/../.."
. tools/ci/pins.sh

# Each step states its start and its end in UTC and its duration, and the job ends with the table
# of the steps. Every step runs also after a failed step, and every test program runs also after
# a failed one (--no-fail-fast), so that one run shows every failure of the commit; the job ends
# with the list of the failed commands and the exit status of the first (ADR 0905).
current=""
failed=""
first=0
began=0
times=""
ended() {
    [ -n "$current" ] || return 0
    local seconds=$(($(date +%s) - began))
    printf '=== %s: ended %s after %dm%02ds\n' "$current" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" $((seconds / 60)) $((seconds % 60))
    times="$times
$(printf '%5dm%02ds  %s' $((seconds / 60)) $((seconds % 60)) "$current")"
}
step() {
    ended
    current=$1
    began=$(date +%s)
    printf '\n=== %s  %s\n' "$1" "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
}
run() {
    local status
    "$@" || {
        status=$?
        [ "$first" -ne 0 ] || first=$status
        failed="$failed
$current (exit $status): $*"
    }
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
case "$(uname -s)" in
    Darwin) system=macos ;;
    MINGW* | MSYS* | CYGWIN*) system=windows ;;
    *) echo "this job runs on macOS or on Windows, not on $(uname -s)" >&2; exit 2 ;;
esac

# A change of Python unit tests only (tools/ci/changes.py, JUNTO_CI_PARTS=python): those tests.
if [ "${JUNTO_CI_PARTS:-}" = python ]; then
    step "Packaging unit tests"
    run python3 -m unittest discover -s packaging -p "test_*.py"
    step "Test service wrapper failure paths"
    run python3 tools/test_test_services.py
    if [ "$system" = windows ]; then
        step "Windows tool unit tests"
        run python3 -m unittest discover -s tools/windows -p "test_*.py"
    else
        step "PostgreSQL for macOS"
        run python3 tools/test_with_native_postgres.py
    fi
    finish
fi
# Code under cfg(windows) and cfg(target_os = "macos") is linted only on its own system.
# --keep-going: the lints of every crate, also after a crate with an error.
step "Lint"
run cargo clippy --workspace --all-targets --locked --keep-going -- -D warnings
# The device checks of the media crate and the native code that only they select, which the
# workspace lint does not compile (ADR 0913).
step "Lint the device checks of the media crate"
run cargo clippy --locked -p junto-media --features native-device-tests --all-targets --keep-going -- -D warnings
# Each package alone, with only the features that it and its dependencies ask for: its library
# and programs, then every target (tools/ci/each_crate.py, ADR 0905).
step "Lint each crate alone"
run python3 tools/ci/each_crate.py clippy --locked --keep-going -- -D warnings
run python3 tools/ci/each_crate.py clippy --locked --all-targets --keep-going -- -D warnings
# Each package alone, with only the features that it and its dependencies ask for: its library
# and programs, then every target (tools/ci/each_crate.py, ADR 0905).
step "Lint each crate alone"
run python3 tools/ci/each_crate.py clippy --locked --keep-going -- -D warnings
run python3 tools/ci/each_crate.py clippy --locked --all-targets --keep-going -- -D warnings
step "Build every target"
run cargo build --workspace --all-targets --locked
if [ "$system" = macos ]; then
    # The WebRTC library ends a blocking call when macOS refuses a condition wait; the media
    # crate defines the two waits for the client (ADR 0786). The check runs the library of
    # this build with and without that module and reads the built binary.
    step "Condition waits of the client on macOS"
    run tools/check-condition-wait.sh "${CARGO_TARGET_DIR:-target}/debug/junto"
    # The unit tests of the media crate, with those of the condition waits and those that the
    # Linux job does not compile: camera colour and capture settings, and screen capture
    # configuration. They open no device and ask for no permission. Also the reading of the
    # sources of the WebRTC wrapper (ADR 0733), whose paths differ between the systems.
    step "Media unit tests on macOS"
    run cargo test --no-fail-fast --locked -p junto-media --lib --test native_exceptions
fi
step "Tests that need no database"
run cargo test --no-fail-fast --locked -p junto-authz -p junto-protocol
run cargo test --no-fail-fast --locked -p junto-client-core --lib --bins --test releases --test release_format --test release_roles --test release_downloads --test update_archives --test release_tool
step "Packaging unit tests"
run python3 -m unittest discover -s packaging -p "test_*.py"
step "Test service wrapper failure paths"
run python3 tools/test_test_services.py
# The emoji as the text system of the system draws them, with the facts of each system
# that the rule of the client must give (ADR 0758). It needs no database, and runs on the
# main thread, which the text system of macOS needs.
step "Emoji of the fonts of the system"
run cargo test --no-fail-fast --locked --workspace --test emoji_fonts
if [ "$system" = macos ]; then
    step "macOS signing and installation checks"
    temporary=$(mktemp -d)
    trap 'rm -rf "$temporary"' EXIT
    export TMPDIR="$temporary"
    run python3 packaging/check_signing.py
    run python3 packaging/check_update.py
    # The bundle of the client build, started with an empty data directory until the window
    # has drawn its first frame (ADR 0725). The window appears without taking the focus.
    step "First frame of the macOS bundle"
    build="${CARGO_TARGET_DIR:-target}/debug"
    run python3 packaging/bundle.py "$build/junto" "$temporary/Junto.app" "$build/junto-recover" "$build/junto-icon"
    run python3 packaging/check_start.py "$temporary/Junto.app"
    # A panic in a task and in a press that AppKit delivers: the client continues, writes the
    # report and states it (ADR 0757). The window appears without taking the focus.
    step "Panics in the callbacks of the platform"
    run env JUNTO_TEST_NATIVE_PANICS=1 cargo test --no-fail-fast --locked --workspace --test native_panics
    step "Native macOS window placement and Dock reopening"
    run env JUNTO_TEST_NATIVE_MACOS_WINDOWS=1 cargo test --no-fail-fast --locked --workspace --test native_macos_windows
    # The window tests of crates/client/tests/app on the test platform of gpui (ADR 0916):
    # updates:: without a database, then the other tests against the PostgreSQL build for
    # macOS of the version of the CI image. The tests of live:: are ignored here: no release of
    # LiveKit for macOS is pinned (ADR 0905).
    # --workspace, as in the build step, so that Cargo builds nothing again.
    step "Window tests without a database"
    run cargo test --no-fail-fast --locked --workspace --test app -- updates::
    step "PostgreSQL for macOS"
    run python3 tools/test_with_native_postgres.py
    run python3 tools/with_native_postgres.py --fetch
    step "Window tests with PostgreSQL"
    run python3 tools/with_native_postgres.py cargo test --no-fail-fast --locked --workspace --test app -- --skip updates::
    # The library and the API tests of the server against the same PostgreSQL. The API tests
    # with --workspace, as in the build step, so that Cargo builds nothing again.
    step "Server tests with PostgreSQL"
    run python3 tools/with_native_postgres.py cargo test --no-fail-fast --locked -p junto-server --lib
    run python3 tools/with_native_postgres.py cargo test --no-fail-fast --locked --workspace --test api
    finish
fi

# A dialog of the system that a test opens stays on the desktop of the account until a person
# closes it, and its dimmed background changes the pictures of the capture tests of later runs.
# The job fails when such a dialog that was not there before the tests runs at its end, also
# after a failed step (ADR 0915).
dialogs=$(mktemp)
python3 tools/windows/dialogs.py record "$dialogs"
trap 'python3 tools/windows/dialogs.py check "$dialogs" || exit 1; rm -f "$dialogs"' EXIT
# The unit tests of the media crate, the reading of the sources of the WebRTC wrapper (ADR 0733),
# and the capture of the screen against the window of a fixture program, which appears on the
# desktop of the account for a few seconds without taking the focus. One test makes the fixture
# play a tone for a few seconds on the default output device (ADR 0706).
step "Media tests and screen capture on Windows"
run cargo test --no-fail-fast --locked -p junto-media --lib --test native_exceptions --test windows_capture
# The keys of a call in every application: the reader of the keyboard against F24, which the
# test presses through SendInput (ADR 0776).
step "Keys of a call read from the keyboard"
run env JUNTO_TEST_GLOBAL_KEYS=1 cargo test --no-fail-fast --locked -p junto-client-core --lib global_keys::windows
# The ZIP of the client: extracted into a folder of the account, started, updated while its
# program runs, and rolled back by its recovery program (ADR 0755).
step "Windows package"
run python3 packaging/check_windows.py
# A panic in a task of the window thread, run by the window procedure: the client continues,
# writes the report and states it (ADR 0757). Its window appears without taking the focus for
# a few seconds and closes with the test.
step "Panics in the callbacks of the platform"
run env JUNTO_TEST_NATIVE_PANICS=1 cargo test --no-fail-fast --locked --workspace --test native_panics
# The window tests of crates/client/tests/app on the test platform of gpui (ADR 0912). This
# step has no database, so a test here that needs one fails. --workspace, as in the build
# step, so that Cargo builds nothing again.
step "Window tests without a database"
run cargo test --no-fail-fast --locked --workspace --test app -- updates::
step "PostgreSQL, LiveKit and FFmpeg for Windows"
run python3 tools/windows/with_test_postgres.py --fetch
run python3 tools/with_native_livekit.py --fetch
run python3 tools/with_native_ffmpeg.py --fetch
run python3 tools/test_native_programs.py
# The remaining window tests use native PostgreSQL, the Windows file store (ADR 0912) and the
# FFmpeg build for Windows; the stand-in of the transcription worker runs in the sh of Git for
# Windows.
step "Window tests with PostgreSQL"
run python3 -m unittest discover -s tools/windows -p "test_*.py"
run python3 tools/windows/with_test_postgres.py python3 tools/with_native_ffmpeg.py \
    cargo test --no-fail-fast --locked --workspace --test app -- --skip updates::
# The library and the API tests of the server against the same PostgreSQL and FFmpeg.
step "Server tests with PostgreSQL"
run python3 tools/windows/with_test_postgres.py python3 tools/with_native_ffmpeg.py \
    cargo test --no-fail-fast --locked -p junto-server --lib
run python3 tools/windows/with_test_postgres.py python3 tools/with_native_ffmpeg.py \
    cargo test --no-fail-fast --locked --workspace --test api
# The window tests of calls against the LiveKit release for Windows of the version of the image
# of the other jobs, every socket of it on the loopback address (tools/with_native_livekit.py).
# The share of a window of the fixture program carries the tone that the fixture plays.
step "Window tests with PostgreSQL and LiveKit"
run python3 tools/windows/with_test_postgres.py python3 tools/with_native_livekit.py \
    cargo test --no-fail-fast --locked --workspace --test app live:: -- --ignored
finish
