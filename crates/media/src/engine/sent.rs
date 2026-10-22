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

/// The codec and the encoder of the own camera and the own share. On macOS the encoder of the
/// system hardware (VideoToolbox) encodes H.264; the other systems encode VP8 in software.
/// Every system decodes both (ADR 0737).
pub(super) fn encoded(options: TrackPublishOptions) -> TrackPublishOptions {
    if cfg!(target_os = "macos") {
        TrackPublishOptions {
            video_codec: VideoCodec::H264,
            video_encoder: VideoEncoderBackend::VideoToolbox,
            ..options
        }
    } else {
        TrackPublishOptions {
            video_codec: VideoCodec::VP8,
            video_encoder: VideoEncoderBackend::Auto,
            ..options
        }
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
        let own = encoded(TrackPublishOptions::default()).video_codec;
        let encoded = mime(factory.get_rtp_sender_capabilities(MediaType::Video).codecs);
        let name = format!("video/{}", own.as_str());
        assert!(
            encoded.contains(&name),
            "{name} is not encoded: {encoded:?}"
        );
    }
}
