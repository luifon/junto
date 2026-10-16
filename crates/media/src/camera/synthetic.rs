//! Generated camera pictures; no hardware is used.

use super::*;
use crate::share::synthetic::{Colour, two_colours};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError, Weak};
use std::thread::JoinHandle;
use std::time::Duration;

struct State {
    access: CameraAccess,
    devices: Vec<CameraDevice>,
    colours: (Colour, Colour),
    delay: Duration,
    starts: usize,
    device_reads: usize,
    active: Vec<Weak<AtomicBool>>,
}

#[derive(Clone)]
pub struct SyntheticCamera {
    state: Arc<Mutex<State>>,
}
impl Default for SyntheticCamera {
    fn default() -> Self {
        Self::new()
    }
}
impl SyntheticCamera {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                access: CameraAccess::Allowed,
                devices: vec![CameraDevice {
                    id: "generated-camera".into(),
                    name: "Generated camera".into(),
                }],
                colours: ((200, 30, 30), (30, 30, 200)),
                delay: Duration::ZERO,
                starts: 0,
                device_reads: 0,
                active: Vec::new(),
            })),
        }
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
    pub fn set_access(&self, access: CameraAccess) {
        self.lock().access = access;
    }
    pub fn set_devices(&self, devices: Vec<CameraDevice>) {
        self.lock().devices = devices;
    }
    pub fn set_colours(&self, left: Colour, right: Colour) {
        self.lock().colours = (left, right);
    }
    pub fn set_start_delay(&self, delay: Duration) {
        self.lock().delay = delay;
    }
    pub fn device_reads(&self) -> usize {
        self.lock().device_reads
    }
    pub fn started(&self) -> usize {
        self.lock().starts
    }
    pub fn active(&self) -> usize {
        let mut state = self.lock();
        state.active.retain(|active| active.upgrade().is_some());
        state
            .active
            .iter()
            .filter_map(Weak::upgrade)
            .filter(|active| active.load(Ordering::Acquire))
            .count()
    }
}
impl CameraCapture for SyntheticCamera {
    fn access(&self) -> CameraAccess {
        self.lock().access
    }
    fn request_access(&self) -> Pending<CameraAccess> {
        let this = self.clone();
        Box::pin(async move {
            crate::executor::require_runtime("the permission request of the synthetic camera");
            let mut state = this.lock();
            if state.access == CameraAccess::NotAsked {
                state.access = CameraAccess::Allowed;
            }
            state.access
        })
    }
    fn devices(&self) -> Result<Vec<CameraDevice>, CameraError> {
        let mut state = self.lock();
        state.device_reads += 1;
        Ok(state.devices.clone())
    }
    fn start(
        &self,
        device: Option<String>,
        sink: Arc<dyn CameraSink>,
    ) -> Pending<Result<CaptureStream, CameraError>> {
        let this = self.clone();
        Box::pin(async move {
            crate::executor::require_runtime("the start of the synthetic camera");
            let delay = this.lock().delay;
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            let selected = {
                let state = this.lock();
                if state.access != CameraAccess::Allowed {
                    return Err(CameraError::NoPermission(state.access));
                }
                choose(&state.devices, device.as_deref())?
            };
            this.lock().starts += 1;
            let active = Arc::new(AtomicBool::new(true));
            this.lock().active.push(Arc::downgrade(&active));
            let running = active.clone();
            let thread = std::thread::Builder::new()
                .name("generated-camera".into())
                .spawn(move || {
                    while running.load(Ordering::Acquire) {
                        let (error, colours) = {
                            let state = this.lock();
                            let error = if state.access != CameraAccess::Allowed {
                                Some(CameraError::NoPermission(state.access))
                            } else if !state.devices.iter().any(|device| device.id == selected.id) {
                                Some(CameraError::DeviceGone)
                            } else {
                                None
                            };
                            (error, state.colours)
                        };
                        if let Some(error) = error {
                            running.store(false, Ordering::Release);
                            sink.ended(error);
                            break;
                        }
                        sink.picture(Arc::new(two_colours(640, 360, colours.0, colours.1)));
                        std::thread::sleep(Duration::from_millis(33));
                    }
                })
                .map_err(|error| CameraError::Failed(error.to_string()))?;
            Ok(CaptureStream::new(
                640,
                360,
                Running {
                    active,
                    thread: Some(thread),
                },
            ))
        })
    }
}

struct Running {
    active: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}
impl Drop for Running {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
