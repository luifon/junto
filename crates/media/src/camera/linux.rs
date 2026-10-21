//! Direct V4L2 capture. No permission dialog or session-manager connection is opened.
use std::ffi::{CString, c_char, c_int, c_void};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::format::Mode;
use super::raw::{Layout, Pixel};
use super::v4l2::{self, Buffer, Dequeued, Range};
use super::{worker, *};

/// The system calls of capture: a null pointer is the system's own, a test gives its device.
#[derive(Clone, Copy)]
pub(super) struct System(*const c_void);
// SAFETY: the table of a test device is immutable and its functions take its own lock.
unsafe impl Send for System {}
// SAFETY: as for Send.
unsafe impl Sync for System {}
const NATIVE: System = System(std::ptr::null());

/// A device node of `/sys/class/video4linux`.
pub(super) struct Node {
    pub identity: String,
    pub name: String,
    pub path: PathBuf,
}

#[repr(C)]
#[derive(Default)]
struct Format {
    width: u32,
    height: u32,
    stride: u32,
    fourcc: u32,
    encoding: u32,
    rate_error: i32,
}

#[repr(C)]
struct Request {
    fourcc: u32,
    width: u32,
    height: u32,
    /// The frame interval in seconds.
    numerator: u32,
    denominator: u32,
}

unsafe extern "C" {
    fn junto_camera_open(
        system: *const c_void,
        path: *const c_char,
        camera: *mut *mut c_void,
    ) -> c_int;
    fn junto_camera_format(camera: *mut c_void, index: u32, fourcc: *mut u32) -> c_int;
    fn junto_camera_size(camera: *mut c_void, fourcc: u32, index: u32, range: *mut Range) -> c_int;
    fn junto_camera_interval(
        camera: *mut c_void,
        fourcc: u32,
        width: u32,
        height: u32,
        index: u32,
        range: *mut Range,
    ) -> c_int;
    fn junto_camera_start(
        camera: *mut c_void,
        request: *const Request,
        format: *mut Format,
    ) -> c_int;
    fn junto_camera_next(camera: *mut c_void, timeout: c_int, buffer: *mut Dequeued) -> c_int;
    fn junto_camera_release(
        camera: *mut c_void,
        index: u32,
        bytes: *mut u8,
        length: usize,
    ) -> c_int;
    fn junto_camera_close(camera: *mut c_void);
}

const NOT_PERMITTED: i32 = -1;
const ACCESS_DENIED: i32 = -13;
const NOT_A_CAPTURE: i32 = -95;

fn error(code: i32) -> CameraError {
    match code {
        NOT_PERMITTED | ACCESS_DENIED => CameraError::NoPermission(CameraAccess::Restricted),
        -2 | -5 | -6 | -19 => CameraError::DeviceGone,
        -22 | -25 | NOT_A_CAPTURE => CameraError::Unsupported,
        _ => CameraError::Failed(std::io::Error::from_raw_os_error(-code).to_string()),
    }
}

/// The pixel formats that the converter reads, by their V4L2 code.
const PIXELS: [(u32, Pixel); 3] = [
    (u32::from_le_bytes(*b"NV12"), Pixel::Nv12),
    (u32::from_le_bytes(*b"YU12"), Pixel::I420),
    (u32::from_le_bytes(*b"YUYV"), Pixel::Yuy2),
];
const SIZES: [(u32, u32); 3] = [(1280, 720), (640, 480), (352, 288)];
/// Bounds on the entries of a driver's enumerations.
const ENTRIES: u32 = 256;

/// One open device node.
struct Camera(*mut c_void);

impl Drop for Camera {
    fn drop(&mut self) {
        // SAFETY: only the thread that opened the camera uses and closes it.
        unsafe { junto_camera_close(self.0) };
    }
}

impl Camera {
    fn open(system: System, path: &Path) -> Result<Self, i32> {
        let path = CString::new(path.as_os_str().as_bytes()).map_err(|_| -2)?;
        let mut pointer = std::ptr::null_mut();
        // SAFETY: a terminated path and an out parameter; success transfers one camera.
        match unsafe { junto_camera_open(system.0, path.as_ptr(), &mut pointer) } {
            0 => Ok(Self(pointer)),
            code => Err(code),
        }
    }

    /// The entries of an enumeration of the driver, through `entry(index, out)`.
    fn entries<T: Default>(
        &self,
        mut entry: impl FnMut(u32, &mut T) -> c_int,
    ) -> Result<Vec<T>, CameraError> {
        let mut entries = Vec::new();
        for index in 0..ENTRIES {
            let mut value = T::default();
            match entry(index, &mut value) {
                1 => entries.push(value),
                0 => break,
                code => return Err(error(code)),
            }
        }
        Ok(entries)
    }

    /// The capture modes of the accepted pixel formats at the capture sizes, with the V4L2
    /// code of each.
    fn modes(&self) -> Result<Vec<(Mode, u32)>, CameraError> {
        // SAFETY: the camera is open and each out parameter is valid for the call.
        let formats =
            self.entries(|index, fourcc| unsafe { junto_camera_format(self.0, index, fourcc) })?;
        let mut modes = Vec::new();
        for fourcc in formats {
            if !PIXELS.iter().any(|(code, _)| *code == fourcc) {
                continue;
            }
            // SAFETY: as above.
            let sizes = self.entries(|index, range| unsafe {
                junto_camera_size(self.0, fourcc, index, range)
            })?;
            for (width, height) in SIZES {
                // A driver without the enumeration of sizes is asked for each capture size.
                if !sizes.is_empty()
                    && !sizes.iter().any(|range| v4l2::fits(range, (width, height)))
                {
                    continue;
                }
                // SAFETY: as above.
                let intervals = self.entries(|index, range| unsafe {
                    junto_camera_interval(self.0, fourcc, width, height, index, range)
                })?;
                let rates = v4l2::rates(&intervals);
                let mode = |rate| Mode {
                    width,
                    height,
                    rate,
                    compressed: false,
                };
                if rates.is_empty() {
                    modes.push((mode(None), fourcc));
                }
                modes.extend(rates.into_iter().map(|rate| (mode(Some(rate)), fourcc)));
            }
        }
        Ok(modes)
    }

    /// Starts streaming in the first mode of `format::order` that the driver sets as asked;
    /// returns the layout of its pictures and the rate of the mode.
    fn start(&self) -> Result<(Layout, Option<(u32, u32)>), CameraError> {
        let modes = self.modes()?;
        let listed: Vec<Mode> = modes.iter().map(|(mode, _)| *mode).collect();
        for index in super::format::order(&listed) {
            let (mode, fourcc) = modes[index];
            let (numerator, denominator) = mode.rate.map_or((1, 30), |(n, d)| (d, n));
            let request = Request {
                fourcc,
                width: mode.width,
                height: mode.height,
                numerator,
                denominator,
            };
            let mut format = Format::default();
            // SAFETY: the camera is open on this thread; the request and format are valid.
            let result = unsafe { junto_camera_start(self.0, &request, &mut format) };
            if result < 0 {
                return Err(error(result));
            }
            // The driver set another format: no buffer is mapped yet.
            if result != 0 {
                continue;
            }
            if let Some(failure) = v4l2::rate_failure(format.rate_error) {
                return Err(failure);
            }
            let layout = Layout {
                width: format.width,
                height: format.height,
                stride: format.stride as usize,
                pixel: PIXELS
                    .iter()
                    .find(|(code, _)| *code == format.fourcc)
                    .map(|(_, pixel)| *pixel)
                    .ok_or(CameraError::Unsupported)?,
                matrix: v4l2::matrix(format.encoding),
            };
            layout.length().ok_or(CameraError::Unsupported)?;
            return Ok((layout, mode.rate));
        }
        Err(CameraError::Unsupported)
    }

    /// The next buffer of the driver within 100 milliseconds.
    fn next(&self) -> Result<Option<Dequeued>, CameraError> {
        let mut buffer = Dequeued::default();
        // SAFETY: the camera streams on this thread and the out parameter is valid.
        match unsafe { junto_camera_next(self.0, 100, &mut buffer) } {
            1 => Ok(Some(buffer)),
            0 => Ok(None),
            code => Err(error(code)),
        }
    }

    /// Gives a buffer back to the driver, after a copy of its picture into `bytes`.
    fn release(&self, index: u32, bytes: Option<&mut [u8]>) -> Result<(), CameraError> {
        let (pointer, length) = bytes.map_or((std::ptr::null_mut(), 0), |bytes| {
            (bytes.as_mut_ptr(), bytes.len())
        });
        // SAFETY: the C function checks the index and the length against its mapping and
        // writes at most `length` bytes to a writable slice.
        match unsafe { junto_camera_release(self.0, index, pointer, length) } {
            0 => Ok(()),
            code => Err(error(code)),
        }
    }
}

fn nodes() -> Result<Vec<Node>, CameraError> {
    let entries = match std::fs::read_dir("/sys/class/video4linux") {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(CameraError::Failed(error.to_string())),
    };
    let mut nodes = Vec::new();
    for entry in entries.flatten() {
        let Ok(identity) = entry.path().canonicalize() else {
            continue;
        };
        let name =
            std::fs::read_to_string(entry.path().join("name")).unwrap_or_else(|_| "Camera".into());
        nodes.push(Node {
            identity: identity.to_string_lossy().into_owned(),
            name: name.trim().to_owned(),
            path: PathBuf::from("/dev").join(entry.file_name()),
        });
    }
    Ok(nodes)
}

enum Probe {
    Usable,
    Denied,
    Other,
}

/// Whether a node is a capture device with a mode that capture can use.
fn probe(system: System, path: &Path) -> Probe {
    match Camera::open(system, path) {
        Ok(camera) => match camera.modes() {
            Ok(modes) => {
                let listed: Vec<Mode> = modes.iter().map(|(mode, _)| *mode).collect();
                if super::format::select(&listed).is_some() {
                    Probe::Usable
                } else {
                    Probe::Other
                }
            }
            Err(_) => Probe::Other,
        },
        Err(NOT_PERMITTED | ACCESS_DENIED) => Probe::Denied,
        Err(_) => Probe::Other,
    }
}

/// The usable capture devices, in the order of their sysfs paths.
pub(super) fn devices(system: System, nodes: Vec<Node>) -> Vec<(CameraDevice, PathBuf)> {
    let mut devices: Vec<_> = nodes
        .into_iter()
        .filter(|node| matches!(probe(system, &node.path), Probe::Usable))
        .map(|node| {
            (
                CameraDevice {
                    id: node.identity,
                    name: node.name,
                },
                node.path,
            )
        })
        .collect();
    devices.sort_by(|a, b| a.0.id.cmp(&b.0.id));
    devices
}

/// The permission state of the camera nodes: restricted when no node is a usable camera and
/// the account may not open at least one of them. Nodes that cannot be opened are not
/// listed, because V4L2 states their kind only after an open.
pub(super) fn access(system: System, nodes: Vec<Node>) -> CameraAccess {
    let mut denied = false;
    for node in nodes {
        match probe(system, &node.path) {
            Probe::Usable => return CameraAccess::Allowed,
            Probe::Denied => denied = true,
            Probe::Other => {}
        }
    }
    if denied {
        CameraAccess::Restricted
    } else {
        CameraAccess::Allowed
    }
}

pub(super) struct SystemCamera;
impl CameraCapture for SystemCamera {
    fn access(&self) -> CameraAccess {
        nodes().map_or(CameraAccess::Allowed, |nodes| access(NATIVE, nodes))
    }
    fn request_access(&self) -> Pending<CameraAccess> {
        Box::pin(async { SystemCamera.access() })
    }
    fn devices(&self) -> Result<Vec<CameraDevice>, CameraError> {
        Ok(devices(NATIVE, nodes()?)
            .into_iter()
            .map(|(device, _)| device)
            .collect())
    }
    fn start(
        &self,
        chosen: Option<String>,
        sink: Arc<dyn CameraSink>,
    ) -> Pending<Result<CaptureStream, CameraError>> {
        Box::pin(start(NATIVE, nodes, chosen, sink))
    }
}

pub(super) async fn start(
    system: System,
    nodes: impl FnOnce() -> Result<Vec<Node>, CameraError> + Send + 'static,
    chosen: Option<String>,
    sink: Arc<dyn CameraSink>,
) -> Result<CaptureStream, CameraError> {
    worker::start(sink, move |delivery, ready| {
        let mut ready = Some(ready);
        let result =
            nodes().and_then(|nodes| capture(system, nodes, chosen, &delivery, &mut ready));
        if let Err(error) = result {
            if let Some(ready) = ready.take() {
                let _ = ready.send(Err(error));
            } else {
                delivery.ended(error);
            }
        }
    })
    .await
}

fn capture(
    system: System,
    nodes: Vec<Node>,
    chosen: Option<String>,
    delivery: &worker::Delivery,
    ready: &mut Option<worker::Ready>,
) -> Result<(), CameraError> {
    let devices = devices(system, nodes);
    let listed: Vec<_> = devices.iter().map(|(device, _)| device.clone()).collect();
    let chosen = choose(&listed, chosen.as_deref())?;
    let (_, path) = devices
        .iter()
        .find(|(device, _)| device.id == chosen.id)
        .ok_or(CameraError::DeviceGone)?;
    // The claim is declared before the camera, so that it is released after the close.
    let Some(_claim) = delivery.claim(&chosen.id) else {
        return Ok(());
    };
    let camera = Camera::open(system, path).map_err(error)?;
    let (layout, rate) = camera.start()?;
    let mut bytes = vec![0; layout.length().ok_or(CameraError::Unsupported)?];
    let start = Instant::now();
    let mut last_picture = start;
    let mut pacer = worker::Pacer::default();
    let mut sequence = None;
    while !delivery.stopped() {
        let Some(buffer) = camera.next()? else {
            if last_picture.elapsed() >= Duration::from_secs(5) {
                return Err(CameraError::Interrupted);
            }
            continue;
        };
        let kind = v4l2::buffer(&buffer, bytes.len());
        if kind != Buffer::Invalid {
            let lost = v4l2::lost(sequence.replace(buffer.sequence), buffer.sequence);
            worker::count(&chosen.id, u64::from(kind == Buffer::Copy), Some(lost));
        }
        match kind {
            Buffer::Invalid => {
                return Err(CameraError::Failed(
                    "the camera driver returned a buffer outside the capture".into(),
                ));
            }
            Buffer::Discard => {
                camera.release(buffer.index, None)?;
                if last_picture.elapsed() >= Duration::from_secs(5) {
                    return Err(CameraError::Interrupted);
                }
            }
            Buffer::Copy => {
                last_picture = Instant::now();
                let at = if buffer.timestamp != 0 {
                    Duration::from_nanos(buffer.timestamp)
                } else {
                    last_picture - start
                };
                if !pacer.due(at) {
                    camera.release(buffer.index, None)?;
                    continue;
                }
                camera.release(buffer.index, Some(&mut bytes))?;
                let frame = layout.convert(&bytes)?;
                if let Some(ready) = ready.take() {
                    let _ = ready.send(Ok(worker::Started {
                        width: layout.width,
                        height: layout.height,
                        rate,
                    }));
                }
                delivery.picture(Arc::new(frame));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod fake;
#[cfg(test)]
mod tests {
    use super::fake::*;
    use super::*;
    use std::sync::{Mutex, mpsc};

    #[test]
    fn permission_and_unplug_errors_are_distinct() {
        for code in [-1, -13] {
            assert_eq!(
                error(code),
                CameraError::NoPermission(CameraAccess::Restricted)
            );
        }
        for code in [-2, -5, -6, -19] {
            assert_eq!(error(code), CameraError::DeviceGone);
        }
        assert_eq!(error(-95), CameraError::Unsupported);
        assert!(matches!(error(-16), CameraError::Failed(_)));
    }

    enum Event {
        Picture(u32, u32, u8),
        Ended(CameraError),
    }
    struct Sink(Mutex<mpsc::Sender<Event>>);
    impl CameraSink for Sink {
        fn picture(&self, buffer: Arc<dyn livekit::webrtc::video_frame::VideoBuffer>) {
            let luma = buffer.as_i420().map_or(0, |i420| i420.data().0[0]);
            let _ =
                self.0
                    .lock()
                    .unwrap()
                    .send(Event::Picture(buffer.width(), buffer.height(), luma));
        }
        fn ended(&self, error: CameraError) {
            let _ = self.0.lock().unwrap().send(Event::Ended(error));
        }
    }

    async fn capture_of(
        fake: &'static Fake,
    ) -> (Result<CaptureStream, CameraError>, mpsc::Receiver<Event>) {
        let (send, receive) = mpsc::channel();
        let sink = Arc::new(Sink(Mutex::new(send)));
        let stream = start(fake.system(), move || Ok(fake.nodes()), None, sink).await;
        (stream, receive)
    }

    fn pictures(receive: &mpsc::Receiver<Event>, count: usize) -> Vec<(u32, u32, u8)> {
        (0..count)
            .map(
                |_| match receive.recv_timeout(std::time::Duration::from_secs(10)) {
                    Ok(Event::Picture(w, h, luma)) => (w, h, luma),
                    Ok(Event::Ended(error)) => panic!("capture ended: {error}"),
                    Err(error) => panic!("no picture: {error}"),
                },
            )
            .collect()
    }

    /// Waits until the capture of the device has closed every descriptor that it opened.
    fn closed(fake: &Fake, device: usize) -> Record {
        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let record = fake.record(device);
            if record.closed == record.opened || Instant::now() > deadline {
                return record;
            }
            std::thread::yield_now();
        }
    }

    #[tokio::test]
    async fn a_buffer_with_the_error_flag_and_a_short_buffer_are_discarded() {
        let mut frames = vec![
            Frame {
                flags: ERROR_FLAG,
                used: 10,
                ..frame(50)
            },
            Frame {
                used: 0,
                ..frame(60)
            },
            Frame {
                flags: ERROR_FLAG,
                ..frame(70)
            },
        ];
        frames.extend((0..10).map(|index| frame(100 + index)));
        let fake = Fake::new(vec![Spec {
            frames,
            ..Spec::default()
        }]);
        let (stream, receive) = capture_of(fake).await;
        let stream = stream.unwrap();
        let lumas: Vec<u8> = pictures(&receive, 10)
            .into_iter()
            .map(|(_, _, luma)| luma)
            .collect();
        assert_eq!(lumas, (100..110).collect::<Vec<u8>>());
        drop(stream);
        let record = closed(fake, 0);
        assert_eq!(record.closed, record.opened);
    }

    /// The device check compares the pictures that capture delivers with those that the
    /// driver gave and those that it lost.
    #[cfg(feature = "native-device-tests")]
    #[tokio::test]
    async fn received_and_lost_pictures_are_counted_for_the_device_check() {
        let mut frames: Vec<Frame> = (0..6).map(|index| frame(100 + index)).collect();
        frames[3].lost = 2;
        frames[4].flags = ERROR_FLAG;
        let fake = Fake::new(vec![Spec {
            frames,
            ..Spec::default()
        }]);
        let device = fake.nodes()[0].identity.clone();
        let (stream, receive) = capture_of(fake).await;
        let stream = stream.unwrap();
        let lumas: Vec<u8> = pictures(&receive, 5)
            .into_iter()
            .map(|(_, _, luma)| luma)
            .collect();
        assert_eq!(lumas, [100, 101, 102, 103, 105]);
        drop(stream);
        closed(fake, 0);
        assert_eq!(
            crate::camera::native_counts_for_test(&device),
            crate::camera::NativeCounts {
                received: 5,
                lost: Some(2),
            }
        );
    }

    #[tokio::test]
    async fn a_buffer_outside_the_mapped_ones_ends_capture() {
        let mut frames = vec![frame(100)];
        frames.push(Frame {
            index_offset: 64,
            ..frame(110)
        });
        let fake = Fake::new(vec![Spec {
            frames,
            ..Spec::default()
        }]);
        let (stream, receive) = capture_of(fake).await;
        let _stream = stream.unwrap();
        assert_eq!(pictures(&receive, 1)[0].2, 100);
        match receive.recv_timeout(std::time::Duration::from_secs(10)) {
            Ok(Event::Ended(CameraError::Failed(_))) => {}
            _ => panic!("capture did not end with a failure"),
        }
    }

    #[test]
    fn only_nodes_with_an_accepted_mode_at_a_capture_size_are_listed() {
        let fake = Fake::new(vec![
            Spec {
                modes: vec![mode(BAYER, 1280, 720, 30)],
                ..Spec::default()
            },
            Spec {
                modes: vec![mode(NV12, 1920, 1080, 30)],
                ..Spec::default()
            },
            Spec {
                capabilities: METADATA,
                ..Spec::default()
            },
            Spec {
                open_error: 13,
                ..Spec::default()
            },
            Spec {
                modes: vec![mode(MJPG, 1280, 720, 30), mode(YUYV, 640, 480, 30)],
                ..Spec::default()
            },
        ]);
        let listed: Vec<String> = devices(fake.system(), fake.nodes())
            .into_iter()
            .map(|(device, _)| device.name)
            .collect();
        assert_eq!(listed, ["Fake camera 4"]);
        assert_eq!(access(fake.system(), fake.nodes()), CameraAccess::Allowed);
    }

    #[test]
    fn nodes_that_refuse_access_without_a_usable_camera_make_access_restricted() {
        let fake = Fake::new(vec![
            Spec {
                open_error: 13,
                ..Spec::default()
            },
            Spec {
                capabilities: METADATA,
                ..Spec::default()
            },
        ]);
        assert!(devices(fake.system(), fake.nodes()).is_empty());
        assert_eq!(
            access(fake.system(), fake.nodes()),
            CameraAccess::Restricted
        );
        let fake = Fake::new(vec![Spec {
            capabilities: METADATA,
            ..Spec::default()
        }]);
        assert_eq!(access(fake.system(), fake.nodes()), CameraAccess::Allowed);
    }

    #[tokio::test]
    async fn the_largest_size_that_reaches_24_fps_is_requested_at_its_rate() {
        let fake = Fake::new(vec![
            Spec {
                modes: vec![mode(YUYV, 1280, 720, 10), mode(YUYV, 640, 480, 30)],
                ..Spec::default()
            },
            Spec {
                modes: vec![mode(NV12, 640, 480, 15), mode(NV12, 352, 288, 15)],
                ..Spec::default()
            },
        ]);
        let (stream, _receive) = capture_of(fake).await;
        let stream = stream.unwrap();
        assert_eq!((stream.width, stream.height), (640, 480));
        let record = fake.record(0);
        assert_eq!(
            (record.fourcc, record.width, record.height),
            (YUYV, 640, 480)
        );
        assert_eq!((record.numerator, record.denominator), (1, 30));
        drop(stream);
        closed(fake, 0);
        let (send, _receive) = mpsc::channel();
        let sink = Arc::new(Sink(Mutex::new(send)));
        let chosen = fake.nodes().remove(1).identity;
        let stream = start(fake.system(), move || Ok(fake.nodes()), Some(chosen), sink)
            .await
            .unwrap();
        assert_eq!((stream.width, stream.height), (640, 480));
        assert_eq!(
            (fake.record(1).numerator, fake.record(1).denominator),
            (1, 15)
        );
    }

    #[tokio::test]
    async fn a_refused_rate_request_starts_and_a_removed_device_does_not() {
        let fake = Fake::new(vec![Spec {
            rate_error: 22,
            ..Spec::default()
        }]);
        let (stream, receive) = capture_of(fake).await;
        let _stream = stream.unwrap();
        pictures(&receive, 3);
        for (code, expected) in [
            (19, CameraError::DeviceGone),
            (16, CameraError::Failed(String::new())),
        ] {
            let fake = Fake::new(vec![Spec {
                rate_error: code,
                ..Spec::default()
            }]);
            let (stream, _) = capture_of(fake).await;
            match (stream, expected) {
                (Err(CameraError::DeviceGone), CameraError::DeviceGone) => {}
                (Err(CameraError::Failed(_)), CameraError::Failed(_)) => {}
                (other, _) => panic!("{code}: {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn each_accepted_pixel_format_is_captured() {
        for fourcc in [NV12, YU12, YUYV] {
            let fake = Fake::new(vec![Spec {
                modes: vec![mode(fourcc, 352, 288, 30)],
                frames: (0..10).map(|index| frame(100 + index)).collect(),
                ..Spec::default()
            }]);
            let (stream, receive) = capture_of(fake).await;
            let stream = stream.unwrap();
            assert_eq!((stream.width, stream.height), (352, 288));
            let lumas: Vec<u8> = pictures(&receive, 3)
                .into_iter()
                .map(|(_, _, luma)| luma)
                .collect();
            assert_eq!(lumas, [100, 101, 102]);
        }
    }

    #[tokio::test]
    async fn a_mode_that_the_driver_sets_otherwise_gives_way_to_the_next() {
        let refused = super::fake::Mode {
            refuse: 1,
            ..mode(YUYV, 1280, 720, 30)
        };
        let full_range = super::fake::Mode {
            full_range: 1,
            ..mode(NV12, 1280, 720, 30)
        };
        for first in [refused, full_range] {
            let fake = Fake::new(vec![Spec {
                modes: vec![first, mode(YUYV, 640, 480, 30)],
                ..Spec::default()
            }]);
            let (stream, receive) = capture_of(fake).await;
            let stream = stream.unwrap();
            assert_eq!((stream.width, stream.height), (640, 480));
            pictures(&receive, 3);
        }
        // The refused mode is answered with the other one, which is full range.
        let fake = Fake::new(vec![Spec {
            modes: vec![
                refused,
                super::fake::Mode {
                    full_range: 1,
                    ..mode(NV12, 640, 480, 30)
                },
            ],
            ..Spec::default()
        }]);
        assert_eq!(
            capture_of(fake).await.0.unwrap_err(),
            CameraError::Unsupported
        );
    }

    /// The driver keeps the queue of a capture until its descriptor closes; a start of the
    /// same device while the earlier capture still closes gets EBUSY from the driver.
    #[tokio::test]
    async fn a_quick_stop_and_start_of_a_device_waits_for_the_first_capture() {
        let fake = Fake::new(vec![Spec {
            frames: (0..5).map(|index| frame(100 + index)).collect(),
            ..Spec::default()
        }]);
        let (stream, receive) = capture_of(fake).await;
        let stream = stream.unwrap();
        // The pictures of the device are used up: the capture waits in its poll.
        pictures(&receive, 5);
        drop(stream);
        let (again, receive) = capture_of(fake).await;
        let again = again.unwrap();
        assert_eq!(pictures(&receive, 5)[0].2, 100);
        drop(again);
        let record = closed(fake, 0);
        assert_eq!(record.closed, record.opened);
    }

    #[tokio::test]
    async fn the_count_of_buffers_that_the_driver_gives_is_mapped() {
        for buffers in [1, 6, 32] {
            let fake = Fake::new(vec![Spec {
                buffers,
                ..Spec::default()
            }]);
            let (stream, receive) = capture_of(fake).await;
            let stream = stream.unwrap();
            pictures(&receive, 3);
            assert_eq!(fake.record(0).mapped, buffers);
            drop(stream);
        }
    }
}
