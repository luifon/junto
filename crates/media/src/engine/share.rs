//! Shares in the media connection (ADR 0737): the own shares, each with its picture track
//! and its sound track, the shares of the other people, the one that the person watches,
//! and the pictures for an interface.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures_util::StreamExt;
use junto_client_core::protocol::call::{MAX_SHARES, ShareAudio};
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
use uuid::Uuid;

use super::order::in_order;
use super::{Actor, Internal, PUBLISH_LIMIT, PUBLISH_RETRY, Publication, Stops, account_of};
use crate::audio::playout::{MAX_GAIN, SourceHandle};
use crate::audio::{Format, PLAYOUT_CHANNELS, SAMPLE_RATE};
use crate::share::picture::VideoPicture;
use crate::share::sound::{SHARE_CHANNELS, SHARE_FRAME_SAMPLES, SoundChain, WAITING_FRAMES};
use crate::share::{
    CaptureError, CaptureNotice, CaptureRequest, CaptureSink, CaptureStream, ShareKind,
    read_sound_name, read_track_name, sound_name, track_name,
};

/// The number of slots of the own shares, one for each share that a person can run.
pub(crate) const MAX_SLOTS: usize = MAX_SHARES as usize;
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
pub(super) fn picture_options(
    size: (u32, u32),
    frame_rate: u32,
    encoder: super::sent::Encoder,
) -> TrackPublishOptions {
    super::sent::encoded_with(
        TrackPublishOptions {
            source: TrackSource::Screenshare,
            video_encoding: Some(picture_encoding(size, frame_rate)),
            simulcast: false,
            simulcast_layers: None,
            degradation_preference: Some(DegradationPreference::MaintainResolution),
            ..TrackPublishOptions::default()
        },
        encoder,
    )
}

/// The person of a camera or a share in the call of this device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Person {
    /// The person of this device.
    Own,
    Of(AccountId),
}

/// A share in the call of this device: its person and the slot that the sharing device gave
/// it. A person runs up to `MAX_SHARES` shares, in slots `0` to `MAX_SHARES - 1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ShareId {
    pub person: Person,
    pub slot: u8,
}

impl ShareId {
    pub const fn own(slot: u8) -> Self {
        Self {
            person: Person::Own,
            slot,
        }
    }

    pub const fn of(account_id: AccountId, slot: u8) -> Self {
        Self {
            person: Person::Of(account_id),
            slot,
        }
    }
}

/// A share of this device as it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareInfo {
    /// The identifier of the share, new at each start: the server tells a new share in a
    /// slot from the share that ran there before (ADR 0510).
    pub id: Uuid,
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

/// A share of this device in its slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShareState {
    /// The capture starts, or the picture track is being published.
    Starting,
    Running(ShareInfo),
}

/// The share of another person, as the media connection shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeenShare {
    pub account_id: AccountId,
    /// The slot that the track names; a track of a client of one share names slot 0.
    pub slot: u8,
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
/// the share of another person that was seen first; otherwise the own share of the lowest
/// slot. `own` holds the slots of the own shares that run, `others` the shares of the
/// other people in the order in which they were seen.
pub fn watched(
    stopped: bool,
    choice: Option<ShareId>,
    own: &[u8],
    others: &[ShareId],
) -> Option<ShareId> {
    if stopped {
        return None;
    }
    let runs = |share: &ShareId| match share.person {
        Person::Own => own.contains(&share.slot),
        Person::Of(_) => others.contains(share),
    };
    match choice {
        Some(choice) if runs(&choice) => Some(choice),
        _ => others
            .first()
            .copied()
            .or_else(|| own.iter().min().map(|slot| ShareId::own(*slot))),
    }
}

fn remember_share_sounds(
    held: &mut HashMap<ShareId, junto_client_core::voice::ShareSound>,
    defaults: &HashMap<AccountId, junto_client_core::voice::ShareSound>,
    fallback: f32,
    shares: &[ShareId],
) {
    for share in shares {
        let default = match share.person {
            Person::Of(account) => defaults.get(&account).copied(),
            Person::Own => None,
        }
        .unwrap_or(junto_client_core::voice::ShareSound {
            volume: (fallback * 100.0).round() as u16,
            muted: false,
        });
        held.entry(*share).or_insert(default);
    }
}

/// The channels to an interface.
#[derive(Clone)]
pub(super) struct Pictures {
    pub receiving: Arc<AtomicBool>,
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
        if !self.receiving.load(Ordering::Acquire) {
            return;
        }
        if view.watched == Some(share)
            && let Some(frame) = VideoPicture::of_buffer(buffer, false, view.size)
        {
            self.main.send_if_modified(|main| {
                if !self.receiving.load(Ordering::Acquire) {
                    return false;
                }
                *main = Some(MainPicture {
                    share,
                    frame: Arc::new(frame),
                });
                true
            });
        }
        if small_at.is_none_or(|at| at.elapsed() >= SMALL_INTERVAL)
            && let Some(frame) = VideoPicture::new(buffer.as_ref(), false, SMALL_PICTURE)
        {
            *small_at = Some(Instant::now());
            self.small.send_if_modified(|small| {
                if !self.receiving.load(Ordering::Acquire) {
                    return false;
                }
                small.insert(share, Arc::new(frame));
                true
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
    slot: u8,
    internal: mpsc::UnboundedSender<Internal>,
    /// The source of the picture track. Absent until the capture has started.
    video: Mutex<Option<NativeVideoSource>>,
    /// The last picture and when it was given to the track.
    last: Mutex<Option<(Arc<dyn VideoBuffer>, Instant)>>,
    /// The width and the height of the last picture of the capture. The share states this
    /// size: the pictures of a shared window follow the size of the window (ADR 0737).
    size: Mutex<Option<(u32, u32)>>,
    arrived: Notify,
    sound: Mutex<SoundChain>,
    ended: AtomicBool,
    stops: Arc<Stops>,
    /// The count of stops of the slot of this share when it was asked for.
    stop: u64,
}

impl ShareSink {
    /// The share was ended by the capture, or stopped at the handle of the engine.
    fn over(&self) -> bool {
        self.ended.load(Ordering::Relaxed) || self.stops.share(self.slot) != self.stop
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

    fn size(&self) -> Option<(u32, u32)> {
        *self.size.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Keeps the size of a picture of the capture, and tells the engine when it changed.
    fn measure(&self, buffer: &dyn VideoBuffer) {
        let size = Some((buffer.width(), buffer.height()));
        let before = std::mem::replace(
            &mut *self.size.lock().unwrap_or_else(PoisonError::into_inner),
            size,
        );
        if before != size {
            let _ = self.internal.send(Internal::ShareSized(self.generation));
        }
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
        self.measure(buffer.as_ref());
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

    fn stop_failed(&self, error: CaptureNotice) {
        let _ = self.internal.send(Internal::ShareStopFailed(error));
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
        pictures.show(ShareId::own(sink.slot), &buffer, now, &mut small_at);
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

/// The shares that the pictures show, in the order of the shares, one entry per share: of two
/// tracks of one share, while one replaces the other in another codec, the one that has a
/// size states it (ADR 0737).
fn one_per_share<T>(
    mut seen: Vec<T>,
    share: impl Fn(&T) -> ShareId,
    order: impl Fn(&T) -> u64,
    sized: impl Fn(&T) -> bool,
) -> Vec<T> {
    seen.sort_by_key(|picture| (order(picture), !sized(picture)));
    seen.dedup_by(|later, earlier| share(later) == share(earlier));
    seen
}

/// The place in the order of the shares of a new track of `share`: the place of the share
/// when another track already shows it, such as a track that the sharer publishes again in
/// another codec, and otherwise the next place.
fn place_of(held: impl IntoIterator<Item = (ShareId, u64)>, share: ShareId, next: &mut u64) -> u64 {
    match held.into_iter().find(|(other, _)| *other == share) {
        Some((_, order)) => order,
        None => {
            *next += 1;
            *next
        }
    }
}

/// A share of this device.
pub(super) struct OwnShare {
    generation: u64,
    id: Uuid,
    slot: u8,
    request: CaptureRequest,
    sink: Arc<ShareSink>,
    /// The capture. Absent while it starts.
    stream: Option<CaptureStream>,
    pub(super) picture: Publication,
    sound_source: Arc<Mutex<Option<NativeAudioSource>>>,
    pub(super) sound: Publication,
    sound_generation: u64,
    /// The call library withdrew the track and publishes it again: it connects anew. The
    /// number names the wait, which has a bound.
    picture_returns: Option<(u64, TrackSid)>,
    sound_returns: Option<(u64, TrackSid)>,
    waits: u64,
    /// The picture track waits until the call library has taken the track permissions of
    /// this number (ADR 0737).
    cleared_by: Option<(u64, LocalVideoTrack, TrackPublishOptions)>,
    /// The maximum bitrate that the published picture track has, when the engine set it.
    bitrate: Option<u64>,
    /// The bitrate whose set failed and was tried once more: a second failure of the same
    /// bitrate is only logged, and another bitrate is tried again at its first failure.
    bitrate_retried: Option<u64>,
    /// The encoder of the published picture track, from the size of the pictures at its
    /// publication.
    encoder: Option<super::sent::Encoder>,
    /// The picture track that is published again in the codec of a new size, until it takes
    /// the place of the published one (ADR 0737).
    replacement: Option<Replacement>,
    /// The encoder and the size of a publication again that failed: it is not tried again
    /// until the size changes.
    refused: Option<(super::sent::Encoder, (u32, u32))>,
    /// The answer of the system to the start of the capture, until the engine takes it.
    /// The share owns it: a capture that has started when the share ends is stopped with
    /// the share.
    started: Started,
    /// The work of the share: the start of its capture, the sound on its way to the track,
    /// and the pictures. It ends with the share.
    tasks: Vec<JoinHandle<()>>,
}

/// A picture track of a share that is published in another codec than the published one.
struct Replacement {
    /// The number of the publication again, which its answer names.
    number: u64,
    encoder: super::sent::Encoder,
    /// The size of the pictures for which the codec was chosen.
    size: (u32, u32),
    /// The publication, once the media server has answered.
    published: Option<LocalTrackPublication>,
    /// The number of the track permissions that place the track for the viewers.
    waits_for: u64,
}

impl OwnShare {
    /// The track that replaces the published picture track, once it is published.
    pub(super) fn replacement_sid(&self) -> Option<TrackSid> {
        self.replacement
            .as_ref()
            .and_then(|replacement| replacement.published.as_ref())
            .map(LocalTrackPublication::sid)
    }

    /// Gives the published picture track the bitrate of the size of the last picture of the
    /// capture: the pictures of a shared window follow the size of the window (ADR 0737).
    fn follow_size(&mut self) {
        let (Publication::Published(publication), Some(size)) = (&self.picture, self.sink.size())
        else {
            return;
        };
        let bitrate = picture_encoding(size, self.request.quality.frame_rate).max_bitrate;
        if self.bitrate == Some(bitrate) {
            return;
        }
        let Some(LocalTrack::Video(track)) = publication.track() else {
            return;
        };
        match track.set_max_bitrate(bitrate) {
            Ok(true) => {
                self.bitrate = Some(bitrate);
                self.bitrate_retried = None;
            }
            Ok(false) => {}
            // The sender refuses parameters that another set changed in between, such as the
            // layers that the media server asks for; the set is tried once more after the
            // work of the engine that is waiting.
            Err(error) if self.bitrate_retried != Some(bitrate) => {
                tracing::info!(%error, slot = self.slot, "the bitrate of the share is set again");
                self.bitrate_retried = Some(bitrate);
                let _ = self
                    .sink
                    .internal
                    .send(Internal::ShareBitrateAgain(self.generation));
            }
            Err(error) => {
                tracing::warn!(%error, slot = self.slot, "the bitrate of the share was not changed");
            }
        }
    }
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

/// A share track whose publication has not answered and that no share uses any more: the
/// picture of a share, by the share, or a sound, by the share and the sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Arriving {
    Picture(u64),
    Sound(u64, u64),
}

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
    share: ShareId,
    publication: RemoteTrackPublication,
    kind: Option<ShareKind>,
    frame_rate: Option<u32>,
    size: Option<(u32, u32)>,
    /// The size of the drawing that the media server was asked for last.
    asked: Option<(u32, u32)>,
    /// The order in which the shares were seen.
    order: u64,
    task: Option<JoinHandle<()>>,
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
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// The sound of the share of another person, while it is received.
pub(super) struct RemoteSound {
    share: ShareId,
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

/// The slot that a start takes: the lowest one without a share, below `limit`.
pub(super) fn free_slot(used: impl IntoIterator<Item = u8>, limit: u8) -> Option<u8> {
    let used: Vec<u8> = used.into_iter().collect();
    (0..limit.min(MAX_SHARES)).find(|slot| !used.contains(slot))
}

impl Actor {
    /// The state of each own share as the caller reads it.
    fn share_states(&self) -> BTreeMap<u8, ShareState> {
        self.own_shares
            .iter()
            .map(|(slot, share)| {
                let state = match (&share.stream, &share.picture) {
                    (Some(stream), Publication::Published(_)) => {
                        let sound = share.request.sound.reported();
                        ShareState::Running(ShareInfo {
                            id: share.id,
                            kind: share.request.picture.kind(),
                            sound,
                            sound_held: sound != ShareAudio::None
                                && !matches!(share.sound, Publication::Published(_)),
                            size: share.sink.size().unwrap_or((stream.width, stream.height)),
                            frame_rate: share.request.quality.frame_rate,
                        })
                    }
                    _ => ShareState::Starting,
                };
                (*slot, state)
            })
            .collect()
    }

    /// The own share of a start or a publication.
    fn own_share_of(&mut self, generation: u64) -> Option<&mut OwnShare> {
        self.own_shares
            .values_mut()
            .find(|share| share.generation == generation)
    }

    /// Tells the state of the shares: the own ones, the ones of the other people in the
    /// order in which they were seen, and the one that is watched.
    pub(super) fn tell_shares(&mut self) {
        let seen = one_per_share(
            self.remote_pictures.values().collect(),
            |picture: &&RemotePicture| picture.share,
            |picture| picture.order,
            |picture| picture.size.is_some(),
        );
        let shares: Vec<SeenShare> = seen
            .iter()
            .filter_map(|picture| match picture.share.person {
                Person::Of(account_id) => Some(SeenShare {
                    account_id,
                    slot: picture.share.slot,
                    kind: picture.kind,
                    frame_rate: picture.frame_rate,
                    size: picture.size,
                }),
                Person::Own => None,
            })
            .collect();
        let others: Vec<ShareId> = shares
            .iter()
            .map(|share| ShareId::of(share.account_id, share.slot))
            .collect();
        let own: Vec<u8> = self
            .own_shares
            .iter()
            .filter(|(_, share)| share.stream.is_some())
            .map(|(slot, _)| *slot)
            .collect();
        remember_share_sounds(
            &mut self.share_sounds,
            &self.share_sound_defaults,
            self.share_volume,
            &others,
        );
        let now = watched(self.watch_stopped, self.watch_choice, &own, &others);
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
        let own_shares = self.share_states();
        let share_ended = self.share_ended.clone();
        let share_sounds = self.share_sounds.clone();
        self.change(|state| {
            state.share_sounds = share_sounds;
            state.own_shares = own_shares;
            state.share_ended = share_ended;
            state.shares = shares;
            state.watched = now;
        });
        self.sync_permissions();
    }

    /// Asks the media server for the picture of each share at the size of its drawing: the
    /// watched share at the size of the interface, every other share at the size of its
    /// small picture.
    fn sync_sizes(&mut self, watched: Option<ShareId>) {
        let view_size = self.view_size;
        for picture in self.remote_pictures.values_mut() {
            let is_watched = watched == Some(picture.share);
            picture.ask(if is_watched { view_size } else { SMALL_PICTURE });
        }
    }

    /// Gives the sound of the watched share its volume. The sound of every other share,
    /// also of another share of the same person, is received and has no volume in the
    /// mix: a change of what the media server sends would have to be kept in order with
    /// the changes before it, and a share sound is a small part of what a share costs.
    fn sync_volumes(&self, watched: Option<ShareId>) {
        for sound in self.remote_sounds.values() {
            let is_watched = watched == Some(sound.share);
            sound.source.set_gain(if is_watched {
                self.share_sounds
                    .get(&sound.share)
                    .copied()
                    .unwrap_or_default()
                    .gain()
            } else {
                0.0
            });
        }
    }

    pub(super) fn set_watch(&mut self, choice: Option<ShareId>) {
        self.pictures.receiving.store(true, Ordering::Release);
        self.watch_stopped = false;
        self.watch_choice = choice;
        self.sync_share_subscriptions();
        self.tell_shares();
    }

    pub(super) fn stop_watching(&mut self) {
        self.pictures.receiving.store(false, Ordering::Release);
        self.watch_stopped = true;
        self.watch_choice = None;
        for picture in self.remote_pictures.values_mut() {
            if let Some(task) = picture.task.take() {
                task.abort();
            }
            self.pictures.forget(picture.share);
        }
        self.remote_sounds.clear();
        self.pictures.main.send_replace(None);
        self.pictures.small.send_replace(HashMap::new());
        self.sync_share_subscriptions();
        self.tell_shares();
    }

    fn sync_share_subscriptions(&self) {
        let Some(room) = &self.room else { return };
        for participant in room.remote_participants().into_values() {
            for publication in participant.track_publications().into_values() {
                if matches!(
                    publication.source(),
                    TrackSource::Screenshare | TrackSource::ScreenshareAudio
                ) && publication.is_desired() == self.watch_stopped
                {
                    publication.set_subscribed(!self.watch_stopped);
                }
            }
        }
    }

    pub(super) fn subscribe_remote_tracks(&mut self) {
        let tracks: Vec<_> = self
            .room
            .iter()
            .flat_map(|room| room.remote_participants().into_values())
            .flat_map(|person| {
                let identity = person.identity().as_str().to_owned();
                person
                    .track_publications()
                    .into_values()
                    .map(move |publication| (publication, identity.clone()))
            })
            .collect();
        for (publication, identity) in tracks {
            self.remote_track_published(publication, &identity);
        }
    }

    pub(super) fn remote_track_published(
        &mut self,
        publication: RemoteTrackPublication,
        identity: &str,
    ) {
        if publication.source() == TrackSource::Screenshare {
            self.remember_remote_picture(publication.clone(), identity);
        }
        let share = matches!(
            publication.source(),
            TrackSource::Screenshare | TrackSource::ScreenshareAudio
        );
        let subscribe = !share || !self.watch_stopped;
        // A publication can arrive before the SDK replaces its session. Reconnected
        // sends the subscription again even when the local intent is already set.
        publication.set_subscribed(subscribe);
    }

    fn remember_remote_picture(&mut self, publication: RemoteTrackPublication, identity: &str) {
        let Some(account_id) = account_of(identity) else {
            return;
        };
        let sid = publication.sid();
        if self.remote_pictures.contains_key(&sid) {
            return;
        }
        let stated = read_track_name(&publication.name());
        let share = ShareId::of(account_id, stated.map_or(0, |stated| stated.slot));
        // A track that shows a share that another track already shows, such as a track that
        // the sharer publishes again in another codec, keeps the place of the share.
        let order = place_of(
            self.remote_pictures
                .values()
                .map(|picture| (picture.share, picture.order)),
            share,
            &mut self.share_order,
        );
        self.remote_pictures.insert(
            sid,
            RemotePicture {
                share,
                publication,
                kind: stated.map(|stated| stated.kind),
                frame_rate: stated.map(|stated| stated.frame_rate),
                size: None,
                asked: None,
                order,
                task: None,
            },
        );
        self.tell_shares();
    }

    pub(super) fn remote_sharer_left(&mut self, identity: &str) {
        let Some(account) = account_of(identity) else {
            return;
        };
        let tracks: Vec<_> = self
            .remote_pictures
            .iter()
            .filter(|(_, picture)| picture.share.person == Person::Of(account))
            .map(|(sid, _)| sid.clone())
            .collect();
        for sid in tracks {
            self.remote_track_ended(&sid);
        }
    }

    pub(super) fn remote_picture_unsubscribed(&mut self, sid: &TrackSid) {
        let Some(picture) = self.remote_pictures.get_mut(sid) else {
            return;
        };
        if let Some(task) = picture.task.take() {
            task.abort();
        }
        let share = picture.share;
        // The track that replaces this one in another codec still shows the share: its
        // picture stays until the new track gives one (ADR 0737).
        let again = self
            .remote_pictures
            .iter()
            .any(|(other, picture)| other != sid && picture.share == share);
        if !again {
            self.pictures.forget(share);
        }
    }

    pub(super) fn set_view_size(&mut self, width: u32, height: u32) {
        let size = (width.clamp(16, 7680), height.clamp(16, 4320));
        if size != self.view_size {
            self.view_size = size;
            self.tell_shares();
        }
    }

    pub(super) fn set_share_sound(
        &mut self,
        share: ShareId,
        mut sound: junto_client_core::voice::ShareSound,
    ) {
        sound.volume = sound
            .volume
            .min(junto_client_core::voice::VOLUME_MAX_PERCENT);
        self.share_sounds.insert(share, sound);
        self.tell_shares();
        self.sync_volumes(self.view.borrow().watched);
    }

    pub(super) fn set_share_volume(&mut self, gain: f32) {
        if let Some(share) = self.view.borrow().watched {
            let sound = self.share_sounds.entry(share).or_default();
            sound.volume = (gain.clamp(0.0, MAX_GAIN) * 100.0).round() as u16;
        } else {
            self.share_volume = gain.clamp(0.0, MAX_GAIN);
        }
        self.tell_shares();
        self.sync_volumes(self.view.borrow().watched);
    }

    /// The picture track of the share of another person arrived. The name of the track
    /// states its kind, its frame rate and its slot.
    pub(super) fn remote_picture(
        &mut self,
        track: &RemoteVideoTrack,
        publication: RemoteTrackPublication,
        identity: &str,
    ) {
        // A participant without an account, such as the recorder, shares nothing.
        self.remember_remote_picture(publication.clone(), identity);
        if self.watch_stopped {
            publication.set_subscribed(false);
            return;
        }
        let sid = publication.sid();
        let Some(picture) = self.remote_pictures.get_mut(&sid) else {
            return;
        };
        let task = self.runtime.spawn(watch_pictures(
            NativeVideoStream::new(track.rtc_track()),
            sid,
            picture.share,
            self.pictures.clone(),
            self.view.subscribe(),
            self.internal.clone(),
        ));
        if let Some(previous) = picture.task.replace(task) {
            previous.abort();
        }
        picture.asked = None;
        self.tell_shares();
        let watched = self.view.borrow().watched;
        self.sync_sizes(watched);
    }

    /// The sound track of the share of another person is received. Its name states the
    /// slot of its share.
    pub(super) fn remote_sound(
        &mut self,
        track: &RemoteAudioTrack,
        publication: RemoteTrackPublication,
        identity: &str,
    ) {
        let Some(account_id) = account_of(identity) else {
            return;
        };
        if self.watch_stopped {
            publication.set_subscribed(false);
            return;
        }
        let sid = publication.sid();
        let share = ShareId::of(account_id, read_sound_name(&publication.name()));
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
                share,
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
        let share = picture.share;
        drop(picture);
        // Another track of the same person and slot can show the same share: the person
        // started a new one before the end of the earlier one was told.
        let again = self
            .remote_pictures
            .values()
            .any(|other| other.share == share);
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
            .map(|picture| picture.share)
            .collect();
        self.remote_pictures.clear();
        self.remote_sounds.clear();
        for share in shares {
            self.pictures.forget(share);
        }
    }

    /// Starts a share of this device in the lowest free slot; the shares that run go on.
    /// `stops`: the count of stops of each slot when the start was asked for; after a
    /// later stop of its slot the share sends nothing. A person who runs as many shares
    /// as the call allows, `MAX_SHARES` or one on a server from before several shares, is
    /// told that no further one starts.
    pub(super) fn start_share(&mut self, request: CaptureRequest, stops: [u64; MAX_SLOTS]) {
        self.share_ended = None;
        if !self.joined() {
            self.share_ended = Some(CaptureNotice::new(CaptureError::Interrupted));
            self.tell_shares();
            return;
        }
        let Some(slot) = free_slot(self.own_shares.keys().copied(), self.share_limit) else {
            self.share_ended = Some(CaptureNotice::new(CaptureError::TooMany));
            self.tell_shares();
            return;
        };
        self.share_generation += 1;
        let generation = self.share_generation;
        let (frames, taken) = mpsc::channel(WAITING_FRAMES);
        let sink = Arc::new(ShareSink {
            generation,
            slot,
            internal: self.internal.clone(),
            video: Mutex::new(None),
            last: Mutex::new(None),
            size: Mutex::new(None),
            arrived: Notify::new(),
            sound: Mutex::new(SoundChain::new(frames)),
            ended: AtomicBool::new(false),
            stops: self.stops.clone(),
            stop: stops[usize::from(slot)],
        });
        let sound_source: Arc<Mutex<Option<NativeAudioSource>>> = Arc::default();
        let starting = self.options.capture.start(request.clone(), sink.clone());
        let internal = self.internal.clone();
        let started: Started = Arc::default();
        let held = started.clone();
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
                *held.lock().unwrap_or_else(PoisonError::into_inner) = Some(answer);
                let _ = internal.send(Internal::ShareStarted(generation));
            }),
        ];
        self.own_shares.insert(
            slot,
            OwnShare {
                generation,
                id: Uuid::now_v7(),
                slot,
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
                cleared_by: None,
                bitrate: None,
                bitrate_retried: None,
                encoder: None,
                replacement: None,
                refused: None,
                started,
                tasks,
            },
        );
        self.tell_shares();
    }

    /// Ends the share of this device in `slot`, or every share without a slot. `error`:
    /// the share ended by itself, and the person is told why; a stop of the person puts
    /// away the statement that a share ended.
    pub(super) fn end_share(&mut self, slot: Option<u8>, error: Option<CaptureError>) {
        let slots: Vec<u8> = match slot {
            Some(slot) => vec![slot],
            None => self.own_shares.keys().copied().collect(),
        };
        for slot in slots {
            let Some(mut share) = self.own_shares.remove(&slot) else {
                continue;
            };
            self.share_generation += 1;
            if matches!(share.picture, Publication::Publishing)
                && share.cleared_by.is_none()
                && share.picture_returns.is_none()
            {
                self.arriving.insert(Arriving::Picture(share.generation));
            }
            if matches!(share.sound, Publication::Publishing) && share.sound_returns.is_none() {
                self.arriving
                    .insert(Arriving::Sound(share.generation, share.sound_generation));
            }
            for publication in [
                std::mem::replace(&mut share.picture, Publication::None),
                std::mem::replace(&mut share.sound, Publication::None),
            ] {
                if let Publication::Published(publication) = publication {
                    self.withdraw(publication);
                }
            }
            if let Some(publication) = share
                .replacement
                .take()
                .and_then(|replacement| replacement.published)
            {
                self.withdraw(publication);
            }
            drop(share);
            self.pictures.forget(ShareId::own(slot));
        }
        self.share_ended = error.map(CaptureNotice::new);
        self.tell_shares();
    }

    /// Puts away the statement that a share ended by itself, and leaves the shares that
    /// run.
    pub(super) fn forget_share_end(&mut self, generation: u64) {
        if self
            .share_ended
            .as_ref()
            .is_some_and(|notice| notice.generation == generation)
        {
            self.share_ended = None;
            self.tell_shares();
        }
    }

    pub(super) fn share_started(&mut self, generation: u64) {
        let started = self
            .own_share_of(generation)
            .filter(|share| share.stream.is_none())
            .and_then(|share| {
                share
                    .started
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take()
                    .map(|started| (share.slot, started))
            });
        let Some((slot, started)) = started else {
            return;
        };
        let stream = match started {
            Ok(stream) => stream,
            Err(error) => {
                tracing::warn!(%error, slot, "the capture of the share did not start");
                self.end_share(Some(slot), Some(error));
                return;
            }
        };
        let rule = *self.encoder_rule.borrow();
        let (Some(_), Some(share)) = (&self.room, self.own_shares.get_mut(&slot)) else {
            self.end_share(Some(slot), Some(CaptureError::Interrupted));
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
            slot,
        );
        let track = LocalVideoTrack::create_video_track(&name, RtcVideoSource::Native(source));
        let encoder = rule.encoder(
            (stream.width, stream.height),
            share.request.quality.frame_rate,
        );
        let options = picture_options(
            (stream.width, stream.height),
            share.request.quality.frame_rate,
            encoder,
        );
        share.encoder = Some(encoder);
        share.stream = Some(stream);
        share.picture = Publication::Publishing;
        share.cleared_by = Some((u64::MAX, track, options));
        // The permissions that count this share reach the call library before its tracks.
        let Some(number) = self.sync_permissions() else {
            self.end_share(
                Some(slot),
                Some(CaptureError::Failed(
                    "the track permissions of the room were not set".to_owned(),
                )),
            );
            return;
        };
        if number <= self.permissions_applied {
            self.publish_share_picture(slot);
        } else {
            if let Some((waits_for, _, _)) = self
                .own_shares
                .get_mut(&slot)
                .and_then(|share| share.cleared_by.as_mut())
            {
                *waits_for = number;
            }
            let internal = self.internal.clone();
            self.runtime.spawn(async move {
                tokio::time::sleep(PUBLISH_LIMIT).await;
                let _ = internal.send(Internal::SharePermissionsDue(generation));
            });
        }
        self.tell_shares();
    }

    /// Publishes the picture track of the share in `slot` that waits for its permissions,
    /// and its sound.
    fn publish_share_picture(&mut self, slot: u8) {
        let (Some(room), Some(share)) = (&self.room, self.own_shares.get_mut(&slot)) else {
            return;
        };
        let Some((_, track, options)) = share.cleared_by.take() else {
            return;
        };
        let generation = share.generation;
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
    }

    /// The call library took the track permissions up to `number`, or failed to take those
    /// of `number`: the shares that wait for them are published, or end.
    pub(super) fn share_permissions_applied(&mut self, number: u64, failure: Option<String>) {
        let replaced: Vec<u64> = self
            .own_shares
            .values()
            .filter(|share| {
                share.replacement.as_ref().is_some_and(|replacement| {
                    replacement.published.is_some() && replacement.waits_for <= number
                })
            })
            .map(|share| share.generation)
            .collect();
        for generation in replaced {
            match &failure {
                None => self.replace_share_picture(generation),
                // The replacement is not placed for the viewers: it is withdrawn, and a later
                // change of size or a new publication of the picture publishes again.
                Some(error) => {
                    tracing::warn!(%error, "the replacement of the picture of a share was withdrawn without its permissions");
                    let published = self
                        .own_share_of(generation)
                        .and_then(|share| share.replacement.take())
                        .and_then(|replacement| replacement.published);
                    if let Some(publication) = published {
                        self.withdraw(publication);
                    }
                    self.sync_permissions();
                }
            }
        }
        let waiting: Vec<u8> = self
            .own_shares
            .iter()
            .filter(|(_, share)| {
                share
                    .cleared_by
                    .as_ref()
                    .is_some_and(|(waits_for, _, _)| *waits_for <= number)
            })
            .map(|(slot, _)| *slot)
            .collect();
        for slot in waiting {
            match &failure {
                None => self.publish_share_picture(slot),
                Some(error) => {
                    tracing::warn!(%error, slot, "the share was not published without its permissions");
                    self.end_share(
                        Some(slot),
                        Some(CaptureError::Failed(format!(
                            "the track permissions of the room were not set: {error}"
                        ))),
                    );
                }
            }
        }
    }

    /// The time of a share to wait for its permissions has passed.
    pub(super) fn share_permissions_due(&mut self, generation: u64) {
        let Some(share) = self
            .own_share_of(generation)
            .filter(|share| share.cleared_by.is_some())
        else {
            return;
        };
        let slot = share.slot;
        tracing::warn!(
            slot,
            "the track permissions of the share were not set in time"
        );
        self.end_share(
            Some(slot),
            Some(CaptureError::Failed(
                "the track permissions of the room were not set in time".to_owned(),
            )),
        );
    }

    pub(super) fn share_picture_published(
        &mut self,
        generation: u64,
        result: Result<LocalTrackPublication, String>,
    ) {
        let current = self
            .own_share_of(generation)
            .filter(|share| matches!(share.picture, Publication::Publishing));
        match (current, result) {
            (Some(share), Ok(publication)) => {
                share.picture = Publication::Published(publication);
                share.bitrate = None;
                share.follow_size();
                self.sync_share_sound();
                self.tell_shares();
                // The size can have changed while the publication waited.
                self.publish_in_codec_of_size(generation);
            }
            (Some(share), Err(error)) => {
                let slot = share.slot;
                tracing::warn!(%error, slot, "the picture of the share was not published");
                self.end_share(Some(slot), Some(CaptureError::Failed(error)));
            }
            // The share ended while its track was published.
            (None, result) => {
                self.arriving.remove(&Arriving::Picture(generation));
                if let Ok(publication) = result {
                    self.withdraw(publication);
                }
                self.sync_permissions();
            }
        }
    }

    /// A track of this device was withdrawn, and this device did not ask for it. The call
    /// library does that when it connects anew, and publishes the same track again; the
    /// media server does it when the person may not send the track any more. The share
    /// waits for the track for a bounded time. Returns whether it was a track of a share.
    pub(super) fn share_track_taken(&mut self, sid: &TrackSid) -> bool {
        // A track that replaces a picture track in another codec is not awaited: the share
        // keeps its picture track, and a later change of size publishes again.
        if let Some(share) = self
            .own_shares
            .values_mut()
            .find(|share| share.replacement_sid().as_ref() == Some(sid))
        {
            share.replacement = None;
            return true;
        }
        let is = |publication: &Publication| matches!(publication, Publication::Published(held) if held.sid() == *sid);
        let Some((share, track)) = self.own_shares.values_mut().find_map(|share| {
            if is(&share.picture) {
                Some((share, ShareTrack::Picture))
            } else if is(&share.sound) {
                Some((share, ShareTrack::Sound))
            } else {
                None
            }
        }) else {
            return false;
        };
        share.waits += 1;
        let wait = share.waits;
        match track {
            ShareTrack::Picture => {
                share.picture = Publication::Publishing;
                share.picture_returns = Some((wait, sid.clone()));
                share.replacement = None;
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
    /// A track that a share waits for is its track again. A track that no share uses is
    /// withdrawn: its share was stopped, or it has published a new track meanwhile.
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
        // Only the track that a share sent returns to it. A track of a share before this
        // one, which the call library held when it connected anew, is not adopted.
        let returned = self.own_shares.values_mut().find_map(|share| {
            let (held, returns) = match track {
                ShareTrack::Picture => (&mut share.picture, &mut share.picture_returns),
                ShareTrack::Sound => (&mut share.sound, &mut share.sound_returns),
            };
            let waits_for = returns.as_ref().map(|(_, sid)| sid);
            let holds = match &*held {
                Publication::Published(before) => Some(before.sid()),
                Publication::None | Publication::Publishing => None,
            };
            is_return(waits_for, holds.as_ref(), previous).then_some((held, returns))
        });
        match returned {
            Some((held, returns)) => {
                let sid = publication.sid();
                *held = Publication::Published(publication);
                *returns = None;
                // The call library published the track again with the options of its
                // first publication.
                if track == ShareTrack::Picture
                    && let Some(share) = self.own_shares.values_mut().find(|share| {
                        matches!(&share.picture, Publication::Published(held) if held.sid() == sid)
                    })
                {
                    share.bitrate = None;
                    share.follow_size();
                    // The size can have changed while the track was away.
                    let generation = share.generation;
                    self.publish_in_codec_of_size(generation);
                }
                // The permission can have changed while the track was away.
                self.sync_share_sound();
            }
            None => self.withdraw(publication),
        }
    }

    /// The time for a withdrawn track to return has passed.
    pub(super) fn share_track_due(&mut self, generation: u64, wait: u64, track: ShareTrack) {
        let Some(share) = self.own_share_of(generation) else {
            return;
        };
        match track {
            ShareTrack::Picture
                if share.picture_returns.as_ref().map(|(held, _)| *held) == Some(wait) =>
            {
                share.picture = Publication::None;
                let slot = share.slot;
                self.end_share(
                    Some(slot),
                    Some(CaptureError::Failed(
                        "the media server ended the picture of the share".to_owned(),
                    )),
                );
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

    /// Publishes or withdraws the sound track of each own share by what holds now: it is
    /// published while the share runs with a sound and the server lets the person send it.
    pub(super) fn sync_share_sound(&mut self) {
        let may_send = self.may_send(SHARE_SOUND_SOURCE) && !self.controls.microphone_blocked;
        let mut withdrawn = Vec::new();
        for share in self.own_shares.values_mut() {
            // The sound is published after the media server answered the publication of the
            // picture: a sound published at the same moment could take the negotiation of the
            // picture before the call library set the codec of the picture, which was then
            // sent in VP8 instead of H.264 (ADR 0737).
            // The condition of the picture holds only for a new publication of the sound: a
            // sound track that the call library returns before the picture track after a new
            // connection stays published.
            let wanted = share.stream.is_some()
                && share.cleared_by.is_none()
                && share.request.sound.reported() != ShareAudio::None
                && may_send;
            let picture_published = matches!(share.picture, Publication::Published(_));
            match (&share.sound, wanted) {
                (Publication::None, true) if picture_published => {
                    let Some(room) = &self.room else {
                        continue;
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
                        &sound_name(share.slot),
                        RtcAudioSource::Native(source),
                    );
                    let options = TrackPublishOptions {
                        source: TrackSource::ScreenshareAudio,
                        audio_encoding: Some(AudioEncoding {
                            max_bitrate: SHARE_SOUND_BITRATE,
                        }),
                        // The sound of a share is music as often as speech: it is sent
                        // without gaps in silence and without a second copy at a low
                        // bitrate.
                        dtx: false,
                        red: false,
                        ..TrackPublishOptions::default()
                    };
                    let (participant, internal) = (room.local_participant(), self.internal.clone());
                    self.runtime.spawn(async move {
                        let publishing =
                            participant.publish_track(LocalTrack::Audio(track), options);
                        let result = match tokio::time::timeout(PUBLISH_LIMIT, publishing).await {
                            Ok(Ok(publication)) => Ok(publication),
                            Ok(Err(error)) => Err(error.to_string()),
                            Err(_) => Err("no answer of the media server".to_owned()),
                        };
                        let _ = internal.send(Internal::ShareSoundPublished(key, result));
                    });
                }
                (Publication::Published(_) | Publication::Publishing, false) => {
                    if matches!(share.sound, Publication::Publishing)
                        && share.sound_returns.is_none()
                    {
                        self.arriving
                            .insert(Arriving::Sound(share.generation, share.sound_generation));
                    }
                    share.sound_generation += 1;
                    share.sound_returns = None;
                    *share
                        .sound_source
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner) = None;
                    if let Publication::Published(publication) =
                        std::mem::replace(&mut share.sound, Publication::None)
                    {
                        withdrawn.push(publication);
                    }
                }
                _ => {}
            }
        }
        for publication in withdrawn {
            self.withdraw(publication);
        }
        self.tell_shares();
    }

    pub(super) fn share_sound_published(
        &mut self,
        key: (u64, u64),
        result: Result<LocalTrackPublication, String>,
    ) {
        let current = self.own_share_of(key.0).filter(|share| {
            share.sound_generation == key.1 && matches!(share.sound, Publication::Publishing)
        });
        match (current, result) {
            (Some(share), Ok(publication)) => {
                share.sound = Publication::Published(publication);
                self.tell_shares();
            }
            (Some(share), Err(error)) => {
                tracing::info!(%error, slot = share.slot, "the sound of the share was not published");
                share.sound = Publication::None;
                self.retry_share_sound_later(key.0);
            }
            (None, result) => {
                self.arriving.remove(&Arriving::Sound(key.0, key.1));
                if let Ok(publication) = result {
                    self.withdraw(publication);
                }
                self.sync_permissions();
            }
        }
    }

    fn retry_share_sound_later(&self, generation: u64) {
        let internal = self.internal.clone();
        self.runtime.spawn(async move {
            tokio::time::sleep(PUBLISH_RETRY).await;
            let _ = internal.send(Internal::ShareSoundRetry(generation));
        });
    }

    pub(super) fn share_sound_retry(&mut self, generation: u64) {
        if self.own_share_of(generation).is_some() {
            self.sync_share_sound();
        }
    }

    /// The pictures of the capture of a share have a new size.
    /// A refused set of the bitrate is tried once more. Only the bitrate: the size of the
    /// pictures did not change, and the codec is checked where the size or the publication
    /// changes.
    pub(super) fn share_bitrate_again(&mut self, generation: u64) {
        if let Some(share) = self.own_share_of(generation) {
            share.follow_size();
        }
    }

    pub(super) fn share_sized(&mut self, generation: u64) {
        if let Some(share) = self.own_share_of(generation) {
            share.follow_size();
            self.publish_in_codec_of_size(generation);
            self.tell_shares();
        }
    }

    /// A share keeps the codec that the size of its pictures allows (ADR 0737, ADR 0773): when
    /// the size of the pictures changes the encoder of the published picture track, a track
    /// in the codec of the new size is published from the same source, under the same name.
    /// It takes the place of the published track once the track permissions place it for the
    /// viewers; the earlier track is then withdrawn.
    fn publish_in_codec_of_size(&mut self, generation: u64) {
        let Some(room) = &self.room else {
            return;
        };
        let participant = room.local_participant();
        let internal = self.internal.clone();
        let rule = *self.encoder_rule.borrow();
        self.replacements += 1;
        let number = self.replacements;
        let Some(share) = self.own_share_of(generation) else {
            return;
        };
        let (Publication::Published(_), None, Some(size), Some(encoder)) = (
            &share.picture,
            &share.replacement,
            share.sink.size(),
            share.encoder,
        ) else {
            return;
        };
        let frame_rate = share.request.quality.frame_rate;
        let wanted = rule.encoder(size, frame_rate);
        // A publication in this codec failed at this size: it is tried again at another size.
        if wanted == encoder || share.refused == Some((wanted, size)) {
            return;
        }
        let Some(source) = share
            .sink
            .video
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
        else {
            return;
        };
        let name = track_name(share.request.picture.kind(), frame_rate, share.slot);
        let track = LocalVideoTrack::create_video_track(&name, RtcVideoSource::Native(source));
        let options = picture_options(size, frame_rate, wanted);
        tracing::info!(
            slot = share.slot,
            ?encoder,
            ?wanted,
            "the picture of the share is published again in the codec of its size"
        );
        share.replacement = Some(Replacement {
            number,
            encoder: wanted,
            size,
            published: None,
            waits_for: u64::MAX,
        });
        self.runtime.spawn(async move {
            let publishing = participant.publish_track(LocalTrack::Video(track), options);
            let result = match tokio::time::timeout(PUBLISH_LIMIT, publishing).await {
                Ok(Ok(publication)) => Ok(publication),
                Ok(Err(error)) => Err(error.to_string()),
                Err(_) => Err("no answer of the media server".to_owned()),
            };
            let _ = internal.send(Internal::ShareReplacementPublished(
                generation, number, result,
            ));
        });
    }

    /// The media server answered the publication again of the picture track of a share.
    pub(super) fn share_replacement_published(
        &mut self,
        generation: u64,
        number: u64,
        result: Result<LocalTrackPublication, String>,
    ) {
        let current = self.own_share_of(generation).filter(|share| {
            share
                .replacement
                .as_ref()
                .is_some_and(|replacement| replacement.number == number)
        });
        let Some(share) = current else {
            // The share ended, or the publication is not the awaited one.
            if let Ok(publication) = result {
                self.withdraw(publication);
            }
            return;
        };
        match result {
            Ok(publication) => {
                if let Some(replacement) = share.replacement.as_mut() {
                    replacement.published = Some(publication);
                }
                match self.sync_permissions() {
                    Some(number) if number > self.permissions_applied => {
                        if let Some(replacement) = self
                            .own_share_of(generation)
                            .and_then(|share| share.replacement.as_mut())
                        {
                            replacement.waits_for = number;
                        }
                    }
                    // The permissions that place the track are taken, or no list is needed.
                    _ => self.replace_share_picture(generation),
                }
            }
            Err(error) => {
                tracing::warn!(%error, slot = share.slot, "the picture of the share was not published again in the codec of its size");
                if let Some(replacement) = share.replacement.take() {
                    share.refused = Some((replacement.encoder, replacement.size));
                }
                // The size can have changed while the publication waited.
                self.publish_in_codec_of_size(generation);
            }
        }
    }

    /// The track in the codec of the new size takes the place of the published picture track
    /// of the share, and the earlier track is withdrawn.
    fn replace_share_picture(&mut self, generation: u64) {
        let Some(share) = self.own_share_of(generation) else {
            return;
        };
        let Some(Replacement {
            encoder,
            published: Some(publication),
            ..
        }) = share.replacement.take()
        else {
            return;
        };
        let earlier = std::mem::replace(&mut share.picture, Publication::Published(publication));
        share.encoder = Some(encoder);
        share.picture_returns = None;
        share.bitrate = None;
        share.follow_size();
        if let Publication::Published(earlier) = earlier {
            self.withdraw(earlier);
        }
        self.sync_permissions();
        // The size can have changed the encoder again while the track was published.
        self.publish_in_codec_of_size(generation);
        self.tell_shares();
    }

    pub(super) fn share_ended(&mut self, generation: u64, error: CaptureError) {
        if let Some(share) = self.own_share_of(generation) {
            let slot = share.slot;
            tracing::info!(%error, slot, "the capture of the share ended");
            self.end_share(Some(slot), Some(error));
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_share_that_two_tracks_show_is_listed_once_at_its_place() {
        use junto_client_core::protocol::ids::AccountId;
        let (one, two) = (AccountId::generate(), AccountId::generate());
        let first = ShareId::of(one, 0);
        let second = ShareId::of(two, 0);
        let mut next = 0;
        let mut held: Vec<(ShareId, u64)> = Vec::new();
        for share in [first, second] {
            let order = place_of(held.clone(), share, &mut next);
            held.push((share, order));
        }
        // The track in the codec of a new size keeps the place of its share.
        let again = place_of(held.clone(), first, &mut next);
        assert_eq!(again, 1);
        assert_eq!(next, 2);
        // (share, order, has a size): the replacement of the first share has no size yet.
        let seen = vec![(second, 2, true), (first, 1, false), (first, 1, true)];
        let listed = one_per_share(seen, |item| item.0, |item| item.1, |item| item.2);
        assert_eq!(listed, [(first, 1, true), (second, 2, true)]);
        let only_new = vec![(first, 1, false)];
        assert_eq!(
            one_per_share(only_new, |item| item.0, |item| item.1, |item| item.2),
            [(first, 1, false)]
        );
    }

    use super::*;

    #[test]
    fn the_watched_share_is_the_choice_while_it_runs_then_the_first_other_then_the_own() {
        let (ann, ben) = (AccountId::generate(), AccountId::generate());
        let (ann_0, ben_0, ben_1) = (
            ShareId::of(ann, 0),
            ShareId::of(ben, 0),
            ShareId::of(ben, 1),
        );
        // Nothing is shared.
        assert_eq!(watched(false, None, &[], &[]), None);
        assert_eq!(watched(false, Some(ShareId::own(0)), &[], &[]), None);
        // The own share alone is the preview of the sharer; of two, the lowest slot.
        assert_eq!(watched(false, None, &[0], &[]), Some(ShareId::own(0)));
        assert_eq!(watched(false, None, &[2, 1], &[]), Some(ShareId::own(1)));
        // Without a choice: the share of another person that was seen first.
        assert_eq!(watched(false, None, &[0], &[ann_0, ben_0]), Some(ann_0));
        // The choice holds while its share runs, also a second share of one person.
        assert_eq!(
            watched(false, Some(ben_0), &[0], &[ann_0, ben_0]),
            Some(ben_0)
        );
        assert_eq!(
            watched(false, Some(ben_1), &[], &[ben_0, ben_1]),
            Some(ben_1)
        );
        assert_eq!(
            watched(false, Some(ShareId::own(1)), &[0, 1], &[ann_0, ben_0]),
            Some(ShareId::own(1))
        );
        // A choice whose share has ended gives way to the rule without a choice: another
        // share of the same person is not chosen for the viewer.
        assert_eq!(
            watched(false, Some(ben_1), &[], &[ann_0, ben_0]),
            Some(ann_0)
        );
        assert_eq!(
            watched(false, Some(ben_0), &[0], &[]),
            Some(ShareId::own(0))
        );
        assert_eq!(
            watched(false, Some(ShareId::own(2)), &[0], &[]),
            Some(ShareId::own(0))
        );
        assert_eq!(
            watched(false, Some(ShareId::own(0)), &[], &[ben_1]),
            Some(ben_1)
        );
    }

    #[test]
    fn a_share_keeps_its_own_sound_while_new_slots_take_the_saved_sound_of_the_person() {
        use junto_client_core::voice::ShareSound;
        let (ann, ben) = (AccountId::generate(), AccountId::generate());
        let (first, second, other) = (
            ShareId::of(ann, 0),
            ShareId::of(ann, 1),
            ShareId::of(ben, 0),
        );
        let mut held = HashMap::new();
        let mut defaults = HashMap::from([(
            ann,
            ShareSound {
                volume: 65,
                muted: true,
            },
        )]);
        remember_share_sounds(&mut held, &defaults, 0.8, &[first, second, other]);
        assert_eq!(held[&first], defaults[&ann]);
        assert_eq!(
            held[&other],
            ShareSound {
                volume: 80,
                muted: false
            }
        );
        held.insert(
            first,
            ShareSound {
                volume: 120,
                muted: false,
            },
        );
        defaults.insert(
            ann,
            ShareSound {
                volume: 45,
                muted: false,
            },
        );
        remember_share_sounds(
            &mut held,
            &defaults,
            1.0,
            &[first, second, ShareId::of(ann, 2)],
        );
        assert_eq!(
            held[&first],
            ShareSound {
                volume: 120,
                muted: false
            }
        );
        assert_eq!(
            held[&second],
            ShareSound {
                volume: 65,
                muted: true
            }
        );
        assert_eq!(held[&ShareId::of(ann, 2)], defaults[&ann]);
    }

    #[test]
    fn stopping_watching_prevents_every_automatic_choice_until_watch() {
        let share = ShareId::of(AccountId::generate(), 0);
        assert_eq!(watched(true, Some(share), &[0], &[share]), None);
        assert_eq!(watched(true, None, &[0], &[]), None);
        assert_eq!(watched(true, None, &[], &[share]), None);
        assert_eq!(watched(false, Some(share), &[0], &[share]), Some(share));
    }

    #[test]
    fn a_start_takes_the_lowest_free_slot_and_no_slot_beyond_the_limit() {
        assert_eq!(free_slot([], MAX_SHARES), Some(0));
        assert_eq!(free_slot([0, 2], MAX_SHARES), Some(1));
        assert_eq!(free_slot([1, 2], MAX_SHARES), Some(0));
        assert_eq!(free_slot([0, 1, 2], MAX_SHARES), None);
        assert_eq!(MAX_SLOTS, 3);
        // A server from before several shares: one share.
        assert_eq!(free_slot([], 1), Some(0));
        assert_eq!(free_slot([0], 1), None);
        assert_eq!(free_slot([], 200), Some(0));
        assert_eq!(free_slot([0, 1, 2], 200), None);
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
        let options = picture_options((1920, 1080), 30, super::super::sent::Encoder::Vp8);
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
                &picture_options(size, 30, super::super::sent::Encoder::Vp8),
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
        let share = ShareId::of(AccountId::generate(), 0);
        let pictures = Pictures {
            receiving: Arc::new(AtomicBool::new(true)),
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
        let share = ShareId::of(AccountId::generate(), 1);
        let pictures = Pictures {
            receiving: Arc::new(AtomicBool::new(true)),
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
