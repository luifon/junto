#!/bin/bash
# The job "Rust on macOS" and "Rust on Windows" (ADR 0905, ADR 0912). Needs the Rust version
# of rust-toolchain.toml with rustfmt and clippy, and Python 3. On Windows it runs in the
# Bash of Git for Windows. On Windows, tools/windows/with_test_postgres.py keeps the
# PostgreSQL download in ~/.cache/junto-postgres, which a runner may keep between runs.
set -euo pipefail
cd "$(dirname "$0")/../.."
. tools/ci/pins.sh

# Each step states its start in UTC, so that the log gives the time of each step.
step() { printf '\n=== %s  %s\n' "$1" "$(date -u +%Y-%m-%dT%H:%M:%SZ)"; }
case "$(uname -s)" in
    Darwin) system=macos ;;
    MINGW* | MSYS* | CYGWIN*) system=windows ;;
    *) echo "this job runs on macOS or on Windows, not on $(uname -s)" >&2; exit 2 ;;
esac

# Code under cfg(windows) and cfg(target_os = "macos") is linted only on its own system.
# --keep-going: the lints of every crate, also after a crate with an error.
step "Lint"
cargo clippy --workspace --all-targets --locked --keep-going -- -D warnings
# The device checks of the media crate and the native code that only they select, which the
# workspace lint does not compile (ADR 0913).
step "Lint the device checks of the media crate"
cargo clippy --locked -p junto-media --features native-device-tests --all-targets --keep-going -- -D warnings
step "Build every target"
cargo build --workspace --all-targets --locked
if [ "$system" = macos ]; then
    # The WebRTC library ends a blocking call when macOS refuses a condition wait; the media
    # crate defines the two waits for the client (ADR 0786). The check runs the library of
    # this build with and without that module and reads the built binary.
    step "Condition waits of the client on macOS"
    tools/check-condition-wait.sh "${CARGO_TARGET_DIR:-target}/debug/junto"
    # The unit tests of the media crate, with those of the condition waits and those that the
    # Linux job does not compile: camera colour and capture settings, and screen capture
    # configuration. They open no device and ask for no permission. Also the reading of the
    # sources of the WebRTC wrapper (ADR 0733), whose paths differ between the systems.
    step "Media unit tests on macOS"
    cargo test --locked -p junto-media --lib --test native_exceptions
fi
# The tests that need PostgreSQL run on Linux only, except the window tests on Windows and
# on macOS.
step "Tests that need no database"
cargo test --locked -p junto-authz -p junto-protocol
# The reading of the free space of a directory, which differs between Unix and Windows
# (ADR 0605).
cargo test --locked -p junto-server --lib storage::
cargo test --locked -p junto-client-core --lib --bins --test releases --test release_format --test release_roles --test release_downloads --test update_archives --test release_tool
step "Packaging unit tests"
python3 -m unittest discover -s packaging -p "test_*.py"
step "Test service wrapper failure paths"
python3 tools/test_test_services.py
# The emoji as the text system of the system draws them, with the facts of each system
# that the rule of the client must give (ADR 0758). It needs no database, and runs on the
# main thread, which the text system of macOS needs.
step "Emoji of the fonts of the system"
cargo test --locked --workspace --test emoji_fonts
if [ "$system" = macos ]; then
    step "macOS signing and installation checks"
    temporary=$(mktemp -d)
    trap 'rm -rf "$temporary"' EXIT
    export TMPDIR="$temporary"
    python3 packaging/check_signing.py
    python3 packaging/check_update.py
    # The bundle of the client build, started with an empty data directory until the window
    # has drawn its first frame (ADR 0725). The window appears without taking the focus.
    step "First frame of the macOS bundle"
    build="${CARGO_TARGET_DIR:-target}/debug"
    python3 packaging/bundle.py "$build/junto" "$temporary/Junto.app" "$build/junto-recover" "$build/junto-icon"
    python3 packaging/check_start.py "$temporary/Junto.app"
    # A panic in a task and in a press that AppKit delivers: the client continues, writes the
    # report and states it (ADR 0757). The window appears without taking the focus.
    step "Panics in the callbacks of the platform"
    JUNTO_TEST_NATIVE_PANICS=1 cargo test --locked --workspace --test native_panics
    # The window tests of crates/client/tests/app on the test platform of gpui (ADR 0916):
    # updates:: without a database, then the other tests against the PostgreSQL build for
    # macOS of the version of the CI image. Not here: the tests of live::, which need LiveKit.
    # --workspace, as in the build step, so that Cargo builds nothing again.
    step "Window tests without a database"
    cargo test --locked --workspace --test app -- updates::
    step "PostgreSQL for macOS"
    python3 tools/test_with_native_postgres.py
    python3 tools/with_native_postgres.py --fetch
    step "Window tests with PostgreSQL"
    python3 tools/with_native_postgres.py cargo test --locked --workspace --test app -- --skip updates:: --skip live::
    exit 0
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
# desktop of the account for a few seconds without taking the focus. No test plays sound
# (ADR 0706).
step "Media tests and screen capture on Windows"
cargo test --locked -p junto-media --lib --test native_exceptions --test windows_capture
# The keys of a call in every application: the reader of the keyboard against F24, which the
# test presses through SendInput (ADR 0776).
step "Keys of a call read from the keyboard"
JUNTO_TEST_GLOBAL_KEYS=1 cargo test --locked -p junto-client-core --lib global_keys::windows
# The ZIP of the client: extracted into a folder of the account, started, updated while its
# program runs, and rolled back by its recovery program (ADR 0755).
step "Windows package"
python3 packaging/check_windows.py
# A panic in a task of the window thread, run by the window procedure: the client continues,
# writes the report and states it (ADR 0757). Its window appears without taking the focus for
# a few seconds and closes with the test.
step "Panics in the callbacks of the platform"
JUNTO_TEST_NATIVE_PANICS=1 cargo test --locked --workspace --test native_panics
# The window tests of crates/client/tests/app on the test platform of gpui (ADR 0912). This
# step has no database, so a test here that needs one fails. --workspace, as in the build
# step, so that Cargo builds nothing again.
step "Window tests without a database"
cargo test --locked --workspace --test app -- updates::
step "PostgreSQL for Windows"
python3 tools/windows/with_test_postgres.py --fetch
# The other window tests, against the PostgreSQL build for Windows of the version of the CI
# image. Not here: the tests of live:: need LiveKit; the tests of recordings:: and
# transcripts::, the files of a message and the export of a channel need the server to store
# files, which it does on Unix systems only (crates/server/src/recordings/files.rs).
step "Window tests with PostgreSQL"
python3 -m unittest discover -s tools/windows -p "test_*.py"
python3 tools/windows/with_test_postgres.py cargo test --locked --workspace --test app -- \
    --skip updates:: --skip live:: --skip recordings:: --skip transcripts:: \
    --skip messages::a_file_is_sent_with_a_message_and_saved_from_one \
    --skip messages::files_attached_before_a_send_are_in_the_message_in_the_order_of_their_drops \
    --skip messages::a_pending_file_picker_cannot_retarget_an_upload \
    --skip messages::the_picture_of_a_message_is_read_again_for_the_next_account_of_the_server \
    --skip messages::a_message_whose_files_the_retention_removed_says_so \
    --skip admin_pages::the_owner_exports_a_channel_saves_the_file_and_removes_the_export \
    --skip composing::a_file_taken_away_before_the_send_is_not_in_the_message \
    --skip composing::a_pasted_picture_is_a_file_of_the_message_and_pasted_text_is_text \
    --skip composing::an_upload_goes_on_after_its_connection_was_cut_and_the_draft_says_so \
    --skip files::the_controls_of_a_file_download_it_cancel_the_download_and_show_the_file \
    --skip threads::in_a_direct_conversation_a_person_replies_edits_reacts_and_sends_a_file \
    --skip thread_actions::a_person_edits_reacts_to_and_attaches_a_file_to_a_reply_in_a_thread \
    --skip admin_pages::exports_actions::a_direct_conversation_is_exported_from_its_header_and_saved \
    --skip admin_pages::exports_actions::a_saved_channel_export_contains_messages_threads_files_recordings_and_transcripts
