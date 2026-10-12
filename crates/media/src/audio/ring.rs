//! The incoming-call tone uses the same injectable output device as call sound.

use super::{AudioError, AudioHost, DeviceId, OpenStream};
use std::sync::Arc;

pub struct RingPlayer {
    host: Arc<dyn AudioHost>,
    stream: Option<OpenStream>,
}
impl RingPlayer {
    pub fn new(host: Arc<dyn AudioHost>) -> Self {
        Self { host, stream: None }
    }
    pub fn is_ringing(&self) -> bool {
        self.stream.is_some()
    }

    pub fn set_ringing(
        &mut self,
        ringing: bool,
        device: Option<&DeviceId>,
    ) -> Result<(), AudioError> {
        if !ringing {
            self.stream = None;
            return Ok(());
        }
        if self.stream.is_some() {
            return Ok(());
        }
        let mut elapsed = 0.0_f64;
        self.stream = Some(self.host.open_output(
            device,
            Box::new(move |samples, format| {
                for frame in samples.chunks_mut(usize::from(format.channels.max(1))) {
                    let cycle = elapsed % 3.0;
                    let amplitude = if cycle < 0.8 { 0.08 } else { 0.0 };
                    let sample = (elapsed * 440.0 * std::f64::consts::TAU).sin() as f32 * amplitude;
                    frame.fill(sample);
                    elapsed += 1.0 / f64::from(format.sample_rate.max(1));
                }
            }),
            Box::new(|error| tracing::debug!(%error, "incoming call sound stopped")),
        )?);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::synthetic::{SyntheticHost, tone_level};
    #[test]
    fn ring_sound_uses_the_injected_output_and_stops() {
        let host = SyntheticHost::with_tone(700.0);
        let mut ring = RingPlayer::new(Arc::new(host.clone()));
        ring.set_ringing(true, None).unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let (samples, rate) = host.played(std::time::Duration::from_millis(100));
            if tone_level(&samples, rate, 440.0) > -40.0 {
                break;
            }
            assert!(
                std::time::Instant::now() < until,
                "ring did not reach the injected output"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        ring.set_ringing(false, None).unwrap();
        assert!(ring.stream.is_none());
    }
}
