use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use junto_media::audio::host::{Deliver, Failed, Fill};
use junto_media::audio::{AudioError, AudioHost, DeviceId, Devices, OpenStream};

pub struct Guard {
    host: Arc<dyn AudioHost>,
    id: DeviceId,
    verify: Box<dyn Fn() -> Result<DeviceId, AudioError> + Send + Sync>,
    failed: Arc<AtomicBool>,
    active: Arc<AtomicUsize>,
}

impl Guard {
    pub fn new(
        host: Arc<dyn AudioHost>,
        verify: impl Fn() -> Result<DeviceId, AudioError> + Send + Sync + 'static,
    ) -> Result<Self, AudioError> {
        let id = verify()?;
        Ok(Self {
            host,
            id,
            verify: Box::new(verify),
            failed: Arc::new(AtomicBool::new(false)),
            active: Arc::new(AtomicUsize::new(0)),
        })
    }

    pub fn id(&self) -> &DeviceId {
        &self.id
    }

    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::SeqCst)
    }

    pub fn active(&self) -> usize {
        self.active.load(Ordering::SeqCst)
    }

    fn check(&self, chosen: Option<&DeviceId>) -> Result<(), AudioError> {
        if self.failed() || chosen != Some(&self.id) || (self.verify)().as_ref() != Ok(&self.id) {
            self.failed.store(true, Ordering::SeqCst);
            return Err(AudioError::Refused);
        }
        Ok(())
    }

    fn failure(&self, mut notify: Failed) -> Failed {
        let failed = self.failed.clone();
        Box::new(move |error| {
            failed.store(true, Ordering::SeqCst);
            notify(error);
        })
    }

    fn opened(
        &self,
        result: Result<OpenStream, AudioError>,
        armed: Arc<AtomicBool>,
    ) -> Result<OpenStream, AudioError> {
        let stream = match result {
            Ok(stream) if stream.device.id == self.id && !self.failed() => stream,
            other => {
                self.failed.store(true, Ordering::SeqCst);
                return Err(other.err().unwrap_or(AudioError::Refused));
            }
        };
        self.check(Some(&stream.device.id))?;
        self.active.fetch_add(1, Ordering::SeqCst);
        armed.store(true, Ordering::SeqCst);
        Ok(OpenStream::new(
            stream.device.clone(),
            Running {
                stream: Some(stream),
                armed,
                active: self.active.clone(),
            },
        ))
    }
}

struct Running {
    stream: Option<OpenStream>,
    armed: Arc<AtomicBool>,
    active: Arc<AtomicUsize>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.armed.store(false, Ordering::SeqCst);
        self.stream.take();
        self.active.fetch_sub(1, Ordering::SeqCst);
    }
}

impl AudioHost for Guard {
    fn devices(&self) -> Result<Devices, AudioError> {
        self.host.devices()
    }
    fn open_input(
        &self,
        id: Option<&DeviceId>,
        mut data: Deliver,
        error: Failed,
    ) -> Result<OpenStream, AudioError> {
        self.check(id)?;
        let armed = Arc::new(AtomicBool::new(false));
        let ready = armed.clone();
        let failed = self.failed.clone();
        self.opened(
            self.host.open_input(
                id,
                Box::new(move |samples, format| {
                    if ready.load(Ordering::SeqCst) && !failed.load(Ordering::SeqCst) {
                        data(samples, format);
                    }
                }),
                self.failure(error),
            ),
            armed,
        )
    }
    fn open_output(
        &self,
        id: Option<&DeviceId>,
        mut data: Fill,
        error: Failed,
    ) -> Result<OpenStream, AudioError> {
        self.check(id)?;
        let armed = Arc::new(AtomicBool::new(false));
        let ready = armed.clone();
        let failed = self.failed.clone();
        self.opened(
            self.host.open_output(
                id,
                Box::new(move |samples, format| {
                    samples.fill(0.0);
                    if ready.load(Ordering::SeqCst) && !failed.load(Ordering::SeqCst) {
                        data(samples, format);
                    }
                    if failed.load(Ordering::SeqCst) {
                        samples.fill(0.0);
                    }
                }),
                self.failure(error),
            ),
            armed,
        )
    }
}
