//! The incoming-call tone uses the same injectable output device as call sound.
//!
//! Opening and closing an output device are calls into the sound system of the platform,
//! which can wait for that system, or for another stream of the same host that is being
//! opened. The window asks for the tone from its own thread, so the device is opened and
//! closed on a thread of the player, and a request only sends the wanted state to it.

use super::{AudioHost, DeviceId, OpenStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

pub struct RingPlayer {
    wanted: Option<Option<DeviceId>>,
    thread: Option<Sender<Option<Option<DeviceId>>>>,
    failed: Arc<AtomicBool>,
}

impl RingPlayer {
    pub fn new(host: Arc<dyn AudioHost>) -> Self {
        let (requests, received) = channel();
        let failed = Arc::new(AtomicBool::new(false));
        let reported = failed.clone();
        let thread = std::thread::Builder::new()
            .name("junto-ring".to_owned())
            .spawn(move || sound(host.as_ref(), &received, &reported));
        let thread = match thread {
            Ok(_) => Some(requests),
            Err(error) => {
                tracing::warn!(%error, "the incoming call sound has no thread");
                None
            }
        };
        Self {
            wanted: None,
            thread,
            failed,
        }
    }

    /// Whether the tone is asked for. The device opens a moment later on the thread of
    /// the player.
    pub fn is_ringing(&self) -> bool {
        self.wanted.is_some()
    }

    /// Asks for the tone on a device, or for silence. It returns at once. A device that
    /// did not open is tried again at the next request for the tone.
    pub fn set_ringing(&mut self, ringing: bool, device: Option<&DeviceId>) {
        let wanted = ringing.then(|| device.cloned());
        let retry = wanted.is_some() && self.failed.swap(false, Ordering::AcqRel);
        if wanted == self.wanted && !retry {
            return;
        }
        if let Some(thread) = &self.thread {
            let _ = thread.send(wanted.clone());
        }
        self.wanted = wanted;
    }
}

/// The thread of the player: it holds the open stream and follows the newest request.
/// It ends when the player is dropped, and the stream closes with it.
fn sound(host: &dyn AudioHost, requests: &Receiver<Option<Option<DeviceId>>>, failed: &AtomicBool) {
    let mut open: Option<(Option<DeviceId>, OpenStream)> = None;
    while let Ok(mut wanted) = requests.recv() {
        while let Ok(newer) = requests.try_recv() {
            wanted = newer;
        }
        let Some(device) = wanted else {
            open = None;
            continue;
        };
        if open.as_ref().is_some_and(|(on, _)| *on == device) {
            continue;
        }
        open = None;
        match host.open_output(
            device.as_ref(),
            Box::new(tone()),
            Box::new(|error| tracing::debug!(%error, "incoming call sound stopped")),
        ) {
            Ok(stream) => open = Some((device, stream)),
            Err(error) => {
                tracing::debug!(%error, "incoming call sound could not start");
                failed.store(true, Ordering::Release);
            }
        }
    }
}

fn tone() -> impl FnMut(&mut [f32], super::Format) + Send + 'static {
    let mut elapsed = 0.0_f64;
    move |samples, format| {
        for frame in samples.chunks_mut(usize::from(format.channels.max(1))) {
            let cycle = elapsed % 3.0;
            let amplitude = if cycle < 0.8 { 0.08 } else { 0.0 };
            let sample = (elapsed * 440.0 * std::f64::consts::TAU).sin() as f32 * amplitude;
            frame.fill(sample);
            elapsed += 1.0 / f64::from(format.sample_rate.max(1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::synthetic::{SyntheticHost, tone_level};
    use crate::audio::test_output::TestOutput;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    const PATIENCE: Duration = Duration::from_secs(10);

    fn until_heard(host: &SyntheticHost) {
        let until = Instant::now() + PATIENCE;
        loop {
            let (samples, rate) = host.played(Duration::from_millis(100));
            if tone_level(&samples, rate, 440.0) > -40.0 {
                return;
            }
            assert!(
                Instant::now() < until,
                "ring did not reach the injected output"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn ring_sound_uses_the_injected_output_and_stops() {
        let host = SyntheticHost::with_tone(700.0);
        let (closed, close) = channel();
        let mut ring = RingPlayer::new(Arc::new(TestOutput {
            host: host.clone(),
            release: None,
            closed,
        }));
        ring.set_ringing(true, None);
        assert!(ring.is_ringing());
        until_heard(&host);
        ring.set_ringing(false, None);
        assert!(!ring.is_ringing());
        close
            .recv_timeout(PATIENCE)
            .expect("the output of the ring was closed");
    }

    #[test]
    fn a_device_that_is_slow_to_open_does_not_hold_the_caller() {
        let host = SyntheticHost::with_tone(700.0);
        let (release, held) = channel();
        let (closed, close) = channel();
        let player = RingPlayer::new(Arc::new(TestOutput {
            host: host.clone(),
            release: Some(Mutex::new(held)),
            closed,
        }));
        let (returned, asked) = channel();
        let caller = std::thread::spawn(move || {
            let mut player = player;
            player.set_ringing(true, None);
            let _ = returned.send(());
            player
        });
        let answer = asked.recv_timeout(PATIENCE);
        release.send(()).unwrap();
        assert!(
            answer.is_ok(),
            "the request for the tone waited for the device to open"
        );
        let player = caller.join().unwrap();
        until_heard(&host);
        drop(player);
        close
            .recv_timeout(PATIENCE)
            .expect("the output closed with the player");
    }

    #[test]
    fn a_device_that_did_not_open_is_tried_again_at_the_next_request() {
        let host = SyntheticHost::with_tone(700.0);
        let missing = DeviceId("unplugged".to_owned());
        let mut ring = RingPlayer::new(Arc::new(host.clone()));
        ring.set_ringing(true, Some(&missing));
        let until = Instant::now() + PATIENCE;
        while !ring.failed.load(Ordering::Acquire) {
            assert!(
                Instant::now() < until,
                "the missing device was not reported"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        ring.set_ringing(true, Some(&missing));
        let until = Instant::now() + PATIENCE;
        while host.opened_outputs().len() < 2 {
            assert!(Instant::now() < until, "the device was not tried again");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
