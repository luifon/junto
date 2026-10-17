//! The delivery of the pictures and the sound of a capture that does not give them in the
//! form and at the times that the engine takes them: Linux (ADR 0751) and Windows (ADR 0706).
//!
//! The capture puts the newest picture into a slot and the samples of its sound into a
//! bounded queue. One thread takes a picture from the slot when the next one is due and
//! converts it; another takes 10 ms of sound at the times of this machine's clock.

pub(crate) mod latest;
pub(crate) mod pixels;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use super::{CaptureError, CaptureSink, ShareQuality};
use crate::audio::Format;
pub(crate) use latest::Latest;

/// The frames that the queue of captured sound holds: 400 ms. The delivery keeps far fewer
/// in it; the room is for the portions of the capture and for a late reader.
pub(crate) const CAPACITY: usize = 48_000 * 2 / 5;

/// The state of one capture that its threads share.
#[derive(Default)]
pub(crate) struct Life {
    stopped: AtomicBool,
    /// The owner of the capture stopped it; the sink is not told an end.
    pub cancelled: AtomicBool,
    failure: Mutex<Option<CaptureError>>,
}
impl Life {
    pub fn stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
    }
    pub fn fail(&self, failure: CaptureError) {
        let mut held = self.failure.lock().unwrap_or_else(PoisonError::into_inner);
        if !self.stopped() {
            *held = Some(failure);
            self.stop();
        }
    }
    pub fn result(&self) -> Result<(), CaptureError> {
        self.failure
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .map_or(Ok(()), Err)
    }
}

/// Delivers pictures on this thread and sound on a thread of its own, so that the
/// conversion of a large picture does not delay the sound. Both have ended before the
/// sink is told that the capture ended.
///
/// The limit of the frame rate is here and not at the capture. A picture that arrives
/// before the next one is due waits in the slot, where a newer one replaces it: the last
/// picture of a screen that then stays still is always sent.
///
/// A capture puts into the slot either the picture itself or what `read` makes the picture
/// from when it is due: the capture of Windows keeps the newest picture on the graphics card
/// and reads it into memory only for a picture that is sent.
pub(crate) fn deliver<P: Send>(
    life: Arc<Life>,
    sink: Arc<dyn CaptureSink>,
    pictures: Arc<Latest<P>>,
    mut read: impl FnMut(P) -> Result<Option<pixels::Frame>, CaptureError> + Send,
    samples: Option<rtrb::Consumer<[f32; 2]>>,
    quality: ShareQuality,
) {
    std::thread::scope(|scope| {
        let sound = samples.map(|samples| {
            std::thread::Builder::new()
                .name("share-sound-delivery".into())
                .spawn_scoped(scope, || deliver_sound(&life, sink.as_ref(), samples))
        });
        if let Some(Err(failure)) = sound {
            life.fail(CaptureError::Failed(failure.to_string()));
        }
        let period = Duration::from_secs_f64(1.0 / f64::from(quality.frame_rate));
        let mut due = Instant::now();
        while pictures.pause(due, || life.stopped())
            && let Some(frame) = pictures.take(|| life.stopped())
        {
            // The times of a steady source stay on one grid; after a time without
            // pictures the grid starts anew.
            due = (due + period).max(Instant::now() + period / 2);
            let frame = match read(frame) {
                Ok(Some(frame)) => frame,
                Ok(None) => continue,
                Err(failure) => {
                    life.fail(failure);
                    break;
                }
            };
            let buffer = frame.convert(quality.resolution);
            if !life.stopped() {
                sink.picture(Arc::new(buffer));
            }
        }
    });
    if !life.cancelled.load(Ordering::Acquire)
        && let Err(failure) = life.result()
    {
        sink.ended(failure);
    }
}

fn deliver_sound(life: &Life, sink: &dyn CaptureSink, samples: rtrb::Consumer<[f32; 2]>) {
    let mut clock = SoundClock::new(samples);
    let mut next = Instant::now();
    while !life.stopped() {
        let now = Instant::now();
        if now >= next {
            let block = clock.tick(now);
            if !life.stopped() {
                sink.sound(
                    &block,
                    Format {
                        sample_rate: 48_000,
                        channels: 2,
                    },
                );
            }
            next += TICK;
            // After a stall the missed blocks are not sent late; the clock removes what
            // the capture queued in that time.
            if now.saturating_duration_since(next) > Duration::from_millis(50) {
                next = now + TICK;
            }
        }
        std::thread::sleep(next.saturating_duration_since(Instant::now()));
    }
}

const TICK: Duration = Duration::from_millis(10);
/// Frames of one block of 10 ms at 48 kHz.
const BLOCK: usize = 480;
/// Ticks of one window in which the clock measures the lowest level of the queue.
const WINDOW: u32 = 100;
/// The clock holds the lowest level of a window between these two.
const LOW: usize = 2 * BLOCK;
const ENOUGH: usize = 8 * BLOCK;
/// A lowest level above this is sound that only waits: it is removed in one step.
const SURPLUS: usize = 12 * BLOCK;
/// The lowest level after such a removal.
const KEPT: usize = 5 * BLOCK;

/// Takes blocks of 10 ms from the queue at the times of this machine's clock.
///
/// The capture writes at the rate of the sound graph, in portions of the size that the
/// graph chose, and the graph has another clock than this machine's. The level of the queue
/// therefore rises and falls with each portion and moves slowly with the difference of the
/// clocks. The clock measures the lowest level of each second, which is what remains just
/// before a portion arrives. While that is outside a band, a block is made from one frame
/// more or less than its length, with linear interpolation: a difference of the clocks of
/// up to 0.2 % changes the pitch by that much and breaks nothing. The band holds the level
/// that a start leaves, so the pitch changes only while the clocks have moved apart. Sound
/// that only waits, which a late reader leaves behind, is removed in one step.
pub(crate) struct SoundClock {
    samples: rtrb::Consumer<[f32; 2]>,
    prime: Option<Instant>,
    playing: bool,
    /// The lowest number of frames that a block left in the queue in the present window.
    lowest: usize,
    /// Blocks taken in the present window.
    ticks: u32,
    /// Frames taken for a block beyond its length: -1, 0 or 1.
    extra: i8,
    /// Steps in the sound that the clock made: an empty queue or a removal.
    pub breaks: u32,
}
impl SoundClock {
    pub fn new(samples: rtrb::Consumer<[f32; 2]>) -> Self {
        Self {
            samples,
            prime: None,
            playing: false,
            lowest: usize::MAX,
            ticks: 0,
            extra: 0,
            breaks: 0,
        }
    }

    fn measure_anew(&mut self) {
        self.lowest = usize::MAX;
        self.ticks = 0;
        self.extra = 0;
    }

    /// Removes the oldest frames. One step in the sound.
    fn remove(&mut self, frames: usize) {
        if let Ok(oldest) = self.samples.read_chunk(frames) {
            oldest.commit_all();
        }
        self.breaks += 1;
        self.measure_anew();
    }

    pub fn tick(&mut self, now: Instant) -> [f32; 2 * BLOCK] {
        let mut out = [0.0; 2 * BLOCK];
        if !self.playing {
            if self.samples.slots() == 0 {
                return out;
            }
            let prime = self.prime.get_or_insert(now + Duration::from_millis(60));
            if now < *prime {
                return out;
            }
            self.playing = true;
            self.measure_anew();
        }
        // A full queue refuses what the capture writes, frame by frame. What stays after
        // this removal is no surplus, so the end of the window removes nothing more.
        let waiting = self.samples.slots();
        if waiting + 2 * BLOCK > self.samples.buffer().capacity() {
            self.remove(waiting.saturating_sub(SURPLUS));
        }
        let waiting = self.samples.slots();
        let take = (BLOCK as isize + isize::from(self.extra)) as usize;
        if waiting < take {
            // The queue ran empty: what is left is sent, and the next start waits again.
            for frame in out.as_chunks_mut::<2>().0 {
                match self.samples.pop() {
                    Ok(pair) => frame.copy_from_slice(&pair),
                    Err(_) => break,
                }
            }
            self.playing = false;
            self.prime = None;
            self.breaks += 1;
            return out;
        }
        let mut taken = [[0.0_f32; 2]; BLOCK + 2];
        for frame in &mut taken[..take] {
            if let Ok(pair) = self.samples.pop() {
                *frame = pair;
            }
        }
        // The frame after the block ends the last interpolation; it stays in the queue.
        taken[take] = self.samples.peek().copied().unwrap_or(taken[take - 1]);
        for (index, frame) in out.as_chunks_mut::<2>().0.iter_mut().enumerate() {
            let place = index as f32 * take as f32 / BLOCK as f32;
            let (before, part) = (place as usize, place.fract());
            let (first, second) = (taken[before], taken[before + 1]);
            *frame = [
                first[0] * (1.0 - part) + second[0] * part,
                first[1] * (1.0 - part) + second[1] * part,
            ];
        }
        self.lowest = self.lowest.min(waiting - take);
        self.ticks += 1;
        if self.ticks == WINDOW {
            if self.lowest > SURPLUS {
                self.remove(self.lowest - KEPT);
            } else {
                let extra = if self.lowest > ENOUGH {
                    1
                } else if self.lowest < LOW {
                    -1
                } else {
                    0
                };
                self.measure_anew();
                self.extra = extra;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_audio_clock_supplies_silence_through_a_pause_and_restarts() {
        let (mut input, output) = rtrb::RingBuffer::new(4_800);
        let mut clock = SoundClock::new(output);
        let now = Instant::now();
        assert_eq!(clock.tick(now), [0.0; 960]);
        for _ in 0..960 {
            assert!(input.push([0.2, 0.3]).is_ok());
        }
        assert_eq!(clock.tick(now), [0.0; 960]);
        assert_eq!(
            &clock.tick(now + Duration::from_millis(60))[..4],
            &[0.2, 0.3, 0.2, 0.3]
        );
        assert_eq!(
            &clock.tick(now + Duration::from_millis(70))[..4],
            &[0.2, 0.3, 0.2, 0.3]
        );
        for tick in 8..20 {
            assert_eq!(
                clock.tick(now + Duration::from_millis(tick * 10)),
                [0.0; 960]
            );
        }
        for _ in 0..480 {
            assert!(input.push([0.4, 0.5]).is_ok());
        }
        assert_eq!(clock.tick(now + Duration::from_millis(200)), [0.0; 960]);
        assert_eq!(
            &clock.tick(now + Duration::from_millis(260))[..4],
            &[0.4, 0.5, 0.4, 0.5]
        );
    }

    /// Keeps the red value of the middle of each delivered picture.
    #[derive(Default)]
    struct Reds(Mutex<Vec<u8>>);
    #[allow(clippy::unwrap_used)]
    impl CaptureSink for Reds {
        fn picture(&self, frame: Arc<dyn livekit::webrtc::video_frame::VideoBuffer>) {
            let picture = crate::share::picture::convert(frame.as_ref(), (64, 64)).unwrap();
            self.0
                .lock()
                .unwrap()
                .push(picture.pixel(32, 32).unwrap().0);
        }
        fn sound(&self, _: &[f32], _: Format) {
            panic!("sound was not selected");
        }
        fn ended(&self, error: CaptureError) {
            panic!("{error}");
        }
    }

    #[allow(clippy::unwrap_used)]
    fn red(value: u8) -> pixels::Frame {
        let pixels = [0, 0, value, 255].repeat(64 * 64);
        pixels::Frame::copy(&pixels, (64, 64), 256, None, pixels::Format::Bgrx).unwrap()
    }

    fn quality(frame_rate: u32) -> crate::share::ShareQuality {
        crate::share::ShareQuality {
            resolution: crate::share::Resolution::P720,
            frame_rate,
        }
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn a_source_faster_than_the_frame_rate_is_limited_and_its_last_picture_is_sent() {
        let life = Arc::new(Life::default());
        let pictures = Arc::new(Latest::default());
        let sink = Arc::new(Reds::default());
        let delivery = std::thread::spawn({
            let (life, sink, pictures) = (life.clone(), sink.clone(), pictures.clone());
            move || {
                deliver(
                    life,
                    sink,
                    pictures,
                    |frame| Ok(Some(frame)),
                    None,
                    quality(30),
                )
            }
        });
        // One second at 60 pictures a second; then the screen stays still.
        let started = Instant::now();
        for index in 0..60_u32 {
            pictures.put(red(if index == 59 { 240 } else { 40 }));
            std::thread::sleep(
                (started + Duration::from_secs(1) * (index + 1) / 60)
                    .saturating_duration_since(Instant::now()),
            );
        }
        while sink.0.lock().unwrap().last().is_none_or(|last| *last < 200) {
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "the last picture was not sent: {:?}",
                sink.0.lock().unwrap()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        life.cancelled.store(true, Ordering::Release);
        life.stop();
        pictures.wake();
        delivery.join().unwrap();
        let sent = sink.0.lock().unwrap().len();
        // 30 a second for a little more than one second, and not the 60 of the source.
        assert!((10..=36).contains(&sent), "{sent} pictures were sent");
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn a_delivery_that_starts_late_sends_the_newest_picture_and_no_older_one() {
        let life = Arc::new(Life::default());
        let pictures = Arc::new(Latest::default());
        // The capture runs while the start still prepares the sound.
        for value in [10, 60, 110, 160, 210] {
            pictures.put(red(value));
        }
        let sink = Arc::new(Reds::default());
        let delivery = std::thread::spawn({
            let (life, sink, pictures) = (life.clone(), sink.clone(), pictures.clone());
            move || {
                deliver(
                    life,
                    sink,
                    pictures,
                    |frame| Ok(Some(frame)),
                    None,
                    quality(30),
                )
            }
        });
        let started = Instant::now();
        while sink.0.lock().unwrap().is_empty() {
            assert!(started.elapsed() < Duration::from_secs(20), "no picture");
            std::thread::sleep(Duration::from_millis(5));
        }
        life.cancelled.store(true, Ordering::Release);
        life.stop();
        pictures.wake();
        delivery.join().unwrap();
        let reds = sink.0.lock().unwrap().clone();
        assert_eq!(reds.len(), 1, "{reds:?}");
        assert!((i16::from(reds[0]) - 210).abs() < 4, "{reds:?}");
    }

    /// A capture that writes portions of `quantum` frames at `rate` times the rate of the
    /// reader.
    struct Run {
        input: rtrb::Producer<[f32; 2]>,
        clock: SoundClock,
        start: Instant,
        quantum: usize,
        owed: f64,
        refused: u32,
    }
    impl Run {
        fn new(quantum: usize) -> Self {
            let (input, output) = rtrb::RingBuffer::new(CAPACITY);
            Self {
                input,
                clock: SoundClock::new(output),
                start: Instant::now(),
                quantum,
                owed: 0.0,
                refused: 0,
            }
        }
        fn ticks(&mut self, ticks: std::ops::Range<u32>, rate: f64) {
            for tick in ticks {
                self.owed += BLOCK as f64 * rate;
                while self.owed >= self.quantum as f64 {
                    self.owed -= self.quantum as f64;
                    for _ in 0..self.quantum {
                        if self.input.push([0.1, 0.1]).is_err() {
                            self.refused += 1;
                        }
                    }
                }
                self.clock.tick(self.start + TICK * tick);
            }
        }
    }

    #[test]
    fn a_capture_clock_that_is_faster_or_slower_never_breaks_the_sound() {
        for quantum in [1024, 2048, 4096, 8192] {
            for rate in [1.001, 0.999, 1.0] {
                let mut run = Run::new(quantum);
                // Ten minutes.
                run.ticks(0..60_000, rate);
                let case = format!("at rate {rate} with portions of {quantum} frames");
                assert_eq!(run.refused, 0, "the queue refused frames {case}");
                assert_eq!(run.clock.breaks, 0, "the sound broke {case}");
                assert!(run.clock.playing, "the sound stopped {case}");
                // Just before a portion arrives the queue holds no more than the band.
                let most = ENOUGH + quantum + 2 * BLOCK;
                let waiting = run.clock.samples.slots();
                assert!(waiting <= most, "{waiting} frames wait {case}");
            }
        }
    }

    #[test]
    fn a_queue_that_is_nearly_full_is_brought_back_with_one_break() {
        let mut run = Run::new(1024);
        run.ticks(0..1_000, 1.0);
        while run.clock.samples.slots() < 18_000 {
            assert!(run.input.push([0.1, 0.1]).is_ok());
        }
        run.ticks(1_000..1_300, 1.0);
        assert_eq!((run.refused, run.clock.breaks), (0, 1));
        run.ticks(1_300..10_000, 1.0);
        assert_eq!((run.refused, run.clock.breaks), (0, 1));
    }

    #[test]
    fn a_reader_that_was_late_removes_the_sound_that_waited_with_one_break() {
        let mut run = Run::new(1024);
        run.ticks(0..1_000, 1.0);
        assert_eq!((run.refused, run.clock.breaks), (0, 0));
        // The reader missed its ticks while the capture went on writing.
        while run.clock.samples.slots() < 12_000 {
            assert!(run.input.push([0.1, 0.1]).is_ok());
        }
        run.ticks(1_000..1_200, 1.0);
        assert_eq!((run.refused, run.clock.breaks), (0, 1));
        run.ticks(1_200..10_000, 1.0);
        assert_eq!((run.refused, run.clock.breaks), (0, 1));
        let waiting = run.clock.samples.slots();
        assert!(waiting <= ENOUGH + 1024 + 2 * BLOCK, "{waiting} frames");
    }
}
