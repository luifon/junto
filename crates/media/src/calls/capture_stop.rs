//! Capture cleanup failures that outlive the engine and its call.

use std::sync::Arc;

use tokio::sync::watch;

use super::CallsView;
use crate::audio::Format;
use crate::share::{
    CaptureError, CaptureNotice, CaptureRequest, CaptureSink, CaptureStream, Pending,
    PictureSource, ScreenAccess, ScreenCapture, ShareSources, picture::PictureFrame,
};
use livekit::webrtc::video_frame::VideoBuffer;

pub(super) fn reporting(
    capture: Arc<dyn ScreenCapture>,
    view: watch::Sender<CallsView>,
) -> Arc<dyn ScreenCapture> {
    Arc::new(Reporting { capture, view })
}

struct Reporting {
    capture: Arc<dyn ScreenCapture>,
    view: watch::Sender<CallsView>,
}

impl ScreenCapture for Reporting {
    fn supported(&self) -> bool {
        self.capture.supported()
    }

    fn access(&self) -> ScreenAccess {
        self.capture.access()
    }

    fn request_access(&self) {
        self.capture.request_access();
    }

    fn open_settings(&self) {
        self.capture.open_settings();
    }

    fn sources(&self) -> Pending<Result<ShareSources, CaptureError>> {
        self.capture.sources()
    }

    fn preview(
        &self,
        source: PictureSource,
        bound: (u32, u32),
    ) -> Pending<Result<PictureFrame, CaptureError>> {
        self.capture.preview(source, bound)
    }

    fn start(
        &self,
        request: CaptureRequest,
        sink: Arc<dyn CaptureSink>,
    ) -> Pending<Result<CaptureStream, CaptureError>> {
        self.capture.start(
            request,
            Arc::new(Sink {
                sink,
                view: self.view.clone(),
            }),
        )
    }
}

struct Sink {
    sink: Arc<dyn CaptureSink>,
    view: watch::Sender<CallsView>,
}

impl CaptureSink for Sink {
    fn picture(&self, buffer: Arc<dyn VideoBuffer>) {
        self.sink.picture(buffer);
    }

    fn sound(&self, samples: &[f32], format: Format) {
        self.sink.sound(samples, format);
    }

    fn ended(&self, error: CaptureError) {
        self.sink.ended(error);
    }

    fn stop_failed(&self, error: CaptureNotice) {
        self.view
            .send_modify(|view| view.share_stop_failed = Some(error));
    }
}

pub(super) fn reporting_camera(
    capture: Arc<dyn crate::camera::CameraCapture>,
    view: watch::Sender<CallsView>,
) -> Arc<dyn crate::camera::CameraCapture> {
    Arc::new(Camera { capture, view })
}
struct Camera {
    capture: Arc<dyn crate::camera::CameraCapture>,
    view: watch::Sender<CallsView>,
}
impl crate::camera::CameraCapture for Camera {
    fn access(&self) -> crate::camera::CameraAccess {
        self.capture.access()
    }
    fn request_access(&self) -> Pending<crate::camera::CameraAccess> {
        self.capture.request_access()
    }
    fn devices(&self) -> Result<Vec<crate::camera::CameraDevice>, crate::camera::CameraError> {
        self.capture.devices()
    }
    fn start(
        &self,
        device: Option<String>,
        sink: Arc<dyn crate::camera::CameraSink>,
    ) -> Pending<Result<CaptureStream, crate::camera::CameraError>> {
        self.capture.start(
            device,
            Arc::new(CameraSink {
                sink,
                view: self.view.clone(),
            }),
        )
    }
}
struct CameraSink {
    sink: Arc<dyn crate::camera::CameraSink>,
    view: watch::Sender<CallsView>,
}
impl crate::camera::CameraSink for CameraSink {
    fn picture(&self, buffer: Arc<dyn VideoBuffer>) {
        self.sink.picture(buffer);
    }
    fn ended(&self, error: crate::camera::CameraError) {
        self.sink.ended(error);
    }
    fn stop_failed(&self, notice: CaptureNotice) {
        self.view
            .send_modify(|view| view.share_stop_failed = Some(notice));
    }
}
