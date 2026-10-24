//! How many events happened in each 10 ms of the last second, counted without a lock.
//!
//! The threads of the sound devices count the frames that pass them. A live test that
//! fails reads the counts and prints them: a time without frames at one place of the
//! pipeline, and the frames that follow it together, are then seen where they happened.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// The time that one count covers.
const SLOT: Duration = Duration::from_millis(10);
const SLOTS: usize = 128;

pub struct Recent {
    origin: Instant,
    /// The number of the slot in the upper half and its count in the lower half.
    slots: [AtomicU64; SLOTS],
}

impl Default for Recent {
    fn default() -> Self {
        Self {
            origin: Instant::now(),
            slots: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
}

impl std::fmt::Debug for Recent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.line(Duration::from_millis(600)))
    }
}

impl Recent {
    fn slot_at(&self, at: Instant) -> u64 {
        // The first slot has the number 1, so that an unused slot belongs to no time.
        (at.saturating_duration_since(self.origin).as_millis() / SLOT.as_millis()) as u64 + 1
    }

    /// Counts one event now.
    pub fn count(&self) {
        self.count_at(Instant::now());
    }

    fn count_at(&self, at: Instant) {
        let number = self.slot_at(at);
        let slot = &self.slots[(number % SLOTS as u64) as usize];
        let _ = slot.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |held| {
            Some(if held >> 32 == number {
                held + 1
            } else {
                (number << 32) | 1
            })
        });
    }

    /// The counts of the slots that end at `now`, the oldest first. A span shorter than one
    /// slot reads the slot of `now`.
    fn counts_at(&self, now: Instant, span: Duration) -> Vec<u32> {
        let last = self.slot_at(now);
        let slots = (span.as_millis() / SLOT.as_millis()).clamp(1, SLOTS as u128 - 1) as u64;
        (last.saturating_sub(slots - 1).max(1)..=last)
            .map(|number| {
                let held = self.slots[(number % SLOTS as u64) as usize].load(Ordering::Relaxed);
                if held >> 32 == number { held as u32 } else { 0 }
            })
            .collect()
    }

    /// The counts of the last `span`, one character for each 10 ms, the oldest first: a
    /// point for no event, a digit for up to nine, a plus sign for more.
    pub fn line(&self, span: Duration) -> String {
        self.counts_at(Instant::now(), span)
            .into_iter()
            .map(|count| match count {
                0 => '.',
                1..=9 => char::from(b'0' + count as u8),
                _ => '+',
            })
            .collect()
    }

    /// The number of events in the last `span`.
    pub fn total(&self, span: Duration) -> u32 {
        self.counts_at(Instant::now(), span).into_iter().sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_are_counted_in_the_time_in_which_they_happened_and_old_ones_are_forgotten() {
        let recent = Recent::default();
        let start = recent.origin;
        let at = |milliseconds: u64| start + Duration::from_millis(milliseconds);
        recent.count_at(at(5));
        recent.count_at(at(25));
        recent.count_at(at(29));
        assert_eq!(
            recent.counts_at(at(39), Duration::from_millis(40)),
            [1, 0, 2, 0]
        );
        // A slot is used again after 1.28 seconds: the old count is not added to the new.
        recent.count_at(at(1_285));
        assert_eq!(
            recent.counts_at(at(1_289), Duration::from_millis(20)),
            [0, 1]
        );
        assert_eq!(
            recent.counts_at(at(2_600), Duration::from_millis(600)),
            [0; 60]
        );
    }

    #[test]
    fn a_span_shorter_than_one_slot_reads_the_slot_of_now() {
        let recent = Recent::default();
        let start = recent.origin;
        let at = |milliseconds: u64| start + Duration::from_millis(milliseconds);
        recent.count_at(at(25));
        recent.count_at(at(29));
        assert_eq!(recent.counts_at(at(29), Duration::from_millis(5)), [2]);
        assert_eq!(recent.counts_at(at(29), Duration::ZERO), [2]);
    }
}
