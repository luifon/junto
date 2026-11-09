use super::*;
use livekit::webrtc::video_frame::VideoBuffer;

fn layout(pixel: Pixel, matrix: Tag) -> Layout {
    Layout {
        width: 4,
        height: 2,
        stride: 8,
        pixel,
        matrix,
    }
}

#[test]
fn padded_nv12_and_i420_have_the_same_601_planes() {
    let nv12 = [
        135, 135, 135, 135, 0, 0, 0, 0, 135, 135, 135, 135, 0, 0, 0, 0, 67, 57, 67, 57, 0, 0, 0, 0,
    ];
    let i420 = [
        135, 135, 135, 135, 0, 0, 0, 0, 135, 135, 135, 135, 0, 0, 0, 0, 67, 67, 0, 0, 57, 57, 0, 0,
    ];
    for (pixel, bytes) in [(Pixel::Nv12, nv12), (Pixel::I420, i420)] {
        let frame = layout(pixel, Tag::Bt601).convert(&bytes).unwrap();
        let (y, u, v) = frame.data();
        assert_eq!(y, &[135; 8]);
        assert_eq!(u, &[67; 2]);
        assert_eq!(v, &[57; 2]);
    }
}

#[test]
fn camera_709_green_is_normalized_before_publication() {
    let bytes = [
        157, 157, 157, 157, 0, 0, 0, 0, 157, 157, 157, 157, 0, 0, 0, 0, 58, 51, 58, 51, 0, 0, 0, 0,
    ];
    let frame = layout(Pixel::Nv12, Tag::Bt709).convert(&bytes).unwrap();
    let (y, u, v) = frame.data();
    assert_eq!(y, &[135; 8]);
    assert_eq!(u, &[67; 2]);
    assert_eq!(v, &[57; 2]);
}

#[test]
fn short_planes_odd_sizes_and_oversized_frames_are_refused() {
    for pixel in [Pixel::Nv12, Pixel::I420, Pixel::Yuy2] {
        let good = layout(pixel, Tag::Missing);
        assert!(good.convert(&[0; 15]).is_err());
        for bad in [
            Layout { stride: 1, ..good },
            Layout { width: 0, ..good },
            Layout { height: 3, ..good },
            Layout {
                width: u32::MAX,
                ..good
            },
            Layout {
                stride: usize::MAX,
                ..good
            },
        ] {
            assert!(bad.convert(&[0; 32]).is_err());
        }
    }
}

#[test]
fn packed_422_averages_chroma_rows_without_mirroring() {
    let bytes = [
        30, 60, 40, 80, 50, 70, 60, 90, 70, 80, 80, 100, 90, 90, 100, 110,
    ];
    let frame = layout(Pixel::Yuy2, Tag::Bt601).convert(&bytes).unwrap();
    let (y, u, v) = frame.data();
    assert_eq!(y, &[30, 40, 50, 60, 70, 80, 90, 100]);
    assert_eq!(u, &[70, 80]);
    assert_eq!(v, &[90, 100]);
}

#[test]
fn packed_422_with_the_chroma_first_gives_the_planes_of_yuy2() {
    let yuy2 = [
        30, 60, 40, 80, 50, 70, 60, 90, 70, 80, 80, 100, 90, 90, 100, 110,
    ];
    let mut uyvy = yuy2;
    for pair in uyvy.as_chunks_mut::<2>().0 {
        pair.swap(0, 1);
    }
    let expected = layout(Pixel::Yuy2, Tag::Bt601).convert(&yuy2).unwrap();
    let frame = layout(Pixel::Uyvy, Tag::Bt601).convert(&uyvy).unwrap();
    assert_eq!(frame.data(), expected.data());
}

/// Two rows of four pixels as blue, green, red and an optional fourth byte, with `pad`
/// bytes after each row.
fn rgb_rows(rows: [[(u8, u8, u8); 4]; 2], size: usize, pad: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    for row in rows {
        for (r, g, b) in row {
            bytes.extend_from_slice(&[b, g, r, 0xff][..size]);
        }
        bytes.extend(std::iter::repeat_n(0, pad));
    }
    bytes
}

#[test]
fn rgb_pictures_are_converted_with_the_601_matrix_in_either_row_order() {
    const WHITE: (u8, u8, u8) = (255, 255, 255);
    const BLACK: (u8, u8, u8) = (0, 0, 0);
    const RED: (u8, u8, u8) = (255, 0, 0);
    let top = [WHITE, WHITE, RED, RED];
    let bottom = [BLACK, BLACK, RED, RED];
    for (size, pad, pixel) in [
        (3, 0, Pixel::Bgr24 { bottom_up: false }),
        (4, 0, Pixel::Bgrx { bottom_up: false }),
        (3, 4, Pixel::Bgr24 { bottom_up: false }),
    ] {
        let layout = Layout {
            width: 4,
            height: 2,
            stride: 4 * size + pad,
            pixel,
            matrix: Tag::Bt709,
        };
        let frame = layout.convert(&rgb_rows([top, bottom], size, pad)).unwrap();
        let (y, u, v) = frame.data();
        assert_eq!(y, &[235, 235, 82, 82, 16, 16, 82, 82], "{pixel:?}");
        // The left block averages white and black to grey; the right block is red.
        assert_eq!(u, &[128, 90], "{pixel:?}");
        assert_eq!(v, &[128, 240], "{pixel:?}");
    }
    // The rows of a bottom-up bitmap are stored from the last row of the picture.
    for (size, pixel) in [
        (3, Pixel::Bgr24 { bottom_up: true }),
        (4, Pixel::Bgrx { bottom_up: true }),
    ] {
        let layout = Layout {
            width: 4,
            height: 2,
            stride: 4 * size,
            pixel,
            matrix: Tag::Missing,
        };
        let frame = layout.convert(&rgb_rows([bottom, top], size, 0)).unwrap();
        assert_eq!(frame.data().0, &[235, 235, 82, 82, 16, 16, 82, 82]);
    }
}

#[test]
fn a_picture_larger_than_the_published_size_is_scaled_to_it() {
    let (width, height) = (1920u32, 1080u32);
    let stride = width as usize;
    let mut bytes = vec![0u8; stride * height as usize * 3 / 2];
    let (luma, chroma) = bytes.split_at_mut(stride * height as usize);
    // The left half is bright and the right half dark; the chroma is grey.
    for row in luma.chunks_exact_mut(stride) {
        row[..stride / 2].fill(200);
        row[stride / 2..].fill(40);
    }
    chroma.fill(128);
    let layout = Layout {
        width,
        height,
        stride,
        pixel: Pixel::Nv12,
        matrix: Tag::Bt601,
    };
    assert_eq!(layout.published(), (1280, 720));
    let frame = layout.convert(&bytes).unwrap();
    assert_eq!((frame.width(), frame.height()), (1280, 720));
    let (y, _, _) = frame.data();
    let stride_y = frame.strides().0 as usize;
    assert_eq!(y[360 * stride_y + 320], 200);
    assert_eq!(y[360 * stride_y + 960], 40);
    // A picture of the published size or smaller keeps its size.
    let small = Layout {
        width: 640,
        height: 480,
        stride: 640,
        ..layout
    };
    let frame = small.convert(&vec![128u8; 640 * 480 * 3 / 2]).unwrap();
    assert_eq!((frame.width(), frame.height()), (640, 480));
}

#[test]
fn a_picture_above_the_largest_size_is_refused() {
    let layout = |width: u32, height: u32| Layout {
        width,
        height,
        stride: width as usize * 2,
        pixel: Pixel::Yuy2,
        matrix: Tag::Missing,
    };
    assert!(layout(1920, 1080).length().is_some());
    assert!(layout(3840, 2160).length().is_some());
    assert!(layout(4096, 2304).length().is_none());
    assert!(layout(8192, 2).length().is_none());
}
