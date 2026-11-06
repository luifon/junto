//! A capture that the program makes itself, for tests and for the screen tool: a picture
//! of two known colours and a tone on each channel. It reads no screen and no sound of the
//! machine.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::{Duration, Instant};

use livekit::webrtc::video_frame::I420Buffer;

use super::{
    ApplicationId, ApplicationPicture, ApplicationSource, CaptureError, CaptureRequest,
    CaptureSink, CaptureStream, DisplaySource, Pending, PictureSource, ScreenAccess, ScreenCapture,
    ShareSources, SoundChoice, WindowSource,
};
use crate::audio::{Format, SAMPLE_RATE};

/// A colour as red, green and blue.
pub type Colour = (u8, u8, u8);

/// The two tones of a sound: the left and the right channel, in hertz.
pub type Tones = (f32, f32);

const AMPLITUDE: f32 = 0.25;

/// Brightness and the two colour values of a colour, as video carries them.
fn video_colour(colour: Colour) -> (u8, u8, u8) {
    let (r, g, b) = (
        f32::from(colour.0),
        f32::from(colour.1),
        f32::from(colour.2),
    );
    let y = 16.0 + 0.257 * r + 0.504 * g + 0.098 * b;
    let u = 128.0 - 0.148 * r - 0.291 * g + 0.439 * b;
    let v = 128.0 + 0.439 * r - 0.368 * g - 0.071 * b;
    (y.round() as u8, u.round() as u8, v.round() as u8)
}

/// A picture whose left half has one colour and whose right half has another one.
pub fn two_colours(width: u32, height: u32, left: Colour, right: Colour) -> I420Buffer {
    let mut buffer = I420Buffer::new(width, height);
    let (stride_y, stride_u, stride_v) = buffer.strides();
    let (left, right) = (video_colour(left), video_colour(right));
    let (chroma_width, half) = (buffer.chroma_width() as usize, (width / 2) as usize);
    let (y, u, v) = buffer.data_mut();
    for row in y.chunks_mut(stride_y as usize) {
        let visible = row.len().min(width as usize);
        for (x, value) in row[..visible].iter_mut().enumerate() {
            *value = if x < half { left.0 } else { right.0 };
        }
    }
    for (plane, stride, values) in [
        (u, stride_u, (left.1, right.1)),
        (v, stride_v, (left.2, right.2)),
    ] {
        for row in plane.chunks_mut(stride as usize) {
            let visible = row.len().min(chroma_width);
            for (x, value) in row[..visible].iter_mut().enumerate() {
                *value = if x < half / 2 { values.0 } else { values.1 };
            }
        }
    }
    buffer
}

struct State {
    access: ScreenAccess,
    sources: ShareSources,
    colours: (Colour, Colour),
    /// The colours of the pictures of one source, in place of `colours`.
    source_colours: HashMap<PictureSource, (Colour, Colour)>,
    system_tones: Tones,
    application_tones: HashMap<ApplicationId, Tones>,
    /// A capture gives one picture and then none, as the capture of a screen that does
    /// not change.
    still: bool,
    /// A capture takes this long to start, as a system that answers late.
    start_delay: Duration,
    /// The picture of a preview of these sources takes this long, as a window of an
    /// application that does not answer.
    preview_delays: HashMap<PictureSource, Duration>,
    fail_next: Option<CaptureError>,
    asked: u32,
    settings_opened: u32,
    previewed: Vec<PictureSource>,
    started: Vec<CaptureRequest>,
    live: Vec<Live>,
    stall_next_stop: Option<(std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>)>,
}

impl State {
    /// The width and the height of a source as it is now: a screen of the portal has
    /// 1280 × 720.
    fn size_of(&self, source: PictureSource) -> Option<(u32, u32)> {
        match source {
            PictureSource::Portal(kind) => {
                self.sources.portal.contains(&kind).then_some((1280, 720))
            }
            PictureSource::Display(id) => self
                .sources
                .displays
                .iter()
                .find(|display| display.id == id)
                .map(|display| (display.width, display.height)),
            PictureSource::Window(id) => self
                .sources
                .windows
                .iter()
                .find(|window| window.id == id)
                .map(|window| (window.width, window.height)),
            PictureSource::Application(process_id) => application_size(&self.sources, process_id),
        }
    }
}

/// A capture that runs: its stop, its sink and the source that it shows.
struct Live {
    stop: Weak<AtomicBool>,
    sink: Arc<dyn CaptureSink>,
    source: PictureSource,
}

/// The capture of the tests. Every clone is the same object.
#[derive(Clone)]
pub struct SyntheticCapture {
    state: Arc<Mutex<State>>,
}

/// The process identifier of the one application of [`SyntheticCapture::new`].
pub const PLAYER: i32 = 4_001;

impl Default for SyntheticCapture {
    fn default() -> Self {
        Self::new()
    }
}

impl SyntheticCapture {
    /// One screen of 1280 × 720, one window of 800 × 600 of the application "Player", the
    /// windows of "Player" on that screen, and the permission of the system.
    pub fn new() -> Self {
        let player = ApplicationId {
            process_id: PLAYER,
            bundle_id: "example.player".to_owned(),
        };
        Self {
            state: Arc::new(Mutex::new(State {
                access: ScreenAccess::Allowed,
                sources: ShareSources {
                    portal: Vec::new(),
                    system_sound: true,
                    started_the_client: Vec::new(),
                    creators_unknown: false,
                    displays: vec![DisplaySource {
                        id: 1,
                        width: 1280,
                        height: 720,
                    }],
                    windows: vec![WindowSource {
                        id: 11,
                        title: "Editor".to_owned(),
                        application: player.clone(),
                        application_name: "Player".to_owned(),
                        width: 800,
                        height: 600,
                    }],
                    application_capture: true,
                    application_pictures: vec![ApplicationPicture {
                        application: player.clone(),
                        name: "Player".to_owned(),
                        display: 1,
                        width: 1280,
                        height: 720,
                        windows: 1,
                    }],
                    applications: vec![ApplicationSource {
                        id: player.clone(),
                        name: "Player".to_owned(),
                    }],
                },
                colours: ((200, 30, 30), (30, 30, 200)),
                source_colours: HashMap::new(),
                system_tones: (700.0, 1_900.0),
                application_tones: HashMap::from([(player, (1_100.0, 2_700.0))]),
                still: false,
                start_delay: Duration::ZERO,
                preview_delays: HashMap::new(),
                fail_next: None,
                asked: 0,
                settings_opened: 0,
                previewed: Vec::new(),
                started: Vec::new(),
                live: Vec::new(),
                stall_next_stop: None,
            })),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn player() -> ApplicationId {
        ApplicationId {
            process_id: PLAYER,
            bundle_id: "example.player".to_owned(),
        }
    }

    pub fn set_access(&self, access: ScreenAccess) {
        self.lock().access = access;
    }

    /// What the capture lists now.
    pub fn listed(&self) -> ShareSources {
        self.lock().sources.clone()
    }

    pub fn set_sources(&self, sources: ShareSources) {
        self.lock().sources = sources;
    }

    /// Gives a screen or a window a new size, as a person who resizes a window. A capture of
    /// it gives its next pictures at the new size.
    pub fn resize_source(&self, source: PictureSource, width: u32, height: u32) {
        let mut state = self.lock();
        match source {
            PictureSource::Display(id) => {
                for display in state
                    .sources
                    .displays
                    .iter_mut()
                    .filter(|display| display.id == id)
                {
                    (display.width, display.height) = (width, height);
                }
            }
            PictureSource::Window(id) => {
                for window in state
                    .sources
                    .windows
                    .iter_mut()
                    .filter(|window| window.id == id)
                {
                    (window.width, window.height) = (width, height);
                }
            }
            PictureSource::Portal(_) => {}
        }
    }

    /// The colours of the left and of the right half of the pictures of one source from
    /// now on, so that two shares of one person are told apart.
    pub fn set_source_colours(&self, source: PictureSource, left: Colour, right: Colour) {
        self.lock().source_colours.insert(source, (left, right));
    }

    /// The colours of the left and of the right half of every picture from now on.
    pub fn set_colours(&self, left: Colour, right: Colour) {
        self.lock().colours = (left, right);
    }

    /// The tones of all system sound.
    pub fn set_system_tones(&self, tones: Tones) {
        self.lock().system_tones = tones;
    }

    pub fn set_still(&self, still: bool) {
        self.lock().still = still;
    }

    /// The preview of `source` takes `delay` from now on.
    pub fn set_preview_delay(&self, source: PictureSource, delay: Duration) {
        self.lock().preview_delays.insert(source, delay);
    }

    pub fn set_start_delay(&self, delay: Duration) {
        self.lock().start_delay = delay;
    }

    pub fn fail_next(&self, error: CaptureError) {
        self.lock().fail_next = Some(error);
    }

    /// How often the system was made to ask for the permission.
    pub fn asked(&self) -> u32 {
        self.lock().asked
    }

    pub fn settings_opened(&self) -> u32 {
        self.lock().settings_opened
    }

    /// The sources whose preview was asked for, in order.
    pub fn previewed(&self) -> Vec<PictureSource> {
        self.lock().previewed.clone()
    }

    /// The requests of the captures that were started.
    /// The next capture also owns a worker that ignores stop until the test releases it.
    pub fn stall_next_stop(&self) -> (std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>) {
        let (release, stalled) = std::sync::mpsc::channel();
        let (done, finished) = std::sync::mpsc::channel();
        self.lock().stall_next_stop = Some((stalled, done));
        (release, finished)
    }

    pub fn started(&self) -> Vec<CaptureRequest> {
        self.lock().started.clone()
    }

    /// The number of captures that run now.
    pub fn running(&self) -> usize {
        let mut state = self.lock();
        state.live.retain(|live| {
            live.stop
                .upgrade()
                .is_some_and(|stop| !stop.load(Ordering::Acquire))
        });
        state.live.len()
    }

    /// Ends every capture that runs, as the system does when a shared window is closed.
    pub fn end_all(&self, error: &CaptureError) {
        let live = std::mem::take(&mut self.lock().live);
        for live in live {
            end(live, error);
        }
    }

    /// Ends the captures of one source, as the system does when that shared window is
    /// closed. The other captures go on.
    pub fn end_source(&self, source: PictureSource, error: &CaptureError) {
        let ended: Vec<Live> = {
            let mut state = self.lock();
            let (ended, kept) = std::mem::take(&mut state.live)
                .into_iter()
                .partition(|live| live.source == source);
            state.live = kept;
            ended
        };
        for live in ended {
            end(live, error);
        }
    }
}

/// The size of the picture of the windows of one application: the screen that it lists.
fn application_size(sources: &ShareSources, process_id: i32) -> Option<(u32, u32)> {
    sources
        .application_pictures
        .iter()
        .find(|listed| listed.application.process_id == process_id)
        .map(|listed| (listed.width, listed.height))
}

fn end(live: Live, error: &CaptureError) {
    if let Some(stop) = live.stop.upgrade() {
        stop.store(true, Ordering::Relaxed);
        live.sink.ended(error.clone());
    }
}

struct Running {
    stop: Arc<AtomicBool>,
    _workers: super::workers::Workers,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl ScreenCapture for SyntheticCapture {
    fn access(&self) -> ScreenAccess {
        self.lock().access
    }

    fn request_access(&self) {
        self.lock().asked += 1;
    }

    fn open_settings(&self) {
        self.lock().settings_opened += 1;
    }

    fn sources(&self) -> Pending<Result<ShareSources, CaptureError>> {
        let state = self.lock();
        let result = match state.access {
            ScreenAccess::Allowed | ScreenAccess::AskOnStart => Ok(state.sources.clone()),
            ScreenAccess::NotAllowed => Err(CaptureError::NotAllowed),
        };
        Box::pin(async move {
            crate::executor::require_runtime("the source list of the synthetic capture");
            result
        })
    }

    /// The picture of a source in the colours of the capture, with the left half of a
    /// screen or window of another identifier in another colour, so that two screens of
    /// one size give two pictures.
    fn preview(
        &self,
        source: PictureSource,
        bound: (u32, u32),
    ) -> Pending<Result<super::picture::PictureFrame, CaptureError>> {
        let capture = self.clone();
        Box::pin(async move {
            crate::executor::require_runtime("the preview of the synthetic capture");
            let delay = capture.lock().preview_delays.get(&source).copied();
            if let Some(delay) = delay {
                tokio::time::sleep(delay).await;
            }
            let mut state = capture.lock();
            if state.access == ScreenAccess::NotAllowed {
                return Err(CaptureError::NotAllowed);
            }
            state.previewed.push(source);
            let id = match source {
                PictureSource::Display(id) | PictureSource::Window(id) => id,
                PictureSource::Application(process_id) => process_id as u32,
                PictureSource::Portal(_) => return Err(CaptureError::Unsupported),
            };
            let (width, height) = state.size_of(source).ok_or(CaptureError::SourceGone)?;
            let (left, right) = state.colours;
            drop(state);
            let left = (
                left.0,
                left.1.wrapping_add((id as u8).wrapping_mul(40)),
                left.2,
            );
            let picture = two_colours(width, height, left, right);
            super::picture::convert(&picture, bound)
                .ok_or_else(|| CaptureError::Failed("the picture has no size".to_owned()))
        })
    }

    fn start(
        &self,
        request: CaptureRequest,
        sink: Arc<dyn CaptureSink>,
    ) -> Pending<Result<CaptureStream, CaptureError>> {
        let capture = self.clone();
        Box::pin(async move {
            crate::executor::require_runtime("the start of the synthetic capture");
            let delay = capture.lock().start_delay;
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            let mut state = capture.lock();
            if state.access == ScreenAccess::NotAllowed {
                return Err(CaptureError::NotAllowed);
            }
            if let Some(error) = state.fail_next.take() {
                return Err(error);
            }
            let (width, height) = state
                .size_of(request.picture)
                .ok_or(CaptureError::SourceGone)?;
            let tones = match &request.sound {
                SoundChoice::None => None,
                SoundChoice::System => Some(state.system_tones),
                SoundChoice::Application(application) => Some(
                    *state
                        .application_tones
                        .get(application)
                        .ok_or(CaptureError::SourceGone)?,
                ),
            };
            let resolution = request.quality.resolution;
            let (width, height) = resolution.fit(width, height);
            let frame_rate = request.quality.frame_rate.max(1);
            let picture = request.picture;
            state.started.push(request);
            let stall = state.stall_next_stop.take();
            let stop = Arc::new(AtomicBool::new(false));
            state.live.push(Live {
                stop: Arc::downgrade(&stop),
                sink: sink.clone(),
                source: picture,
            });
            drop(state);
            let stopped = stop.clone();
            let failures = sink.clone();
            let life = Arc::new(super::life::Life::default());
            let (mut workers, result) = super::workers::Workers::new(life.clone());
            workers.report_failures_to(failures);
            life.on_stop({
                let stop = stop.clone();
                move || stop.store(true, Ordering::Release)
            });
            workers.spawn("generated-share", move || {
                produce(
                    &capture,
                    &stopped,
                    sink.as_ref(),
                    (picture, resolution),
                    frame_rate,
                    tones,
                );
            })?;
            if let Some((stall, done)) = stall {
                let stop = stop.clone();
                workers.spawn("stalled-capture", move || {
                    let _ = stall.recv_timeout(Duration::from_secs(120));
                    drop(stop);
                    let _ = done.send(());
                })?;
            }
            let stream = CaptureStream::new(
                width,
                height,
                Running {
                    stop,
                    _workers: workers,
                },
            );
            Ok(stream.with_stop_result(result))
        })
    }
}

/// Gives pictures at the frame rate and sound in blocks of 10 ms, by the clock, until the
/// capture is stopped. Each picture has the size of the source as it is then, within the
/// resolution of the share, as a capture of the system follows the size of a window.
fn produce(
    capture: &SyntheticCapture,
    stop: &AtomicBool,
    sink: &dyn CaptureSink,
    (source, resolution): (PictureSource, super::Resolution),
    frame_rate: u32,
    tones: Option<Tones>,
) {
    let started = Instant::now();
    let (mut pictures, mut blocks) = (0u64, 0u64);
    let format = Format {
        sample_rate: SAMPLE_RATE,
        channels: 2,
    };
    let block = (SAMPLE_RATE / 100) as usize;
    let mut samples = vec![0.0f32; block * 2];
    while !stop.load(Ordering::Relaxed) {
        let elapsed = started.elapsed();
        let pictures_due = 1 + elapsed.as_millis() as u64 * u64::from(frame_rate) / 1000;
        if pictures < pictures_due {
            let (colours, still, size) = {
                let state = capture.lock();
                let colours = state.source_colours.get(&source).copied();
                (
                    colours.unwrap_or(state.colours),
                    state.still,
                    state.size_of(source),
                )
            };
            if let Some((width, height)) = size
                && (pictures == 0 || !still)
            {
                let (width, height) = resolution.fit(width, height);
                let picture = two_colours(width, height, colours.0, colours.1);
                sink.picture(Arc::new(picture));
            }
            pictures = pictures_due;
        }
        if let Some((left, right)) = tones {
            let blocks_due = elapsed.as_millis() as u64 / 10;
            while blocks < blocks_due {
                for (index, pair) in samples.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                    let at = (blocks * block as u64 + index as u64) as f64 / f64::from(SAMPLE_RATE);
                    let turn = at * std::f64::consts::TAU;
                    pair[0] = AMPLITUDE * (turn * f64::from(left)).sin() as f32;
                    pair[1] = AMPLITUDE * (turn * f64::from(right)).sin() as f32;
                }
                sink.sound(&samples, format);
                blocks += 1;
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::share::ShareQuality;
    struct Sink;
    impl CaptureSink for Sink {
        fn picture(&self, _: Arc<dyn livekit::webrtc::video_frame::VideoBuffer>) {}
        fn sound(&self, _: &[f32], _: Format) {}
        fn ended(&self, _: CaptureError) {}
    }
    #[tokio::test]
    async fn a_cancelled_capture_is_not_counted_while_its_worker_is_still_held() {
        let capture = SyntheticCapture::new();
        let (release, finished) = capture.stall_next_stop();
        let stream = capture
            .start(
                CaptureRequest {
                    picture: PictureSource::Display(1),
                    sound: SoundChoice::None,
                    quality: ShareQuality::default(),
                    dialog_parent: None,
                },
                Arc::new(Sink),
            )
            .await
            .unwrap();
        assert_eq!(capture.running(), 1);
        drop(stream);
        let count = capture.running();
        let premature = finished.try_recv();
        release.send(()).unwrap();
        finished.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(premature.is_err());
        assert_eq!(
            count, 0,
            "a cancelled capture was counted because its worker was alive"
        );
    }
}
