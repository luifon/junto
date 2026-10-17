//! The rehearsal: two native windows of the application go through the first use of a
//! deployment that was installed with `docs/install.md` (ADR 0902). The steps are those of
//! `tests/app/rehearsal.rs`; this program runs them on the platform of macOS, with
//! synthetic sound, camera and screen, and saves what the windows drew.
//!
//! It runs on the main thread, which the windows of the system need, so it is a test
//! without the test runner. It does nothing on other systems than macOS, and nothing
//! without its variables:
//!
//! ```sh
//! JUNTO_REHEARSAL_ADDRESS=http://127.0.0.1:8080 \
//! JUNTO_REHEARSAL_OWNER_CODE_FILE=<file with the owner code> \
//! JUNTO_REHEARSAL_SPEECH=<directory with en_quick and trio_second> \
//! JUNTO_REHEARSAL_WORK=<new directory> \
//!     tools/serial.sh cargo test -p junto-client --test rehearsal
//! ```
//!
//! The speech files come from `workers/transcription/tests/speech/generate.py`. The work
//! directory gets the two profiles, the saved files of the recording, the pictures of
//! each step and `steps.txt`.

#![allow(clippy::unwrap_used, clippy::expect_used)]
// A test waits for its files by design; the refusals of the file system in `clippy.toml`
// are for the code of the application (ADR 0730).
#![allow(clippy::disallowed_methods)]

use std::process::ExitCode;

// The visual test context of gpui exists only on macOS, so everything below except the
// `main` of other systems is compiled there only (ADR 0902).
#[cfg(target_os = "macos")]
use std::panic::{AssertUnwindSafe, catch_unwind};
#[cfg(target_os = "macos")]
use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../../../client-core/tests/e2e/recording_server.rs"]
mod recording_server;
#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../../../client-core/tests/e2e/support.rs"]
mod support;

#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../app/harness.rs"]
mod harness;
#[cfg(target_os = "macos")]
mod native;
#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../app/rehearsal.rs"]
mod rehearsal;

#[cfg(target_os = "macos")]
use rehearsal::{Deployment, Rehearsal, Speech};

#[cfg(not(target_os = "macos"))]
fn main() -> ExitCode {
    println!("rehearsal: not run, the native rehearsal runs only on macOS");
    ExitCode::SUCCESS
}

#[cfg(target_os = "macos")]
fn variable(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

#[cfg(target_os = "macos")]
fn main() -> ExitCode {
    let Some(address) = variable("JUNTO_REHEARSAL_ADDRESS") else {
        println!("rehearsal: not run, JUNTO_REHEARSAL_ADDRESS is not set");
        return ExitCode::SUCCESS;
    };
    let need = |name: &str| variable(name).unwrap_or_else(|| panic!("{name} is not set"));
    let owner_code = std::fs::read_to_string(need("JUNTO_REHEARSAL_OWNER_CODE_FILE"))
        .unwrap()
        .trim()
        .to_owned();
    let speech = PathBuf::from(need("JUNTO_REHEARSAL_SPEECH"));
    let work = PathBuf::from(need("JUNTO_REHEARSAL_WORK"));
    let (a_speech, b_speech) = (
        read_speech(&speech, "en_quick"),
        read_speech(&speech, "trio_second"),
    );
    let evidence = work.join("pictures");
    std::fs::create_dir_all(&evidence).unwrap();
    let tag = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .to_string();

    let mut native = native::Native::start();
    let mut run = Rehearsal::new(
        &mut native.cx,
        &work.join("profiles"),
        Deployment {
            address,
            owner_code,
        },
        true,
        Some(evidence),
        &tag,
    );
    let mut report = Vec::new();
    let mut failed = false;
    let mut step = |name: &str, run: &mut Rehearsal, act: &mut dyn FnMut(&mut Rehearsal)| {
        if failed {
            report.push(format!("{name}: not run"));
            return;
        }
        let started = std::time::Instant::now();
        match catch_unwind(AssertUnwindSafe(|| act(run))) {
            Ok(()) => {
                run.pictures(name);
                report.push(format!("{name}: passed in {:?}", started.elapsed()));
            }
            Err(panic) => {
                failed = true;
                let reason = panic
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(|text| (*text).to_owned()))
                    .unwrap_or_default();
                let _ = catch_unwind(AssertUnwindSafe(|| run.pictures(&format!("{name}-failed"))));
                if let Ok(state) = catch_unwind(AssertUnwindSafe(|| run.describe())) {
                    println!("rehearsal: the windows after the failure:\n{state}");
                }
                report.push(format!("{name}: failed: {reason}"));
            }
        }
        println!("rehearsal: {}", report.last().unwrap());
    };
    let downloads = work.join("downloads");
    let mut heard_from_b = String::new();
    step("1-first-owner", &mut run, &mut |run| run.first_owner());
    step("2-invitation", &mut run, &mut |run| run.invitation());
    step("3-text", &mut run, &mut |run| run.text());
    step("4-thread", &mut run, &mut |run| run.thread());
    step("5-call", &mut run, &mut |run| run.call());
    step("6-share", &mut run, &mut |run| run.share());
    step("7-recording", &mut run, &mut |run| {
        run.recording(&a_speech, &b_speech)
    });
    step("8-files", &mut run, &mut |run| {
        run.files(&downloads);
    });
    step("9-transcript", &mut run, &mut |run| {
        heard_from_b = run.transcript(&a_speech, &b_speech).1;
    });
    // A word that only B said, as the transcript has it, finds the line of B.
    let a_words = rehearsal::words(&a_speech.text);
    let spoken = rehearsal::words(&b_speech.text)
        .into_iter()
        .filter(|word| word.len() >= 6 && !a_words.contains(word))
        .find(|word| rehearsal::words(&heard_from_b).contains(word));
    // The native run always searches for a spoken word: without one the step fails.
    step("10-search", &mut run, &mut |run| {
        let word = spoken
            .as_deref()
            .expect("no word of six or more letters that only B said is in the transcript of B");
        run.search(Some(word));
    });
    let files = run
        .found
        .downloads
        .iter()
        .map(|(path, bytes, kind, person)| {
            format!(
                "{}\t{bytes}\t{kind:?}\t{}",
                path.display(),
                person.as_deref().unwrap_or("-")
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(work.join("files.tsv"), files + "\n").unwrap();
    if failed {
        report.push("11-persistence: not run".to_owned());
    } else {
        let result = catch_unwind(AssertUnwindSafe(move || run.persistence()));
        report.push(match result {
            Ok(()) => "11-persistence: passed".to_owned(),
            Err(_) => {
                failed = true;
                "11-persistence: failed".to_owned()
            }
        });
        println!("rehearsal: {}", report.last().unwrap());
    }
    std::fs::write(work.join("steps.txt"), report.join("\n") + "\n").unwrap();
    drop(native);
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(target_os = "macos")]
/// A generated speech file: a WAV file of 16-bit mono samples, and the description that
/// the generator wrote beside it, with the text.
fn read_speech(directory: &Path, name: &str) -> Speech {
    let description: serde_json::Value =
        serde_json::from_slice(&std::fs::read(directory.join(format!("{name}.json"))).unwrap())
            .unwrap();
    let text = description["text"].as_str().unwrap().to_owned();
    let bytes = std::fs::read(directory.join(format!("{name}.wav"))).unwrap();
    let (samples, rate) = wav(&bytes);
    Speech {
        samples,
        rate,
        text,
    }
}

#[cfg(target_os = "macos")]
/// The samples and the rate of a WAV file with one channel of 16-bit samples.
fn wav(bytes: &[u8]) -> (Vec<f32>, u32) {
    assert!(&bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE");
    let (mut at, mut rate, mut samples) = (12, None, None);
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let body = &bytes[at + 8..(at + 8 + size).min(bytes.len())];
        match id {
            b"fmt " => {
                let format = u16::from_le_bytes([body[0], body[1]]);
                let channels = u16::from_le_bytes([body[2], body[3]]);
                let bits = u16::from_le_bytes([body[14], body[15]]);
                assert_eq!((format, channels, bits), (1, 1, 16), "16-bit mono PCM");
                rate = Some(u32::from_le_bytes(body[4..8].try_into().unwrap()));
            }
            b"data" => {
                samples = Some(
                    body.as_chunks::<2>()
                        .0
                        .iter()
                        .map(|pair| f32::from(i16::from_le_bytes(*pair)) / 32768.0)
                        .collect(),
                );
            }
            _ => {}
        }
        at += 8 + size + size % 2;
    }
    (samples.unwrap(), rate.unwrap())
}
