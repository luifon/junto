//! The emoji as the text system of this system draws them, with the fonts of the design
//! (ADR 0758): facts of each system that the rule of `junto_client::emoji` must give, and
//! the time of the measurement of the whole table.
//!
//! The text system of macOS is made on the main thread, so the target runs without the
//! test runner and reads no filter of test names:
//!
//! ```sh
//! cargo test -p junto-client --test emoji_fonts
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used)]
// The target reads whether it runs in the Linux build image, whose fonts it knows; the
// refusals of the file system in `clippy.toml` are for the code of the application.
#![allow(clippy::disallowed_methods)]

use std::sync::Arc;
use std::time::Instant;

use gpui::TextSystem;
use junto_client::emoji::{Drawing, Measure};

/// Signs that every system draws, or that no system draws whole.
const EVERY_SYSTEM: &[(&str, &[Drawing])] = &[
    ("\u{1f600}", &[Drawing::Colour, Drawing::Text]),
    // A code point of private use, which no font has.
    ("\u{10fffd}", &[Drawing::Broken]),
    // Two faces joined, which no font draws as one picture.
    ("\u{1f600}\u{200d}\u{1f600}", &[Drawing::Broken]),
];

/// Segoe UI Emoji has no country flags: a flag is two letters. It draws the chequered flag.
#[cfg(target_os = "windows")]
const THIS_SYSTEM: &[(&str, &[Drawing])] = &[
    ("\u{1f1e7}\u{1f1f7}", &[Drawing::Broken]),
    ("\u{1f3c1}", &[Drawing::Colour]),
    ("\u{1f44d}\u{1f3fd}", &[Drawing::Colour]),
    ("\u{2764}\u{fe0f}", &[Drawing::Colour]),
    (
        "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}",
        &[Drawing::Colour],
    ),
];

/// Apple Color Emoji draws the flags and every emoji of the first versions. The signs
/// whose first character the font of the text also has are drawn whole.
#[cfg(target_os = "macos")]
const THIS_SYSTEM: &[(&str, &[Drawing])] = &[
    ("\u{1f600}", &[Drawing::Colour]),
    ("\u{1f1e7}\u{1f1f7}", &[Drawing::Colour]),
    ("\u{1f44d}\u{1f3fd}", &[Drawing::Colour]),
    ("\u{a9}\u{fe0f}", &[Drawing::Colour, Drawing::Text]),
    ("\u{ae}\u{fe0f}", &[Drawing::Colour, Drawing::Text]),
    ("\u{2122}\u{fe0f}", &[Drawing::Colour, Drawing::Text]),
    ("#\u{fe0f}\u{20e3}", &[Drawing::Colour, Drawing::Text]),
    ("*\u{fe0f}\u{20e3}", &[Drawing::Colour, Drawing::Text]),
    ("0\u{fe0f}\u{20e3}", &[Drawing::Colour, Drawing::Text]),
    ("1\u{fe0f}\u{20e3}", &[Drawing::Colour, Drawing::Text]),
    ("9\u{fe0f}\u{20e3}", &[Drawing::Colour, Drawing::Text]),
];

/// Noto Color Emoji 2.042 of the Linux build image: Emoji 15.0, with flags; DejaVu Sans
/// draws the faces of the first versions in one colour.
#[cfg(target_os = "linux")]
const THIS_SYSTEM: &[(&str, &[Drawing])] = &[
    ("\u{1f1e7}\u{1f1f7}", &[Drawing::Colour]),
    ("\u{1f44d}\u{1f3fd}", &[Drawing::Colour]),
    ("\u{1f44d}", &[Drawing::Colour]),
    ("\u{1f634}", &[Drawing::Text]),
    // Emoji 16.0.
    ("\u{1fae9}", &[Drawing::Broken]),
    ("\u{1fabe}", &[Drawing::Broken]),
];

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
const THIS_SYSTEM: &[(&str, &[Drawing])] = &[];

fn main() {
    // A headless platform of Windows has a text system without fonts; DirectWrite comes
    // with the platform that can open windows. It opens none here.
    let platform = gpui_platform::current_platform(!cfg!(target_os = "windows")).text_system();
    let text = Arc::new(TextSystem::new(platform));
    assert!(
        !text.all_font_names().is_empty(),
        "the platform gave a text system without fonts"
    );
    text.add_fonts(junto_client::assets::fonts())
        .expect("the fonts of the design load");
    let family = junto_client::theme::Theme::stone().fonts.sans;
    let measure = Measure::new(text.clone(), family.clone());
    // The facts of Linux hold for the fonts of the Linux build image.
    let own_fonts = !cfg!(target_os = "linux") || std::path::Path::new("/opt/junto-linux").exists();
    let facts = EVERY_SYSTEM
        .iter()
        .chain(THIS_SYSTEM.iter().filter(|_| own_fonts));
    let mut wrong = Vec::new();
    for (sequence, expected) in facts {
        let drawing = measure.drawing(sequence);
        println!(
            "{}: {drawing:?}: {}",
            sequence.escape_unicode(),
            measure.glyphs(sequence)
        );
        if !expected.contains(&drawing) {
            wrong.push(format!(
                "{} is {drawing:?}, expected one of {expected:?}",
                sequence.escape_unicode()
            ));
        }
    }
    // The application measures on a thread of the background executor, with the text
    // system of the measurement: a first start without a kept measurement, and a start
    // that finds the kept measurement of the same fonts.
    let before = memory();
    let window_text = text.clone();
    let (table, kept, first, again) = std::thread::spawn(move || {
        let started = Instant::now();
        let (table, kept) = junto_client::emoji::measure(
            junto_client::emoji::measurement_text_system(window_text.clone()),
            family.clone(),
            None,
        );
        let first = started.elapsed();
        let started = Instant::now();
        let (_, measured_again) = junto_client::emoji::measure(
            junto_client::emoji::measurement_text_system(window_text),
            family,
            kept.clone(),
        );
        assert!(
            measured_again.is_none(),
            "the kept measurement of the same fonts is used"
        );
        (table, kept, first, started.elapsed())
    })
    .join()
    .expect("the measurement on another thread ends");
    let count = |kind| table.values().filter(|drawing| **drawing == kind).count();
    println!(
        "The table of {} emoji was measured in {} ms: {} in colour, {} without colour, {} not drawn whole.",
        table.len(),
        first.as_millis(),
        count(Drawing::Colour),
        count(Drawing::Text),
        count(Drawing::Broken)
    );
    println!(
        "A start with the kept measurement took {} ms. The kept file holds {} bytes of emoji.",
        again.as_millis(),
        kept.map_or(0, |kept| kept.broken.concat().len()
            + kept.text.concat().len())
    );
    if let (Some(before), Some(after)) = (before, memory()) {
        println!("Memory of the process: {before} before the measurements, {after} after.");
    }
    let mut broken: Vec<&str> = table
        .iter()
        .filter(|(_, drawing)| **drawing == Drawing::Broken)
        .map(|(sequence, _)| *sequence)
        .collect();
    broken.sort_unstable();
    for sequence in broken.iter().take(12) {
        println!(
            "not drawn whole: {}: {}",
            sequence.escape_unicode(),
            measure.glyphs(sequence)
        );
    }
    if !wrong.is_empty() {
        for line in &wrong {
            eprintln!("{line}");
        }
        std::process::exit(1);
    }
}

/// The resident memory and its peak, where the system tells them to the process.
fn memory() -> Option<String> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let field = |name: &str| {
        status
            .lines()
            .find(|line| line.starts_with(name))
            .map(|line| line[name.len()..].trim().to_owned())
    };
    Some(format!(
        "resident {}, peak {}",
        field("VmRSS:")?,
        field("VmHWM:")?
    ))
}
