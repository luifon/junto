//! The scale of planes of brightness and colour that a picture reads where they are.

use livekit::webrtc::video_frame::{I420Buffer, VideoBuffer};

/// New planes of `size` with the picture of `planes`, read without a copy of them. The call
/// library scales only planes that it may write (ADR 0774).
pub(super) fn scaled(planes: &I420Buffer, size: (u32, u32)) -> Option<I420Buffer> {
    let (width, height) = (i32::try_from(size.0).ok()?, i32::try_from(size.1).ok()?);
    let source = (
        i32::try_from(planes.width()).ok()?,
        i32::try_from(planes.height()).ok()?,
    );
    let (src_y, src_u, src_v) = planes.data();
    let (src_stride_y, src_stride_u, src_stride_v) = planes.strides();
    let mut scaled = I420Buffer::new(size.0, size.1);
    let (dst_stride_y, dst_stride_u, dst_stride_v) = scaled.strides();
    let (dst_y, dst_u, dst_v) = scaled.data_mut();
    // SAFETY: each pointer is the start of a plane of its buffer with the stride and the size
    // of that buffer; `planes` is borrowed and `scaled` is owned here for the whole call.
    unsafe {
        webrtc_sys::yuv_helper::ffi::i420_scale(
            src_y.as_ptr(),
            src_stride_y as i32,
            src_u.as_ptr(),
            src_stride_u as i32,
            src_v.as_ptr(),
            src_stride_v as i32,
            source.0,
            source.1,
            dst_y.as_mut_ptr(),
            dst_stride_y as i32,
            dst_u.as_mut_ptr(),
            dst_stride_u as i32,
            dst_v.as_mut_ptr(),
            dst_stride_v as i32,
            width,
            height,
        )
    }
    .ok()?;
    Some(scaled)
}
