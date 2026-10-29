use junto_client_core::window_placement::WindowPlacement;

#[derive(Default)]
pub(crate) struct Writes {
    pending: Option<WindowPlacement>,
    running: bool,
}

impl Writes {
    pub(crate) fn push(&mut self, settings: WindowPlacement) -> bool {
        self.pending = Some(settings);
        !std::mem::replace(&mut self.running, true)
    }
    pub(crate) fn take(&mut self) -> Option<WindowPlacement> {
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
        let settings = |x| WindowPlacement {
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
