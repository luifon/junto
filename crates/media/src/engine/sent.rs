//! The pictures that this device gives the call library for its own tracks, and how they
//! are encoded.

use std::sync::LazyLock;
use std::time::Instant;

use livekit::options::{TrackPublishOptions, VideoCodec, VideoEncoderBackend};
use livekit::webrtc::video_frame::{VideoBuffer, VideoFrame, VideoRotation};

static ORIGIN: LazyLock<Instant> = LazyLock::new(Instant::now);

/// A frame for the source of an own track, with its capture time in microseconds of a
/// monotonic clock of the process, never zero. For a frame with the time zero the call
/// library reads the system clock and panics when it is before 1970; it maps the time of
/// each frame to its own clock (ADR 0733).
pub(super) fn frame<T: AsRef<dyn VideoBuffer>>(buffer: T) -> VideoFrame<T> {
    let mut frame = VideoFrame::new(VideoRotation::VideoRotation0, buffer);
    frame.timestamp_us = i64::try_from(ORIGIN.elapsed().as_micros())
        .unwrap_or(i64::MAX)
        .max(1);
    frame
}

/// How the pictures of an own track are encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Encoder {
    /// H.264, by the encoder of the system hardware (VideoToolbox on macOS).
    HardwareH264,
    /// VP8, by the encoder that WebRTC chooses: its software encoder on every system that
    /// this client is built for.
    Vp8,
}

/// Whether this system encodes H.264 with the encoder of its hardware.
const HARDWARE_H264: bool = cfg!(target_os = "macos");

/// The encoder of an own track of `size` pixels at `frame_rate` pictures per second, on a
/// system that has a hardware H.264 encoder or not.
pub(super) fn encoder(hardware_h264: bool, size: (u32, u32), frame_rate: u32) -> Encoder {
    let _ = (size, frame_rate);
    if hardware_h264 {
        Encoder::HardwareH264
    } else {
        Encoder::Vp8
    }
}

/// The options of an own track of `size` pixels at `frame_rate` pictures per second with the
/// codec and the encoder of [`encoder`]. Every system decodes both codecs (ADR 0737).
pub(super) fn encoded(
    options: TrackPublishOptions,
    size: (u32, u32),
    frame_rate: u32,
) -> TrackPublishOptions {
    let (video_codec, video_encoder) = match encoder(HARDWARE_H264, size, frame_rate) {
        Encoder::HardwareH264 => (VideoCodec::H264, VideoEncoderBackend::VideoToolbox),
        Encoder::Vp8 => (VideoCodec::VP8, VideoEncoderBackend::Auto),
    };
    TrackPublishOptions {
        video_codec,
        video_encoder,
        ..options
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use livekit::webrtc::video_frame::I420Buffer;

    #[test]
    fn a_sent_frame_has_a_capture_time_that_is_not_zero_and_does_not_go_back() {
        let first = frame(I420Buffer::new(2, 2));
        let second = frame(I420Buffer::new(2, 2));
        assert!(first.timestamp_us > 0);
        assert!(second.timestamp_us >= first.timestamp_us);
    }

    /// A picture that one system encodes is decoded on every system.
    #[test]
    fn every_system_decodes_the_codecs_that_the_systems_encode() {
        use livekit::webrtc::peer_connection_factory::PeerConnectionFactory;
        use livekit::webrtc::prelude::MediaType;
        let factory = PeerConnectionFactory::default();
        let mime = |codecs: Vec<livekit::webrtc::rtp_parameters::RtpCodecCapability>| {
            codecs
                .into_iter()
                .map(|codec| codec.mime_type.to_lowercase())
                .collect::<Vec<_>>()
        };
        let decoded = mime(
            factory
                .get_rtp_receiver_capabilities(MediaType::Video)
                .codecs,
        );
        for codec in [VideoCodec::H264, VideoCodec::VP8] {
            let name = format!("video/{}", codec.as_str());
            assert!(
                decoded.contains(&name),
                "{name} is not decoded: {decoded:?}"
            );
        }
        let own = encoded(TrackPublishOptions::default(), (1920, 1080), 30).video_codec;
        let encoded = mime(factory.get_rtp_sender_capabilities(MediaType::Video).codecs);
        let name = format!("video/{}", own.as_str());
        assert!(
            encoded.contains(&name),
            "{name} is not encoded: {encoded:?}"
        );
    }

    /// The hardware encoder of H.264 encodes a picture within the limits of level 5.2, the
    /// highest level of the encoder of macOS: 36,864 blocks of 16 × 16 pixels, 2,073,600
    /// blocks per second, and 543 blocks on each side. A larger share is encoded in VP8 at
    /// its full size (ADR 0773).
    #[test]
    fn a_picture_beyond_the_levels_of_h264_is_encoded_in_vp8_at_its_size() {
        use Encoder::{HardwareH264, Vp8};
        let on_macos = |size, frame_rate| encoder(true, size, frame_rate);
        assert_eq!(on_macos((1280, 720), 30), HardwareH264);
        assert_eq!(on_macos((1920, 1080), 60), HardwareH264);
        assert_eq!(on_macos((3840, 2160), 60), HardwareH264);
        assert_eq!(on_macos((4096, 2304), 56), HardwareH264);
        assert_eq!(on_macos((4096, 2304), 60), Vp8);
        assert_eq!(on_macos((4096, 2320), 15), Vp8);
        assert_eq!(on_macos((5120, 2880), 15), Vp8);
        assert_eq!(on_macos((6016, 3384), 15), Vp8);
        assert_eq!(on_macos((8688, 400), 15), HardwareH264);
        assert_eq!(on_macos((8689, 400), 15), Vp8);
        assert_eq!(on_macos((400, 8689), 15), Vp8);
        for (size, frame_rate) in [((1280, 720), 30), ((5120, 2880), 15)] {
            assert_eq!(encoder(false, size, frame_rate), Vp8);
        }
        let options = encoded(TrackPublishOptions::default(), (5120, 2880), 15);
        assert_eq!(
            (options.video_codec, options.video_encoder),
            (VideoCodec::VP8, VideoEncoderBackend::Auto)
        );
    }
}
