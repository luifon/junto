//! The capture of Windows: Windows Graphics Capture for the picture and process loopback of
//! the audio session API for the sound, and the start of other applications by the shell,
//! outside the process tree of the client (ADR 0706).
//!
//! The decisions are plain functions in `rules`, which the ordinary tests of every system
//! run. The other modules call the system and are built on Windows only.

// The system modules use the rules on Windows; elsewhere only their tests do.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) mod rules;

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod capture;
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod graphics;
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod shell;
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod sound;
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod sources;
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod still;

#[cfg(target_os = "windows")]
pub use capture::SystemCapture;
/// The start of a program with arguments through the shell, for the tests of the start of
/// applications outside the client's tree.
#[cfg(all(target_os = "windows", feature = "test-support"))]
pub use shell::open_through_the_shell;
#[cfg(target_os = "windows")]
pub use shell::{Opened, Target, open};

/// The frames that the system gave to the sound captures of this process, silent ones
/// included.
#[cfg(all(target_os = "windows", feature = "test-support"))]
pub fn captured_sound_frames() -> u64 {
    sound::CAPTURED_FRAMES.load(std::sync::atomic::Ordering::Relaxed)
}

/// The graphics memory that this process uses, in bytes.
#[cfg(all(target_os = "windows", feature = "test-support"))]
pub fn graphics_memory_in_use() -> Result<u64, crate::share::CaptureError> {
    graphics::memory_in_use()
}
