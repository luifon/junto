use super::RingOutcome;
use std::time::Instant;

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
    pub(super) fn observe(
        &mut self,
        other_present: Option<bool>,
        now: Instant,
    ) -> Option<RingOutcome> {
        if other_present == Some(true) {
            self.answered = true;
        }
        if self.answered {
            (other_present == Some(false)).then_some(RingOutcome::Ended)
        } else {
            (now.duration_since(self.started) >= std::time::Duration::from_secs(60))
                .then_some(RingOutcome::Missed)
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
            call.observe(Some(false), start + Duration::from_secs(59)),
            None
        );
        assert_eq!(
            call.observe(Some(false), start + Duration::from_secs(60)),
            Some(RingOutcome::Missed)
        );
    }

    #[test]
    fn an_answered_direct_call_ends_when_the_other_person_leaves() {
        let start = Instant::now();
        let mut call = Lifetime::new(start);
        assert_eq!(call.observe(Some(true), start), None);
        assert_eq!(call.observe(None, start + Duration::from_secs(90)), None);
        assert_eq!(
            call.observe(Some(false), start + Duration::from_secs(91)),
            Some(RingOutcome::Ended)
        );
    }

    #[test]
    fn a_missing_resolution_does_not_leave_an_outgoing_call_forever() {
        let start = Instant::now();
        let mut call = Lifetime::new(start);
        assert_eq!(
            call.observe(None, start + Duration::from_secs(60)),
            Some(RingOutcome::Missed)
        );
    }
}
