//! Camera capture and publication belong to the lifetime of one media connection.
use super::*;
use crate::camera::CameraFrame;
use crate::camera::{CameraAccess, CameraError, CameraSink, CameraState};
use crate::share::CaptureStream;
use livekit::options::{VideoEncoding, VideoPreset};
use livekit::prelude::{LocalVideoTrack, RemoteTrackPublication, RemoteVideoTrack};
use livekit::track::TrackDimension;
use livekit::webrtc::video_frame::{BoxVideoFrame, VideoBuffer, VideoFrame, VideoRotation};
use livekit::webrtc::video_source::{RtcVideoSource, VideoResolution, native::NativeVideoSource};
use livekit::webrtc::video_stream::native::NativeVideoStream;
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) struct Sink {
    generation: u64,
    internal: mpsc::UnboundedSender<Internal>,
    source: Mutex<Option<NativeVideoSource>>,
    last: Mutex<Option<Arc<dyn VideoBuffer>>>,
    ended: AtomicBool,
}
impl CameraSink for Sink {
    fn picture(&self, frame: Arc<dyn VideoBuffer>) {
        if self.ended.load(Ordering::Acquire) {
            return;
        }
        if let Some(source) = &*self.source.lock().unwrap_or_else(PoisonError::into_inner) {
            source.capture_frame(&VideoFrame::new(
                VideoRotation::VideoRotation0,
                frame.clone(),
            ));
        }
        *self.last.lock().unwrap_or_else(PoisonError::into_inner) = Some(frame);
    }
    fn ended(&self, error: CameraError) {
        if !self.ended.swap(true, Ordering::AcqRel) {
            let _ = self
                .internal
                .send(Internal::CameraEnded(self.generation, error));
        }
    }
}
pub(super) struct OwnCamera {
    generation: u64,
    sink: Arc<Sink>,
    started: Arc<Mutex<Option<Result<CaptureStream, CameraError>>>>,
    stream: Option<CaptureStream>,
    task: JoinHandle<()>,
    publication: Publication,
    returning: Option<TrackSid>,
    seen: Option<Arc<dyn VideoBuffer>>,
}
impl Drop for OwnCamera {
    fn drop(&mut self) {
        self.sink.ended.store(true, Ordering::Release);
        self.task.abort();
        self.stream.take();
        self.started
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
    }
}
pub(super) struct RemoteCamera {
    account: AccountId,
    publication: RemoteTrackPublication,
    frames: watch::Receiver<Option<Arc<BoxVideoFrame>>>,
    task: JoinHandle<()>,
}
impl Drop for RemoteCamera {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Actor {
    pub(super) fn start_camera(&mut self, device: Option<String>) {
        self.end_camera(None);
        if !self.joined() || !self.may_send(1) {
            self.change(|s| s.camera = CameraState::Failed(CameraError::Blocked));
            return;
        }
        self.camera_generation += 1;
        let generation = self.camera_generation;
        let sink = Arc::new(Sink {
            generation,
            internal: self.internal.clone(),
            source: Mutex::new(None),
            last: Mutex::new(None),
            ended: AtomicBool::new(false),
        });
        let started = Arc::new(Mutex::new(None));
        let (capture, target, result, internal) = (
            self.options.camera.clone(),
            sink.clone(),
            started.clone(),
            self.internal.clone(),
        );
        let task = self.runtime.spawn(async move {
            let access = if capture.access() == CameraAccess::NotAsked {
                capture.request_access().await
            } else {
                capture.access()
            };
            let answer = if access == CameraAccess::Allowed {
                capture.start(device, target).await
            } else {
                Err(CameraError::NoPermission(access))
            };
            *result.lock().unwrap_or_else(PoisonError::into_inner) = Some(answer);
            let _ = internal.send(Internal::CameraStarted(generation));
        });
        self.camera = Some(OwnCamera {
            generation,
            sink,
            started,
            stream: None,
            task,
            publication: Publication::None,
            returning: None,
            seen: None,
        });
        self.change(|s| s.camera = CameraState::Starting);
    }
    pub(super) fn camera_started(&mut self, generation: u64) {
        let Some(camera) = self.camera.as_mut().filter(|c| c.generation == generation) else {
            return;
        };
        let answer = camera
            .started
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let stream = match answer {
            Some(Ok(stream)) => stream,
            Some(Err(error)) => {
                self.end_camera(Some(error));
                return;
            }
            None => return,
        };
        let Some(room) = &self.room else {
            return;
        };
        let source = NativeVideoSource::new(
            VideoResolution {
                width: stream.width,
                height: stream.height,
            },
            false,
        );
        *camera
            .sink
            .source
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(source.clone());
        if let Some(frame) = camera
            .sink
            .last
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
        {
            source.capture_frame(&VideoFrame::new(VideoRotation::VideoRotation0, frame));
        }
        camera.stream = Some(stream);
        camera.publication = Publication::Publishing;
        let track = LocalVideoTrack::create_video_track("camera", RtcVideoSource::Native(source));
        let options = TrackPublishOptions {
            source: TrackSource::Camera,
            video_encoding: Some(VideoEncoding {
                max_bitrate: 1_700_000,
                max_framerate: 30.0,
            }),
            simulcast_layers: Some(vec![
                VideoPreset::new(320, 180, 150_000, 15.0),
                VideoPreset::new(640, 360, 500_000, 30.0),
            ]),
            ..TrackPublishOptions::default()
        };
        let (participant, internal) = (room.local_participant(), self.internal.clone());
        self.runtime.spawn(async move {
            let result = match tokio::time::timeout(
                PUBLISH_LIMIT,
                participant.publish_track(LocalTrack::Video(track), options),
            )
            .await
            {
                Ok(Ok(publication)) => Ok(publication),
                Ok(Err(error)) => Err(error.to_string()),
                Err(_) => Err("camera publication timed out".to_owned()),
            };
            let _ = internal.send(Internal::CameraPublished(generation, result));
        });
    }
    pub(super) fn camera_published(
        &mut self,
        generation: u64,
        result: Result<LocalTrackPublication, String>,
    ) {
        match (
            self.camera.as_mut().filter(|c| c.generation == generation),
            result,
        ) {
            (Some(camera), Ok(publication)) => {
                camera.publication = Publication::Published(publication);
                self.change(|s| s.camera = CameraState::Running);
            }
            (Some(_), Err(error)) => self.end_camera(Some(CameraError::Failed(error))),
            (None, Ok(publication)) => self.withdraw(publication),
            (None, Err(_)) => {}
        }
    }
    pub(super) fn camera_ended(&mut self, generation: u64, error: CameraError) {
        if self
            .camera
            .as_ref()
            .is_some_and(|c| c.generation == generation)
        {
            self.end_camera(Some(error));
        }
    }
    pub(super) fn end_camera(&mut self, error: Option<CameraError>) {
        if let Some(mut camera) = self.camera.take()
            && let Publication::Published(publication) =
                std::mem::replace(&mut camera.publication, Publication::None)
        {
            self.withdraw(publication);
        }
        self.camera_pictures
            .send_if_modified(|pictures| pictures.remove(&ShareId::Own).is_some());
        self.change(|s| s.camera = error.map(CameraState::Failed).unwrap_or(CameraState::Off));
    }
    pub(super) fn camera_track_taken(&mut self, sid: &TrackSid) {
        let Some(camera) = self.camera.as_mut() else {
            return;
        };
        if !matches!(&camera.publication, Publication::Published(p) if p.sid() == *sid) {
            return;
        }
        camera.returning = Some(sid.clone());
        camera.publication = Publication::Publishing;
        let (generation, sid, internal) = (camera.generation, sid.clone(), self.internal.clone());
        self.change(|s| s.camera = CameraState::Starting);
        self.runtime.spawn(async move {
            tokio::time::sleep(PUBLISH_LIMIT).await;
            let _ = internal.send(Internal::CameraDue(generation, sid));
        });
    }
    pub(super) fn camera_republished(
        &mut self,
        previous: TrackSid,
        publication: LocalTrackPublication,
    ) {
        let matches = self.camera.as_ref().is_some_and(|c| {
            c.returning.as_ref() == Some(&previous)
                || matches!(&c.publication, Publication::Published(p) if p.sid() == previous)
        });
        if matches && self.may_send(1) {
            let Some(camera) = self.camera.as_mut() else {
                return;
            };
            camera.returning = None;
            camera.publication = Publication::Published(publication);
            self.change(|s| s.camera = CameraState::Running);
        } else {
            self.withdraw(publication);
        }
    }
    pub(super) fn camera_due(&mut self, generation: u64, sid: TrackSid) {
        if self
            .camera
            .as_ref()
            .is_some_and(|c| c.generation == generation && c.returning.as_ref() == Some(&sid))
        {
            self.end_camera(Some(CameraError::Interrupted));
        }
    }
    pub(super) fn remote_camera(
        &mut self,
        track: &RemoteVideoTrack,
        publication: RemoteTrackPublication,
        identity: &str,
    ) {
        let Some(account) = account_of(identity) else {
            return;
        };
        let mut stream = NativeVideoStream::new(track.rtc_track());
        let (frames, receiver) = watch::channel(None);
        let task = self.runtime.spawn(async move {
            while let Some(frame) = stream.next().await {
                frames.send_replace(Some(Arc::new(frame)));
            }
        });
        self.remote_cameras.insert(
            publication.sid(),
            RemoteCamera {
                account,
                publication,
                frames: receiver,
                task,
            },
        );
        self.tell_cameras();
    }
    pub(super) fn camera_track_ended(&mut self, sid: &TrackSid) {
        if let Some(camera) = self.remote_cameras.remove(sid) {
            self.camera_pictures
                .send_if_modified(|p| p.remove(&ShareId::Of(camera.account)).is_some());
            self.tell_cameras();
        }
    }
    fn tell_cameras(&mut self) {
        let mut accounts: Vec<_> = self.remote_cameras.values().map(|c| c.account).collect();
        accounts.sort();
        accounts.dedup();
        let size = if accounts.len() > 1 {
            (640, 360)
        } else {
            (1280, 720)
        };
        for camera in self.remote_cameras.values() {
            let publication = camera.publication.clone();
            order::in_order(move || {
                publication.update_video_dimensions(TrackDimension(size.0, size.1))
            });
        }
        self.change(|s| s.cameras = accounts);
    }
    pub(super) fn draw_cameras(&mut self) {
        let size = if self.remote_cameras.len() > 1 {
            (640, 360)
        } else {
            (1280, 720)
        };
        let mut changed = Vec::new();
        if let Some(camera) = &mut self.camera {
            let frame = camera
                .sink
                .last
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            if let Some(frame) = frame
                && camera
                    .seen
                    .as_ref()
                    .is_none_or(|old| !Arc::ptr_eq(old, &frame))
            {
                if let Some(picture) = CameraFrame::new(frame.as_ref(), true, (640, 360)) {
                    changed.push((ShareId::Own, Arc::new(picture)));
                }
                camera.seen = Some(frame);
            }
        }
        for camera in self.remote_cameras.values_mut() {
            if camera.frames.has_changed().unwrap_or(false)
                && let Some(frame) = camera.frames.borrow_and_update().clone()
                && let Some(picture) = CameraFrame::new(frame.buffer.as_ref(), false, size)
            {
                changed.push((ShareId::Of(camera.account), Arc::new(picture)));
            }
        }
        if !changed.is_empty() {
            self.camera_pictures.send_modify(|p| p.extend(changed));
        }
    }
}
