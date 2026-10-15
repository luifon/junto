use super::RingOutcome;
use std::time::Instant;

/// The end of a direct call that this device is in. The call ends when the other person
/// left after being inside; before that, when the ring of the call was declined or not
/// answered, or at the latest 60 seconds after the call started on this device.
pub(super) struct Lifetime {
    started: Instant,
    answered: bool,
}
impl Lifetime {
    pub(super) fn new(now: Instant) -> Self {
        Self {
            started: now,
            answered: false,
        }
    }

    /// The outcome with which the call ends now, while it has not ended. `other_present` is whether the
    /// state of the server lists the other person in the call, and `ring` is the outcome
    /// of the newest ring that this account made for the call. Both are read again at each
    /// call, so the order in which they arrive does not change the outcome.
    pub(super) fn observe(
        &mut self,
        other_present: Option<bool>,
        ring: Option<RingOutcome>,
        now: Instant,
    ) -> Option<RingOutcome> {
        if other_present == Some(true) {
            self.answered = true;
        }
        if self.answered {
            (other_present == Some(false)).then_some(RingOutcome::Ended)
        } else {
            match ring {
                Some(outcome @ (RingOutcome::Declined | RingOutcome::Missed)) => Some(outcome),
                _ => (now.duration_since(self.started) >= std::time::Duration::from_secs(60))
                    .then_some(RingOutcome::Missed),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_direct_call_without_a_ring_ends_at_the_same_deadline_as_no_answer() {
        let start = Instant::now();
        let mut call = Lifetime::new(start);
        assert_eq!(
            call.observe(Some(false), None, start + Duration::from_secs(59)),
            None
        );
        assert_eq!(
            call.observe(Some(false), None, start + Duration::from_secs(60)),
            Some(RingOutcome::Missed)
        );
    }

    #[test]
    fn an_answered_direct_call_ends_when_the_other_person_leaves() {
        let start = Instant::now();
        let mut call = Lifetime::new(start);
        assert_eq!(call.observe(Some(true), None, start), None);
        assert_eq!(
            call.observe(None, None, start + Duration::from_secs(90)),
            None
        );
        assert_eq!(
            call.observe(Some(false), None, start + Duration::from_secs(91)),
            Some(RingOutcome::Ended)
        );
    }

    #[test]
    fn a_missing_resolution_does_not_leave_an_outgoing_call_forever() {
        let start = Instant::now();
        let mut call = Lifetime::new(start);
        assert_eq!(
            call.observe(None, None, start + Duration::from_secs(60)),
            Some(RingOutcome::Missed)
        );
    }

    #[test]
    fn a_declined_or_unanswered_ring_ends_the_call_before_its_deadline() {
        let start = Instant::now();
        for ring in [RingOutcome::Declined, RingOutcome::Missed] {
            let mut call = Lifetime::new(start);
            assert_eq!(call.observe(None, Some(RingOutcome::Waiting), start), None);
            assert_eq!(call.observe(None, Some(ring), start), Some(ring));
            // A call that is still there after its end is left again with the same outcome.
            assert_eq!(call.observe(Some(false), Some(ring), start), Some(ring));
        }
    }

    #[test]
    fn the_ring_of_an_answered_call_does_not_end_it() {
        let start = Instant::now();
        let mut call = Lifetime::new(start);
        assert_eq!(
            call.observe(Some(true), Some(RingOutcome::Missed), start),
            None
        );
        assert_eq!(
            call.observe(Some(false), Some(RingOutcome::Missed), start),
            Some(RingOutcome::Ended)
        );
    }
}
