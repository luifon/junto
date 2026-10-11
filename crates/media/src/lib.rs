//! The media engine of the Junto client (ADR 0733, ADR 0734): the sound devices, the media
//! connection of one call, and the one call that a client is in over all its servers. The
//! crate has no interface; the desktop client draws what [`calls::Calls`] tells.

pub mod audio;
pub mod calls;
pub mod engine;
pub mod permission;

pub use calls::{Calls, MediaOptions};
