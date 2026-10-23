//! The software fallback of a hardware video encoder (ADR 0737): an encoder of the backend
//! VideoToolbox that does not start is replaced by the software encoder of its codec, so the
//! track keeps its codec and sends pictures.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use webrtc_sys::encoder_fallback::ffi::encode_with_a_failing_hardware_encoder;

#[test]
fn a_hardware_encoder_that_does_not_start_falls_back_to_the_software_encoder_of_its_codec() {
    for (codec, software) in [("H264", "OpenH264"), ("VP8", "libvpx")] {
        let encoding = encode_with_a_failing_hardware_encoder(codec);
        assert_eq!(encoding.init, 0, "{codec}: {encoding:?}");
        assert_eq!(encoding.encode, 0, "{codec}: {encoding:?}");
        assert!(encoding.images >= 1, "{codec}: {encoding:?}");
        assert_eq!(encoding.codec, codec, "{encoding:?}");
        assert!(
            encoding.implementation.contains(software),
            "{codec}: {encoding:?}"
        );
    }
}
