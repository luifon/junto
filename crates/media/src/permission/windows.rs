//! The microphone setting of the privacy settings of Windows, read through the access
//! information of the class of sound capture devices.

use ::windows::Devices::Enumeration::{DeviceAccessInformation, DeviceAccessStatus, DeviceClass};

use super::MicrophoneAccess;

pub(super) fn state() -> MicrophoneAccess {
    match DeviceAccessInformation::CreateFromDeviceClass(DeviceClass::AudioCapture)
        .and_then(|info| info.CurrentStatus())
    {
        Ok(DeviceAccessStatus::DeniedByUser | DeviceAccessStatus::DeniedBySystem) => {
            MicrophoneAccess::Refused
        }
        _ => MicrophoneAccess::Allowed,
    }
}
