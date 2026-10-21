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
            let (u, v) = (&u[row * us as usize..], &v[row * vs as usize..]);
            for column in 0..columns {
                let (first, second) = sources(width, column, mirror);
                target[column * 2] = chroma(mean(u[first], u[second]));
                target[column * 2 + 1] = chroma(mean(v[first], v[second]));
            }
        }
    }
    buffer.unlock_base_address(0);
    Some(buffer)
}

/// The colour columns of the source that the two pixels of the colour column `column` of
/// the pixel buffer show. Without the mirror both are `column`. A mirror of an even width
/// keeps the two pixels of a column together. A mirror of an odd width moves the pixels by
/// one against the columns of colour: the two pixels of a column of the buffer come from
/// two neighbouring columns of the source, and the last column of the buffer holds one
/// pixel. The buffer takes the mean of the two, as a picture mirrored at its full size and
/// then reduced to one colour value for two pixels has.
pub(super) fn sources(width: usize, column: usize, mirror: bool) -> (usize, usize) {
    if !mirror {
        return (column, column);
    }
    let first = width - 1 - column * 2;
    let second = if column * 2 + 1 < width {
        first - 1
    } else {
        first
    };
    (first / 2, second / 2)
}

fn mean(one: u8, other: u8) -> u8 {
    ((u16::from(one) + u16::from(other)).div_ceil(2)) as u8
}

pub(super) fn luma(value: u8) -> u8 {
    ((i32::from(value) - 16) * 255 / 219).clamp(0, 255) as u8
}

pub(super) fn chroma(value: u8) -> u8 {
    ((i32::from(value) - 128) * 255 / 224 + 128).clamp(0, 255) as u8
}
