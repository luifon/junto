//! Camera capture and publication belong to the lifetime of one media connection.
use super::*;
use crate::camera::{CameraAccess, CameraError, CameraSink, CameraState};
use crate::share::CaptureStream;
use crate::share::picture::VideoPicture;
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
    stops: Arc<Stops>,
    /// The count of camera stops when this camera was asked for.
    stop: u64,
}
impl CameraSink for Sink {
    fn picture(&self, frame: Arc<dyn VideoBuffer>) {
        if self.ended.load(Ordering::Acquire)
            || self.stops.camera.load(Ordering::Relaxed) != self.stop
        {
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
    /// The number of the publication that this camera asked for last. The result of a
    /// publication with another number is not the publication of the camera: the camera
    /// was held, or the connection was lost, after the request.
    round: u64,
    returning: Option<TrackSid>,
    seen: Option<Arc<dyn VideoBuffer>>,
}
impl OwnCamera {
    /// Ends the publication of the camera and keeps the capture. A publication that was
    /// asked for and has no result yet is old from now on. Returns the track that was
    /// published.
    fn unpublish(&mut self) -> Option<LocalTrackPublication> {
        self.round += 1;
        *self
            .sink
            .source
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
        self.returning = None;
        match std::mem::replace(&mut self.publication, Publication::None) {
            Publication::Published(publication) => Some(publication),
            Publication::None | Publication::Publishing => None,
        }
    }
}

/// What becomes of the result of a publication of the camera.
#[derive(Debug, PartialEq, Eq)]
enum Taken {
    /// It is the publication of the camera.
    Kept,
    /// The publication failed, and the camera ends.
    Failed,
    /// A track was published that is not the one of the camera: it is withdrawn.
    Withdrawn,
    /// A failure of a publication that the camera does not wait for.
    Dropped,
}

/// `current`: the number of the publication that the camera of that capture waits for,
/// absent when that camera has ended. `round`: the number of the publication with this
/// result.
fn taken(current: Option<u64>, round: u64, published: bool) -> Taken {
    match (current == Some(round), published) {
        (true, true) => Taken::Kept,
        (true, false) => Taken::Failed,
        (false, true) => Taken::Withdrawn,
        (false, false) => Taken::Dropped,
    }
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
/// Reads the camera permission once, on a blocking thread because the Linux camera opens each
/// device node to read it, asks for it when it was never asked, then starts capture.
async fn open(
    capture: Arc<dyn crate::camera::CameraCapture>,
    device: Option<String>,
    target: Arc<dyn CameraSink>,
) -> Result<CaptureStream, CameraError> {
    let reader = capture.clone();
    let access = tokio::task::spawn_blocking(move || reader.access())
        .await
        .map_err(|_| CameraError::Failed("the camera permission was not read".into()))?;
    let access = if access == CameraAccess::NotAsked {
        capture.request_access().await
    } else {
        access
    };
    if access == CameraAccess::Allowed {
        capture.start(device, target).await
    } else {
        Err(CameraError::NoPermission(access))
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
    /// `stop`: the count of camera stops when the start was asked for. After a later stop
    /// the camera sends no frame.
    pub(super) fn start_camera(&mut self, device: Option<String>, stop: u64) {
        self.end_camera(None);
        if let Some(error) =
            start_error(self.connection() == Connection::Connected, self.may_send(1))
        {
            self.change(|s| s.camera = CameraState::Failed(error));
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
            stops: self.stops.clone(),
            stop,
        });
        let started = Arc::new(Mutex::new(None));
        let (capture, target, result, internal) = (
            self.options.camera.clone(),
            sink.clone(),
            started.clone(),
            self.internal.clone(),
        );
        let task = self.runtime.spawn(async move {
            let answer = open(capture, device, target).await;
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
            round: 0,
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
        camera.stream = Some(stream);
        self.publish_camera();
    }
    pub(super) fn pause_camera(&mut self) {
        if let Some(camera) = &mut self.camera {
            // The connection of the track is gone: nothing is withdrawn.
            camera.unpublish();
            self.change(|s| s.camera = CameraState::Starting);
        }
    }
    /// The seat is not admitted any more: the track of the camera is withdrawn, and the
    /// capture goes on for the next publication.
    pub(super) fn hold_camera(&mut self) {
        let Some(camera) = self.camera.as_mut() else {
            return;
        };
        if let Some(publication) = camera.unpublish() {
            self.withdraw(publication);
        }
        self.change(|s| s.camera = CameraState::Starting);
    }
    pub(super) fn publish_camera(&mut self) {
        if self.room.is_none() || self.controls.camera_held {
            return;
        }
        if !self.may_send(1) {
            if self.camera.is_some() {
                self.end_camera(Some(CameraError::Blocked));
            }
            return;
        }
        let Some(room) = &self.room else {
            return;
        };
        let Some(camera) = self.camera.as_mut() else {
            return;
        };
        if !matches!(camera.publication, Publication::None) {
            return;
        }
        let Some(stream) = &camera.stream else {
            return;
        };
        let generation = camera.generation;
        let attempt = self.attempt;
        camera.round += 1;
        let round = camera.round;
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
            let _ = internal.send(Internal::CameraPublished(
                generation, attempt, round, result,
            ));
        });
    }
    pub(super) fn camera_published(
        &mut self,
        generation: u64,
        attempt: u64,
        round: u64,
        result: Result<LocalTrackPublication, String>,
    ) {
        if attempt != self.attempt {
            return;
        }
        let camera = self.camera.as_mut().filter(|c| c.generation == generation);
        let current = camera.as_ref().map(|camera| camera.round);
        match (taken(current, round, result.is_ok()), camera, result) {
            (Taken::Kept, Some(camera), Ok(publication)) => {
                camera.publication = Publication::Published(publication);
                self.change(|s| s.camera = CameraState::Running);
            }
            (Taken::Failed, _, Err(error)) => self.end_camera(Some(CameraError::Failed(error))),
            // The camera was held, or it ended, while the media server published the
            // track: the track is not the one of the camera.
            (_, _, Ok(publication)) => self.withdraw(publication),
            (_, _, Err(_)) => {}
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
        self.apply_camera_views();
        self.change(|s| s.cameras = accounts);
    }
    pub(super) fn set_camera_views(&mut self, views: HashMap<ShareId, (u32, u32)>) {
        self.camera_pictures
            .send_modify(|pictures| pictures.retain(|id, _| views.contains_key(id)));
        self.camera_views = Some(views);
        if let Some(camera) = &mut self.camera {
            camera.seen = None;
        }
        self.apply_camera_views();
        self.draw_cameras();
    }
    fn apply_camera_views(&self) {
        let default = if self.remote_cameras.len() > 1 {
            (640, 360)
        } else {
            (1280, 720)
        };
        for camera in self.remote_cameras.values() {
            let requested = match &self.camera_views {
                Some(views) => views.get(&ShareId::Of(camera.account)).copied(),
                None => Some(default),
            };
            let publication = camera.publication.clone();
            order::in_order(move || {
                publication.set_enabled(requested.is_some());
                if let Some(size) = requested {
                    publication.update_video_dimensions(TrackDimension(size.0, size.1));
                }
            });
        }
    }
    pub(super) fn draw_cameras(&mut self) {
        let size = if self.remote_cameras.len() > 1 {
            (640, 360)
        } else {
            (1280, 720)
        };
        let mut changed = Vec::new();
        let own_size = match &self.camera_views {
            Some(views) => views.get(&ShareId::Own).copied(),
            None => Some((640, 360)),
        };
        if let Some(camera) = &mut self.camera
            && let Some(own_size) = own_size
        {
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
                if let Some(picture) = VideoPicture::of_buffer(&frame, true, own_size) {
                    changed.push((ShareId::Own, Arc::new(picture)));
                }
                camera.seen = Some(frame);
            }
        }
        for camera in self.remote_cameras.values_mut() {
            let size = match &self.camera_views {
                Some(views) => match views.get(&ShareId::Of(camera.account)) {
                    Some(size) => *size,
                    None => continue,
                },
                None => size,
            };
            if camera.frames.has_changed().unwrap_or(false)
                && let Some(frame) = camera.frames.borrow_and_update().clone()
                && let Some(picture) = VideoPicture::of_frame(&frame, false, size)
            {
                changed.push((ShareId::Of(camera.account), Arc::new(picture)));
            }
        }
        if !changed.is_empty() {
            self.camera_pictures.send_modify(|p| p.extend(changed));
        }
    }
}

fn start_error(connected: bool, allowed: bool) -> Option<CameraError> {
    if !connected {
        Some(CameraError::NotConnected)
    } else {
        (!allowed).then_some(CameraError::Blocked)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    /// A camera that counts the reads of its permission and starts no capture.
    struct Counting {
        reads: AtomicUsize,
        access: CameraAccess,
    }
    impl crate::camera::CameraCapture for Counting {
        fn access(&self) -> CameraAccess {
            self.reads.fetch_add(1, Ordering::Relaxed);
            self.access
        }
        fn request_access(&self) -> crate::share::Pending<CameraAccess> {
            Box::pin(async { CameraAccess::Allowed })
        }
        fn devices(&self) -> Result<Vec<crate::camera::CameraDevice>, CameraError> {
            Ok(Vec::new())
        }
        fn start(
            &self,
            _: Option<String>,
            _: Arc<dyn CameraSink>,
        ) -> crate::share::Pending<Result<CaptureStream, CameraError>> {
            Box::pin(async { Err(CameraError::DeviceGone) })
        }
    }

    struct Ignored;
    impl CameraSink for Ignored {
        fn picture(&self, _: Arc<dyn VideoBuffer>) {}
        fn ended(&self, _: CameraError) {}
    }

    /// The Linux camera opens every device node to read its permission.
    #[tokio::test]
    async fn a_start_reads_the_camera_permission_once() {
        for (access, answer) in [
            (CameraAccess::Allowed, CameraError::DeviceGone),
            (CameraAccess::NotAsked, CameraError::DeviceGone),
            (
                CameraAccess::Refused,
                CameraError::NoPermission(CameraAccess::Refused),
            ),
        ] {
            let capture = Arc::new(Counting {
                reads: AtomicUsize::new(0),
                access,
            });
            let result = open(capture.clone(), None, Arc::new(Ignored)).await;
            assert_eq!(result.unwrap_err(), answer);
            assert_eq!(capture.reads.load(Ordering::Relaxed), 1, "{access:?}");
        }
    }
    /// A camera with a capture and a publication that the media server has not answered.
    fn publishing() -> OwnCamera {
        let (internal, _) = mpsc::unbounded_channel();
        let sink = Arc::new(Sink {
            generation: 1,
            internal,
            source: Mutex::new(None),
            last: Mutex::new(None),
            ended: AtomicBool::new(false),
            stops: Arc::new(Stops::default()),
            stop: 0,
        });
        OwnCamera {
            generation: 1,
            sink,
            started: Arc::new(Mutex::new(None)),
            stream: None,
            task: tokio::spawn(async {}),
            publication: Publication::Publishing,
            round: 1,
            returning: None,
            seen: None,
        }
    }

    #[tokio::test]
    async fn a_publication_that_ends_after_a_hold_is_not_the_publication_of_the_camera() {
        // The media server answers while the camera waits for it.
        let camera = publishing();
        assert_eq!(taken(Some(camera.round), 1, true), Taken::Kept);
        assert_eq!(taken(Some(camera.round), 1, false), Taken::Failed);
        // The seat is not admitted any more, or the connection is lost, before the answer.
        let mut camera = publishing();
        assert!(camera.unpublish().is_none());
        assert!(matches!(camera.publication, Publication::None));
        assert_eq!(
            taken(Some(camera.round), 1, true),
            Taken::Withdrawn,
            "a track from before the hold stays published at a seat that is not admitted"
        );
        // A failure of that publication does not end the camera.
        assert_eq!(taken(Some(camera.round), 1, false), Taken::Dropped);
        // The camera has ended.
        assert_eq!(taken(None, 1, true), Taken::Withdrawn);
        assert_eq!(taken(None, 1, false), Taken::Dropped);
    }

    #[test]
    fn a_camera_waiting_for_connection_does_not_claim_a_server_block() {
        assert_ne!(start_error(false, true), Some(CameraError::Blocked));
        assert!(start_error(false, true).is_some());
        assert_eq!(start_error(true, false), Some(CameraError::Blocked));
        assert_eq!(start_error(true, true), None);
    }
}
