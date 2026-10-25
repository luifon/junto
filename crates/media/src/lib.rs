//! The media engine of the Junto client (ADR 0733, ADR 0734): the sound devices, the media
//! connection of one call, and the one call that a client is in over all its servers. The
//! crate has no interface; the desktop client draws what [`calls::Calls`] tells.

pub mod audio;
mod callback;
pub mod calls;
pub mod camera;
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
pub mod com;
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
pub mod condition_wait;
pub mod engine;
mod executor;
pub mod permission;
pub mod share;

pub use calls::{Calls, MediaOptions};
