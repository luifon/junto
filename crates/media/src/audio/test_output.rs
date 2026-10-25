//! An output for the tests of the players of the window: a synthetic host whose output
//! opens only when the test lets it, as a sound system that waits for another stream does,
//! and that tells when it closes.

use super::host::{Deliver, Failed, Fill};
use super::synthetic::SyntheticHost;
use super::{AudioError, AudioHost, DeviceId, Devices, OpenStream};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Mutex, PoisonError};

/// Reports when the stream that holds it is dropped.
struct Closed(Sender<()>);

impl Drop for Closed {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

pub(crate) struct TestOutput {
    pub(crate) host: SyntheticHost,
    pub(crate) release: Option<Mutex<Receiver<()>>>,
    pub(crate) closed: Sender<()>,
}

impl AudioHost for TestOutput {
    fn devices(&self) -> Result<Devices, AudioError> {
        self.host.devices()
    }
    fn open_input(
        &self,
        device: Option<&DeviceId>,
        deliver: Deliver,
        failed: Failed,
    ) -> Result<OpenStream, AudioError> {
        self.host.open_input(device, deliver, failed)
    }
    fn open_output(
        &self,
        device: Option<&DeviceId>,
        fill: Fill,
        failed: Failed,
    ) -> Result<OpenStream, AudioError> {
        if let Some(release) = &self.release {
            let _ = release
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .recv();
        }
        let stream = self.host.open_output(device, fill, failed)?;
        let device = stream.device.clone();
        Ok(OpenStream::new(
            device,
            (stream, Closed(self.closed.clone())),
        ))
    }
}
