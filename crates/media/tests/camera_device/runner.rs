//! Opt-in Windows/Linux device check; listing never starts capture or requests permission.
//! With `JUNTO_TEST_CAMERA_DEVICE=1` it captures from a listed camera and checks the size,
//! the content and the rate of its pictures. On Windows, `--virtual` instead registers the
//! two sources of `virtual_source.rs`, starts a virtual camera with each, checks both and
//! removes them.

#[cfg(target_os = "windows")]
#[path = "virtual_profiles.rs"]
mod profiles;

#[cfg(any(target_os = "windows", target_os = "linux"))]
mod check {
    use junto_media::camera::{
        CameraCapture, CameraError, CameraSink, native_counts_for_test, native_rate_for_test,
    };
    use livekit::webrtc::video_frame::VideoBuffer;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    pub type Failure = Box<dyn std::error::Error>;

    /// The size, the colours (Y, Cb and Cr at the centres of the quarters) and the rate of a
    /// known source.
    #[derive(Clone, Copy)]
    pub struct Expected {
        pub size: (u32, u32),
        pub colours: [(u8, u8, u8); 4],
        /// The rate that capture asks the driver for.
        pub rate: (u32, u32),
    }

    /// What one picture holds.
    pub struct Picture {
        pub size: (u32, u32),
        /// Whether the luma plane has more than one value.
        pub luma_varies: bool,
        /// Whether a chroma plane has a value other than 0.
        pub chroma_set: bool,
        /// Y, Cb and Cr at the centres of the four quarters: top left, top right, bottom
        /// left, bottom right.
        pub quarters: [(u8, u8, u8); 4],
    }

    enum Event {
        Picture(Picture),
        Unreadable,
        Ended(CameraError),
    }

    struct Sink(std::sync::Mutex<tokio::sync::mpsc::UnboundedSender<Event>>);
    impl CameraSink for Sink {
        fn picture(&self, frame: Arc<dyn VideoBuffer>) {
            let event = frame.as_i420().map_or(Event::Unreadable, |i420| {
                let (y, u, v) = i420.data();
                let (sy, su, sv) = i420.strides();
                let (w, h) = (frame.width() as usize, frame.height() as usize);
                let quarter = |x: usize, row: usize| {
                    let luma = y[row * sy as usize + x];
                    let (x, row) = (x / 2, row / 2);
                    (luma, u[row * su as usize + x], v[row * sv as usize + x])
                };
                Event::Picture(Picture {
                    size: (frame.width(), frame.height()),
                    luma_varies: y.iter().any(|value| *value != y[0]),
                    chroma_set: u.iter().chain(v).any(|value| *value != 0),
                    quarters: [
                        quarter(w / 4, h / 4),
                        quarter(3 * w / 4, h / 4),
                        quarter(w / 4, 3 * h / 4),
                        quarter(3 * w / 4, 3 * h / 4),
                    ],
                })
            });
            let _ = self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .send(event);
        }
        fn ended(&self, error: CameraError) {
            let _ = self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .send(Event::Ended(error));
        }
    }

    async fn next(
        receive: &mut tokio::sync::mpsc::UnboundedReceiver<Event>,
    ) -> Result<Picture, Failure> {
        match tokio::time::timeout(Duration::from_secs(10), receive.recv()).await {
            Ok(Some(Event::Picture(picture))) => Ok(picture),
            Ok(Some(Event::Unreadable)) => Err("a picture is not in I420 planes".into()),
            Ok(Some(Event::Ended(error))) => Err(error.into()),
            Ok(None) => Err("camera delivery closed".into()),
            Err(_) => Err("no picture for 10 seconds".into()),
        }
    }

    /// Captures from the camera `id`: thirty pictures of the declared size whose planes hold
    /// a picture, then three seconds in which the driver loses no picture for lack of a
    /// buffer and capture delivers the pictures that the limit of 30 per second keeps of
    /// those that the driver gave. `expected` checks the size and the colours of a known
    /// source, and that it gives at least 80 percent of the rate that the driver was asked
    /// for. A real camera may give fewer pictures than it was asked for: it lowers its rate
    /// for a longer exposure in low light, so its rate is only printed.
    pub async fn capture(
        camera: &Arc<dyn CameraCapture>,
        id: String,
        expected: Option<Expected>,
    ) -> Result<(), Failure> {
        let (send, mut receive) = tokio::sync::mpsc::unbounded_channel();
        let sink = Arc::new(Sink(std::sync::Mutex::new(send)));
        if !matches!(
            camera
                .start(Some("absent-test-camera".into()), sink.clone())
                .await,
            Err(CameraError::DeviceGone)
        ) {
            return Err("an absent camera did not fail as gone".into());
        }
        let stream = camera.start(Some(id.clone()), sink).await?;
        let size = (stream.width, stream.height);
        if let Some(expected) = expected
            && expected.size != size
        {
            return Err(format!("capture size {size:?}, expected {:?}", expected.size).into());
        }
        for _ in 0..30 {
            let picture = next(&mut receive).await?;
            if picture.size != size {
                return Err(
                    format!("picture size {:?} differs from {size:?}", picture.size).into(),
                );
            }
            if !picture.luma_varies || !picture.chroma_set {
                return Err("a picture has a constant luma plane or empty chroma planes".into());
            }
            if let Some(Expected { colours, .. }) = expected {
                for (found, wanted) in picture.quarters.iter().zip(colours) {
                    if found.0.abs_diff(wanted.0) > 4
                        || found.1.abs_diff(wanted.1) > 4
                        || found.2.abs_diff(wanted.2) > 4
                    {
                        return Err(format!(
                            "colours {:?}, expected {colours:?}",
                            picture.quarters
                        )
                        .into());
                    }
                }
            }
        }
        if let Some(expected) = expected
            && native_rate_for_test() != Some(expected.rate)
        {
            return Err(format!(
                "capture asked for {:?} pictures per second, expected {:?}",
                native_rate_for_test(),
                expected.rate
            )
            .into());
        }
        let asked = native_rate_for_test().map(|(n, d)| f64::from(n) / f64::from(d));
        let before = native_counts_for_test(&id);
        let start = Instant::now();
        let mut count = 0u32;
        while start.elapsed() < Duration::from_secs(3) {
            next(&mut receive).await?;
            count += 1;
        }
        let seconds = start.elapsed().as_secs_f64();
        let after = native_counts_for_test(&id);
        drop(stream);
        let received = after.received - before.received;
        let lost = after.lost.map(|lost| lost - before.lost.unwrap_or(0));
        let source = received as f64 / seconds;
        let measured = f64::from(count) / seconds;
        println!(
            "Received 30 checked pictures at {} by {}; {measured:.1} per second of {source:.1} that the driver gave ({}) for a driver rate of {}; capture stopped",
            size.0,
            size.1,
            lost.map_or("the driver numbers no pictures".into(), |lost| format!(
                "{lost} lost"
            )),
            asked.map_or("not stated".into(), |rate| format!("{rate:.2}"))
        );
        if let Some(lost) = lost
            && lost > 0
        {
            return Err(format!(
                "the driver lost {lost} pictures for lack of a buffer: capture did not keep up"
            )
            .into());
        }
        // The limit keeps every picture up to 30.5 per second, else every second, third or
        // later one, with even gaps (ADR 0770). Two pictures may be between the driver and
        // this receiver at either end of the measured time.
        let kept = received as f64 / (source / 30.5).ceil().max(1.0);
        if f64::from(count) + 2.0 < 0.95 * kept || f64::from(count) > kept + 2.0 {
            return Err(format!(
                "{count} pictures delivered of {received} that the driver gave; the limit keeps {kept:.0}"
            )
            .into());
        }
        if let Some(asked) = asked {
            let limit = asked.min(30.0);
            if expected.is_some() && source < 0.8 * limit {
                return Err(format!(
                    "the source gave {source:.1} pictures per second for a limit of {limit:.2}"
                )
                .into());
            }
            if source < 0.8 * limit {
                println!(
                    "The camera gave fewer pictures than its driver rate; a camera lowers its rate for a longer exposure in low light."
                );
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod virtual_cameras {
    use super::check::Failure;
    use junto_media::camera::CameraCapture;
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::CoCreateGuid;
    use windows::Win32::System::Registry::*;
    use windows::core::{GUID, HSTRING, PCWSTR};

    fn key(class: &GUID) -> String {
        format!("SOFTWARE\\Classes\\CLSID\\{{{class:?}}}")
    }

    fn wide(text: &str) -> Vec<u8> {
        text.encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect()
    }

    /// Writes string values (`None`: the default value) to a volatile key of the machine.
    fn write(path: &str, values: &[(Option<&str>, &str)]) -> Result<(), Failure> {
        let mut handle = HKEY::default();
        // SAFETY: valid strings and out parameter; the key is closed below.
        unsafe {
            let status = RegCreateKeyExW(
                HKEY_LOCAL_MACHINE,
                &HSTRING::from(path),
                None,
                PCWSTR::null(),
                REG_OPTION_VOLATILE,
                KEY_WRITE,
                None,
                &mut handle,
                None,
            );
            if status != ERROR_SUCCESS {
                return Err(format!("the source cannot be registered: {status:?}").into());
            }
            let mut failure = None;
            for (name, value) in values {
                let name = name.map(HSTRING::from);
                let name = name
                    .as_ref()
                    .map_or(PCWSTR::null(), |name| PCWSTR(name.as_ptr()));
                let status = RegSetValueExW(handle, name, None, REG_SZ, Some(&wide(value)));
                if status != ERROR_SUCCESS {
                    failure = Some(status);
                }
            }
            let _ = RegCloseKey(handle);
            match failure {
                Some(status) => Err(format!("the source cannot be registered: {status:?}").into()),
                None => Ok(()),
            }
        }
    }

    /// The registration of the two sources for this check, removed when dropped. Each check
    /// registers new class identifiers, because COM in the Frame Server keeps the class of an
    /// identifier that it loaded once, and a later check would use the earlier library. The
    /// Frame Server runs as a service and reads the classes of the machine, so this needs an
    /// administrator.
    struct Registered(Vec<GUID>);

    impl Registered {
        fn new(library: &std::path::Path) -> Result<Self, Failure> {
            let mut registered = Registered(Vec::new());
            for profile in [super::profiles::UNCOMPRESSED, super::profiles::COMPRESSED] {
                // SAFETY: creates a new identifier.
                let class = unsafe { CoCreateGuid()? };
                registered.0.push(class);
                write(&key(&class), &[(Some(super::profiles::PROFILE), profile)])?;
                write(
                    &format!("{}\\InprocServer32", key(&class)),
                    &[
                        (None, &library.to_string_lossy()),
                        (Some("ThreadingModel"), "Both"),
                    ],
                )?;
            }
            Ok(registered)
        }
    }

    impl Drop for Registered {
        fn drop(&mut self) {
            for class in &self.0 {
                // SAFETY: removes only a key that `new` created.
                let _ = unsafe { RegDeleteTreeW(HKEY_LOCAL_MACHINE, &HSTRING::from(key(class))) };
            }
        }
    }

    /// The copy of the source library that this check registers. The Frame Server keeps a
    /// library loaded until it stops, and Windows does not replace a loaded file, so each check
    /// registers a copy named by its process; a copy that is no longer loaded is removed by a
    /// later check.
    struct LibraryCopy(std::path::PathBuf);

    impl LibraryCopy {
        fn new(built: &std::path::Path) -> Result<Self, Failure> {
            let folder = built.parent().ok_or("the library has no folder")?;
            for entry in std::fs::read_dir(folder)?.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with("virtual_camera_source-") && name.ends_with(".dll") {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
            let copy = folder.join(format!("virtual_camera_source-{}.dll", std::process::id()));
            std::fs::copy(built, &copy)?;
            Ok(Self(copy))
        }
    }

    impl Drop for LibraryCopy {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// A started virtual camera, removed when dropped.
    struct Started(IMFVirtualCamera);

    impl Drop for Started {
        fn drop(&mut self) {
            // SAFETY: the camera was created and started by this process.
            unsafe {
                let _ = self.0.Remove();
                let _ = self.0.Shutdown();
            }
        }
    }

    pub fn check(
        camera: &Arc<dyn CameraCapture>,
        runtime: &tokio::runtime::Runtime,
    ) -> Result<(), Failure> {
        let built = std::env::current_exe()?.with_file_name("virtual_camera_source.dll");
        if !built.exists() {
            return Err(format!("{} is not built", built.display()).into());
        }
        let copy = LibraryCopy::new(&built)?;
        let registered = Registered::new(&copy.0)?;
        // The colours of the four quarters of `virtual_source.rs`.
        let colours = [
            (150, 63, 63),
            (130, 73, 183),
            (110, 183, 73),
            (100, 193, 193),
        ];
        for (class, name) in registered
            .0
            .iter()
            .zip(["Junto test camera NV12", "Junto test camera MJPEG"])
        {
            let source = HSTRING::from(format!("{{{class:?}}}"));
            // SAFETY: valid strings; the camera lives for this session of the process.
            let started = unsafe {
                let created = MFCreateVirtualCamera(
                    MFVirtualCameraType_SoftwareCameraSource,
                    MFVirtualCameraLifetime_Session,
                    MFVirtualCameraAccess_CurrentUser,
                    &HSTRING::from(name),
                    &source,
                    None,
                )?;
                created.Start(None)?;
                Started(created)
            };
            let deadline = Instant::now() + Duration::from_secs(10);
            let id = loop {
                let devices = camera.devices()?;
                // Windows adds the kind of the camera to the name.
                if let Some(device) = devices.iter().find(|device| device.name.starts_with(name)) {
                    break device.id.clone();
                }
                if Instant::now() > deadline {
                    let names: Vec<_> = devices.into_iter().map(|device| device.name).collect();
                    // SAFETY: asks the Frame Server for the source of the started camera.
                    let source = unsafe { started.0.GetMediaSource() }.map(|_| ());
                    return Err(format!(
                        "{name} is not listed (listed: {names:?}); its source: {source:?}"
                    )
                    .into());
                }
                std::thread::sleep(Duration::from_millis(200));
            };
            println!("{name}:");
            let expected = super::check::Expected {
                size: (1280, 720),
                colours,
                rate: (30, 1),
            };
            runtime.block_on(super::check::capture(camera, id, Some(expected)))?;
            drop(started);
        }
        Ok(())
    }
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn main() -> Result<(), check::Failure> {
    use junto_media::camera::{CameraAccess, native_for_test};

    let camera = native_for_test();
    let devices = camera.devices()?;
    println!(
        "Camera devices: {}; access: {:?}",
        devices.len(),
        camera.access()
    );
    if std::env::args().any(|arg| arg == "--list") {
        return Ok(());
    }
    if std::env::var("JUNTO_TEST_CAMERA_DEVICE").as_deref() != Ok("1") {
        return Err("set JUNTO_TEST_CAMERA_DEVICE=1 to authorize this device check".into());
    }
    let runtime = tokio::runtime::Runtime::new()?;
    #[cfg(target_os = "windows")]
    if std::env::args().any(|arg| arg == "--virtual") {
        return virtual_cameras::check(&camera, &runtime);
    }
    if devices.is_empty() {
        return Err("no camera exists; native capture was not tested".into());
    }
    if camera.access() != CameraAccess::Allowed {
        return Err("camera permission is not already granted; no request was made".into());
    }
    let index: usize = std::env::var("JUNTO_TEST_CAMERA_INDEX")
        .unwrap_or_else(|_| "0".into())
        .parse()?;
    let chosen = devices
        .get(index)
        .ok_or("camera index is absent")?
        .id
        .clone();
    runtime.block_on(check::capture(&camera, chosen, None))
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
fn main() {
    eprintln!("This device check is for Windows and Linux; use the native camera stage on macOS.");
}
