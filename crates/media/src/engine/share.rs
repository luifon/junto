//! Shares in the media connection (ADR 0737): the own share with its picture track and its
//! sound track, the shares of the other people, the one that the person watches, and the
//! pictures for an interface.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures_util::StreamExt;
use junto_client_core::protocol::call::ShareAudio;
use junto_client_core::protocol::ids::AccountId;
use livekit::options::{AudioEncoding, DegradationPreference, TrackPublishOptions, VideoEncoding};
use livekit::prelude::{
    LocalAudioTrack, LocalTrack, LocalTrackPublication, LocalVideoTrack, RemoteAudioTrack,
    RemoteTrackPublication, RemoteVideoTrack, TrackSid, TrackSource,
};
use livekit::track::TrackDimension;
use livekit::webrtc::audio_frame::AudioFrame;
use livekit::webrtc::audio_source::native::NativeAudioSource;
use livekit::webrtc::audio_source::{AudioSourceOptions, RtcAudioSource};
use livekit::webrtc::audio_stream::native::NativeAudioStream;
use livekit::webrtc::video_frame::{BoxVideoFrame, VideoBuffer};
use livekit::webrtc::video_source::native::NativeVideoSource;
use livekit::webrtc::video_source::{RtcVideoSource, VideoResolution};
use livekit::webrtc::video_stream::native::NativeVideoStream;
use tokio::sync::{Notify, mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::order::in_order;
use super::{Actor, Internal, PUBLISH_LIMIT, PUBLISH_RETRY, Publication, Stops, account_of};
use crate::audio::playout::{MAX_GAIN, SourceHandle};
use crate::audio::{Format, PLAYOUT_CHANNELS, SAMPLE_RATE};
use crate::share::picture::VideoPicture;
use crate::share::sound::{SHARE_CHANNELS, SHARE_FRAME_SAMPLES, SoundChain, WAITING_FRAMES};
use crate::share::{
    CaptureError, CaptureRequest, CaptureSink, CaptureStream, ShareKind, read_track_name,
    track_name,
};

/// The number of the sound of a screen share among the track sources in the protocol of
/// the media server (`TrackSource.SCREEN_SHARE_AUDIO`).
const SHARE_SOUND_SOURCE: i32 = 4;
/// The bitrate of the sound of a share. It is higher than the one of a voice, because the
/// sound holds music and several voices (ADR 0502).
const SHARE_SOUND_BITRATE: u64 = 128_000;
/// While the capture gives no picture, the last one is sent again after this time, so
/// that a person who starts to watch gets a picture of a screen that does not change.
pub(super) const IDLE_RESEND: Duration = Duration::from_secs(1);
/// The size in pixels inside which a share is drawn where the person does not watch it:
/// the card of the bar of shares, at twice its design size.
pub const SMALL_PICTURE: (u32, u32) = (176, 110);
/// The size inside which the watched share is drawn until the interface tells its own.
pub const DEFAULT_VIEW: (u32, u32) = (1280, 800);
/// A small picture is made again after this time.
const SMALL_INTERVAL: Duration = Duration::from_secs(1);
/// The own share is drawn for the sharer at most this often.
const PREVIEW_INTERVAL: Duration = Duration::from_millis(66);

/// The picture of the share of another person is drawn at most this often: 30 times a
/// second.
const PICTURE_INTERVAL: Duration = Duration::from_millis(33);

/// Decides when a picture is drawn: at most once in an interval. A picture that arrives
/// earlier waits for the end of the interval, so the last picture of a share is always
/// drawn.
#[derive(Debug)]
pub(super) struct Pace {
    interval: Duration,
    drawn_at: Option<Instant>,
}

impl Pace {
    pub(super) fn new(interval: Duration) -> Self {
        Self {
            interval,
            drawn_at: None,
        }
    }

    /// When a picture that arrived now is drawn: now (nothing), or at the returned time.
    pub(super) fn wait(&self, now: Instant) -> Option<Instant> {
        self.drawn_at
            .map(|at| at + self.interval)
            .filter(|due| *due > now)
    }

    pub(super) fn drawn(&mut self, now: Instant) {
        self.drawn_at = Some(now);
    }
}

/// The bitrate of a picture of a share: 3 Mbit/s for 1920 × 1080 at 30 pictures per
/// second, in proportion to the pixels, and half of it in proportion to the frame rate.
fn picture_bitrate(size: (u32, u32), frame_rate: u32) -> u64 {
    let pixels = u64::from(size.0) * u64::from(size.1);
    let at_30 = 3_000_000 * pixels / (1920 * 1080);
    (at_30 * u64::from(30 + frame_rate.min(60)) / 60).clamp(150_000, 8_000_000)
}

/// The full-resolution encoding keeps the chosen cadence within the publication cap.
pub(super) fn picture_encoding(size: (u32, u32), frame_rate: u32) -> VideoEncoding {
    let frame_rate = frame_rate.clamp(1, 60);
    VideoEncoding {
        max_bitrate: picture_bitrate(size, frame_rate),
        max_framerate: f64::from(frame_rate),
    }
}

/// A lower simulcast layer would permit bandwidth adaptation to replace readable text.
pub(super) fn picture_options(size: (u32, u32), frame_rate: u32) -> TrackPublishOptions {
    super::sent::encoded(
        TrackPublishOptions {
            source: TrackSource::Screenshare,
            video_encoding: Some(picture_encoding(size, frame_rate)),
            simulcast: false,
            simulcast_layers: None,
            degradation_preference: Some(DegradationPreference::MaintainResolution),
            ..TrackPublishOptions::default()
        },
        size,
        frame_rate,
    )
}

/// A share in the call of this device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShareId {
    /// The share of this device.
    Own,
    Of(AccountId),
}

/// The share of this device as it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareInfo {
    pub kind: ShareKind,
    /// The sound that the sharer chose.
    pub sound: ShareAudio,
    /// The chosen sound is not sent: the server holds every sound source of the person
    /// closed (ADR 0510).
    pub sound_held: bool,
    /// The size of the picture in pixels.
    pub size: (u32, u32),
    pub frame_rate: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ShareState {
    #[default]
    Off,
    /// The capture starts, or the picture track is being published.
    Starting,
    Running(ShareInfo),
    /// The share ended by itself. The state stays until the person starts a share or
    /// puts the statement away.
    Ended(CaptureError),
}

/// The share of another person, as the media connection shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeenShare {
    pub account_id: AccountId,
    /// What the track states about itself. Absent for a track of another client.
    pub kind: Option<ShareKind>,
    pub frame_rate: Option<u32>,
    /// The size of the picture as it arrives. Absent until the first picture.
    pub size: Option<(u32, u32)>,
}

/// The picture of the share that the person watches, in the form of a camera picture: the
/// window draws it in the same way (ADR 0774).
#[derive(Debug, Clone)]
pub struct MainPicture {
    pub share: ShareId,
    pub frame: Arc<VideoPicture>,
}

/// Two pictures are the same when they are one picture of one share.
impl PartialEq for MainPicture {
    fn eq(&self, other: &Self) -> bool {
        self.share == other.share && Arc::ptr_eq(&self.frame, &other.frame)
    }
}

impl Eq for MainPicture {}

/// What the tasks that make pictures read: the watched share and the size of its drawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct View {
    pub watched: Option<ShareId>,
    pub size: (u32, u32),
}

/// The share that is watched: the choice of the person while that share runs; otherwise
/// the share of another person that was seen first; otherwise the own share.
pub fn watched(choice: Option<ShareId>, own: bool, others: &[AccountId]) -> Option<ShareId> {
    match choice {
        Some(ShareId::Own) if own => Some(ShareId::Own),
        Some(ShareId::Of(account_id)) if others.contains(&account_id) => {
            Some(ShareId::Of(account_id))
        }
        _ => others
            .first()
            .map(|account_id| ShareId::Of(*account_id))
            .or(own.then_some(ShareId::Own)),
    }
}

/// The channels to an interface.
#[derive(Clone)]
pub(super) struct Pictures {
    pub main: Arc<watch::Sender<Option<MainPicture>>>,
    pub small: Arc<watch::Sender<HashMap<ShareId, Arc<VideoPicture>>>>,
}

impl Pictures {
    /// Makes the pictures of one buffer of a share that are due now.
    fn show(
        &self,
        share: ShareId,
        buffer: &Arc<dyn VideoBuffer>,
        view: View,
        small_at: &mut Option<Instant>,
    ) {
        if view.watched == Some(share)
            && let Some(frame) = VideoPicture::of_buffer(buffer, false, view.size)
        {
            self.main.send_replace(Some(MainPicture {
                share,
                frame: Arc::new(frame),
            }));
        }
        if small_at.is_none_or(|at| at.elapsed() >= SMALL_INTERVAL)
            && let Some(frame) = VideoPicture::new(buffer.as_ref(), false, SMALL_PICTURE)
        {
            *small_at = Some(Instant::now());
            self.small.send_modify(|small| {
                small.insert(share, Arc::new(frame));
            });
        }
    }

    fn forget(&self, share: ShareId) {
        self.small
            .send_if_modified(|small| small.remove(&share).is_some());
        self.main.send_if_modified(|main| {
            let shown = main.as_ref().is_some_and(|main| main.share == share);
            if shown {
                *main = None;
            }
            shown
        });
    }
}

/// What the capture of the own share writes into, from the threads of the capture.
pub(super) struct ShareSink {
    generation: u64,
    internal: mpsc::UnboundedSender<Internal>,
    /// The source of the picture track. Absent until the capture has started.
    video: Mutex<Option<NativeVideoSource>>,
    /// The last picture and when it was given to the track.
    last: Mutex<Option<(Arc<dyn VideoBuffer>, Instant)>>,
    arrived: Notify,
    sound: Mutex<SoundChain>,
    ended: AtomicBool,
    stops: Arc<Stops>,
    /// The count of share stops when this share was asked for.
    stop: u64,
}

impl ShareSink {
    /// The share was ended by the capture, or stopped at the handle of the engine.
    fn over(&self) -> bool {
        self.ended.load(Ordering::Relaxed) || self.stops.share.load(Ordering::Relaxed) != self.stop
    }

    fn send(&self, buffer: &Arc<dyn VideoBuffer>) {
        let source = self
            .video
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(source) = source {
            source.capture_frame(&super::sent::frame(buffer.clone()));
        }
    }

    fn last(&self) -> Option<(Arc<dyn VideoBuffer>, Instant)> {
        self.last
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn keep(&self, buffer: Arc<dyn VideoBuffer>) {
        *self.last.lock().unwrap_or_else(PoisonError::into_inner) = Some((buffer, Instant::now()));
    }
}

impl CaptureSink for ShareSink {
    fn picture(&self, buffer: Arc<dyn VideoBuffer>) {
        if self.over() {
            return;
        }
        self.send(&buffer);
        self.keep(buffer);
        self.arrived.notify_one();
    }

    fn sound(&self, samples: &[f32], format: Format) {
        if self.over() {
            return;
        }
        self.sound
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .deliver(samples, format);
    }

    fn ended(&self, error: CaptureError) {
        if !self.ended.swap(true, Ordering::Relaxed) {
            let _ = self
                .internal
                .send(Internal::ShareEnded(self.generation, error));
        }
    }
}

/// Sends the last picture again while the capture gives none, and draws the own share for
/// the sharer.
async fn tend(sink: Arc<ShareSink>, pictures: Pictures, mut view: watch::Receiver<View>) {
    let mut tick = tokio::time::interval(IDLE_RESEND / 4);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut small_at: Option<Instant> = None;
    // What was drawn last: the picture, the view, and when.
    let mut drawn: Option<(Arc<dyn VideoBuffer>, View, Instant)> = None;
    loop {
        tokio::select! {
            () = sink.arrived.notified() => {}
            changed = view.changed() => {
                if changed.is_err() {
                    return;
                }
            }
            _ = tick.tick() => {
                if !sink.over()
                    && let Some((buffer, at)) = sink.last()
                    && at.elapsed() >= IDLE_RESEND
                {
                    sink.send(&buffer);
                    sink.keep(buffer);
                }
            }
        }
        if sink.over() {
            return;
        }
        let Some((buffer, _)) = sink.last() else {
            continue;
        };
        let now = *view.borrow_and_update();
        // A picture is drawn once. A new picture is drawn at the rate of the preview, and
        // a change of what is watched, or of its size, at once.
        let due = match &drawn {
            Some((shown, seen, at)) => {
                *seen != now || (!Arc::ptr_eq(shown, &buffer) && at.elapsed() >= PREVIEW_INTERVAL)
            }
            None => true,
        };
        if !due {
            continue;
        }
        pictures.show(ShareId::Own, &buffer, now, &mut small_at);
        drawn = Some((buffer, now, Instant::now()));
    }
}

/// Gives the frames of the sound of the share to the audio source of its track.
async fn pump_sound(
    mut frames: mpsc::Receiver<Vec<i16>>,
    source: Arc<Mutex<Option<NativeAudioSource>>>,
) {
    while let Some(samples) = frames.recv().await {
        let current = source
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let Some(current) = current else {
            continue;
        };
        debug_assert_eq!(samples.len(), SHARE_FRAME_SAMPLES);
        let frame = AudioFrame {
            data: samples.into(),
            sample_rate: SAMPLE_RATE,
            num_channels: SHARE_CHANNELS as u32,
            samples_per_channel: (SHARE_FRAME_SAMPLES / SHARE_CHANNELS) as u32,
        };
        if let Err(error) = current.capture_frame(&frame).await {
            tracing::debug!(error = %error.message, "a frame of the share sound was not taken");
        }
    }
}

/// The share of this device.
pub(super) struct OwnShare {
    generation: u64,
    request: CaptureRequest,
    sink: Arc<ShareSink>,
    /// The capture. Absent while it starts.
    stream: Option<CaptureStream>,
    picture: Publication,
    sound_source: Arc<Mutex<Option<NativeAudioSource>>>,
    sound: Publication,
    sound_generation: u64,
    /// The call library withdrew the track and publishes it again: it connects anew. The
    /// number names the wait, which has a bound.
    picture_returns: Option<(u64, TrackSid)>,
    sound_returns: Option<(u64, TrackSid)>,
    waits: u64,
    /// The answer of the system to the start of the capture, until the engine takes it.
    /// The share owns it: a capture that has started when the share ends is stopped with
    /// the share.
    started: Started,
    /// The work of the share: the start of its capture, the sound on its way to the track,
    /// and the pictures. It ends with the share.
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for OwnShare {
    fn drop(&mut self) {
        self.sink.ended.store(true, Ordering::Relaxed);
        for task in &self.tasks {
            task.abort();
        }
        // A capture that started and that the engine has not taken ends here. When the
        // work that starts it is past its last wait, it stores the capture after this,
        // and the capture ends when that work ends.
        drop(
            self.started
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take(),
        );
    }
}

type Started = Arc<Mutex<Option<Result<CaptureStream, CaptureError>>>>;

/// Whether a track that the call library published again is the track of a place: the
/// place waits for the track with that identifier, or holds it.
pub(super) fn is_return<S: PartialEq>(
    waits_for: Option<&S>,
    holds: Option<&S>,
    previous: &S,
) -> bool {
    waits_for == Some(previous) || holds == Some(previous)
}

/// The picture of the share of another person.
pub(super) struct RemotePicture {
    account_id: AccountId,
    publication: RemoteTrackPublication,
    kind: Option<ShareKind>,
    frame_rate: Option<u32>,
    size: Option<(u32, u32)>,
    /// The size of the drawing that the media server was asked for last.
    asked: Option<(u32, u32)>,
    /// The order in which the shares were seen.
    order: u64,
    task: JoinHandle<()>,
}

impl RemotePicture {
    /// Asks the media server for the layer of a drawn size, when the size is another one
    /// than the last. The requests of all shares reach the media server in the order in
    /// which they are made here.
    fn ask(&mut self, size: (u32, u32)) {
        if self.asked == Some(size) {
            return;
        }
        self.asked = Some(size);
        let publication = self.publication.clone();
        in_order(move || publication.update_video_dimensions(TrackDimension(size.0, size.1)));
    }
}

impl Drop for RemotePicture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// The sound of the share of another person, while it is received.
pub(super) struct RemoteSound {
    account_id: AccountId,
    /// The sound in the mix. Its volume is set here, also while no frame arrives.
    source: Arc<SourceHandle>,
    task: JoinHandle<()>,
}

/// A track of a share of this device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ShareTrack {
    Picture,
    Sound,
}

impl Drop for RemoteSound {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn until(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// Reads the pictures of the share of another person, and draws at most 30 of them in a
/// second. The last picture is drawn again when the person starts to watch the share or
/// the size of its drawing changes: a screen that does not change sends a picture only
/// once a second.
async fn watch_pictures(
    mut stream: impl futures_util::Stream<Item = BoxVideoFrame> + Unpin,
    sid: TrackSid,
    share: ShareId,
    pictures: Pictures,
    mut view: watch::Receiver<View>,
    internal: mpsc::UnboundedSender<Internal>,
) {
    let mut small_at = None;
    let mut size = None;
    // Shared, so that a picture at the size of the drawing holds it without a copy.
    let mut last: Option<Arc<dyn VideoBuffer>> = None;
    let mut pace = Pace::new(PICTURE_INTERVAL);
    // A picture that arrived inside the interval of the one before is drawn at this time.
    let mut waits: Option<Instant> = None;
    loop {
        tokio::select! {
            frame = stream.next() => {
                let Some(frame) = frame else {
                    return;
                };
                let now = (frame.buffer.width(), frame.buffer.height());
                if size != Some(now) {
                    size = Some(now);
                    let _ = internal.send(Internal::SharePictureSize(sid.clone(), now));
                }
                last = Some(Arc::from(frame.buffer));
                waits = pace.wait(Instant::now());
                if waits.is_some() {
                    continue;
                }
            }
            changed = view.changed() => {
                if changed.is_err() {
                    return;
                }
            }
            () = until(waits) => {}
        }
        if let Some(buffer) = &last {
            let now = *view.borrow_and_update();
            pictures.show(share, buffer, now, &mut small_at);
            pace.drawn(Instant::now());
            waits = None;
        }
    }
}

/// Carries the sound of a share into the mix until its track ends.
async fn play_sound(mut stream: NativeAudioStream, source: Arc<SourceHandle>) {
    while let Some(frame) = stream.next().await {
        source.push(&frame.data);
    }
}

impl Actor {
    /// The state of the own share as the caller reads it.
    fn share_state(&self) -> ShareState {
        let Some(share) = &self.share else {
            return self.state.borrow().share.clone();
        };
        match (&share.stream, &share.picture) {
            (Some(stream), Publication::Published(_)) => {
                let sound = share.request.sound.reported();
                ShareState::Running(ShareInfo {
                    kind: share.request.picture.kind(),
                    sound,
                    sound_held: sound != ShareAudio::None
                        && !matches!(share.sound, Publication::Published(_)),
                    size: (stream.width, stream.height),
                    frame_rate: share.request.quality.frame_rate,
                })
            }
            _ => ShareState::Starting,
        }
    }

    /// Tells the state of the shares: the own one, the ones of the other people in the
    /// order in which they were seen, and the one that is watched.
    pub(super) fn tell_shares(&mut self) {
        let mut seen: Vec<&RemotePicture> = self.remote_pictures.values().collect();
        seen.sort_by_key(|picture| picture.order);
        let shares: Vec<SeenShare> = seen
            .iter()
            .map(|picture| SeenShare {
                account_id: picture.account_id,
                kind: picture.kind,
                frame_rate: picture.frame_rate,
                size: picture.size,
            })
            .collect();
        let others: Vec<AccountId> = shares.iter().map(|share| share.account_id).collect();
        let own = self
            .share
            .as_ref()
            .is_some_and(|share| share.stream.is_some());
        let now = watched(self.watch_choice, own, &others);
        let view = View {
            watched: now,
            size: self.view_size,
        };
        let changed = self.view.send_if_modified(|held| {
            let changed = *held != view;
            *held = view;
            changed
        });
        if changed {
            self.pictures.main.send_if_modified(|main| {
                let other = main.as_ref().is_some_and(|main| Some(main.share) != now);
                if other {
                    *main = None;
                }
                other
            });
            self.sync_sizes(now);
            self.sync_volumes(now);
        }
        let share = self.share_state();
        self.change(|state| {
            state.share = share;
            state.shares = shares;
            state.watched = now;
        });
    }

    /// Asks the media server for the picture of each share at the size of its drawing: the
    /// watched share at the size of the interface, every other share at the size of its
    /// small picture.
    fn sync_sizes(&mut self, watched: Option<ShareId>) {
        let view_size = self.view_size;
        for picture in self.remote_pictures.values_mut() {
            let is_watched = watched == Some(ShareId::Of(picture.account_id));
            picture.ask(if is_watched { view_size } else { SMALL_PICTURE });
        }
    }

    /// Gives the sound of the watched share its volume. The sound of every other share is
    /// received and has no volume in the mix: a change of what the media server sends
    /// would have to be kept in order with the changes before it, and a share sound is a
    /// small part of what a share costs.
    fn sync_volumes(&self, watched: Option<ShareId>) {
        for sound in self.remote_sounds.values() {
            let is_watched = watched == Some(ShareId::Of(sound.account_id));
            sound
                .source
                .set_gain(if is_watched { self.share_volume } else { 0.0 });
        }
    }

    pub(super) fn set_watch(&mut self, choice: Option<ShareId>) {
        self.watch_choice = choice;
        self.tell_shares();
    }

    pub(super) fn set_view_size(&mut self, width: u32, height: u32) {
        let size = (width.clamp(16, 7680), height.clamp(16, 4320));
        if size != self.view_size {
            self.view_size = size;
            self.tell_shares();
        }
    }

    pub(super) fn set_share_volume(&mut self, gain: f32) {
        self.share_volume = gain.clamp(0.0, MAX_GAIN);
        self.sync_volumes(self.view.borrow().watched);
    }

    /// The picture track of the share of another person arrived.
    pub(super) fn remote_picture(
        &mut self,
        track: &RemoteVideoTrack,
        publication: RemoteTrackPublication,
        identity: &str,
    ) {
        // A participant without an account, such as the recorder, shares nothing.
        let Some(account_id) = account_of(identity) else {
            return;
        };
        let sid = publication.sid();
        let stated = read_track_name(&publication.name());
        self.share_order += 1;
        let task = self.runtime.spawn(watch_pictures(
            NativeVideoStream::new(track.rtc_track()),
            sid.clone(),
            ShareId::Of(account_id),
            self.pictures.clone(),
            self.view.subscribe(),
            self.internal.clone(),
        ));
        self.remote_pictures.insert(
            sid,
            RemotePicture {
                account_id,
                publication,
                kind: stated.map(|(kind, _)| kind),
                frame_rate: stated.map(|(_, rate)| rate),
                size: None,
                asked: None,
                order: self.share_order,
                task,
            },
        );
        self.tell_shares();
        let watched = self.view.borrow().watched;
        self.sync_sizes(watched);
    }

    /// The sound track of the share of another person is received.
    pub(super) fn remote_sound(
        &mut self,
        track: &RemoteAudioTrack,
        publication: RemoteTrackPublication,
        identity: &str,
    ) {
        let Some(account_id) = account_of(identity) else {
            return;
        };
        let sid = publication.sid();
        let source = Arc::new(self.mixer.add(0.0));
        let stream = NativeAudioStream::new(
            track.rtc_track(),
            SAMPLE_RATE as i32,
            PLAYOUT_CHANNELS as i32,
        );
        let task = self.runtime.spawn(play_sound(stream, source.clone()));
        self.remote_sounds.insert(
            sid,
            RemoteSound {
                account_id,
                source,
                task,
            },
        );
        self.sync_volumes(self.view.borrow().watched);
    }

    /// A track of another person ended. Returns whether it belonged to a share.
    pub(super) fn remote_track_ended(&mut self, sid: &TrackSid) -> bool {
        let sound = self.remote_sounds.remove(sid).is_some();
        let Some(picture) = self.remote_pictures.remove(sid) else {
            return sound;
        };
        let share = ShareId::Of(picture.account_id);
        drop(picture);
        // Another track of the same person can show the same share: the person started a
        // new one before the end of the earlier one was told.
        let again = self
            .remote_pictures
            .values()
            .any(|other| ShareId::Of(other.account_id) == share);
        if !again {
            self.pictures.forget(share);
        }
        self.tell_shares();
        true
    }

    pub(super) fn picture_size(&mut self, sid: &TrackSid, size: (u32, u32)) {
        if let Some(picture) = self.remote_pictures.get_mut(sid)
            && picture.size != Some(size)
        {
            picture.size = Some(size);
            self.tell_shares();
        }
    }

    /// Forgets every share of another person: the room was left.
    pub(super) fn forget_remote_shares(&mut self) {
        let shares: Vec<ShareId> = self
            .remote_pictures
            .values()
            .map(|picture| ShareId::Of(picture.account_id))
            .collect();
        self.remote_pictures.clear();
        self.remote_sounds.clear();
        for share in shares {
            self.pictures.forget(share);
        }
    }

    /// Starts the share of this device. A share that runs is replaced. `stop`: the count
    /// of share stops when the start was asked for; after a later stop the share sends
    /// nothing.
    pub(super) fn start_share(&mut self, request: CaptureRequest, stop: u64) {
        self.end_share(None);
        if !self.joined() {
            self.change(|state| state.share = ShareState::Ended(CaptureError::Interrupted));
            return;
        }
        self.share_generation += 1;
        let generation = self.share_generation;
        let (frames, taken) = mpsc::channel(WAITING_FRAMES);
        let sink = Arc::new(ShareSink {
            generation,
            internal: self.internal.clone(),
            video: Mutex::new(None),
            last: Mutex::new(None),
            arrived: Notify::new(),
            sound: Mutex::new(SoundChain::new(frames)),
            ended: AtomicBool::new(false),
            stops: self.stops.clone(),
            stop,
        });
        let sound_source: Arc<Mutex<Option<NativeAudioSource>>> = Arc::default();
        let starting = self.options.capture.start(request.clone(), sink.clone());
        let internal = self.internal.clone();
        let started: Started = Arc::default();
        let slot = started.clone();
        let tasks = vec![
            self.runtime.spawn(pump_sound(taken, sound_source.clone())),
            self.runtime.spawn(tend(
                sink.clone(),
                self.pictures.clone(),
                self.view.subscribe(),
            )),
            // The start of the capture belongs to the share: a share that the person
            // stops while the system still prepares the capture ends this work, and what
            // the system had started is stopped with it. A capture that has started when
            // nobody takes it any more is dropped with its message.
            self.runtime.spawn(async move {
                let answer = starting.await;
                *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(answer);
                let _ = internal.send(Internal::ShareStarted(generation));
            }),
        ];
        self.share = Some(OwnShare {
            generation,
            request,
            sink,
            stream: None,
            picture: Publication::None,
            sound_source,
            sound: Publication::None,
            sound_generation: 0,
            picture_returns: None,
            sound_returns: None,
            waits: 0,
            started,
            tasks,
        });
        self.tell_shares();
    }

    /// Ends the share of this device. `error`: the share ended by itself, and the person
    /// is told why.
    pub(super) fn end_share(&mut self, error: Option<CaptureError>) {
        let Some(mut share) = self.share.take() else {
            if error.is_none() {
                self.change(|state| state.share = ShareState::Off);
            }
            return;
        };
        self.share_generation += 1;
        for publication in [
            std::mem::replace(&mut share.picture, Publication::None),
            std::mem::replace(&mut share.sound, Publication::None),
        ] {
            if let Publication::Published(publication) = publication {
                self.withdraw(publication);
            }
        }
        drop(share);
        self.pictures.forget(ShareId::Own);
        self.change(|state| {
            state.share = match error {
                Some(error) => ShareState::Ended(error),
                None => ShareState::Off,
            };
        });
        self.tell_shares();
    }

    pub(super) fn share_started(&mut self, generation: u64) {
        let started = self
            .share
            .as_ref()
            .filter(|share| share.generation == generation && share.stream.is_none())
            .and_then(|share| {
                share
                    .started
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take()
            });
        let Some(started) = started else {
            return;
        };
        let stream = match started {
            Ok(stream) => stream,
            Err(error) => {
                tracing::warn!(%error, "the capture of the share did not start");
                self.end_share(Some(error));
                return;
            }
        };
        let (Some(room), Some(share)) = (&self.room, &mut self.share) else {
            self.end_share(Some(CaptureError::Interrupted));
            return;
        };
        let source = NativeVideoSource::new(
            VideoResolution {
                width: stream.width,
                height: stream.height,
            },
            true,
        );
        *share
            .sink
            .video
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(source.clone());
        // A picture that arrived before the source existed is the first one of the track.
        if let Some((buffer, _)) = share.sink.last() {
            share.sink.send(&buffer);
        }
        let name = track_name(
            share.request.picture.kind(),
            share.request.quality.frame_rate,
        );
        let track = LocalVideoTrack::create_video_track(&name, RtcVideoSource::Native(source));
        let options = picture_options(
            (stream.width, stream.height),
            share.request.quality.frame_rate,
        );
        share.stream = Some(stream);
        share.picture = Publication::Publishing;
        let (participant, internal) = (room.local_participant(), self.internal.clone());
        self.runtime.spawn(async move {
            let publishing = participant.publish_track(LocalTrack::Video(track), options);
            let result = match tokio::time::timeout(PUBLISH_LIMIT, publishing).await {
                Ok(Ok(publication)) => Ok(publication),
                Ok(Err(error)) => Err(error.to_string()),
                Err(_) => Err("no answer of the media server".to_owned()),
            };
            let _ = internal.send(Internal::SharePicturePublished(generation, result));
        });
        self.sync_share_sound();
        self.tell_shares();
    }

    pub(super) fn share_picture_published(
        &mut self,
        generation: u64,
        result: Result<LocalTrackPublication, String>,
    ) {
        let current = self.share.as_mut().filter(|share| {
            share.generation == generation && matches!(share.picture, Publication::Publishing)
        });
        match (current, result) {
            (Some(share), Ok(publication)) => {
                share.picture = Publication::Published(publication);
                self.tell_shares();
            }
            (Some(_), Err(error)) => {
                tracing::warn!(%error, "the picture of the share was not published");
                self.end_share(Some(CaptureError::Failed(error)));
            }
            // The share ended while its track was published.
            (None, Ok(publication)) => self.withdraw(publication),
            (None, Err(_)) => {}
        }
    }

    /// A track of this device was withdrawn, and this device did not ask for it. The call
    /// library does that when it connects anew, and publishes the same track again; the
    /// media server does it when the person may not send the track any more. The share
    /// waits for the track for a bounded time. Returns whether it was a track of the share.
    pub(super) fn share_track_taken(&mut self, sid: &TrackSid) -> bool {
        let Some(share) = &mut self.share else {
            return false;
        };
        let is = |publication: &Publication| matches!(publication, Publication::Published(held) if held.sid() == *sid);
        let track = if is(&share.picture) {
            ShareTrack::Picture
        } else if is(&share.sound) {
            ShareTrack::Sound
        } else {
            return false;
        };
        share.waits += 1;
        let wait = share.waits;
        match track {
            ShareTrack::Picture => {
                share.picture = Publication::Publishing;
                share.picture_returns = Some((wait, sid.clone()));
            }
            ShareTrack::Sound => {
                share.sound = Publication::Publishing;
                share.sound_returns = Some((wait, sid.clone()));
            }
        }
        let (generation, internal) = (share.generation, self.internal.clone());
        self.runtime.spawn(async move {
            tokio::time::sleep(PUBLISH_LIMIT).await;
            let _ = internal.send(Internal::ShareTrackDue(generation, wait, track));
        });
        self.tell_shares();
        true
    }

    /// The call library published a track of this device again under another identifier.
    /// A track that the share waits for is its track again. A track that no share uses
    /// is withdrawn: the share was stopped, or it has published a new track meanwhile.
    pub(super) fn share_track_republished(
        &mut self,
        previous: &TrackSid,
        publication: LocalTrackPublication,
    ) {
        let track = match publication.source() {
            TrackSource::Screenshare => ShareTrack::Picture,
            TrackSource::ScreenshareAudio => ShareTrack::Sound,
            _ => return,
        };
        let Some(share) = &mut self.share else {
            self.withdraw(publication);
            return;
        };
        let (held, returns) = match track {
            ShareTrack::Picture => (&mut share.picture, &mut share.picture_returns),
            ShareTrack::Sound => (&mut share.sound, &mut share.sound_returns),
        };
        // Only the track that this share sent returns to it. A track of a share before
        // this one, which the call library held when it connected anew, is not adopted.
        let waits_for = returns.as_ref().map(|(_, sid)| sid);
        let holds = match &*held {
            Publication::Published(before) => Some(before.sid()),
            Publication::None | Publication::Publishing => None,
        };
        if is_return(waits_for, holds.as_ref(), previous) {
            *held = Publication::Published(publication);
            *returns = None;
            // The permission can have changed while the track was away.
            self.sync_share_sound();
        } else {
            self.withdraw(publication);
        }
    }

    /// The time for a withdrawn track to return has passed.
    pub(super) fn share_track_due(&mut self, generation: u64, wait: u64, track: ShareTrack) {
        let Some(share) = self
            .share
            .as_mut()
            .filter(|share| share.generation == generation)
        else {
            return;
        };
        match track {
            ShareTrack::Picture
                if share.picture_returns.as_ref().map(|(held, _)| *held) == Some(wait) =>
            {
                share.picture = Publication::None;
                self.end_share(Some(CaptureError::Failed(
                    "the media server ended the picture of the share".to_owned(),
                )));
            }
            ShareTrack::Sound
                if share.sound_returns.as_ref().map(|(held, _)| *held) == Some(wait) =>
            {
                // The track is published anew when the person may send it.
                share.sound_returns = None;
                share.sound = Publication::None;
                *share
                    .sound_source
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = None;
                self.sync_share_sound();
            }
            _ => {}
        }
    }

    /// Publishes or withdraws the sound track of the share by what holds now: it is
    /// published while the share runs with a sound and the server lets the person send it.
    pub(super) fn sync_share_sound(&mut self) {
        let may_send = self.may_send(SHARE_SOUND_SOURCE) && !self.controls.microphone_blocked;
        let Some(share) = &mut self.share else {
            return;
        };
        let wanted = share.stream.is_some()
            && share.request.sound.reported() != ShareAudio::None
            && may_send;
        match (&share.sound, wanted) {
            (Publication::None, true) => {
                let Some(room) = &self.room else {
                    return;
                };
                share.sound_generation += 1;
                let key = (share.generation, share.sound_generation);
                share.sound = Publication::Publishing;
                let source = NativeAudioSource::new(
                    AudioSourceOptions {
                        echo_cancellation: false,
                        noise_suppression: false,
                        auto_gain_control: false,
                    },
                    SAMPLE_RATE,
                    SHARE_CHANNELS as u32,
                    0,
                );
                *share
                    .sound_source
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = Some(source.clone());
                let track = LocalAudioTrack::create_audio_track(
                    "share-sound",
                    RtcAudioSource::Native(source),
                );
                let options = TrackPublishOptions {
                    source: TrackSource::ScreenshareAudio,
                    audio_encoding: Some(AudioEncoding {
                        max_bitrate: SHARE_SOUND_BITRATE,
                    }),
                    // The sound of a share is music as often as speech: it is sent without
                    // gaps in silence and without a second copy at a low bitrate.
                    dtx: false,
                    red: false,
                    ..TrackPublishOptions::default()
                };
                let (participant, internal) = (room.local_participant(), self.internal.clone());
                self.runtime.spawn(async move {
                    let publishing = participant.publish_track(LocalTrack::Audio(track), options);
                    let result = match tokio::time::timeout(PUBLISH_LIMIT, publishing).await {
                        Ok(Ok(publication)) => Ok(publication),
                        Ok(Err(error)) => Err(error.to_string()),
                        Err(_) => Err("no answer of the media server".to_owned()),
                    };
                    let _ = internal.send(Internal::ShareSoundPublished(key, result));
                });
            }
            (Publication::Published(_) | Publication::Publishing, false) => {
                share.sound_generation += 1;
                share.sound_returns = None;
                *share
                    .sound_source
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = None;
                if let Publication::Published(publication) =
                    std::mem::replace(&mut share.sound, Publication::None)
                {
                    self.withdraw(publication);
                }
            }
            _ => {}
        }
        self.tell_shares();
    }

    pub(super) fn share_sound_published(
        &mut self,
        key: (u64, u64),
        result: Result<LocalTrackPublication, String>,
    ) {
        let current = self.share.as_mut().filter(|share| {
            (share.generation, share.sound_generation) == key
                && matches!(share.sound, Publication::Publishing)
        });
        match (current, result) {
            (Some(share), Ok(publication)) => {
                share.sound = Publication::Published(publication);
                self.tell_shares();
            }
            (Some(share), Err(error)) => {
                tracing::info!(%error, "the sound of the share was not published");
                share.sound = Publication::None;
                self.retry_share_sound_later();
            }
            (None, Ok(publication)) => self.withdraw(publication),
            (None, Err(_)) => {}
        }
    }

    fn retry_share_sound_later(&self) {
        let Some(share) = &self.share else {
            return;
        };
        let (generation, internal) = (share.generation, self.internal.clone());
        self.runtime.spawn(async move {
            tokio::time::sleep(PUBLISH_RETRY).await;
            let _ = internal.send(Internal::ShareSoundRetry(generation));
        });
    }

    pub(super) fn share_sound_retry(&mut self, generation: u64) {
        if self
            .share
            .as_ref()
            .is_some_and(|share| share.generation == generation)
        {
            self.sync_share_sound();
        }
    }

    pub(super) fn share_ended(&mut self, generation: u64, error: CaptureError) {
        if self
            .share
            .as_ref()
            .is_some_and(|share| share.generation == generation)
        {
            tracing::info!(%error, "the capture of the share ended");
            self.end_share(Some(error));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_watched_share_is_the_choice_while_it_runs_then_the_first_other_then_the_own() {
        let (ann, ben) = (AccountId::generate(), AccountId::generate());
        // Nothing is shared.
        assert_eq!(watched(None, false, &[]), None);
        assert_eq!(watched(Some(ShareId::Own), false, &[]), None);
        // The own share alone is the preview of the sharer.
        assert_eq!(watched(None, true, &[]), Some(ShareId::Own));
        // Without a choice: the share of another person that was seen first.
        assert_eq!(watched(None, true, &[ann, ben]), Some(ShareId::Of(ann)));
        // The choice holds while its share runs.
        assert_eq!(
            watched(Some(ShareId::Of(ben)), true, &[ann, ben]),
            Some(ShareId::Of(ben))
        );
        assert_eq!(
            watched(Some(ShareId::Own), true, &[ann, ben]),
            Some(ShareId::Own)
        );
        // A choice whose share has ended gives way to the rule without a choice.
        assert_eq!(
            watched(Some(ShareId::Of(ben)), false, &[ann]),
            Some(ShareId::Of(ann))
        );
        assert_eq!(
            watched(Some(ShareId::Of(ben)), true, &[]),
            Some(ShareId::Own)
        );
        assert_eq!(
            watched(Some(ShareId::Own), false, &[ben]),
            Some(ShareId::Of(ben))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_picture_is_drawn_at_most_once_in_an_interval_and_the_last_one_always() {
        let interval = Duration::from_millis(33);
        let mut pace = Pace::new(interval);
        let start = Instant::now();
        // The first picture is drawn at once.
        assert_eq!(pace.wait(start), None);
        pace.drawn(start);
        // A share that sends 60 pictures in a second: the next one arrives after 16 ms and
        // waits for the end of the interval; it is drawn then, not dropped.
        let early = start + Duration::from_millis(16);
        assert_eq!(pace.wait(early), Some(start + interval));
        assert_eq!(pace.wait(start + interval), None);
        pace.drawn(start + interval);
        // A picture after a pause is drawn at once.
        assert_eq!(pace.wait(start + Duration::from_secs(1)), None);
    }

    #[test]
    fn the_picture_track_is_encoded_at_the_chosen_frame_rate_and_bitrate_cap() {
        let main = picture_encoding((1920, 1080), 30);
        assert_eq!((main.max_bitrate, main.max_framerate), (3_000_000, 30.0));
        let main = picture_encoding((800, 600), 30);
        assert_eq!(main.max_framerate, 30.0);
        assert!(
            (500_000..1_000_000).contains(&main.max_bitrate),
            "{}",
            main.max_bitrate
        );
        assert_eq!(picture_encoding((1280, 720), 15).max_framerate, 15.0);
        let main = picture_encoding((1280, 720), 60);
        assert_eq!(main.max_framerate, 60.0);
        assert!(main.max_bitrate > picture_encoding((1280, 720), 30).max_bitrate);
        assert_eq!(picture_encoding((7680, 4320), 60).max_bitrate, 8_000_000);
    }

    #[test]
    fn a_busy_processor_lowers_the_frame_rate_of_a_share_and_keeps_its_size() {
        let options = picture_options((1920, 1080), 30);
        assert_eq!(options.source, TrackSource::Screenshare);
        // The client states the rule itself and does not use the default of the call
        // library for a screen share.
        assert_eq!(
            options.degradation_preference,
            Some(DegradationPreference::MaintainResolution)
        );
        assert!(!options.simulcast);
        assert!(options.simulcast_layers.is_none());
    }

    #[test]
    fn a_share_has_no_lower_resolution_layer_when_bandwidth_falls() {
        for size in [(800, 600), (1280, 720), (1920, 1080)] {
            let encodings = livekit::options::compute_video_encodings(
                size.0,
                size.1,
                &picture_options(size, 30),
            );
            assert_eq!(encodings.len(), 1);
            assert_eq!(encodings[0].scale_resolution_down_by, Some(1.0));
        }
    }

    #[test]
    fn only_the_track_that_a_place_sent_returns_to_it() {
        let (own, other) = ("TR_own", "TR_other");
        // The place waits for its track, or still holds it.
        assert!(is_return(Some(&own), None, &own));
        assert!(is_return(None, Some(&own), &own));
        // The track of a share before this one is not the track of this share, also
        // while this share waits for its own track.
        assert!(!is_return(Some(&own), None, &other));
        assert!(!is_return(None, Some(&own), &other));
        assert!(!is_return(None::<&&str>, None, &own));
    }

    /// Pictures of one colour that arrive at a rate, and then stop without an end.
    fn pictures_at(
        rate: u64,
        count: u64,
    ) -> impl futures_util::Stream<Item = BoxVideoFrame> + Unpin {
        use livekit::webrtc::video_frame::{I420Buffer, VideoFrame, VideoRotation};
        Box::pin(futures_util::stream::unfold(0u64, move |sent| async move {
            if sent == count {
                return std::future::pending().await;
            }
            tokio::time::sleep(Duration::from_micros(1_000_000 / rate)).await;
            let mut buffer = I420Buffer::new(64, 36);
            // A grey whose brightness tells which picture this is.
            let (y, u, v) = buffer.data_mut();
            y.fill(40 + sent as u8);
            u.fill(128);
            v.fill(128);
            let buffer: Box<dyn VideoBuffer> = Box::new(buffer);
            Some((
                VideoFrame::new(VideoRotation::VideoRotation0, buffer),
                sent + 1,
            ))
        }))
    }

    /// A received picture in planes at the size of the drawing is held as it arrived; a
    /// smaller drawing makes planes of its size.
    #[test]
    fn a_picture_at_the_size_of_the_drawing_holds_the_received_planes() {
        let share = ShareId::Of(AccountId::generate());
        let pictures = Pictures {
            main: Arc::new(watch::Sender::new(None)),
            small: Arc::new(watch::Sender::new(HashMap::new())),
        };
        let buffer: Arc<dyn VideoBuffer> = Arc::new(crate::share::synthetic::two_colours(
            64,
            36,
            (200, 30, 30),
            (30, 30, 200),
        ));
        let received = buffer.as_i420().map(|planes| planes.data().0.as_ptr());
        let mut small_at = None;
        let view = |size| View {
            watched: Some(share),
            size,
        };
        pictures.show(share, &buffer, view((64, 36)), &mut small_at);
        let shown = pictures.main.borrow().clone().unwrap();
        assert_eq!(shown.frame.brightness(), received);
        pictures.show(share, &buffer, view((32, 18)), &mut small_at);
        let shown = pictures.main.borrow().clone().unwrap();
        assert_eq!((shown.frame.width, shown.frame.height), (32, 18));
        assert_ne!(shown.frame.brightness(), received);
    }

    #[tokio::test(start_paused = true)]
    async fn a_share_of_60_pictures_per_second_is_drawn_30_times_and_its_last_picture_always() {
        let account_id = AccountId::generate();
        let share = ShareId::Of(account_id);
        let pictures = Pictures {
            main: Arc::new(watch::Sender::new(None)),
            small: Arc::new(watch::Sender::new(HashMap::new())),
        };
        let view = watch::Sender::new(View {
            watched: Some(share),
            size: (64, 36),
        });
        let (internal, mut told) = mpsc::unbounded_channel();
        let sid = TrackSid::try_from("TR_test".to_owned()).unwrap();
        let task = tokio::spawn(watch_pictures(
            pictures_at(60, 61),
            sid,
            share,
            pictures.clone(),
            view.subscribe(),
            internal,
        ));
        // Counts each picture that an interface would draw.
        let mut main = pictures.main.subscribe();
        let counting = tokio::spawn(async move {
            let (mut drawn, mut last) = (0u32, None);
            while main.changed().await.is_ok() {
                if let Some(picture) = main.borrow_and_update().clone() {
                    drawn += 1;
                    last = picture.frame.pixel(0, 0);
                }
                if drawn > 1_000 {
                    break;
                }
            }
            (drawn, last)
        });
        // 61 pictures in about one second, then two seconds without a picture.
        tokio::time::sleep(Duration::from_secs(3)).await;
        task.abort();
        drop(pictures);
        let (drawn, last) = counting.await.unwrap();
        assert!((28..=32).contains(&drawn), "{drawn}");
        // The last picture has the brightness 100 of video, which is the grey 98 of a
        // screen: it was drawn, although it arrived inside the interval of the picture
        // before it. The picture before it has the grey 97.
        let (red, green, blue) = last.unwrap();
        assert!((red, green, blue) == (98, 98, 98), "{last:?}");
        assert!(matches!(
            told.try_recv(),
            Ok(Internal::SharePictureSize(_, (64, 36)))
        ));
        // A small picture exists too.
        drop(view);
    }
}
