//! The exceptions of the vendored WebRTC wrapper (ADR 0733). The wrapper is built with the
//! defines of the WebRTC library, which has `_HAS_EXCEPTIONS=0` on Windows. With that value
//! `std::runtime_error` keeps only a pointer to its message, and the catch clause of the cxx
//! bridge does not catch the exceptions of the C++ runtime. So every throw of the wrapper throws
//! `livekit_ffi::Exception`, which owns its message, and every bridge with a function that can
//! fail includes `livekit/exception.h`, whose catch replaces the one of cxx.
//!
//! What is wrong shows only in a Windows build, so this test reads the sources. The NVIDIA codec
//! code in `src/nvidia` is excluded: it is built only on Linux, whose C++ library copies the
//! message, and its encoders catch their own exceptions.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};

const WRAPPER: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../vendor/webrtc-sys-0.3.48"
);

fn files(directory: &Path, extensions: &[&str], found: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            if path.file_name().unwrap() != "nvidia" {
                files(&path, extensions, found);
            }
        } else if path
            .extension()
            .is_some_and(|extension| extensions.iter().any(|wanted| extension == *wanted))
        {
            found.push(path);
        }
    }
}

fn sources(extensions: &[&str]) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for directory in ["src", "include"] {
        files(&Path::new(WRAPPER).join(directory), extensions, &mut found);
    }
    assert!(!found.is_empty(), "no source under {WRAPPER}");
    found.sort();
    found
        .into_iter()
        .map(|path| {
            let text = fs::read_to_string(&path).unwrap();
            (path.display().to_string(), text)
        })
        .collect()
}

/// The code of a line of C++, without a `//` comment.
fn code(line: &str) -> &str {
    line.split("//").next().unwrap_or_default()
}

/// What follows each `throw` keyword in `code`.
fn operands(code: &str) -> Vec<&str> {
    let name = |c: char| c.is_ascii_alphanumeric() || c == '_';
    code.match_indices("throw")
        .filter(|(start, _)| !code[..*start].ends_with(name))
        .map(|(start, keyword)| &code[start + keyword.len()..])
        .filter(|rest| !rest.starts_with(name))
        .map(str::trim_start)
        .collect()
}

#[test]
fn every_throw_of_the_wrapper_throws_the_exception_that_owns_its_message() {
    let mut other = Vec::new();
    for (path, text) in sources(&["cpp", "h"]) {
        // The test functions of the bridge throw other values on purpose.
        if path.ends_with("src/exception.cpp") {
            continue;
        }
        for (number, line) in text.lines().enumerate() {
            for operand in operands(code(line)) {
                let rethrow = operand.starts_with(';');
                if !rethrow && !operand.starts_with("Exception(") {
                    other.push(format!("{path}:{}: {}", number + 1, line.trim()));
                }
            }
        }
    }
    assert!(
        other.is_empty(),
        "throws of another type than livekit_ffi::Exception:\n{}",
        other.join("\n")
    );
}

#[test]
fn every_bridge_with_a_function_that_can_fail_includes_the_catch_of_the_wrapper() {
    let mut missing = Vec::new();
    for (path, text) in sources(&["rs"]) {
        let fails = text.split("extern \"C++\"").skip(1).any(|block| {
            block
                .split("extern \"Rust\"")
                .next()
                .unwrap()
                .contains("-> Result<")
        });
        if fails && !text.contains("include!(\"livekit/exception.h\");") {
            missing.push(path);
        }
    }
    assert!(
        missing.is_empty(),
        "bridges without livekit/exception.h:\n{}",
        missing.join("\n")
    );
}
