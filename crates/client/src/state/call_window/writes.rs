use junto_client_core::call_window::CallWindowSettings;

#[derive(Default)]
pub(crate) struct Writes {
    pending: Option<CallWindowSettings>,
    running: bool,
}

impl Writes {
    pub(crate) fn push(&mut self, settings: CallWindowSettings) -> bool {
        self.pending = Some(settings);
        !std::mem::replace(&mut self.running, true)
    }
    pub(crate) fn take(&mut self) -> Option<CallWindowSettings> {
        let pending = self.pending.take();
        if pending.is_none() {
            self.running = false;
        }
        pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn moving_a_window_coalesces_placements_and_keeps_the_last_write() {
        let mut writes = Writes::default();
        let settings = |x| CallWindowSettings {
            x: Some(x),
            y: Some(40),
            ..Default::default()
        };
        assert!(writes.push(settings(0)));
        for x in 1..100 {
            assert!(!writes.push(settings(x)));
        }
        assert_eq!(writes.take(), Some(settings(99)));
        assert!(!writes.push(settings(100)));
        assert_eq!(writes.take(), Some(settings(100)));
        assert_eq!(writes.take(), None);
        assert!(writes.push(settings(101)));
        assert_eq!(writes.take(), Some(settings(101)));
    }
}
