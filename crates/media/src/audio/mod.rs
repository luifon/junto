//! Sound in the client: devices, the way from the microphone to the media connection, and
//! the way from the other people to the output device (ADR 0733).
//!
//! Every stage works at 48 kHz in frames of 10 ms, which is what the audio processing and
//! the audio sources of the call library take. A device has its own rate and channel count;
//! the two ends convert, in steps that are whole samples on both sides ([`rate::Span`]).

pub mod capture;
pub mod cpal_host;
pub mod host;
pub mod level;
#[cfg(target_os = "linux")]
pub(crate) mod linux;
pub mod playout;
pub mod processing;
pub mod rate;
pub mod recent;
#[cfg(feature = "test-support")]
pub mod synthetic;

pub use host::{AudioError, AudioHost, DeviceId, DeviceInfo, Devices, Format, OpenStream};
pub use processing::Processing;

pub(crate) fn system() -> std::sync::Arc<dyn AudioHost> {
    #[cfg(target_os = "linux")]
    {
        std::sync::Arc::new(linux::PipeWireHost::default())
    }
    #[cfg(not(target_os = "linux"))]
    {
        std::sync::Arc::new(cpal_host::CpalHost::new())
    }
}

/// The sample rate of the pipeline.
pub const SAMPLE_RATE: u32 = 48_000;
/// The samples of one channel in one frame of 10 ms.
pub const FRAME_SAMPLES: usize = 480;
/// The channels of the mix that is played. The voice of a person is mono and is played on
/// both.
pub const PLAYOUT_CHANNELS: usize = 2;

pub(crate) fn to_i16(sample: f32) -> i16 {
    // The cast saturates and maps a value that is no number to zero.
    (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16
}

pub(crate) fn to_f32(sample: i16) -> f32 {
    f32::from(sample) / f32::from(i16::MAX)
}
