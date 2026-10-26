//! Screen share (ADR 0502, ADR 0736): the capture of a screen or a window with its sound.
//!
//! The engine takes pictures and sound through one interface, [`ScreenCapture`]. The
//! product implements it with the capture of the operating system; the tests implement it
//! with a picture and a tone that the program makes. The engine knows no system.

#[cfg(any(target_os = "linux", target_os = "windows", test))]
pub(crate) mod delivery;
#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(any(target_os = "macos", target_os = "windows", test))]
pub(crate) mod listing;
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
pub mod macos;
pub mod picture;
pub mod sound;
#[cfg(feature = "test-support")]
pub mod synthetic;
#[cfg(any(target_os = "windows", test))]
pub mod windows;

use std::any::Any;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use junto_client_core::protocol::call::{MAX_SHARES, ShareAudio};
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
    /// The owning process on macOS and Windows; zero for a Linux graph identity.
    pub process_id: i32,
    /// A macOS bundle identifier, the path of the program file in lower case on Windows, or
    /// the stable application identity of the Linux graph.
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
    /// Applications of listed windows whose sound includes the sound of the client, so that
    /// their sound is not offered alone. On Windows these are the applications whose
    /// process started the client, directly or through other processes (ADR 0706); the
    /// other systems list none.
    pub started_the_client: Vec<ApplicationId>,
    /// On Windows, the processes that started the client could not be read, so that any
    /// application can have started it: no application is offered for its sound alone
    /// (ADR 0706). The other systems state false.
    pub creators_unknown: bool,
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

/// The largest picture of a source in the dialog of a share.
pub const PREVIEW_BOUND: (u32, u32) = (320, 180);
/// A preview that the system has not made after this time fails, so that one window of an
/// application that does not answer does not hold the previews after it.
pub const PREVIEW_LIMIT: std::time::Duration = std::time::Duration::from_secs(3);

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
    /// The sound of the requested application includes the sound of the client: on
    /// Windows, the application started the client (ADR 0706).
    #[error("the sound of this application includes the sound of Junto")]
    StartedTheClient,
    /// Windows, the processes that started the client could not be read (ADR 0706).
    #[error("the applications that started Junto could not be read")]
    CreatorsUnknown,
    /// This build has no capture for the system that it runs on.
    #[error("screen share is not available on this system")]
    Unsupported,
    /// The media connection that carried the share ended.
    #[error("the media connection of the call ended")]
    Interrupted,
    /// The person runs as many shares as one person can (`MAX_SHARES`).
    #[error("a person runs at most {} shares at once", MAX_SHARES)]
    TooMany,
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

    /// One still picture of a listed screen or window for the dialog in which the person
    /// chooses what to share, at most `bound` pixels with the proportions of the source.
    /// A system without such a picture answers [`CaptureError::Unsupported`]: on Linux the
    /// desktop portal shows its own dialog (ADR 0736).
    fn preview(
        &self,
        source: PictureSource,
        bound: (u32, u32),
    ) -> Pending<Result<picture::PictureFrame, CaptureError>> {
        let _ = (source, bound);
        Box::pin(async { Err(CaptureError::Unsupported) })
    }

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
    #[cfg(target_os = "windows")]
    {
        Arc::new(windows::SystemCapture::new())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        Arc::new(NoCapture)
    }
}

/// What the name of the picture track of a share states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrackName {
    pub kind: ShareKind,
    pub frame_rate: u32,
    /// The slot of the share among the shares of its person.
    pub slot: u8,
}

/// The name of the track of a share picture. It states what is shared, at which frame
/// rate and in which slot: `screen/30` for slot 0, which a client of one share reads, and
/// `window/30/1` for slot 1 (ADR 0737). The server reads the slot from it (ADR 0510).
pub fn track_name(kind: ShareKind, frame_rate: u32, slot: u8) -> String {
    let kind = match kind {
        ShareKind::Screen => "screen",
        ShareKind::Window => "window",
    };
    match slot {
        0 => format!("{kind}/{frame_rate}"),
        slot => format!("{kind}/{frame_rate}/{slot}"),
    }
}

/// Reads [`track_name`]. A name of another form, as another client gives it, states
/// nothing; its track is the share of slot 0.
pub fn read_track_name(name: &str) -> Option<TrackName> {
    let mut parts = name.split('/');
    let kind = match parts.next()? {
        "screen" => ShareKind::Screen,
        "window" => ShareKind::Window,
        _ => return None,
    };
    let frame_rate = parts
        .next()?
        .parse()
        .ok()
        .filter(|rate| (1..=240).contains(rate))?;
    let slot = match parts.next() {
        None => 0,
        Some(slot) => slot.parse().ok().filter(|slot| *slot < MAX_SHARES)?,
    };
    if parts.next().is_some() {
        return None;
    }
    Some(TrackName {
        kind,
        frame_rate,
        slot,
    })
}

/// The name of the sound track of a share: `share-sound` for slot 0, as a client of one
/// share names it, and `share-sound/1` for slot 1.
pub fn sound_name(slot: u8) -> String {
    match slot {
        0 => "share-sound".to_owned(),
        slot => format!("share-sound/{slot}"),
    }
}

/// The slot of the share of a sound track. Every name other than [`sound_name`] of a slot
/// is slot 0.
pub fn read_sound_name(name: &str) -> u8 {
    name.strip_prefix("share-sound/")
        .and_then(|slot| slot.parse().ok())
        .filter(|slot| *slot < MAX_SHARES)
        .unwrap_or(0)
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
    fn the_name_of_a_share_track_states_its_kind_its_frame_rate_and_its_slot() {
        let named = |kind, frame_rate, slot| TrackName {
            kind,
            frame_rate,
            slot,
        };
        // Slot 0 keeps the name that a client of one share reads.
        assert_eq!(track_name(ShareKind::Screen, 30, 0), "screen/30");
        assert_eq!(
            read_track_name("screen/30"),
            Some(named(ShareKind::Screen, 30, 0))
        );
        assert_eq!(track_name(ShareKind::Window, 60, 2), "window/60/2");
        assert_eq!(
            read_track_name(&track_name(ShareKind::Window, 60, 2)),
            Some(named(ShareKind::Window, 60, 2))
        );
        for other in [
            "",
            "screen",
            "camera/30",
            "screen/0",
            "screen/fast",
            "window/999",
            "screen/30/3",
            "screen/30/x",
            "screen/30/1/1",
        ] {
            assert_eq!(read_track_name(other), None, "{other}");
        }
        assert_eq!(sound_name(0), "share-sound");
        assert_eq!(sound_name(1), "share-sound/1");
        for (name, slot) in [
            ("share-sound", 0),
            ("share-sound/2", 2),
            ("share-sound/3", 0),
            ("other", 0),
        ] {
            assert_eq!(read_sound_name(name), slot, "{name}");
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
