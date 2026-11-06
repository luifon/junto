//! The time that a share spends in each phase of its pictures, for the measurement of the
//! processor time of a share (ADR 0738, ADR 0914). Only a build with the support of the
//! tests counts; the counts are of the whole process.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// A phase of the pictures of a share.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// The reading of the windows of a shared application and the start and end of their
    /// captures.
    Follow,
    /// The copy of a captured picture from the graphics card into memory.
    Read,
    /// The composition of the windows of an application into one picture.
    Compose,
    /// The conversion of a picture into the planes of the encoder, with its scale.
    Convert,
}

const PHASES: usize = 4;

static NANOS: [AtomicU64; PHASES] = [const { AtomicU64::new(0) }; PHASES];

/// Adds the time since `started` to a phase.
pub(crate) fn add(phase: Phase, started: Instant) {
    let nanos = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
    NANOS[phase as usize].fetch_add(nanos, Ordering::Relaxed);
}

/// The time of each phase since the last [`take`], in milliseconds, in the order of
/// [`Phase`], and the counts start again from zero.
pub fn take() -> [(Phase, f64); PHASES] {
    let millis = |phase: Phase| NANOS[phase as usize].swap(0, Ordering::Relaxed) as f64 / 1e6;
    [
        (Phase::Follow, millis(Phase::Follow)),
        (Phase::Read, millis(Phase::Read)),
        (Phase::Compose, millis(Phase::Compose)),
        (Phase::Convert, millis(Phase::Convert)),
    ]
}
