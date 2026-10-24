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

/// The limits of level 3.1 of H.264, the level that the media server answers and at which
/// WebRTC sends (ADR 0737): the blocks of 16 × 16 pixels of a picture, the blocks per second,
/// and the blocks of the longest side (the square root of 8 times the blocks of a picture).
const H264_BLOCKS: u64 = 3_600;
const H264_BLOCKS_PER_SECOND: u64 = 108_000;
const H264_SIDE_BLOCKS: u64 = 169;

/// The encoder of an own track of `size` pixels at `frame_rate` pictures per second, on a
/// system that has a hardware H.264 encoder or not. A picture beyond the negotiated level of
/// H.264 is encoded in VP8 at its size, not made smaller (ADR 0773).
pub(super) fn encoder(hardware_h264: bool, size: (u32, u32), frame_rate: u32) -> Encoder {
    let blocks = |pixels: u32| u64::from(pixels).div_ceil(16);
    let (wide, high) = (blocks(size.0), blocks(size.1));
    let within = wide.max(high) <= H264_SIDE_BLOCKS
        && wide * high <= H264_BLOCKS
        && wide * high * u64::from(frame_rate) <= H264_BLOCKS_PER_SECOND;
    if hardware_h264 && within {
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

    /// The frames of the own camera and of the own share are made by [`frame`], so none has
    /// the capture time zero: the engine outside its test modules makes no frame of its own.
    #[test]
    fn every_frame_of_an_own_track_is_made_with_a_capture_time() {
        for (file, source) in [
            ("camera.rs", include_str!("camera.rs")),
            ("share.rs", include_str!("share.rs")),
            ("engine.rs", include_str!("../engine.rs")),
        ] {
            let product = source
                .split("#[cfg(test)]\nmod tests")
                .next()
                .unwrap_or(source);
            assert!(
                !product.contains("VideoFrame::new") && !product.contains("VideoFrame {"),
                "{file} makes a frame without engine::sent::frame"
            );
        }
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
        // A picture within the negotiated level: H.264 on macOS, VP8 elsewhere.
        let own = encoded(TrackPublishOptions::default(), (1280, 720), 30).video_codec;
        let encoded = mime(factory.get_rtp_sender_capabilities(MediaType::Video).codecs);
        let name = format!("video/{}", own.as_str());
        assert!(
            encoded.contains(&name),
            "{name} is not encoded: {encoded:?}"
        );
    }

    /// The hardware encoder of H.264 encodes a picture within the limits of level 3.1, the
    /// level that the media server answers: 3,600 blocks of 16 × 16 pixels, 108,000 blocks
    /// per second, and 169 blocks on each side. A larger picture is encoded in VP8 at its full
    /// size (ADR 0737, ADR 0773).
    #[test]
    fn a_picture_beyond_the_negotiated_level_of_h264_is_encoded_in_vp8_at_its_size() {
        use Encoder::{HardwareH264, Vp8};
        let on_macos = |size, frame_rate| encoder(true, size, frame_rate);
        assert_eq!(on_macos((640, 360), 30), HardwareH264);
        assert_eq!(on_macos((1280, 720), 30), HardwareH264);
        assert_eq!(on_macos((1280, 720), 31), Vp8);
        assert_eq!(on_macos((1280, 720), 60), Vp8);
        assert_eq!(on_macos((960, 540), 30), HardwareH264);
        assert_eq!(on_macos((960, 540), 60), Vp8);
        assert_eq!(on_macos((1280, 736), 15), Vp8);
        assert_eq!(on_macos((1920, 1080), 15), Vp8);
        assert_eq!(on_macos((5120, 2880), 15), Vp8);
        assert_eq!(on_macos((2704, 160), 15), HardwareH264);
        assert_eq!(on_macos((2705, 160), 15), Vp8);
        assert_eq!(on_macos((160, 2705), 15), Vp8);
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
