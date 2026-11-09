//! The start and the end of a share on Windows: one thread for the picture, one for the
//! sound, and the delivery of both (ADR 0706).

use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::share::workers::Workers;
use std::time::Duration;

use tokio::sync::oneshot;
use windows::Graphics::Capture::GraphicsCaptureSession;
use windows::Win32::Foundation::HWND;

use super::application;
use super::graphics::{Gpu, Session, Target};
use super::rules::{self, Creators};
use super::sound::{Capture, Event, Loopback};
use super::sources;
use super::still::{self, Still};
use crate::share::delivery::pixels::Frame;
use crate::share::delivery::{CAPACITY, Latest, Life, deliver};
use crate::share::listing::Own;
use crate::share::picture::PictureFrame;
use crate::share::{
    ApplicationId, CaptureError, CaptureRequest, CaptureSink, CaptureStream, FRAME_RATES, Pending,
    PictureSource, ScreenAccess, ScreenCapture, ShareSources, SoundChoice,
};

/// How long the system has to start the capture of the picture and of the sound. Without
/// an answer the share does not start; the worker supervisor reports unfinished cleanup.
const ANSWER_LIMIT: Duration = Duration::from_secs(10);

/// The threads of a capture use the multithreaded apartment of the process without entering
/// it: a thread that left the apartment at its end would end it while the system still holds
/// objects of the capture, and their release would then fail (ADR 0706).
pub(super) fn apartment() -> Result<(), CaptureError> {
    crate::com::keep_multithreaded().map_err(CaptureError::Failed)
}

/// Whether this system has Windows Graphics Capture, asked on a thread of its own in the
/// multithreaded apartment, whatever apartment the calling thread is in.
fn graphics_capture_supported() -> bool {
    let (ready, answer) = std::sync::mpsc::sync_channel(1);
    let started = std::thread::Builder::new()
        .name("share-support".into())
        .spawn(move || {
            let result = apartment().and_then(|()| {
                GraphicsCaptureSession::IsSupported()
                    .map_err(|error| CaptureError::Failed(error.to_string()))
            });
            let _ = ready.send(result);
        });
    if let Err(error) = started {
        tracing::error!(%error, "could not start the screen capture support check");
        return false;
    }
    match answer.recv_timeout(ANSWER_LIMIT) {
        Ok(Ok(supported)) => supported,
        Ok(Err(error)) => {
            tracing::error!(%error, "screen capture support check failed");
            false
        }
        Err(error) => {
            tracing::error!(%error, "screen capture support check did not answer");
            false
        }
    }
}

/// The capture of the system.
pub struct SystemCapture {
    own: Own,
    supported: bool,
}

impl Default for SystemCapture {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemCapture {
    pub fn new() -> Self {
        Self {
            own: sources::own(),
            supported: graphics_capture_supported(),
        }
    }
}

impl ScreenCapture for SystemCapture {
    fn supported(&self) -> bool {
        self.supported
    }

    /// An application of the desktop asks Windows for no permission to capture the screen.
    fn access(&self) -> ScreenAccess {
        if self.supported {
            ScreenAccess::Allowed
        } else {
            ScreenAccess::NotAllowed
        }
    }

    fn request_access(&self) {}

    fn open_settings(&self) {}

    fn sources(&self) -> Pending<Result<ShareSources, CaptureError>> {
        let (own, supported) = (self.own.clone(), self.supported);
        Box::pin(async move {
            if !supported {
                return Err(CaptureError::Unsupported);
            }
            tokio::task::spawn_blocking(move || sources::sources(&own))
                .await
                .map_err(|error| CaptureError::Failed(error.to_string()))
        })
    }

    /// A screen picture from GDI or a bounded window capture from the compositor (ADR 0736).
    fn preview(
        &self,
        source: PictureSource,
        bound: (u32, u32),
    ) -> Pending<Result<PictureFrame, CaptureError>> {
        let supported = self.supported;
        Box::pin(async move {
            if !supported {
                return Err(CaptureError::Unsupported);
            }
            tokio::task::spawn_blocking(move || {
                let still = match source {
                    PictureSource::Portal(_) => return Err(CaptureError::Unsupported),
                    PictureSource::Application(process_id) => {
                        return application::preview(process_id, bound);
                    }
                    PictureSource::Display(id) => Still::Monitor(
                        sources::displays()
                            .into_iter()
                            .find(|display| display.source.id == id)
                            .ok_or(CaptureError::SourceGone)?
                            .monitor,
                    ),
                    PictureSource::Window(id) => {
                        if !sources::windows().iter().any(|window| window.id == id) {
                            return Err(CaptureError::SourceGone);
                        }
                        Still::Window(HWND(rules::window_handle(id) as *mut c_void))
                    }
                };
                still::picture(&still, bound)
            })
            .await
            .map_err(|error| CaptureError::Failed(error.to_string()))?
        })
    }

    fn start(
        &self,
        request: CaptureRequest,
        sink: Arc<dyn CaptureSink>,
    ) -> Pending<Result<CaptureStream, CaptureError>> {
        let (own, supported) = (self.own.clone(), self.supported);
        Box::pin(async move {
            if !supported {
                return Err(CaptureError::Unsupported);
            }
            start(own, request, sink).await
        })
    }
}

/// What the picture of a share captures.
enum Picture {
    /// One screen or one window.
    One(Target),
    /// Every window of one application on one screen.
    Application(application::Shared),
}

/// The picture and the sound of a request, checked against the lists of now: a share
/// starts only with a source that a person could choose. Also whether the picture shows the
/// client itself (ADR 0738).
fn resolve(
    own: &Own,
    request: &CaptureRequest,
) -> Result<(Picture, Option<Loopback>, bool), CaptureError> {
    let is_client = |(id, name): (&ApplicationId, &str)| own.is(id.process_id, &id.bundle_id, name);
    let (picture, shows_the_client) = match request.picture {
        PictureSource::Portal(_) => return Err(CaptureError::Unsupported),
        PictureSource::Display(id) => (
            Picture::One(Target::Monitor(
                sources::displays()
                    .into_iter()
                    .find(|display| display.source.id == id)
                    .ok_or(CaptureError::SourceGone)?
                    .monitor,
            )),
            false,
        ),
        PictureSource::Window(id) => {
            let window = sources::windows()
                .into_iter()
                .find(|window| window.id == id)
                .ok_or(CaptureError::SourceGone)?;
            (
                Picture::One(Target::Window(
                    HWND(rules::window_handle(id) as *mut c_void),
                )),
                is_client((&window.application, &window.application_name)),
            )
        }
        PictureSource::Application(process_id) => {
            let shared = application::resolve(process_id)?;
            let shows = is_client((&shared.application.0, &shared.application.1));
            (Picture::Application(shared), shows)
        }
    };
    let loopback = match &request.sound {
        SoundChoice::None => None,
        SoundChoice::System => Some(Loopback::Exclude(own.process_id as u32)),
        SoundChoice::Application(wanted) => {
            // The process must still run the same program: a number of an ended process
            // can belong to another one now.
            let (id, name) = sources::application_of(wanted.process_id as u32)
                .ok_or(CaptureError::SourceGone)?;
            if id != *wanted || own.is(id.process_id, &id.bundle_id, &name) {
                return Err(CaptureError::SourceGone);
            }
            // The tree of an application that started the client holds the client.
            match sources::ancestors(own) {
                Creators::Unknown => return Err(CaptureError::CreatorsUnknown),
                Creators::Known(creators) if creators.contains(&(id.process_id as u32)) => {
                    return Err(CaptureError::StartedTheClient);
                }
                Creators::Known(_) => {}
            }
            Some(Loopback::Include(id.process_id as u32))
        }
    };
    Ok((picture, loopback, shows_the_client))
}

async fn answer<T>(
    ready: oneshot::Receiver<Result<T, CaptureError>>,
    life: &Life,
) -> Result<T, CaptureError> {
    match tokio::time::timeout(ANSWER_LIMIT, ready).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => {
            life.result()?;
            Err(CaptureError::Failed(
                "the capture ended before it started".to_owned(),
            ))
        }
        Err(_) => Err(CaptureError::Failed("no answer of the system".to_owned())),
    }
}

async fn start(
    own: Own,
    request: CaptureRequest,
    sink: Arc<dyn CaptureSink>,
) -> Result<CaptureStream, CaptureError> {
    if !FRAME_RATES.contains(&request.quality.frame_rate) {
        return Err(CaptureError::Failed(
            "unsupported share frame rate".to_owned(),
        ));
    }
    let (target, loopback, shows_the_client) = tokio::task::spawn_blocking({
        let request = request.clone();
        move || resolve(&own, &request)
    })
    .await
    .map_err(|error| CaptureError::Failed(error.to_string()))??;
    let life = Arc::new(Life::default());
    let pictures = Arc::new(Latest::default());
    life.on_stop({
        let pictures = pictures.clone();
        move || pictures.wake()
    });
    let (mut workers, stopped) = Workers::new(life.clone());
    workers.report_failures_to(sink.clone());
    type Read = Box<dyn FnMut(()) -> Result<Option<Frame>, CaptureError> + Send>;
    let (read, size): (Read, (u32, u32)) = match target {
        Picture::One(target) => {
            let (ready, picture_ready) = oneshot::channel();
            workers.spawn("share-picture", {
                let (life, pictures) = (life.clone(), pictures.clone());
                move || picture(life, target, pictures, ready)
            })?;
            let (gpu, size) = answer(picture_ready, &life).await?;
            (Box::new(move |()| gpu.read()) as Read, size)
        }
        Picture::Application(shared) => {
            let size = shared.size;
            let drawn = application::Drawn::default();
            let (ready, picture_ready) = oneshot::channel();
            workers.spawn("share-application", {
                let (life, pictures, drawn) = (life.clone(), pictures.clone(), drawn.clone());
                move || application::picture(life, shared, pictures, drawn, ready)
            })?;
            answer(picture_ready, &life).await?;
            (
                Box::new(move |()| application::draw(&drawn, size)) as Read,
                size,
            )
        }
    };
    let samples = match loopback {
        None => None,
        Some(loopback) => {
            let (send_samples, samples) = rtrb::RingBuffer::new(CAPACITY);
            let (ready, sound_ready) = oneshot::channel();
            workers.spawn("share-sound", {
                let life = life.clone();
                move || sound(life, loopback, send_samples, ready)
            })?;
            answer(sound_ready, &life).await?;
            Some(samples)
        }
    };
    life.result()?;
    workers.spawn("share-delivery", {
        let life = life.clone();
        let quality = request.quality;
        move || deliver(life, sink, pictures, read, samples, quality)
    })?;
    let (width, height) = request.quality.resolution.fit(size.0, size.1);
    Ok(CaptureStream::new(width, height, workers)
        .with_stop_result(stopped)
        .showing_the_client(shows_the_client))
}

pub(super) type Ready<T> = oneshot::Sender<Result<T, CaptureError>>;

/// A failure is sent as the answer to the start while nobody has an answer yet, so the
/// start reads the reason and not a closed channel.
pub(super) fn finish<T>(
    life: &Life,
    ready: &mut Option<Ready<T>>,
    result: Result<(), CaptureError>,
) {
    if let Err(failure) = result {
        if let Some(ready) = ready.take() {
            let _ = ready.send(Err(failure.clone()));
        }
        if life.cancelled.load(Ordering::Acquire) {
            life.cleanup_failed(failure);
        } else {
            tracing::error!(%failure, "screen capture worker failed");
            life.fail(failure);
        }
    }
}

fn picture(
    life: Arc<Life>,
    target: Target,
    pictures: Arc<Latest<()>>,
    ready: Ready<(Arc<Gpu>, (u32, u32))>,
) {
    let mut ready = Some(ready);
    let result = (|| {
        apartment()?;
        let owner = Thread::current();
        let waker = owner.waker(&life)?;
        life.on_stop(move || {
            if let Err(error) = waker.wake() {
                tracing::warn!(%error, "the picture wake failed; the bounded wait will check cancellation");
            }
        });
        let session = Session::start(target, life.clone(), pictures)?;
        if let Some(ready) = ready.take() {
            let _ = ready.send(Ok((session.gpu.clone(), session.size)));
        }
        while !life.stopped() {
            owner.pump()?;
        }
        drop(session);
        Ok(())
    })();
    finish(&life, &mut ready, result);
}

fn sound(
    life: Arc<Life>,
    loopback: Loopback,
    mut samples: rtrb::Producer<[f32; 2]>,
    ready: Ready<()>,
) {
    let mut ready = Some(ready);
    let result = (|| {
        apartment()?;
        let stop = Arc::new(Event::new(&life)?);
        life.on_stop({
            let stop = stop.clone();
            move || {
                if let Err(error) = stop.set() {
                    tracing::warn!(%error, "the sound wake failed; the bounded wait will check cancellation");
                }
            }
        });
        let capture = Capture::start(loopback, life.clone())?;
        if let Some(ready) = ready.take() {
            let _ = ready.send(Ok(()));
        }
        // A full queue refuses the frame: the delivery removes what waits too long.
        capture.run(&life, &stop, |frame| {
            let _ = samples.push(frame);
        })
    })();
    finish(&life, &mut ready, result);
}

/// The picture thread of a capture, which runs a message loop, and which the stop of the
/// capture wakes.
pub(super) struct Thread(u32);

/// Wakes the picture thread from another thread. It holds the thread open: the system gives
/// the identifier of a thread to another thread only after the thread object is destroyed,
/// and an open handle keeps that object, so the identifier names this thread while the
/// waker exists, also after the thread has ended.
pub(super) struct ThreadWaker {
    id: u32,
    handle: windows::Win32::Foundation::HANDLE,
    life: std::sync::Weak<Life>,
}

// SAFETY: a thread handle can be used and closed from any thread; the waker only posts to the
// thread and closes the handle once, when it is dropped.
unsafe impl Send for ThreadWaker {}
// SAFETY: as above; `wake` does not change the waker.
unsafe impl Sync for ThreadWaker {}

impl ThreadWaker {
    /// Makes `pump` of the thread return.
    pub(super) fn wake(&self) -> Result<(), CaptureError> {
        use windows::Win32::Foundation::{LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_NULL};
        // SAFETY: the identifier names the thread of the waker (see the type); posting to it
        // after it has ended fails and changes nothing.
        unsafe { PostThreadMessageW(self.id, WM_NULL, WPARAM(0), LPARAM(0)) }.map_err(|error| {
            CaptureError::Failed(format!(
                "could not wake the capture picture worker: {error}"
            ))
        })
    }
}

impl Drop for ThreadWaker {
    fn drop(&mut self) {
        // SAFETY: the handle was opened by `Thread::waker` and is closed only here.
        if let Err(error) = unsafe { windows::Win32::Foundation::CloseHandle(self.handle) } {
            let failure = CaptureError::Failed(format!(
                "could not close the capture picture thread handle: {error}"
            ));
            if let Some(life) = self.life.upgrade() {
                life.cleanup_failed(failure);
            } else {
                tracing::error!(%failure, "screen capture cleanup failed");
            }
        }
    }
}

impl Thread {
    /// The calling thread, with the message queue that `wake` posts to.
    pub(super) fn current() -> Self {
        use windows::Win32::UI::WindowsAndMessaging::{MSG, PM_NOREMOVE, PeekMessageW};
        let mut message = MSG::default();
        // SAFETY: the calls read the identifier of this thread and make its message queue.
        unsafe {
            let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
            Self(windows::Win32::System::Threading::GetCurrentThreadId())
        }
    }

    /// A waker of this thread, which holds the thread open.
    pub(super) fn waker(&self, life: &Arc<Life>) -> Result<ThreadWaker, CaptureError> {
        use windows::Win32::System::Threading::{OpenThread, THREAD_SYNCHRONIZE};
        // SAFETY: the call opens the thread of this identifier, which is the running caller.
        let handle = unsafe { OpenThread(THREAD_SYNCHRONIZE, false, self.0) }
            .map_err(|error| CaptureError::Failed(error.message()))?;
        Ok(ThreadWaker {
            id: self.0,
            handle,
            life: Arc::downgrade(life),
        })
    }

    /// Waits for messages of the calling thread and handles them. A message that waits
    /// already ends the wait at once.
    pub(super) fn pump(&self) -> Result<(), CaptureError> {
        use windows::Win32::Foundation::WAIT_FAILED;
        use windows::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, MSG, MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, PM_REMOVE,
            PeekMessageW, QS_ALLINPUT, TranslateMessage,
        };
        // SAFETY: the calls wait for and handle the messages of this thread's own queue.
        unsafe {
            let result = MsgWaitForMultipleObjectsEx(None, 100, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
            if result == WAIT_FAILED {
                return Err(CaptureError::Failed(format!(
                    "the capture picture wait failed: {}",
                    windows::core::Error::from_thread()
                )));
            }
            let mut message = MSG::default();
            for _ in 0..64 {
                if !PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                    break;
                }
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn the_picture_wait_observes_stop_without_a_posted_wake() {
        let life = Arc::new(Life::default());
        let (ready, created) = std::sync::mpsc::sync_channel(0);
        let (done, ended) = std::sync::mpsc::sync_channel(1);
        let stopped = life.clone();
        std::thread::spawn(move || {
            let owner = Thread::current();
            ready.send(owner.waker(&stopped).unwrap()).unwrap();
            loop {
                owner.pump().unwrap();
                if stopped.stopped() {
                    break;
                }
            }
            let _ = done.send(());
        });
        let waker = created.recv_timeout(Duration::from_secs(2)).unwrap();
        life.stop();
        let bounded = ended.recv_timeout(Duration::from_secs(2));
        if bounded.is_err() {
            let _ = waker.wake();
            ended.recv_timeout(Duration::from_secs(2)).unwrap();
        }
        assert!(
            bounded.is_ok(),
            "the picture wait depended on a posted message to stop"
        );
    }

    #[test]
    fn waking_an_ended_picture_thread_returns_the_system_failure() {
        let life = Arc::new(Life::default());
        let (send, ready) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let owner = Thread::current();
            let waker = owner.waker(&life).unwrap();
            let _ = send.send(waker);
        });
        let waker = ready.recv_timeout(Duration::from_secs(2)).unwrap();
        // SAFETY: the waker holds this thread handle open throughout the bounded wait.
        let ended =
            unsafe { windows::Win32::System::Threading::WaitForSingleObject(waker.handle, 2_000) };
        assert_eq!(ended, windows::Win32::Foundation::WAIT_OBJECT_0);
        assert!(waker.wake().is_err());
    }
}
