//! The capture of Windows: Windows Graphics Capture for the picture and process loopback of
//! the audio session API for the sound (ADR 0706).
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
mod sound;
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod sources;

#[cfg(target_os = "windows")]
pub use capture::SystemCapture;
