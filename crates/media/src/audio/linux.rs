//! Initialize the PipeWire symbols that the call library and the sound host share.

use std::sync::OnceLock;

unsafe extern "C" {
    fn junto_initialize_pipewire() -> bool;
}

pub(super) fn host() -> cpal::Host {
    static READY: OnceLock<bool> = OnceLock::new();
    // WebRTC's linked stubs take precedence over libpipewire's symbols. They must be
    // initialized before CPAL calls pw_init, even when no screen capture is started.
    // The bridge only loads the library and its symbols; it opens no device or portal.
    if *READY.get_or_init(|| unsafe { junto_initialize_pipewire() }) {
        match cpal::host_from_id(cpal::HostId::PipeWire) {
            Ok(host) => return host,
            Err(error) => tracing::warn!(%error, "PipeWire is not available"),
        }
    } else {
        tracing::warn!("the call library could not load PipeWire's symbols");
    }
    cpal::default_host()
}

use super::cpal_host::CpalHost;
use super::host::{AudioError, AudioHost, Deliver, DeviceId, Devices, Failed, Fill, OpenStream};

pub(super) struct PipeWireHost;

// CPAL's PipeWire host holds one registry snapshot. Each operation needs a current
// snapshot; an opened stream owns its connection independently of that snapshot.
impl AudioHost for PipeWireHost {
    fn devices(&self) -> Result<Devices, AudioError> {
        CpalHost::new().devices()
    }

    fn open_input(
        &self,
        device: Option<&DeviceId>,
        deliver: Deliver,
        failed: Failed,
    ) -> Result<OpenStream, AudioError> {
        CpalHost::new().open_input(device, deliver, failed)
    }

    fn open_output(
        &self,
        device: Option<&DeviceId>,
        fill: Fill,
        failed: Failed,
    ) -> Result<OpenStream, AudioError> {
        CpalHost::new().open_output(device, fill, failed)
    }
}
