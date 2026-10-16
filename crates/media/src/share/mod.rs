//! Screen share (ADR 0502, ADR 0736): the capture of a screen or a window with its sound.
//!
//! The engine takes pictures and sound through one interface, [`ScreenCapture`]. The
//! product implements it with the capture of the operating system; the tests implement it
//! with a picture and a tone that the program makes. The engine knows no system.

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
pub mod macos;
pub mod picture;
pub mod sound;
#[cfg(feature = "test-support")]
pub mod synthetic;

use std::any::Any;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use junto_client_core::protocol::call::ShareAudio;
use livekit::webrtc::video_frame::VideoBuffer;

use crate::audio::Format;

/// Whether the system lets this application record the screen and the sound of other
/// applications. macOS does not tell a permission that was never asked for from one that
/// was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenAccess {
    Allowed,
    NotAllowed,
    /// The desktop asks for authorisation when a share starts.
    AskOnStart,
}

/// An application as the system identifies it for a capture.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ApplicationId {
    /// The owning process on macOS; zero for a Linux graph identity.
    pub process_id: i32,
    /// A macOS bundle identifier, or the stable application identity of the Linux graph.
    pub bundle_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplaySource {
    pub id: u32,
    /// The size in pixels.
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowSource {
    pub id: u32,
    pub title: String,
    pub application: ApplicationId,
    pub application_name: String,
    /// The size in pixels.
    pub width: u32,
    pub height: u32,
}

/// An application whose sound can be shared alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplicationSource {
    pub id: ApplicationId,
    pub name: String,
}

/// What a person can share now. Application-owned lists exclude the client itself.
/// A desktop portal owns its chooser and cannot be given an exclusion list (ADR 0751).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ShareSources {
    pub displays: Vec<DisplaySource>,
    pub windows: Vec<WindowSource>,
    pub applications: Vec<ApplicationSource>,
    /// Source kinds chosen by the desktop at start, without application-owned IDs.
    pub portal: Vec<ShareKind>,
    pub system_sound: bool,
}

/// The picture of a share.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PictureSource {
    Display(u32),
    Window(u32),
    Portal(ShareKind),
}

/// What a share shows: a whole screen, or one window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShareKind {
    Screen,
    Window,
}

impl PictureSource {
    pub const fn kind(self) -> ShareKind {
        match self {
            Self::Display(_) => ShareKind::Screen,
            Self::Window(_) => ShareKind::Window,
            Self::Portal(kind) => kind,
        }
    }
}

/// The sound of a share, as the sharer chose it (ADR 0502). Nothing is chosen for the
/// person, and no choice is replaced by another one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SoundChoice {
    None,
    /// All system sound, without the sound of the client itself.
    System,
    /// The sound of one application.
    Application(ApplicationId),
}

impl SoundChoice {
    /// The choice as the server and the viewers are told it.
    pub const fn reported(&self) -> ShareAudio {
        match self {
            Self::None => ShareAudio::None,
            Self::System => ShareAudio::System,
            Self::Application(_) => ShareAudio::Application,
        }
    }
}

/// The largest picture that a share sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Resolution {
    /// At most 1280 × 720.
    P720,
    /// At most 1920 × 1080.
    #[default]
    P1080,
    /// The size of the source.
    Source,
}

impl Resolution {
    /// The size of the picture of a source of `width` × `height` pixels: the source is
    /// made smaller until it fits, keeps its proportions, and has even sides, which the
    /// video encoders need. A source is never made larger.
    pub fn fit(self, width: u32, height: u32) -> (u32, u32) {
        let bound = match self {
            Self::P720 => Some((1280, 720)),
            Self::P1080 => Some((1920, 1080)),
            Self::Source => None,
        };
        let (width, height) = (width.max(2), height.max(2));
        let (fit_width, fit_height) = match bound {
            Some(bound) => picture::fit((width, height), bound),
            None => (width, height),
        };
        ((fit_width & !1).max(2), (fit_height & !1).max(2))
    }
}

/// The frame rates that a person chooses from.
pub const FRAME_RATES: [u32; 3] = [15, 30, 60];
pub const DEFAULT_FRAME_RATE: u32 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShareQuality {
    pub resolution: Resolution,
    /// Pictures per second, one of [`FRAME_RATES`].
    pub frame_rate: u32,
}

impl Default for ShareQuality {
    fn default() -> Self {
        Self {
            resolution: Resolution::default(),
            frame_rate: DEFAULT_FRAME_RATE,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureRequest {
    pub picture: PictureSource,
    pub sound: SoundChoice,
    pub quality: ShareQuality,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CaptureError {
    #[error("screen sharing was cancelled")]
    Cancelled,
    /// The system does not let this application record the screen.
    #[error("the system does not allow this application to record the screen")]
    NotAllowed,
    /// The screen, the window or the application of the request is not there any more.
    #[error("the screen, the window or the application is not available any more")]
    SourceGone,
    /// This build has no capture for the system that it runs on.
    #[error("screen share is not available on this system")]
    Unsupported,
    /// The media connection that carried the share ended.
    #[error("the media connection of the call ended")]
    Interrupted,
    #[error("the capture failed: {0}")]
    Failed(String),
}

/// What a capture gives. The calls come from threads of the capture, in the order of the
/// pictures and of the sound.
pub trait CaptureSink: Send + Sync + 'static {
    /// One picture of the screen or the window.
    fn picture(&self, buffer: Arc<dyn VideoBuffer>);

    /// Interleaved samples of the sound of the share. A capture without sound never calls
    /// this.
    fn sound(&self, samples: &[f32], format: Format);

    /// The capture ended by itself: the window was closed, or the system stopped it. No
    /// further call follows.
    fn ended(&self, error: CaptureError);
}

/// A capture that runs. It ends when this value is dropped.
pub struct CaptureStream {
    /// The size of the pictures in pixels.
    pub width: u32,
    pub height: u32,
    _stream: Box<dyn Any + Send>,
}

impl CaptureStream {
    pub fn new(width: u32, height: u32, stream: impl Any + Send) -> Self {
        Self {
            width,
            height,
            _stream: Box::new(stream),
        }
    }
}

impl fmt::Debug for CaptureStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CaptureStream")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

pub type Pending<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// The capture of screens, windows and their sound on one system. The crate calls the
/// functions that return a future, and polls those futures, only in tasks of the runtime of
/// the client (ADR 0733); the window reads the sources through [`crate::Calls`].
pub trait ScreenCapture: Send + Sync + 'static {
    /// Whether this build has a capture for the system that it runs on.
    fn supported(&self) -> bool {
        true
    }

    /// The permission of the system now. The call shows no question.
    fn access(&self) -> ScreenAccess;

    /// Makes the system list this application among the ones that ask for the permission,
    /// and show its question when it has not shown it before. The person gives the
    /// permission in the settings of the system (ADR 0502).
    fn request_access(&self);

    /// Opens the part of the settings of the system that holds the permission.
    fn open_settings(&self);

    /// The screens, windows and applications that can be shared now.
    fn sources(&self) -> Pending<Result<ShareSources, CaptureError>>;

    /// Starts a capture. Pictures and sound go to `sink` until the stream is dropped or
    /// the capture ends by itself.
    fn start(
        &self,
        request: CaptureRequest,
        sink: Arc<dyn CaptureSink>,
    ) -> Pending<Result<CaptureStream, CaptureError>>;
}

/// A system without a capture in this build.
pub struct NoCapture;

impl ScreenCapture for NoCapture {
    fn supported(&self) -> bool {
        false
    }

    fn access(&self) -> ScreenAccess {
        ScreenAccess::NotAllowed
    }

    fn request_access(&self) {}

    fn open_settings(&self) {}

    fn sources(&self) -> Pending<Result<ShareSources, CaptureError>> {
        Box::pin(async { Err(CaptureError::Unsupported) })
    }

    fn start(
        &self,
        _request: CaptureRequest,
        _sink: Arc<dyn CaptureSink>,
    ) -> Pending<Result<CaptureStream, CaptureError>> {
        Box::pin(async { Err(CaptureError::Unsupported) })
    }
}

/// The capture of the system that this build runs on.
pub fn system() -> Arc<dyn ScreenCapture> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(macos::SystemCapture::new())
    }
    #[cfg(target_os = "linux")]
    {
        Arc::new(linux::SystemCapture)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Arc::new(NoCapture)
    }
}

/// The name of the track of a share picture. It states what is shared and at which frame
/// rate, because the record of a share on the server holds only its sound (ADR 0737).
pub fn track_name(kind: ShareKind, frame_rate: u32) -> String {
    let kind = match kind {
        ShareKind::Screen => "screen",
        ShareKind::Window => "window",
    };
    format!("{kind}/{frame_rate}")
}

/// Reads [`track_name`]. A name of another form, as another client gives it, states
/// nothing.
pub fn read_track_name(name: &str) -> Option<(ShareKind, u32)> {
    let (kind, rate) = name.split_once('/')?;
    let kind = match kind {
        "screen" => ShareKind::Screen,
        "window" => ShareKind::Window,
        _ => return None,
    };
    let rate = rate.parse().ok().filter(|rate| (1..=240).contains(rate))?;
    Some((kind, rate))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_fits_its_bound_keeps_its_proportions_and_has_even_sides() {
        assert_eq!(Resolution::P1080.fit(1920, 1080), (1920, 1080));
        assert_eq!(Resolution::P1080.fit(3024, 1964), (1662, 1080));
        assert_eq!(Resolution::P720.fit(3024, 1964), (1108, 720));
        // A tall window is bound by its height, a wide one by its width.
        assert_eq!(Resolution::P720.fit(1000, 2000), (360, 720));
        assert_eq!(Resolution::P720.fit(4000, 500), (1280, 160));
        // A source is never made larger, and odd sides become even.
        assert_eq!(Resolution::P1080.fit(641, 481), (640, 480));
        assert_eq!(Resolution::Source.fit(3025, 1965), (3024, 1964));
        assert_eq!(Resolution::Source.fit(0, 1), (2, 2));
    }

    #[test]
    fn the_name_of_a_share_track_states_its_kind_and_its_frame_rate() {
        assert_eq!(track_name(ShareKind::Screen, 30), "screen/30");
        assert_eq!(read_track_name("screen/30"), Some((ShareKind::Screen, 30)));
        assert_eq!(
            read_track_name(&track_name(ShareKind::Window, 60)),
            Some((ShareKind::Window, 60))
        );
        for other in [
            "",
            "screen",
            "camera/30",
            "screen/0",
            "screen/fast",
            "window/999",
        ] {
            assert_eq!(read_track_name(other), None, "{other}");
        }
    }

    #[test]
    fn the_sound_choice_is_reported_as_the_server_names_it() {
        let application = ApplicationId {
            process_id: 7,
            bundle_id: "example.player".to_owned(),
        };
        assert_eq!(SoundChoice::None.reported(), ShareAudio::None);
        assert_eq!(SoundChoice::System.reported(), ShareAudio::System);
        assert_eq!(
            SoundChoice::Application(application).reported(),
            ShareAudio::Application
        );
    }
}
