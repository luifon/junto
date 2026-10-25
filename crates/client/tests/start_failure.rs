//! A start that fails before its data directory opens (ADR 0757): the program `junto` ends
//! with a failure, states the reason on standard error, and keeps it in the start-failure file
//! of the temporary directory of the account. Standard error is a pipe here, so a Windows
//! program shows no message box.

// A test reads its files by design; the refusals of the file system in `clippy.toml` are for
// the code of the application (ADR 0730).
#![allow(clippy::disallowed_methods)]

use std::process::Command;

#[test]
fn a_start_without_a_data_directory_states_the_reason_and_keeps_it_in_a_file() {
    let temporary = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_junto"))
        .env("JUNTO_DATA_DIR", "relative-data")
        .env("TMPDIR", temporary.path())
        .env("TMP", temporary.path())
        .env("TEMP", temporary.path())
        .env("XDG_RUNTIME_DIR", temporary.path())
        .output()
        .unwrap();
    let error = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{error}");
    let reason = "junto cannot start: JUNTO_DATA_DIR must be an absolute path";
    assert!(error.contains(reason), "{error}");
    let kept = std::fs::read_to_string(temporary.path().join("junto-start-failure.txt")).unwrap();
    assert!(kept.trim_end().ends_with(reason), "{kept}");
}
