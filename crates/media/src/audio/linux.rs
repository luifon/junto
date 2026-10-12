//! Initialise the PipeWire symbols that the call library and the sound host share.

#[allow(unsafe_code)]
mod native;

pub(crate) fn initialise() -> bool {
    native::initialise()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Backend {
    PipeWire,
}

fn choose<T>(
    ready: bool,
    mut open: impl FnMut(Backend) -> Result<T, AudioError>,
) -> Result<T, AudioError> {
    if !ready {
        return Err(AudioError::Failed("PipeWire is not available".to_owned()));
    }
    open(Backend::PipeWire)
}

pub(super) fn host() -> Result<cpal::Host, AudioError> {
    let ready = initialise();
    choose(ready, |backend| match backend {
        Backend::PipeWire => cpal::host_from_id(cpal::HostId::PipeWire)
            .map_err(|error| AudioError::Failed(error.to_string())),
    })
}

use super::cpal_host::CpalHost;
use super::host::{AudioError, AudioHost, Deliver, DeviceId, Devices, Failed, Fill, OpenStream};

#[derive(Default)]
pub(super) struct PipeWireHost(std::sync::Mutex<Option<CpalHost>>);

impl PipeWireHost {
    fn snapshot(&self) -> std::sync::MutexGuard<'_, Option<CpalHost>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

// Refresh when listing devices; opening the chosen devices uses that same snapshot.
// An opened stream owns its connection independently of the snapshot.
impl AudioHost for PipeWireHost {
    fn devices(&self) -> Result<Devices, AudioError> {
        let mut held = self.snapshot();
        let current = CpalHost::new();
        let devices = current.devices();
        *held = Some(current);
        devices
    }

    fn open_input(
        &self,
        device: Option<&DeviceId>,
        deliver: Deliver,
        failed: Failed,
    ) -> Result<OpenStream, AudioError> {
        self.snapshot()
            .get_or_insert_with(CpalHost::new)
            .open_input(device, deliver, failed)
    }

    fn open_output(
        &self,
        device: Option<&DeviceId>,
        fill: Fill,
        failed: Failed,
    ) -> Result<OpenStream, AudioError> {
        self.snapshot()
            .get_or_insert_with(CpalHost::new)
            .open_output(device, fill, failed)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn missing_native_symbols_never_enter_an_automatic_host() {
        let mut entered = Vec::new();
        let result = choose(false, |backend| {
            entered.push(backend);
            Ok(())
        });
        assert!(result.is_err());
        assert!(entered.is_empty());
    }

    #[test]
    fn a_failed_pipewire_connection_is_not_attempted_again_by_the_fallback() {
        let mut entered = Vec::new();
        let result = choose(true, |backend| {
            entered.push(backend);
            match backend {
                Backend::PipeWire => Err::<(), _>(AudioError::NoDevice),
            }
        });
        assert!(result.is_err());
        assert_eq!(entered, [Backend::PipeWire]);
    }
    #[test]
    #[ignore = "needs isolated virtual PipeWire devices: tools/linux-container.sh"]
    fn listing_and_opening_use_one_native_snapshot() {
        use super::super::cpal_host::HOSTS_CREATED;
        assert_eq!(
            std::env::var("JUNTO_TEST_VIRTUAL_AUDIO").as_deref(),
            Ok("1")
        );
        let before = HOSTS_CREATED.get();
        let host = PipeWireHost::default();
        host.devices().unwrap();
        let missing = DeviceId("missing-device".to_owned());
        assert_eq!(
            host.open_input(Some(&missing), Box::new(|_, _| {}), Box::new(|_| {}))
                .unwrap_err(),
            AudioError::NoDevice
        );
        assert_eq!(
            host.open_output(Some(&missing), Box::new(|_, _| {}), Box::new(|_| {}))
                .unwrap_err(),
            AudioError::NoDevice
        );
        assert_eq!(HOSTS_CREATED.get() - before, 1);
    }
}
