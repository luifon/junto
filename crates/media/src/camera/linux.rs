//! Direct V4L2 capture. No permission dialog or session-manager connection is opened.
use std::ffi::{CString, c_char, c_int, c_void};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::matrix::Tag;
use super::raw::{Layout, Pixel};
use super::{worker, *};

#[repr(C)]
#[derive(Default)]
struct Format {
    width: u32,
    height: u32,
    stride: u32,
    pixel: u32,
    matrix: u32,
}

unsafe extern "C" {
    fn junto_camera_probe(path: *const c_char) -> c_int;
    fn junto_camera_open(
        path: *const c_char,
        format: *mut Format,
        camera: *mut *mut c_void,
    ) -> c_int;
    fn junto_camera_next(camera: *mut c_void, bytes: *mut u8, length: usize) -> c_int;
    fn junto_camera_close(camera: *mut c_void);
}

fn error(code: i32) -> CameraError {
    match code {
        -1 | -13 => CameraError::NoPermission(CameraAccess::Refused),
        -2 | -5 | -6 | -19 => CameraError::DeviceGone,
        -22 | -25 | -95 => CameraError::Unsupported,
        _ => CameraError::Failed(std::io::Error::from_raw_os_error(-code).to_string()),
    }
}

fn devices() -> Result<Vec<(CameraDevice, PathBuf)>, CameraError> {
    let entries = match std::fs::read_dir("/sys/class/video4linux") {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(CameraError::Failed(error.to_string())),
    };
    let mut devices = Vec::new();
    for entry in entries.flatten() {
        let node = PathBuf::from("/dev").join(entry.file_name());
        let Ok(path) = CString::new(node.as_os_str().as_bytes()) else {
            continue;
        };
        // SAFETY: the C function reads a terminated path and closes its probe descriptor.
        match unsafe { junto_camera_probe(path.as_ptr()) } {
            1 | -1 | -13 => {}
            _ => continue,
        }
        let Ok(identity) = entry.path().canonicalize() else {
            continue;
        };
        let name =
            std::fs::read_to_string(entry.path().join("name")).unwrap_or_else(|_| "Camera".into());
        devices.push((
            CameraDevice {
                id: identity.to_string_lossy().into_owned(),
                name: name.trim().to_owned(),
            },
            node,
        ));
    }
    devices.sort_by(|a, b| a.0.id.cmp(&b.0.id));
    Ok(devices)
}

pub(super) struct SystemCamera;
impl CameraCapture for SystemCamera {
    fn access(&self) -> CameraAccess {
        CameraAccess::Allowed
    }
    fn request_access(&self) -> Pending<CameraAccess> {
        Box::pin(async { CameraAccess::Allowed })
    }
    fn devices(&self) -> Result<Vec<CameraDevice>, CameraError> {
        Ok(devices()?.into_iter().map(|(device, _)| device).collect())
    }
    fn start(
        &self,
        chosen: Option<String>,
        sink: Arc<dyn CameraSink>,
    ) -> Pending<Result<CaptureStream, CameraError>> {
        Box::pin(worker::start(sink, move |delivery, ready| {
            let mut ready = Some(ready);
            let result = capture(chosen, &delivery, &mut ready);
            if let Err(error) = result {
                if let Some(ready) = ready.take() {
                    let _ = ready.send(Err(error));
                } else {
                    delivery.ended(error);
                }
            }
        }))
    }
}

struct Device(*mut c_void);
impl Drop for Device {
    fn drop(&mut self) {
        // SAFETY: only the capture thread owns the pointer returned by open.
        unsafe { junto_camera_close(self.0) };
    }
}

fn capture(
    chosen: Option<String>,
    delivery: &worker::Delivery,
    ready: &mut Option<worker::Ready>,
) -> Result<(), CameraError> {
    let devices = devices()?;
    let listed: Vec<_> = devices.iter().map(|(device, _)| device.clone()).collect();
    let chosen = choose(&listed, chosen.as_deref())?;
    let (_, path) = devices
        .iter()
        .find(|(device, _)| device.id == chosen.id)
        .ok_or(CameraError::DeviceGone)?;
    let path = CString::new(path.as_os_str().as_bytes()).map_err(|_| CameraError::DeviceGone)?;
    let mut format = Format::default();
    let mut pointer = std::ptr::null_mut();
    // SAFETY: valid out parameters; a successful call transfers one native capture.
    let result = unsafe { junto_camera_open(path.as_ptr(), &mut format, &mut pointer) };
    if result < 0 {
        return Err(error(result));
    }
    let device = Device(pointer);
    let layout = Layout {
        width: format.width,
        height: format.height,
        stride: format.stride as usize,
        pixel: match format.pixel {
            0 => Pixel::Nv12,
            1 => Pixel::I420,
            2 => Pixel::Yuy2,
            _ => return Err(CameraError::Unsupported),
        },
        matrix: match format.matrix {
            0 => Tag::Missing,
            1 => Tag::Bt601,
            2 => Tag::Bt709,
            8 => Tag::Smpte240m,
            6 => Tag::Bt2020,
            _ => Tag::Other,
        },
    };
    super::format::choose(|size| size == (layout.width, layout.height))
        .ok_or(CameraError::Unsupported)?;
    let mut bytes = vec![0; layout.length().ok_or(CameraError::Unsupported)?];
    let mut last_picture = Instant::now();
    let mut last_delivery = None;
    while !delivery.stopped() {
        // SAFETY: the capture is alive on this thread and bytes is writable for its length.
        let result = unsafe { junto_camera_next(device.0, bytes.as_mut_ptr(), bytes.len()) };
        if result < 0 {
            return Err(error(result));
        }
        if result == 0 {
            if last_picture.elapsed() >= Duration::from_secs(5) {
                return Err(CameraError::Interrupted);
            }
            continue;
        }
        last_picture = Instant::now();
        if last_delivery
            .is_some_and(|last: Instant| last.elapsed() < Duration::from_secs_f64(1.0 / 30.0))
        {
            continue;
        }
        let frame = layout.convert(&bytes)?;
        if let Some(ready) = ready.take() {
            let _ = ready.send(Ok((layout.width, layout.height)));
        }
        delivery.picture(Arc::new(frame));
        last_delivery = Some(Instant::now());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn permission_and_unplug_errors_are_distinct() {
        for code in [-1, -13] {
            assert_eq!(
                error(code),
                CameraError::NoPermission(CameraAccess::Refused)
            );
        }
        for code in [-2, -5, -6, -19] {
            assert_eq!(error(code), CameraError::DeviceGone);
        }
        assert_eq!(error(-95), CameraError::Unsupported);
        assert!(matches!(error(-16), CameraError::Failed(_)));
    }
}
