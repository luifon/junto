//! The start and the end of a share on Windows: one thread for the picture, one for the
//! sound, and the delivery of both (ADR 0706).

use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;
use std::time::Duration;

use tokio::sync::oneshot;
use windows::Graphics::Capture::GraphicsCaptureSession;
use windows::Win32::Foundation::HWND;

use super::graphics::{Gpu, Session, Target};
use super::rules::{self, Creators};
use super::sound::{Capture, Event, Loopback};
use super::sources;
use super::still::{self, Still};
use crate::share::delivery::{CAPACITY, Latest, Life, deliver};
use crate::share::listing::Own;
use crate::share::picture::PictureFrame;
use crate::share::{
    CaptureError, CaptureRequest, CaptureSink, CaptureStream, FRAME_RATES, Pending, PictureSource,
    ScreenAccess, ScreenCapture, ShareSources, SoundChoice,
};

/// How long the system has to start the capture of the picture and of the sound. Without
/// an answer the share does not start, and nothing keeps running.
const ANSWER_LIMIT: Duration = Duration::from_secs(10);

/// The threads of a capture use the multithreaded apartment of the process without entering
/// it: a thread that left the apartment at its end would end it while the system still holds
/// objects of the capture, and their release would then fail (ADR 0706).
fn apartment() -> Result<(), CaptureError> {
    crate::com::keep_multithreaded().map_err(CaptureError::Failed)
}

/// Whether this system has Windows Graphics Capture, asked on a thread of its own in the
/// multithreaded apartment, whatever apartment the calling thread is in.
fn graphics_capture_supported() -> bool {
    std::thread::Builder::new()
        .name("share-support".into())
        .spawn(|| {
            apartment().ok()?;
            GraphicsCaptureSession::IsSupported().ok()
        })
        .ok()
        .and_then(|thread| thread.join().ok().flatten())
        .unwrap_or(false)
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

    /// A picture made with GDI on a blocking thread of the runtime: one picture needs no
    /// session of Windows Graphics Capture and no graphics device (ADR 0736).
    fn preview(
        &self,
        source: PictureSource,
        bound: (u32, u32),
    ) -> Pending<Result<PictureFrame, CaptureError>> {
        let (own, supported) = (self.own.clone(), self.supported);
        Box::pin(async move {
            if !supported {
                return Err(CaptureError::Unsupported);
            }
            tokio::task::spawn_blocking(move || {
                let still = match source {
                    PictureSource::Portal(_) => return Err(CaptureError::Unsupported),
                    PictureSource::Display(id) => Still::Monitor(
                        sources::displays()
                            .into_iter()
                            .find(|display| display.source.id == id)
                            .ok_or(CaptureError::SourceGone)?
                            .monitor,
                    ),
                    PictureSource::Window(id) => {
                        if !sources::windows(&own).iter().any(|window| window.id == id) {
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

/// The screen or window and the sound of a request, checked against the lists of now: a
/// share starts only with a source that a person could choose.
fn resolve(
    own: &Own,
    request: &CaptureRequest,
) -> Result<(Target, Option<Loopback>), CaptureError> {
    let target = match request.picture {
        PictureSource::Portal(_) => return Err(CaptureError::Unsupported),
        PictureSource::Display(id) => Target::Monitor(
            sources::displays()
                .into_iter()
                .find(|display| display.source.id == id)
                .ok_or(CaptureError::SourceGone)?
                .monitor,
        ),
        PictureSource::Window(id) => {
            if !sources::windows(own).iter().any(|window| window.id == id) {
                return Err(CaptureError::SourceGone);
            }
            Target::Window(HWND(rules::window_handle(id) as *mut c_void))
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
    Ok((target, loopback))
}

/// The threads of a share. Each waits for the system or for a picture without a time
/// limit, and the stop of the life wakes it: a drop ends them at once.
struct Workers {
    life: Arc<Life>,
    threads: Vec<JoinHandle<()>>,
}

impl Drop for Workers {
    fn drop(&mut self) {
        self.life.cancelled.store(true, Ordering::Release);
        self.life.stop();
        for thread in self.threads.drain(..) {
            if thread.thread().id() != std::thread::current().id() {
                let _ = thread.join();
            }
        }
    }
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

fn spawn(name: &str, work: impl FnOnce() + Send + 'static) -> Result<JoinHandle<()>, CaptureError> {
    std::thread::Builder::new()
        .name(name.into())
        .spawn(work)
        .map_err(|error| CaptureError::Failed(error.to_string()))
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
    let (target, loopback) = tokio::task::spawn_blocking({
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
    let mut workers = Workers {
        life: life.clone(),
        threads: Vec::new(),
    };
    let (ready, picture_ready) = oneshot::channel();
    workers.threads.push(spawn("share-picture", {
        let (life, pictures) = (life.clone(), pictures.clone());
        move || picture(life, target, pictures, ready)
    })?);
    let (gpu, size) = answer(picture_ready, &life).await?;
    let samples = match loopback {
        None => None,
        Some(loopback) => {
            let (send_samples, samples) = rtrb::RingBuffer::new(CAPACITY);
            let (ready, sound_ready) = oneshot::channel();
            workers.threads.push(spawn("share-sound", {
                let life = life.clone();
                move || sound(life, loopback, send_samples, ready)
            })?);
            answer(sound_ready, &life).await?;
            Some(samples)
        }
    };
    life.result()?;
    workers.threads.push(spawn("share-delivery", {
        let life = life.clone();
        let quality = request.quality;
        move || deliver(life, sink, pictures, move |()| gpu.read(), samples, quality)
    })?);
    let (width, height) = request.quality.resolution.fit(size.0, size.1);
    Ok(CaptureStream::new(width, height, workers))
}

type Ready<T> = oneshot::Sender<Result<T, CaptureError>>;

/// A failure is sent as the answer to the start while nobody has an answer yet, so the
/// start reads the reason and not a closed channel.
fn finish<T>(life: &Life, ready: &mut Option<Ready<T>>, result: Result<(), CaptureError>) {
    if let Err(failure) = result {
        if let Some(ready) = ready.take() {
            let _ = ready.send(Err(failure.clone()));
        }
        life.fail(failure);
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
        let waker = owner.waker()?;
        life.on_stop(move || waker.wake());
        let session = Session::start(target, life.clone(), pictures)?;
        if let Some(ready) = ready.take() {
            let _ = ready.send(Ok((session.gpu.clone(), session.size)));
        }
        while !life.stopped() {
            owner.pump();
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
        let stop = Arc::new(Event::new()?);
        life.on_stop({
            let stop = stop.clone();
            move || stop.set()
        });
        let capture = Capture::start(loopback)?;
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
}

// SAFETY: a thread handle can be used and closed from any thread; the waker only posts to the
// thread and closes the handle once, when it is dropped.
unsafe impl Send for ThreadWaker {}
// SAFETY: as above; `wake` does not change the waker.
unsafe impl Sync for ThreadWaker {}

impl ThreadWaker {
    /// Makes `pump` of the thread return.
    pub(super) fn wake(&self) {
        use windows::Win32::Foundation::{LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_NULL};
        // SAFETY: the identifier names the thread of the waker (see the type); posting to it
        // after it has ended fails and changes nothing.
        let _ = unsafe { PostThreadMessageW(self.id, WM_NULL, WPARAM(0), LPARAM(0)) };
    }
}

impl Drop for ThreadWaker {
    fn drop(&mut self) {
        // SAFETY: the handle was opened by `Thread::waker` and is closed only here.
        let _ = unsafe { windows::Win32::Foundation::CloseHandle(self.handle) };
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
    pub(super) fn waker(&self) -> Result<ThreadWaker, CaptureError> {
        use windows::Win32::System::Threading::{OpenThread, THREAD_SYNCHRONIZE};
        // SAFETY: the call opens the thread of this identifier, which is the running caller.
        let handle = unsafe { OpenThread(THREAD_SYNCHRONIZE, false, self.0) }
            .map_err(|error| CaptureError::Failed(error.message()))?;
        Ok(ThreadWaker { id: self.0, handle })
    }

    /// Waits for messages of the calling thread and handles them. A message that waits
    /// already ends the wait at once.
    pub(super) fn pump(&self) {
        use windows::Win32::System::Threading::INFINITE;
        use windows::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, MSG, MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, PM_REMOVE,
            PeekMessageW, QS_ALLINPUT, TranslateMessage,
        };
        // SAFETY: the calls wait for and handle the messages of this thread's own queue.
        unsafe {
            let _ = MsgWaitForMultipleObjectsEx(None, INFINITE, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
            let mut message = MSG::default();
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }
}
