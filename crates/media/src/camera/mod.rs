//! Camera capture with explicit permission and device selection.

use std::sync::Arc;

use livekit::webrtc::video_frame::VideoBuffer;

use crate::share::{CaptureStream, Pending};

#[cfg(any(
    test,
    all(
        any(target_os = "windows", target_os = "linux"),
        any(not(feature = "test-support"), feature = "native-device-tests")
    )
))]
mod raw;

#[cfg(any(
    test,
    all(
        any(target_os = "windows", target_os = "linux"),
        any(not(feature = "test-support"), feature = "native-device-tests")
    )
))]
mod worker;

#[cfg(any(
    test,
    all(
        target_os = "linux",
        any(not(feature = "test-support"), feature = "native-device-tests")
    )
))]
mod v4l2;

#[cfg(all(
    target_os = "linux",
    any(test, not(feature = "test-support"), feature = "native-device-tests")
))]
#[allow(unsafe_code)]
mod linux;

#[cfg(all(
    target_os = "windows",
    any(test, not(feature = "test-support"), feature = "native-device-tests")
))]
#[allow(unsafe_code)]
mod windows;

#[cfg(any(
    test,
    all(
        any(target_os = "macos", target_os = "windows", target_os = "linux"),
        any(not(feature = "test-support"), feature = "native-device-tests")
    )
))]
mod format;

#[cfg(any(
    test,
    all(
        any(target_os = "macos", target_os = "windows", target_os = "linux"),
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

/// The camera of one system. The crate calls the functions that return a future, and polls
/// those futures, only in tasks of the runtime of the client (ADR 0733).
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

/// Explicit opt-in factory; ordinary test-support builds never select a hardware camera.
#[cfg(all(
    any(target_os = "windows", target_os = "linux"),
    any(test, feature = "native-device-tests")
))]
pub fn native_for_test() -> Arc<dyn CameraCapture> {
    #[cfg(target_os = "windows")]
    {
        Arc::new(windows::SystemCamera)
    }
    #[cfg(target_os = "linux")]
    {
        Arc::new(linux::SystemCamera)
    }
}

/// The frame rate that the last camera start asked the driver for, as a fraction of frames
/// per second; `None` when the driver states no rate.
#[cfg(all(
    any(target_os = "windows", target_os = "linux"),
    feature = "native-device-tests"
))]
pub fn native_rate_for_test() -> Option<(u32, u32)> {
    *worker::LAST_RATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The counts of pictures of the native captures of the camera `device` in the process so far.
#[cfg(all(
    any(target_os = "windows", target_os = "linux"),
    feature = "native-device-tests"
))]
pub fn native_counts_for_test(device: &str) -> NativeCounts {
    worker::COUNTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .find(|(id, _)| id == device)
        .map_or_else(NativeCounts::default, |(_, counts)| *counts)
}

#[cfg(all(
    any(target_os = "windows", target_os = "linux"),
    feature = "native-device-tests"
))]
pub use worker::Counts as NativeCounts;

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
    #[cfg(target_os = "windows")]
    {
        Arc::new(windows::SystemCamera)
    }
    #[cfg(target_os = "linux")]
    {
        Arc::new(linux::SystemCamera)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        Arc::new(NoCamera)
    }
}

#[cfg(test)]
mod tests;
