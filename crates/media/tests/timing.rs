//! Timings that the documents state (ADR 0733, ADR 0774, `docs/client-performance.md`). They
//! print figures and check nothing; each runs only when it is named:
//!
//!   cargo test -p junto-media --test timing -- --ignored --nocapture
//!
//! The rate of the processing module is a setting of the vendored `webrtc-sys`: to compare two
//! rates, build this test once with each setting and run the two programs in turn.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Instant;

use junto_media::audio::processing::{Processing, Processor};
use junto_media::audio::{FRAME_SAMPLES, PLAYOUT_CHANNELS};
use junto_media::share::picture::VideoPicture;
use livekit::webrtc::video_frame::{I420Buffer, VideoBuffer};

/// The median of six rounds, after one round that is not counted.
fn median(mut round: impl FnMut() -> f64) -> (f64, Vec<f64>) {
    round();
    let mut rounds: Vec<f64> = (0..6).map(|_| round()).collect();
    rounds.sort_by(f64::total_cmp);
    (rounds[3], rounds)
}

/// One frame of the playout and one of the microphone through the processing module with the
/// echo canceller, the noise suppression and the gain control, in microseconds.
#[test]
#[ignore = "a timing: run it by name"]
fn the_processing_of_one_frame_of_each_direction() {
    const FRAMES: u32 = 3_000;
    let processor = Processor::new(Processing {
        echo_cancellation: true,
        noise_suppression: true,
        automatic_gain: true,
    });
    let (median, rounds) = median(|| {
        let started = Instant::now();
        for index in 0..FRAMES as usize {
            let mut played = vec![0i16; FRAME_SAMPLES * PLAYOUT_CHANNELS];
            for (n, sample) in played.iter_mut().enumerate() {
                *sample = (((index * FRAME_SAMPLES + n / 2) as f32 * 0.0573).sin() * 8000.0) as i16;
            }
            processor.render(&mut played);
            let mut frame = vec![0i16; FRAME_SAMPLES];
            for (n, sample) in frame.iter_mut().enumerate() {
                *sample = (((index * FRAME_SAMPLES + n) as f32 * 0.0311).sin() * 6000.0) as i16;
            }
            processor.capture(&mut frame);
        }
        started.elapsed().as_secs_f64() * 1e6 / f64::from(FRAMES)
    });
    println!(
        "the processing of one frame of each direction: median {median:.1} µs, rounds {rounds:.1?}"
    );
}

/// A received picture of 1920 × 1080 in planes scaled to 1128 × 634: from its planes where
/// they are, and after a copy of them as the engine made before (ADR 0774), in milliseconds.
#[test]
#[ignore = "a timing: run it by name"]
fn the_scale_of_a_picture_with_and_without_a_copy_of_its_planes() {
    const PICTURES: u32 = 1_000;
    let mut source = I420Buffer::new(1920, 1080);
    let (y, u, v) = source.data_mut();
    for (index, value) in y.iter_mut().enumerate() {
        *value = (index * 7 % 251) as u8;
    }
    for (index, value) in u.iter_mut().enumerate() {
        *value = (index * 3 % 241) as u8;
    }
    for (index, value) in v.iter_mut().enumerate() {
        *value = (index * 5 % 239) as u8;
    }
    let copied = || {
        let mut copy = I420Buffer::new(source.width(), source.height());
        let (from_y, from_u, from_v) = source.data();
        let (y, u, v) = copy.data_mut();
        y.copy_from_slice(from_y);
        u.copy_from_slice(from_u);
        v.copy_from_slice(from_v);
        copy.scale(1128, 634)
    };
    let (with_copy, rounds_with) = median(|| {
        let started = Instant::now();
        for _ in 0..PICTURES {
            std::hint::black_box(copied());
        }
        started.elapsed().as_secs_f64() * 1e3 / f64::from(PICTURES)
    });
    let (without, rounds_without) = median(|| {
        let started = Instant::now();
        for _ in 0..PICTURES {
            std::hint::black_box(VideoPicture::new(&source, false, (1128, 634)).unwrap());
        }
        started.elapsed().as_secs_f64() * 1e3 / f64::from(PICTURES)
    });
    println!(
        "the scale of a picture: median {with_copy:.2} ms with the copy, {without:.2} ms without it; rounds {rounds_with:.2?} and {rounds_without:.2?}"
    );
}
