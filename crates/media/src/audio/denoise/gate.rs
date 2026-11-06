//! A gate on the probability of voice that the network of the noise suppressor gives for each
//! frame, measured for ADR 0733 and not run by the client: it keeps the starts of words only
//! when the sound waits 50 ms for it. The unit tests of `audio::denoise` and the measurement of
//! `tests/noise_quality.rs` run it after the network.

use std::collections::VecDeque;

/// The samples of one frame of 10 ms at 48 kHz.
const FRAME: usize = 480;

/// The settings of the gate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GateSettings {
    /// The probability of voice above which the gate opens.
    pub voice: f32,
    /// How many frames of 10 ms the gate stays open after the last frame with voice.
    pub hangover_frames: usize,
    /// The gain of the closed gate.
    pub closed: f32,
    /// The part of the way to the closed gain that one frame goes after the hangover.
    pub closing: f32,
    /// How many frames of 10 ms the sound waits, so that the gate opens before the frames
    /// whose probability rises late at the start of a word.
    pub lookahead_frames: usize,
}

/// The gate that keeps the starts of words in the measurement of ADR 0733: open above a
/// probability of 0.6, 80 ms of hangover, -25 dB when closed, and 50 ms of lookahead.
pub const GATE: GateSettings = GateSettings {
    voice: 0.6,
    hangover_frames: 8,
    closed: 0.056,
    closing: 0.05,
    lookahead_frames: 5,
};

/// The gains of the gate from the probabilities alone. It opens in the frame in which the
/// probability of a frame within its lookahead crosses the threshold, and closes slowly after
/// the hangover.
pub struct Gate {
    settings: GateSettings,
    gain: f32,
    hold: usize,
    /// The probabilities of the frames that wait, the oldest first.
    ahead: VecDeque<f32>,
}

impl Gate {
    pub fn new(settings: GateSettings) -> Self {
        Self {
            settings,
            gain: 1.0,
            hold: 0,
            ahead: VecDeque::new(),
        }
    }

    /// Takes the probability of the newest frame and gives the gain at the start and at the
    /// end of the frame that leaves the lookahead. Without a lookahead that is the frame of
    /// the probability.
    pub fn next(&mut self, voice: f32) -> (f32, f32) {
        self.ahead.push_back(voice);
        if self.ahead.len() > self.settings.lookahead_frames + 1 {
            self.ahead.pop_front();
        }
        if self.ahead.iter().any(|voice| *voice > self.settings.voice) {
            self.hold = self.settings.hangover_frames.max(1);
        } else {
            self.hold = self.hold.saturating_sub(1);
        }
        let start = self.gain;
        self.gain = if self.hold > 0 {
            1.0
        } else {
            self.gain + (self.settings.closed - self.gain) * self.settings.closing
        };
        (start, self.gain)
    }
}

/// The gate after the network: it takes each frame that the network gave with the
/// probability of voice that the network gave with it.
pub struct Gated {
    gate: Gate,
    lookahead_frames: usize,
    waiting: VecDeque<[i16; FRAME]>,
}

impl Gated {
    pub fn new(settings: GateSettings) -> Self {
        Self {
            gate: Gate::new(settings),
            lookahead_frames: settings.lookahead_frames,
            waiting: VecDeque::new(),
        }
    }

    /// The delay that the gate adds, in samples.
    pub fn delay(&self) -> usize {
        FRAME * self.lookahead_frames
    }

    /// Replaces the frame with the frame that leaves the lookahead, its gain moving along the
    /// frame so that a change makes no step in the sound; silence while the lookahead fills.
    pub fn process(&mut self, frame: &mut [i16; FRAME], voice: f32) {
        self.waiting.push_back(*frame);
        let (start, end) = self.gate.next(voice);
        if self.waiting.len() <= self.lookahead_frames {
            frame.fill(0);
            return;
        }
        let Some(leaving) = self.waiting.pop_front() else {
            return;
        };
        let per_sample = (end - start) / FRAME as f32;
        for (index, (to, from)) in frame.iter_mut().zip(leaving.iter()).enumerate() {
            let gain = start + per_sample * index as f32;
            *to = (f32::from(*from) * gain)
                .round()
                .clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16;
        }
    }
}
