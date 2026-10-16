//! Normalize tagged camera buffers to the publication path's limited-range 601 matrix.
use core_foundation::base::TCFType;
use core_video::pixel_buffer::CVPixelBuffer;
use livekit::webrtc::video_frame::{VideoBuffer, native::NativeBuffer};
use std::sync::Arc;

pub(super) fn convert(image: &CVPixelBuffer) -> Option<Arc<dyn VideoBuffer>> {
    use core_foundation::string::CFString;
    use core_video::buffer::TCVBuffer;
    use core_video::image_buffer::{
        kCVImageBufferYCbCrMatrix_ITU_R_601_4, kCVImageBufferYCbCrMatrix_ITU_R_709_2,
        kCVImageBufferYCbCrMatrixKey,
    };
    use livekit::webrtc::video_frame::{I420Buffer, native::VideoFrameBufferExt};

    if image.get_pixel_format() != u32::from_be_bytes(*b"420v") {
        return None;
    }
    // SAFETY: these immutable framework strings are valid for the life of the process.
    let matrix = unsafe {
        let tag = image.as_buffer().get_attachment(
            &CFString::wrap_under_get_rule(kCVImageBufferYCbCrMatrixKey),
            None,
        )?;
        if tag == CFString::wrap_under_get_rule(kCVImageBufferYCbCrMatrix_ITU_R_601_4).as_CFType() {
            false
        } else if tag
            == CFString::wrap_under_get_rule(kCVImageBufferYCbCrMatrix_ITU_R_709_2).as_CFType()
        {
            true
        } else {
            return None;
        }
    };
    let owned = image.clone();
    let raw = owned.as_concrete_TypeRef();
    std::mem::forget(owned);
    // SAFETY: the native buffer takes ownership of the retained pixel buffer.
    let native = unsafe { NativeBuffer::from_cv_pixel_buffer(raw.cast()) };
    if !matrix {
        return Some(Arc::new(native));
    }
    let source = native.to_i420();
    let (width, height) = (source.width() as usize, source.height() as usize);
    let (sy, su, sv) = source.strides();
    let (y, u, v) = source.data();
    let mut output = I420Buffer::new(width as u32, height as u32);
    let (dy, du, dv) = output.strides();
    let (oy, ou, ov) = output.data_mut();
    let byte = |value: f32| value.round().clamp(0.0, 255.0) as u8;
    for row in 0..height.div_ceil(2) {
        for col in 0..width.div_ceil(2) {
            let cb = f32::from(u[row * su as usize + col]) - 128.0;
            let cr = f32::from(v[row * sv as usize + col]) - 128.0;
            // 709 inverse followed by 601 forward, without an intermediate RGB clip.
            let red = 2.0 * (1.0 - 0.2126) * cr;
            let blue = 2.0 * (1.0 - 0.0722) * cb;
            let green = -(0.2126 * red + 0.0722 * blue) / 0.7152;
            let luma = 0.299 * red + 0.587 * green + 0.114 * blue;
            ou[row * du as usize + col] = byte(128.0 + (blue - luma) / 1.772);
            ov[row * dv as usize + col] = byte(128.0 + (red - luma) / 1.402);
            for py in row * 2..(row * 2 + 2).min(height) {
                for px in col * 2..(col * 2 + 2).min(width) {
                    oy[py * dy as usize + px] =
                        byte(f32::from(y[py * sy as usize + px]) + luma * (219.0 / 224.0));
                }
            }
        }
    }
    Some(Arc::new(output))
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_foundation::string::CFString;
    use core_video::buffer::{TCVBuffer, kCVAttachmentMode_ShouldPropagate};
    use core_video::image_buffer::{
        kCVImageBufferYCbCrMatrix_ITU_R_709_2, kCVImageBufferYCbCrMatrixKey,
    };

    #[test]
    fn tagged_709_camera_green_reaches_the_receiver_as_green() {
        let image = CVPixelBuffer::new(u32::from_be_bytes(*b"420v"), 4, 4, None).unwrap();
        assert_eq!(image.lock_base_address(0), 0);
        // SAFETY: the allocated planes are locked and each row has at least four bytes.
        unsafe {
            for row in 0..4 {
                std::ptr::write_bytes(
                    image
                        .get_base_address_of_plane(0)
                        .cast::<u8>()
                        .add(row * image.get_bytes_per_row_of_plane(0)),
                    148,
                    4,
                );
            }
            for row in 0..2 {
                let uv = image
                    .get_base_address_of_plane(1)
                    .cast::<u8>()
                    .add(row * image.get_bytes_per_row_of_plane(1));
                for col in 0..2 {
                    *uv.add(col * 2) = 67;
                    *uv.add(col * 2 + 1) = 56;
                }
            }
            image.as_buffer().set_attachment(
                &CFString::wrap_under_get_rule(kCVImageBufferYCbCrMatrixKey),
                &CFString::wrap_under_get_rule(kCVImageBufferYCbCrMatrix_ITU_R_709_2).as_CFType(),
                kCVAttachmentMode_ShouldPropagate,
            );
        }
        assert_eq!(image.unlock_base_address(0), 0);
        let buffer = convert(&image).unwrap();
        let frame = crate::share::picture::convert(&*buffer, (4, 4)).unwrap();
        let actual = frame.pixel(2, 2).unwrap();
        for (a, expected) in [(actual.0, 26), (actual.1, 204), (actual.2, 26)] {
            assert!(a.abs_diff(expected) <= 4, "{actual:?}");
        }
    }
}
