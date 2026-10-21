//! The class identifiers of the two virtual camera sources of the Windows device check. The
//! source library (`virtual_source.rs`) and the check (`runner.rs`) both read this file.

/// Offers NV12 at 1280 by 720 and YUY2 at 640 by 480, both at 30 pictures per second.
pub const UNCOMPRESSED: u128 = 0x6b2c0f3e_5f0a_4f2e_9a57_3c1d7a1e4b21;
/// Offers YUY2 at 1280 by 720 at 10 pictures per second and MJPEG at 1280 by 720 at 30, as
/// common USB cameras do.
pub const COMPRESSED: u128 = 0x6b2c0f3e_5f0a_4f2e_9a57_3c1d7a1e4b22;
