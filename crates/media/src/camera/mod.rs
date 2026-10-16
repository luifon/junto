//! Camera capture with explicit permission and device selection.

use std::sync::Arc;

use livekit::webrtc::video_frame::VideoBuffer;

use crate::share::{CaptureStream, Pending};

#[cfg(any(
    test,
    all(
        target_os = "macos",
        any(not(feature = "test-support"), feature = "native-device-tests")
    )
))]
mod format;

#[cfg(any(
    test,
    all(
        target_os = "macos",
        any(not(feature = "test-support"), feature = "native-device-tests")
    )
))]
mod matrix;

#[cfg(all(
    target_os = "macos",
    any(test, not(feature = "test-support"), feature = "native-device-tests")
))]
#[allow(unsafe_code)]
mod color;

#[cfg(all(
    target_os = "macos",
    any(test, not(feature = "test-support"), feature = "native-device-tests")
))]
#[allow(unsafe_code)]
mod settings;

#[cfg(all(
    target_os = "macos",
    any(
        not(any(test, feature = "test-support")),
        feature = "native-device-tests"
    )
))]
#[allow(unsafe_code)]
mod macos;
#[cfg(any(test, feature = "test-support"))]
pub mod synthetic;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraAccess {
    NotAsked,
    Allowed,
    Refused,
    Restricted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraDevice {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CameraError {
    #[error("camera permission has not been granted")]
    NoPermission(CameraAccess),
    #[error("the selected camera is no longer available")]
    DeviceGone,
    #[error("camera capture is not available on this system")]
    Unsupported,
    #[error("the camera failed: {0}")]
    Failed(String),
    #[error("the media connection of the call ended")]
    Interrupted,
    #[error("the server does not allow camera publication")]
    Blocked,
    #[error("the call is not connected yet")]
    NotConnected,
}

pub trait CameraSink: Send + Sync + 'static {
    fn picture(&self, buffer: Arc<dyn VideoBuffer>);
    fn ended(&self, error: CameraError);
}

pub trait CameraCapture: Send + Sync + 'static {
    fn access(&self) -> CameraAccess;
    fn request_access(&self) -> Pending<CameraAccess>;
    fn devices(&self) -> Result<Vec<CameraDevice>, CameraError>;
    /// An absent choice uses the first camera; a missing explicit choice is refused.
    /// This method never asks for a system permission.
    fn start(
        &self,
        device: Option<String>,
        sink: Arc<dyn CameraSink>,
    ) -> Pending<Result<CaptureStream, CameraError>>;
}

pub fn choose(devices: &[CameraDevice], chosen: Option<&str>) -> Result<CameraDevice, CameraError> {
    match chosen {
        Some(id) => devices.iter().find(|device| device.id == id),
        None => devices.first(),
    }
    .cloned()
    .ok_or(CameraError::DeviceGone)
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum CameraState {
    #[default]
    Off,
    Starting,
    Running,
    Failed(CameraError),
}

pub struct NoCamera;
impl CameraCapture for NoCamera {
    fn access(&self) -> CameraAccess {
        CameraAccess::Restricted
    }
    fn request_access(&self) -> Pending<CameraAccess> {
        Box::pin(async { CameraAccess::Restricted })
    }
    fn devices(&self) -> Result<Vec<CameraDevice>, CameraError> {
        Err(CameraError::Unsupported)
    }
    fn start(
        &self,
        _: Option<String>,
        _: Arc<dyn CameraSink>,
    ) -> Pending<Result<CaptureStream, CameraError>> {
        Box::pin(async { Err(CameraError::Unsupported) })
    }
}

#[cfg(any(test, feature = "test-support"))]
pub fn system() -> Arc<NoCamera> {
    Arc::new(NoCamera)
}

/// Explicit native factory for the opt-in device executable. Creation requests no access.
#[cfg(all(target_os = "macos", feature = "native-device-tests"))]
pub fn native_for_test() -> Arc<dyn CameraCapture> {
    Arc::new(macos::SystemCamera)
}

/// For the native stage: whether a camera buffer delivered without conversion is in the 601
/// matrix of the publication path. `None` for a buffer that the camera code converted.
#[cfg(all(target_os = "macos", feature = "native-device-tests"))]
pub fn native_is_601_for_test(buffer: &dyn VideoBuffer) -> Option<bool> {
    color::native_is_601(buffer)
}

#[cfg(not(any(test, feature = "test-support")))]
pub fn system() -> Arc<dyn CameraCapture> {
    #[cfg(all(target_os = "macos", not(any(test, feature = "test-support"))))]
    {
        Arc::new(macos::SystemCamera)
    }
    #[cfg(any(not(target_os = "macos"), test))]
    {
        Arc::new(NoCamera)
    }
}

#[cfg(test)]
mod tests;

/// Mirrors a preview; transmitted frames keep the camera's orientation.
pub fn mirror(frame: &crate::share::picture::PictureFrame) -> crate::share::picture::PictureFrame {
    let mut pixels = frame.bgra.to_vec();
    let row_bytes = frame.width as usize * 4;
    if row_bytes > 0 {
        for row in pixels.chunks_exact_mut(row_bytes) {
            for x in 0..frame.width as usize / 2 {
                let other = frame.width as usize - 1 - x;
                for channel in 0..4 {
                    row.swap(x * 4 + channel, other * 4 + channel);
                }
            }
        }
    }
    crate::share::picture::PictureFrame {
        bgra: pixels.into(),
        ..frame.clone()
    }
}

mod frame;
pub use frame::CameraFrame;
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod surface;
