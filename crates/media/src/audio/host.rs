//! The interface to the sound devices of the machine. The product opens real devices
//! through CPAL, with refreshed PipeWire snapshots on Linux; a test gives a tone and a sink
//! that measures what would be played.

use crate::share::life::Life;
use crate::share::workers::Workers;
use crate::share::{CaptureError, CaptureKind, CaptureNotice, CaptureStream, Pending};
use std::any::Any;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// The identifier of a sound device, as its host gives it. It names the same device after
/// a restart and after the device was disconnected and connected again, where the system
/// has such an identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DeviceId(pub String);

impl fmt::Display for DeviceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub id: DeviceId,
    pub name: String,
    /// The device that the system uses when no device is chosen.
    pub is_default: bool,
}

/// The sound devices at one moment.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Devices {
    pub inputs: Vec<DeviceInfo>,
    pub outputs: Vec<DeviceInfo>,
}

/// The form of the samples that a device gives or takes: interleaved, with this rate and
/// this number of channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Format {
    pub sample_rate: u32,
    pub channels: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AudioError {
    /// The system has no device of the kind, or the chosen device is not connected.
    #[error("no such sound device")]
    NoDevice,
    /// The system refused the access to the device.
    #[error("the system refused the access to the sound device")]
    Refused,
    /// The device has no form of samples that this client can use.
    #[error("the sound device has no usable format: {0}")]
    Unsupported(String),
    #[error("the sound device failed: {0}")]
    Failed(String),
}

/// Takes the samples of an input device. It runs on a thread of the device and returns
/// quickly.
pub type Deliver = Box<dyn FnMut(&[f32], Format) + Send + 'static>;
/// Fills the buffer of an output device. It runs on a thread of the device and returns
/// quickly. The buffer holds silence before the call.
pub type Fill = Box<dyn FnMut(&mut [f32], Format) + Send + 'static>;
/// Told when an open stream has failed, for example because its device was disconnected.
pub type Failed = Box<dyn FnMut(AudioError) + Send + 'static>;

/// A device that delivers or takes samples until this value is dropped.
pub struct OpenStream {
    pub device: DeviceInfo,
    resource: Resource,
    stopped: Option<tokio::sync::oneshot::Receiver<Result<(), CaptureError>>>,
}

struct Resource {
    value: Option<Box<dyn Any + Send>>,
    life: Arc<Life>,
    workers: Workers,
}

impl Drop for Resource {
    fn drop(&mut self) {
        self.life.cancelled.store(true, Ordering::Release);
        self.life.stop();
        let value = self.value.take();
        let _ = self
            .workers
            .spawn("sound-device-close", move || drop(value));
    }
}

impl OpenStream {
    pub fn new(device: DeviceInfo, stream: impl Any + Send) -> Self {
        Self::with_life(device, stream, Arc::new(Life::default()))
    }

    pub(super) fn with_life(device: DeviceInfo, stream: impl Any + Send, life: Arc<Life>) -> Self {
        let (workers, stopped) = Workers::new(life.clone());
        Self {
            device,
            resource: Resource {
                value: Some(Box::new(stream)),
                life,
                workers,
            },
            stopped: Some(stopped),
        }
    }

    /// Fences callbacks synchronously when this stream is cancelled.
    pub fn on_cancel(self, cancel: impl Fn() + Send + Sync + 'static) -> Self {
        self.resource.life.on_stop(cancel);
        self
    }

    pub(crate) fn reporting(
        mut self,
        kind: CaptureKind,
        report: impl Fn(CaptureNotice) + Send + Sync + 'static,
    ) -> Self {
        self.resource
            .workers
            .report_failures_with(move |mut notice| {
                notice.kind = kind;
                report(notice);
            });
        self
    }

    #[cfg(feature = "test-support")]
    pub(crate) fn keep_alive(mut self, guard: impl Any + Send) -> Self {
        let value = self.resource.value.take();
        self.resource.value = Some(Box::new((value, guard)));
        self
    }

    /// Cancels delivery and awaits bounded native cleanup.
    pub fn stop(mut self) -> Pending<Result<(), CaptureError>> {
        let stopped = self.stopped.take();
        let stream = CaptureStream::new(0, 0, self);
        match stopped {
            Some(stopped) => stream.with_stop_result(stopped),
            None => stream,
        }
        .stop()
    }
}

impl fmt::Debug for OpenStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenStream")
            .field("device", &self.device)
            .finish_non_exhaustive()
    }
}

pub trait AudioHost: Send + Sync + 'static {
    fn devices(&self) -> Result<Devices, AudioError>;

    /// Opens an input device: the chosen one, or the default of the system without a
    /// choice. A chosen device that is not connected gives [`AudioError::NoDevice`].
    fn open_input(
        &self,
        device: Option<&DeviceId>,
        deliver: Deliver,
        failed: Failed,
    ) -> Result<OpenStream, AudioError>;

    fn open_output(
        &self,
        device: Option<&DeviceId>,
        fill: Fill,
        failed: Failed,
    ) -> Result<OpenStream, AudioError>;
}

/// The device that a stream is opened on: the chosen device while it is connected, and the
/// default of the system otherwise. The choice of the person stays stored, so the device
/// is used again when it is back.
pub fn usable<'a>(chosen: Option<&'a DeviceId>, connected: &[DeviceInfo]) -> Option<&'a DeviceId> {
    chosen.filter(|id| connected.iter().any(|device| device.id == **id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(id: &str, is_default: bool) -> DeviceInfo {
        DeviceInfo {
            id: DeviceId(id.to_owned()),
            name: id.to_uppercase(),
            is_default,
        }
    }

    #[test]
    fn a_stalled_sound_device_destructor_does_not_hold_its_owner() {
        struct Native(std::sync::mpsc::Receiver<()>);
        impl Drop for Native {
            fn drop(&mut self) {
                let _ = self.0.recv_timeout(std::time::Duration::from_secs(10));
            }
        }
        let (release, held) = std::sync::mpsc::channel();
        let stream = OpenStream::new(device("capture", true), Native(held));
        let (returned, dropped) = std::sync::mpsc::channel();
        let owner = std::thread::spawn(move || {
            drop(stream);
            let _ = returned.send(());
        });
        let result = dropped.recv_timeout(std::time::Duration::from_secs(1));
        let _ = release.send(());
        owner.join().unwrap();
        assert!(result.is_ok(), "the sound device destructor held its owner");
    }

    #[tokio::test]
    async fn a_stalled_device_reports_cleanup_after_synchronous_cancellation() {
        struct Native(std::sync::mpsc::Receiver<()>);
        impl Drop for Native {
            fn drop(&mut self) {
                let _ = self.0.recv_timeout(std::time::Duration::from_secs(10));
            }
        }
        let (release, held) = std::sync::mpsc::channel();
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = cancelled.clone();
        let (reported, failure) = std::sync::mpsc::channel();
        let stream = OpenStream::new(device("capture", true), Native(held))
            .on_cancel(move || flag.store(true, Ordering::Release))
            .reporting(CaptureKind::Microphone, move |notice| {
                let _ = reported.send(notice);
            });
        let stop = stream.stop();
        assert!(cancelled.load(Ordering::Acquire));
        let result = tokio::time::timeout(std::time::Duration::from_secs(4), stop).await;
        let notice = failure.recv_timeout(std::time::Duration::from_secs(1));
        let _ = release.send(());
        assert!(matches!(result.unwrap(), Err(CaptureError::StopFailed(_))));
        assert_eq!(notice.unwrap().kind, CaptureKind::Microphone);
    }

    #[test]
    fn a_chosen_device_is_used_only_while_it_is_connected() {
        let connected = [device("built-in", true), device("desk", false)];
        let (desk, gone) = (DeviceId("desk".to_owned()), DeviceId("headset".to_owned()));
        assert_eq!(usable(Some(&desk), &connected), Some(&desk));
        assert_eq!(usable(Some(&gone), &connected), None);
        assert_eq!(usable(None, &connected), None);
        assert_eq!(usable(Some(&desk), &[]), None);
    }
}
