//! The profiles of the two virtual camera sources of the Windows device check. Each check
//! registers new class identifiers and stores the profile of each as the value `PROFILE` of
//! its class key; the source library (`virtual_source.rs`) reads it. The check (`runner.rs`)
//! and the library both read this file.

pub const PROFILE: &str = "JuntoTestProfile";
/// Offers NV12 at 1280 by 720 and YUY2 at 640 by 480, both at 30 pictures per second.
pub const UNCOMPRESSED: &str = "uncompressed";
/// Offers YUY2 at 1280 by 720 at 10 pictures per second and MJPEG at 1280 by 720 at 30, as
/// common USB cameras do.
pub const COMPRESSED: &str = "compressed";
