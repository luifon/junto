//! The V4L2 device in memory of `v4l2_fake.c`.
use std::ffi::{CString, c_char, c_int, c_void};
use std::path::PathBuf;

use super::{Node, System};

pub const NV12: u32 = u32::from_le_bytes(*b"NV12");
pub const YU12: u32 = u32::from_le_bytes(*b"YU12");
pub const YUYV: u32 = u32::from_le_bytes(*b"YUYV");
pub const MJPG: u32 = u32::from_le_bytes(*b"MJPG");
pub const BAYER: u32 = u32::from_le_bytes(*b"BA81");
/// V4L2_CAP_VIDEO_CAPTURE | V4L2_CAP_STREAMING.
pub const CAPTURE: u32 = 0x1 | 0x0400_0000;
/// V4L2_CAP_META_CAPTURE | V4L2_CAP_STREAMING.
pub const METADATA: u32 = 0x0080_0000 | 0x0400_0000;
pub const ERROR_FLAG: u32 = 0x40;
/// The whole image.
pub const FULL: u32 = u32::MAX;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Mode {
    pub fourcc: u32,
    pub width: u32,
    pub height: u32,
    /// The frame interval, in seconds; 0 when the device states none.
    pub numerator: u32,
    pub denominator: u32,
}

pub fn mode(fourcc: u32, width: u32, height: u32, rate: u32) -> Mode {
    Mode {
        fourcc,
        width,
        height,
        numerator: u32::from(rate != 0),
        denominator: rate,
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Frame {
    pub flags: u32,
    pub used: u32,
    pub index_offset: u32,
    pub value: u8,
}

pub fn frame(value: u8) -> Frame {
    Frame {
        flags: 0,
        used: FULL,
        index_offset: 0,
        value,
    }
}

#[repr(C)]
struct Device {
    path: *const c_char,
    open_error: c_int,
    capabilities: u32,
    modes: *const Mode,
    mode_count: usize,
    colorspace: u32,
    encoding: u32,
    quantization: u32,
    rate_error: c_int,
    buffers: u32,
    frames: *const Frame,
    frame_count: usize,
}

#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Record {
    pub fourcc: u32,
    pub width: u32,
    pub height: u32,
    pub numerator: u32,
    pub denominator: u32,
    pub buffers: u32,
    pub mapped: u32,
    pub opened: u32,
    pub closed: u32,
}

unsafe extern "C" {
    fn junto_fake_system(devices: *const Device, count: usize) -> *mut c_void;
    fn junto_fake_record(system: *mut c_void, device: usize, record: *mut Record);
}

/// The settings of one device of the fake.
#[derive(Clone)]
pub struct Spec {
    pub open_error: i32,
    pub capabilities: u32,
    pub modes: Vec<Mode>,
    /// V4L2 colorspace, Y'CbCr encoding and quantization of the format.
    pub colour: (u32, u32, u32),
    pub rate_error: i32,
    /// The count of buffers that the driver gives; 0 for the count asked for.
    pub buffers: u32,
    pub frames: Vec<Frame>,
}

impl Default for Spec {
    fn default() -> Self {
        Self {
            open_error: 0,
            capabilities: CAPTURE,
            modes: vec![mode(NV12, 640, 480, 30)],
            // sRGB, the default encoding and limited range, as cameras of uvcvideo report.
            colour: (8, 0, 2),
            rate_error: 0,
            buffers: 0,
            frames: (0..200)
                .map(|index| frame(16 + (index % 200) as u8))
                .collect(),
        }
    }
}

/// Devices in memory. A fake is never freed, because a capture thread can still use it after
/// its test has ended.
pub struct Fake {
    system: *mut c_void,
    paths: Vec<CString>,
    _specs: Vec<Spec>,
    _devices: Vec<Device>,
}
// SAFETY: the C devices take a lock for every call and their settings are not changed.
unsafe impl Send for Fake {}
// SAFETY: as for Send.
unsafe impl Sync for Fake {}

impl Fake {
    pub fn new(specs: Vec<Spec>) -> &'static Self {
        let paths: Vec<CString> = (0..specs.len())
            .map(|index| CString::new(format!("/dev/video{index}")).unwrap_or_default())
            .collect();
        let devices: Vec<Device> = specs
            .iter()
            .zip(&paths)
            .map(|(spec, path)| Device {
                path: path.as_ptr(),
                open_error: spec.open_error,
                capabilities: spec.capabilities,
                modes: spec.modes.as_ptr(),
                mode_count: spec.modes.len(),
                colorspace: spec.colour.0,
                encoding: spec.colour.1,
                quantization: spec.colour.2,
                rate_error: spec.rate_error,
                buffers: spec.buffers,
                frames: spec.frames.as_ptr(),
                frame_count: spec.frames.len(),
            })
            .collect();
        // SAFETY: the devices, their paths and settings live as long as the fake.
        let system = unsafe { junto_fake_system(devices.as_ptr(), devices.len()) };
        assert!(!system.is_null());
        Box::leak(Box::new(Self {
            system,
            paths,
            _specs: specs,
            _devices: devices,
        }))
    }

    pub fn system(&self) -> System {
        System(self.system.cast_const())
    }

    pub fn nodes(&self) -> Vec<Node> {
        self.paths
            .iter()
            .enumerate()
            .map(|(index, path)| Node {
                identity: format!("/sys/devices/fake/video4linux/video{index}"),
                name: format!("Fake camera {index}"),
                path: PathBuf::from(path.to_string_lossy().into_owned()),
            })
            .collect()
    }

    pub fn record(&self, device: usize) -> Record {
        let mut record = Record::default();
        // SAFETY: the system is alive and the index is one of its devices.
        unsafe { junto_fake_record(self.system, device, &mut record) };
        record
    }
}
