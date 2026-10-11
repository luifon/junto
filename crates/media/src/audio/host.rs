//! The interface to the sound devices of the machine. The product opens real devices
//! through [`super::cpal_host::CpalHost`]; a test gives a tone as the microphone and a sink
//! that measures what would be played.

use std::any::Any;
use std::fmt;

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
    _stream: Box<dyn Any + Send>,
}

impl OpenStream {
    pub fn new(device: DeviceInfo, stream: impl Any + Send) -> Self {
        Self {
            device,
            _stream: Box::new(stream),
        }
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
    fn a_chosen_device_is_used_only_while_it_is_connected() {
        let connected = [device("built-in", true), device("desk", false)];
        let (desk, gone) = (DeviceId("desk".to_owned()), DeviceId("headset".to_owned()));
        assert_eq!(usable(Some(&desk), &connected), Some(&desk));
        assert_eq!(usable(Some(&gone), &connected), None);
        assert_eq!(usable(None, &connected), None);
        assert_eq!(usable(Some(&desk), &[]), None);
    }
}
