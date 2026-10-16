//! AVFoundation capture. Session objects stay on one capture thread.

use std::sync::{Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{Bool, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use objc2_av_foundation::{
    AVAuthorizationStatus, AVCaptureConnection, AVCaptureDevice, AVCaptureDeviceInput,
    AVCaptureOutput, AVCaptureSession, AVCaptureSessionPreset352x288,
    AVCaptureSessionPreset640x480, AVCaptureSessionPreset1280x720, AVCaptureVideoDataOutput,
    AVCaptureVideoDataOutputSampleBufferDelegate, AVMediaTypeVideo,
};
use objc2_core_media::CMSampleBuffer;
use objc2_foundation::NSString;
use tokio::sync::oneshot;

use super::*;

pub(super) struct SystemCamera;

fn access() -> CameraAccess {
    // SAFETY: the framework constant is static; this reads without asking permission.
    let status = unsafe {
        let Some(video) = AVMediaTypeVideo else {
            return CameraAccess::Restricted;
        };
        AVCaptureDevice::authorizationStatusForMediaType(video)
    };
    match status {
        AVAuthorizationStatus::Authorized => CameraAccess::Allowed,
        AVAuthorizationStatus::NotDetermined => CameraAccess::NotAsked,
        AVAuthorizationStatus::Restricted => CameraAccess::Restricted,
        _ => CameraAccess::Refused,
    }
}

impl CameraCapture for SystemCamera {
    fn access(&self) -> CameraAccess {
        access()
    }

    fn request_access(&self) -> Pending<CameraAccess> {
        Box::pin(async {
            if access() != CameraAccess::NotAsked {
                return access();
            }
            let (tx, rx) = oneshot::channel();
            {
                let tx = Mutex::new(Some(tx));
                let handler = RcBlock::new(move |_: Bool| {
                    if let Ok(mut tx) = tx.lock()
                        && let Some(tx) = tx.take()
                    {
                        let _ = tx.send(());
                    }
                });
                // SAFETY: the framework copies the block, which owns its sender.
                unsafe {
                    let Some(video) = AVMediaTypeVideo else {
                        return CameraAccess::Restricted;
                    };
                    AVCaptureDevice::requestAccessForMediaType_completionHandler(video, &handler);
                }
            }
            let _ = rx.await;
            access()
        })
    }

    fn devices(&self) -> Result<Vec<CameraDevice>, CameraError> {
        // SAFETY: enumeration opens no device and asks no permission.
        autoreleasepool(|_| unsafe {
            let video = AVMediaTypeVideo.ok_or(CameraError::Unsupported)?;
            #[allow(deprecated)]
            let devices = AVCaptureDevice::devicesWithMediaType(video);
            Ok(devices
                .iter()
                .filter(|device| device.isConnected())
                .map(|device| CameraDevice {
                    id: device.uniqueID().to_string(),
                    name: device.localizedName().to_string(),
                })
                .collect())
        })
    }

    fn start(
        &self,
        chosen: Option<String>,
        sink: Arc<dyn CameraSink>,
    ) -> Pending<Result<CaptureStream, CameraError>> {
        Box::pin(async move {
            let permission = access();
            if permission != CameraAccess::Allowed {
                return Err(CameraError::NoPermission(permission));
            }
            let device = choose(&SystemCamera.devices()?, chosen.as_deref())?;
            let (ready, result) = oneshot::channel();
            let (stop, stopped) = mpsc::channel();
            let thread = std::thread::Builder::new()
                .name("camera-capture".into())
                .spawn(move || {
                    // SAFETY: all session operations are on this thread. The delegate is
                    // retained until the session and its serial delivery queue have stopped.
                    unsafe {
                        match autoreleasepool(|_| Session::open(&device.id, sink.clone())) {
                            Err(error) => {
                                let _ = ready.send(Err(error));
                            }
                            Ok(session) => {
                                if ready.send(Ok(session.size)).is_err() {
                                    return;
                                }
                                loop {
                                    match stopped.recv_timeout(Duration::from_millis(100)) {
                                        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                                    }
                                    let failure = autoreleasepool(|_| {
                                        if !session.device.isConnected() {
                                            Some(CameraError::DeviceGone)
                                        } else if access_now_denied() {
                                            Some(CameraError::NoPermission(access()))
                                        } else if !session.session.isRunning()
                                            || session.session.isInterrupted()
                                        {
                                            Some(CameraError::Interrupted)
                                        } else {
                                            None
                                        }
                                    });
                                    if let Some(error) = failure {
                                        drop(session);
                                        sink.ended(error);
                                        return;
                                    }
                                }
                            }
                        }
                    }
                })
                .map_err(|error| CameraError::Failed(error.to_string()))?;
            let stream = Running {
                stop,
                thread: Some(thread),
            };
            let (width, height) = result
                .await
                .map_err(|_| CameraError::Failed("capture stopped before starting".into()))??;
            Ok(CaptureStream::new(width, height, stream))
        })
    }
}

fn access_now_denied() -> bool {
    access() != CameraAccess::Allowed
}

struct Running {
    stop: mpsc::Sender<()>,
    thread: Option<JoinHandle<()>>,
}
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        // The capture thread owns shutdown; dropping a stream must not block the call.
        self.thread.take();
    }
}

struct Ivars {
    sink: Arc<dyn CameraSink>,
}

define_class!(
    // SAFETY: NSObject imposes no subclass requirements; ivars own their values.
    #[unsafe(super(NSObject))]
    #[ivars = Ivars]
    struct Output;
    unsafe impl NSObjectProtocol for Output {}
    unsafe impl AVCaptureVideoDataOutputSampleBufferDelegate for Output {
        #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
        fn frame(&self, _: &AVCaptureOutput, sample: &CMSampleBuffer, _: &AVCaptureConnection) {
            // SAFETY: sample is valid during the callback; the retained image reference
            // is transferred to the video buffer, as required by the call library.
            unsafe {
                let Some(image) = sample.image_buffer() else {
                    return;
                };
                use core_foundation::base::TCFType;
                let image = core_video::pixel_buffer::CVPixelBuffer::wrap_under_get_rule(
                    (&*image as *const _ as *mut std::ffi::c_void).cast(),
                );
                if let Some(buffer) = super::color::convert(&image) {
                    self.ivars().sink.picture(buffer);
                } else {
                    self.ivars().sink.ended(CameraError::Unsupported);
                }
            }
        }
    }
);

struct Session {
    size: (u32, u32),
    session: Retained<AVCaptureSession>,
    device: Retained<AVCaptureDevice>,
    output: Retained<AVCaptureVideoDataOutput>,
    _delegate: Retained<Output>,
    queue: dispatch2::DispatchRetained<DispatchQueue>,
}
impl Session {
    unsafe fn open(id: &str, sink: Arc<dyn CameraSink>) -> Result<Self, CameraError> {
        // SAFETY: objects are configured before starting and kept alive by Session.
        unsafe {
            let device = AVCaptureDevice::deviceWithUniqueID(&NSString::from_str(id))
                .ok_or(CameraError::DeviceGone)?;
            let input = AVCaptureDeviceInput::deviceInputWithDevice_error(&device)
                .map_err(|error| CameraError::Failed(error.localizedDescription().to_string()))?;
            let session = AVCaptureSession::new();
            if !session.canAddInput(&input) {
                return Err(CameraError::Failed("camera input is unavailable".into()));
            }
            session.addInput(&input);
            let preset = |size| match size {
                (1280, 720) => AVCaptureSessionPreset1280x720,
                (640, 480) => AVCaptureSessionPreset640x480,
                _ => AVCaptureSessionPreset352x288,
            };
            let size = super::format::choose(|size| session.canSetSessionPreset(preset(size)))
                .ok_or_else(|| {
                    CameraError::Failed("camera has no supported capture size".into())
                })?;
            session.setSessionPreset(preset(size));
            let output = AVCaptureVideoDataOutput::new();
            output.setAlwaysDiscardsLateVideoFrames(true);
            let settings = super::settings::video(size).ok_or(CameraError::Unsupported)?;
            output.setVideoSettings(Some(&settings));
            if !session.canAddOutput(&output) {
                return Err(CameraError::Failed("camera output is unavailable".into()));
            }
            session.addOutput(&output);
            let delegate: Retained<Output> =
                msg_send![super(Output::alloc().set_ivars(Ivars { sink })), init];
            let queue = DispatchQueue::new("camera-frames", None);
            output.setSampleBufferDelegate_queue(
                Some(ProtocolObject::from_ref(&*delegate)),
                Some(&queue),
            );
            if let Some(video) = AVMediaTypeVideo
                && let Some(connection) = output.connectionWithMediaType(video)
                && connection.isVideoMirroringSupported()
            {
                connection.setAutomaticallyAdjustsVideoMirroring(false);
                connection.setVideoMirrored(false);
            }
            let held = Self {
                size,
                session,
                device,
                output,
                _delegate: delegate,
                queue,
            };
            held.session.startRunning();
            if !held.session.isRunning() {
                return Err(CameraError::Failed("camera did not start".into()));
            }
            Ok(held)
        }
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: shutdown runs on the owning thread, outside the delivery queue.
        autoreleasepool(|_| unsafe {
            self.session.stopRunning();
            self.output.setSampleBufferDelegate_queue(None, None);
        });
        self.queue.exec_sync(|| {});
    }
}
