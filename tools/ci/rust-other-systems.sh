#!/bin/bash
# The job "Rust on macOS" and "Rust on Windows" (ADR 0905, ADR 0912). Needs the Rust version
# of rust-toolchain.toml with rustfmt and clippy, and Python 3. On Windows it runs in the
# Bash of Git for Windows. On Windows, tools/windows/with_test_postgres.py keeps the
# PostgreSQL download in ~/.cache/junto-postgres, which a runner may keep between runs.
set -euo pipefail
cd "$(dirname "$0")/../.."
. tools/ci/pins.sh

step() { printf '\n=== %s\n' "$1"; }
case "$(uname -s)" in
    Darwin) system=macos ;;
    MINGW* | MSYS* | CYGWIN*) system=windows ;;
    *) echo "this job runs on macOS or on Windows, not on $(uname -s)" >&2; exit 2 ;;
esac

step "Build every target"
cargo build --workspace --all-targets --locked
if [ "$system" = macos ]; then
    # The WebRTC library ends a blocking call when macOS refuses a condition wait; the media
    # crate defines the two waits for the client (ADR 0786). The check runs the library of
    # this build with and without that module and reads the built binary.
    step "Condition waits of the client on macOS"
    tools/check-condition-wait.sh "${CARGO_TARGET_DIR:-target}/debug/junto"
    cargo test --locked -p junto-media --lib condition_wait
fi
# The tests that need PostgreSQL run on Linux only, except the window tests on Windows.
step "Tests that need no database"
cargo test --locked -p junto-authz -p junto-protocol
cargo test --locked -p junto-client-core --lib --bins --test releases --test release_format --test release_roles --test release_downloads --test update_archives --test release_tool
step "Packaging unit tests"
python3 -m unittest discover -s packaging -p "test_*.py"
if [ "$system" = macos ]; then
    step "macOS signing and installation checks"
    temporary=$(mktemp -d)
    trap 'rm -rf "$temporary"' EXIT
    export TMPDIR="$temporary"
    python3 packaging/check_signing.py
    python3 packaging/check_update.py
    cargo clippy --locked -p junto-client-core --all-targets -- -D warnings
    exit 0
fi

# The ZIP of the client: extracted into a folder of the account, started, updated while its
# program runs, and rolled back by its recovery program (ADR 0755).
step "Windows package"
python3 packaging/check_windows.py
# The window tests of crates/client/tests/app on the test platform of gpui (ADR 0912). This
# step has no database, so a test here that needs one fails. --workspace, as in the build
# step, so that Cargo builds nothing again.
step "Window tests without a database"
cargo test --locked --workspace --test app -- updates::
step "PostgreSQL for Windows"
python3 tools/windows/with_test_postgres.py --fetch
# The other window tests, against the PostgreSQL build for Windows of the version of the CI
# image. Not here: the tests of live:: need LiveKit; the tests of recordings::, the file of
# a message and the export of a channel need the server to store files, which it does on
# Unix systems only (crates/server/src/recordings/files.rs).
step "Window tests with PostgreSQL"
python3 -m unittest discover -s tools/windows -p "test_*.py"
python3 tools/windows/with_test_postgres.py cargo test --locked --workspace --test app -- \
    --skip updates:: --skip live:: --skip recordings:: \
    --skip messages::a_file_is_sent_with_a_message_and_saved_from_one \
    --skip messages::a_pending_file_picker_cannot_retarget_an_upload \
    --skip messages::the_picture_of_a_message_is_read_again_for_the_next_account_of_the_server \
    --skip admin_pages::the_owner_exports_a_channel_saves_the_file_and_removes_the_export
