//! A window of a separate program for the tests of the Windows capture (ADR 0706). The
//! capture never lists a window of the program that captures, so the window that a test
//! captures belongs to this program.
//!
//! `junto-share-fixture <title>` opens a window with that title whose inner area shows four
//! quarters of known colours, above the other windows and without taking the focus. It
//! writes one line to its standard output when the window is drawn:
//!
//! `ready <window number> <screen number> <left> <top> <right> <bottom>`
//!
//! with the inner area in pixels of the screen, from the top left corner of that screen. It
//! reads lines from its standard input: `turn` turns the colours by one quarter, `close`
//! closes the window. The end of the input also closes it.

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod window;

#[cfg(target_os = "windows")]
fn main() {
    let title = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "Junto share fixture".to_owned());
    std::process::exit(window::run(&title));
}

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("the share fixture is a window of Windows");
    std::process::exit(2);
}
