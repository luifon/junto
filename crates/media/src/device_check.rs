use std::sync::{
    Arc, Mutex, MutexGuard, PoisonError, Weak,
    atomic::{AtomicBool, Ordering},
};

use livekit::webrtc::video_frame::VideoBuffer;
use tokio::sync::watch;

use crate::MediaOptions;
use crate::audio::{AudioError, check::AudioCheck};
use crate::camera::{CameraAccess, CameraError, CameraSink, CameraState};
use crate::engine::{Levels, VoiceConfig};
use crate::permission::MicrophoneAccess;
use crate::share::{CaptureStream, picture::VideoPicture};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioCheckKind {
    Microphone,
    Output,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum AudioCheckState {
    #[default]
    Off,
    Starting(AudioCheckKind),
    Running(AudioCheckKind),
    Failed(AudioError),
}

impl AudioCheckState {
    pub fn kind(&self) -> Option<AudioCheckKind> {
        match self {
            Self::Starting(kind) | Self::Running(kind) => Some(*kind),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DeviceCheckView {
    pub audio: AudioCheckState,
    pub camera: CameraState,
}

#[derive(Default)]
struct State {
    audio: u64,
    camera: u64,
    audio_stream: Option<AudioCheck>,
    audio_active: Option<Arc<AtomicBool>>,
    camera_stream: Option<CaptureStream>,
}

struct Inner {
    state: Mutex<State>,
    options: MediaOptions,
    runtime: tokio::runtime::Handle,
    levels: watch::Sender<Levels>,
    view: watch::Sender<DeviceCheckView>,
    picture: watch::Sender<Option<Arc<VideoPicture>>>,
}

impl Inner {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
    fn audio_failed(&self, generation: u64, error: AudioError) {
        let stream = {
            let mut state = self.state();
            if state.audio != generation {
                return;
            }
            state.audio += 1;
            if let Some(active) = state.audio_active.take() {
                active.store(false, Ordering::Release);
            }
            self.view
                .send_modify(|view| view.audio = AudioCheckState::Failed(error));
            state.audio_stream.take()
        };
        self.runtime.spawn_blocking(move || drop(stream));
        self.levels.send_modify(|levels| levels.input = 0.0);
    }
    fn camera_failed(&self, generation: u64, error: CameraError) {
        let stream = {
            let mut state = self.state();
            if state.camera != generation {
                return;
            }
            state.camera += 1;
            self.view
                .send_modify(|view| view.camera = CameraState::Failed(error));
            self.picture.send_replace(None);
            state.camera_stream.take()
        };
        self.runtime.spawn_blocking(move || drop(stream));
    }
}

struct Sink {
    inner: Weak<Inner>,
    generation: u64,
}

impl CameraSink for Sink {
    fn picture(&self, buffer: Arc<dyn VideoBuffer>) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        if inner.state().camera != self.generation {
            return;
        }
        let picture = VideoPicture::of_buffer(&buffer, true, (640, 360)).map(Arc::new);
        let state = inner.state();
        if state.camera == self.generation && picture.is_some() {
            inner.picture.send_replace(picture);
        }
    }
    fn ended(&self, error: CameraError) {
        if let Some(inner) = self.inner.upgrade() {
            inner.camera_failed(self.generation, error);
        }
    }
}

pub(crate) struct DeviceChecks {
    inner: Arc<Inner>,
}

impl DeviceChecks {
    pub(crate) fn new(
        options: MediaOptions,
        runtime: tokio::runtime::Handle,
        levels: watch::Sender<Levels>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State::default()),
                options,
                runtime,
                levels,
                view: watch::Sender::new(DeviceCheckView::default()),
                picture: watch::Sender::new(None),
            }),
        }
    }
    pub(crate) fn view(&self) -> DeviceCheckView {
        self.inner.view.borrow().clone()
    }
    pub(crate) fn watch(&self) -> watch::Receiver<DeviceCheckView> {
        self.inner.view.subscribe()
    }
    pub(crate) fn picture(&self) -> watch::Receiver<Option<Arc<VideoPicture>>> {
        self.inner.picture.subscribe()
    }

    pub(crate) fn preview(&self) -> (u64, Option<Arc<VideoPicture>>) {
        let state = self.inner.state();
        (state.camera, self.inner.picture.borrow().clone())
    }

    pub(crate) fn start_audio(&self, kind: AudioCheckKind, voice: VoiceConfig) {
        let active = Arc::new(AtomicBool::new(true));
        let (generation, replaced) = {
            let mut state = self.inner.state();
            state.audio += 1;
            if let Some(active) = state.audio_active.take() {
                active.store(false, Ordering::Release);
            }
            state.audio_active = Some(active.clone());
            self.inner
                .view
                .send_modify(|view| view.audio = AudioCheckState::Starting(kind));
            (state.audio, state.audio_stream.take())
        };
        self.inner.runtime.spawn_blocking(move || drop(replaced));
        self.inner.levels.send_modify(|levels| levels.input = 0.0);
        let inner = self.inner.clone();
        self.inner.runtime.spawn(async move {
            if kind == AudioCheckKind::Microphone {
                let permission = inner.options.permission.clone();
                let access = tokio::task::spawn_blocking(move || permission.state()).await;
                if inner.state().audio != generation {
                    return;
                }
                let access = match access {
                    Ok(MicrophoneAccess::NotAsked) => inner.options.permission.request().await,
                    Ok(access) => access,
                    Err(_) => MicrophoneAccess::Refused,
                };
                if access != MicrophoneAccess::Allowed {
                    inner.audio_failed(generation, AudioError::Refused);
                    return;
                }
            }
            if inner.state().audio != generation {
                return;
            }
            let weak = Arc::downgrade(&inner);
            let failed: crate::audio::check::Failure = Arc::new(move |error| {
                if let Some(inner) = weak.upgrade() {
                    inner.audio_failed(generation, error);
                }
            });
            let opener = inner.clone();
            let opened = tokio::task::spawn_blocking(move || match kind {
                AudioCheckKind::Microphone => AudioCheck::microphone(
                    opener.options.host.as_ref(),
                    &voice,
                    opener.levels.clone(),
                    failed,
                    active,
                    &opener.runtime,
                ),
                AudioCheckKind::Output => {
                    AudioCheck::output(opener.options.host.as_ref(), &voice, failed, active)
                }
            })
            .await
            .unwrap_or_else(|error| Err(AudioError::Failed(error.to_string())));
            match opened {
                Ok(stream) => {
                    let discarded = {
                        let mut state = inner.state();
                        if state.audio == generation {
                            inner
                                .view
                                .send_modify(|view| view.audio = AudioCheckState::Running(kind));
                            state.audio_stream.replace(stream)
                        } else {
                            Some(stream)
                        }
                    };
                    inner.runtime.spawn_blocking(move || drop(discarded));
                }
                Err(error) => inner.audio_failed(generation, error),
            }
        });
    }

    pub(crate) fn stop_audio(&self) {
        let stream = {
            let mut state = self.inner.state();
            state.audio += 1;
            if let Some(active) = state.audio_active.take() {
                active.store(false, Ordering::Release);
            }
            self.inner.view.send_if_modified(|view| {
                let changed = view.audio != AudioCheckState::Off;
                view.audio = AudioCheckState::Off;
                changed
            });
            state.audio_stream.take()
        };
        self.inner.runtime.spawn_blocking(move || drop(stream));
        self.inner.levels.send_modify(|levels| levels.input = 0.0);
    }

    pub(crate) fn start_camera(&self, device: Option<String>) -> tokio::task::JoinHandle<()> {
        let (generation, replaced) = {
            let mut state = self.inner.state();
            state.camera += 1;
            self.inner.picture.send_replace(None);
            self.inner
                .view
                .send_modify(|view| view.camera = CameraState::Starting);
            (state.camera, state.camera_stream.take())
        };
        self.inner.runtime.spawn_blocking(move || drop(replaced));
        let inner = self.inner.clone();
        self.inner.runtime.spawn(async move {
            let capture = inner.options.camera.clone();
            let reader = capture.clone();
            let access = tokio::task::spawn_blocking(move || reader.access()).await;
            if inner.state().camera != generation {
                return;
            }
            let access = match access {
                Ok(CameraAccess::NotAsked) => capture.request_access().await,
                Ok(access) => access,
                Err(_) => CameraAccess::Refused,
            };
            if inner.state().camera != generation {
                return;
            }
            let opened = if access == CameraAccess::Allowed {
                capture
                    .start(
                        device,
                        Arc::new(Sink {
                            inner: Arc::downgrade(&inner),
                            generation,
                        }),
                    )
                    .await
            } else {
                Err(CameraError::NoPermission(access))
            };
            match opened {
                Ok(stream) => {
                    let discarded = {
                        let mut state = inner.state();
                        if state.camera == generation {
                            inner
                                .view
                                .send_modify(|view| view.camera = CameraState::Running);
                            state.camera_stream.replace(stream)
                        } else {
                            Some(stream)
                        }
                    };
                    inner.runtime.spawn_blocking(move || drop(discarded));
                }
                Err(error) => inner.camera_failed(generation, error),
            }
        })
    }

    pub(crate) fn stop_camera(&self) {
        let stream = {
            let mut state = self.inner.state();
            state.camera += 1;
            self.inner.picture.send_replace(None);
            self.inner.view.send_if_modified(|view| {
                let changed = view.camera != CameraState::Off;
                view.camera = CameraState::Off;
                changed
            });
            state.camera_stream.take()
        };
        self.inner.runtime.spawn_blocking(move || drop(stream));
    }
    pub(crate) fn stop(&self) {
        self.stop_audio();
        self.stop_camera();
    }
}

impl Drop for DeviceChecks {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests;
