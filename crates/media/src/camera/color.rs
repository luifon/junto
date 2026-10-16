//! Normalize camera buffers to the publication path's limited-range 601 matrix (ADR 0770).
use core_foundation::base::TCFType;
use core_foundation::string::CFString;
use core_video::pixel_buffer::CVPixelBuffer;
use livekit::webrtc::video_frame::{VideoBuffer, native::NativeBuffer};
use std::sync::Arc;

use super::matrix::{self, Matrix, Tag};

/// The pixel format that the capture output requests: limited-range 4:2:0 with two planes.
const LIMITED_420: u32 = u32::from_be_bytes(*b"420v");

/// The matrix of a limited-range 4:2:0 camera buffer, or `None` for another pixel format.
fn source(image: &CVPixelBuffer) -> Option<Matrix> {
    use core_video::buffer::TCVBuffer;
    use core_video::image_buffer::{
        kCVImageBufferYCbCrMatrix_ITU_R_601_4, kCVImageBufferYCbCrMatrix_ITU_R_709_2,
        kCVImageBufferYCbCrMatrix_ITU_R_2020, kCVImageBufferYCbCrMatrix_SMPTE_240M_1995,
        kCVImageBufferYCbCrMatrixKey,
    };

    if image.get_pixel_format() != LIMITED_420 {
        return None;
    }
    // SAFETY: these immutable framework strings are valid for the life of the process.
    let tag = unsafe {
        let named = |name| CFString::wrap_under_get_rule(name).as_CFType();
        match image.as_buffer().get_attachment(
            &CFString::wrap_under_get_rule(kCVImageBufferYCbCrMatrixKey),
            None,
        ) {
            None => Tag::Missing,
            Some(tag) if tag == named(kCVImageBufferYCbCrMatrix_ITU_R_601_4) => Tag::Bt601,
            Some(tag) if tag == named(kCVImageBufferYCbCrMatrix_ITU_R_709_2) => Tag::Bt709,
            Some(tag) if tag == named(kCVImageBufferYCbCrMatrix_SMPTE_240M_1995) => Tag::Smpte240m,
            Some(tag) if tag == named(kCVImageBufferYCbCrMatrix_ITU_R_2020) => Tag::Bt2020,
            Some(_) => Tag::Other,
        }
    };
    Some(matrix::source(tag, image.get_height() as u32))
}

/// Delivers a 601 buffer on the native path and converts a buffer in another matrix into owned
/// 601 I420 planes. Returns `None` for a pixel format other than the requested one.
pub(super) fn convert(image: &CVPixelBuffer) -> Option<Arc<dyn VideoBuffer>> {
    use livekit::webrtc::video_frame::native::VideoFrameBufferExt;

    let from = source(image)?;
    let owned = image.clone();
    let raw = owned.as_concrete_TypeRef();
    std::mem::forget(owned);
    // SAFETY: the native buffer takes ownership of the retained pixel buffer.
    let native = unsafe { NativeBuffer::from_cv_pixel_buffer(raw.cast()) };
    if from == Matrix::BT601 {
        return Some(Arc::new(native));
    }
    Some(Arc::new(matrix::to_601(&native.to_i420(), from)))
}

/// For the native stage: whether a buffer that the camera delivered on the native path is in the
/// 601 matrix. `None` for a converted buffer.
#[cfg(feature = "native-device-tests")]
pub(super) fn native_is_601(buffer: &dyn VideoBuffer) -> Option<bool> {
    let native = buffer.as_native()?;
    // SAFETY: the native buffer holds a valid pixel buffer; the wrapper takes its own reference.
    let image = unsafe { CVPixelBuffer::wrap_under_get_rule(native.get_cv_pixel_buffer().cast()) };
    Some(source(&image) == Some(Matrix::BT601))
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_foundation::string::CFStringRef;
    use core_video::buffer::{TCVBuffer, kCVAttachmentMode_ShouldPropagate};
    use core_video::image_buffer::{
        kCVImageBufferYCbCrMatrix_ITU_R_601_4, kCVImageBufferYCbCrMatrix_ITU_R_709_2,
        kCVImageBufferYCbCrMatrix_ITU_R_2020, kCVImageBufferYCbCrMatrix_P3_D65,
        kCVImageBufferYCbCrMatrix_SMPTE_240M_1995, kCVImageBufferYCbCrMatrixKey,
    };
    use livekit::webrtc::video_frame::VideoBufferType;

    /// A limited-range 420v buffer of one colour, with the given matrix tag.
    fn camera_buffer(
        (width, height): (usize, usize),
        (luma, cb, cr): (u8, u8, u8),
        tag: Option<CFString>,
    ) -> CVPixelBuffer {
        let image = CVPixelBuffer::new(LIMITED_420, width, height, None).unwrap();
        assert_eq!(image.lock_base_address(0), 0);
        // SAFETY: the planes are locked; each luma row has `width` bytes and each chroma row
        // has `width.div_ceil(2)` byte pairs.
        unsafe {
            for row in 0..height {
                std::ptr::write_bytes(
                    image
                        .get_base_address_of_plane(0)
                        .cast::<u8>()
                        .add(row * image.get_bytes_per_row_of_plane(0)),
                    luma,
                    width,
                );
            }
            for row in 0..height.div_ceil(2) {
                let uv = image
                    .get_base_address_of_plane(1)
                    .cast::<u8>()
                    .add(row * image.get_bytes_per_row_of_plane(1));
                for col in 0..width.div_ceil(2) {
                    *uv.add(col * 2) = cb;
                    *uv.add(col * 2 + 1) = cr;
                }
            }
            if let Some(tag) = tag {
                image.as_buffer().set_attachment(
                    &CFString::wrap_under_get_rule(kCVImageBufferYCbCrMatrixKey),
                    &tag.as_CFType(),
                    kCVAttachmentMode_ShouldPropagate,
                );
            }
        }
        assert_eq!(image.unlock_base_address(0), 0);
        image
    }

    fn tag(name: CFStringRef) -> Option<CFString> {
        // SAFETY: the framework strings are valid for the life of the process.
        Some(unsafe { CFString::wrap_under_get_rule(name) })
    }

    /// Converts a buffer and decodes its centre as the receiver does, with the 601 matrix.
    fn received(image: &CVPixelBuffer) -> (VideoBufferType, (u8, u8, u8)) {
        let buffer = convert(image).expect("the camera buffer is accepted");
        let size = (buffer.width(), buffer.height());
        let frame = crate::share::picture::convert(&*buffer, size).unwrap();
        let pixel = frame.pixel(size.0 / 2, size.1 / 2).unwrap();
        (buffer.buffer_type(), (pixel.0, pixel.1, pixel.2))
    }

    fn assert_green(actual: (u8, u8, u8)) {
        for (a, expected) in [(actual.0, 26), (actual.1, 204), (actual.2, 26)] {
            assert!(a.abs_diff(expected) <= 4, "{actual:?}");
        }
    }

    // The limited-range encodings of the colour (26, 204, 26) under each matrix.
    const GREEN_601: (u8, u8, u8) = (128, 76, 63);
    const GREEN_709: (u8, u8, u8) = (148, 68, 57);
    const GREEN_240M: (u8, u8, u8) = (145, 68, 58);
    const GREEN_2020: (u8, u8, u8) = (142, 72, 56);

    #[test]
    fn tagged_709_camera_green_reaches_the_receiver_as_green() {
        let image = camera_buffer(
            (4, 4),
            GREEN_709,
            tag(unsafe { kCVImageBufferYCbCrMatrix_ITU_R_709_2 }),
        );
        let (kind, colour) = received(&image);
        assert_eq!(kind, VideoBufferType::I420);
        assert_green(colour);
    }

    #[test]
    fn tagged_601_camera_buffers_keep_the_native_path() {
        let image = camera_buffer(
            (4, 4),
            GREEN_601,
            tag(unsafe { kCVImageBufferYCbCrMatrix_ITU_R_601_4 }),
        );
        let (kind, colour) = received(&image);
        assert_eq!(kind, VideoBufferType::Native);
        assert_green(colour);
    }

    #[test]
    fn tagged_240m_and_2020_camera_green_reaches_the_receiver_as_green() {
        for (name, encoded) in [
            (
                unsafe { kCVImageBufferYCbCrMatrix_SMPTE_240M_1995 },
                GREEN_240M,
            ),
            (unsafe { kCVImageBufferYCbCrMatrix_ITU_R_2020 }, GREEN_2020),
        ] {
            let (kind, colour) = received(&camera_buffer((4, 4), encoded, tag(name)));
            assert_eq!(kind, VideoBufferType::I420);
            assert_green(colour);
        }
    }

    #[test]
    fn untagged_camera_buffers_use_the_matrix_of_their_size() {
        let (kind, colour) = received(&camera_buffer((640, 480), GREEN_601, None));
        assert_eq!(kind, VideoBufferType::Native);
        assert_green(colour);
        let (kind, colour) = received(&camera_buffer((1280, 720), GREEN_709, None));
        assert_eq!(kind, VideoBufferType::I420);
        assert_green(colour);
    }

    #[test]
    fn camera_buffers_with_another_matrix_tag_use_the_matrix_of_their_size() {
        let unknown = Some(CFString::from_static_string("Matrix_Unknown_To_Junto"));
        for other in [tag(unsafe { kCVImageBufferYCbCrMatrix_P3_D65 }), unknown] {
            let (kind, colour) = received(&camera_buffer((352, 288), GREEN_601, other.clone()));
            assert_eq!(kind, VideoBufferType::Native);
            assert_green(colour);
            let (kind, colour) = received(&camera_buffer((1280, 720), GREEN_709, other));
            assert_eq!(kind, VideoBufferType::I420);
            assert_green(colour);
        }
    }

    #[test]
    fn camera_buffers_of_another_pixel_format_are_refused() {
        let image = CVPixelBuffer::new(u32::from_be_bytes(*b"420f"), 4, 4, None).unwrap();
        // SAFETY: the framework strings are valid for the life of the process.
        unsafe {
            image.as_buffer().set_attachment(
                &CFString::wrap_under_get_rule(kCVImageBufferYCbCrMatrixKey),
                &CFString::wrap_under_get_rule(kCVImageBufferYCbCrMatrix_ITU_R_601_4).as_CFType(),
                kCVAttachmentMode_ShouldPropagate,
            );
        }
        assert!(convert(&image).is_none());
    }
}
