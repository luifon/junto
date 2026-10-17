//! A capture that the program makes itself, for tests and for the screen tool: a picture
//! of two known colours and a tone on each channel. It reads no screen and no sound of the
//! machine.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use livekit::webrtc::video_frame::I420Buffer;

use super::{
    ApplicationId, ApplicationSource, CaptureError, CaptureRequest, CaptureSink, CaptureStream,
    DisplaySource, Pending, PictureSource, ScreenAccess, ScreenCapture, ShareSources, SoundChoice,
    WindowSource,
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
    system_tones: Tones,
    application_tones: HashMap<ApplicationId, Tones>,
    /// A capture gives one picture and then none, as the capture of a screen that does
    /// not change.
    still: bool,
    /// A capture takes this long to start, as a system that answers late.
    start_delay: Duration,
    fail_next: Option<CaptureError>,
    asked: u32,
    settings_opened: u32,
    started: Vec<CaptureRequest>,
    live: Vec<(Weak<AtomicBool>, Arc<dyn CaptureSink>)>,
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
    /// One screen of 1280 × 720, one window of 800 × 600 of the application "Player", and
    /// the permission of the system.
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
                    applications: vec![ApplicationSource {
                        id: player.clone(),
                        name: "Player".to_owned(),
                    }],
                },
                colours: ((200, 30, 30), (30, 30, 200)),
                system_tones: (700.0, 1_900.0),
                application_tones: HashMap::from([(player, (1_100.0, 2_700.0))]),
                still: false,
                start_delay: Duration::ZERO,
                fail_next: None,
                asked: 0,
                settings_opened: 0,
                started: Vec::new(),
                live: Vec::new(),
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

    pub fn set_sources(&self, sources: ShareSources) {
        self.lock().sources = sources;
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

    /// The requests of the captures that were started.
    pub fn started(&self) -> Vec<CaptureRequest> {
        self.lock().started.clone()
    }

    /// The number of captures that run now.
    pub fn running(&self) -> usize {
        let mut state = self.lock();
        state.live.retain(|(stop, _)| stop.strong_count() > 0);
        state.live.len()
    }

    /// Ends every capture that runs, as the system does when a shared window is closed.
    pub fn end_all(&self, error: &CaptureError) {
        let live = std::mem::take(&mut self.lock().live);
        for (stop, sink) in live {
            if let Some(stop) = stop.upgrade() {
                stop.store(true, Ordering::Relaxed);
                sink.ended(error.clone());
            }
        }
    }
}

struct Running {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
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
            let source = match request.picture {
                PictureSource::Portal(kind) => {
                    state.sources.portal.contains(&kind).then_some((1280, 720))
                }
                PictureSource::Display(id) => state
                    .sources
                    .displays
                    .iter()
                    .find(|display| display.id == id)
                    .map(|display| (display.width, display.height)),
                PictureSource::Window(id) => state
                    .sources
                    .windows
                    .iter()
                    .find(|window| window.id == id)
                    .map(|window| (window.width, window.height)),
            };
            let (width, height) = source.ok_or(CaptureError::SourceGone)?;
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
            let (width, height) = request.quality.resolution.fit(width, height);
            let frame_rate = request.quality.frame_rate.max(1);
            state.started.push(request);
            let stop = Arc::new(AtomicBool::new(false));
            state.live.push((Arc::downgrade(&stop), sink.clone()));
            drop(state);
            let stopped = stop.clone();
            let thread = std::thread::spawn(move || {
                produce(
                    &capture,
                    &stopped,
                    sink.as_ref(),
                    (width, height),
                    frame_rate,
                    tones,
                );
            });
            Ok(CaptureStream::new(
                width,
                height,
                Running {
                    stop,
                    thread: Some(thread),
                },
            ))
        })
    }
}

/// Gives pictures at the frame rate and sound in blocks of 10 ms, by the clock, until the
/// capture is stopped.
fn produce(
    capture: &SyntheticCapture,
    stop: &AtomicBool,
    sink: &dyn CaptureSink,
    size: (u32, u32),
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
            let (colours, still) = {
                let state = capture.lock();
                (state.colours, state.still)
            };
            if pictures == 0 || !still {
                let picture = two_colours(size.0, size.1, colours.0, colours.1);
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
