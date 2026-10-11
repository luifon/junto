//! The capture of macOS: ScreenCaptureKit (ADR 0502, ADR 0736).
//!
//! A share uses two streams. The stream of the picture captures a screen or one window and
//! no sound. The stream of the sound has the configuration that the step 0 test ran: a
//! filter that selects the sound, 48 kHz stereo, the sound of this process excluded, and a
//! picture of 16 × 16 pixels that nobody reads. The two are apart, so the person chooses
//! the sound whatever the picture is.
//!
//! The functions that decide something are plain functions with tests: which application
//! is the client itself, which windows are listed, and how a sound buffer of the system
//! becomes interleaved samples. The calls to the system are not run by a test: a test
//! captures no screen and causes no permission question (ADR 0902).

use std::ffi::c_void;
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use block2::RcBlock;
use livekit::webrtc::video_frame::VideoBuffer;
use livekit::webrtc::video_frame::native::NativeBuffer;
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use objc2_core_audio_types::{AudioBuffer, AudioBufferList, AudioStreamBasicDescription};
use objc2_core_foundation::CFRetained;
use objc2_core_graphics::{
    CGDisplayCopyDisplayMode, CGDisplayMode, CGMainDisplayID, CGPreflightScreenCaptureAccess,
    CGRequestScreenCaptureAccess,
};
use objc2_core_media::{
    CMAudioFormatDescriptionGetStreamBasicDescription, CMBlockBuffer, CMSampleBuffer, CMTime,
};
use objc2_foundation::{NSArray, NSBundle, NSError, NSProcessInfo};
use objc2_screen_capture_kit::{
    SCContentFilter, SCDisplay, SCRunningApplication, SCShareableContent, SCStream,
    SCStreamConfiguration, SCStreamDelegate, SCStreamOutput, SCStreamOutputType,
};
use tokio::sync::oneshot;

use super::{
    ApplicationId, ApplicationSource, CaptureError, CaptureRequest, CaptureSink, CaptureStream,
    DisplaySource, Pending, PictureSource, ScreenAccess, ScreenCapture, ShareSources, SoundChoice,
    WindowSource,
};
use crate::audio::Format;

/// The pixel format of the pictures: two planes of brightness and colour in video range,
/// which the video encoders of the system take without a conversion (`420v`).
const PIXEL_FORMAT: u32 = u32::from_be_bytes(*b"420v");
/// The format of linear PCM in an audio description (`lpcm`).
const LINEAR_PCM: u32 = u32::from_be_bytes(*b"lpcm");
const FLAG_FLOAT: u32 = 1 << 0;
const FLAG_NOT_INTERLEAVED: u32 = 1 << 5;
/// The code with which the system ends a stream that the person stopped.
const USER_STOPPED: isize = -3817;
/// The code of a capture that the system does not allow.
const USER_DECLINED: isize = -3801;
/// A window is listed when each side has at least this many points. Smaller ones are
/// parts of an interface that a system or an application draws as windows.
const SMALLEST_WINDOW: f64 = 64.0;
/// How long the system has to answer a request for its lists and for the start of a
/// stream. Without an answer the share does not start, and nothing keeps running.
const ANSWER_LIMIT: std::time::Duration = std::time::Duration::from_secs(10);
/// The part of the settings of the system that holds the permission.
const SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";

/// What the client is to the system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Own {
    pub process_id: i32,
    pub bundle_id: String,
    pub name: String,
}

impl Own {
    fn read() -> Self {
        let bundle_id = NSBundle::mainBundle()
            .bundleIdentifier()
            .map(|id| id.to_string())
            .unwrap_or_default();
        Self {
            process_id: std::process::id() as i32,
            bundle_id,
            name: NSProcessInfo::processInfo().processName().to_string(),
        }
    }

    /// Whether an application is the client: this process, or another start of the same
    /// program. The exclusion of the own sound has no effect on a capture that selects an
    /// application of the same program, so none of them is offered (ADR 0502).
    pub(crate) fn is(&self, process_id: i32, bundle_id: &str, name: &str) -> bool {
        process_id == self.process_id || (bundle_id == self.bundle_id && name == self.name)
    }
}

/// A window as the system lists it, before the rule of the list is applied.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ListedWindow {
    pub id: u32,
    pub title: String,
    pub layer: isize,
    pub on_screen: bool,
    /// The size in points.
    pub size: (f64, f64),
    /// The pixels of one point on the screen that shows the window.
    pub scale: f64,
    pub application: Option<(ApplicationId, String)>,
}

/// The windows that a person can share: ordinary windows on a screen that belong to
/// another application than the client.
pub(crate) fn shareable_windows(listed: Vec<ListedWindow>, own: &Own) -> Vec<WindowSource> {
    listed
        .into_iter()
        .filter(|window| window.layer == 0 && window.on_screen)
        .filter(|window| window.size.0 >= SMALLEST_WINDOW && window.size.1 >= SMALLEST_WINDOW)
        .filter_map(|window| {
            let (application, application_name) = window.application?;
            if own.is(
                application.process_id,
                &application.bundle_id,
                &application_name,
            ) {
                return None;
            }
            Some(WindowSource {
                id: window.id,
                title: window.title,
                application,
                application_name,
                width: (window.size.0 * window.scale).round() as u32,
                height: (window.size.1 * window.scale).round() as u32,
            })
        })
        .collect()
}

/// The applications whose sound can be shared alone: every listed application with a name
/// except the client, once each.
pub(crate) fn shareable_applications(
    listed: Vec<(ApplicationId, String)>,
    own: &Own,
) -> Vec<ApplicationSource> {
    let mut applications: Vec<ApplicationSource> = Vec::new();
    for (id, name) in listed {
        if name.is_empty()
            || own.is(id.process_id, &id.bundle_id, &name)
            || applications.iter().any(|held| held.id == id)
        {
            continue;
        }
        applications.push(ApplicationSource { id, name });
    }
    applications.sort_by(|one, other| {
        one.name
            .to_lowercase()
            .cmp(&other.name.to_lowercase())
            .then(one.id.process_id.cmp(&other.id.process_id))
    });
    applications
}

/// The format of the samples of a sound buffer, as its description states it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SoundDescription {
    pub format_id: u32,
    pub flags: u32,
    pub sample_rate: f64,
    pub channels: u32,
    pub bits: u32,
}

/// Writes the samples of a sound buffer of the system into `out` as interleaved values
/// from -1 to 1. `planes` holds the bytes of each buffer of the list: one buffer per
/// channel, or one buffer with every channel. Returns the format, or nothing for a form
/// that the capture does not read.
pub(crate) fn interleave(
    description: SoundDescription,
    planes: &[&[u8]],
    out: &mut Vec<f32>,
) -> Option<Format> {
    out.clear();
    let channels = usize::try_from(description.channels).ok()?;
    let sample_rate = description.sample_rate;
    if description.format_id != LINEAR_PCM
        || !(1..=8).contains(&channels)
        || !(8_000.0..=192_000.0).contains(&sample_rate)
    {
        return None;
    }
    let float = description.flags & FLAG_FLOAT != 0;
    let width = match (float, description.bits) {
        (true, 32) => 4,
        (false, 16) => 2,
        _ => return None,
    };
    let read = |bytes: &[u8]| -> f32 {
        match (float, bytes) {
            (true, [a, b, c, d]) => f32::from_ne_bytes([*a, *b, *c, *d]),
            (false, [a, b]) => f32::from(i16::from_ne_bytes([*a, *b])) / f32::from(i16::MAX),
            _ => 0.0,
        }
    };
    if description.flags & FLAG_NOT_INTERLEAVED != 0 {
        if planes.len() != channels {
            return None;
        }
        let frames = planes.iter().map(|plane| plane.len() / width).min()?;
        out.reserve(frames * channels);
        for frame in 0..frames {
            for plane in planes {
                out.push(read(&plane[frame * width..(frame + 1) * width]));
            }
        }
    } else {
        let [plane] = planes else {
            return None;
        };
        let samples = plane.len() / width / channels * channels;
        out.extend(plane.chunks_exact(width).take(samples).map(read));
    }
    Some(Format {
        sample_rate: sample_rate.round() as u32,
        channels: channels as u16,
    })
}

struct OutputIvars {
    sink: Arc<dyn CaptureSink>,
    /// The sink was told the end of the capture.
    ended: Arc<AtomicBool>,
    /// The stream shows one window. When the system tells that the window is gone, the
    /// share has ended.
    one_window: bool,
    /// The samples of one sound buffer, kept between the calls of one stream.
    samples: Mutex<Vec<f32>>,
}

define_class!(
    // SAFETY: `NSObject` has no subclassing requirements, and `Output` does not implement
    // `Drop`; `define_class!` manages the destruction of its ivars.
    #[unsafe(super(NSObject))]
    #[ivars = OutputIvars]
    struct Output;

    unsafe impl NSObjectProtocol for Output {}

    unsafe impl SCStreamOutput for Output {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn stream_did_output_sample_buffer_of_type(
            &self,
            _stream: &SCStream,
            sample: &CMSampleBuffer,
            kind: SCStreamOutputType,
        ) {
            if self.ivars().ended.load(Ordering::Relaxed) {
                return;
            }
            if kind == SCStreamOutputType::Screen {
                self.picture(sample);
            } else if kind == SCStreamOutputType::Audio {
                self.sound(sample);
            }
        }
    }

    unsafe impl SCStreamDelegate for Output {
        #[unsafe(method(stream:didStopWithError:))]
        fn stream_did_stop_with_error(&self, _stream: &SCStream, error: &NSError) {
            let reason = match error.code() {
                USER_STOPPED => CaptureError::SourceGone,
                USER_DECLINED => CaptureError::NotAllowed,
                _ => CaptureError::Failed(error.localizedDescription().to_string()),
            };
            if !self.ivars().ended.swap(true, Ordering::Relaxed) {
                self.ivars().sink.ended(reason);
            }
        }

        // The system tells this when every window of the stream was closed (macOS 15.2
        // and later). The stream itself goes on; the share of a window does not.
        #[unsafe(method(streamDidBecomeInactive:))]
        fn stream_did_become_inactive(&self, _stream: &SCStream) {
            if self.ivars().one_window && !self.ivars().ended.swap(true, Ordering::Relaxed) {
                self.ivars().sink.ended(CaptureError::SourceGone);
            }
        }
    }
);

impl Output {
    fn new(sink: Arc<dyn CaptureSink>, ended: Arc<AtomicBool>, one_window: bool) -> Retained<Self> {
        let this = Self::alloc().set_ivars(OutputIvars {
            sink,
            ended,
            one_window,
            samples: Mutex::new(Vec::new()),
        });
        // SAFETY: the ivars are set, and `NSObject` is initialized with `init`.
        unsafe { msg_send![super(this), init] }
    }

    fn picture(&self, sample: &CMSampleBuffer) {
        // A sample without a picture tells that the screen did not change.
        // SAFETY: the sample buffer is valid for the time of this call, and the method
        // returns a retained reference to its image buffer.
        let Some(image) = (unsafe { sample.image_buffer() }) else {
            return;
        };
        let image: NonNull<c_void> = CFRetained::into_raw(image).cast();
        // SAFETY: the pointer is a pixel buffer with one reference that this call gives
        // away: the call library takes its own reference and releases the given one.
        let buffer = unsafe { NativeBuffer::from_cv_pixel_buffer(image.as_ptr()) };
        let buffer: Arc<dyn VideoBuffer> = Arc::new(buffer);
        self.ivars().sink.picture(buffer);
    }

    fn sound(&self, sample: &CMSampleBuffer) {
        let Ok(mut samples) = self.ivars().samples.lock() else {
            return;
        };
        // SAFETY: the sample buffer is valid for the time of this call.
        let format = unsafe { read_sound(sample, &mut samples) };
        if let Some(format) = format {
            self.ivars().sink.sound(&samples, format);
        }
    }
}

/// Reads the samples of a sound buffer of the system.
///
/// # Safety
///
/// `sample` is a valid sample buffer.
unsafe fn read_sound(sample: &CMSampleBuffer, out: &mut Vec<f32>) -> Option<Format> {
    // SAFETY: the description belongs to the sample buffer, and the pointer into it is
    // read while the retained description lives.
    let description = unsafe {
        let description = sample.format_description()?;
        let basic: *const AudioStreamBasicDescription =
            CMAudioFormatDescriptionGetStreamBasicDescription(&description);
        let basic = basic.as_ref()?;
        SoundDescription {
            format_id: basic.mFormatID,
            flags: basic.mFormatFlags,
            sample_rate: basic.mSampleRate,
            channels: basic.mChannelsPerFrame,
            bits: basic.mBitsPerChannel,
        }
    };
    // The list of buffers has one entry per channel, so its size is asked for first.
    let mut needed = 0usize;
    // SAFETY: without a list, the call only writes the size that a list needs.
    let status = unsafe {
        sample.audio_buffer_list_with_retained_block_buffer(
            &mut needed,
            ptr::null_mut(),
            0,
            None,
            None,
            0,
            ptr::null_mut(),
        )
    };
    let header = std::mem::size_of::<AudioBufferList>();
    if status != 0 || needed < header || needed > header + 64 * std::mem::size_of::<AudioBuffer>() {
        return None;
    }
    // Words of eight bytes give the list the alignment of its pointers.
    let mut storage = vec![0u64; needed.div_ceil(8)];
    let list: *mut AudioBufferList = storage.as_mut_ptr().cast();
    let mut block: *mut CMBlockBuffer = ptr::null_mut();
    // SAFETY: `list` points to `needed` writable bytes with the alignment of the list, and
    // `block` receives a retained block buffer that holds the samples.
    let status = unsafe {
        sample.audio_buffer_list_with_retained_block_buffer(
            ptr::null_mut(),
            list,
            needed,
            None,
            None,
            0,
            &mut block,
        )
    };
    // SAFETY: a block buffer that the call returned is retained once for the caller. It
    // is released when `held` is dropped, after the last read of the samples.
    let held = NonNull::new(block).map(|block| unsafe { CFRetained::from_raw(block) });
    if status != 0 {
        return None;
    }
    // SAFETY: the call filled the list: `mNumberBuffers` entries follow its header inside
    // the `needed` bytes, and each entry points to `mDataByteSize` bytes that the block
    // buffer keeps alive.
    let format = unsafe {
        let count = (*list).mNumberBuffers as usize;
        let fits = header + count.saturating_sub(1) * std::mem::size_of::<AudioBuffer>() <= needed;
        if !fits {
            return None;
        }
        let buffers = std::slice::from_raw_parts((*list).mBuffers.as_ptr(), count);
        let planes: Vec<&[u8]> = buffers
            .iter()
            .filter(|buffer| !buffer.mData.is_null())
            .map(|buffer| {
                std::slice::from_raw_parts(buffer.mData.cast::<u8>(), buffer.mDataByteSize as usize)
            })
            .collect();
        interleave(description, &planes, out)
    };
    drop(held);
    format
}

/// One stream of the system with the object that takes its output.
struct Stream {
    stream: Retained<SCStream>,
    output: Retained<Output>,
    kind: SCStreamOutputType,
}

impl Stream {
    fn new(
        filter: &SCContentFilter,
        configuration: &SCStreamConfiguration,
        kind: SCStreamOutputType,
        sink: Arc<dyn CaptureSink>,
        ended: Arc<AtomicBool>,
        one_window: bool,
    ) -> Result<Self, CaptureError> {
        let output = Output::new(sink, ended, one_window);
        // SAFETY: the filter and the configuration are valid objects. The stream keeps no
        // strong reference to its delegate; `Stream` holds the object for as long as the
        // stream lives.
        let stream = unsafe {
            SCStream::initWithFilter_configuration_delegate(
                SCStream::alloc(),
                filter,
                configuration,
                Some(ProtocolObject::from_ref(&*output)),
            )
        };
        // SAFETY: the output object implements the protocol, and without a queue the
        // system calls it on a queue of its own.
        let added = unsafe {
            stream.addStreamOutput_type_sampleHandlerQueue_error(
                ProtocolObject::from_ref(&*output),
                kind,
                None,
            )
        };
        added.map_err(|error| CaptureError::Failed(error.localizedDescription().to_string()))?;
        Ok(Self {
            stream,
            output,
            kind,
        })
    }

    /// Starts the stream. The answer of the system arrives on the returned channel.
    fn start(&self) -> oneshot::Receiver<Result<(), CaptureError>> {
        let (started, result) = oneshot::channel();
        let started = Mutex::new(Some(started));
        let stream = Shared(self.stream.clone());
        let handler = RcBlock::new(move |error: *mut NSError| {
            // SAFETY: the system passes a valid error or null.
            let outcome = match unsafe { error.as_ref() } {
                None => Ok(()),
                Some(error) if error.code() == USER_DECLINED => Err(CaptureError::NotAllowed),
                Some(error) => Err(CaptureError::Failed(
                    error.localizedDescription().to_string(),
                )),
            };
            let waited_for = started
                .lock()
                .ok()
                .and_then(|mut started| started.take())
                .is_some_and(|started| started.send(outcome).is_ok());
            if !waited_for {
                // Nobody waits for this start any more: the share was stopped while the
                // system prepared the stream. A stop that was asked for before the start
                // had completed may not have ended it, so it is asked for again.
                // SAFETY: the stream is a valid object that this block keeps alive.
                unsafe { stream.0.stopCaptureWithCompletionHandler(None) };
            }
        });
        // SAFETY: the system copies the block and calls it once on a queue of its own;
        // the block owns what it uses.
        unsafe {
            self.stream
                .startCaptureWithCompletionHandler(Some(&handler))
        };
        result
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.output.ivars().ended.store(true, Ordering::Relaxed);
        // SAFETY: the output was added to this stream with this type. A stream that the
        // system has stopped already answers both calls with an error, which changes
        // nothing.
        unsafe {
            let output = ProtocolObject::from_ref(&*self.output);
            let _ = self.stream.removeStreamOutput_type_error(output, self.kind);
            self.stream.stopCaptureWithCompletionHandler(None);
        }
    }
}

/// A stream that the block of a completion handler holds.
struct Shared(Retained<SCStream>);

// SAFETY: a stream of ScreenCaptureKit takes its calls on any thread.
unsafe impl Send for Shared {}

/// The streams of one share.
struct Streams {
    picture: Stream,
    sound: Option<Stream>,
}

// SAFETY: a stream of ScreenCaptureKit takes its calls on any thread, and the output object
// holds only values that are `Send` and `Sync`. The streams are created on a queue of the
// system and are stopped on the thread that drops them.
unsafe impl Send for Streams {}

/// What the system lists, read on the queue on which the system gave it.
struct Content {
    content: Retained<SCShareableContent>,
}

// SAFETY: the content is an immutable list that the system gave to this process. It is
// read on one thread at a time.
unsafe impl Send for Content {}

/// Asks the system for what can be shared. Without the permission the system answers with
/// an error and shows its question to the person.
fn content() -> oneshot::Receiver<Result<Content, CaptureError>> {
    let (listed, result) = oneshot::channel();
    let listed = Mutex::new(Some(listed));
    let handler = RcBlock::new(
        move |content: *mut SCShareableContent, error: *mut NSError| {
            // SAFETY: the system passes a valid content or null, and a valid error or null.
            let outcome = unsafe {
                match (Retained::retain(content), error.as_ref()) {
                    (Some(content), _) => Ok(Content { content }),
                    (None, Some(error)) if error.code() == USER_DECLINED => {
                        Err(CaptureError::NotAllowed)
                    }
                    (None, Some(error)) => Err(CaptureError::Failed(
                        error.localizedDescription().to_string(),
                    )),
                    (None, None) => Err(CaptureError::Failed("no content".to_owned())),
                }
            };
            if let Ok(mut listed) = listed.lock()
                && let Some(listed) = listed.take()
            {
                let _ = listed.send(outcome);
            }
        },
    );
    // SAFETY: the system copies the block and calls it once on a queue of its own; the
    // block owns what it uses.
    unsafe {
        SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(
            true, true, &handler,
        );
    }
    result
}

/// Waits for an answer of the system, for a bounded time.
async fn answer<T>(waiting: oneshot::Receiver<T>) -> Result<T, CaptureError> {
    match tokio::time::timeout(ANSWER_LIMIT, waiting).await {
        Ok(Ok(answer)) => Ok(answer),
        Ok(Err(_)) | Err(_) => Err(CaptureError::Failed("no answer of the system".to_owned())),
    }
}

/// The size of a screen in pixels, and the pixels of one of its points.
fn display_pixels(display: &SCDisplay) -> ((u32, u32), f64) {
    // SAFETY: the display is a valid object of the list of the system.
    let (id, points) = unsafe { (display.displayID(), display.width().max(1) as f64) };
    let mode = CGDisplayCopyDisplayMode(id);
    let (width, height) = (
        CGDisplayMode::pixel_width(mode.as_deref()),
        CGDisplayMode::pixel_height(mode.as_deref()),
    );
    if width == 0 || height == 0 {
        // SAFETY: as above.
        let size = unsafe {
            (
                display.width().max(1) as u32,
                display.height().max(1) as u32,
            )
        };
        return (size, 1.0);
    }
    ((width as u32, height as u32), width as f64 / points)
}

fn application_of(application: &SCRunningApplication) -> (ApplicationId, String) {
    // SAFETY: the application is a valid object of the list of the system.
    unsafe {
        (
            ApplicationId {
                process_id: application.processID(),
                bundle_id: application.bundleIdentifier().to_string(),
            },
            application.applicationName().to_string(),
        )
    }
}

impl Content {
    fn sources(&self, own: &Own) -> ShareSources {
        // SAFETY: the content and the objects of its lists are valid, and the calls read
        // values that do not change.
        unsafe {
            let displays = self.content.displays();
            let scales: Vec<_> = displays
                .iter()
                .map(|display| (display.frame(), display_pixels(&display).1))
                .collect();
            let listed = self
                .content
                .windows()
                .iter()
                .map(|window| {
                    let frame = window.frame();
                    let centre = (
                        frame.origin.x + frame.size.width / 2.0,
                        frame.origin.y + frame.size.height / 2.0,
                    );
                    // The screen that holds the middle of the window gives its scale.
                    let scale = scales
                        .iter()
                        .find(|(screen, _)| {
                            centre.0 >= screen.origin.x
                                && centre.0 < screen.origin.x + screen.size.width
                                && centre.1 >= screen.origin.y
                                && centre.1 < screen.origin.y + screen.size.height
                        })
                        .or(scales.first())
                        .map_or(1.0, |(_, scale)| *scale);
                    ListedWindow {
                        id: window.windowID(),
                        title: window
                            .title()
                            .map(|title| title.to_string())
                            .unwrap_or_default(),
                        layer: window.windowLayer(),
                        on_screen: window.isOnScreen(),
                        size: (frame.size.width, frame.size.height),
                        scale,
                        application: window
                            .owningApplication()
                            .map(|application| application_of(&application)),
                    }
                })
                .collect();
            let applications = self
                .content
                .applications()
                .iter()
                .map(|application| application_of(&application))
                .collect();
            ShareSources {
                displays: displays
                    .iter()
                    .map(|display| {
                        let ((width, height), _) = display_pixels(&display);
                        DisplaySource {
                            id: display.displayID(),
                            width,
                            height,
                        }
                    })
                    .collect(),
                windows: shareable_windows(listed, own),
                applications: shareable_applications(applications, own),
            }
        }
    }

    /// Makes the streams of a share. They do not run yet.
    fn streams(
        &self,
        request: &CaptureRequest,
        sink: &Arc<dyn CaptureSink>,
        own: &Own,
    ) -> Result<(Streams, (u32, u32)), CaptureError> {
        let ended = Arc::new(AtomicBool::new(false));
        // SAFETY: the content and the objects of its lists are valid. Each filter and each
        // configuration is made and used on this thread.
        unsafe {
            let displays = self.content.displays();
            let (filter, source) = match request.picture {
                PictureSource::Display(id) => {
                    let display = displays
                        .iter()
                        .find(|display| display.displayID() == id)
                        .ok_or(CaptureError::SourceGone)?;
                    let filter = SCContentFilter::initWithDisplay_excludingWindows(
                        SCContentFilter::alloc(),
                        &display,
                        &NSArray::new(),
                    );
                    (filter, display_pixels(&display).0)
                }
                PictureSource::Window(id) => {
                    let sources = self.sources(own);
                    let listed = sources
                        .windows
                        .iter()
                        .find(|window| window.id == id)
                        .ok_or(CaptureError::SourceGone)?;
                    let window = self
                        .content
                        .windows()
                        .iter()
                        .find(|window| window.windowID() == id)
                        .ok_or(CaptureError::SourceGone)?;
                    // The window server of this process is in use before a filter of one
                    // window is made; a process that never asked it stops in that call.
                    let _ = CGMainDisplayID();
                    let filter = SCContentFilter::initWithDesktopIndependentWindow(
                        SCContentFilter::alloc(),
                        &window,
                    );
                    (filter, (listed.width, listed.height))
                }
            };
            let (width, height) = request.quality.resolution.fit(source.0, source.1);
            let configuration = SCStreamConfiguration::new();
            configuration.setWidth(width as usize);
            configuration.setHeight(height as usize);
            configuration.setScalesToFit(true);
            configuration.setPixelFormat(PIXEL_FORMAT);
            configuration.setShowsCursor(true);
            configuration.setMinimumFrameInterval(CMTime::new(
                1,
                request.quality.frame_rate.clamp(1, 120) as i32,
            ));
            let picture = Stream::new(
                &filter,
                &configuration,
                SCStreamOutputType::Screen,
                sink.clone(),
                ended.clone(),
                matches!(request.picture, PictureSource::Window(_)),
            )?;

            let sound_filter = match &request.sound {
                SoundChoice::None => None,
                SoundChoice::System => {
                    let display = displays.iter().next().ok_or(CaptureError::SourceGone)?;
                    Some(SCContentFilter::initWithDisplay_excludingWindows(
                        SCContentFilter::alloc(),
                        &display,
                        &NSArray::new(),
                    ))
                }
                SoundChoice::Application(wanted) => {
                    let display = displays.iter().next().ok_or(CaptureError::SourceGone)?;
                    let application = self
                        .content
                        .applications()
                        .iter()
                        .find(|application| {
                            let (id, name) = application_of(application);
                            id == *wanted && !own.is(id.process_id, &id.bundle_id, &name)
                        })
                        .ok_or(CaptureError::SourceGone)?;
                    Some(
                        SCContentFilter::initWithDisplay_includingApplications_exceptingWindows(
                            SCContentFilter::alloc(),
                            &display,
                            &NSArray::from_slice(&[&*application]),
                            &NSArray::new(),
                        ),
                    )
                }
            };
            let sound = match sound_filter {
                None => None,
                Some(filter) => {
                    // The configuration that the step 0 test ran (ADR 0502).
                    let configuration = SCStreamConfiguration::new();
                    configuration.setWidth(16);
                    configuration.setHeight(16);
                    configuration.setMinimumFrameInterval(CMTime::new(1, 1));
                    configuration.setCapturesAudio(true);
                    configuration.setSampleRate(48_000);
                    configuration.setChannelCount(2);
                    configuration.setExcludesCurrentProcessAudio(true);
                    Some(Stream::new(
                        &filter,
                        &configuration,
                        SCStreamOutputType::Audio,
                        sink.clone(),
                        ended,
                        false,
                    )?)
                }
            };
            Ok((Streams { picture, sound }, (width, height)))
        }
    }
}

/// The capture of the system.
pub struct SystemCapture {
    own: Own,
}

impl Default for SystemCapture {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemCapture {
    pub fn new() -> Self {
        Self { own: Own::read() }
    }
}

impl ScreenCapture for SystemCapture {
    fn access(&self) -> ScreenAccess {
        if CGPreflightScreenCaptureAccess() {
            ScreenAccess::Allowed
        } else {
            ScreenAccess::NotAllowed
        }
    }

    fn request_access(&self) {
        let _ = CGRequestScreenCaptureAccess();
    }

    fn open_settings(&self) {
        if let Err(error) = std::process::Command::new("open").arg(SETTINGS_URL).spawn() {
            tracing::warn!(%error, "the settings of the system were not opened");
        }
    }

    fn sources(&self) -> Pending<Result<ShareSources, CaptureError>> {
        let own = self.own.clone();
        let allowed = self.access() == ScreenAccess::Allowed;
        Box::pin(async move {
            // The list is not asked for without the permission: the question of the
            // system appears only when the person asks for it (ADR 0736).
            if !allowed {
                return Err(CaptureError::NotAllowed);
            }
            let content = answer(content()).await??;
            Ok(content.sources(&own))
        })
    }

    fn start(
        &self,
        request: CaptureRequest,
        sink: Arc<dyn CaptureSink>,
    ) -> Pending<Result<CaptureStream, CaptureError>> {
        let own = self.own.clone();
        let allowed = self.access() == ScreenAccess::Allowed;
        Box::pin(async move {
            if !allowed {
                return Err(CaptureError::NotAllowed);
            }
            let content = answer(content()).await??;
            let (streams, (width, height)) = content.streams(&request, &sink, &own)?;
            drop(content);
            let mut starting = vec![streams.picture.start()];
            if let Some(sound) = &streams.sound {
                starting.push(sound.start());
            }
            // A share whose sound does not start does not run without it: no choice of
            // the sharer is replaced by another one (ADR 0502).
            // The streams are dropped, and with that stopped, when one of them does not
            // start, when the system does not answer, and when this work is ended.
            for started in starting {
                answer(started).await??;
            }
            Ok(CaptureStream::new(width, height, streams))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn own() -> Own {
        Own {
            process_id: 100,
            bundle_id: "chat.junto.client".to_owned(),
            name: "Junto".to_owned(),
        }
    }

    fn application(process_id: i32, bundle_id: &str) -> ApplicationId {
        ApplicationId {
            process_id,
            bundle_id: bundle_id.to_owned(),
        }
    }

    fn window(id: u32, owner: Option<(ApplicationId, &str)>) -> ListedWindow {
        ListedWindow {
            id,
            title: format!("Window {id}"),
            layer: 0,
            on_screen: true,
            size: (800.0, 600.0),
            scale: 2.0,
            application: owner.map(|(id, name)| (id, name.to_owned())),
        }
    }

    #[test]
    fn the_state_of_the_permission_is_read_without_a_question_of_the_system() {
        // The call reads what the system holds for the program that runs the test: it
        // captures nothing and shows nothing.
        let capture = SystemCapture::new();
        assert!(capture.supported());
        let access = capture.access();
        assert_eq!(capture.access(), access);
        assert_eq!(capture.own.process_id, std::process::id() as i32);
        assert!(!capture.own.name.is_empty());
        assert!(capture.own.is(capture.own.process_id, "", ""));
    }

    #[test]
    fn the_client_is_its_own_process_and_every_other_start_of_the_same_program() {
        let own = own();
        assert!(own.is(100, "chat.junto.client", "Junto"));
        // A second instance of the application.
        assert!(own.is(200, "chat.junto.client", "Junto"));
        // Another application, also one that took the process identifier meanwhile.
        assert!(!own.is(200, "example.player", "Player"));
        assert!(own.is(100, "example.player", "Player"));
        // A build without a bundle has no bundle identifier: the name decides.
        let unbundled = Own {
            process_id: 100,
            bundle_id: String::new(),
            name: "junto".to_owned(),
        };
        assert!(unbundled.is(300, "", "junto"));
        assert!(!unbundled.is(300, "", "ffplay"));
        assert!(!unbundled.is(300, "example.player", "junto"));
    }

    #[test]
    fn only_ordinary_windows_of_other_applications_are_listed_in_pixels() {
        let player = application(7, "example.player");
        let mut small = window(3, Some((player.clone(), "Player")));
        small.size = (800.0, 40.0);
        let mut menu = window(4, Some((player.clone(), "Player")));
        menu.layer = 25;
        let mut hidden = window(5, Some((player.clone(), "Player")));
        hidden.on_screen = false;
        let listed = vec![
            window(1, Some((player.clone(), "Player"))),
            window(2, Some((application(100, "chat.junto.client"), "Junto"))),
            small,
            menu,
            hidden,
            window(6, None),
            window(7, Some((application(200, "chat.junto.client"), "Junto"))),
        ];
        let windows = shareable_windows(listed, &own());
        assert_eq!(windows.len(), 1);
        assert_eq!(
            windows[0],
            WindowSource {
                id: 1,
                title: "Window 1".to_owned(),
                application: player,
                application_name: "Player".to_owned(),
                width: 1600,
                height: 1200,
            }
        );
    }

    #[test]
    fn the_client_is_not_among_the_applications_whose_sound_is_offered() {
        let listed = vec![
            (application(9, "example.zeta"), "zeta".to_owned()),
            (application(100, "chat.junto.client"), "Junto".to_owned()),
            (application(7, "example.player"), "Player".to_owned()),
            (application(200, "chat.junto.client"), "Junto".to_owned()),
            (application(7, "example.player"), "Player".to_owned()),
            (application(8, "example.nameless"), String::new()),
        ];
        let names: Vec<String> = shareable_applications(listed, &own())
            .into_iter()
            .map(|application| application.name)
            .collect();
        assert_eq!(names, ["Player", "zeta"]);
    }

    fn description(flags: u32, channels: u32, bits: u32) -> SoundDescription {
        SoundDescription {
            format_id: LINEAR_PCM,
            flags,
            sample_rate: 48_000.0,
            channels,
            bits,
        }
    }

    fn bytes(samples: &[f32]) -> Vec<u8> {
        samples
            .iter()
            .flat_map(|sample| sample.to_ne_bytes())
            .collect()
    }

    #[test]
    fn a_buffer_per_channel_becomes_interleaved_samples() {
        // The form that the system was observed to give: 32-bit floats, one buffer per
        // channel (flags 0x29).
        let (left, right) = (bytes(&[0.1, 0.2, 0.3]), bytes(&[-0.1, -0.2, -0.3]));
        let mut out = vec![9.0];
        let format = interleave(description(0x29, 2, 32), &[&left, &right], &mut out);
        assert_eq!(
            format,
            Some(Format {
                sample_rate: 48_000,
                channels: 2
            })
        );
        assert_eq!(out, [0.1, -0.1, 0.2, -0.2, 0.3, -0.3]);
        // Buffers of unequal length give the frames that every channel has.
        let short = bytes(&[0.5]);
        interleave(description(0x29, 2, 32), &[&left, &short], &mut out);
        assert_eq!(out, [0.1, 0.5]);
    }

    #[test]
    fn one_buffer_with_every_channel_and_16_bit_samples_are_read_too() {
        let both = bytes(&[0.1, -0.1, 0.2, -0.2]);
        let mut out = Vec::new();
        let format = interleave(description(FLAG_FLOAT, 2, 32), &[&both], &mut out);
        assert_eq!(format.map(|format| format.channels), Some(2));
        assert_eq!(out, [0.1, -0.1, 0.2, -0.2]);

        let whole: Vec<u8> = [i16::MAX, i16::MIN + 1, 0]
            .iter()
            .flat_map(|sample| sample.to_ne_bytes())
            .collect();
        let format = interleave(description(0, 1, 16), &[&whole], &mut out);
        assert_eq!(format.map(|format| format.channels), Some(1));
        assert_eq!(out, [1.0, -1.0, 0.0]);
    }

    #[test]
    fn a_buffer_of_a_form_that_is_not_read_gives_nothing() {
        let plane = bytes(&[0.1, 0.2]);
        let mut out = Vec::new();
        // Another format than linear PCM.
        let mut other = description(0x29, 1, 32);
        other.format_id = u32::from_be_bytes(*b"aac ");
        assert_eq!(interleave(other, &[&plane], &mut out), None);
        // A sample width that the capture does not read.
        assert_eq!(
            interleave(description(0x29, 1, 64), &[&plane], &mut out),
            None
        );
        assert_eq!(
            interleave(description(0x20, 1, 24), &[&plane], &mut out),
            None
        );
        // The number of buffers does not match the channels.
        assert_eq!(
            interleave(description(0x29, 2, 32), &[&plane], &mut out),
            None
        );
        assert_eq!(
            interleave(description(FLAG_FLOAT, 2, 32), &[&plane, &plane], &mut out),
            None
        );
        // No channel, and a rate outside what a device has.
        assert_eq!(interleave(description(0x29, 0, 32), &[], &mut out), None);
        let mut slow = description(0x29, 1, 32);
        slow.sample_rate = 10.0;
        assert_eq!(interleave(slow, &[&plane], &mut out), None);
        assert!(out.is_empty());
    }
}
