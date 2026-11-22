//! When the notice "You are muted" is shown: after continuous speech while the microphone
//! is muted, and until some time after the speech ended (ADR 0735).

use std::time::{Duration, Instant};

/// How long the speech lasts before the notice appears.
pub const ONSET: Duration = Duration::from_millis(2500);
/// A pause shorter than this keeps the speech continuous.
pub const PAUSE: Duration = Duration::from_millis(800);
/// The notice stays this long after the last speech.
pub const HOLD: Duration = Duration::from_secs(5);
/// The statement that the notice was turned off stays this long before the notice fades.
pub const TURNED_OFF: Duration = Duration::from_secs(3);
/// The fade of the notice after the statement.
pub const FADE: Duration = Duration::from_millis(400);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MutedSpeech {
    /// The start of the speech that is going on.
    started: Option<Instant>,
    /// The input reached the threshold at the last step. The levels change only when the
    /// input changes, so a steady level is speech that goes on.
    heard: bool,
    /// The moment at which the input fell below the threshold.
    quiet_since: Option<Instant>,
    shown: bool,
    /// The person turned the notice off while it was shown, at this moment; the notice
    /// says so until [`TURNED_OFF`] after the later of this moment and the end of a hold.
    turned_off: Option<Instant>,
    /// The statement has had its time and the notice fades out.
    fading: bool,
    /// The pointer is over the notice or its check box has the keyboard focus.
    held: bool,
}

impl MutedSpeech {
    /// The state at `now`. `heard`: the input of the microphone reaches the threshold of
    /// speech. `muted`: the person is in a connected call with the microphone muted.
    /// Returns whether what the notice shows changed.
    pub fn step(&mut self, now: Instant, heard: bool, muted: bool) -> bool {
        let before = self.drawn();
        if !muted {
            *self = Self {
                held: self.held,
                ..Self::default()
            };
            return before != self.drawn();
        }
        match (self.heard, heard) {
            (false, true) => {
                let continues = self.started.is_some()
                    && (self.shown
                        || self
                            .quiet_since
                            .is_some_and(|quiet| now.saturating_duration_since(quiet) <= PAUSE));
                if !continues {
                    self.started = Some(now);
                }
                self.quiet_since = None;
            }
            (true, false) => self.quiet_since = Some(now),
            _ => {}
        }
        self.heard = heard;
        let quiet = self
            .quiet_since
            .map(|quiet| now.saturating_duration_since(quiet));
        if self.shown {
            if quiet.is_some_and(|quiet| quiet >= HOLD) {
                self.shown = false;
                self.started = None;
            }
        } else if quiet.is_some_and(|quiet| quiet > PAUSE) {
            self.started = None;
        } else if self.heard
            && self
                .started
                .is_some_and(|start| now.saturating_duration_since(start) >= ONSET)
        {
            self.shown = true;
        }
        if let Some(at) = self.turned_off
            && !self.held
        {
            let passed = now.saturating_duration_since(at);
            if passed >= TURNED_OFF + FADE {
                self.turned_off = None;
                self.fading = false;
                self.started = None;
                self.shown = false;
            } else if passed >= TURNED_OFF {
                self.fading = true;
            }
        }
        before != self.drawn()
    }

    fn drawn(&self) -> (bool, bool, bool) {
        (self.shown(), self.turned_off(), self.fading)
    }

    /// The notice fades out after the statement that it was turned off.
    pub fn fading(&self) -> bool {
        self.fading
    }

    /// The notice is drawn.
    pub fn shown(&self) -> bool {
        self.shown || self.turned_off.is_some()
    }

    /// The notice states that it was turned off.
    pub fn turned_off(&self) -> bool {
        self.turned_off.is_some()
    }

    /// The person unchecked the box while the notice was drawn.
    pub fn turn_off(&mut self, now: Instant) {
        if self.shown() {
            self.turned_off = Some(now);
        }
    }

    /// The person checked the box again while the notice said that it was turned off.
    pub fn turn_on(&mut self) {
        self.turned_off = None;
        self.fading = false;
    }

    /// The pointer came over the notice or left it, or its check box took or lost the
    /// keyboard focus. The end of a hold starts the time of the statement again.
    pub fn hold(&mut self, held: bool, now: Instant) {
        if self.held && !held && self.turned_off.is_some() {
            self.turned_off = Some(now);
        }
        if held {
            self.fading = false;
        }
        self.held = held;
    }

    /// When the drawn notice can change without a change of the input: the moment to step
    /// again.
    pub fn wake(&self) -> Option<Instant> {
        let speech = match (self.shown, self.heard, self.started, self.quiet_since) {
            (false, true, Some(start), _) => Some(start + ONSET),
            (true, false, _, Some(quiet)) => Some(quiet + HOLD),
            _ => None,
        };
        let statement = self.turned_off.filter(|_| !self.held).map(|at| {
            if self.fading {
                at + TURNED_OFF + FADE
            } else {
                at + TURNED_OFF
            }
        });
        match (speech, statement) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TICK: Duration = Duration::from_millis(50);

    /// Speech from `from` for `length`, with the levels changing at every tick, and the
    /// fall below the threshold at its end. Returns the end.
    fn speak(state: &mut MutedSpeech, from: Instant, length: Duration) -> Instant {
        let mut at = from;
        while at < from + length {
            state.step(at, true, true);
            at += TICK;
        }
        state.step(at, false, true);
        at
    }

    #[test]
    fn the_notice_appears_after_continuous_speech_with_short_pauses() {
        let start = Instant::now();
        let mut state = MutedSpeech::default();
        // Single words with longer pauses never show it.
        let mut at = start;
        for _ in 0..10 {
            at = speak(&mut state, at, Duration::from_millis(600));
            at += Duration::from_secs(1);
            state.step(at, false, true);
            assert!(!state.shown());
        }
        // Speech with pauses shorter than the limit shows it after the onset.
        at = speak(&mut state, at, Duration::from_millis(800));
        at = speak(&mut state, at + Duration::from_millis(600), Duration::from_millis(800));
        assert!(!state.shown());
        speak(&mut state, at + TICK, Duration::from_millis(400));
        assert!(state.shown());
    }

    /// A steady level changes no level: the speech goes on, and the wake shows the notice
    /// at its onset.
    #[test]
    fn a_steady_level_of_speech_shows_the_notice_at_its_onset() {
        let start = Instant::now();
        let mut state = MutedSpeech::default();
        state.step(start, true, true);
        assert_eq!(state.wake(), Some(start + ONSET));
        assert!(!state.step(start + ONSET - TICK, true, true));
        assert!(state.step(start + ONSET, true, true));
        assert!(state.shown());
        assert_eq!(state.wake(), None);
    }

    #[test]
    fn pauses_keep_the_notice_and_it_goes_after_the_hold() {
        let start = Instant::now();
        let mut state = MutedSpeech::default();
        let mut at = speak(&mut state, start, ONSET + TICK);
        assert!(state.shown());
        // Pauses of two and four seconds between words do not hide it.
        for pause in [2, 4, 3] {
            at += Duration::from_secs(pause);
            assert!(!state.step(at, false, true));
            assert!(state.shown());
            at = speak(&mut state, at, Duration::from_millis(300));
        }
        // It goes by itself after the hold, at the moment that it asks to be woken.
        assert_eq!(state.wake(), Some(at + HOLD));
        assert!(!state.step(at + HOLD - TICK, false, true));
        assert!(state.step(at + HOLD, false, true));
        assert!(!state.shown());
        assert_eq!(state.wake(), None);
    }

    #[test]
    fn an_unmute_hides_the_notice_and_the_next_mute_needs_new_speech() {
        let start = Instant::now();
        let mut state = MutedSpeech::default();
        let at = speak(&mut state, start, ONSET + TICK);
        assert!(state.shown());
        assert!(state.step(at + TICK, true, false));
        assert!(!state.shown());
        state.step(at + TICK * 2, true, true);
        assert!(!state.shown());
    }

    #[test]
    fn turned_off_the_notice_says_so_for_three_seconds_and_waits_while_held() {
        let start = Instant::now();
        let mut state = MutedSpeech::default();
        let at = speak(&mut state, start, ONSET + TICK);
        state.turn_off(at);
        assert!(state.shown() && state.turned_off());
        // Checked again within the time: the notice is as before.
        state.turn_on();
        assert!(state.shown() && !state.turned_off());
        state.turn_off(at);
        // The pointer over the notice keeps it past the three seconds.
        state.hold(true, at + Duration::from_secs(1));
        state.step(at + TURNED_OFF + TICK, false, true);
        assert!(state.turned_off());
        assert_eq!(state.wake(), Some(at + HOLD));
        let left = at + Duration::from_secs(4);
        state.hold(false, left);
        assert_eq!(state.wake(), Some((at + HOLD).min(left + TURNED_OFF)));
        assert!(!state.step(left + TURNED_OFF - TICK, false, true));
        // Then it fades and goes.
        assert!(state.step(left + TURNED_OFF, false, true));
        assert!(state.shown() && state.fading());
        assert_eq!(state.wake(), Some(left + TURNED_OFF + FADE));
        assert!(state.step(left + TURNED_OFF + FADE, false, true));
        assert!(!state.shown() && !state.fading());
    }

    #[test]
    fn the_notice_is_not_turned_off_while_it_is_not_drawn() {
        let mut state = MutedSpeech::default();
        state.turn_off(Instant::now());
        assert!(!state.shown() && !state.turned_off());
    }
}
