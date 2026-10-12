//! The pixel buffer is written before publication and remains immutable while drawn.
use core_foundation::{
    base::{CFType, TCFType},
    boolean::CFBoolean,
    dictionary::CFDictionary,
    string::CFString,
};
use core_video::pixel_buffer::{
    CVPixelBuffer, kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferMetalCompatibilityKey,
    kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
};
use livekit::webrtc::video_frame::{I420Buffer, VideoBuffer};

pub(super) fn convert(planes: &I420Buffer, mirror: bool) -> Option<CVPixelBuffer> {
    let (width, height) = (planes.width() as usize, planes.height() as usize);
    let empty: CFDictionary<CFString, CFType> = CFDictionary::from_CFType_pairs(&[]);
    let options = CFDictionary::from_CFType_pairs(&[
        (
            unsafe { CFString::wrap_under_get_rule(kCVPixelBufferIOSurfacePropertiesKey) },
            empty.as_CFType(),
        ),
        (
            unsafe { CFString::wrap_under_get_rule(kCVPixelBufferMetalCompatibilityKey) },
            CFBoolean::true_value().as_CFType(),
        ),
    ]);
    let buffer = CVPixelBuffer::new(
        kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
        width,
        height,
        Some(&options),
    )
    .ok()?;
    if buffer.lock_base_address(0) != 0 {
        return None;
    }
    let (y, u, v) = planes.data();
    let (ys, us, vs) = planes.strides();
    unsafe {
        let base = buffer.get_base_address_of_plane(0).cast::<u8>();
        let stride = buffer.get_bytes_per_row_of_plane(0);
        for row in 0..height {
            let target = std::slice::from_raw_parts_mut(base.add(row * stride), width);
            let source = &y[row * ys as usize..row * ys as usize + width];
            if mirror {
                for (to, from) in target.iter_mut().zip(source.iter().rev()) {
                    *to = luma(*from);
                }
            } else {
                for (to, from) in target.iter_mut().zip(source) {
                    *to = luma(*from);
                }
            }
        }
        let base = buffer.get_base_address_of_plane(1).cast::<u8>();
        let stride = buffer.get_bytes_per_row_of_plane(1);
        let columns = width.div_ceil(2);
        for row in 0..height.div_ceil(2) {
            let target = std::slice::from_raw_parts_mut(base.add(row * stride), columns * 2);
            for column in 0..columns {
                let source = if mirror { columns - column - 1 } else { column };
                target[column * 2] = chroma(u[row * us as usize + source]);
                target[column * 2 + 1] = chroma(v[row * vs as usize + source]);
            }
        }
    }
    buffer.unlock_base_address(0);
    Some(buffer)
}

fn luma(value: u8) -> u8 {
    ((i32::from(value) - 16) * 255 / 219).clamp(0, 255) as u8
}
fn chroma(value: u8) -> u8 {
    ((i32::from(value) - 128) * 255 / 224 + 128).clamp(0, 255) as u8
}
